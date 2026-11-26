//! Príkazový riadok balíka `formaty` – prevod, rozbor a meranie (aj pre nezávislé overenie v CI).
//!
//! - `formaty info <súbor>…` – formát, rozmery, kanály, bity
//! - `formaty prevod <vstup> <výstup>` – prevod podľa prípony výstupu (`.surove` = surové vzorky na porovnanie)
//! - `formaty metadata <súbor>` – položky Exif, IPTC a veľkosť XMP (riadky oddelené tabulátorom)
//! - `formaty bezstratovo <vstup.jpg> <výstup.jpg> <ziadna|h|v|transp|transv|90|180|270|exif> [x,y,šírka,výška]`
//!   – bezstratové otočenie a orezanie JPEG v oblasti DCT
//! - `formaty zlib-zbal <úroveň> <vstup> <výstup>`, `formaty zlib-rozbal <vstup> <výstup>`
//!
//! Formát `.surove`: riadok „šírka výška kanály typ“ (typ u8, u16 alebo f32) a vzorky little-endian.
use orchidea_formaty::{Format, Limity, Obraz, Vzorky, citaj, deflate, exr, jpeg, zapis};
use std::process::ExitCode;

fn surove(o: &Obraz) -> Vec<u8> {
    let typ = match o.vzorky {
        Vzorky::U8(_) => "u8",
        Vzorky::U16(_) => "u16",
        Vzorky::F32(_) => "f32",
    };
    let mut d = format!("{} {} {} {typ}\n", o.sirka, o.vyska, o.kanaly()).into_bytes();
    match &o.vzorky {
        Vzorky::U8(v) => d.extend_from_slice(v),
        Vzorky::U16(v) => v.iter().for_each(|x| d.extend_from_slice(&x.to_le_bytes())),
        Vzorky::F32(v) => v.iter().for_each(|x| d.extend_from_slice(&x.to_le_bytes())),
    }
    d
}

fn citaj_subor(cesta: &str) -> Result<Obraz, String> {
    let d = std::fs::read(cesta).map_err(|e| format!("{cesta}: {e}"))?;
    citaj(&d, &Limity::default()).map_err(|e| format!("{cesta}: {e}"))
}

fn spusti(a: &[String]) -> Result<(), String> {
    match a.get(1).map(String::as_str) {
        Some("info") => {
            for c in &a[2..] {
                let o = citaj_subor(c)?;
                println!(
                    "{c}: {}×{} {:?} alfa={} kanály={:?} bitov={} icc={} B",
                    o.sirka,
                    o.vyska,
                    o.model,
                    o.alfa,
                    o.mena_kanalov,
                    o.bitov,
                    o.farba.icc.as_ref().map_or(0, |i| i.len())
                );
            }
            Ok(())
        }
        Some("prevod") if a.len() == 4 => {
            let o = citaj_subor(&a[2])?;
            let pripona = a[3].rsplit('.').next().unwrap_or("");
            let d = if pripona == "surove" {
                surove(&o)
            } else {
                let f = Format::z_pripony(pripona).ok_or_else(|| format!("neznáma prípona {pripona}"))?;
                match (f, std::env::var("ORCHIDEA_JPEG")) {
                    // nastavenie kodéra JPEG pre skúšky: „kvalita[,p]“
                    (Format::Jpeg, Ok(n)) => {
                        let mut casti = n.split(',');
                        let kvalita = casti.next().and_then(|k| k.parse().ok()).unwrap_or(90);
                        let progresivny = casti.next() == Some("p");
                        let nast = jpeg::NastavenieJpeg { kvalita, progresivny, ..Default::default() };
                        jpeg::zapis(&o, &nast).map_err(|e| e.to_string())?
                    }
                    _ => match (f, std::env::var("ORCHIDEA_EXR")) {
                        // nastavenie zápisu OpenEXR pre skúšky: „zip|zips|rle|none,half|float“
                        (Format::Exr, Ok(n)) => {
                            let kompresia = match n.split(',').next() {
                                Some("none") => exr::Kompresia::Ziadna,
                                Some("rle") => exr::Kompresia::Rle,
                                Some("zips") => exr::Kompresia::Zips,
                                _ => exr::Kompresia::Zip,
                            };
                            let nast = exr::NastavenieExr { polovicna: !n.ends_with("float"), kompresia };
                            exr::zapis(&o, &nast).map_err(|e| e.to_string())?
                        }
                        _ => zapis(&o, f).map_err(|e| e.to_string())?,
                    },
                }
            };
            std::fs::write(&a[3], d).map_err(|e| e.to_string())
        }
        Some("metadata") if a.len() == 3 => {
            let o = citaj_subor(&a[2])?;
            if let Some(e) = &o.metadata.exif {
                for (skupina, meno, znacka, h) in orchidea_formaty::exif::polozky(e) {
                    println!("EXIF\t{skupina}\t{meno}\t{znacka}\t{}", h.text());
                }
            }
            if let Some(i) = &o.metadata.iptc {
                for (meno, t) in orchidea_formaty::exif::polozky_iptc(i) {
                    println!("IPTC\t{meno}\t{t}");
                }
            }
            if let Some(x) = &o.metadata.xmp {
                println!("XMP\t{}", x.len());
            }
            Ok(())
        }
        Some("bezstratovo") if a.len() >= 5 => {
            use orchidea_formaty::jpeg::bezstratovo::{Transformacia as T, Vyrez, uprav};
            let d = std::fs::read(&a[2]).map_err(|e| e.to_string())?;
            let (t, exif) = match a[4].as_str() {
                "ziadna" => (T::Ziadna, false),
                "h" => (T::ZrkadloVodorovne, false),
                "v" => (T::ZrkadloZvisle, false),
                "transp" => (T::Transpozicia, false),
                "transv" => (T::Transverzia, false),
                "90" => (T::Otoc90, false),
                "180" => (T::Otoc180, false),
                "270" => (T::Otoc270, false),
                "exif" => {
                    let n = orchidea_formaty::citaj(&d, &Limity::default()).map_err(|e| e.to_string())?.metadata.natocenie;
                    (T::z_natocenia(n), true)
                }
                x => return Err(format!("neznáma transformácia {x}")),
            };
            let vyrez = a.get(5).map(|v| {
                let c: Vec<usize> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                Vyrez { x: c[0], y: c[1], sirka: c[2], vyska: c[3] }
            });
            let von = uprav(&d, t, vyrez, exif, &Limity::default()).map_err(|e| e.to_string())?;
            std::fs::write(&a[3], von).map_err(|e| e.to_string())
        }
        Some("zlib-zbal") if a.len() == 5 => {
            let d = std::fs::read(&a[3]).map_err(|e| e.to_string())?;
            std::fs::write(&a[4], deflate::zlib_zbal(&d, a[2].parse().unwrap_or(6))).map_err(|e| e.to_string())
        }
        Some("zlib-rozbal") if a.len() == 4 => {
            let d = std::fs::read(&a[2]).map_err(|e| e.to_string())?;
            let r = deflate::zlib_rozbal(&d, 1 << 31).map_err(|e| e.to_string())?;
            std::fs::write(&a[3], r).map_err(|e| e.to_string())
        }
        _ => Err("použitie: formaty info <súbor>… | prevod <vstup> <výstup> | zlib-zbal <úroveň> <vstup> <výstup> \
                  | zlib-rozbal <vstup> <výstup>"
            .into()),
    }
}

fn main() -> ExitCode {
    let a: Vec<String> = std::env::args().collect();
    match spusti(&a) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("chyba: {e}");
            ExitCode::FAILURE
        }
    }
}
