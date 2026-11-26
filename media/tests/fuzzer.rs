//! Fuzzer balíka `media` (R211): náhodné a pozmenené vstupy nesmú spôsobiť pád ani zacyklenie – len chybu.
//! Etapa L0 pokrýva rozpoznanie kontajnera, konfigurácie avcC a hvcC, prevod jednotiek NAL na Annex B,
//! prevodník videa (R5a) a prekladanie zvuku (R5b). Každá ďalšia čítačka a dekodér sa pridá sem.
//! Semienko je pevné – chyba sa dá zopakovať; dlhý beh cez ORCHIDEA_FUZZER a ORCHIDEA_FUZZER_SEMIENKO.

use orchidea_jadro::nahoda::Nahoda;
use orchidea_media::cas::zakladna_z_ns;
use orchidea_media::r5::{self, HlbkaPcm, PrevodnikVidea};
use orchidea_media::{BlokZvuku, Kodek, Paket, RozlozenieKanalov, Stopa, VzorkyZvuku, rozpoznaj};

fn pocet_pokusov() -> usize {
    let zaklad = if cfg!(debug_assertions) { 2_000 } else { 10_000 };
    std::env::var("ORCHIDEA_FUZZER").ok().and_then(|v| v.parse().ok()).unwrap_or(zaklad)
}

fn nahoda(semienko: u64) -> Nahoda {
    let extra: u64 = std::env::var("ORCHIDEA_FUZZER_SEMIENKO").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    Nahoda::zo_semienka(semienko ^ extra.wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

fn bajty(n: &mut Nahoda, najviac: u64) -> Vec<u8> {
    let l = n.mensie_ako(najviac + 1) as usize;
    (0..l).map(|_| n.dalsie() as u8).collect()
}

fn zmutuj(zaklad: &[u8], n: &mut Nahoda) -> Vec<u8> {
    let mut d = zaklad.to_vec();
    for _ in 0..1 + n.mensie_ako(4) {
        if d.is_empty() {
            break;
        }
        let i = n.mensie_ako(d.len() as u64) as usize;
        match n.mensie_ako(6) {
            0..=2 => d[i] = n.dalsie() as u8,
            3 => d[i] ^= 1 << n.mensie_ako(8),
            4 => d.insert(i, n.dalsie() as u8),
            _ => d.truncate(i),
        }
    }
    d
}

/// Platný avcC (SPS, PPS) a hvcC (VPS, SPS, PPS) ako základ mutácií.
fn zaklady_konfiguracie() -> (Vec<u8>, Vec<u8>) {
    let avcc = vec![1, 0x64, 0, 0x28, 0xFF, 0xE1, 0, 4, 0x67, 0x64, 0, 0x28, 1, 0, 3, 0x68, 0xEE, 0x3C];
    let mut hvcc = vec![
        1u8, 1, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 0x5D, 0xF0, 0, 0xFC, 0xFD, 0xF8, 0xF8, 0, 0, 0x0F, 3,
    ];
    for (typ, nal) in [(32u8, &[0x40u8, 1, 0x0C][..]), (33, &[0x42, 1, 1][..]), (34, &[0x44, 1, 0xC1][..])] {
        hvcc.extend_from_slice(&[0x80 | typ, 0, 1, 0, nal.len() as u8]);
        hvcc.extend_from_slice(nal);
    }
    (avcc, hvcc)
}

#[test]
fn rozpoznanie_kontajnera() {
    let mut n = nahoda(1);
    let mut zaklady = vec![
        vec![0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42, 0x86, 0x81, 1],
        b"\0\0\0\x18ftypisom\0\0\x02\0isomiso2".to_vec(),
    ];
    let mut ts = vec![0u8; 188 * 3];
    for k in 0..3 {
        ts[k * 188] = 0x47;
    }
    zaklady.push(ts);
    for i in 0..pocet_pokusov() {
        let d = if i % 4 == 0 { bajty(&mut n, 600) } else { zmutuj(&zaklady[i % zaklady.len()], &mut n) };
        let _ = rozpoznaj(&d);
    }
}

#[test]
fn konfiguracia_a_annex_b() {
    let mut n = nahoda(2);
    let (avcc, hvcc) = zaklady_konfiguracie();
    for i in 0..pocet_pokusov() {
        let a = if i % 5 == 0 { bajty(&mut n, 64) } else { zmutuj(&avcc, &mut n) };
        let _ = r5::avcc(&a);
        let h = if i % 5 == 0 { bajty(&mut n, 64) } else { zmutuj(&hvcc, &mut n) };
        let _ = r5::hvcc(&h);
        let p = bajty(&mut n, 256);
        let mut v = Vec::new();
        if let Ok(k) = r5::annex_b_z_dlzok(&p, 1 + (i % 4) as u8, &mut v) {
            // každá úspešne prevedená jednotka má štartovací kód a výstup nie je dlhší ako vstup + kódy
            assert!(v.len() <= p.len() + 4 * k);
        }
    }
}

#[test]
fn prevodnik_videa() {
    let mut n = nahoda(3);
    let (avcc, hvcc) = zaklady_konfiguracie();
    let zakladna = zakladna_z_ns(1_000_000).unwrap();
    for i in 0..pocet_pokusov() / 4 {
        let (kodek, konfig) = match i % 4 {
            0 => (Kodek::H264, zmutuj(&avcc, &mut n)),
            1 => (Kodek::Hevc, zmutuj(&hvcc, &mut n)),
            2 => (Kodek::Av1, bajty(&mut n, 16)),
            _ => (Kodek::Mpeg2Video, bajty(&mut n, 16)),
        };
        let mut s = Stopa::nova(1, kodek, zakladna);
        s.konfiguracia = konfig;
        let Ok(mut pv) = PrevodnikVidea::novy(&s) else { continue };
        let mut posledne = 0;
        for _ in 0..8 {
            let paket = Paket {
                data: bajty(&mut n, 128),
                pts: Some(n.dalsie() as i64),
                trvanie: Some(n.mensie_ako(100) as i64),
                klucovy: n.mensie_ako(2) == 0,
                ..Paket::default()
            };
            if let Ok(v) = pv.prevod(&paket) {
                assert!(v.id > posledne, "id musí stúpať");
                posledne = v.id;
            }
        }
    }
}

#[test]
fn prekladanie_zvuku() {
    let mut n = nahoda(4);
    for i in 0..pocet_pokusov() / 4 {
        let kanalov = 1 + n.mensie_ako(8) as u8;
        let ramcov = n.mensie_ako(64) as usize;
        let rozlozenie = RozlozenieKanalov::predvolene(kanalov).unwrap();
        let pocet = kanalov as usize * ramcov + if i % 7 == 0 { 1 } else { 0 };
        let vzorky = if i % 2 == 0 {
            let bitov = 1 + n.mensie_ako(32) as u8;
            VzorkyZvuku::Cele { bitov, data: (0..pocet).map(|_| n.dalsie() as i32).collect() }
        } else {
            VzorkyZvuku::F32((0..pocet).map(|_| f32::from_bits(n.dalsie() as u32)).collect())
        };
        let blok = BlokZvuku { frekvencia: 48_000, rozlozenie, ramcov, vzorky, pts_ns: None };
        for h in [HlbkaPcm::S16, HlbkaPcm::S32] {
            let mut v = Vec::new();
            match r5::prekladaj(&blok, h, &mut v) {
                Ok(()) => assert_eq!(v.len(), pocet * if h == HlbkaPcm::S16 { 2 } else { 4 }),
                Err(_) => assert!(pocet != kanalov as usize * ramcov || ramcov == 0 && pocet == 0),
            }
        }
    }
}
