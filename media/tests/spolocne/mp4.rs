//! Malý zapisovač MP4 len pre skúšky (ISO/IEC 14496-12): súbory s presne známym obsahom vrátane okrajových
//! prípadov (moov na konci, co64, zoznam úprav, ctts, fragmenty, QuickTime PCM, veľká veľkosť 1).
#![allow(dead_code)]

pub fn bx(typ: &[u8; 4], obsah: &[u8]) -> Vec<u8> {
    let mut v = ((obsah.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(typ);
    v.extend_from_slice(obsah);
    v
}

/// Box s 64-bitovou veľkosťou (pole veľkosti = 1).
pub fn bx64(typ: &[u8; 4], obsah: &[u8]) -> Vec<u8> {
    let mut v = 1u32.to_be_bytes().to_vec();
    v.extend_from_slice(typ);
    v.extend_from_slice(&((obsah.len() + 16) as u64).to_be_bytes());
    v.extend_from_slice(obsah);
    v
}

pub fn plny(typ: &[u8; 4], verzia: u8, priznaky: u32, obsah: &[u8]) -> Vec<u8> {
    let mut v = (((verzia as u32) << 24) | priznaky).to_be_bytes().to_vec();
    v.extend_from_slice(obsah);
    bx(typ, &v)
}

pub fn u32s(x: &[u32]) -> Vec<u8> {
    x.iter().flat_map(|v| v.to_be_bytes()).collect()
}

/// Opis stopy pre zapisovač.
pub struct StopaMp4 {
    pub id: u32,
    pub video: bool,
    pub mierka: u32,
    /// (údaje, trvanie, posun zobrazenia, kľúčová)
    pub vzorky: Vec<(Vec<u8>, u32, i32, bool)>,
    pub vzoriek_v_kusku: u32,
    /// media_time zoznamu úprav (None = bez edts), prázdna úprava pred ním (v mierke filmu 1000).
    pub uprava: Option<(i64, u32)>,
    /// Záznam stsd (typ a telo bez hlavičky).
    pub zaznam: ([u8; 4], Vec<u8>),
}

pub fn avc1() -> ([u8; 4], Vec<u8>) {
    let mut t = vec![0u8; 24];
    t.extend_from_slice(&320u16.to_be_bytes());
    t.extend_from_slice(&240u16.to_be_bytes());
    t.extend(vec![0u8; 50]);
    let avcc = [1u8, 0x64, 0, 0x28, 0xFF, 0xE1, 0, 4, 0x67, 0x64, 0, 0x28, 1, 0, 3, 0x68, 0xEE, 0x3C];
    t.extend(bx(b"avcC", &avcc));
    (*b"avc1", t)
}

/// Zvukový záznam (verzia 0): kanály, bity, frekvencia, deti.
pub fn zvuk(typ: &[u8; 4], kanalov: u16, bitov: u16, frekvencia: u32, deti: &[u8]) -> ([u8; 4], Vec<u8>) {
    let mut t = vec![0u8; 8];
    t.extend_from_slice(&[0u8; 8]);
    t.extend_from_slice(&kanalov.to_be_bytes());
    t.extend_from_slice(&bitov.to_be_bytes());
    t.extend_from_slice(&[0u8; 4]);
    t.extend_from_slice(&(frekvencia << 16).to_be_bytes());
    t.extend_from_slice(deti);
    (*typ, t)
}

/// Celý súbor: ftyp, (moov), mdat, (moov na konci). Kúsky v mdat sa prekladajú medzi stopami.
pub fn subor(stopy: &[StopaMp4], moov_na_konci: bool, co64: bool, qt: bool) -> Vec<u8> {
    let ftyp =
        if qt { bx(b"ftyp", b"qt  \0\0\x02\0qt  ") } else { bx(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41") };
    // kúsky: (stopa, prvá vzorka, počet)
    let mut kusky = Vec::new();
    let mut i = 0;
    loop {
        let mut nieco = false;
        for (s, st) in stopy.iter().enumerate() {
            let od = i * st.vzoriek_v_kusku as usize;
            if od < st.vzorky.len() {
                kusky.push((s, od, (st.vzoriek_v_kusku as usize).min(st.vzorky.len() - od)));
                nieco = true;
            }
        }
        if !nieco {
            break;
        }
        i += 1;
    }
    let mdat_telo: Vec<u8> = kusky
        .iter()
        .flat_map(|&(s, od, n)| stopy[s].vzorky[od..od + n].iter().flat_map(|v| v.0.clone()))
        .collect();
    let moov_pre = |zaciatok_mdat: u64| -> Vec<u8> {
        let mut polohy = vec![Vec::new(); stopy.len()];
        let mut p = zaciatok_mdat;
        for &(s, od, n) in &kusky {
            polohy[s].push(p);
            p += stopy[s].vzorky[od..od + n].iter().map(|v| v.0.len() as u64).sum::<u64>();
        }
        let mut trak = Vec::new();
        for (s, st) in stopy.iter().enumerate() {
            let tkhd = plny(b"tkhd", 0, 3, &[u32s(&[0, 0, st.id, 0, 0]), vec![0; 60]].concat());
            let mdhd = plny(b"mdhd", 0, 0, &[u32s(&[0, 0, st.mierka, 0]), vec![0x55, 0xC4, 0, 0]].concat());
            let hdlr = plny(
                b"hdlr",
                0,
                0,
                &[u32s(&[0]), if st.video { b"vide".to_vec() } else { b"soun".to_vec() }, vec![0; 13]]
                    .concat(),
            );
            let stsd = plny(b"stsd", 0, 0, &[u32s(&[1]), bx(&st.zaznam.0, &st.zaznam.1)].concat());
            let stts: Vec<u32> = st.vzorky.iter().flat_map(|v| [1, v.1]).collect();
            let stts = plny(b"stts", 0, 0, &[u32s(&[st.vzorky.len() as u32]), u32s(&stts)].concat());
            let ctts: Vec<u32> = st.vzorky.iter().flat_map(|v| [1, v.2 as u32]).collect();
            let ctts = plny(b"ctts", 1, 0, &[u32s(&[st.vzorky.len() as u32]), u32s(&ctts)].concat());
            let stsc = plny(b"stsc", 0, 0, &u32s(&[1, 1, st.vzoriek_v_kusku, 1]));
            let stsz: Vec<u32> = st.vzorky.iter().map(|v| v.0.len() as u32).collect();
            let stsz = plny(b"stsz", 0, 0, &[u32s(&[0, stsz.len() as u32]), u32s(&stsz)].concat());
            let stco = if co64 {
                plny(
                    b"co64",
                    0,
                    0,
                    &[
                        u32s(&[polohy[s].len() as u32]),
                        polohy[s].iter().flat_map(|p| p.to_be_bytes()).collect(),
                    ]
                    .concat(),
                )
            } else {
                plny(
                    b"stco",
                    0,
                    0,
                    &[
                        u32s(&[polohy[s].len() as u32]),
                        u32s(&polohy[s].iter().map(|&p| p as u32).collect::<Vec<_>>()),
                    ]
                    .concat(),
                )
            };
            let kluce: Vec<u32> =
                st.vzorky.iter().enumerate().filter(|v| v.1.3).map(|v| v.0 as u32 + 1).collect();
            let stss = if kluce.len() == st.vzorky.len() {
                vec![]
            } else {
                plny(b"stss", 0, 0, &[u32s(&[kluce.len() as u32]), u32s(&kluce)].concat())
            };
            let stbl = bx(b"stbl", &[stsd, stts, ctts, stsc, stsz, stco, stss].concat());
            let minf = bx(b"minf", &stbl);
            let mdia = bx(b"mdia", &[mdhd, hdlr, minf].concat());
            let edts = match st.uprava {
                None => vec![],
                Some((cas, prazdna)) => {
                    let mut z = Vec::new();
                    let mut n = 1;
                    if prazdna > 0 {
                        z.extend(u32s(&[prazdna, u32::MAX, 0x10000]));
                        n += 1;
                    }
                    z.extend(u32s(&[1000, cas as u32, 0x10000]));
                    bx(b"edts", &plny(b"elst", 0, 0, &[u32s(&[n]), z].concat()))
                }
            };
            trak.push(bx(b"trak", &[tkhd, edts, mdia].concat()));
        }
        let mvhd = plny(b"mvhd", 0, 0, &[u32s(&[0, 0, 1000, 3000]), vec![0; 80]].concat());
        bx(b"moov", &[mvhd, trak.concat()].concat())
    };
    if moov_na_konci {
        let zaciatok = ftyp.len() as u64 + 8;
        let moov = moov_pre(zaciatok);
        [ftyp, bx(b"mdat", &mdat_telo), moov].concat()
    } else {
        let dlzka_moov = moov_pre(0).len() as u64;
        let zaciatok = ftyp.len() as u64 + dlzka_moov + 8;
        let moov = moov_pre(zaciatok);
        [ftyp, moov, bx(b"mdat", &mdat_telo)].concat()
    }
}

/// Typický súbor: H.264 25 snímok/s s B-snímkami (ctts) a zoznamom úprav + zvuk 48 kHz po 1024.
pub fn typicky(sekund: u32, moov_na_konci: bool, co64: bool) -> Vec<u8> {
    let mut v = Vec::new();
    for k in 0..25 * sekund {
        // poradie I P B B v každej skupine obrázkov: zobrazenie 0, 3, 1, 2 (× 512) → posun = PTS − DTS + 1024
        let posun = [1024i32, 2048, 512, 512][((k % 25) % 4) as usize];
        v.push((vec![(k % 251) as u8; 20 + (k % 7) as usize], 512, posun, k % 25 == 0));
    }
    let a = (0..47 * sekund).map(|k| (vec![0xA0 | (k % 16) as u8, k as u8], 1024, 0, true)).collect();
    subor(
        &[
            StopaMp4 {
                id: 1,
                video: true,
                mierka: 12800,
                vzorky: v,
                vzoriek_v_kusku: 5,
                uprava: Some((1024, 0)),
                zaznam: avc1(),
            },
            StopaMp4 {
                id: 2,
                video: false,
                mierka: 48000,
                vzorky: a,
                vzoriek_v_kusku: 10,
                uprava: Some((1024, 0)),
                zaznam: zvuk(b"mp4a", 2, 16, 48000, &[]),
            },
        ],
        moov_na_konci,
        co64,
        false,
    )
}

/// Fragmentované MP4: moov s mvex/trex a prázdnymi tabuľkami, potom `n` fragmentov (moof + mdat) po 5 vzorkách
/// jednej videostopy (trvanie 512, prvá vzorka fragmentu kľúčová, údaje vzhľadom na moof).
pub fn fragmentovany(n: u32) -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"iso6\0\0\x02\0iso6cmfc");
    let prazdne = |t: &[u8; 4], extra: &[u32]| plny(t, 0, 0, &u32s(extra));
    let (zt, zz) = avc1();
    let stbl = bx(
        b"stbl",
        &[
            plny(b"stsd", 0, 0, &[u32s(&[1]), bx(&zt, &zz)].concat()),
            prazdne(b"stts", &[0]),
            prazdne(b"stsc", &[0]),
            prazdne(b"stsz", &[0, 0]),
            prazdne(b"stco", &[0]),
        ]
        .concat(),
    );
    let mdia = bx(
        b"mdia",
        &[
            plny(b"mdhd", 0, 0, &[u32s(&[0, 0, 12800, 0]), vec![0x55, 0xC4, 0, 0]].concat()),
            plny(b"hdlr", 0, 0, &[u32s(&[0]), b"vide".to_vec(), vec![0; 13]].concat()),
            bx(b"minf", &stbl),
        ]
        .concat(),
    );
    let trak =
        bx(b"trak", &[plny(b"tkhd", 0, 3, &[u32s(&[0, 0, 1, 0, 0]), vec![0; 60]].concat()), mdia].concat());
    let mvex = bx(b"mvex", &plny(b"trex", 0, 0, &u32s(&[1, 1, 512, 0, 0x0001_0000])));
    let moov = bx(
        b"moov",
        &[plny(b"mvhd", 0, 0, &[u32s(&[0, 0, 1000, 0]), vec![0; 80]].concat()), trak, mvex].concat(),
    );
    let mut out = [ftyp, moov].concat();
    for f in 0..n {
        let vzorky: Vec<Vec<u8>> = (0..5).map(|k| vec![(f * 5 + k) as u8; 10 + k as usize]).collect();
        // trun: data-offset, first-sample-flags, veľkosti
        let trun_telo = |offset: u32| {
            let mut t = u32s(&[5, offset, 0]);
            t.extend(u32s(&vzorky.iter().map(|v| v.len() as u32).collect::<Vec<_>>()));
            plny(b"trun", 0, 0x205, &t)
        };
        let traf_bez = |offset: u32| {
            bx(
                b"traf",
                &[
                    plny(b"tfhd", 0, 0x2_0000, &u32s(&[1])),
                    plny(b"tfdt", 1, 0, &(f as u64 * 2560).to_be_bytes()),
                    trun_telo(offset),
                ]
                .concat(),
            )
        };
        let mfhd = plny(b"mfhd", 0, 0, &u32s(&[f + 1]));
        let dlzka_moof = bx(b"moof", &[mfhd.clone(), traf_bez(0)].concat()).len() as u32;
        let moof = bx(b"moof", &[mfhd, traf_bez(dlzka_moof + 8)].concat());
        out.extend(moof);
        out.extend(bx(b"mdat", &vzorky.concat()));
    }
    out
}
