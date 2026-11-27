//! **Zdroj bajtov** – čítanie na pozícii bez vnútorného kurzora (súbor, pamäť).
//!
//! Čítačka kontajnera sa pýta „daj mi bajty od pozície P“ – rovnako pre súbor na disku, USB kľúč v ORCHIDEA OS
//! aj údaje v pamäti (skúšky, sieť). Bez kurzora je zdroj použiteľný cez `&self` aj z viacerých miest naraz.
//! Čítanie dopredu po dávkach (aby úložisko medzi dávkami spalo, R306) pribudne v etape L1 nad týmto rozhraním.

use orchidea_jadro::{Chyba, Vysledok};
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

/// Zdroj bajtov s náhodným prístupom.
pub trait ZdrojBajtov: std::fmt::Debug {
    /// Dĺžka v bajtoch.
    fn dlzka(&self) -> u64;

    /// Prečíta najviac `ciel.len()` bajtov od `pozicia`; vráti počet prečítaných (0 = koniec alebo za koncom).
    fn citaj_na(&self, pozicia: u64, ciel: &mut [u8]) -> Vysledok<usize>;

    /// Prečíta presne `ciel.len()` bajtov od `pozicia`, inak chyba (skrátený súbor).
    fn citaj_presne(&self, pozicia: u64, ciel: &mut [u8]) -> Vysledok<()> {
        let mut hotovo = 0;
        while hotovo < ciel.len() {
            let n = self.citaj_na(pozicia + hotovo as u64, &mut ciel[hotovo..])?;
            if n == 0 {
                return Err(Chyba::PoskodeneData(format!(
                    "súbor končí: z {} B od pozície {pozicia} prečítaných len {hotovo} B",
                    ciel.len()
                )));
            }
            hotovo += n;
        }
        Ok(())
    }
}

/// Údaje v pamäti (skúšky, malé súbory, údaje zo siete). Kópia je lacná – údaje sú zdieľané.
#[derive(Debug, Clone)]
pub struct PametovyZdroj {
    data: Arc<[u8]>,
}

impl PametovyZdroj {
    pub fn novy(data: impl Into<Arc<[u8]>>) -> PametovyZdroj {
        PametovyZdroj { data: data.into() }
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

/// Zdieľaný zdroj – ten istý súbor pre viac pokusov o otvorenie (rozpoznanie formátu bez druhého otvorenia).
impl<Z: ZdrojBajtov + ?Sized> ZdrojBajtov for std::sync::Arc<Z> {
    fn dlzka(&self) -> u64 {
        (**self).dlzka()
    }
    fn citaj_na(&self, pozicia: u64, ciel: &mut [u8]) -> Vysledok<usize> {
        (**self).citaj_na(pozicia, ciel)
    }
}

impl ZdrojBajtov for PametovyZdroj {
    fn dlzka(&self) -> u64 {
        self.data.len() as u64
    }

    fn citaj_na(&self, pozicia: u64, ciel: &mut [u8]) -> Vysledok<usize> {
        let Ok(p) = usize::try_from(pozicia) else { return Ok(0) };
        if p >= self.data.len() {
            return Ok(0);
        }
        let n = ciel.len().min(self.data.len() - p);
        ciel[..n].copy_from_slice(&self.data[p..p + n]);
        Ok(n)
    }
}

/// Súbor na disku. Dĺžka sa zistí pri otvorení (súbor, do ktorého sa práve zapisuje, sa číta po túto dĺžku).
#[derive(Debug)]
pub struct SuborovyZdroj {
    subor: Subor,
    dlzka: u64,
}

#[cfg(any(unix, windows))]
type Subor = File;
#[cfg(not(any(unix, windows)))]
type Subor = std::sync::Mutex<File>;

impl SuborovyZdroj {
    pub fn otvor(cesta: impl AsRef<Path>) -> Vysledok<SuborovyZdroj> {
        let cesta = cesta.as_ref();
        let chyba = |e: std::io::Error| Chyba::VstupVystup(format!("{}: {e}", cesta.display()));
        let subor = File::open(cesta).map_err(chyba)?;
        let dlzka = subor.metadata().map_err(chyba)?.len();
        #[cfg(not(any(unix, windows)))]
        let subor = std::sync::Mutex::new(subor);
        Ok(SuborovyZdroj { subor, dlzka })
    }
}

impl ZdrojBajtov for SuborovyZdroj {
    fn dlzka(&self) -> u64 {
        self.dlzka
    }

    fn citaj_na(&self, pozicia: u64, ciel: &mut [u8]) -> Vysledok<usize> {
        if pozicia >= self.dlzka {
            return Ok(0);
        }
        let n = ciel.len().min(usize::try_from(self.dlzka - pozicia).unwrap_or(usize::MAX));
        let ciel = &mut ciel[..n];
        let chyba = |e: std::io::Error| Chyba::VstupVystup(format!("čítanie na pozícii {pozicia}: {e}"));
        loop {
            #[cfg(unix)]
            let r = std::os::unix::fs::FileExt::read_at(&self.subor, ciel, pozicia);
            #[cfg(windows)]
            let r = std::os::windows::fs::FileExt::seek_read(&self.subor, ciel, pozicia);
            #[cfg(not(any(unix, windows)))]
            let r = {
                use std::io::{Read, Seek, SeekFrom};
                let mut s = self.subor.lock().map_err(|_| Chyba::VstupVystup("zámok súboru".into()))?;
                s.seek(SeekFrom::Start(pozicia)).and_then(|_| s.read(ciel))
            };
            match r {
                Ok(n) => return Ok(n),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(chyba(e)),
            }
        }
    }
}

/// **Čítač s čítaním dopredu po dávkach** (chladné čítanie, R306): údaje sa z úložiska berú vo veľkých súvislých
/// kusoch (predvolene 4 MiB) a ďalšie žiadosti sa obslúžia z pamäte – USB kľúč alebo SD karta medzi dávkami spí.
/// Pri posune dozadu sa okno načíta znova; pri posune dopredu sa prekrytá časť okna ponechá.
#[derive(Debug)]
pub struct Citac<Z> {
    zdroj: Z,
    okno: Vec<u8>,
    zaciatok: u64,
    davka: usize,
    citani: u64,
    precitanych: u64,
}

impl<Z: ZdrojBajtov> Citac<Z> {
    /// Predvolená dávka čítania – 4 MiB (pri 8 Mb/s filme ≈ 4 s, pri hudbe FLAC ≈ 30 s).
    pub const PREDVOLENA_DAVKA: usize = 4 << 20;

    pub fn novy(zdroj: Z, davka: usize) -> Citac<Z> {
        Citac { zdroj, okno: Vec::new(), zaciatok: 0, davka: davka.max(4096), citani: 0, precitanych: 0 }
    }

    pub fn dlzka(&self) -> u64 {
        self.zdroj.dlzka()
    }

    pub fn zdroj(&self) -> &Z {
        &self.zdroj
    }

    pub fn davka(&self) -> usize {
        self.davka
    }

    /// Zmení veľkosť dávky (napríklad malé dávky pri delení intervalu počas posunu).
    pub fn nastav_davku(&mut self, davka: usize) {
        self.davka = davka.max(4096);
    }

    /// Počet čítaní zo zdroja a prečítaných bajtov – meranie chladného čítania.
    pub fn statistika(&self) -> (u64, u64) {
        (self.citani, self.precitanych)
    }

    /// Najviac `n` bajtov od `pozicia` (menej len na konci zdroja).
    pub fn bajty(&mut self, pozicia: u64, n: usize) -> Vysledok<&[u8]> {
        let koniec_okna = self.zaciatok + self.okno.len() as u64;
        let dlzka = self.zdroj.dlzka();
        if pozicia >= dlzka {
            return Ok(&[]);
        }
        let ziadane_do = pozicia.saturating_add(n as u64).min(dlzka);
        if pozicia >= self.zaciatok && ziadane_do <= koniec_okna {
            let a = (pozicia - self.zaciatok) as usize;
            let b = (ziadane_do - self.zaciatok) as usize;
            return Ok(&self.okno[a..b]);
        }
        let k = usize::try_from((n.max(self.davka) as u64).min(dlzka - pozicia)).unwrap_or(usize::MAX);
        // prekrytie: začiatok žiadosti je v okne – ponechá sa koniec okna, dočíta sa zvyšok
        let ponechat = if pozicia >= self.zaciatok && pozicia < koniec_okna {
            let a = (pozicia - self.zaciatok) as usize;
            self.okno.drain(..a);
            self.okno.len()
        } else {
            0
        };
        // ponechaná časť môže byť dlhšia ako nová dávka (dávka sa zmenšila, predošlá žiadosť bola veľká)
        let ponechat = ponechat.min(k);
        self.okno.resize(k, 0);
        self.zaciatok = pozicia;
        let mut hotovo = ponechat;
        while hotovo < k {
            let m = self.zdroj.citaj_na(pozicia + hotovo as u64, &mut self.okno[hotovo..])?;
            self.citani += 1;
            if m == 0 {
                break;
            }
            self.precitanych += m as u64;
            hotovo += m;
        }
        self.okno.truncate(hotovo);
        let b = hotovo.min(n);
        Ok(&self.okno[..b])
    }

    /// Presne `n` bajtov od `pozicia`, inak chyba (súbor je skrátený).
    pub fn presne(&mut self, pozicia: u64, n: usize) -> Vysledok<&[u8]> {
        let d = self.bajty(pozicia, n)?;
        if d.len() < n {
            return Err(Chyba::PoskodeneData(format!(
                "súbor končí: od pozície {pozicia} chýba {} B",
                n - d.len()
            )));
        }
        Ok(d)
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn pamat_na_poziciach() {
        let z = PametovyZdroj::novy((0u8..100).collect::<Vec<_>>());
        let mut b = [0u8; 8];
        assert_eq!(z.citaj_na(10, &mut b).unwrap(), 8);
        assert_eq!(b, [10, 11, 12, 13, 14, 15, 16, 17]);
        assert_eq!(z.citaj_na(96, &mut b).unwrap(), 4);
        assert_eq!(z.citaj_na(100, &mut b).unwrap(), 0);
        assert_eq!(z.citaj_na(u64::MAX, &mut b).unwrap(), 0);
        assert!(z.citaj_presne(95, &mut b).is_err());
        assert!(z.citaj_presne(92, &mut b).is_ok());
    }

    #[test]
    fn subor_na_poziciach() {
        // vo WebAssembly (WASI) je prístupný len pracovný priečinok (`wasmtime run --dir=.`)
        let priecinok =
            if cfg!(target_family = "wasm") { std::path::PathBuf::from(".") } else { std::env::temp_dir() };
        // `std::process::id` vo WASI nie je – meno podľa času
        let cas = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let cesta = priecinok.join(format!("orchidea-media-zdroj-{cas}.bin"));
        std::fs::write(&cesta, (0u8..=255).cycle().take(5000).collect::<Vec<_>>()).unwrap();
        let z = SuborovyZdroj::otvor(&cesta).unwrap();
        assert_eq!(z.dlzka(), 5000);
        let mut b = [0u8; 4];
        z.citaj_presne(4096, &mut b).unwrap();
        assert_eq!(b, [0, 1, 2, 3]);
        // poradie čítaní nič nemení – zdroj nemá kurzor
        z.citaj_presne(1, &mut b).unwrap();
        assert_eq!(b, [1, 2, 3, 4]);
        assert_eq!(z.citaj_na(4998, &mut b).unwrap(), 2);
        assert_eq!(z.citaj_na(5000, &mut b).unwrap(), 0);
        assert!(z.citaj_presne(4999, &mut b).is_err());
        drop(z);
        let _ = std::fs::remove_file(&cesta);
        assert!(SuborovyZdroj::otvor(&cesta).is_err());
    }

    #[test]
    fn citac_po_davkach() {
        let z = PametovyZdroj::novy((0..100_000u32).map(|i| i as u8).collect::<Vec<_>>());
        let mut c = Citac::novy(z, 10_000);
        assert_eq!(c.bajty(0, 4).unwrap(), &[0, 1, 2, 3]);
        assert_eq!(c.statistika(), (1, 10_000));
        // ďalšie žiadosti v okne sa nečítajú zo zdroja
        for p in (0..9_000).step_by(500) {
            assert_eq!(c.bajty(p, 100).unwrap()[0], p as u8);
        }
        assert_eq!(c.statistika(), (1, 10_000));
        // prekrytie: ponechá sa koniec okna, dočíta sa len zvyšok
        assert_eq!(c.bajty(9_990, 20).unwrap()[19], (10_009u32 % 256) as u8);
        assert_eq!(c.statistika(), (2, 19_990));
        // veľká žiadosť zväčší okno, koniec zdroja vráti menej
        assert_eq!(c.bajty(50_000, 30_000).unwrap().len(), 30_000);
        assert_eq!(c.bajty(99_990, 100).unwrap().len(), 10);
        assert!(c.bajty(100_000, 1).unwrap().is_empty());
        assert!(c.presne(99_990, 11).is_err());
        assert_eq!(c.presne(99_990, 10).unwrap()[0], (99_990u32 % 256) as u8);
        // dozadu
        assert_eq!(c.bajty(5, 2).unwrap(), &[5, 6]);
        // ponechaný koniec okna dlhší ako nová (menšia) dávka – nájdené fuzzerom
        let z = PametovyZdroj::novy((0..100_000u32).map(|i| i as u8).collect::<Vec<_>>());
        let mut c = Citac::novy(z, 50_000);
        c.bajty(0, 10).unwrap();
        c.nastav_davku(4096);
        assert_eq!(c.bajty(100, 10).unwrap()[0], 100);
        assert_eq!(c.bajty(60_000, 10).unwrap()[0], (60_000u32 % 256) as u8);
        assert_eq!(c.bajty(99_990, 20).unwrap().len(), 10);
        // za koncom zdroja, keď okno siaha po koniec – nájdené fuzzerom
        assert!(c.bajty(100_005, 3).unwrap().is_empty());
    }
}
