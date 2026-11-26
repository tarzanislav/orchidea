//! Skúšky čítačky MP4/MOV na súboroch z vlastného zapisovača (`spolocne::mp4`).

mod spolocne;

use orchidea_media::mp4::Mp4;
use orchidea_media::{FormatKontajnera, Kodek, Kontajner, Limity, Paket, PametovyZdroj, Posun};
use spolocne::mp4::*;

fn vsetky(m: &mut impl Kontajner) -> Vec<Paket> {
    std::iter::from_fn(|| m.dalsi_paket().unwrap()).collect()
}

#[test]
fn typicky_subor_vsetky_varianty() {
    for (na_konci, co64) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut m = Mp4::otvor(PametovyZdroj::novy(typicky(2, na_konci, co64)), Limity::default()).unwrap();
        assert_eq!(m.format(), FormatKontajnera::Mp4);
        assert_eq!(m.trvanie_ns(), Some(3_000_000_000));
        let s = m.stopy();
        assert_eq!((s[0].kodek.clone(), s[1].kodek.clone()), (Kodek::H264, Kodek::Aac));
        assert_eq!(s[0].video.unwrap().sirka, 320);
        assert_eq!(s[0].jazyk.as_deref(), Some("und"));
        let p = vsetky(&mut m);
        let v: Vec<_> = p.iter().filter(|p| p.stopa == 0).collect();
        let a: Vec<_> = p.iter().filter(|p| p.stopa == 1).collect();
        assert_eq!((v.len(), a.len()), (50, 94));
        // ISO/IEC 14496-12: PTS = DTS + posun − media_time zoznamu úprav
        assert_eq!(v[..4].iter().map(|p| p.pts.unwrap()).collect::<Vec<_>>(), [0, 1536, 512, 1024]);
        assert_eq!(v[..3].iter().map(|p| p.dts.unwrap()).collect::<Vec<_>>(), [-1024, -512, 0]);
        assert_eq!(a[0].pts, Some(-1024));
        assert!(v[0].klucovy && !v[1].klucovy && v[25].klucovy);
        assert_eq!(v[3].data, vec![3u8; 23]);
        // pakety v poradí súboru
        assert!(p.windows(2).all(|w| w[0].pozicia < w[1].pozicia));
    }
}

#[test]
fn posun_na_klucovu_snimku() {
    let mut m = Mp4::otvor(PametovyZdroj::novy(typicky(4, false, false)), Limity::default()).unwrap();
    for (ciel_ms, ocak_ms) in [(0i64, 0i64), (1_500, 1_000), (3_990, 3_000), (9_999, 3_000)] {
        let t = m.posun(ciel_ms * 1_000_000, Posun::KlucovyPred).unwrap();
        assert_eq!(t, ocak_ms * 1_000_000, "cieľ {ciel_ms}");
        let p = vsetky(&mut m);
        let v = p.iter().find(|p| p.stopa == 0).unwrap();
        assert!(v.klucovy && v.prerusenie);
        assert_eq!(v.pts.unwrap() * 1000 / 12800, ocak_ms);
        // zvuk začne najneskôr s obrazom
        let a = p.iter().find(|p| p.stopa == 1).unwrap();
        assert!(a.dts.unwrap() * 1000 / 48000 <= ocak_ms);
    }
}

#[test]
fn quicktime_pcm_v_paketoch_20_ms() {
    let vzorky = (0..48_000u32).map(|i| ((i as u16).to_le_bytes().to_vec(), 1, 0, true)).collect();
    let d = subor(
        &[StopaMp4 {
            id: 1,
            video: false,
            mierka: 48_000,
            vzorky,
            vzoriek_v_kusku: 24_000,
            uprava: None,
            zaznam: zvuk(b"sowt", 1, 16, 48_000, &[]),
        }],
        false,
        false,
        true,
    );
    let mut m = Mp4::otvor(PametovyZdroj::novy(d), Limity::default()).unwrap();
    assert_eq!(m.format(), FormatKontajnera::QuickTime);
    assert!(matches!(m.stopy()[0].kodek, Kodek::Pcm(_)));
    let p = vsetky(&mut m);
    // 48 000 vzoriek po 960 (20 ms) = 50 paketov, nie 48 000
    assert_eq!(p.len(), 50);
    assert_eq!(p[1].pts, Some(960));
    assert_eq!(p.iter().map(|p| p.data.len()).sum::<usize>(), 96_000);
    assert_eq!(&p[0].data[..4], &[0, 0, 1, 0]);
}

#[test]
fn prazdna_uprava_a_velky_box() {
    let vzorky = (0..10).map(|k| (vec![k as u8], 100, 0, true)).collect();
    let d = subor(
        &[StopaMp4 {
            id: 7,
            video: true,
            mierka: 1000,
            vzorky,
            vzoriek_v_kusku: 3,
            uprava: Some((0, 500)),
            zaznam: avc1(),
        }],
        false,
        false,
        false,
    );
    let mut m = Mp4::otvor(PametovyZdroj::novy(d.clone()), Limity::default()).unwrap();
    let p = vsetky(&mut m);
    // prázdna úprava 500 ms posunie začiatok dopredu
    assert_eq!(p[0].pts, Some(500));
    assert_eq!(p[9].pts, Some(1400));
    // mdat s 64-bitovou veľkosťou na konci súboru
    let mut x = d[..d.len()].to_vec();
    x.extend(bx64(b"free", &[0; 5]));
    assert_eq!(vsetky(&mut Mp4::otvor(PametovyZdroj::novy(x), Limity::default()).unwrap()).len(), 10);
}

#[test]
fn skratene_a_poskodene_nikdy_pad() {
    let d = typicky(1, true, false);
    for n in (0..d.len()).step_by(5) {
        if let Ok(mut m) = Mp4::otvor(PametovyZdroj::novy(d[..n].to_vec()), Limity::default()) {
            while let Ok(Some(_)) = m.dalsi_paket() {}
        }
    }
    // stsz tvrdí 2^30 vzoriek v malom súbore: chyba, nie alokácia
    let mut z = d.clone();
    let i = z.windows(4).position(|w| w == b"stsz").unwrap();
    z[i + 12..i + 16].copy_from_slice(&(1u32 << 30).to_be_bytes());
    assert!(Mp4::otvor(PametovyZdroj::novy(z), Limity::default()).is_err());
}

#[test]
fn otvorenie_podla_obsahu() {
    let mut k =
        orchidea_media::otvor(PametovyZdroj::novy(typicky(1, false, false)), Limity::default()).unwrap();
    assert_eq!(k.format(), FormatKontajnera::Mp4);
    assert!(k.dalsi_paket().unwrap().is_some());
}

#[test]
fn fragmenty() {
    let mut m = Mp4::otvor(PametovyZdroj::novy(fragmentovany(4)), Limity::default()).unwrap();
    let p = vsetky(&mut m);
    assert_eq!(p.len(), 20);
    assert_eq!(
        p.iter().map(|p| p.pts.unwrap()).collect::<Vec<_>>()[..7],
        [0, 512, 1024, 1536, 2048, 2560, 3072]
    );
    assert!(p[0].klucovy && !p[1].klucovy && p[5].klucovy, "príznaky z trun a trex");
    assert_eq!(p[7].data, vec![7u8; 12]);
    // posun na začiatok fragmentov
    m.posun(0, Posun::KlucovyPred).unwrap();
    assert_eq!(vsetky(&mut m).len(), 20);
}
