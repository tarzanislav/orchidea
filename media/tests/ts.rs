//! Skúšky čítačky MPEG-TS a M2TS na súboroch z vlastného zapisovača (`spolocne::ts`).

mod spolocne;

use orchidea_media::ts::Ts;
use orchidea_media::{FormatKontajnera, Kodek, Kontajner, Limity, Paket, PametovyZdroj, Posun, otvor};
use spolocne::ts::*;

fn vsetky(m: &mut impl Kontajner) -> Vec<Paket> {
    std::iter::from_fn(|| m.dalsi_paket().unwrap()).collect()
}

fn otvor_ts(d: Vec<u8>) -> Ts<PametovyZdroj> {
    Ts::otvor(PametovyZdroj::novy(d), Limity::default()).unwrap()
}

const START: u64 = 126_000;

/// Tiky 90 kHz na ns (k najbližšiemu, ako `cas::na_ns`).
fn ns(tik: u64) -> i64 {
    ((tik as i128 * 100_000 + 4) / 9) as i64
}

fn trvanie_okolo(m: &impl Kontajner, sekund: i64) {
    let t = m.trvanie_ns().unwrap();
    assert!((t - sekund * 1_000_000_000).abs() < 100_000_000, "trvanie {t}");
}

#[test]
fn typicky_subor_188_192_204() {
    for velkost in [188usize, 192, 204] {
        let (d, dlzky) = typicky(velkost, 3, START);
        let mut m = otvor_ts(d);
        assert_eq!(m.format(), FormatKontajnera::MpegTs { velkost_paketu: velkost as u16 });
        let s = m.stopy();
        assert_eq!((s[0].kodek.clone(), s[1].kodek.clone()), (Kodek::H264, Kodek::Aac));
        assert_eq!((s[0].cislo, s[1].cislo), (0x100, 0x101));
        assert_eq!(s[1].jazyk.as_deref(), Some("slk"));
        let z = s[1].zvuk.unwrap();
        assert_eq!((z.frekvencia, z.kanalov), (48_000, 2));
        // trvanie: posledný PTS − prvý DTS
        trvanie_okolo(&m, 3);
        let p = vsetky(&mut m);
        let v: Vec<_> = p.iter().filter(|p| p.stopa == 0).collect();
        let a: Vec<_> = p.iter().filter(|p| p.stopa == 1).collect();
        assert_eq!(v.len(), 75);
        assert_eq!(a.len(), dlzky.len());
        for (k, p) in v.iter().enumerate() {
            assert_eq!(p.dts, Some((START + k as u64 * 3600) as i64));
            assert_eq!(p.pts, Some((START + 3600 + k as u64 * 3600) as i64));
            assert_eq!(p.klucovy, k % 25 == 0, "snímka {k}");
            assert_eq!(p.data, h264(k % 25 == 0, 300 + (k % 7) * 50, k as u8));
            assert!(!p.prerusenie);
        }
        // rámce zvuku cez hranice PES: každý rámec celý, čas presne podľa počtu vzoriek
        for (k, p) in a.iter().enumerate() {
            assert_eq!(p.data, adts(dlzky[k], k as u8), "rámec {k}");
            assert_eq!(p.pts, Some((START + 3600 + k as u64 * 1920) as i64), "rámec {k}");
            assert_eq!(p.trvanie, Some(1920));
            assert!(p.klucovy && !p.prerusenie);
        }
    }
}

#[test]
fn rozbalenie_33_bitov() {
    // začiatok 2 s pred pretečením 33-bitových hodín
    let zaciatok = (1u64 << 33) - 180_000;
    let (d, _) = typicky(188, 4, zaciatok);
    let mut m = otvor_ts(d);
    trvanie_okolo(&m, 4);
    let p = vsetky(&mut m);
    let v: Vec<i64> = p.iter().filter(|p| p.stopa == 0).map(|p| p.dts.unwrap()).collect();
    assert_eq!(v.len(), 100);
    assert!(v.windows(2).all(|w| w[1] - w[0] == 3600), "čas rastie aj cez 2^33");
    assert_eq!(*v.last().unwrap(), (zaciatok + 99 * 3600) as i64);
    // posun za pretečenie: IDR snímky 75 (PTS = začiatok + 3600 + 3 s)
    let idr = zaciatok + 3600 + 3 * 90_000;
    assert_eq!(m.posun(ns(idr) + 1, Posun::KlucovyPred).unwrap(), ns(idr));
    let v = vsetky(&mut m);
    assert_eq!(v.iter().find(|p| p.stopa == 0).unwrap().pts, Some(idr as i64));
}

#[test]
fn posun_na_kluc() {
    let (d, _) = typicky(192, 6, START);
    let mut m = otvor_ts(d);
    // IDR každú sekundu: PTS snímky 25·k je START + 3600 + k·90 000
    for (ciel_s, idr) in [(0u64, 0u64), (1, 1), (2, 2), (3, 3), (5, 5), (100, 5)] {
        let ciel = ns(START + 3600 + ciel_s * 90_000) + 1;
        let t = m.posun(ciel, Posun::KlucovyPred).unwrap();
        assert_eq!(t, ns(START + 3600 + idr * 90_000), "cieľ {ciel_s} s");
        let p = vsetky(&mut m);
        let v: Vec<_> = p.iter().filter(|p| p.stopa == 0).collect();
        assert!(v[0].klucovy && v[0].prerusenie);
        assert_eq!(v[0].pts, Some((START + 3600 + idr * 90_000) as i64));
        assert_eq!(v.len() as u64, 150 - idr * 25);
        // zvuk pokračuje od nasledujúceho začiatku PES – len celé rámce
        let a: Vec<_> = p.iter().filter(|p| p.stopa == 1).collect();
        assert!(a[0].prerusenie && a.iter().all(|p| p.data.len() >= 200));
    }
    // pred začiatkom súboru: od začiatku (prvý DTS)
    assert_eq!(m.posun(0, Posun::Presny).unwrap(), ns(START));
    assert_eq!(vsetky(&mut m).iter().filter(|p| p.stopa == 0).count(), 150);
}

#[test]
fn strata_synchronizacie_a_kontinuity() {
    let (d, dlzky) = typicky(188, 3, START);
    // vložený odpad uprostred (nie násobok 188) a vynechaný transportný paket
    let mut p = d[..188 * 200].to_vec();
    p.extend_from_slice(&[0x47, 1, 2, 3, 0x47, 9, 9]);
    p.extend_from_slice(&d[188 * 200..188 * 400]);
    p.extend_from_slice(&d[188 * 401..]);
    let mut m = otvor_ts(p);
    let pakety = vsetky(&mut m);
    let v = pakety.iter().filter(|p| p.stopa == 0).count();
    let a = pakety.iter().filter(|p| p.stopa == 1).count();
    assert!((72..=75).contains(&v), "video {v}");
    assert!(a + 6 >= dlzky.len() && a <= dlzky.len(), "zvuk {a} z {}", dlzky.len());
    assert!(pakety.iter().filter(|p| p.prerusenie).count() >= 2);
    // všetky rámce zvuku sú celé a pôvodné (rámec s chýbajúcimi údajmi sa zahodí alebo preskočí)
    for p in pakety.iter().filter(|p| p.stopa == 1) {
        let k = (p.pts.unwrap() as u64 - START - 3600) / 1920;
        assert_eq!(p.data, adts(dlzky[k as usize], k as u8));
    }
}

#[test]
fn ac3_eac3_mp2_a_opus() {
    let mut z = Zapisovac::novy(188);
    z.pat(&[(1, 0x1000)]);
    z.pmt(
        0x1000,
        1,
        &[],
        &[
            (0x81, 0x101, vec![]),
            (0x06, 0x102, vec![0x7A, 1, 0]),
            (0x03, 0x103, vec![]),
            (0x06, 0x104, vec![0x05, 4, b'O', b'p', b'u', b's', 0x7F, 2, 0x80, 2]),
        ],
    );
    for k in 0..10u64 {
        let t = 90_000 + k * 2880;
        let a = [ac3(k as u8), ac3(k as u8 + 100)].concat();
        z.pes(0x101, 0xBD, Some(t), None, false, &a, true);
        // E-AC-3: nezávislý rámec a jeho závislý podtok → jeden paket
        let e = [eac3(400, false, 1), eac3(200, true, 2), eac3(400, false, 3), eac3(200, true, 4)].concat();
        z.pes(0x102, 0xBD, Some(t), None, false, &e, true);
        z.pes(0x103, 0xC0, Some(t), None, false, &[mp2(1), mp2(2)].concat(), true);
        // Opus: dve jednotky po 20 ms, prvá jednotka súboru s orezaním začiatku
        let mut o = Vec::new();
        for j in 0..2 {
            let paket = [vec![15 << 3], vec![k as u8; 40]].concat();
            if k == 0 && j == 0 {
                o.extend_from_slice(&[0x7F, 0xF0, paket.len() as u8, 0x01, 0x38]);
            } else {
                o.extend_from_slice(&[0x7F, 0xE0, paket.len() as u8]);
            }
            o.extend_from_slice(&paket);
        }
        z.pes(0x104, 0xBD, Some(t), None, false, &o, true);
    }
    let mut m = otvor_ts(z.data);
    let s = m.stopy();
    assert_eq!(
        s.iter().map(|s| s.kodek.clone()).collect::<Vec<_>>(),
        [Kodek::Ac3, Kodek::Eac3, Kodek::MpegAudio { vrstva: 2 }, Kodek::Opus]
    );
    assert_eq!(s[3].zvuk.unwrap().kanalov, 2);
    assert_eq!(&s[3].konfiguracia[..8], b"OpusHead");
    let p = vsetky(&mut m);
    let z: Vec<Vec<&Paket>> = (0..4).map(|i| p.iter().filter(|p| p.stopa == i).collect()).collect();
    assert_eq!(z.iter().map(|x| x.len()).collect::<Vec<_>>(), [20, 20, 20, 20]);
    assert_eq!(z[0][1].pts, Some(90_000 + 2880));
    assert_eq!(z[0][1].data, ac3(100));
    assert_eq!(z[1][0].data.len(), 600);
    assert_eq!(z[1][1].pts, Some(90_000 + 2880));
    assert_eq!(z[2][1].pts, Some(90_000 + 2160));
    assert_eq!(z[2][1].trvanie, Some(2160));
    assert_eq!((z[3][0].orezat_zaciatok, z[3][1].orezat_zaciatok), (312, 0));
    assert_eq!(z[3][1].pts, Some(90_000 + 1800));
    assert_eq!(z[3][1].data.len(), 41);
}

#[test]
fn otvorenie_podla_obsahu_a_poskodenie() {
    let (d, _) = typicky(192, 2, START);
    let m = otvor(PametovyZdroj::novy(d.clone()), Limity::default()).unwrap();
    assert_eq!(m.format(), FormatKontajnera::MpegTs { velkost_paketu: 192 });
    // skrátené a poškodené súbory: nikdy pád
    for dlzka in [0usize, 100, 192, 193, 192 * 3, 192 * 50 + 7, d.len() / 2] {
        if let Ok(mut m) = Ts::otvor(PametovyZdroj::novy(d[..dlzka].to_vec()), Limity::default()) {
            while let Ok(Some(_)) = m.dalsi_paket() {}
        }
    }
    let mut zle = d.clone();
    for i in (0..zle.len()).step_by(997) {
        zle[i] ^= 0x5A;
    }
    if let Ok(mut m) = Ts::otvor(PametovyZdroj::novy(zle), Limity::default()) {
        while let Ok(Some(_)) = m.dalsi_paket() {}
        let _ = m.posun(1_000_000_000, Posun::Presny);
        while let Ok(Some(_)) = m.dalsi_paket() {}
    }
    // bez PAT
    let mut z = Zapisovac::novy(188);
    z.pes(0x100, 0xE0, Some(0), None, true, &h264(true, 500, 1), false);
    assert!(Ts::otvor(PametovyZdroj::novy(z.data), Limity::default()).is_err());
}
