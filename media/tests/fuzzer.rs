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
fn mpa_vrstva3() {
    use orchidea_media::mpa::DekoderMpa;
    let mut n = nahoda(17);
    let mut d = DekoderMpa::novy();
    for i in 0..pocet_pokusov() / 20 {
        // hlavička: MPEG-1, vrstva III, bez CRC / s CRC, toky 32–320 kb/s, 44,1/48/32 kHz, všetky režimy
        let tok = 1 + (n.dalsie() % 14) as u8;
        let f = (n.dalsie() % 3) as u8;
        let rezim = (n.dalsie() % 4) as u8;
        let mut p = vec![
            0xFF,
            0xFA | (i % 2) as u8,
            (tok << 4) | (f << 2),
            (rezim << 6) | ((n.dalsie() % 4) as u8) << 4,
        ];
        p.extend(bajty(&mut n, 1500));
        // celé dekódovanie: rozbor, spätná kvantizácia, stereo, preradenie, banka filtrov – žiadny pád,
        // výstup vždy konečný (aj pri nezmyselných ziskoch a polohách intenzity)
        if let Ok(pcm) = d.dekoduj_ramec(&p) {
            assert!(pcm.iter().all(|k| k.len() == 1152 && k.iter().all(|v| v.is_finite())));
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
