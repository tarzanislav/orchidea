//! Skúšky čítačky Matroska/WebM na súboroch s presne známym obsahom (vlastný zapisovač v `spolocne`).

mod spolocne;

use orchidea_media::mkv::Mkv;
use orchidea_media::{DruhStopy, FormatKontajnera, Kodek, Kontajner, Limity, Paket, PametovyZdroj, Posun};
use spolocne::*;

fn otvor(d: Vec<u8>) -> Mkv<PametovyZdroj> {
    Mkv::otvor(PametovyZdroj::novy(d), Limity::default()).unwrap()
}

fn vsetky(m: &mut impl Kontajner) -> Vec<Paket> {
    let mut v = Vec::new();
    while let Some(p) = m.dalsi_paket().unwrap() {
        v.push(p);
    }
    v
}

#[test]
fn typicky_subor() {
    let (d, ocak) = typicky(4, true);
    let mut m = otvor(d);
    assert_eq!(m.format(), FormatKontajnera::Matroska);
    assert_eq!(m.trvanie_ns(), Some(4_000_000_000));
    let s = m.stopy();
    assert_eq!(s.len(), 2);
    assert_eq!((s[0].kodek.clone(), s[0].druh, s[0].cislo), (Kodek::H264, DruhStopy::Video, 1));
    let v = s[0].video.unwrap();
    assert_eq!((v.sirka, v.vyska), (320, 240));
    assert_eq!(v.snimkova_frekvencia.unwrap().to_string(), "25");
    assert_eq!(s[1].kodek, Kodek::Flac);
    assert_eq!(s[1].jazyk.as_deref(), Some("slk"));
    assert_eq!(s[1].zvuk.unwrap().frekvencia, 48_000);
    assert_eq!(s[0].jazyk.as_deref(), Some("eng"), "predvolený jazyk podľa RFC 9559");
    let p = vsetky(&mut m);
    assert_eq!(p.len(), ocak.len());
    for (a, (stopa, pts, kluc, data)) in p.iter().zip(&ocak) {
        assert_eq!((a.stopa, a.pts, a.klucovy, &a.data), (*stopa, Some(*pts), *kluc, data));
    }
    assert_eq!(p[0].trvanie, Some(40));
    assert_eq!(p[1].trvanie, Some(20));
    // prvý paket každej stopy po otvorení má príznak prerušenia, ďalšie nie
    assert!(p[0].prerusenie && p[1].prerusenie && !p[2].prerusenie && !p[3].prerusenie);
    // chladné čítanie: celý malý súbor jedným čítaním zo zdroja
    assert_eq!(m.citac().statistika().0, 1);
}

#[test]
fn webm_a_neznama_velkost() {
    // živý tok: segment aj zhluky bez veľkosti
    let mut d = ebml("webm");
    d.extend(neznamy(0x18538067));
    d.extend(info(1_000_000, 0.0));
    d.extend(stopy(&[stopa(
        1,
        2,
        "A_OPUS",
        &[zvuk(48000.0, 2), uint(0x56AA, 6_500_000), uint(0x56BB, 80_000_000)],
    )]));
    for z in 0..3u64 {
        let bloky: Vec<_> = (0..5).map(|k| simple(1, k * 20, true, &[z as u8, k as u8])).collect();
        d.extend(zhluk_neznamy(z * 100, &bloky));
    }
    let mut m = otvor(d);
    assert_eq!(m.format(), FormatKontajnera::WebM);
    let s = &m.stopy()[0];
    assert_eq!((s.oneskorenie_kodeku_ns, s.predbeh_posunu_ns), (6_500_000, 80_000_000));
    let p = vsetky(&mut m);
    assert_eq!(p.len(), 15);
    assert_eq!(p.iter().map(|p| p.pts.unwrap()).collect::<Vec<_>>()[..7], [0, 20, 40, 60, 80, 100, 120]);
    assert_eq!(p[14].data, [2, 4]);
}

#[test]
fn lacing_skupiny_a_odstranena_hlavicka() {
    let ramce: [&[u8]; 3] = [&[1; 300], &[2; 7], &[3; 5]];
    let xiph = prvok(0xA3, &telo_bloku(1, 0, 0x80 | 0x02, &ramce));
    let ebml_l = prvok(0xA3, &telo_bloku(1, 100, 0x80 | 0x06, &ramce));
    let pevny = prvok(0xA3, &telo_bloku(1, 200, 0x80 | 0x04, &[&[9; 4], &[8; 4]]));
    // BlockGroup: trvanie 30 ms, odkaz na iný blok (nie kľúčový), DiscardPadding 2,5 ms → 120 vzoriek pri 48 kHz
    let skupina = prvok(
        0xA0,
        &spoj(&[
            prvok(0xA1, &telo_bloku(2, 300, 0, &[&[0xAB, 0xCD]])),
            uint(0x9B, 30),
            prvok(0xFB, &[0xFF, 0xD8]),
            uint(0x75A2, 2_500_000),
        ]),
    );
    let obsah = spoj(&[
        info(1_000_000, 1000.0),
        stopy(&[
            stopa(1, 2, "A_MPEG/L3", &[uint(0x23E383, 24_000_000), zvuk(48000.0, 2)]),
            stopa(2, 2, "A_AC3", &[zvuk(48000.0, 6), odstranena_hlavicka(&[0x0B, 0x77])]),
        ]),
        zhluk(0, &[xiph, ebml_l, pevny, skupina]),
    ]);
    let mut m = otvor(spoj(&[ebml("matroska"), segment(&obsah)]));
    let p = vsetky(&mut m);
    assert_eq!(p.len(), 9);
    let dlzky: Vec<_> = p.iter().map(|p| p.data.len()).collect();
    assert_eq!(dlzky, [300, 7, 5, 300, 7, 5, 4, 4, 4]);
    // ďalšie rámce lacingu majú čas podľa DefaultDuration (24 ms)
    assert_eq!(
        p.iter().map(|p| p.pts.unwrap()).collect::<Vec<_>>(),
        [0, 24, 48, 100, 124, 148, 200, 224, 300]
    );
    let g = &p[8];
    assert_eq!(g.data, [0x0B, 0x77, 0xAB, 0xCD], "odstránená hlavička sa vráti pred rámec");
    assert!(!g.klucovy);
    assert_eq!((g.trvanie, g.orezat_koniec), (Some(30), 120));
}

#[test]
fn posun_podla_cues_aj_delenim() {
    for s_cues in [true, false] {
        let (d, _) = typicky(30, s_cues);
        let mut m = otvor(d);
        for (ciel, ocakavany) in [
            (0i64, 0i64),
            (1_500_000_000, 1_000_000_000),
            (29_999_000_000, 29_000_000_000),
            (99_000_000_000, 29_000_000_000),
            (-5, 0),
        ] {
            let t = m.posun(ciel, Posun::KlucovyPred).unwrap();
            assert_eq!(t, ocakavany, "cieľ {ciel}, Cues {s_cues}");
            let p = m.dalsi_paket().unwrap().unwrap();
            assert_eq!(p.pts, Some(ocakavany / 1_000_000));
            assert!(p.klucovy && p.prerusenie);
        }
    }
}

#[test]
fn stopy_a_cues_na_konci_cez_seek_head() {
    // Info, potom zhluky, Tracks a Cues až na konci – nájdu sa len cez SeekHead na začiatku segmentu
    let inf = info(1_000_000, 3000.0);
    let zh: Vec<Vec<u8>> = (0..3u64)
        .map(|z| zhluk(z * 1000, &[simple(1, 0, true, &[z as u8; 3]), simple(1, 500, false, &[9])]))
        .collect();
    let trk = stopy(&[stopa(1, 1, "V_VP9", &[video(64, 48)])]);
    let telo_zhlukov: u64 = zh.iter().map(|z| z.len() as u64).sum();
    let mut sh = Vec::new();
    for _ in 0..3 {
        // poloha Tracks a Cues závisí od dĺžky samotného SeekHead – ustáli sa po pár krokoch
        let p_trk = sh.len() as u64 + inf.len() as u64 + telo_zhlukov;
        sh = seek_head(&[(0x1654AE6B, p_trk), (0x1C53BB6B, p_trk + trk.len() as u64)]);
    }
    let za_sh = sh.len() as u64 + inf.len() as u64;
    let mut polohy = vec![za_sh];
    for z in &zh {
        polohy.push(polohy.last().unwrap() + z.len() as u64);
    }
    let c = cues(&[(0, 1, polohy[0]), (1000, 1, polohy[1]), (2000, 1, polohy[2])]);
    let obsah = spoj(&[sh, inf, spoj(&zh), trk, c]);
    let mut m = otvor(spoj(&[ebml("matroska"), segment(&obsah)]));
    assert_eq!(m.stopy()[0].kodek, Kodek::Vp9);
    assert_eq!(m.posun(2_100_000_000, Posun::KlucovyPred).unwrap(), 2_000_000_000);
    assert_eq!(m.dalsi_paket().unwrap().unwrap().data, [2, 2, 2]);
    m.posun(0, Posun::Presny).unwrap();
    assert_eq!(vsetky(&mut m).len(), 6, "Tracks a Cues za zhlukmi sa nečítajú ako pakety");
}

#[test]
fn poskodenie_a_skratenie() {
    let (d, ocak) = typicky(5, true);
    // smeti v strede druhého zhluku: čítanie pokračuje ďalším zhlukom s príznakom prerušenia
    let mut zla = d.clone();
    let i = zla.windows(4).enumerate().filter(|(_, w)| *w == [0x1F, 0x43, 0xB6, 0x75]).nth(1).unwrap().0;
    for b in &mut zla[i + 40..i + 60] {
        *b = 0;
    }
    let mut m = otvor(zla);
    let p = vsetky(&mut m);
    assert!(p.len() < ocak.len() && p.len() >= 4 * 75, "{}", p.len());
    let po = p.iter().position(|p| p.pts == Some(2000)).unwrap();
    assert!(p[po].prerusenie);
    // skrátenie na každom bajte: nikdy pád, nikdy viac paketov
    for n in (0..d.len()).step_by(7) {
        if let Ok(mut m) = Mkv::otvor(PametovyZdroj::novy(d[..n].to_vec()), Limity::default()) {
            let mut k = 0;
            while let Ok(Some(_)) = m.dalsi_paket() {
                k += 1;
            }
            assert!(k <= ocak.len());
        }
    }
}

#[test]
fn obrovsky_prvok_bez_alokacie() {
    // CodecPrivate tvrdí 2^50 B v malom súbore – chyba, nie pokus o alokáciu
    let mut s = vec![0xAE];
    s.extend(velkost(1 << 50));
    s.extend([0x63, 0xA2]);
    let mut obsah = info(1_000_000, 0.0);
    obsah.extend(prvok(0x1654AE6B, &s));
    assert!(
        Mkv::otvor(PametovyZdroj::novy(spoj(&[ebml("matroska"), segment(&obsah)])), Limity::default())
            .is_err()
    );
    assert!(Mkv::otvor(PametovyZdroj::novy(ebml("avi")), Limity::default()).is_err());
}

#[test]
fn otvorenie_podla_obsahu() {
    let (d, _) = typicky(1, true);
    let mut k = orchidea_media::otvor(PametovyZdroj::novy(d), Limity::default()).unwrap();
    assert_eq!(k.stopy().len(), 2);
    assert!(k.dalsi_paket().unwrap().is_some());
}
