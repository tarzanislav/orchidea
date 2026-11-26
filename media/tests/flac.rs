//! Skúšky dekodéra FLAC a čítačky súborov FLAC (RFC 9639) na malých súboroch z referenčného kodéra `flac` 1.4.3
//! (`tests/data`, zapísané v zdroje.json; vložené do programu skúšok): bitová zhoda s MD5 v STREAMINFO, posun, FLAC v Matroske.

mod spolocne;

use orchidea_hevc::odtlacok::Md5;
use orchidea_media::flac::FlacDekoder;
use orchidea_media::flac_subor::FlacSubor;
use orchidea_media::mkv::Mkv;
use orchidea_media::{
    BlokZvuku, DekoderZvuku, FormatKontajnera, Kontajner, Limity, PametovyZdroj, Posun, RozlozenieKanalov,
    VzorkyZvuku,
};

const SUBORY: [&str; 3] = ["stereo16_lpc.flac", "kanaly6_24b.flac", "mono8_pevny.flac"];

/// Súbory sú vložené do programu skúšok – bežia rovnako vo WebAssembly, na Androide aj v QEMU.
fn subor(meno: &str) -> Vec<u8> {
    match meno {
        "stereo16_lpc.flac" => include_bytes!("data/stereo16_lpc.flac").to_vec(),
        "kanaly6_24b.flac" => include_bytes!("data/kanaly6_24b.flac").to_vec(),
        "mono8_pevny.flac" => include_bytes!("data/mono8_pevny.flac").to_vec(),
        _ => unreachable!("{meno}"),
    }
}

/// MD5 dekódovaného zvuku podľa RFC 9639 (prekladané, malokoncové, ⌈bitov/8⌉ B) a počet rámcov.
fn md5_dekodovania(k: &mut dyn Kontajner) -> ([u8; 16], usize) {
    let mut d = orchidea_media::dekoder_zvuku(&k.stopy()[0]).unwrap();
    let mut b = BlokZvuku::cele(0, RozlozenieKanalov::STEREO, 16);
    let mut md5 = Md5::novy();
    let mut ramcov = 0;
    while let Some(p) = k.dalsi_paket().unwrap() {
        d.dekoduj(&p, &mut b).unwrap();
        ramcov += 1;
        let VzorkyZvuku::Cele { bitov, data } = &b.vzorky else { panic!() };
        let (n, kan) = (b.ramcov, b.kanalov());
        for i in 0..n {
            for c in 0..kan {
                md5.pridaj(&data[c * n + i].to_le_bytes()[..bitov.div_ceil(8) as usize]);
            }
        }
    }
    (md5.vysledok(), ramcov)
}

#[test]
fn bitova_zhoda_s_md5_v_streaminfo() {
    for meno in SUBORY {
        let mut f = FlacSubor::otvor(PametovyZdroj::novy(subor(meno)), Limity::default()).unwrap();
        assert_eq!(f.format(), FormatKontajnera::Flac);
        let ocakavane = f.streaminfo().unwrap().md5;
        let (md5, ramcov) = md5_dekodovania(&mut f);
        assert_eq!(md5, ocakavane, "{meno}");
        assert!(ramcov >= 5, "{meno}: {ramcov}");
    }
}

#[test]
fn parametre_a_rozlozenie() {
    let f = FlacSubor::otvor(PametovyZdroj::novy(subor("kanaly6_24b.flac")), Limity::default()).unwrap();
    let z = f.stopy()[0].zvuk.unwrap();
    assert_eq!((z.frekvencia, z.kanalov, z.bitov), (48_000, 6, Some(24)));
    assert_eq!(
        z.rozlozenie,
        RozlozenieKanalov::S5_1_ZADNE,
        "5.1 vo FLAC má zadné reproduktory (RFC 9639, 9.1.3)"
    );
    assert_eq!(f.trvanie_ns(), Some(50_000_000));
    // otvorenie podľa obsahu, aj so značkou ID3v2 pred súborom
    let mut s = b"ID3\x04\x00\x00\x00\x00\x00\x05ABCDE".to_vec();
    s.extend(subor("mono8_pevny.flac"));
    let mut k = orchidea_media::otvor(PametovyZdroj::novy(s), Limity::default()).unwrap();
    assert_eq!(k.format(), FormatKontajnera::Flac);
    assert!(k.dalsi_paket().unwrap().is_some());
}

#[test]
fn posun_na_ramec() {
    let mut f = FlacSubor::otvor(PametovyZdroj::novy(subor("mono8_pevny.flac")), Limity::default()).unwrap();
    let vsetky: Vec<_> = std::iter::from_fn(|| f.dalsi_paket().unwrap()).collect();
    // bloky po 256 vzoriek pri 8 kHz = 32 ms
    for (ciel_ms, ocak) in [(0i64, 0i64), (100, 96), (200, 192), (10_000, (vsetky.len() as i64 - 1) * 32)] {
        let t = f.posun(ciel_ms * 1_000_000, Posun::Presny).unwrap();
        assert_eq!(t, ocak * 1_000_000, "cieľ {ciel_ms} ms");
        let p = f.dalsi_paket().unwrap().unwrap();
        assert!(p.prerusenie);
        assert_eq!(p.pts, Some(ocak * 8));
        let i = vsetky.iter().position(|v| v.pts == p.pts).unwrap();
        assert_eq!(p.data, vsetky[i].data);
    }
}

#[test]
fn flac_v_matroske() {
    use spolocne::*;
    // rámce zo súboru FLAC do Matrosky (CodecPrivate = „fLaC“ + STREAMINFO) – výsledok musí mať to isté MD5
    let mut f = FlacSubor::otvor(PametovyZdroj::novy(subor("stereo16_lpc.flac")), Limity::default()).unwrap();
    let md5 = f.streaminfo().unwrap().md5;
    let konfig = f.stopy()[0].konfiguracia.clone();
    let ramce: Vec<_> = std::iter::from_fn(|| f.dalsi_paket().unwrap()).collect();
    let bloky: Vec<_> =
        ramce.iter().enumerate().map(|(i, r)| simple(1, (i * 26) as i16, true, &r.data)).collect();
    let obsah = spoj(&[
        info(1_000_000, 120.0),
        stopy(&[stopa(1, 2, "A_FLAC", &[prvok(0x63A2, &konfig), zvuk(44100.0, 2)])]),
        zhluk(0, &bloky),
    ]);
    let mut m =
        Mkv::otvor(PametovyZdroj::novy(spoj(&[ebml("matroska"), segment(&obsah)])), Limity::default())
            .unwrap();
    let (nase, ramcov) = md5_dekodovania(&mut m);
    assert_eq!((nase, ramcov), (md5, ramce.len()));
}

#[test]
fn poskodeny_ramec_je_chyba_nie_pad() {
    let mut f = FlacSubor::otvor(PametovyZdroj::novy(subor("stereo16_lpc.flac")), Limity::default()).unwrap();
    let mut d = FlacDekoder::novy(&f.stopy()[0].konfiguracia, None).unwrap();
    let p = f.dalsi_paket().unwrap().unwrap();
    let mut b = BlokZvuku::cele(0, RozlozenieKanalov::STEREO, 16);
    for i in 0..p.data.len() {
        let mut zly = p.clone();
        zly.data[i] ^= 0x10;
        // CRC-16 rámca zachytí každú zmenu jedného bajtu
        assert!(d.dekoduj(&zly, &mut b).is_err(), "bajt {i}");
    }
    assert!(d.dekoduj(&p, &mut b).is_ok());
}
