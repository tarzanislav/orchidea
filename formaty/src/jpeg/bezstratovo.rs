//! Bezstratové úpravy JPEG v oblasti DCT: otočenie o 90°, 180°, 270°, zrkadlenia, transpozícia a orezanie –
//! bez dekódovania do bodov, preto bez akejkoľvek straty kvality (a s malou prácou – chladne).
//!
//! Princíp (vlastnosti 2D DCT-II): zrkadlenie bloku 8 × 8 vodorovne zmení znamienko koeficientov s nepárnou
//! vodorovnou frekvenciou, zvislo s nepárnou zvislou; transpozícia bloku transponuje koeficienty (aj
//! kvantizačnú tabuľku). Otočenie = transpozícia + zrkadlenie. Bloky sa preusporiadajú v mriežke každej zložky,
//! podvzorkovanie sa pri transpozícii vymení (h ↔ v).
//!
//! Neúplné bloky MCU na okraji, ktorý sa zrkadlením presunie dovnútra obrazu, sa odstránia (ako voľba „trim“
//! bežných nástrojov) – obraz sa zmenší najviac o 15 bodov. Orezanie začína na hranici MCU (začiatok sa
//! zaokrúhli nadol, rozmer sa podľa toho zväčší); robí sa po otočení, v súradniciach výsledku.
//! Segmenty APPn a COM (Exif, XMP, ICC …) sa prenesú; natočenie v Exif sa dá nastaviť na 1.

use super::dekoder::{Koeficienty, citaj_koeficienty};
use super::koder::{Rovina, zapis_koeficienty};
use crate::obraz::Limity;
use orchidea_jadro::{Chyba, Vysledok};

/// Geometrická transformácia (skupina symetrií štvorca).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transformacia {
    Ziadna,
    ZrkadloVodorovne,
    ZrkadloZvisle,
    /// Výmena osí (zrkadlenie podľa hlavnej uhlopriečky).
    Transpozicia,
    /// Zrkadlenie podľa vedľajšej uhlopriečky.
    Transverzia,
    /// O 90° v smere hodinových ručičiek.
    Otoc90,
    Otoc180,
    /// O 90° proti smeru hodinových ručičiek.
    Otoc270,
}

impl Transformacia {
    /// Transformácia, ktorá obraz s natočením Exif `n` (1–8) postaví do správnej polohy.
    pub fn z_natocenia(n: u8) -> Transformacia {
        match n {
            2 => Transformacia::ZrkadloVodorovne,
            3 => Transformacia::Otoc180,
            4 => Transformacia::ZrkadloZvisle,
            5 => Transformacia::Transpozicia,
            6 => Transformacia::Otoc90,
            7 => Transformacia::Transverzia,
            8 => Transformacia::Otoc270,
            _ => Transformacia::Ziadna,
        }
    }

    /// (vymení osi, zrkadlí pôvodnú os x, zrkadlí pôvodnú os y)
    fn rozklad(self) -> (bool, bool, bool) {
        match self {
            Transformacia::Ziadna => (false, false, false),
            Transformacia::ZrkadloVodorovne => (false, true, false),
            Transformacia::ZrkadloZvisle => (false, false, true),
            Transformacia::Transpozicia => (true, false, false),
            Transformacia::Transverzia => (true, true, true),
            // nové x = H − 1 − y, nové y = x: zrkadlí sa pôvodná os y
            Transformacia::Otoc90 => (true, false, true),
            Transformacia::Otoc180 => (false, true, true),
            Transformacia::Otoc270 => (true, true, false),
        }
    }

    /// Zloženie: najprv `self`, potom `b`.
    pub fn potom(self, b: Transformacia) -> Transformacia {
        // reprezentácia maticou 2 × 2 so znamienkami
        let m = |t: Transformacia| -> [[i8; 2]; 2] {
            match t {
                Transformacia::Ziadna => [[1, 0], [0, 1]],
                Transformacia::ZrkadloVodorovne => [[-1, 0], [0, 1]],
                Transformacia::ZrkadloZvisle => [[1, 0], [0, -1]],
                Transformacia::Transpozicia => [[0, 1], [1, 0]],
                Transformacia::Transverzia => [[0, -1], [-1, 0]],
                Transformacia::Otoc90 => [[0, -1], [1, 0]],
                Transformacia::Otoc180 => [[-1, 0], [0, -1]],
                Transformacia::Otoc270 => [[0, 1], [-1, 0]],
            }
        };
        let (x, y) = (m(self), m(b));
        let mut z = [[0i8; 2]; 2];
        for i in 0..2 {
            for j in 0..2 {
                z[i][j] = y[i][0] * x[0][j] + y[i][1] * x[1][j];
            }
        }
        [
            Transformacia::Ziadna,
            Transformacia::ZrkadloVodorovne,
            Transformacia::ZrkadloZvisle,
            Transformacia::Transpozicia,
            Transformacia::Transverzia,
            Transformacia::Otoc90,
            Transformacia::Otoc180,
            Transformacia::Otoc270,
        ]
        .into_iter()
        .find(|&t| m(t) == z)
        .unwrap_or(Transformacia::Ziadna)
    }
}

/// Výrez v bodoch výsledku (po transformácii).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vyrez {
    pub x: usize,
    pub y: usize,
    pub sirka: usize,
    pub vyska: usize,
}

/// Mriežka, na ktorú sa zarovná začiatok bezstratového orezania, v súradniciach obrazu po transformácii `t`
/// **bez** odstránenia neúplných MCU (tak, ako ho ukáže prehliadač pri bežnom otočení).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mriezka {
    /// Rozmer MCU po transformácii (šírka, výška).
    pub mcu: (usize, usize),
    /// Neúplný okraj vľavo a hore, ktorý bezstratová transformácia odstráni (0 – 15 bodov): bod (x, y)
    /// prehliadača je v bezstratovom výsledku bod (x − posun.0, y − posun.1).
    pub posun: (usize, usize),
}

/// Mriežka bezstratového orezania – stačí hlavička súboru (po SOF), koeficienty sa nečítajú.
pub fn mriezka(data: &[u8], t: Transformacia) -> Option<Mriezka> {
    let (w, h, hmax, vmax) = hlavicka_sof(data)?;
    let (mw, mh) = (8 * hmax, 8 * vmax);
    let (vymen, zrk_x, zrk_y) = t.rozklad();
    // zrkadlená pôvodná os: jej neúplné MCU na konci sa presunie na začiatok a odstráni sa
    let (px, py) = (if zrk_x { w % mw } else { 0 }, if zrk_y { h % mh } else { 0 });
    Some(if vymen {
        Mriezka { mcu: (mh, mw), posun: (py, px) }
    } else {
        Mriezka { mcu: (mw, mh), posun: (px, py) }
    })
}

/// (šírka, výška, najväčšie vodorovné a zvislé podvzorkovanie) z prvého SOF.
fn hlavicka_sof(d: &[u8]) -> Option<(usize, usize, usize, usize)> {
    if d.get(0..2) != Some(&[0xFF, 0xD8]) {
        return None;
    }
    let mut i = 2;
    loop {
        while d.get(i) == Some(&0xFF) && d.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        if *d.get(i)? != 0xFF {
            return None;
        }
        let m = *d.get(i + 1)?;
        i += 2;
        if m == 0x01 || (0xD0..=0xD8).contains(&m) {
            continue;
        }
        if m == 0xD9 || m == 0xDA {
            return None;
        }
        let dlzka = u16::from_be_bytes([*d.get(i)?, *d.get(i + 1)?]) as usize;
        if matches!(m, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            let s = d.get(i + 2..i + dlzka)?;
            let (h, w, n) = (
                u16::from_be_bytes([*s.get(1)?, *s.get(2)?]) as usize,
                u16::from_be_bytes([*s.get(3)?, *s.get(4)?]) as usize,
                *s.get(5)? as usize,
            );
            let (mut hmax, mut vmax) = (1, 1);
            for c in 0..n {
                let b = *s.get(6 + c * 3 + 1)?;
                hmax = hmax.max((b >> 4) as usize);
                vmax = vmax.max((b & 15) as usize);
            }
            return (w > 0 && h > 0).then_some((w, h, hmax, vmax));
        }
        i += dlzka;
    }
}

/// Bezstratovo upraví JPEG: transformácia, potom voliteľné orezanie; `natocenie_1` nastaví v Exif natočenie 1.
pub fn uprav(
    data: &[u8],
    t: Transformacia,
    vyrez: Option<Vyrez>,
    natocenie_1: bool,
    limity: &Limity,
) -> Vysledok<Vec<u8>> {
    let k = citaj_koeficienty(data, limity)?;
    let k = transformuj(k, t)?;
    let mut k = match vyrez {
        Some(v) => orez(k, v)?,
        None => k,
    };
    if natocenie_1 {
        for (z, seg) in k.segmenty.iter_mut() {
            if *z == 0xE1 && seg.starts_with(b"Exif\0\0") {
                crate::exif::nastav_natocenie(&mut seg[6..], 1);
            }
        }
    }
    zapis(&k)
}

fn zapis(k: &Koeficienty) -> Vysledok<Vec<u8>> {
    // kvantizačné tabuľky: každá zložka má svoju (rovnaké sa zlúčia), najviac 4
    let mut tabulky: Vec<(u8, [u16; 64])> = Vec::new();
    let mut tq = Vec::new();
    for z in &k.zlozky {
        let i = match tabulky.iter().position(|t| t.1 == z.kvant) {
            Some(i) => i,
            None => {
                if tabulky.len() == 4 {
                    return Err(Chyba::Limit("JPEG: viac ako 4 kvantizačné tabuľky".into()));
                }
                tabulky.push((tabulky.len() as u8, z.kvant));
                tabulky.len() - 1
            }
        };
        tq.push(i);
    }
    let hmax = k.zlozky.iter().map(|z| z.h).max().unwrap_or(1);
    let vmax = k.zlozky.iter().map(|z| z.v).max().unwrap_or(1);
    let roviny: Vec<Rovina> = k
        .zlozky
        .iter()
        .zip(&tq)
        .map(|(z, &t)| Rovina {
            w: z.bw * 8,
            h: z.bh * 8,
            vzorky: Vec::new(),
            hf: z.h,
            vf: z.v,
            tq: t,
            bw_ucinne: (k.sirka * z.h).div_ceil(hmax).div_ceil(8),
            bh_ucinne: (k.vyska * z.v).div_ceil(vmax).div_ceil(8),
            koef: z.koef.clone(),
        })
        .collect();
    let ids: Vec<u8> = k.zlozky.iter().map(|z| z.id).collect();
    zapis_koeficienty(&roviny, &ids, (k.sirka, k.vyska), k.presnost, &tabulky, &k.segmenty)
}

/// Transformuje koeficienty jedného bloku (prirodzené poradie: index = v·8 + u).
fn blok(src: &[i16], dst: &mut [i16], vymen: bool, zrk_u: bool, zrk_v: bool) {
    for v in 0..8 {
        for u in 0..8 {
            // (u, v) vo výsledku; zdroj podľa výmeny osí
            let s = if vymen { src[u * 8 + v] } else { src[v * 8 + u] };
            let znamienko = ((zrk_u && u % 2 == 1) as u8 ^ (zrk_v && v % 2 == 1) as u8) == 1;
            dst[v * 8 + u] = if znamienko { s.wrapping_neg() } else { s };
        }
    }
}

fn transformuj(k: Koeficienty, t: Transformacia) -> Vysledok<Koeficienty> {
    if t == Transformacia::Ziadna {
        return Ok(k);
    }
    let (vymen, zrk_x, zrk_y) = t.rozklad();
    let hmax = k.zlozky.iter().map(|z| z.h).max().unwrap_or(1);
    let vmax = k.zlozky.iter().map(|z| z.v).max().unwrap_or(1);
    // orezanie neúplných MCU na zrkadlených osiach (pôvodné súradnice)
    let w_t = if zrk_x { k.sirka / (8 * hmax) * 8 * hmax } else { k.sirka };
    let h_t = if zrk_y { k.vyska / (8 * vmax) * 8 * vmax } else { k.vyska };
    if w_t == 0 || h_t == 0 {
        return Err(Chyba::NezhodaTvaru(
            "JPEG: obraz je menší ako jedno MCU – nedá sa otočiť bezstratovo".into(),
        ));
    }
    let (w_n, h_n) = if vymen { (h_t, w_t) } else { (w_t, h_t) };
    let (hmax_n, vmax_n) = if vymen { (vmax, hmax) } else { (hmax, vmax) };
    let (mcux_n, mcuy_n) = (w_n.div_ceil(8 * hmax_n), h_n.div_ceil(8 * vmax_n));
    let zlozky = k
        .zlozky
        .into_iter()
        .map(|z| {
            // počet blokov so skutočnými údajmi po orezaní (pôvodné osi)
            let sbw = (w_t * z.h).div_ceil(hmax).div_ceil(8);
            let sbh = (h_t * z.v).div_ceil(vmax).div_ceil(8);
            let (h, v) = if vymen { (z.v, z.h) } else { (z.h, z.v) };
            let (bw, bh) = (mcux_n * h, mcuy_n * v);
            let mut koef = vec![0i16; bw * bh * 64];
            for oy in 0..bh {
                for ox in 0..bw {
                    // súradnice v pôvodnej mriežke pred zrkadlením
                    let (mut sx, mut sy) = if vymen { (oy, ox) } else { (ox, oy) };
                    if sx >= sbw || sy >= sbh {
                        continue;
                    }
                    if zrk_x {
                        sx = sbw - 1 - sx;
                    }
                    if zrk_y {
                        sy = sbh - 1 - sy;
                    }
                    if sx >= z.bw || sy >= z.bh {
                        continue;
                    }
                    let s = (sy * z.bw + sx) * 64;
                    let d = (oy * bw + ox) * 64;
                    // zrkadlenie pôvodnej osi x mení znamienko pôvodnej vodorovnej frekvencie
                    let (zu, zv) = if vymen { (zrk_y, zrk_x) } else { (zrk_x, zrk_y) };
                    blok(&z.koef[s..s + 64], &mut koef[d..d + 64], vymen, zu, zv);
                }
            }
            let mut kvant = z.kvant;
            if vymen {
                for v in 0..8 {
                    for u in 0..8 {
                        kvant[v * 8 + u] = z.kvant[u * 8 + v];
                    }
                }
            }
            super::dekoder::ZlozkaKoef { id: z.id, h, v, kvant, bw, bh, koef }
        })
        .collect();
    Ok(Koeficienty { sirka: w_n, vyska: h_n, presnost: k.presnost, zlozky, segmenty: k.segmenty })
}

fn orez(k: Koeficienty, v: Vyrez) -> Vysledok<Koeficienty> {
    let hmax = k.zlozky.iter().map(|z| z.h).max().unwrap_or(1);
    let vmax = k.zlozky.iter().map(|z| z.v).max().unwrap_or(1);
    let (mw, mh) = (8 * hmax, 8 * vmax);
    if v.x >= k.sirka || v.y >= k.vyska || v.sirka == 0 || v.vyska == 0 {
        return Err(Chyba::MimoRozsahu("JPEG: výrez mimo obrazu".into()));
    }
    let (x0, y0) = (v.x / mw * mw, v.y / mh * mh);
    let w = (v.sirka + v.x - x0).min(k.sirka - x0);
    let h = (v.vyska + v.y - y0).min(k.vyska - y0);
    let (mcux, mcuy) = (w.div_ceil(mw), h.div_ceil(mh));
    let zlozky = k
        .zlozky
        .into_iter()
        .map(|z| {
            let (ox, oy) = (x0 / mw * z.h, y0 / mh * z.v);
            let (bw, bh) = (mcux * z.h, mcuy * z.v);
            let mut koef = vec![0i16; bw * bh * 64];
            for by in 0..bh {
                for bx in 0..bw {
                    let (sx, sy) = (ox + bx, oy + by);
                    if sx < z.bw && sy < z.bh {
                        let s = (sy * z.bw + sx) * 64;
                        let d = (by * bw + bx) * 64;
                        koef[d..d + 64].copy_from_slice(&z.koef[s..s + 64]);
                    }
                }
            }
            super::dekoder::ZlozkaKoef { bw, bh, koef, ..z }
        })
        .collect();
    Ok(Koeficienty { sirka: w, vyska: h, presnost: k.presnost, zlozky, segmenty: k.segmenty })
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn mriezka_orezania_podla_hlavicky() {
        // SOF0: 100 × 75, Y 2×2, Cb Cr 1×1 (4:2:0) → MCU 16 × 16, neúplné 4 a 11 bodov
        let mut d = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC0, 0, 17, 8, 0, 75, 0, 100, 3];
        d.extend_from_slice(&[1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        d.extend_from_slice(&[0xFF, 0xD9]);
        let m = |t| mriezka(&d, t).unwrap();
        assert_eq!(m(Transformacia::Ziadna), Mriezka { mcu: (16, 16), posun: (0, 0) });
        assert_eq!(m(Transformacia::ZrkadloVodorovne), Mriezka { mcu: (16, 16), posun: (4, 0) });
        assert_eq!(m(Transformacia::Otoc180), Mriezka { mcu: (16, 16), posun: (4, 11) });
        // otočenie vpravo: nové x = H − 1 − y – vľavo je pôvodný spodný neúplný okraj
        assert_eq!(m(Transformacia::Otoc90), Mriezka { mcu: (16, 16), posun: (11, 0) });
        assert_eq!(m(Transformacia::Otoc270), Mriezka { mcu: (16, 16), posun: (0, 4) });
        assert!(mriezka(&d[..12], Transformacia::Ziadna).is_none());
        assert!(mriezka(b"nie je JPEG", Transformacia::Ziadna).is_none());
    }

    #[test]
    fn zlozenie_transformacii() {
        use Transformacia::*;
        assert_eq!(Otoc90.potom(Otoc90), Otoc180);
        assert_eq!(Otoc90.potom(Otoc270), Ziadna);
        assert_eq!(ZrkadloVodorovne.potom(ZrkadloVodorovne), Ziadna);
        assert_eq!(Otoc180.potom(ZrkadloVodorovne), ZrkadloZvisle);
        for n in 1..=8 {
            let _ = Transformacia::z_natocenia(n);
        }
    }
}
