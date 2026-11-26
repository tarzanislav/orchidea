//! Výpis stôp a paketov kontajnera pre nezávislého rozhodcu v CI (`media/nastroje/over_media.py`).
//!
//! Použitie: `media_pakety <súbor>` – na stdout riadky v pevnom tvare:
//! ```text
//! STOPA index=<i> cislo=<n> druh=<Video|Zvuk|Titulky|Ine> kodek=<meno>
//! PAKET stopa=<i> pts=<tiky|-> dts=<tiky|-> trvanie=<tiky|-> kluc=<0|1> velkost=<B> orezat=<vzoriek na konci> blok=<poloha v súbore> md5=<32 hex>
//! TOK index=<i> paketov=<n> bajtov=<n> md5=<32 hex>          (MD5 všetkých údajov stopy za sebou)
//! VYSLEDOK paketov=<n>
//! ```
//! `blok` je poloha bloku v súbore – rámce jedného bloku (lacing) ju majú rovnakú. Časy sú v tikoch stopy (pri Matroske `TimestampScale`). Návratový kód 0 ok, 1 chyba čítania, 2 zlé argumenty.

use orchidea_hevc::odtlacok::Md5;
use orchidea_media::{Limity, SuborovyZdroj, otvor};

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
    let mut m = match otvor(zdroj, Limity::default()) {
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
    let mut toky: Vec<(Md5, u64, u64)> = (0..m.stopy().len()).map(|_| (Md5::novy(), 0, 0)).collect();
    loop {
        match m.dalsi_paket() {
            Ok(Some(p)) => {
                n += 1;
                let mut h = Md5::novy();
                h.pridaj(&p.data);
                if let Some(t) = toky.get_mut(p.stopa) {
                    t.0.pridaj(&p.data);
                    t.1 += 1;
                    t.2 += p.data.len() as u64;
                }
                let md5: String = h.vysledok().iter().map(|b| format!("{b:02x}")).collect();
                println!(
                    "PAKET stopa={} pts={} dts={} trvanie={} kluc={} velkost={} orezat={} blok={} md5={md5}",
                    p.stopa,
                    cas(p.pts),
                    cas(p.dts),
                    cas(p.trvanie),
                    p.klucovy as u8,
                    p.data.len(),
                    p.orezat_koniec,
                    p.pozicia
                );
            }
            Ok(None) => break,
            Err(e) => {
                println!("CHYBA {e}");
                std::process::exit(1);
            }
        }
    }
    for (i, (h, pakety, bajty)) in toky.into_iter().enumerate() {
        let md5: String = h.vysledok().iter().map(|b| format!("{b:02x}")).collect();
        println!("TOK index={i} paketov={pakety} bajtov={bajty} md5={md5}");
    }
    println!("VYSLEDOK paketov={n}");
}
