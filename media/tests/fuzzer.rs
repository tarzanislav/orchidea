//! Fuzzer balíka `media` (R211): náhodné a pozmenené vstupy nesmú spôsobiť pád ani zacyklenie – len chybu.
//! Etapa L0 pokrýva rozpoznanie kontajnera, konfigurácie avcC a hvcC, prevod jednotiek NAL na Annex B,
//! prevodník videa (R5a) a prekladanie zvuku (R5b). Každá ďalšia čítačka a dekodér sa pridá sem.
//! Semienko je pevné – chyba sa dá zopakovať; dlhý beh cez ORCHIDEA_FUZZER a ORCHIDEA_FUZZER_SEMIENKO.

mod spolocne;

use orchidea_jadro::nahoda::Nahoda;
use orchidea_media::cas::zakladna_z_ns;
use orchidea_media::flac::FlacDekoder;
use orchidea_media::flac_subor::FlacSubor;
use orchidea_media::mkv::Mkv;
use orchidea_media::mp4::Mp4;
use orchidea_media::r5::{self, HlbkaPcm, PrevodnikVidea};
use orchidea_media::{
    BlokZvuku, Kodek, Kontajner, Limity, Paket, PametovyZdroj, Posun, RozlozenieKanalov, Stopa, VzorkyZvuku,
    rozpoznaj,
};

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

/// Prísne limity – fuzzer nesmie alokovať veľa pamäte ani pri „platnej“ obrovskej veľkosti v hlavičke.
fn prisne() -> Limity {
    Limity { prvok: 1 << 20, paket: 1 << 20, index: 1 << 20, stop: 16, hlbka: 8 }
}

fn zaklady_matrosky() -> Vec<Vec<u8>> {
    use spolocne::*;
    let (typ, _) = typicky(2, true);
    let (bez_cues, _) = typicky(3, false);
    let mut zivy = ebml("webm");
    zivy.extend(neznamy(0x18538067));
    zivy.extend(info(1_000_000, 0.0));
    zivy.extend(stopy(&[stopa(1, 2, "A_OPUS", &[zvuk(48000.0, 2)]), stopa(2, 1, "V_VP9", &[video(64, 48)])]));
    for z in 0..3u64 {
        let ramce: [&[u8]; 3] = [&[1; 30], &[2; 7], &[3; 5]];
        let bloky = vec![
            prvok(0xA3, &telo_bloku(1, 0, 0x82, &ramce)),
            prvok(0xA3, &telo_bloku(1, 10, 0x86, &ramce)),
            prvok(0xA3, &telo_bloku(2, 5, 0x84, &[&[4; 4], &[5; 4]])),
            prvok(
                0xA0,
                &spoj(&[
                    prvok(0xA1, &telo_bloku(2, 30, 0, &[&[6, 7]])),
                    uint(0x9B, 30),
                    prvok(0xFB, &[0xFF]),
                ]),
            ),
        ];
        zivy.extend(zhluk_neznamy(z * 100, &bloky));
    }
    vec![typ, bez_cues, zivy]
}

#[test]
fn matroska() {
    let mut n = nahoda(5);
    let zaklady = zaklady_matrosky();
    let mut otvorenych = 0;
    for i in 0..pocet_pokusov() / 4 {
        let d = if i % 10 == 0 { bajty(&mut n, 400) } else { zmutuj(&zaklady[i % zaklady.len()], &mut n) };
        let Ok(mut m) = Mkv::otvor(PametovyZdroj::novy(d), prisne()) else { continue };
        otvorenych += 1;
        let stopy = m.stopy().len();
        for _ in 0..2_000 {
            match m.dalsi_paket() {
                Ok(Some(p)) => assert!(p.stopa < stopy),
                _ => break,
            }
        }
        let ciel = n.mensie_ako(5_000_000_000) as i64 - 1_000_000_000;
        let sposob = if n.mensie_ako(2) == 0 { Posun::KlucovyPred } else { Posun::Presny };
        if m.posun(ciel, sposob).is_ok() {
            for _ in 0..200 {
                if !matches!(m.dalsi_paket(), Ok(Some(_))) {
                    break;
                }
            }
        }
    }
    assert!(otvorenych > pocet_pokusov() / 40, "mutácie musia často ostať otvoriteľné ({otvorenych})");
}

fn subory_flac() -> Vec<Vec<u8>> {
    // vložené do programu skúšok – fuzzer beží rovnako vo WebAssembly, na Androide aj v QEMU
    vec![
        include_bytes!("data/stereo16_lpc.flac").to_vec(),
        include_bytes!("data/kanaly6_24b.flac").to_vec(),
        include_bytes!("data/mono8_pevny.flac").to_vec(),
    ]
}

#[test]
fn flac_subory_a_dekoder() {
    let mut n = nahoda(6);
    let zaklady = subory_flac();
    let mut blok = BlokZvuku::cele(0, RozlozenieKanalov::STEREO, 16);
    for i in 0..pocet_pokusov() / 20 {
        let d = if i % 10 == 0 { bajty(&mut n, 400) } else { zmutuj(&zaklady[i % zaklady.len()], &mut n) };
        let Ok(mut f) = FlacSubor::otvor(PametovyZdroj::novy(d), prisne()) else { continue };
        let Ok(mut dek) = FlacDekoder::novy(&f.stopy()[0].konfiguracia, None) else { continue };
        for _ in 0..64 {
            match f.dalsi_paket() {
                Ok(Some(p)) => {
                    let _ = dek.dekoduj_ramec(&p.data, &mut blok);
                }
                _ => break,
            }
        }
        let _ = f.posun(n.mensie_ako(200_000_000) as i64, Posun::Presny);
        let _ = f.dalsi_paket();
    }
}

#[test]
fn flac_ramce_s_platnym_crc() {
    // mutácie vnútri rámca s prepočítaným CRC-16 – dekodér musí prejsť cez podrámce a zvyšky, nie skončiť na CRC
    let mut n = nahoda(7);
    let mut ramce = Vec::new();
    let mut konfiguracie = Vec::new();
    for d in subory_flac() {
        let mut f = FlacSubor::otvor(PametovyZdroj::novy(d), Limity::default()).unwrap();
        let k = f.stopy()[0].konfiguracia.clone();
        while let Some(p) = f.dalsi_paket().unwrap() {
            ramce.push((konfiguracie.len(), p.data));
        }
        konfiguracie.push(k);
    }
    let mut blok = BlokZvuku::cele(0, RozlozenieKanalov::STEREO, 16);
    for i in 0..pocet_pokusov() / 4 {
        let (k, r) = &ramce[i % ramce.len()];
        let mut m = zmutuj(r, &mut n);
        if m.len() > 8 {
            let l = m.len() - 2;
            let c = orchidea_media::flac::crc16(&m[..l]);
            m[l..].copy_from_slice(&c.to_be_bytes());
        }
        let mut d = FlacDekoder::novy(&konfiguracie[*k], None).unwrap();
        let _ = d.dekoduj_ramec(&m, &mut blok);
    }
}

#[test]
fn mp4_a_mov() {
    use spolocne::mp4::*;
    let mut n = nahoda(8);
    let pcm = (0..2_000u32).map(|i| ((i as u16).to_be_bytes().to_vec(), 1, 0, true)).collect();
    let zaklady = [
        typicky(1, false, false),
        typicky(1, true, true),
        fragmentovany(3),
        subor(
            &[StopaMp4 {
                id: 1,
                video: false,
                mierka: 8_000,
                vzorky: pcm,
                vzoriek_v_kusku: 500,
                uprava: None,
                zaznam: zvuk(b"twos", 1, 16, 8_000, &[]),
            }],
            false,
            false,
            true,
        ),
    ];
    let mut otvorenych = 0;
    for i in 0..pocet_pokusov() / 4 {
        let d = if i % 10 == 0 { bajty(&mut n, 400) } else { zmutuj(&zaklady[i % zaklady.len()], &mut n) };
        let Ok(mut m) = Mp4::otvor(PametovyZdroj::novy(d), prisne()) else { continue };
        otvorenych += 1;
        let stopy = m.stopy().len();
        for _ in 0..2_000 {
            match m.dalsi_paket() {
                Ok(Some(p)) => assert!(p.stopa < stopy),
                _ => break,
            }
        }
        let sposob = if n.mensie_ako(2) == 0 { Posun::KlucovyPred } else { Posun::Presny };
        if m.posun(n.mensie_ako(4_000_000_000) as i64, sposob).is_ok() {
            for _ in 0..100 {
                if !matches!(m.dalsi_paket(), Ok(Some(_))) {
                    break;
                }
            }
        }
    }
    assert!(otvorenych > pocet_pokusov() / 40, "mutácie musia často ostať otvoriteľné ({otvorenych})");
}

#[test]
fn ts_a_m2ts() {
    use orchidea_media::ts::Ts;
    use spolocne::ts::*;
    let mut n = nahoda(9);
    let mut zmiesane = Zapisovac::novy(188);
    zmiesane.pat(&[(1, 0x1000)]);
    zmiesane.pmt(
        0x1000,
        1,
        &[],
        &[
            (0x81, 0x101, vec![]),
            (0x06, 0x102, vec![0x7A, 1, 0]),
            (0x03, 0x103, vec![]),
            (0x06, 0x104, vec![0x05, 4, b'O', b'p', b'u', b's', 0x7F, 2, 0x80, 2]),
            (0x24, 0x105, vec![]),
        ],
    );
    for k in 0..6u64 {
        let t = 90_000 + k * 2880;
        zmiesane.pes(0x101, 0xBD, Some(t), None, false, &[ac3(1), ac3(2)].concat(), true);
        zmiesane.pes(
            0x102,
            0xBD,
            Some(t),
            None,
            false,
            &[eac3(400, false, 1), eac3(200, true, 2)].concat(),
            true,
        );
        zmiesane.pes(0x103, 0xC0, Some(t), None, false, &mp2(3), true);
        zmiesane.pes(0x104, 0xBD, Some(t), None, false, &[0x7F, 0xF0, 3, 0, 9, 15 << 3, 1, 2], true);
        zmiesane.pes(0x105, 0xE0, Some(t), Some(t - 3000), k == 0, &[0, 0, 1, 0x26, 1, 0xAF, 9], false);
    }
    let zaklady = [
        typicky(188, 1, 126_000).0,
        typicky(192, 1, (1 << 33) - 50_000).0,
        typicky(204, 1, 0).0,
        zmiesane.data,
    ];
    let mut otvorenych = 0;
    for i in 0..pocet_pokusov() / 4 {
        let d = if i % 10 == 0 { bajty(&mut n, 2_000) } else { zmutuj(&zaklady[i % zaklady.len()], &mut n) };
        let Ok(mut m) = Ts::otvor(PametovyZdroj::novy(d), prisne()) else { continue };
        otvorenych += 1;
        let stopy = m.stopy().len();
        for _ in 0..2_000 {
            match m.dalsi_paket() {
                Ok(Some(p)) => assert!(p.stopa < stopy),
                _ => break,
            }
        }
        let sposob = if n.mensie_ako(2) == 0 { Posun::KlucovyPred } else { Posun::Presny };
        if m.posun(n.mensie_ako(4_000_000_000) as i64, sposob).is_ok() {
            for _ in 0..100 {
                if !matches!(m.dalsi_paket(), Ok(Some(_))) {
                    break;
                }
            }
        }
    }
    assert!(otvorenych > pocet_pokusov() / 40, "mutácie musia často ostať otvoriteľné ({otvorenych})");
}

#[test]
fn prevodnik_zvuku_hdmi() {
    use orchidea_media::hdmi::{CestaZvuku, PrevodnikZvuku, VolbyZvuku, sloty_cta};
    use orchidea_media::r5::{FormatVzoriekHdmi, SchopnostiHdmi};
    use orchidea_media::{Kodek, Paket, ParametreZvuku, Stopa};
    let mut n = nahoda(10);
    let formaty = [FormatVzoriekHdmi::S16Le, FormatVzoriekHdmi::S32Le, FormatVzoriekHdmi::Iec958PodramecLe];
    for i in 0..pocet_pokusov() / 8 {
        let maska = n.dalsie() as u32 & 0x3_FFFF;
        let rozlozenie = RozlozenieKanalov(if maska == 0 { 3 } else { maska });
        if let Some((_, s)) = sloty_cta(rozlozenie) {
            assert!(s.len() >= 2 && s.len() <= 8 && s.len() % 2 == 0);
            assert!(s.iter().flatten().all(|&k| k < rozlozenie.kanalov() as usize));
        }
        let kodek = [Kodek::Flac, Kodek::Ac3, Kodek::Eac3, Kodek::Opus][i % 4].clone();
        let mut stopa = Stopa::nova(1, kodek, orchidea_media::cas::zakladna_90k());
        let bitov = 1 + n.mensie_ako(32) as u8;
        stopa.zvuk = Some(ParametreZvuku {
            frekvencia: [44_100, 48_000, 96_000, 7][n.mensie_ako(4) as usize],
            kanalov: rozlozenie.kanalov() as u8,
            rozlozenie,
            bitov: Some(bitov),
        });
        let schopnosti = SchopnostiHdmi {
            formaty: formaty.iter().copied().filter(|_| n.mensie_ako(2) == 0).collect(),
            max_kanalov: n.mensie_ako(10) as u8,
            frekvencie: vec![],
            max_bitov: n.mensie_ako(30) as u8,
            priechod_ac3: n.mensie_ako(2) == 0,
            priechod_eac3: n.mensie_ako(2) == 0,
            oneskorenie_tv_ns: None,
        };
        let volby = VolbyZvuku { priechod: n.mensie_ako(4) != 0, stereo: n.mensie_ako(8) == 0 };
        let Ok(mut p) = PrevodnikZvuku::novy(&stopa, &schopnosti, volby) else { continue };
        let bajtov = p.bajtov_na_ramec();
        assert!(p.nastavenie().kanalov >= 2 && p.nastavenie().kanalov <= 8);
        if let CestaZvuku::Priechod(_) = p.cesta() {
            for _ in 0..4 {
                let mut data = bajty(&mut n, 3000);
                if n.mensie_ako(2) == 0 && data.len() > 8 {
                    data[..2].copy_from_slice(&[0x0B, 0x77]);
                }
                if let Ok(z) = p.priechod(&Paket { data, pts: Some(n.dalsie() as i64), ..Paket::default() }) {
                    assert_eq!(z.data.len(), z.ramcov * bajtov);
                }
            }
        } else {
            let ramcov = n.mensie_ako(40) as usize;
            let pocet = rozlozenie.kanalov() as usize * ramcov;
            let vzorky = if n.mensie_ako(2) == 0 {
                VzorkyZvuku::Cele { bitov, data: (0..pocet).map(|_| n.dalsie() as i32).collect() }
            } else {
                VzorkyZvuku::F32((0..pocet).map(|_| f32::from_bits(n.dalsie() as u32)).collect())
            };
            let f = stopa.zvuk.unwrap().frekvencia;
            let blok = BlokZvuku { frekvencia: f, rozlozenie, ramcov, vzorky, pts_ns: None };
            if let Ok(z) = p.pcm(&blok) {
                assert_eq!(z.data.len(), ramcov * bajtov);
            }
        }
    }
}

#[test]
fn hodiny_a_synchronizacia() {
    use orchidea_media::hodiny::{HodinyPrehravania, Synchronizacia};
    use orchidea_media::r5::{MeranieZvuku, Obnova};
    let mut n = nahoda(11);
    for _ in 0..pocet_pokusov() / 200 {
        let mut h = if n.mensie_ako(4) == 0 {
            HodinyPrehravania::monotonne(n.dalsie() as i64, n.dalsie() as i64)
        } else {
            HodinyPrehravania::zo_zvuku(n.dalsie() as u32, n.dalsie() as i64)
        };
        let mut s = Synchronizacia::nova(n.dalsie() as i64);
        let mut t = n.dalsie() as i64 / 4;
        let mut posledny: Option<i64> = None;
        for _ in 0..200 {
            let skok =
                if n.mensie_ako(8) == 0 { n.dalsie() as i64 } else { n.mensie_ako(300_000_000) as i64 };
            t = t.saturating_add(skok);
            match n.mensie_ako(10) {
                0 => h.pauza(t),
                1 => h.pokracuj(t),
                2 => {
                    h.posun(n.dalsie() as i64, t);
                    posledny = None;
                    s.prerusenie();
                }
                _ => {
                    let m = MeranieZvuku {
                        odovzdanych_ramcov: n.dalsie(),
                        oneskorenie_ramcov: n.dalsie() >> n.mensie_ako(64),
                        monotonny_ns: t.saturating_add(n.dalsie() as i64 >> 40),
                    };
                    let _ = h.meranie(&m);
                }
            }
            let fronta: Vec<i64> = {
                let mut f: Vec<i64> = (0..n.mensie_ako(6)).map(|_| n.dalsie() as i64).collect();
                f.sort();
                f
            };
            let r = s.obnova(&h, Obnova { monotonny_ns: t, perioda_ns: n.dalsie() as i64 }, &fronta);
            if let Some(i) = r.zobraz {
                assert!(i < fronta.len() && r.vynechat == i);
            }
            let _ = posledny.replace(h.cas_ns(t).unwrap_or(0));
        }
    }
}

#[test]
fn opus_pakety() {
    use orchidea_media::opus::DekoderOpus;
    use orchidea_media::opus::balenie::rozbal;
    let mut n = nahoda(12);
    let mut d = DekoderOpus::novy();
    for i in 0..pocet_pokusov() / 20 {
        let mut p = bajty(&mut n, 1300);
        if let Some(b) = p.first_mut() {
            // polovica SILK (konfigurácie 0–11), štvrtina CELT (16–31), zvyšok ľubovoľná TOC
            match i % 4 {
                0 | 1 => *b = (*b % 96) | (*b & 7),
                2 => *b |= 0x80,
                _ => {}
            }
        }
        let _ = rozbal(&p, n.mensie_ako(2) == 0);
        if let Ok(v) = d.dekoduj_paket(&p) {
            // CELT z náhodných bitov môže mať energiu až 2^32 (strop RFC 8251) – podmienkou je konečná hodnota
            assert!(v.kanaly.iter().all(|k| k.iter().all(|s| s.is_finite())));
        }
        if n.mensie_ako(50) == 0 {
            d = DekoderOpus::novy();
        }
    }
}

/// Dekodér stopy Opus s viacerými prúdmi (5.1, rodina 1): náhodné pakety zložené zo samooddeľovacích častí
/// rôznych režimov a ich poškodenie – nikdy pád, výstup konečný, tvar bloku sedí.
#[test]
fn opus_viac_prudov() {
    use orchidea_media::opus::dekoder::OpusDekoder;
    use orchidea_media::zvuk::DekoderZvuku;
    let mut h = b"OpusHead".to_vec();
    h.extend_from_slice(&[1, 6, 0x38, 0x01, 0x80, 0xBB, 0, 0, 0, 0, 1, 4, 2, 0, 4, 1, 2, 3, 5]);
    let mut d = OpusDekoder::novy(&h, None).unwrap();
    let mut blok = BlokZvuku::f32(48_000, RozlozenieKanalov::S5_1);
    let mut n = nahoda(13);
    for i in 0..pocet_pokusov() / 40 {
        // 3 samooddeľovacie prúdy (kód 0: TOC, dĺžka, rámec) + posledný bežný; rovnaká dĺžka rámca (20 ms)
        let mut p = Vec::new();
        for s in 0..4 {
            let konfig = [3u8, 15, 31, 7][(i + s) % 4];
            let stereo = (s < 2) as u8;
            let telo = bajty(&mut n, 200);
            p.push((konfig << 3) | (stereo << 2));
            if s < 3 {
                p.push(telo.len().min(251) as u8);
                p.extend_from_slice(&telo[..telo.len().min(251)]);
            } else {
                p.extend_from_slice(&telo);
            }
        }
        if i % 3 == 0 && !p.is_empty() {
            let k = n.dalsie() as usize % p.len();
            p[k] ^= 1 << (n.dalsie() % 8);
        }
        let paket = Paket { data: p, ..Paket::default() };
        if d.dekoduj(&paket, &mut blok).is_ok() {
            blok.over().unwrap();
            if let VzorkyZvuku::F32(v) = &blok.vzorky {
                assert!(v.iter().all(|x| x.is_finite()));
            }
        }
    }
}

/// Rozbor vrstvy III MPEG-1 (L8): platná hlavička + náhodné vedľajšie informácie a hlavné údaje (zásobník bitov,
/// škálovacie faktory, Huffman) – nikdy pád ani zacyklenie.
#[test]
fn mpa_vrstvy() {
    use orchidea_media::mpa::DekoderMpa;
    let mut n = nahoda(17);
    let mut d = DekoderMpa::novy();
    for i in 0..pocet_pokusov() / 20 {
        // hlavička: bez CRC / s CRC, všetky toky, frekvencie a režimy
        let tok = 1 + (n.dalsie() % 14) as u8;
        let f = (n.dalsie() % 3) as u8;
        let rezim = (n.dalsie() % 4) as u8;
        // vrstvy I, II, III (MPEG-1), vrstvy I, II, III MPEG-2 LSF a vrstva III MPEG-2.5
        let vrstva = [0xFAu8, 0xFC, 0xFE, 0xF2, 0xF4, 0xF6, 0xE2][(n.dalsie() % 7) as usize];
        let mut p = vec![
            0xFF,
            vrstva | (i % 2) as u8,
            (tok << 4) | (f << 2),
            (rezim << 6) | ((n.dalsie() % 4) as u8) << 4,
        ];
        p.extend(bajty(&mut n, 1500));
        // celé dekódovanie: rozbor, spätná kvantizácia, stereo, preradenie, banka filtrov – žiadny pád,
        // výstup vždy konečný (aj pri nezmyselných ziskoch a polohách intenzity)
        if let Ok(pcm) = d.dekoduj_ramec(&p) {
            assert!(
                pcm.iter().all(|k| [1152, 576, 384].contains(&k.len()) && k.iter().all(|v| v.is_finite()))
            );
        }
        if i % 97 == 0 {
            d.reset();
        }
    }
}

/// Súbor MPEG audio: ID3v2, rámec Info so značkou LAME a TOC, rámce s náhodnými údajmi, APEv2 a ID3v1.
fn subor_mpa(n: &mut Nahoda, ramcov: usize) -> Vec<u8> {
    let mut d = b"ID3\x03\x00\x00\x00\x00\x00\x08\xFF\xFB\x90\x00\xFF\xFB\x00\x00".to_vec();
    let mut info = vec![0u8; 417];
    info[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
    info[36..40].copy_from_slice(b"Info");
    info[40..44].copy_from_slice(&7u32.to_be_bytes());
    info[44..48].copy_from_slice(&(ramcov as u32).to_be_bytes());
    info[48..52].copy_from_slice(&((ramcov as u32 + 1) * 418).to_be_bytes());
    for i in 0..100 {
        info[52 + i] = (i * 256 / 100) as u8;
    }
    info[152..161].copy_from_slice(b"LAME3.100");
    info[173..176].copy_from_slice(&[0x24, 0x05, 0xA0]);
    d.extend(info);
    for _ in 0..ramcov {
        let mut r = vec![0xFF, 0xFB, 0x92, (n.mensie_ako(4) as u8) << 6 | (n.mensie_ako(4) as u8) << 4];
        r.extend(bajty(n, 414).into_iter().chain(std::iter::repeat(0)).take(414));
        d.extend(r);
    }
    d.extend(b"APETAGEX\xD0\x07\x00\x00\x20\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00");
    d.extend(b"TAG");
    d.extend([0u8; 125]);
    d
}

#[test]
fn mpa_subor() {
    use orchidea_media::zvuk::dekoder_zvuku;
    let mut n = nahoda(18);
    let zaklady: Vec<Vec<u8>> = (0..4).map(|i| subor_mpa(&mut n, 20 + 30 * i)).collect();
    for i in 0..pocet_pokusov() / 10 {
        let d = if i % 8 == 0 { zaklady[i % 4].clone() } else { zmutuj(&zaklady[i % 4], &mut n) };
        let Ok(mut k) = orchidea_media::kontajner::otvor(PametovyZdroj::novy(d), Limity::default()) else {
            continue;
        };
        let _ = k.trvanie_ns();
        let mut dek = k.stopy().first().and_then(|s| dekoder_zvuku(s).ok());
        let mut blok = BlokZvuku::f32(44_100, RozlozenieKanalov::STEREO);
        for _ in 0..400 {
            match k.dalsi_paket() {
                Ok(Some(p)) => {
                    assert!(
                        p.orezat_zaciatok as i64 + p.orezat_koniec as i64
                            <= p.trvanie.unwrap_or(1152).max(1152)
                    );
                    if let Some(dek) = dek.as_mut() {
                        let _ = dek.dekoduj(&p, &mut blok);
                    }
                }
                _ => break,
            }
        }
        let _ = k.posun(n.mensie_ako(3_000_000_000) as i64, Posun::Presny);
        for _ in 0..3 {
            let _ = k.dalsi_paket();
        }
    }
}

/// Stránka Ogg (RFC 3533) s CRC podľa `framing.html` (mnohočlen 0x04C11DB7, počiatok 0, bez zrkadlenia).
fn ogg_stranka(priznaky: u8, granula: i64, poradie: u32, pakety: &[Vec<u8>]) -> Vec<u8> {
    let (mut seg, mut data): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());
    for p in pakety {
        seg.extend(std::iter::repeat_n(255u8, p.len() / 255));
        seg.push((p.len() % 255) as u8);
        data.extend(p);
    }
    let mut d = b"OggS\x00".to_vec();
    d.push(priznaky);
    d.extend(granula.to_le_bytes());
    d.extend(0x1234u32.to_le_bytes());
    d.extend(poradie.to_le_bytes());
    d.extend([0; 4]);
    d.push(seg.len() as u8);
    d.extend(seg);
    d.extend(data);
    let mut c = 0u32;
    for &b in &d {
        c ^= (b as u32) << 24;
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
        }
    }
    d[22..26].copy_from_slice(&c.to_le_bytes());
    d
}

/// Súbor Ogg Opus: OpusHead (1–2 kanály), OpusTags, stránky s pakety s platným TOC a náhodnými dátami.
fn subor_ogg(n: &mut Nahoda, stranok: usize) -> Vec<u8> {
    let kanalov = 1 + n.mensie_ako(2) as u8;
    let mut h = b"OpusHead\x01".to_vec();
    h.push(kanalov);
    h.extend(312u16.to_le_bytes());
    h.extend(48_000u32.to_le_bytes());
    h.extend([0, 0, 0]);
    let mut d = ogg_stranka(2, 0, 0, &[h]);
    let polozka = b"R128_TRACK_GAIN=-1";
    let mut t = b"OpusTags\x01\x00\x00\x00x\x01\x00\x00\x00".to_vec();
    t.extend((polozka.len() as u32).to_le_bytes());
    t.extend(polozka);
    d.extend(ogg_stranka(0, 0, 1, &[t]));
    let mut granula = 0i64;
    for s in 0..stranok {
        let pakety: Vec<Vec<u8>> = (0..1 + n.mensie_ako(4))
            .map(|_| {
                let toc = (n.dalsie() as u8 & 0xFC) | if kanalov == 2 { 4 } else { 0 };
                std::iter::once(toc).chain(bajty(n, 60)).collect()
            })
            .collect();
        granula += pakety.len() as i64 * 960;
        let posledna = s + 1 == stranok;
        d.extend(ogg_stranka(if posledna { 4 } else { 0 }, granula, s as u32 + 2, &pakety));
    }
    d
}

#[test]
fn ogg_subory() {
    use orchidea_media::zvuk::dekoder_zvuku;
    let mut n = nahoda(19);
    let zaklady: Vec<Vec<u8>> = (0..4).map(|i| subor_ogg(&mut n, 5 + 10 * i)).collect();
    for i in 0..pocet_pokusov() / 10 {
        let d = if i % 8 == 0 { zaklady[i % 4].clone() } else { zmutuj(&zaklady[i % 4], &mut n) };
        let Ok(mut k) = orchidea_media::kontajner::otvor(PametovyZdroj::novy(d), prisne()) else {
            continue;
        };
        let _ = (k.trvanie_ns(), k.znacky().map(|z| z.replaygain()));
        let mut dek = k.stopy().first().and_then(|s| dekoder_zvuku(s).ok());
        let mut blok = BlokZvuku::f32(48_000, RozlozenieKanalov::STEREO);
        for _ in 0..300 {
            match k.dalsi_paket() {
                Ok(Some(p)) => {
                    assert!(p.data.len() as u64 <= prisne().paket);
                    if let Some(dek) = dek.as_mut() {
                        let _ = dek.dekoduj(&p, &mut blok);
                    }
                }
                _ => break,
            }
        }
        let _ = k.posun(n.mensie_ako(2_000_000_000) as i64, Posun::Presny);
        for _ in 0..3 {
            let _ = k.dalsi_paket();
        }
    }
}

/// Zapisovač bitov od najvyššieho (rámce AC-3 vo fuzzeri).
struct ZapisBitov {
    d: Vec<u8>,
    bitov: usize,
}

impl ZapisBitov {
    fn zapis(&mut self, hodnota: u32, n: u32) {
        for i in (0..n).rev() {
            if self.bitov.is_multiple_of(8) {
                self.d.push(0);
            }
            if (hodnota >> i) & 1 != 0 {
                *self.d.last_mut().expect("bajt") |= 0x80 >> (self.bitov & 7);
            }
            self.bitov += 1;
        }
    }
}

/// Syntakticky platný rámec AC-3 (A/52 tabuľky 5.1–5.3): náhodný režim kanálov, väzba, rematrix, stratégie
/// exponentov, parametre rozdelenia bitov a delta. Pri `ticho` sú všetky SNR offsety nulové – bap = 0 (§7.2.2.1.1),
/// mantisy nemajú bity a všetkých 6 blokov je platných; inak je platný blok 0 a ďalej náhodné bity.
fn ramec_ac3(n: &mut Nahoda, ticho: bool) -> Vec<u8> {
    let fscod = n.mensie_ako(3) as u32;
    // tiché rámce (6 platných blokov) potrebujú miesto – vyššie toky
    let frmsizecod = if ticho { 24 + n.mensie_ako(14) as u32 } else { n.mensie_ako(38) as u32 };
    let acmod = n.mensie_ako(8) as u32;
    let lfeon = n.mensie_ako(2) as u32;
    let nfchans = [2usize, 1, 2, 3, 3, 4, 4, 5][acmod as usize];
    // tabuľka 5.18: 48 kHz 2 slová na kb/s, 32 kHz 3, 44,1 kHz ⌊tok · 1536 / 16 / 44,1⌋ + nepárny kód
    let tok = [32u32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 576, 640]
        [frmsizecod as usize / 2];
    let slov = match fscod {
        0 => tok * 2,
        1 => tok * 1536 * 10 / 16 / 441 + (frmsizecod & 1),
        _ => tok * 3,
    };
    let mut w = ZapisBitov { d: Vec::new(), bitov: 0 };
    w.zapis(0x0B77, 16);
    w.zapis(0, 16);
    w.zapis(fscod, 2);
    w.zapis(frmsizecod, 6);
    w.zapis(8, 5);
    w.zapis(0, 3);
    w.zapis(acmod, 3);
    if acmod & 1 != 0 && acmod != 1 {
        w.zapis(n.mensie_ako(3) as u32, 2);
    }
    if acmod & 4 != 0 {
        w.zapis(n.mensie_ako(3) as u32, 2);
    }
    if acmod == 2 {
        w.zapis(n.mensie_ako(4) as u32, 2);
    }
    w.zapis(lfeon, 1);
    for _ in 0..if acmod == 0 { 2 } else { 1 } {
        w.zapis(n.mensie_ako(32) as u32, 5);
        w.zapis(0, 3); // compre, langcode, audprodie
    }
    w.zapis(0, 5); // copyrightb, origbs, timecod1e, timecod2e, addbsie
    let (mut cplbegf, mut cplendf) = (0u32, 0u32);
    for blk in 0..6 {
        if blk > 0 && !ticho {
            break;
        }
        for _ in 0..nfchans * 2 {
            w.zapis(n.mensie_ako(2) as u32, 1); // blksw, dithflag
        }
        let dynrnge = n.mensie_ako(2) as u32;
        w.zapis(dynrnge, 1);
        if dynrnge == 1 {
            w.zapis(n.mensie_ako(256) as u32, 8);
        }
        if acmod == 0 {
            w.zapis(0, 1);
        }
        w.zapis(1, 1); // cplstre
        let cplinu = acmod >= 2 && n.mensie_ako(2) == 1;
        w.zapis(cplinu as u32, 1);
        if cplinu {
            for _ in 0..nfchans {
                w.zapis(1, 1);
            }
            if acmod == 2 {
                w.zapis(1, 1); // phsflginu
            }
            cplbegf = n.mensie_ako(8) as u32;
            cplendf = cplbegf + n.mensie_ako(16 - cplbegf as u64) as u32;
            w.zapis(cplbegf, 4);
            w.zapis(cplendf, 4);
            let mut ncplbnd = 1;
            for _ in 1..3 + cplendf - cplbegf {
                let b = n.mensie_ako(2) as u32;
                ncplbnd += 1 - b;
                w.zapis(b, 1);
            }
            for _ in 0..nfchans {
                w.zapis(1, 1); // cplcoe
                w.zapis(n.mensie_ako(4) as u32, 2);
                for _ in 0..ncplbnd {
                    w.zapis(n.mensie_ako(256) as u32, 8);
                }
            }
            if acmod == 2 {
                for _ in 0..ncplbnd {
                    w.zapis(n.mensie_ako(2) as u32, 1);
                }
            }
        }
        if acmod == 2 {
            w.zapis(1, 1); // rematstr
            let pasiem = if !cplinu || cplbegf > 2 {
                4
            } else if cplbegf > 0 {
                3
            } else {
                2
            };
            for _ in 0..pasiem {
                w.zapis(n.mensie_ako(2) as u32, 1);
            }
        }
        let strategia = |n: &mut Nahoda| 1 + n.mensie_ako(3) as u32;
        let cplstrat = if cplinu { strategia(n) } else { 0 };
        if cplinu {
            w.zapis(cplstrat, 2);
        }
        let strat: Vec<u32> = (0..nfchans).map(|_| strategia(n)).collect();
        for &s in &strat {
            w.zapis(s, 2);
        }
        if lfeon == 1 {
            w.zapis(1, 1);
        }
        let mut endmant = vec![37 + 12 * cplbegf as usize; nfchans];
        if !cplinu {
            for e in endmant.iter_mut() {
                let chbwcod = n.mensie_ako(61) as usize;
                w.zapis(chbwcod as u32, 6);
                *e = (chbwcod + 12) * 3 + 37;
            }
        }
        if cplinu {
            w.zapis(n.mensie_ako(16) as u32, 4);
            let skup = [0, 3, 6, 12][cplstrat as usize];
            for _ in 0..(12 * (3 + cplendf - cplbegf) as usize) / skup {
                w.zapis(62, 7); // tri rozdiely 0
            }
        }
        for (ch, &s) in strat.iter().enumerate() {
            w.zapis(n.mensie_ako(16) as u32, 4);
            let e = endmant[ch];
            let ngrps = match s {
                1 => (e - 1) / 3,
                2 => (e - 1 + 3) / 6,
                _ => (e - 1 + 9) / 12,
            };
            for _ in 0..ngrps {
                w.zapis(62, 7);
            }
            w.zapis(0, 2); // gainrng
        }
        if lfeon == 1 {
            w.zapis(n.mensie_ako(16) as u32, 4);
            w.zapis(62, 7);
            w.zapis(62, 7);
        }
        w.zapis(1, 1); // baie
        w.zapis(n.mensie_ako(2048) as u32, 11);
        w.zapis(1, 1); // snroffste
        let snr = |n: &mut Nahoda, bitov: u32| if ticho { 0 } else { n.mensie_ako(1 << bitov) as u32 };
        let c = snr(n, 6);
        w.zapis(c, 6);
        for _ in 0..nfchans + cplinu as usize + lfeon as usize {
            let f = snr(n, 4);
            w.zapis(f, 4);
            w.zapis(n.mensie_ako(8) as u32, 3);
        }
        if cplinu {
            w.zapis(1, 1);
            w.zapis(n.mensie_ako(64) as u32, 6);
        }
        let deltbaie = n.mensie_ako(2) as u32;
        w.zapis(deltbaie, 1);
        if deltbaie == 1 {
            for _ in 0..nfchans + cplinu as usize {
                w.zapis(1, 2); // nové údaje: jeden segment
                w.zapis(0, 3);
                w.zapis(n.mensie_ako(32) as u32, 5);
                w.zapis(n.mensie_ako(16) as u32, 4);
                w.zapis(n.mensie_ako(8) as u32, 3);
            }
        }
        w.zapis(0, 1); // skiple
    }
    let mut d = w.d;
    let dlzka = slov as usize * 2;
    while d.len() < dlzka {
        d.push(n.dalsie() as u8);
    }
    d.truncate(dlzka.max(8));
    d
}

#[test]
fn ac3_ramce() {
    use orchidea_media::ac3::DekoderAc3;
    let mut n = nahoda(23);
    let mut dek = DekoderAc3::novy();
    dek.kontrola_crc = false;
    let (mut uspesnych, mut tichych) = (0, 0);
    for i in 0..pocet_pokusov() {
        let ticho = i % 2 == 0;
        let zaklad = ramec_ac3(&mut n, ticho);
        let d = if i % 4 < 2 { zaklad } else { zmutuj(&zaklad, &mut n) };
        if let Ok(k) = dek.dekoduj_ramec(&d) {
            assert!(k.iter().all(|kanal| kanal.len() == 1536 && kanal.iter().all(|v| v.is_finite())));
            uspesnych += 1;
            tichych += ticho as usize;
        }
        if i % 97 == 0 {
            dek.reset();
        }
    }
    // generátor musí naozaj vytvárať platné rámce (inak by fuzzer skúšal len odmietnutie hlavičky)
    assert!(tichych * 5 > pocet_pokusov() / 4, "platných rámcov {uspesnych}, tichých {tichych}");
}

#[test]
fn surovy_ac3_subor() {
    use orchidea_media::zvuk::dekoder_zvuku;
    let mut n = nahoda(24);
    // súbor .ac3 z platných rámcov generátora (aj s CRC – dekodér ich kontroluje), s odpadom pred prvým rámcom
    let zaklady: Vec<Vec<u8>> = (0..4)
        .map(|i| {
            let mut d = bajty(&mut n, 40 * i as u64);
            for _ in 0..(6 + 4 * i) {
                d.extend(ramec_ac3(&mut n, i % 2 == 0));
            }
            d
        })
        .collect();
    for i in 0..pocet_pokusov() / 10 {
        let d = if i % 8 == 0 { zaklady[i % 4].clone() } else { zmutuj(&zaklady[i % 4], &mut n) };
        let Ok(mut k) = orchidea_media::kontajner::otvor(PametovyZdroj::novy(d), Limity::default()) else {
            continue;
        };
        let _ = k.trvanie_ns();
        let mut dek = k.stopy().first().and_then(|s| dekoder_zvuku(s).ok());
        let mut blok = BlokZvuku::f32(48_000, RozlozenieKanalov::STEREO);
        let mut predosly = i64::MIN;
        for _ in 0..200 {
            match k.dalsi_paket() {
                Ok(Some(p)) => {
                    // čas stúpa (aj po strate synchronizácie) a paket nie je prázdny
                    let t = p.pts.unwrap_or(0);
                    assert!(t > predosly && !p.data.is_empty());
                    predosly = t;
                    if let Some(dek) = dek.as_mut() {
                        let _ = dek.dekoduj(&p, &mut blok);
                    }
                }
                _ => break,
            }
        }
        let _ = k.posun(n.mensie_ako(2_000_000_000) as i64, Posun::Presny);
        for _ in 0..3 {
            let _ = k.dalsi_paket();
        }
    }
}

#[test]
fn prehravanie_stroj() {
    use orchidea_media::prehravanie::{Prehravanie, VolbyPrehravania};
    use orchidea_media::r5::{FormatVzoriekHdmi, MeranieZvuku, Obnova, SchopnostiHdmi};
    let mut n = nahoda(25);
    let mut zaklady = zaklady_matrosky();
    zaklady.extend(subory_flac());
    zaklady.push(subor_mpa(&mut n, 40));
    zaklady.push(subor_ogg(&mut n, 6));
    let schopnosti = SchopnostiHdmi {
        formaty: vec![FormatVzoriekHdmi::S16Le],
        max_kanalov: 2,
        frekvencie: vec![32_000, 44_100, 48_000, 96_000],
        max_bitov: 16,
        priechod_ac3: true,
        priechod_eac3: true,
        oneskorenie_tv_ns: None,
    };
    let mut otvorenych = 0;
    for i in 0..pocet_pokusov() / 20 {
        let z = &zaklady[i % zaklady.len()];
        let d = if i % 8 == 0 { z.clone() } else { zmutuj(z, &mut n) };
        let Ok(k) = orchidea_media::kontajner::otvor(PametovyZdroj::novy(d), prisne()) else { continue };
        let Ok(mut p) = Prehravanie::otvor(k, &schopnosti, &VolbyPrehravania::default()) else { continue };
        otvorenych += 1;
        let (mut t, mut odovzdanych) = (0i64, 0u64);
        for krok in 0..300 {
            t += 16_666_667;
            if let Ok(Some(v)) = p.dalsi_paket_videa() {
                let _ = p.snimka(v.id);
                let _ = p.snimka(v.id.wrapping_add(n.mensie_ako(3)));
            }
            let _ = p.obnova(Obnova { monotonny_ns: t, perioda_ns: 16_666_667 });
            if let Ok(Some(z)) = p.dalsi_zvuk() {
                odovzdanych += z.ramcov as u64;
                p.meranie_zvuku(MeranieZvuku {
                    odovzdanych_ramcov: odovzdanych,
                    oneskorenie_ramcov: n.mensie_ako(odovzdanych + 1),
                    monotonny_ns: t + n.mensie_ako(4_000_000) as i64 - 2_000_000,
                });
            }
            if krok % 97 == 50 {
                let sposob = if n.mensie_ako(2) == 0 { Posun::KlucovyPred } else { Posun::Presny };
                let _ = p.posun(n.mensie_ako(4_000_000_000) as i64 - 500_000_000, sposob);
                odovzdanych = 0;
            }
            match krok % 131 {
                70 => p.pauza(t),
                90 => p.pokracuj(t),
                _ => {}
            }
            let _ = p.stav(t);
        }
    }
    assert!(otvorenych > 0, "stroj sa musí otvoriť aspoň pre časť súborov");
}
