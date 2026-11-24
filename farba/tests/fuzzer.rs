//! Fuzzer čítačky profilov ICC (R211): náhodne pozmenené profily (s maticou, s tabuľkami lut16 a lutBtoA)
//! nesmú spôsobiť pád, zacyklenie ani vyčerpanie pamäte – len chybu. Ak sa profil prečíta, prevod cez neho
//! musí prebehnúť (výsledok môže byť nezmyselný, ale bez paniky). Semienko je pevné.

use orchidea_farba::icc::{self, Profil};
use orchidea_farba::{Prenos, Prevod, Priestor, Zamer, primarne};
use orchidea_jadro::nahoda::Nahoda;
use std::sync::Arc;

fn pocet_pokusov() -> usize {
    let zaklad = if cfg!(debug_assertions) { 1_500 } else { 10_000 };
    std::env::var("ORCHIDEA_FUZZER").ok().and_then(|v| v.parse().ok()).unwrap_or(zaklad)
}

fn be16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}

fn be32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

fn s15(v: &mut Vec<u8>, x: f64) {
    v.extend_from_slice(&((x * 65536.0).round() as i32).to_be_bytes());
}

/// Profil CMYK → Lab s tabuľkami: A2B0 lut16 (mriežka 3 body) a B2A0 lutBtoA (krivky, matica, CLUT 2 body).
fn tabulkovy_profil() -> Vec<u8> {
    let mut a2b = b"mft2\0\0\0\0".to_vec();
    a2b.extend_from_slice(&[4, 3, 3, 0]);
    for i in 0..9 {
        s15(&mut a2b, if i % 4 == 0 { 1.0 } else { 0.0 });
    }
    be16(&mut a2b, 2);
    be16(&mut a2b, 2);
    for _ in 0..4 {
        be16(&mut a2b, 0);
        be16(&mut a2b, 65535);
    }
    for i in 0..81u32 {
        for k in 0..3u32 {
            be16(&mut a2b, ((i * 811 + k * 20011) % 65536) as u16);
        }
    }
    for _ in 0..3 {
        be16(&mut a2b, 0);
        be16(&mut a2b, 65535);
    }
    // lutBtoA: hlavička 32 bajtov, potom B krivky (3 × para typ 0), matica, M krivky, CLUT, A krivky (4 × curv)
    let mut b2a = b"mBA \0\0\0\0".to_vec();
    b2a.extend_from_slice(&[3, 4, 0, 0]);
    let para = |v: &mut Vec<u8>| {
        v.extend_from_slice(b"para\0\0\0\0\0\0\0\0");
        s15(v, 1.0);
    };
    let mut telo = Vec::new();
    let o_b = 32;
    for _ in 0..3 {
        para(&mut telo);
    }
    let o_m = 32 + telo.len();
    for i in 0..12 {
        s15(&mut telo, if i < 9 && i % 4 == 0 { 1.0 } else { 0.0 });
    }
    let o_mk = 32 + telo.len();
    for _ in 0..3 {
        para(&mut telo);
    }
    let o_c = 32 + telo.len();
    telo.extend_from_slice(&[2, 2, 2]);
    telo.extend_from_slice(&[0; 13]);
    telo.extend_from_slice(&[2, 0, 0, 0]);
    for i in 0..8u32 * 4 {
        be16(&mut telo, ((i * 2111) % 65536) as u16);
    }
    let o_a = 32 + telo.len();
    for _ in 0..4 {
        telo.extend_from_slice(b"curv\0\0\0\0\0\0\0\0");
    }
    for o in [o_b, o_m, o_mk, o_c, o_a] {
        be32(&mut b2a, o as u32);
    }
    b2a.extend_from_slice(&telo);
    let znacky = [(*b"A2B0", a2b), (*b"B2A0", b2a)];
    let mut data = Vec::new();
    let mut tab = Vec::new();
    let zaciatok = 132 + 12 * znacky.len();
    for (z, d) in &znacky {
        tab.push((*z, zaciatok + data.len(), d.len()));
        data.extend_from_slice(d);
        while data.len() % 4 != 0 {
            data.push(0);
        }
    }
    let mut v = Vec::new();
    be32(&mut v, (zaciatok + data.len()) as u32);
    v.extend_from_slice(&[0; 4]);
    v.extend_from_slice(&[4, 0x30, 0, 0]);
    v.extend_from_slice(b"prtrCMYKLab ");
    v.extend_from_slice(&[0; 12]);
    v.extend_from_slice(b"acsp");
    v.resize(128, 0);
    be32(&mut v, tab.len() as u32);
    for (z, p, n) in tab {
        v.extend_from_slice(&z);
        be32(&mut v, p as u32);
        be32(&mut v, n as u32);
    }
    v.extend_from_slice(&data);
    v
}

fn zmutuj(zaklad: &[u8], n: &mut Nahoda) -> Vec<u8> {
    let mut d = zaklad.to_vec();
    for _ in 0..1 + n.mensie_ako(6) {
        if d.is_empty() {
            break;
        }
        let i = n.mensie_ako(d.len() as u64) as usize;
        match n.mensie_ako(6) {
            0 | 1 => d[i] = n.mensie_ako(256) as u8,
            2 => d[i] ^= 1 << n.mensie_ako(8),
            3 => d.truncate(i),
            4 => {
                let k = (i + 1 + n.mensie_ako(16) as usize).min(d.len());
                d.drain(i..k);
            }
            _ => d.insert(i, [0, 0xFF, 0x7F, 0x80][n.mensie_ako(4) as usize]),
        }
    }
    d
}

#[test]
fn icc_bez_padu() {
    let zaklady = [
        icc::zapis_rgb(&primarne::SRGB, &Prenos::Srgb, "sRGB"),
        icc::zapis_rgb(&primarne::BT2020, &Prenos::Pq, "PQ (tabuľka)"),
        tabulkovy_profil(),
    ];
    // základy sú platné a prevod cez ne funguje
    for z in &zaklady {
        let p = Profil::citaj(z).unwrap_or_else(|e| panic!("platný základ {}: {e:?}", z.len()));
        let pr =
            Prevod::novy(&Priestor::Icc(Arc::new(p)), &Priestor::srgb(), Zamer::Percepcny).expect("prevod");
        let k = pr.vstupov();
        let v: Vec<u8> = (0..k * 64).map(|i| (i * 37) as u8).collect();
        assert_eq!(pr.preved_u8(&v, k, 3).unwrap().len(), 64 * 3);
    }
    let semienko = std::env::var("ORCHIDEA_FUZZER_SEMIENKO").ok().and_then(|v| v.parse().ok()).unwrap_or(7);
    let mut n = Nahoda::zo_semienka(semienko);
    let mut precitanych = 0;
    for _ in 0..pocet_pokusov() {
        let z = &zaklady[n.mensie_ako(zaklady.len() as u64) as usize];
        let d = zmutuj(z, &mut n);
        if let Ok(p) = Profil::citaj(&d) {
            precitanych += 1;
            let p = Priestor::Icc(Arc::new(p));
            for (zdroj, ciel) in [(&p, &Priestor::srgb()), (&Priestor::srgb(), &p)] {
                if let Ok(pr) = Prevod::novy(zdroj, ciel, Zamer::RelativnyKolorimetricky) {
                    let k = pr.vstupov();
                    let v: Vec<u8> = (0..k * 16).map(|i| (i * 53) as u8).collect();
                    let _ = pr.preved_u8(&v, k, pr.vystupov());
                    let f: Vec<f32> = v.iter().map(|&x| x as f32 / 255.0).collect();
                    let _ = pr.preved_f32(&f, k, pr.vystupov());
                }
            }
        }
    }
    assert!(precitanych > 0, "žiadny pozmenený profil sa neprečítal – mutácie sú priveľké");
}
