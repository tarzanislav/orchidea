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
}
