//! Malý zapisovač Matrosky len pre skúšky – súbory s presne známym obsahom vrátane okrajových prípadov
//! (neznáma veľkosť, lacing, odstránená hlavička, Cues na konci cez SeekHead). Podľa RFC 8794 a RFC 9559.
#![allow(dead_code)]

/// Bajty ID (ID v Matroske už obsahujú bity dĺžky).
fn id_bajty(id: u32) -> Vec<u8> {
    let b = id.to_be_bytes();
    let z = b.iter().position(|&x| x != 0).unwrap_or(3);
    b[z..].to_vec()
}

/// Veľkosť ako číslo s premenlivou dĺžkou (najkratší tvar, ktorý nie je „neznáma veľkosť“).
pub fn velkost(v: u64) -> Vec<u8> {
    for n in 1..=8u32 {
        if v < (1u64 << (7 * n)) - 1 {
            let mut x = v | (1u64 << (7 * n));
            let mut out = vec![0u8; n as usize];
            for i in (0..n as usize).rev() {
                out[i] = x as u8;
                x >>= 8;
            }
            return out;
        }
    }
    panic!("veľkosť")
}

pub fn prvok(id: u32, obsah: &[u8]) -> Vec<u8> {
    let mut v = id_bajty(id);
    v.extend(velkost(obsah.len() as u64));
    v.extend_from_slice(obsah);
    v
}

/// Prvok s neznámou veľkosťou (len hlavička – obsah nasleduje).
pub fn neznamy(id: u32) -> Vec<u8> {
    let mut v = id_bajty(id);
    v.push(0xFF);
    v
}

pub fn uint(id: u32, x: u64) -> Vec<u8> {
    let b = x.to_be_bytes();
    let z = b.iter().position(|&c| c != 0).unwrap_or(7);
    prvok(id, &b[z..])
}

pub fn text(id: u32, s: &str) -> Vec<u8> {
    prvok(id, s.as_bytes())
}

pub fn float(id: u32, f: f64) -> Vec<u8> {
    prvok(id, &f.to_be_bytes())
}

pub fn spoj(casti: &[Vec<u8>]) -> Vec<u8> {
    casti.concat()
}

pub fn ebml(doc_type: &str) -> Vec<u8> {
    prvok(
        0x1A45DFA3,
        &spoj(&[
            uint(0x4286, 1),
            uint(0x42F7, 1),
            uint(0x42F2, 4),
            uint(0x42F3, 8),
            text(0x4282, doc_type),
            uint(0x4287, 4),
            uint(0x4285, 2),
        ]),
    )
}

pub fn info(scale: u64, trvanie: f64) -> Vec<u8> {
    prvok(0x1549A966, &spoj(&[uint(0x2AD7B1, scale), float(0x4489, trvanie)]))
}

/// Stopa: číslo, typ (1 video, 2 zvuk), CodecID a ďalšie prvky.
pub fn stopa(cislo: u64, typ: u64, codec: &str, ine: &[Vec<u8>]) -> Vec<u8> {
    let mut c = vec![uint(0xD7, cislo), uint(0x73C5, cislo * 1000), uint(0x83, typ), text(0x86, codec)];
    c.extend_from_slice(ine);
    prvok(0xAE, &spoj(&c))
}

pub fn video(w: u64, h: u64) -> Vec<u8> {
    prvok(0xE0, &spoj(&[uint(0xB0, w), uint(0xBA, h)]))
}

pub fn zvuk(f: f64, kanalov: u64) -> Vec<u8> {
    prvok(0xE1, &spoj(&[float(0xB5, f), uint(0x9F, kanalov)]))
}

pub fn odstranena_hlavicka(bajty: &[u8]) -> Vec<u8> {
    prvok(
        0x6D80,
        &prvok(
            0x6240,
            &spoj(&[uint(0x5033, 0), prvok(0x5034, &spoj(&[uint(0x4254, 3), prvok(0x4255, bajty)]))]),
        ),
    )
}

pub fn stopy(stopy: &[Vec<u8>]) -> Vec<u8> {
    prvok(0x1654AE6B, &spoj(stopy))
}

/// Telo bloku: číslo stopy, relatívny čas, príznaky, rámce (lacing podľa príznakov 0x06).
pub fn telo_bloku(stopa: u64, rel: i16, priznaky: u8, ramce: &[&[u8]]) -> Vec<u8> {
    let mut v = velkost(stopa);
    v.extend_from_slice(&rel.to_be_bytes());
    v.push(priznaky);
    let lacing = (priznaky >> 1) & 3;
    if lacing != 0 {
        v.push((ramce.len() - 1) as u8);
        match lacing {
            1 => {
                for r in &ramce[..ramce.len() - 1] {
                    let mut n = r.len();
                    while n >= 255 {
                        v.push(255);
                        n -= 255;
                    }
                    v.push(n as u8);
                }
            }
            3 => {
                v.extend(velkost(ramce[0].len() as u64));
                for k in 1..ramce.len() - 1 {
                    // rozdiel v 2 bajtoch: hodnota + (2^13 − 1)
                    let r = ramce[k].len() as i64 - ramce[k - 1].len() as i64 + ((1 << 13) - 1);
                    v.extend_from_slice(&((r as u16) | 0x4000).to_be_bytes());
                }
            }
            _ => {}
        }
    }
    for r in ramce {
        v.extend_from_slice(r);
    }
    v
}

pub fn simple(stopa: u64, rel: i16, kluc: bool, data: &[u8]) -> Vec<u8> {
    prvok(0xA3, &telo_bloku(stopa, rel, if kluc { 0x80 } else { 0 }, &[data]))
}

pub fn zhluk(cas: u64, bloky: &[Vec<u8>]) -> Vec<u8> {
    let mut c = vec![uint(0xE7, cas)];
    c.extend_from_slice(bloky);
    prvok(0x1F43B675, &spoj(&c))
}

pub fn zhluk_neznamy(cas: u64, bloky: &[Vec<u8>]) -> Vec<u8> {
    let mut v = neznamy(0x1F43B675);
    v.extend(uint(0xE7, cas));
    for b in bloky {
        v.extend_from_slice(b);
    }
    v
}

/// Cues: (čas, stopa, poloha zhluku od začiatku dát segmentu).
pub fn cues(body: &[(u64, u64, u64)]) -> Vec<u8> {
    let b: Vec<Vec<u8>> = body
        .iter()
        .map(|&(c, s, p)| {
            prvok(0xBB, &spoj(&[uint(0xB3, c), prvok(0xB7, &spoj(&[uint(0xF7, s), uint(0xF1, p)]))]))
        })
        .collect();
    prvok(0x1C53BB6B, &spoj(&b))
}

pub fn seek_head(odkazy: &[(u32, u64)]) -> Vec<u8> {
    let s: Vec<Vec<u8>> = odkazy
        .iter()
        .map(|&(i, p)| prvok(0x4DBB, &spoj(&[prvok(0x53AB, &id_bajty(i)), uint(0x53AC, p)])))
        .collect();
    prvok(0x114D9B74, &spoj(&s))
}

pub fn segment(obsah: &[u8]) -> Vec<u8> {
    prvok(0x18538067, obsah)
}

/// Typický súbor: video H.264 (25 snímok/s) + zvuk FLAC, `n` zhlukov po 1 s, Cues pre video.
/// Vracia súbor a zoznam (stopa, čas v ms, kľúčový, údaje) všetkých paketov v poradí súboru.
pub fn typicky(n: u64, s_cues: bool) -> (Vec<u8>, Vec<(usize, i64, bool, Vec<u8>)>) {
    let avcc = vec![1u8, 0x64, 0, 0x28, 0xFF, 0xE1, 0, 4, 0x67, 0x64, 0, 0x28, 1, 0, 3, 0x68, 0xEE, 0x3C];
    let hlavicka = spoj(&[
        info(1_000_000, n as f64 * 1000.0),
        stopy(&[
            stopa(
                1,
                1,
                "V_MPEG4/ISO/AVC",
                &[uint(0x23E383, 40_000_000), prvok(0x63A2, &avcc), video(320, 240)],
            ),
            stopa(2, 2, "A_FLAC", &[uint(0x23E383, 20_000_000), zvuk(48000.0, 2), text(0x22B59C, "slk")]),
        ]),
    ]);
    let mut ocakavane = Vec::new();
    let mut zhluky = Vec::new();
    let mut polohy = Vec::new();
    let mut poloha = hlavicka.len() as u64;
    for z in 0..n {
        let mut bloky = Vec::new();
        for k in 0..25i64 {
            let v: Vec<u8> = vec![(z * 25 + k as u64) as u8; 10 + k as usize];
            bloky.push(simple(1, (k * 40) as i16, k == 0, &v));
            ocakavane.push((0usize, z as i64 * 1000 + k * 40, k == 0, v));
            for a in 0..2i64 {
                let d: Vec<u8> = vec![0xF0 | a as u8, k as u8, z as u8];
                bloky.push(simple(2, (k * 40 + a * 20) as i16, true, &d));
                ocakavane.push((1usize, z as i64 * 1000 + k * 40 + a * 20, true, d));
            }
        }
        let zh = zhluk(z * 1000, &bloky);
        polohy.push(poloha);
        poloha += zh.len() as u64;
        zhluky.push(zh);
    }
    let mut obsah = hlavicka;
    obsah.extend(spoj(&zhluky));
    if s_cues {
        let body: Vec<_> = polohy.iter().enumerate().map(|(i, &p)| (i as u64 * 1000, 1, p)).collect();
        obsah.extend(cues(&body));
    }
    (spoj(&[ebml("matroska"), segment(&obsah)]), ocakavane)
}
