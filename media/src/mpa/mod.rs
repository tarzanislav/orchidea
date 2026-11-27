//! **MPEG audio** (L8) – vrstva III MPEG-1 podľa textu normy: návrh CD 11172-3 (1991) a Sieler, Sperschneider:
//! „MPEG-Layer3 – Bitstream Syntax and Decoding“ (1997, ďalej [S]). Čísla tabuliek v komentároch sú z [S].
//!
//! Časť 1: rozbor bitového toku – hlavička (tabuľka 2), CRC (tabuľka 3), vedľajšie informácie (tabuľka 4), zásobník
//! bitov (`main_data_begin`), škálovacie faktory (tabuľka 5, tabuľky 15 a 16) a Huffmanovo dekódovanie (tabuľka 6,
//! [S] §4.3.4, tabuľky 24–41). Dôkaz správnosti: každá granula spotrebuje presne `part2_3_length` bitov.
//!
//! Časť 2: rekonštrukcia (`rekonstrukcia` – spätná kvantizácia, stereo, preradenie) a hybridná banka filtrov
//! (`synteza` – alias, IMDCT, okná, prekrytie, polyfázová syntéza s oknom D[i]). Dôkaz: presnosť podľa
//! ISO/IEC 11172-4 proti nezávislému rozhodcovi.
//!
//! Vrstvy I a II (MPEG-1 aj MPEG-2 LSF): `vrstva12` – tabuľky ETSI TS 103 466 (`tabulky_l2`).

#![allow(clippy::needless_range_loop)]

pub mod dekoder;
mod rekonstrukcia;
mod synteza;
#[allow(clippy::unreadable_literal, clippy::excessive_precision)]
mod tabulky;
#[allow(clippy::unreadable_literal, clippy::excessive_precision)]
mod tabulky_l2;
mod vrstva12;

use orchidea_jadro::{Chyba, Vysledok};
use std::sync::OnceLock;

/// Čítanie bitov od najvyššieho (bslbf / uimsbf).
#[derive(Clone, Debug)]
struct Bity<'a> {
    data: &'a [u8],
    poloha: usize,
}

impl<'a> Bity<'a> {
    fn nove(data: &'a [u8]) -> Bity<'a> {
        Bity { data, poloha: 0 }
    }
    fn bit(&mut self) -> u32 {
        let b = self.data.get(self.poloha >> 3).map_or(0, |&b| (b >> (7 - (self.poloha & 7))) & 1);
        self.poloha += 1;
        b as u32
    }
    fn citaj(&mut self, n: u32) -> u32 {
        (0..n).fold(0, |v, _| (v << 1) | self.bit())
    }
}

/// Hlavička rámca (tabuľka 2) – MPEG-1, MPEG-2 LSF a MPEG 2.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hlavicka {
    /// 1 = MPEG-1, 2 = MPEG-2 (LSF), 25 = MPEG 2.5
    pub verzia: u8,
    pub vrstva: u8,
    pub crc: bool,
    pub frekvencia: u32,
    /// index frekvencie do tabuliek pásiem (0–8: 44,1; 48; 32; 22,05; 24; 16; 11,025; 12; 8 kHz)
    pub index_frekvencie: usize,
    pub vypln: bool,
    /// 0 stereo, 1 spojené stereo, 2 dva kanály, 3 jeden kanál (tabuľka 10)
    pub rezim: u8,
    pub rozsirenie: u8,
    pub dlzka: usize,
}

impl Hlavicka {
    pub fn kanalov(&self) -> usize {
        if self.rezim == 3 { 1 } else { 2 }
    }
    /// Voľný formát (index toku 0 – tabuľka 8 [S]): pevný tok mimo tabuľky, dĺžka rámca sa určí zo vzdialenosti
    /// ďalšej hlavičky. Vráti `true`, ak je hlavička platná a vo voľnom formáte.
    pub fn volny_format(b: &[u8]) -> bool {
        b.len() >= 4
            && b[0] == 0xFF
            && b[1] & 0xE0 == 0xE0
            && (b[1] >> 3) & 3 != 1
            && (b[1] >> 1) & 3 != 0
            && b[2] >> 4 == 0
            && (b[2] >> 2) & 3 != 3
    }

    /// Dĺžka rámca voľného formátu bez výplňového bajtu: najmenšia vzdialenosť ďalšej hlavičky s rovnakou verziou,
    /// vrstvou, frekvenciou a tokom 0, za ktorou nasleduje ešte jedna taká hlavička (náhodná zhoda v údajoch sa
    /// tak nepoužije). `d` = údaje od začiatku rámca.
    pub fn dlzka_volneho_formatu(d: &[u8]) -> Option<usize> {
        if !Hlavicka::volny_format(d) {
            return None;
        }
        let zhodna = |i: usize| {
            d.get(i..i + 4).is_some_and(|h| {
                Hlavicka::volny_format(h) && h[1] & 0xFE == d[1] & 0xFE && h[2] & 0xFC == d[2] & 0xFC
            })
        };
        let vypln = |i: usize| ((d[i + 2] >> 1) & 1) as usize;
        (21..d.len().min(8192)).find_map(|i| {
            if !zhodna(i) {
                return None;
            }
            let zaklad = i - vypln(0);
            let dalsi = i + zaklad + vypln(i);
            (zhodna(dalsi) || dalsi >= d.len()).then_some(zaklad)
        })
    }

    pub fn z_bajtov(b: &[u8]) -> Option<Hlavicka> {
        if Hlavicka::volny_format(b) {
            // voľný formát: rámec je celý predaný úsek (dĺžku určil volajúci – `dlzka_volneho_formatu`)
            let verzia = match (b[1] >> 3) & 3 {
                3 => 1,
                2 => 2,
                _ => 25,
            };
            let f = ((b[2] >> 2) & 3) as usize;
            let zaklad = [44_100u32, 48_000, 32_000][f];
            return Some(Hlavicka {
                verzia,
                vrstva: 4 - ((b[1] >> 1) & 3),
                crc: b[1] & 1 == 0,
                frekvencia: match verzia {
                    1 => zaklad,
                    2 => zaklad / 2,
                    _ => zaklad / 4,
                },
                index_frekvencie: match verzia {
                    1 => f,
                    2 => 3 + f,
                    _ => 6 + f,
                },
                vypln: (b[2] >> 1) & 1 == 1,
                rezim: b[3] >> 6,
                rozsirenie: (b[3] >> 4) & 3,
                dlzka: b.len(),
            });
        }
        let r = crate::ramce::mpeg_audio(b)?;
        let verzia = match (b[1] >> 3) & 3 {
            3 => 1,
            2 => 2,
            _ => 25,
        };
        let f = ((b[2] >> 2) & 3) as usize;
        let index_frekvencie = match verzia {
            1 => f,
            2 => 3 + f,
            _ => 6 + f,
        };
        Some(Hlavicka {
            verzia,
            vrstva: r.vrstva,
            crc: b[1] & 1 == 0,
            frekvencia: r.frekvencia,
            index_frekvencie,
            vypln: (b[2] >> 1) & 1 == 1,
            rezim: b[3] >> 6,
            rozsirenie: (b[3] >> 4) & 3,
            dlzka: r.dlzka,
        })
    }
}

/// Vedľajšie informácie jednej granuly a kanála (tabuľka 4).
#[derive(Clone, Copy, Debug, Default)]
pub struct Granula {
    pub part2_3: u32,
    pub big_values: u32,
    pub global_gain: u32,
    pub scalefac_compress: u32,
    pub prepinanie: bool,
    pub typ_bloku: u8,
    pub zmiesany: bool,
    pub tabulky: [u8; 3],
    pub subblock_gain: [u8; 3],
    pub region0: u32,
    pub region1: u32,
    pub preflag: bool,
    pub scalefac_scale: bool,
    pub count1_tabulka: bool,
}

/// Vedľajšie informácie rámca (tabuľka 4) – MPEG-1 (2 granuly) aj MPEG-2 LSF / 2.5 (ID = 0, 1 granula).
#[derive(Clone, Debug, Default)]
pub struct VedlajsieInfo {
    pub main_data_begin: usize,
    pub scfsi: [[bool; 4]; 2],
    pub granuly: [[Granula; 2]; 2],
}

fn vedlajsie_info(b: &mut Bity, nch: usize, lsf: bool) -> VedlajsieInfo {
    let mut v = VedlajsieInfo {
        main_data_begin: b.citaj(if lsf { 8 } else { 9 }) as usize,
        ..VedlajsieInfo::default()
    };
    b.citaj(match (lsf, nch) {
        (false, 1) => 5,
        (false, _) => 3,
        (true, 1) => 1,
        (true, _) => 2,
    }); // private_bits
    if !lsf {
        for ch in 0..nch {
            for s in 0..4 {
                v.scfsi[ch][s] = b.bit() == 1;
            }
        }
    }
    for gr in 0..if lsf { 1 } else { 2 } {
        for ch in 0..nch {
            let g = &mut v.granuly[gr][ch];
            g.part2_3 = b.citaj(12);
            g.big_values = b.citaj(9);
            g.global_gain = b.citaj(8);
            g.scalefac_compress = b.citaj(if lsf { 9 } else { 4 });
            g.prepinanie = b.bit() == 1;
            if g.prepinanie {
                g.typ_bloku = b.citaj(2) as u8;
                g.zmiesany = b.bit() == 1;
                for r in 0..2 {
                    g.tabulky[r] = b.citaj(5) as u8;
                }
                for w in 0..3 {
                    g.subblock_gain[w] = b.citaj(3) as u8;
                }
                // [S] §4.2 region0_count, region1_count: implicitne pri prepínaní okien
                g.region0 = if g.typ_bloku == 2 && !g.zmiesany { 8 } else { 7 };
                g.region1 = 20 - g.region0; // všetko zvyšné v region1, region2 sa nepoužíva
            } else {
                for r in 0..3 {
                    g.tabulky[r] = b.citaj(5) as u8;
                }
                g.region0 = b.citaj(4);
                g.region1 = b.citaj(3);
            }
            // LSF: preflag sa neprenáša – určí ho scalefac_compress (skalovacie_lsf)
            if !lsf {
                g.preflag = b.bit() == 1;
            }
            g.scalefac_scale = b.bit() == 1;
            g.count1_tabulka = b.bit() == 1;
        }
    }
    v
}

/// Škálovacie faktory granuly (dlhé `l[sfb]`, krátke `s[sfb][okno]`) a počet bitov každého pásma (najväčšia
/// poloha intenzity LSF je 2^slen − 1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Skalovacie {
    pub l: [u8; 22],
    pub s: [[u8; 3]; 13],
    pub slen_l: [u8; 22],
    pub slen_s: [u8; 13],
    /// LSF: intensity_scale (scalefac_compress mod 2 pravého kanála pri intenzite)
    pub intensity_scale: bool,
}

/// Rozdelenie škálovacích faktorov LSF ([S] §4.2.4): (slen1–4, nr_of_sfb [dlhé, krátke, zmiešané][1–4], preflag).
/// Pri pravom kanáli intenzity (`kanal_intenzity`) sa delí int_scalefac_compress = scalefac_compress / 2.
fn rozdelenie_lsf(sc: u32, kanal_intenzity: bool) -> ([u32; 4], [[u32; 4]; 3], bool) {
    if !kanal_intenzity {
        if sc < 400 {
            (
                [(sc >> 4) / 5, (sc >> 4) % 5, (sc % 16) >> 2, sc % 4],
                [[6, 5, 5, 5], [9, 9, 9, 9], [6, 9, 9, 9]],
                false,
            )
        } else if sc < 500 {
            let x = sc - 400;
            ([(x >> 2) / 5, (x >> 2) % 5, x % 4, 0], [[6, 5, 7, 3], [9, 9, 12, 6], [6, 9, 12, 6]], false)
        } else {
            let x = sc - 500;
            ([x / 3, x % 3, 0, 0], [[11, 10, 0, 0], [18, 18, 0, 0], [15, 18, 0, 0]], true)
        }
    } else {
        let i = sc >> 1;
        if i < 180 {
            ([i / 36, (i % 36) / 6, (i % 36) % 6, 0], [[7, 7, 7, 0], [12, 12, 12, 0], [6, 15, 12, 0]], false)
        } else if i < 244 {
            let x = i - 180;
            ([(x % 64) >> 4, (x % 16) >> 2, x % 4, 0], [[6, 6, 6, 3], [12, 9, 9, 6], [6, 12, 9, 6]], false)
        } else {
            let x = i - 244;
            ([x / 3, x % 3, 0, 0], [[8, 8, 5, 0], [15, 12, 9, 0], [6, 18, 9, 0]], false)
        }
    }
}

/// Škálovacie faktory MPEG-2 LSF / 2.5 ([S] §4.2.4, scalefac_compress pri ID = 0): štyri časti s počtami pásiem
/// `nr_of_sfb1..4` a bitmi `slen1..4`; pravý kanál pri intenzite (mode_extension 01 alebo 11) iná tabuľka.
/// Vráti faktory a preflag.
fn skalovacie_lsf(b: &mut Bity, g: &Granula, kanal_intenzity: bool) -> (Skalovacie, bool) {
    let sc = g.scalefac_compress;
    let kratke = g.prepinanie && g.typ_bloku == 2;
    let typ = if !kratke {
        0
    } else if g.zmiesany {
        2
    } else {
        1
    };
    let mut sf = Skalovacie { intensity_scale: kanal_intenzity && sc & 1 == 1, ..Skalovacie::default() };
    let (slen, nr, preflag) = rozdelenie_lsf(sc, kanal_intenzity);
    // poradie prenosu: dlhé pásma 0–20, krátke pásmo po pásme a okno po okne, zmiešané 6 dlhých + krátke 3–11
    let mut miesta: Vec<(usize, Option<usize>)> = Vec::with_capacity(36);
    match typ {
        0 => miesta.extend((0..21).map(|sfb| (sfb, None))),
        1 => miesta.extend((0..12).flat_map(|sfb| (0..3).map(move |w| (sfb, Some(w))))),
        _ => {
            miesta.extend((0..6).map(|sfb| (sfb, None)));
            miesta.extend((3..12).flat_map(|sfb| (0..3).map(move |w| (sfb, Some(w)))));
        }
    }
    let mut k = 0;
    for cast in 0..4 {
        for _ in 0..nr[typ][cast] {
            let Some(&(sfb, w)) = miesta.get(k) else { break };
            let n = slen[cast];
            // slen 0 pri nenulovom počte pásiem: faktor 0 (poloha intenzity 0) – [S] §4.2.4
            let v = b.citaj(n) as u8;
            match w {
                None => {
                    sf.l[sfb] = v;
                    sf.slen_l[sfb] = n as u8;
                }
                Some(w) => {
                    sf.s[sfb][w] = v;
                    sf.slen_s[sfb] = n as u8;
                }
            }
            k += 1;
        }
    }
    (sf, preflag)
}

/// Škálovacie faktory MPEG-1 (tabuľka 5; dĺžky slen1/slen2 podľa tabuľky 16 a [S] 4.2.4 – slen1 pre pásma 0–10
/// dlhých, 0–5 krátkych; slen2 pre zvyšok; pri zmiešanom bloku dlhé 0–7 a krátke 3–5 slen1).
fn skalovacie(b: &mut Bity, g: &Granula, scfsi: &[bool; 4], gr: usize, pred: &Skalovacie) -> Skalovacie {
    let (slen1, slen2) = tabulky::SLEN[g.scalefac_compress as usize];
    let (slen1, slen2) = (slen1 as u32, slen2 as u32);
    let mut sf = Skalovacie::default();
    if g.prepinanie && g.typ_bloku == 2 {
        if g.zmiesany {
            for sfb in 0..8 {
                sf.l[sfb] = b.citaj(slen1) as u8;
            }
            for sfb in 3..12 {
                let n = if sfb < 6 { slen1 } else { slen2 };
                for w in 0..3 {
                    sf.s[sfb][w] = b.citaj(n) as u8;
                }
            }
        } else {
            for sfb in 0..12 {
                let n = if sfb < 6 { slen1 } else { slen2 };
                for w in 0..3 {
                    sf.s[sfb][w] = b.citaj(n) as u8;
                }
            }
        }
    } else {
        let sk = &tabulky::SCFSI_SKUPINY;
        for skupina in 0..4 {
            let (od, po) = (sk[skupina] as usize, sk[skupina + 1] as usize);
            if gr == 1 && scfsi[skupina] {
                sf.l[od..po].copy_from_slice(&pred.l[od..po]); // scfsi: prevezmú sa z granuly 0
            } else {
                let n = if od < 11 { slen1 } else { slen2 };
                for sfb in od..po {
                    sf.l[sfb] = b.citaj(n) as u8;
                }
            }
        }
    }
    sf
}

/// Strom Huffmanovho kódu: uzol = [ľavý, pravý]; list = 0x8000 | hodnota.
struct Strom {
    uzly: Vec<[u16; 2]>,
}

impl Strom {
    fn z_kodov(kody: &[(u32, u8, u8)]) -> Strom {
        let mut uzly = vec![[0u16; 2]];
        for &(kod, dlzka, hodnota) in kody {
            let mut u = 0usize;
            for i in (0..dlzka).rev() {
                let b = ((kod >> i) & 1) as usize;
                if i == 0 {
                    uzly[u][b] = 0x8000 | hodnota as u16;
                } else {
                    if uzly[u][b] == 0 {
                        uzly.push([0, 0]);
                        uzly[u][b] = (uzly.len() - 1) as u16;
                    }
                    u = uzly[u][b] as usize;
                }
            }
        }
        Strom { uzly }
    }
    fn dekoduj(&self, b: &mut Bity, koniec: usize) -> Option<u8> {
        let mut u = 0usize;
        loop {
            if b.poloha >= koniec + 32 {
                return None;
            }
            let v = self.uzly[u][b.bit() as usize];
            if v & 0x8000 != 0 {
                return Some((v & 0xFF) as u8);
            }
            if v == 0 {
                return None;
            }
            u = v as usize;
        }
    }
}

struct Stromy {
    velke: Vec<Strom>,
    a: Strom,
    b: Strom,
}

fn stromy() -> &'static Stromy {
    static S: OnceLock<Stromy> = OnceLock::new();
    S.get_or_init(|| Stromy {
        velke: tabulky::KODY.iter().map(|k| Strom::z_kodov(k)).collect(),
        a: Strom::z_kodov(&tabulky::HUFF_A),
        b: Strom::z_kodov(&tabulky::HUFF_B),
    })
}

/// Výsledok Huffmanovho dekódovania granuly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spotreba {
    /// skončilo presne na konci part2_3_length
    Presne,
    /// posledná štvorica prečnievala za koniec (zahodí sa)
    Prebeh,
    /// 576 hodnôt pred koncom – zvyšok sú výplňové bity
    Vypln,
    /// chyba (neplatný kód, priveľa bitov už v časti veľkých hodnôt)
    Chyba,
}

/// Huffmanovo dekódovanie (tabuľka 6, [S] §4.3.4): `is[0..576]`, vráti spotrebu a počet nenulových čiar.
fn huffman(
    b: &mut Bity,
    g: &Granula,
    koniec: usize,
    index_f: usize,
    is: &mut [i32; 576],
) -> (Spotreba, usize) {
    is.fill(0);
    let st = stromy();
    let dlhe = &tabulky::PASMA_DLHE[index_f];
    let kratke = &tabulky::PASMA_KRATKE[index_f];
    let bv = (g.big_values as usize * 2).min(576);
    // hranice oblastí region0/1/2 (v čiarach): pri krátkych blokoch sa pásma počítajú 3× (po oknách)
    let (r1, r2) = if g.prepinanie && g.typ_bloku == 2 && !g.zmiesany {
        (kratke[((g.region0 + 1) / 3) as usize] as usize * 3, 576)
    } else if g.prepinanie && g.typ_bloku == 2 && index_f >= 3 {
        // zmiešaný blok LSF – poznámka [S] k region0_count: dlhé pásma 0–5 a pásmo 3 prvých dvoch krátkych okien
        // (pri 24 kHz čiara 48). FFmpeg končí oblasť 0 s dlhou časťou (36 čiar, pri 8 kHz 72 – zmerané na
        // syntetických tokoch); text je jednoznačný, ponechaný, rozhodca túto hranicu neoveruje
        (dlhe[6] as usize + 2 * (kratke[4] - kratke[3]) as usize, 576)
    } else if g.prepinanie {
        (dlhe[(g.region0 + 1) as usize] as usize, 576)
    } else {
        let a = dlhe[((g.region0 + 1) as usize).min(22)] as usize;
        let c = dlhe[((g.region0 + g.region1 + 2) as usize).min(22)] as usize;
        (a, c)
    };
    let mut l = 0usize;
    while l < bv {
        let oblast = if l < r1 {
            0
        } else if l < r2 {
            1
        } else {
            2
        };
        let t = g.tabulky[oblast] as usize;
        let (ix, _rozmer, linbits) = tabulky::HUFF_TABULKY[t];
        let (mut x, mut y) = (0i32, 0i32);
        if ix != 0 {
            let Some(h) = st.velke[ix as usize].dekoduj(b, koniec) else { return (Spotreba::Chyba, l) };
            x = (h >> 4) as i32;
            y = (h & 15) as i32;
            if x == 15 && linbits > 0 {
                x += b.citaj(linbits as u32) as i32;
            }
            if x != 0 && b.bit() == 1 {
                x = -x;
            }
            if y == 15 && linbits > 0 {
                y += b.citaj(linbits as u32) as i32;
            }
            if y != 0 && b.bit() == 1 {
                y = -y;
            }
        }
        is[l] = x;
        is[l + 1] = y;
        l += 2;
    }
    if b.poloha > koniec {
        return (Spotreba::Chyba, l);
    }
    // oblasť count1: štvorice až do konca part2_3_length alebo 576 čiar
    let strom = if g.count1_tabulka { &st.b } else { &st.a };
    while l + 4 <= 576 {
        if b.poloha >= koniec {
            break;
        }
        let zaciatok = b.poloha;
        let Some(h) = strom.dekoduj(b, koniec) else { return (Spotreba::Chyba, l) };
        let mut q = [((h >> 3) & 1) as i32, ((h >> 2) & 1) as i32, ((h >> 1) & 1) as i32, (h & 1) as i32];
        for v in q.iter_mut() {
            if *v != 0 && b.bit() == 1 {
                *v = -*v;
            }
        }
        if b.poloha > koniec {
            // štvorica prečnieva: neplatná, zahodí sa ([S] 4.3.4 – dekóduje sa len do vyčerpania bitov)
            b.poloha = zaciatok;
            return (Spotreba::Prebeh, l);
        }
        is[l..l + 4].copy_from_slice(&q);
        l += 4;
    }
    if b.poloha == koniec { (Spotreba::Presne, l) } else { (Spotreba::Vypln, l) }
}

/// Počty granúl a nástrojov (pokrytie skúšok a dôkaz spotreby bitov).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatistikaMpa {
    pub ramcov: u64,
    pub granul: u64,
    pub presne: u64,
    pub prebeh: u64,
    pub vypln: u64,
    /// výplňové granuly, ktorých zvyšné bity sú všetky rovnaké (0…0 alebo 1…1) – výplň kodéra
    pub vypln_rovnaka: u64,
    pub chyby: u64,
    pub chyba_skalovacich: u64,
    pub kratke: u64,
    pub zmiesane: u64,
    pub start_stop: u64,
    pub scfsi: u64,
    pub preflag: u64,
    pub scalefac_scale: u64,
    pub linbits: u64,
    pub tabulka_b: u64,
    pub ms: u64,
    pub intenzita: u64,
    pub nedostatok_zasobnika: u64,
    /// granuly spojeného sterea s rôznym typom bloku v kanáloch (norma to zakazuje) – stereo sa nepoužije
    pub stereo_nezhoda: u64,
    /// pásma a okná dekódované intenzitným stereom
    pub intenzitne_pasma: u64,
}

/// Rozobraná granula jedného kanála: vedľajšie informácie, škálovacie faktory a kvantované spektrum.
#[derive(Clone, Debug)]
pub struct RozobranaGranula {
    pub granula: Granula,
    pub skalovacie: Skalovacie,
    pub is: [i32; 576],
}

/// Dekodér vrstvy III MPEG-1.
#[derive(Clone, Debug, Default)]
pub struct DekoderMpa {
    zasobnik: Vec<u8>,
    statistika: StatistikaMpa,
    /// posledný rozobraný rámec: `[gr·nch + ch]` (prázdne pri nedostatku zásobníka bitov)
    pub posledne: Vec<RozobranaGranula>,
    skalovacie: [Skalovacie; 2],
    synteza: [synteza::StavSyntezy; 2],
    hlavicka: Option<Hlavicka>,
}

impl DekoderMpa {
    pub fn novy() -> DekoderMpa {
        DekoderMpa::default()
    }

    pub fn statistika(&self) -> StatistikaMpa {
        self.statistika
    }

    /// Po posune: zásobník bitov a stav banky filtrov sa vymažú (predbeh dekódovania to dorovná).
    pub fn reset(&mut self) {
        self.zasobnik.clear();
        self.skalovacie = [Skalovacie::default(); 2];
        for s in &mut self.synteza {
            s.reset();
        }
    }

    /// Hlavička posledného rozobraného rámca.
    pub fn hlavicka(&self) -> Option<Hlavicka> {
        self.hlavicka
    }

    /// Dekóduje rámec: vrstva III MPEG-1 (1152 vzoriek na kanál), vrstva II (1152) a I (384) MPEG-1 aj MPEG-2
    /// LSF; rozsah ±1,0, `[kanál][vzorka]`.
    /// Pri nedostatku zásobníka bitov (začiatok toku po posune) dá ticho a banka filtrov pokračuje.
    pub fn dekoduj_ramec(&mut self, ramec: &[u8]) -> Vysledok<Vec<Vec<f32>>> {
        let h =
            Hlavicka::z_bajtov(ramec).ok_or_else(|| Chyba::PoskodeneData("MPEG audio: hlavička".into()))?;
        if h.vrstva != 3 {
            // vrstvy I a II: rámce sú nezávislé (bez zásobníka bitov), banka filtrov pokračuje
            self.hlavicka = Some(h);
            self.statistika.ramcov += 1;
            let mut von = vec![Vec::new(); h.kanalov()];
            vrstva12::dekoduj(&h, &ramec[..h.dlzka.min(ramec.len())], &mut self.synteza, &mut von)?;
            return Ok(von);
        }
        self.rozober_ramec(ramec)?;
        let h = self.hlavicka.ok_or_else(|| Chyba::PoskodeneData("MPEG audio: hlavička".into()))?;
        let nch = h.kanalov();
        let ngr = if h.verzia == 1 { 2 } else { 1 };
        let mut von = vec![vec![0f32; 576 * ngr]; nch];
        for gr in 0..ngr {
            let mut xr = [[0f32; 576]; 2];
            let mut bloky = [synteza::Blok { typ: 0, zmiesany: false, dlhych: 2, sb_amount: 0 }; 2];
            // koniec dlhej časti zmiešaného bloku: dlhé pásma 0–7 (MPEG-1), 0–5 (LSF) – v čiarach / 18 podpásiem
            let dlhych =
                (tabulky::PASMA_DLHE[h.index_frekvencie][if h.verzia == 1 { 8 } else { 6 }] / 18) as u8;
            if self.posledne.len() == ngr * nch {
                for ch in 0..nch {
                    let r = &self.posledne[gr * nch + ch];
                    rekonstrukcia::kvantizacia(
                        &r.is,
                        &r.granula,
                        &r.skalovacie,
                        h.index_frekvencie,
                        &mut xr[ch],
                    );
                    bloky[ch] = synteza::Blok {
                        typ: if r.granula.prepinanie { r.granula.typ_bloku } else { 0 },
                        zmiesany: r.granula.prepinanie && r.granula.zmiesany,
                        dlhych,
                        sb_amount: if h.verzia == 1 { 0 } else { dlhych },
                    };
                }
                let (ms, intenzita) =
                    (h.rezim == 1 && h.rozsirenie & 2 != 0, h.rezim == 1 && h.rozsirenie & 1 != 0);
                if nch == 2 && (ms || intenzita) {
                    let (gl, gp) = (&self.posledne[gr * 2].granula, &self.posledne[gr * 2 + 1].granula);
                    if bloky[0] != bloky[1] || gl.prepinanie != gp.prepinanie {
                        self.statistika.stereo_nezhoda += 1;
                    } else {
                        let p = &self.posledne[gr * 2 + 1];
                        let [l, pr] = &mut xr;
                        self.statistika.intenzitne_pasma += rekonstrukcia::stereo(
                            l,
                            pr,
                            &p.is,
                            &p.granula,
                            &p.skalovacie,
                            h.index_frekvencie,
                            ms,
                            intenzita,
                            h.verzia != 1,
                        ) as u64;
                    }
                }
                for ch in 0..nch {
                    rekonstrukcia::preradenie(
                        &mut xr[ch],
                        &self.posledne[gr * nch + ch].granula,
                        h.index_frekvencie,
                    );
                }
            }
            for ch in 0..nch {
                synteza::alias(&mut xr[ch], bloky[ch]);
                let mut pasma = [[0f32; 32]; 18];
                self.synteza[ch].hybrid(&xr[ch], bloky[ch], &mut pasma);
                for t in 0..18 {
                    let z = gr * 576 + 32 * t;
                    self.synteza[ch].polyfaza(&pasma[t], &mut von[ch][z..z + 32]);
                }
            }
        }
        Ok(von)
    }

    /// Rozoberie rámec vrstvy III MPEG-1: kvantované spektrum `posledne[gr·nch + ch]`.
    pub fn rozober_ramec(&mut self, ramec: &[u8]) -> Vysledok<()> {
        let h =
            Hlavicka::z_bajtov(ramec).ok_or_else(|| Chyba::PoskodeneData("MPEG audio: hlavička".into()))?;
        if h.vrstva != 3 {
            return Err(Chyba::MimoRozsahu("MPEG audio: rozbor granúl len pri vrstve III".into()));
        }
        let nch = h.kanalov();
        let lsf = h.verzia != 1;
        self.hlavicka = Some(h);
        let zac_vi = 4 + if h.crc { 2 } else { 0 };
        let dlzka_vi = match (lsf, nch) {
            (false, 1) => 17,
            (false, _) => 32,
            (true, 1) => 9,
            (true, _) => 17,
        };
        let telo = ramec
            .get(zac_vi..zac_vi + dlzka_vi)
            .ok_or_else(|| Chyba::PoskodeneData("MPEG audio: krátky".into()))?;
        let vi = vedlajsie_info(&mut Bity::nove(telo), nch, lsf);
        let ngr = if lsf { 1 } else { 2 };
        // zásobník bitov: hlavné údaje začínajú main_data_begin bajtov pred koncom doterajších hlavných údajov
        let main = ramec.get(zac_vi + dlzka_vi..h.dlzka.min(ramec.len())).unwrap_or_default();
        let st = &mut self.statistika;
        st.ramcov += 1;
        if vi.main_data_begin > self.zasobnik.len() {
            st.nedostatok_zasobnika += 1;
            self.zasobnik.extend_from_slice(main);
            let n = self.zasobnik.len();
            if n > 4096 {
                self.zasobnik.drain(..n - 4096);
            }
            self.posledne.clear();
            return Ok(());
        }
        let zaciatok = self.zasobnik.len() - vi.main_data_begin;
        self.zasobnik.extend_from_slice(main);
        let data = &self.zasobnik[zaciatok..];
        let mut b = Bity::nove(data);
        if h.rezim == 1 {
            st.ms += (h.rozsirenie & 2 != 0) as u64;
            st.intenzita += (h.rozsirenie & 1 != 0) as u64;
        }
        let mut vysl = Vec::with_capacity(ngr * nch);
        for gr in 0..ngr {
            for ch in 0..nch {
                let mut g = vi.granuly[gr][ch];
                let koniec = b.poloha + g.part2_3 as usize;
                let pred = self.skalovacie[ch];
                let sf = if lsf {
                    let intenzita = h.rezim == 1 && h.rozsirenie & 1 != 0 && ch == 1;
                    let (sf, preflag) = skalovacie_lsf(&mut b, &g, intenzita);
                    g.preflag = preflag;
                    sf
                } else {
                    skalovacie(&mut b, &g, &vi.scfsi[ch], gr, &pred)
                };
                self.skalovacie[ch] = sf;
                let mut is = [0i32; 576];
                st.granul += 1;
                if b.poloha > koniec {
                    st.chyba_skalovacich += 1;
                    b.poloha = koniec;
                } else {
                    let (s, _) = huffman(&mut b, &g, koniec, h.index_frekvencie, &mut is);
                    match s {
                        Spotreba::Presne => st.presne += 1,
                        Spotreba::Prebeh => st.prebeh += 1,
                        Spotreba::Vypln => {
                            st.vypln += 1;
                            let mut z = b.clone();
                            let prvy = z.bit();
                            st.vypln_rovnaka += (b.poloha + 1..koniec).all(|_| z.bit() == prvy) as u64;
                        }
                        Spotreba::Chyba => st.chyby += 1,
                    }
                }
                b.poloha = koniec;
                st.kratke += (g.prepinanie && g.typ_bloku == 2) as u64;
                st.zmiesane += (g.prepinanie && g.zmiesany) as u64;
                st.start_stop += (g.prepinanie && g.typ_bloku != 2) as u64;
                st.scfsi += (gr == 1 && vi.scfsi[ch].iter().any(|&x| x)) as u64;
                st.preflag += g.preflag as u64;
                st.scalefac_scale += g.scalefac_scale as u64;
                st.linbits += g.tabulky.iter().any(|&t| tabulky::HUFF_TABULKY[t as usize].2 > 0) as u64;
                st.tabulka_b += g.count1_tabulka as u64;
                vysl.push(RozobranaGranula { granula: g, skalovacie: sf, is });
            }
        }
        self.posledne = vysl;
        let n = self.zasobnik.len();
        if n > 4096 {
            self.zasobnik.drain(..n - 4096);
        }
        Ok(())
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn rozdelenie_lsf_pokryje_vsetky_pasma() {
        // každý scalefac_compress: 21 dlhých pásiem, 12 × 3 krátkych, 6 dlhých + 9 × 3 krátkych pri zmiešanom
        for sc in 0..512 {
            for intenzita in [false, true] {
                let (slen, nr, preflag) = rozdelenie_lsf(sc, intenzita);
                assert_eq!(nr.map(|r| r.iter().sum::<u32>()), [21, 36, 33], "sc {sc}");
                assert!(slen.iter().all(|&n| n <= if intenzita { 5 } else { 4 }));
                assert_eq!(preflag, !intenzita && sc >= 500);
                // part s nulovým počtom pásiem nemá bity
                assert!((0..4).all(|i| nr[0][i] > 0 || slen[i] == 0));
            }
        }
    }

    #[test]
    fn huffmanove_stromy_su_uplne() {
        // každá tabuľka: každý kód vedie k listu, žiadny uzol bez potomka (Kraft = 1 overený v generátore)
        let st = stromy();
        for s in st.velke.iter().skip(1).chain([&st.a, &st.b]) {
            assert!(s.uzly.iter().all(|u| u[0] != 0 && u[1] != 0));
        }
        // tabuľka 1 ([S] tabuľka 27): 1 → (0,0), 001 → (0,1), 01 → (1,0), 000 → (1,1)
        let data = [0b1001_0100u8, 0b0000_0000];
        let mut b = Bity::nove(&data);
        let s = &st.velke[1];
        assert_eq!(
            [s.dekoduj(&mut b, 16), s.dekoduj(&mut b, 16), s.dekoduj(&mut b, 16), s.dekoduj(&mut b, 16)],
            [Some(0x00), Some(0x01), Some(0x10), Some(0x11)]
        );
    }

    #[test]
    fn pasma_su_suvisle() {
        for f in 0..9 {
            assert!(tabulky::PASMA_DLHE[f].windows(2).all(|w| w[0] < w[1]));
            assert!(tabulky::PASMA_KRATKE[f].windows(2).all(|w| w[0] < w[1]));
            assert_eq!((tabulky::PASMA_DLHE[f][22], tabulky::PASMA_KRATKE[f][13]), (576, 192));
        }
    }
}
