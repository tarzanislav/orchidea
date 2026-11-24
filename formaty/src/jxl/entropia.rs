//! Entropické kódovanie JPEG XL: asymetrické číselné sústavy (ANS) alebo prefixové kódy, mapa kontextov,
//! hybridné celé čísla a voliteľné opakovania (LZ77).

use super::bity::{Citac, D, ceil_log2, chyba};
use orchidea_jadro::Vysledok;

const LOG_TABULKY: u32 = 12;
const VELKOST_TABULKY: u32 = 1 << LOG_TABULKY;

/// Nastavenie hybridného celého čísla: malé hodnoty priamo v znaku, väčšie s doplnkovými bitmi.
#[derive(Debug, Clone, Copy)]
struct HybridUint {
    split_exp: u32,
    msb: u32,
    lsb: u32,
}

impl HybridUint {
    fn citaj_nastavenie(c: &mut Citac, log_abeceda: u32) -> Vysledok<HybridUint> {
        let split_exp = c.u(ceil_log2(log_abeceda + 1));
        if split_exp > log_abeceda {
            return Err(chyba("neplatný split_exponent"));
        }
        let (mut msb, mut lsb) = (0, 0);
        if split_exp != log_abeceda {
            msb = c.u(ceil_log2(split_exp + 1));
            if msb > split_exp {
                return Err(chyba("neplatný msb_in_token"));
            }
            lsb = c.u(ceil_log2(split_exp - msb + 1));
            if msb + lsb > split_exp {
                return Err(chyba("neplatný lsb_in_token"));
            }
        }
        Ok(HybridUint { split_exp, msb, lsb })
    }

    #[inline]
    fn hodnota(&self, c: &mut Citac, znak: u32) -> Vysledok<u32> {
        let split = 1u32 << self.split_exp;
        if znak < split {
            return Ok(znak);
        }
        let n = self.split_exp - (self.msb + self.lsb) + ((znak - split) >> (self.msb + self.lsb));
        if n > 31 {
            return Err(chyba("hybridné číslo nad 32 bitov"));
        }
        let low = znak & ((1 << self.lsb) - 1);
        let t = znak >> self.lsb;
        let bity = c.u(n);
        let hi = t & ((1 << self.msb) - 1);
        let v = (((((1u64 << self.msb) | hi as u64) << n) | bity as u64) << self.lsb) | low as u64;
        u32::try_from(v).map_err(|_| chyba("hybridné číslo nad 32 bitov"))
    }
}

/// Rozdelenie ANS s tabuľkou aliasov (norma D.3.6).
#[derive(Clone)]
struct Ans {
    pravd: Vec<u32>,
    cutoff: Vec<u32>,
    vpravo: Vec<u32>,
    posun: Vec<u32>,
    log_vedro: u32,
}

impl Ans {
    fn nova(d: Vec<u32>, log_abeceda: u32) -> Vysledok<Ans> {
        let velkost = 1usize << log_abeceda;
        let log_vedro = LOG_TABULKY - log_abeceda;
        let vedro = 1u32 << log_vedro;
        if d.len() > velkost {
            return Err(chyba("rozdelenie je väčšie ako abeceda"));
        }
        let mut pravd = d;
        pravd.resize(velkost, 0);
        let mut cutoff = vec![0u32; velkost];
        let mut vpravo = vec![0u32; velkost];
        let mut posun = vec![0u32; velkost];
        // jediný znak s celou pravdepodobnosťou – všetky vedrá ukazujú naň (stav sa nemení)
        if let Some(s) = pravd.iter().position(|&p| p == VELKOST_TABULKY) {
            for i in 0..velkost {
                vpravo[i] = s as u32;
                cutoff[i] = 0;
                posun[i] = vedro * i as u32;
            }
            return Ok(Ans { pravd, cutoff, vpravo, posun, log_vedro });
        }
        let mut podplne = Vec::new();
        let mut preplne = Vec::new();
        for i in 0..velkost {
            cutoff[i] = pravd[i];
            if cutoff[i] > vedro {
                preplne.push(i);
            } else if cutoff[i] < vedro {
                podplne.push(i);
            }
        }
        while let Some(o) = preplne.pop() {
            let u = podplne.pop().ok_or_else(|| chyba("neplatné rozdelenie ANS"))?;
            let chyba_u = vedro - cutoff[u];
            cutoff[o] -= chyba_u;
            vpravo[u] = o as u32;
            posun[u] = cutoff[o];
            if cutoff[o] < vedro {
                podplne.push(o);
            } else if cutoff[o] > vedro {
                preplne.push(o);
            }
        }
        for i in 0..velkost {
            if cutoff[i] == vedro {
                vpravo[i] = i as u32;
                posun[i] = 0;
                cutoff[i] = 0;
            } else {
                posun[i] = posun[i].wrapping_sub(cutoff[i]);
            }
        }
        Ok(Ans { pravd, cutoff, vpravo, posun, log_vedro })
    }

    #[inline]
    fn znak(&self, stav: &mut u32, c: &mut Citac) -> u32 {
        let x = *stav & (VELKOST_TABULKY - 1);
        let i = (x >> self.log_vedro) as usize;
        let pos = x & ((1 << self.log_vedro) - 1);
        let (s, off) = if pos >= self.cutoff[i] {
            (self.vpravo[i], self.posun[i].wrapping_add(pos))
        } else {
            (i as u32, pos)
        };
        *stav = self.pravd[s as usize].wrapping_mul(*stav >> LOG_TABULKY).wrapping_add(off);
        if *stav < (1 << 16) {
            *stav = (*stav << 16) | c.u(16);
        }
        s
    }
}

/// Prefixový kód (Brotli, RFC 7932 3.4–3.5) pre jeden zhluk.
#[derive(Clone)]
struct Prefix {
    /// Jediný znak bez bitov.
    jediny: Option<u32>,
    /// Tabuľka podľa 15 bitov: (dĺžka, znak); dlhšie kódy nie sú (Brotli má najviac 15).
    tabulka: Vec<(u8, u16)>,
}

const MAX_PREFIX: u32 = 15;

impl Prefix {
    fn z_dlzok(dlzky: &[u8]) -> Vysledok<Prefix> {
        let pocet_nenulovych = dlzky.iter().filter(|&&l| l > 0).count();
        if pocet_nenulovych == 0 {
            return Err(chyba("prázdny prefixový kód"));
        }
        if pocet_nenulovych == 1 {
            let s = dlzky.iter().position(|&l| l > 0).unwrap_or(0);
            return Ok(Prefix { jediny: Some(s as u32), tabulka: Vec::new() });
        }
        let mut kraft = 0u64;
        for &l in dlzky {
            if l > 0 {
                kraft += 1 << (MAX_PREFIX - l as u32);
            }
        }
        if kraft != 1 << MAX_PREFIX {
            return Err(chyba("neúplný alebo prečerpaný prefixový kód"));
        }
        let kody = crate::huffman::kanonicke_kody(dlzky);
        let mut tabulka = vec![(0u8, 0u16); 1 << MAX_PREFIX];
        for (s, &l) in dlzky.iter().enumerate() {
            if l == 0 {
                continue;
            }
            let r = crate::huffman::obrat_bity(kody[s], l as u32);
            let mut i = r as usize;
            while i < tabulka.len() {
                tabulka[i] = (l, s as u16);
                i += 1 << l;
            }
        }
        Ok(Prefix { jediny: None, tabulka })
    }

    #[inline]
    fn znak(&self, c: &mut Citac) -> u32 {
        if let Some(s) = self.jediny {
            return s;
        }
        let (l, s) = self.tabulka[c.pozri(MAX_PREFIX) as usize];
        c.zahod(l as u32);
        s as u32
    }

    /// Prečíta kód s abecedou `velkost`.
    fn citaj(c: &mut Citac, velkost: u32) -> Vysledok<Prefix> {
        if velkost == 1 {
            return Ok(Prefix { jediny: Some(0), tabulka: Vec::new() });
        }
        let hskip = c.u(2);
        let bity_abecedy = ceil_log2(velkost);
        let mut dlzky = vec![0u8; velkost as usize];
        if hskip == 1 {
            // jednoduchý kód: 1–4 znaky
            let nsym = c.u(2) + 1;
            let mut znaky = Vec::with_capacity(4);
            for _ in 0..nsym {
                let s = c.u(bity_abecedy);
                if s >= velkost || znaky.contains(&s) {
                    return Err(chyba("neplatný jednoduchý prefixový kód"));
                }
                znaky.push(s);
            }
            match nsym {
                1 => return Ok(Prefix { jediny: Some(znaky[0]), tabulka: Vec::new() }),
                2 => {
                    dlzky[znaky[0] as usize] = 1;
                    dlzky[znaky[1] as usize] = 1;
                }
                3 => {
                    dlzky[znaky[0] as usize] = 1;
                    dlzky[znaky[1] as usize] = 2;
                    dlzky[znaky[2] as usize] = 2;
                }
                _ => {
                    if c.bool() {
                        let mut z = znaky.clone();
                        z[2..].sort();
                        dlzky[z[0] as usize] = 1;
                        dlzky[z[1] as usize] = 2;
                        dlzky[z[2] as usize] = 3;
                        dlzky[z[3] as usize] = 3;
                    } else {
                        for &s in &znaky {
                            dlzky[s as usize] = 2;
                        }
                    }
                }
            }
            return Prefix::z_dlzok(&dlzky);
        }
        // zložitý kód: najprv dĺžky kódu dĺžok
        const PORADIE: [usize; 18] = [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let mut dlzky_dlzok = [0u8; 18];
        let mut priestor = 32i32;
        let mut nenulovych = 0;
        for &i in &PORADIE[hskip as usize..] {
            // pevný kód dĺžok (bity čítané od najnižšieho)
            let v = c.pozri(4) as u32;
            let (l, d) = match v & 3 {
                0 => (0, 2),
                1 => (4, 2),
                2 => (3, 2),
                _ => match v & 7 {
                    3 => (2, 3),
                    _ => {
                        if v == 7 {
                            (1, 4)
                        } else {
                            (5, 4)
                        }
                    }
                },
            };
            c.zahod(d);
            dlzky_dlzok[i] = l;
            if l != 0 {
                priestor -= 32 >> l;
                nenulovych += 1;
                if priestor <= 0 {
                    break;
                }
            }
        }
        if nenulovych != 1 && priestor != 0 {
            return Err(chyba("neplatný kód dĺžok"));
        }
        let kod_dlzok = Prefix::z_dlzok(&dlzky_dlzok)?;
        let mut i = 0usize;
        let mut predosla = 8u8;
        let mut opakuj = 0u32;
        let mut opakovana = 0u8;
        let mut priestor = 32_768i32;
        while i < velkost as usize && priestor > 0 {
            let z = kod_dlzok.znak(c);
            if z < 16 {
                opakuj = 0;
                dlzky[i] = z as u8;
                i += 1;
                if z != 0 {
                    predosla = z as u8;
                    priestor -= 32_768 >> z;
                }
            } else {
                let (extra, nova) = if z == 16 { (2, predosla) } else { (3, 0) };
                if opakovana != nova {
                    opakuj = 0;
                    opakovana = nova;
                }
                let stare = opakuj;
                if opakuj > 0 {
                    opakuj -= 2;
                    opakuj <<= extra;
                }
                opakuj += c.u(extra) + 3;
                let delta = (opakuj - stare) as usize;
                if i + delta > velkost as usize {
                    return Err(chyba("opakovanie dĺžok za koncom abecedy"));
                }
                dlzky[i..i + delta].fill(opakovana);
                i += delta;
                if opakovana != 0 {
                    priestor -= (delta as i32) << (15 - opakovana as i32);
                }
            }
        }
        if priestor != 0 {
            return Err(chyba("neúplný prefixový kód"));
        }
        Prefix::z_dlzok(&dlzky)
    }
}

#[derive(Clone)]
enum Kody {
    Ans(Vec<Ans>),
    Prefix(Vec<Prefix>),
}

#[derive(Clone, Copy, Debug)]
struct Lz77 {
    min_znak: u32,
    min_dlzka: u32,
    dlzka: HybridUint,
}

/// Nastavenie entropického dekodéra (rozdelenia, mapa kontextov, LZ77) – môže sa použiť viackrát.
#[derive(Clone)]
pub(super) struct Histogramy {
    lz77: Option<Lz77>,
    mapa: Vec<u8>,
    nastavenia: Vec<HybridUint>,
    kody: Kody,
}

/// Prečíta `n` z abecedy kódu dĺžok rozdelenia ANS (prefixový kód z normy, 7 bitov).
fn log_pocet(c: &mut Citac) -> u32 {
    // (dĺžka, kód od najnižšieho bitu) pre hodnoty 0–13; 13 = opakovanie
    const DLZKY: [u32; 14] = [5, 4, 4, 4, 4, 4, 3, 3, 3, 3, 3, 6, 7, 7];
    const KODY: [u32; 14] = [17, 11, 15, 3, 9, 7, 4, 2, 5, 6, 0, 33, 1, 65];
    let v = c.pozri(7) as u32;
    for s in 0..14 {
        if v & ((1 << DLZKY[s]) - 1) == KODY[s] {
            c.zahod(DLZKY[s]);
            return s as u32;
        }
    }
    unreachable!("kód je úplný")
}

fn rozdelenie_ans(c: &mut Citac, log_abeceda: u32) -> Vysledok<Vec<u32>> {
    let abeceda = 1usize << log_abeceda;
    if c.bool() {
        // jednoduché: jeden alebo dva znaky
        if c.bool() {
            let (v1, v2) = (c.u8_() as usize, c.u8_() as usize);
            if v1 == v2 || v1 >= abeceda || v2 >= abeceda {
                return Err(chyba("neplatné jednoduché rozdelenie"));
            }
            let p = c.u(LOG_TABULKY);
            let mut d = vec![0u32; v1.max(v2) + 1];
            d[v1] = p;
            d[v2] = VELKOST_TABULKY - p;
            return Ok(d);
        }
        let v = c.u8_() as usize;
        if v >= abeceda {
            return Err(chyba("znak mimo abecedy"));
        }
        let mut d = vec![0u32; v + 1];
        d[v] = VELKOST_TABULKY;
        return Ok(d);
    }
    if c.bool() {
        // rovnomerné
        let n = c.u8_() as usize + 1;
        if n > abeceda {
            return Err(chyba("rovnomerné rozdelenie nad abecedou"));
        }
        let mut d = vec![VELKOST_TABULKY / n as u32; n];
        for x in d.iter_mut().take(VELKOST_TABULKY as usize % n) {
            *x += 1;
        }
        return Ok(d);
    }
    let mut dlzka = 0;
    while dlzka < 3 && c.bool() {
        dlzka += 1;
    }
    let posun = c.u(dlzka) + (1 << dlzka) - 1;
    if posun > 13 {
        return Err(chyba("neplatný posun rozdelenia"));
    }
    let n = c.u8_() as usize + 3;
    if n > abeceda {
        return Err(chyba("rozdelenie nad abecedou"));
    }
    let mut logy = vec![0u32; n];
    let mut rovnake = vec![0u32; n];
    let (mut vynechaj_log, mut vynechaj_poz) = (-1i64, -1i64);
    let mut i = 0;
    while i < n {
        logy[i] = log_pocet(c);
        if logy[i] == 13 {
            let r = c.u8_();
            rovnake[i] = r + 5;
            i += r as usize + 4;
            continue;
        }
        if logy[i] as i64 > vynechaj_log {
            vynechaj_log = logy[i] as i64;
            vynechaj_poz = i as i64;
        }
        i += 1;
    }
    if vynechaj_poz < 0 || i > n {
        return Err(chyba("neplatné rozdelenie (chýba najväčší počet)"));
    }
    let mut d = vec![0u32; n];
    let mut spolu = 0u32;
    let (mut predosly, mut zostava) = (0u32, 0u32);
    for i in 0..n {
        if rovnake[i] != 0 {
            zostava = rovnake[i] - 1;
            predosly = if i > 0 { d[i - 1] } else { 0 };
        }
        if zostava > 0 {
            d[i] = predosly;
            zostava -= 1;
        } else {
            let kod = logy[i];
            if i as i64 == vynechaj_poz || kod == 0 {
                continue;
            } else if kod == 1 {
                d[i] = 1;
            } else {
                let lc = kod - 1;
                let presnost =
                    (lc as i32).min((posun as i32 - ((LOG_TABULKY - lc) >> 1) as i32).max(0)) as u32;
                d[i] = (1 << lc) + (c.u(presnost) << (lc - presnost));
            }
        }
        spolu = spolu.saturating_add(d[i]);
    }
    if spolu >= VELKOST_TABULKY {
        return Err(chyba("rozdelenie prekračuje súčet 4096"));
    }
    d[vynechaj_poz as usize] = VELKOST_TABULKY - spolu;
    Ok(d)
}

impl Histogramy {
    /// Prečíta nastavenie pre `pocet_kontextov` kontextov.
    pub fn citaj(c: &mut Citac, pocet_kontextov: usize) -> Vysledok<Histogramy> {
        Histogramy::citaj_vnutri(c, pocet_kontextov, 0)
    }

    fn citaj_vnutri(c: &mut Citac, mut pocet_kontextov: usize, hlbka: u32) -> Vysledok<Histogramy> {
        if hlbka > 2 {
            return Err(chyba("príliš hlboko vnorená mapa kontextov"));
        }
        let lz77 = if c.bool() {
            let min_znak = c.u32([D::V(224), D::V(512), D::V(4096), D::B(15, 8)]);
            let min_dlzka = c.u32([D::V(3), D::V(4), D::B(2, 5), D::B(8, 9)]);
            let dlzka = HybridUint::citaj_nastavenie(c, 8)?;
            pocet_kontextov += 1;
            Some(Lz77 { min_znak, min_dlzka, dlzka })
        } else {
            None
        };
        if pocet_kontextov > 1 << 20 {
            return Err(chyba("priveľa kontextov"));
        }
        let mapa = if pocet_kontextov > 1 { mapa_kontextov(c, pocet_kontextov, hlbka)? } else { vec![0] };
        let zhlukov = *mapa.iter().max().unwrap_or(&0) as usize + 1;
        let prefix = c.bool();
        let log_abeceda = if prefix { 15 } else { 5 + c.u(2) };
        let mut nastavenia = Vec::with_capacity(zhlukov);
        for _ in 0..zhlukov {
            nastavenia.push(HybridUint::citaj_nastavenie(c, log_abeceda)?);
        }
        let kody = if prefix {
            let mut velkosti = Vec::with_capacity(zhlukov);
            for _ in 0..zhlukov {
                velkosti.push(if c.bool() {
                    let n = c.u(4);
                    1 + (1 << n) + c.u(n)
                } else {
                    1
                });
            }
            let mut v = Vec::with_capacity(zhlukov);
            for &n in &velkosti {
                if n > 1 << 15 {
                    return Err(chyba("priveľká abeceda prefixového kódu"));
                }
                v.push(Prefix::citaj(c, n)?);
            }
            Kody::Prefix(v)
        } else {
            let mut v = Vec::with_capacity(zhlukov);
            for _ in 0..zhlukov {
                v.push(Ans::nova(rozdelenie_ans(c, log_abeceda)?, log_abeceda)?);
            }
            Kody::Ans(v)
        };
        c.over()?;
        Ok(Histogramy { lz77, mapa, nastavenia, kody })
    }

    /// Začne čítať tok znakov (ANS potrebuje úvodný 32-bitový stav).
    pub fn citac(&self, c: &mut Citac) -> TokZnakov {
        let stav = match self.kody {
            Kody::Ans(_) => c.u(32),
            Kody::Prefix(_) => 0,
        };
        TokZnakov {
            stav,
            okno: Vec::new(),
            lz77: self.lz77.is_some(),
            dekodovanych: 0,
            kopirovat: 0,
            poz_kopie: 0,
            nasobok_vzdialenosti: 0,
        }
    }
}

fn mapa_kontextov(c: &mut Citac, n: usize, hlbka: u32) -> Vysledok<Vec<u8>> {
    let mut mapa = vec![0u8; n];
    if c.bool() {
        let bity = c.u(2);
        for x in mapa.iter_mut() {
            *x = c.u(bity) as u8;
        }
    } else {
        let mtf = c.bool();
        let h = Histogramy::citaj_vnutri(c, 1, hlbka + 1)?;
        let mut t = h.citac(c);
        for x in mapa.iter_mut() {
            let v = t.citaj(&h, c, 0)?;
            if v >= 256 {
                return Err(chyba("zhluk nad 255"));
            }
            *x = v as u8;
        }
        t.koniec(&h)?;
        if mtf {
            let mut poradie: Vec<u8> = (0..=255).collect();
            for x in mapa.iter_mut() {
                let i = *x as usize;
                let v = poradie[i];
                poradie.copy_within(0..i, 1);
                poradie[0] = v;
                *x = v;
            }
        }
    }
    // všetky zhluky do najväčšieho musia byť použité
    let max = *mapa.iter().max().unwrap_or(&0) as usize;
    let mut videne = vec![false; max + 1];
    for &x in &mapa {
        videne[x as usize] = true;
    }
    if videne.iter().any(|&v| !v) {
        return Err(chyba("mapa kontextov s nepoužitým zhlukom"));
    }
    Ok(mapa)
}

/// Vzdialenosti v LZ77 pri násobku (dx, dy) – relatívne polohy v obraze (norma, tabuľka 120 dvojíc).
const VZDIALENOSTI: [(i8, i8); 120] = [
    (0, 1),
    (1, 0),
    (1, 1),
    (-1, 1),
    (0, 2),
    (2, 0),
    (1, 2),
    (-1, 2),
    (2, 1),
    (-2, 1),
    (2, 2),
    (-2, 2),
    (0, 3),
    (3, 0),
    (1, 3),
    (-1, 3),
    (3, 1),
    (-3, 1),
    (2, 3),
    (-2, 3),
    (3, 2),
    (-3, 2),
    (0, 4),
    (4, 0),
    (1, 4),
    (-1, 4),
    (4, 1),
    (-4, 1),
    (3, 3),
    (-3, 3),
    (2, 4),
    (-2, 4),
    (4, 2),
    (-4, 2),
    (0, 5),
    (3, 4),
    (-3, 4),
    (4, 3),
    (-4, 3),
    (5, 0),
    (1, 5),
    (-1, 5),
    (5, 1),
    (-5, 1),
    (2, 5),
    (-2, 5),
    (5, 2),
    (-5, 2),
    (4, 4),
    (-4, 4),
    (3, 5),
    (-3, 5),
    (5, 3),
    (-5, 3),
    (0, 6),
    (6, 0),
    (1, 6),
    (-1, 6),
    (6, 1),
    (-6, 1),
    (2, 6),
    (-2, 6),
    (6, 2),
    (-6, 2),
    (4, 5),
    (-4, 5),
    (5, 4),
    (-5, 4),
    (3, 6),
    (-3, 6),
    (6, 3),
    (-6, 3),
    (0, 7),
    (7, 0),
    (1, 7),
    (-1, 7),
    (5, 5),
    (-5, 5),
    (7, 1),
    (-7, 1),
    (4, 6),
    (-4, 6),
    (6, 4),
    (-6, 4),
    (2, 7),
    (-2, 7),
    (7, 2),
    (-7, 2),
    (3, 7),
    (-3, 7),
    (7, 3),
    (-7, 3),
    (5, 6),
    (-5, 6),
    (6, 5),
    (-6, 5),
    (8, 0),
    (4, 7),
    (-4, 7),
    (7, 4),
    (-7, 4),
    (8, 1),
    (8, 2),
    (6, 6),
    (-6, 6),
    (8, 3),
    (5, 7),
    (-5, 7),
    (7, 5),
    (-7, 5),
    (8, 4),
    (6, 7),
    (-6, 7),
    (7, 6),
    (-7, 6),
    (8, 5),
    (7, 7),
    (-7, 7),
    (8, 6),
    (8, 7),
];

/// Stav čítania jedného toku znakov.
pub(super) struct TokZnakov {
    stav: u32,
    /// Okno LZ77 (rastie podľa potreby až po 2^20 hodnôt, potom kruhovo).
    okno: Vec<u32>,
    lz77: bool,
    dekodovanych: u64,
    kopirovat: u32,
    poz_kopie: u64,
    /// Násobok pre vzdialenosti LZ77 (šírka kanála v režime Modular, inak 0).
    pub nasobok_vzdialenosti: u32,
}

impl TokZnakov {
    #[inline]
    fn znak(&mut self, h: &Histogramy, c: &mut Citac, zhluk: usize) -> u32 {
        match &h.kody {
            Kody::Ans(v) => v[zhluk].znak(&mut self.stav, c),
            Kody::Prefix(v) => v[zhluk].znak(c),
        }
    }

    /// Prečíta hybridné celé číslo v kontexte `ctx`.
    #[inline]
    pub fn citaj(&mut self, h: &Histogramy, c: &mut Citac, ctx: usize) -> Vysledok<u32> {
        if self.kopirovat > 0 {
            let r = self.okno[(self.poz_kopie & 0xFFFFF) as usize];
            self.poz_kopie += 1;
            self.kopirovat -= 1;
            self.uloz(r);
            return Ok(r);
        }
        let zhluk = *h.mapa.get(ctx).ok_or_else(|| chyba("kontext mimo mapy"))? as usize;
        let znak = self.znak(h, c, zhluk);
        if let Some(lz) = h.lz77
            && znak >= lz.min_znak
        {
            let dlzka = lz.dlzka.hodnota(c, znak - lz.min_znak)?.saturating_add(lz.min_dlzka);
            let zv = h.mapa[h.mapa.len() - 1] as usize;
            let tv = self.znak(h, c, zv);
            let mut vzd = h.nastavenia[zv].hodnota(c, tv)? as u64;
            if self.nasobok_vzdialenosti == 0 {
                vzd += 1;
            } else if vzd < 120 {
                let (dx, dy) = VZDIALENOSTI[vzd as usize];
                vzd = (dx as i64 + self.nasobok_vzdialenosti as i64 * dy as i64).max(1) as u64;
            } else {
                vzd -= 119;
            }
            if self.dekodovanych == 0 {
                return Err(chyba("LZ77 bez predchádzajúcich hodnôt"));
            }
            vzd = vzd.min(self.dekodovanych).min(1 << 20);
            self.poz_kopie = self.dekodovanych - vzd;
            self.kopirovat = dlzka;
            return self.citaj(h, c, ctx);
        }
        let r = h.nastavenia[zhluk].hodnota(c, znak)?;
        if self.lz77 {
            self.uloz(r);
        }
        Ok(r)
    }

    #[inline]
    fn uloz(&mut self, r: u32) {
        if self.okno.len() < 1 << 20 {
            self.okno.push(r);
        } else {
            self.okno[(self.dekodovanych & 0xFFFFF) as usize] = r;
        }
        self.dekodovanych += 1;
    }

    /// Koniec toku: stav ANS sa musí vrátiť na počiatočnú hodnotu.
    pub fn koniec(&self, h: &Histogramy) -> Vysledok<()> {
        if matches!(h.kody, Kody::Ans(_)) && self.stav != 0x13_0000 {
            return Err(chyba("konečný stav ANS nesedí"));
        }
        Ok(())
    }
}
