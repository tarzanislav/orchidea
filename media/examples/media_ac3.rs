//! Dekódovanie AC-3 (L9) – nástroj rozhodcu `media/nastroje/over_ac3.py`.
//!
//! - `media_ac3 <súbor.ac3> [výstup.f32]` – elementárny tok AC-3: nájde rámce podľa synchronizačného slova
//!   a veľkosti (tabuľka 5.18), každý dekóduje a voliteľne zapíše prekladané f32 LE (kanály v spoločnom poradí).
//!   Na stdout `AC3 …` (pokrytie nástrojov), `ZVUK kanalov= frekvencia= vzoriek=` a `VYSLEDOK ramcov= chyb=`.
//!   Prepínače pred súborom: `--bez-ditheru` zakáže dither, `--bez-dynrng` dynamický rozsah (§7.7.1 – voľba
//!   poslucháča), `--bloky` vypíše `BLOK <n> …` (príznaky a parametre každého bloku) pred riadkom `AC3`.
//! - `media_ac3 --uprav <vstup.ac3> <výstup.ac3> [--dither 0|1] [--blksw-kazdy N [--blksw-spolu]]` – prepíše bity
//!   dithflag a blksw v rámcoch a prepočíta crc1 a crc2 (A/52 §7.10.1); na stdout `BLKSW <blok> <kanál>` pre každý
//!   krátky blok (číslovanie blokov od začiatku toku). Zmena týchto bitov nemení dĺžku ani polohu
//!   ostatných polí, preto vznikne platný tok s inými nástrojmi – rozhodca ho dekóduje rovnako ako my.
use orchidea_media::ac3::{DekoderAc3, Hlavicka, crc16};
use std::io::Write;

fn ramce(d: &[u8]) -> Vec<(usize, Hlavicka)> {
    let mut v = Vec::new();
    let mut i = 0;
    while i + 7 <= d.len() {
        match Hlavicka::z_bajtov(&d[i..]) {
            Some(h) if i + h.dlzka <= d.len() => {
                v.push((i, h));
                i += h.dlzka;
            }
            Some(_) if v.last().is_some_and(|&(z, h): &(usize, Hlavicka)| z + h.dlzka == i) => {
                // hlavička hneď za posledným rámcom, ale rámec presahuje koniec súboru (skrátený súbor)
                eprintln!("posledný rámec je neúplný (súbor je skrátený)");
                break;
            }
            _ => i += 1,
        }
    }
    v
}

fn main() {
    let mut a: Vec<String> = std::env::args().skip(1).collect();
    if a.first().map(String::as_str) == Some("--uprav") {
        uprav(&a[1..]);
        return;
    }
    let mut bez_ditheru = false;
    let mut bez_dynrng = false;
    let mut bloky = false;
    while let Some(p) = a.first().filter(|p| p.starts_with("--")).cloned() {
        match p.as_str() {
            "--bez-ditheru" => bez_ditheru = true,
            "--bez-dynrng" => bez_dynrng = true,
            "--bloky" => bloky = true,
            x => panic!("neznámy prepínač {x}"),
        }
        a.remove(0);
    }
    let d = std::fs::read(&a[0]).expect("súbor");
    let mut von = a.get(1).map(|c| std::io::BufWriter::new(std::fs::File::create(c).expect("výstup")));
    let mut dek = DekoderAc3::novy();
    dek.bez_ditheru = bez_ditheru;
    dek.dynrng = !bez_dynrng;
    if bloky {
        dek.zaznam = Some(Vec::new());
    }
    let (mut ramcov, mut chyb, mut vzoriek, mut kanalov, mut frekvencia) = (0, 0, 0usize, 0, 0);
    for (i, h) in ramce(&d) {
        ramcov += 1;
        match dek.dekoduj_ramec(&d[i..i + h.dlzka]) {
            Ok(k) => {
                kanalov = k.len();
                frekvencia = h.frekvencia;
                vzoriek += 1536;
                if let Some(v) = von.as_mut() {
                    let mut b = Vec::with_capacity(1536 * k.len() * 4);
                    for t in 0..1536 {
                        for kanal in &k {
                            b.extend_from_slice(&kanal[t].to_le_bytes());
                        }
                    }
                    v.write_all(&b).expect("zápis");
                }
            }
            Err(e) => {
                chyb += 1;
                eprintln!("rámec {ramcov}: {e}");
            }
        }
    }
    for (i, r) in dek.zaznam.take().unwrap_or_default().iter().enumerate() {
        println!("BLOK {i} {r}");
    }
    let s = dek.statistika();
    println!(
        "AC3 ramcov={} blokov={} kratke={} vazba={} fazove={} rematrix={} d15={} d25={} d45={} opakovane={} delta={} \
         dither={} dynrng={} preskok={} lfe={} nulove={} chyby_crc={}",
        s.ramcov,
        s.blokov,
        s.kratke,
        s.vazba,
        s.fazove_priznaky,
        s.rematrix,
        s.d15,
        s.d25,
        s.d45,
        s.opakovane_exponenty,
        s.delta,
        s.dither,
        s.dynrng,
        s.preskok,
        s.lfe,
        s.nulove_rozdelenie,
        s.chyby_crc
    );
    println!("ZVUK kanalov={kanalov} frekvencia={frekvencia} vzoriek={vzoriek}");
    println!("VYSLEDOK ramcov={ramcov} chyb={chyb}");
}

/// Prepis dithflag a blksw a nové CRC.
fn uprav(a: &[String]) {
    let mut d = std::fs::read(&a[0]).expect("vstup");
    let mut dither: Option<bool> = None;
    let mut blksw_kazdy = 0usize;
    let mut spolu = false;
    let mut i = 2;
    while i < a.len() {
        match a[i].as_str() {
            "--dither" => dither = Some(a[i + 1] == "1"),
            "--blksw-kazdy" => blksw_kazdy = a[i + 1].parse().expect("číslo"),
            "--blksw-spolu" => {
                spolu = true;
                i -= 1;
            }
            x => panic!("neznámy prepínač {x}"),
        }
        i += 2;
    }
    let mut dek = DekoderAc3::novy();
    let mut pocitadlo = 0usize;
    for (r, (zac, h)) in ramce(&d.clone()).into_iter().enumerate() {
        let ramec = &mut d[zac..zac + h.dlzka];
        let polohy = dek.polohy_priznakov(ramec).expect("rozbor rámca");
        for (blk, (blksw, dith)) in polohy.iter().enumerate() {
            let blok_celkom = r * 6 + blk;
            for (ch, &p) in blksw.iter().enumerate() {
                if blksw_kazdy > 0 {
                    pocitadlo += 1;
                    // spolu: všetky kanály bloku rovnako; inak každý kanál zvlášť (posunutý vzor)
                    let kratky = if spolu {
                        (blok_celkom + 2).is_multiple_of(blksw_kazdy)
                    } else {
                        (pocitadlo + blk + ch).is_multiple_of(blksw_kazdy)
                    };
                    nastav_bit(ramec, p, kratky);
                    if kratky {
                        println!("BLKSW {blok_celkom} {ch}");
                    }
                }
            }
            if let Some(v) = dither {
                for &p in dith {
                    nastav_bit(ramec, p, v);
                }
            }
        }
        // crc2: posledné slovo tak, aby CRC celého rámca bez synchronizačného slova bolo 0; crc1 podobne pre 5/8
        let slov = h.dlzka / 2;
        let pat = ((slov >> 1) + (slov >> 3)) * 2;
        nastav_crc1(ramec, pat);
        let n = ramec.len();
        ramec[n - 2] = 0;
        ramec[n - 1] = 0;
        let c = crc16(&ramec[2..n - 2]);
        ramec[n - 2..].copy_from_slice(&c.to_be_bytes());
        assert_eq!(crc16(&ramec[2..]), 0);
        assert_eq!(crc16(&ramec[2..pat]), 0);
    }
    std::fs::write(&a[1], d).expect("výstup");
}

fn nastav_bit(d: &mut [u8], poloha: usize, hodnota: bool) {
    let maska = 0x80u8 >> (poloha & 7);
    if hodnota {
        d[poloha >> 3] |= maska;
    } else {
        d[poloha >> 3] &= !maska;
    }
}

/// crc1 (slovo 1) tak, aby CRC bajtov 2 … 5/8 bolo nulové: CRC je lineárne v bitoch crc1 – riešenie sústavy
/// 16 × 16 nad GF(2) z vplyvu každého bitu crc1.
fn nastav_crc1(ramec: &mut [u8], pat: usize) {
    ramec[2] = 0;
    ramec[3] = 0;
    let ciel = crc16(&ramec[2..pat]);
    let mut stlpce = [0u16; 16];
    let mut nuly = vec![0u8; pat - 2];
    for (bit, s) in stlpce.iter_mut().enumerate() {
        nuly[0] = 0;
        nuly[1] = 0;
        let v = 0x8000u16 >> bit;
        nuly[..2].copy_from_slice(&v.to_be_bytes());
        *s = crc16(&nuly);
    }
    // hľadá sa x s Σ x_i · stlpce[i] = ciel (Gaussova eliminácia)
    let mut riadky: Vec<(u16, u16)> = stlpce.iter().enumerate().map(|(i, &s)| (s, 0x8000u16 >> i)).collect();
    let mut vysledok = 0u16;
    let mut zvysok = ciel;
    for bit in (0..16).rev() {
        let m = 1u16 << bit;
        if let Some(p) = riadky.iter().position(|r| r.0 & m != 0) {
            let pivot = riadky.remove(p);
            for r in riadky.iter_mut() {
                if r.0 & m != 0 {
                    r.0 ^= pivot.0;
                    r.1 ^= pivot.1;
                }
            }
            if zvysok & m != 0 {
                zvysok ^= pivot.0;
                vysledok ^= pivot.1;
            }
        }
    }
    assert_eq!(zvysok, 0, "crc1 bez riešenia");
    ramec[2..4].copy_from_slice(&vysledok.to_be_bytes());
}
