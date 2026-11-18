//! Denník – trvalý zápis zmien do súboru (zásada A7: nič sa nestratí, výpadok prúdu nepokazí nič).
//!
//! Súbor sa len dopĺňa. Každý záznam má dĺžku, obsah a odtlačok obsahu; po zápise sa dáta vynútia na disk.
//! Pri otvorení sa prečítajú všetky celé a neporušené záznamy; neúplný alebo poškodený koniec (napr. po
//! výpadku prúdu uprostred zápisu) sa nahlási a pred ďalším zápisom sa odreže – platné zmeny pred ním
//! ostanú nedotknuté a odrezané bajty sa najprv uložia vedľa denníka (súbor `….zvysok`), takže sa nestratí
//! ani poškodený koniec.

use crate::chyba::{Chyba, Vysledok};
use crate::kodovanie::Kodovatelne;
use crate::odtlacok::{Hasovac, Odtlacok};
use crate::zmena::Zmena;
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const HLAVICKA: &[u8; 16] = b"ORCHIDEA-DENNIK\x01";
/// Najväčší záznam (jedna zmena) – 1 GiB.
const NAJVACSI_ZAZNAM: u64 = 1 << 30;

/// Čo sa zistilo pri otvorení denníka.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpravaOtvorenia {
    pub zmien: usize,
    /// Počet bajtov poškodeného alebo neúplného konca (0 = denník je celý v poriadku).
    pub poskodeny_koniec: u64,
}

/// Denník zmien v súbore.
#[derive(Debug)]
pub struct Dennik {
    cesta: PathBuf,
    subor: File,
    zmeny: Vec<Zmena>,
    identity: BTreeSet<Odtlacok>,
    platna_dlzka: u64,
    treba_orezat: bool,
    zvysok: Vec<u8>,
}

fn odtlacok_zaznamu(obsah: &[u8]) -> Odtlacok {
    let mut h = Hasovac::novy();
    h.pridaj(b"orchidea.dennik.zaznam.1").pridaj(obsah);
    h.dokonci()
}

impl Dennik {
    /// Otvorí existujúci denník alebo vytvorí nový.
    pub fn otvor(cesta: impl AsRef<Path>) -> Vysledok<(Dennik, SpravaOtvorenia)> {
        let cesta = cesta.as_ref().to_path_buf();
        let mut subor =
            OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&cesta)?;
        let mut data = Vec::new();
        subor.read_to_end(&mut data)?;
        if data.is_empty() {
            subor.write_all(HLAVICKA)?;
            subor.sync_all()?;
            let d = Dennik {
                cesta,
                subor,
                zmeny: Vec::new(),
                identity: BTreeSet::new(),
                platna_dlzka: HLAVICKA.len() as u64,
                treba_orezat: false,
                zvysok: Vec::new(),
            };
            return Ok((d, SpravaOtvorenia::default()));
        }
        if data.len() < HLAVICKA.len() || &data[..HLAVICKA.len()] != HLAVICKA {
            return Err(Chyba::PoskodeneData(format!("{} nie je denník ORCHIDEE", cesta.display())));
        }
        let mut poloha = HLAVICKA.len();
        let mut zmeny = Vec::new();
        let mut identity = BTreeSet::new();
        loop {
            if poloha == data.len() {
                break;
            }
            let Some(zaznam) = precitaj_zaznam(&data[poloha..]) else { break };
            let (zmena, dlzka) = zaznam;
            identity.insert(zmena.id());
            zmeny.push(zmena);
            poloha += dlzka;
        }
        let sprava = SpravaOtvorenia { zmien: zmeny.len(), poskodeny_koniec: (data.len() - poloha) as u64 };
        let d = Dennik {
            cesta,
            subor,
            zmeny,
            identity,
            platna_dlzka: poloha as u64,
            treba_orezat: sprava.poskodeny_koniec > 0,
            zvysok: data[poloha..].to_vec(),
        };
        Ok((d, sprava))
    }

    pub fn cesta(&self) -> &Path {
        &self.cesta
    }

    pub fn zmeny(&self) -> &[Zmena] {
        &self.zmeny
    }

    pub fn obsahuje(&self, id: &Odtlacok) -> bool {
        self.identity.contains(id)
    }

    /// Zapíše zmenu (ak tam ešte nie je). Vráti, či bola nová.
    pub fn zapis(&mut self, z: &Zmena) -> Vysledok<bool> {
        let id = z.id();
        if self.identity.contains(&id) {
            return Ok(false);
        }
        if self.treba_orezat {
            self.uloz_zvysok()?;
            self.subor.set_len(self.platna_dlzka)?;
            self.subor.sync_all()?;
            self.treba_orezat = false;
        }
        let obsah = z.do_bajtov();
        if obsah.len() as u64 > NAJVACSI_ZAZNAM {
            return Err(Chyba::Limit("zmena je väčšia ako 1 GiB".into()));
        }
        let mut zaznam = Vec::with_capacity(obsah.len() + 40);
        zaznam.extend_from_slice(&(obsah.len() as u64).to_le_bytes());
        zaznam.extend_from_slice(&obsah);
        zaznam.extend_from_slice(&odtlacok_zaznamu(&obsah).0);
        self.subor.seek(SeekFrom::Start(self.platna_dlzka))?;
        self.subor.write_all(&zaznam)?;
        self.subor.sync_data()?;
        self.platna_dlzka += zaznam.len() as u64;
        self.identity.insert(id);
        self.zmeny.push(z.clone());
        Ok(true)
    }

    /// Uloží poškodený koniec vedľa denníka (nič sa nestratí, ani poškodené bajty).
    fn uloz_zvysok(&mut self) -> Vysledok<()> {
        if self.zvysok.is_empty() {
            return Ok(());
        }
        let zaklad = self.cesta.as_os_str().to_os_string();
        for i in 0u32..10_000 {
            let mut meno = zaklad.clone();
            meno.push(if i == 0 { ".zvysok".to_string() } else { format!(".zvysok-{i}") });
            let cesta = PathBuf::from(meno);
            match OpenOptions::new().write(true).create_new(true).open(&cesta) {
                Ok(mut f) => {
                    f.write_all(&self.zvysok)?;
                    f.sync_all()?;
                    self.zvysok.clear();
                    return Ok(());
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(Chyba::VstupVystup("nepodarilo sa uložiť poškodený koniec denníka".into()))
    }

    /// Doplní zmeny z iného zdroja (iné zariadenie, záloha). Vráti počet nových zmien.
    pub fn zluc(&mut self, zmeny: &[Zmena]) -> Vysledok<usize> {
        let mut nove = 0;
        for z in zmeny {
            if self.zapis(z)? {
                nove += 1;
            }
        }
        Ok(nove)
    }
}

/// Prečíta jeden záznam; None = neúplný alebo poškodený.
fn precitaj_zaznam(d: &[u8]) -> Option<(Zmena, usize)> {
    if d.len() < 8 {
        return None;
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[..8]);
    let n = u64::from_le_bytes(b);
    if n > NAJVACSI_ZAZNAM {
        return None;
    }
    let n = n as usize;
    let spolu = 8 + n + 32;
    if d.len() < spolu {
        return None;
    }
    let obsah = &d[8..8 + n];
    if odtlacok_zaznamu(obsah).0 != d[8 + n..spolu] {
        return None;
    }
    let zmena = Zmena::z_bajtov(obsah).ok()?;
    Some((zmena, spolu))
}
