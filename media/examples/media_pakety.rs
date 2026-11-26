//! Výpis stôp a paketov kontajnera pre nezávislého rozhodcu v CI (`media/nastroje/over_media.py`).
//!
//! Použitie: `media_pakety <súbor>` – na stdout riadky v pevnom tvare:
//! ```text
//! STOPA index=<i> cislo=<n> druh=<Video|Zvuk|Titulky|Ine> kodek=<meno>
//! PAKET stopa=<i> pts=<tiky|-> trvanie=<tiky|-> kluc=<0|1> velkost=<B> orezat=<vzoriek na konci> md5=<32 hex>
//! VYSLEDOK paketov=<n> citani=<n> bajtov=<n>
//! ```
//! Časy sú v tikoch stopy (pri Matroske `TimestampScale`). Návratový kód 0 ok, 1 chyba čítania, 2 zlé argumenty.

use orchidea_hevc::odtlacok::Md5;
use orchidea_media::mkv::Mkv;
use orchidea_media::{Kontajner, Limity, SuborovyZdroj};

fn main() {
    let Some(cesta) = std::env::args().nth(1) else {
        eprintln!("použitie: media_pakety <súbor>");
        std::process::exit(2);
    };
    let zdroj = match SuborovyZdroj::otvor(&cesta) {
        Ok(z) => z,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let mut m = match Mkv::otvor(zdroj, Limity::default()) {
        Ok(m) => m,
        Err(e) => {
            println!("CHYBA {e}");
            std::process::exit(1);
        }
    };
    for (i, s) in m.stopy().iter().enumerate() {
        println!(
            "STOPA index={i} cislo={} druh={:?} kodek={}",
            s.cislo,
            s.druh,
            s.kodek.meno().replace(' ', "_")
        );
    }
    let cas = |t: Option<i64>| t.map_or("-".to_string(), |t| t.to_string());
    let mut n = 0u64;
    loop {
        match m.dalsi_paket() {
            Ok(Some(p)) => {
                n += 1;
                let mut h = Md5::novy();
                h.pridaj(&p.data);
                let md5: String = h.vysledok().iter().map(|b| format!("{b:02x}")).collect();
                println!(
                    "PAKET stopa={} pts={} trvanie={} kluc={} velkost={} orezat={} md5={md5}",
                    p.stopa,
                    cas(p.pts),
                    cas(p.trvanie),
                    p.klucovy as u8,
                    p.data.len(),
                    p.orezat_koniec
                );
            }
            Ok(None) => break,
            Err(e) => {
                println!("CHYBA {e}");
                std::process::exit(1);
            }
        }
    }
    let (citani, bajtov) = m.citac().statistika();
    println!("VYSLEDOK paketov={n} citani={citani} bajtov={bajtov}");
}
