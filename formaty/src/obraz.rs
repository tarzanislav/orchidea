//! Obraz (rozbalený obrázok) – spoločný popis pre všetky formáty: rozmery, kanály, vzorky, farba, metadáta.
//!
//! Vzorky sú po riadkoch, kanály za sebou (napríklad R, G, B, A). Celočíselné vzorky sú v celom rozsahu typu
//! (8 bitov 0–255, 16 bitov 0–65 535; formát s menej bitmi sa rozšíri, skutočný počet bitov je v `bitov`),
//! plávajúce sú v jednotkách formátu (OpenEXR: lineárne svetlo scény, 1,0 = biela; môžu byť aj nad 1).

use orchidea_dlazdice::Rastr;
use orchidea_jadro::signal::PrvokSignalu;
use orchidea_jadro::{Chyba, Vysledok};
use orchidea_zariadenie::ProfilHardveru;

/// Vzorky obrazu.
#[derive(Debug, Clone, PartialEq)]
pub enum Vzorky {
    U8(Vec<u8>),
    U16(Vec<u16>),
    F32(Vec<f32>),
}

impl Vzorky {
    pub fn dlzka(&self) -> usize {
        match self {
            Vzorky::U8(v) => v.len(),
            Vzorky::U16(v) => v.len(),
            Vzorky::F32(v) => v.len(),
        }
    }

    /// Bajtov na jednu vzorku.
    pub fn bajtov_na_vzorku(&self) -> usize {
        match self {
            Vzorky::U8(_) => 1,
            Vzorky::U16(_) => 2,
            Vzorky::F32(_) => 4,
        }
    }

    /// Hodnota vzorky `i` prevedená na rozsah 0–1 (celé čísla) alebo bez zmeny (plávajúce).
    pub fn normovana(&self, i: usize) -> f32 {
        match self {
            Vzorky::U8(v) => v[i] as f32 / 255.0,
            Vzorky::U16(v) => v[i] as f32 / 65_535.0,
            Vzorky::F32(v) => v[i],
        }
    }
}

/// Farebný model kanálov.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    /// Jeden kanál jasu (odtiene sivej).
    Siva,
    /// Červená, zelená, modrá.
    Rgb,
    /// Azúrová, purpurová, žltá, čierna (tlač); hodnota 0 = bez farby.
    Cmyk,
    /// Iné kanály (napríklad hĺbka, vrstvy OpenEXR) – mená sú v `mena_kanalov`.
    Iny,
}

/// Kódovanie farby podľa ITU-T H.273 (kúsok cICP v PNG, údaje JPEG XL a videa).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cicp {
    /// Základné farby: 1 = BT.709/sRGB, 9 = BT.2020, 12 = Display P3 …
    pub zakladne: u8,
    /// Prenosová funkcia: 13 = sRGB, 8 = lineárna, 16 = PQ, 18 = HLG …
    pub prenos: u8,
    /// Matica (0 = RGB).
    pub matica: u8,
    pub plny_rozsah: bool,
}

/// Popis farby obrázka – čo je v súbore. Prevod medzi priestormi robí balík `farba`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Farba {
    /// Profil ICC (úplné bajty profilu).
    pub icc: Option<Vec<u8>>,
    pub cicp: Option<Cicp>,
    /// Gama súboru z PNG (napríklad 0,45455 = obrátená 2,2).
    pub gama: Option<f64>,
    /// Chromatickosti: biela x, y; červená x, y; zelená x, y; modrá x, y.
    pub chromaticity: Option<[f64; 8]>,
    /// Súbor sa hlási k sRGB (hodnota = zámer vykreslenia 0–3).
    pub srgb: Option<u8>,
    /// Vzorky sú v lineárnom svetle (OpenEXR, plávajúci TIFF).
    pub linearne: bool,
}

/// Metadáta obrázka.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata {
    /// Údaje Exif (blok TIFF začínajúci „II“ alebo „MM“, bez hlavičky „Exif\0\0“).
    pub exif: Option<Vec<u8>>,
    /// Údaje XMP (XML).
    pub xmp: Option<Vec<u8>>,
    /// Údaje IPTC-IIM (množiny 0x1C …), ako ich uložil Photoshop (JPEG APP13, TIFF značka 33723).
    pub iptc: Option<Vec<u8>>,
    /// Textové položky (kľúč, hodnota).
    pub texty: Vec<(String, String)>,
    /// Natočenie podľa Exif 1–8 (1 = bez zmeny).
    pub natocenie: u8,
    /// Rozlíšenie v bodoch na palec (vodorovne, zvislo), ak ho súbor uvádza.
    pub dpi: Option<(f64, f64)>,
}

/// Rozbalený obrázok.
#[derive(Debug, Clone, PartialEq)]
pub struct Obraz {
    pub sirka: usize,
    pub vyska: usize,
    pub model: Model,
    /// Posledný kanál je priehľadnosť (alfa), nenásobená farbou.
    pub alfa: bool,
    /// Mená kanálov (napríklad „R“, „G“, „B“, „A“; vrstvy OpenEXR „diffuse.R“).
    pub mena_kanalov: Vec<String>,
    pub vzorky: Vzorky,
    /// Skutočný počet bitov formátu (1–16 pre celé, 16 pre polovičnú presnosť, 32 pre plávajúce).
    pub bitov: u8,
    pub farba: Farba,
    pub metadata: Metadata,
}

impl Obraz {
    /// Nový obrázok s bežnými menami kanálov podľa modelu.
    pub fn novy(sirka: usize, vyska: usize, model: Model, alfa: bool, vzorky: Vzorky) -> Vysledok<Obraz> {
        let mut mena: Vec<String> = match model {
            Model::Siva => vec!["Y".into()],
            Model::Rgb => vec!["R".into(), "G".into(), "B".into()],
            Model::Cmyk => vec!["C".into(), "M".into(), "Y".into(), "K".into()],
            Model::Iny => Vec::new(),
        };
        if alfa {
            mena.push("A".into());
        }
        let bitov = match &vzorky {
            Vzorky::U8(_) => 8,
            Vzorky::U16(_) => 16,
            Vzorky::F32(_) => 32,
        };
        let o = Obraz {
            sirka,
            vyska,
            model,
            alfa,
            mena_kanalov: mena,
            vzorky,
            bitov,
            farba: Farba::default(),
            metadata: Metadata { natocenie: 1, ..Default::default() },
        };
        o.over()?;
        Ok(o)
    }

    pub fn kanaly(&self) -> usize {
        self.mena_kanalov.len()
    }

    /// Overí, že počet vzoriek sedí s rozmermi a kanálmi.
    pub fn over(&self) -> Vysledok<()> {
        let n = self.sirka.checked_mul(self.vyska).and_then(|p| p.checked_mul(self.kanaly()));
        if self.sirka == 0 || self.vyska == 0 || self.kanaly() == 0 || n != Some(self.vzorky.dlzka()) {
            return Err(Chyba::NezhodaTvaru(format!(
                "obraz {}×{}×{} má {} vzoriek",
                self.sirka,
                self.vyska,
                self.kanaly(),
                self.vzorky.dlzka()
            )));
        }
        Ok(())
    }

    /// Obraz na dlaždiciach (R70). Vzorky sa prevedú na typ `T`: celé čísla medzi sebou podľa rozsahu
    /// (8 → 16 bitov ×257), celé na plávajúce do 0–1, plávajúce na celé orezaním a zaokrúhlením.
    pub fn do_rastra<T: PrvokSignalu>(&self, strana: usize) -> Vysledok<Rastr<T>> {
        let mena: Vec<&str> = self.mena_kanalov.iter().map(String::as_str).collect();
        let ciel = T::TYP;
        use orchidea_jadro::TypPrvku as Tp;
        let data: Vec<T> = match (&self.vzorky, ciel) {
            (Vzorky::U8(v), Tp::U8) => v.iter().map(|&x| T::z_f64(x as f64).0).collect(),
            (Vzorky::U16(v), Tp::U16) => v.iter().map(|&x| T::z_f64(x as f64).0).collect(),
            (Vzorky::F32(v), Tp::F32) => v.iter().map(|&x| T::z_f64(x as f64).0).collect(),
            (Vzorky::U8(v), Tp::U16) => v.iter().map(|&x| T::z_f64(x as f64 * 257.0).0).collect(),
            _ => {
                let max = match ciel {
                    Tp::U8 => 255.0,
                    Tp::U16 => 65_535.0,
                    Tp::I8 => 127.0,
                    Tp::I16 => 32_767.0,
                    Tp::I32 => 2_147_483_647.0,
                    Tp::F32 | Tp::F64 => 1.0,
                };
                (0..self.vzorky.dlzka()).map(|i| T::z_f64(self.vzorky.normovana(i) as f64 * max).0).collect()
            }
        };
        Rastr::z_pixelov(self.sirka, self.vyska, &mena, strana, &data)
    }

    /// Obraz z rastra s vzorkami `u8`, `u16` alebo `f32`.
    pub fn z_rastra<T: PrvokSignalu>(rastr: &Rastr<T>, model: Model, alfa: bool) -> Vysledok<Obraz> {
        let s = rastr.do_signalu(rastr.cely())?;
        use orchidea_jadro::TypPrvku as Tp;
        let vzorky = match T::TYP {
            Tp::U8 => Vzorky::U8(s.hodnoty::<u8>().map(|h| h.to_vec()).unwrap_or_default()),
            Tp::U16 => Vzorky::U16(s.hodnoty::<u16>().map(|h| h.to_vec()).unwrap_or_default()),
            Tp::F32 => Vzorky::F32(s.hodnoty::<f32>().map(|h| h.to_vec()).unwrap_or_default()),
            t => {
                return Err(Chyba::NezhodaTypov {
                    popis: format!("obraz z rastra {}", t.meno()),
                    navrh: None,
                });
            }
        };
        let mut o = Obraz::novy(rastr.sirka(), rastr.vyska(), model, alfa, vzorky)?;
        if let Some(mena) = rastr.os_kanalov().mena_vzoriek()
            && mena.len() == o.kanaly()
        {
            o.mena_kanalov = mena.iter().map(|m| m.to_string()).collect();
        }
        Ok(o)
    }
}

/// Prevod násobenej alfy (farba × alfa) na nenásobenú.
pub fn rozdel_alfu(o: &mut Obraz) {
    let k = o.kanaly();
    match &mut o.vzorky {
        Vzorky::U8(v) => {
            for p in v.chunks_exact_mut(k) {
                let a = p[k - 1] as u32;
                for x in &mut p[..k - 1] {
                    if let Some(v) = (*x as u32 * 255 + a / 2).checked_div(a) {
                        *x = v.min(255) as u8;
                    }
                }
            }
        }
        Vzorky::U16(v) => {
            for p in v.chunks_exact_mut(k) {
                let a = p[k - 1] as u32;
                for x in &mut p[..k - 1] {
                    if let Some(v) = (*x as u32 * 65_535 + a / 2).checked_div(a) {
                        *x = v.min(65_535) as u16;
                    }
                }
            }
        }
        Vzorky::F32(v) => {
            for p in v.chunks_exact_mut(k) {
                let a = p[k - 1];
                if a > 0.0 {
                    for x in &mut p[..k - 1] {
                        *x /= a;
                    }
                }
            }
        }
    }
}

/// Popis obrázka bez bodov (rozmery, hĺbka, farba, metadáta) – pre katalóg a rýchle prehľady.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Popis {
    pub sirka: usize,
    pub vyska: usize,
    pub bitov: u8,
    pub kanalov: usize,
    pub farba: Farba,
    pub metadata: Metadata,
}

/// Bezpečnostné limity čítačiek (R211). Súbor, ktorý by ich prekročil, skončí chybou `Limit` – nikdy nie
/// vyčerpaním pamäte alebo pádom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limity {
    /// Najviac pixelov obrázka (šírka × výška).
    pub pixelov: u64,
    /// Najviac bajtov pamäte na rozbalené vzorky.
    pub bajtov: u64,
    /// Najviac bajtov metadát (ICC, Exif, XMP, texty) spolu.
    pub metadat: u64,
}

impl Default for Limity {
    fn default() -> Self {
        Limity { pixelov: 1 << 28, bajtov: 2 << 30, metadat: 64 << 20 }
    }
}

impl Limity {
    /// Limity podľa zariadenia: rozbalené vzorky smú zabrať najviac polovicu pamäte (Raspberry Pi 3 s 1 GB
    /// teda ≈ 500 MB – fotka 100 Mpx v 8 bitoch RGB).
    pub fn pre_profil(profil: &ProfilHardveru) -> Limity {
        let pamat = if profil.pamat_kb > 0 { profil.pamat_kb * 1024 } else { 4 << 30 };
        Limity { bajtov: (pamat / 2).max(64 << 20), ..Limity::default() }
    }

    /// Overí rozmery a pamäť pred rozbalením (`bajtov_na_pixel` všetkých kanálov spolu).
    pub fn over(&self, sirka: u64, vyska: u64, bajtov_na_pixel: u64) -> Vysledok<()> {
        if sirka == 0 || vyska == 0 {
            return Err(Chyba::PoskodeneData(format!("nulový rozmer obrázka {sirka}×{vyska}")));
        }
        let px = sirka.saturating_mul(vyska);
        if px > self.pixelov {
            return Err(Chyba::Limit(format!("{sirka}×{vyska} = {px} pixelov je viac ako {}", self.pixelov)));
        }
        let b = px.saturating_mul(bajtov_na_pixel);
        if b > self.bajtov {
            return Err(Chyba::Limit(format!("obrázok by zabral {b} B pamäte, dovolené {}", self.bajtov)));
        }
        Ok(())
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn rastr_tam_a_spat() {
        let data: Vec<u16> = (0..37 * 23 * 4).map(|i| (i * 977 % 65_536) as u16).collect();
        let o = Obraz::novy(37, 23, Model::Rgb, true, Vzorky::U16(data)).unwrap();
        let r = o.do_rastra::<u16>(16).unwrap();
        assert_eq!(r.mriezka(), (3, 2));
        let spat = Obraz::z_rastra(&r, Model::Rgb, true).unwrap();
        assert_eq!(spat, o);
        // 16 bitov do plávajúcich 0–1
        let f = o.do_rastra::<f32>(16).unwrap();
        assert_eq!(f.pixel(1, 0)[0], (4.0 * 977.0) / 65_535.0f32);
    }

    #[test]
    fn limity() {
        let l = Limity { pixelov: 100, bajtov: 300, metadat: 0 };
        assert!(l.over(10, 10, 3).is_ok());
        assert!(matches!(l.over(10, 11, 1), Err(Chyba::Limit(_))));
        assert!(matches!(l.over(10, 10, 4), Err(Chyba::Limit(_))));
        assert!(l.over(0, 10, 1).is_err());
        assert!(Obraz::novy(2, 2, Model::Rgb, false, Vzorky::U8(vec![0; 11])).is_err());
    }
}
