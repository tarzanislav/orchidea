//! Odtlačok – vlastná implementácia hašovacej funkcie BLAKE3 podľa jej špecifikácie.
//!
//! Slúži na adresovanie obsahu (kúsky diela, vyrovnávacia pamäť grafu), na identitu zmien v denníku
//! a na overenie bitovej presnosti. Podporuje bežný odtlačok, odtlačok s kľúčom a odvodenie kľúča.
//! Správnosť overujú testy s hodnotami z nezávislej referenčnej implementácie.

use std::fmt;

const DLZKA_BLOKU: usize = 64;
const DLZKA_KUSKA: usize = 1024;

const ZACIATOK_KUSKA: u32 = 1 << 0;
const KONIEC_KUSKA: u32 = 1 << 1;
const RODIC: u32 = 1 << 2;
const KOREN: u32 = 1 << 3;
const S_KLUCOM: u32 = 1 << 4;
const KONTEXT_ODVODENIA: u32 = 1 << 5;
const MATERIAL_ODVODENIA: u32 = 1 << 6;

const IV: [u32; 8] =
    [0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C, 0x1F83D9AB, 0x5BE0CD19];

const PERMUTACIA: [usize; 16] = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];

#[inline(always)]
fn g(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize, x: u32, y: u32) {
    s[a] = s[a].wrapping_add(s[b]).wrapping_add(x);
    s[d] = (s[d] ^ s[a]).rotate_right(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_right(12);
    s[a] = s[a].wrapping_add(s[b]).wrapping_add(y);
    s[d] = (s[d] ^ s[a]).rotate_right(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_right(7);
}

#[inline(always)]
fn kolo(s: &mut [u32; 16], m: &[u32; 16]) {
    g(s, 0, 4, 8, 12, m[0], m[1]);
    g(s, 1, 5, 9, 13, m[2], m[3]);
    g(s, 2, 6, 10, 14, m[4], m[5]);
    g(s, 3, 7, 11, 15, m[6], m[7]);
    g(s, 0, 5, 10, 15, m[8], m[9]);
    g(s, 1, 6, 11, 12, m[10], m[11]);
    g(s, 2, 7, 8, 13, m[12], m[13]);
    g(s, 3, 4, 9, 14, m[14], m[15]);
}

fn komprimuj(cv: &[u32; 8], blok: &[u32; 16], pocitadlo: u64, dlzka: u32, priznaky: u32) -> [u32; 16] {
    let mut s = [
        cv[0],
        cv[1],
        cv[2],
        cv[3],
        cv[4],
        cv[5],
        cv[6],
        cv[7],
        IV[0],
        IV[1],
        IV[2],
        IV[3],
        pocitadlo as u32,
        (pocitadlo >> 32) as u32,
        dlzka,
        priznaky,
    ];
    let mut m = *blok;
    for i in 0..7 {
        kolo(&mut s, &m);
        if i < 6 {
            let mut p = [0u32; 16];
            for (j, &k) in PERMUTACIA.iter().enumerate() {
                p[j] = m[k];
            }
            m = p;
        }
    }
    for i in 0..8 {
        s[i] ^= s[i + 8];
        s[i + 8] ^= cv[i];
    }
    s
}

fn slova(bajty: &[u8; DLZKA_BLOKU]) -> [u32; 16] {
    let mut w = [0u32; 16];
    for (i, kus) in bajty.chunks_exact(4).enumerate() {
        w[i] = u32::from_le_bytes([kus[0], kus[1], kus[2], kus[3]]);
    }
    w
}

fn prvych_8(s: [u32; 16]) -> [u32; 8] {
    [s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]
}

#[derive(Clone, Copy)]
struct Vystup {
    cv: [u32; 8],
    blok: [u32; 16],
    pocitadlo: u64,
    dlzka: u32,
    priznaky: u32,
}

impl Vystup {
    fn retazova_hodnota(&self) -> [u32; 8] {
        prvych_8(komprimuj(&self.cv, &self.blok, self.pocitadlo, self.dlzka, self.priznaky))
    }

    fn korenove_bajty(&self, ciel: &mut [u8]) {
        for (i, kus) in ciel.chunks_mut(2 * 32).enumerate() {
            let w = komprimuj(&self.cv, &self.blok, i as u64, self.dlzka, self.priznaky | KOREN);
            for (slovo, bajty) in w.iter().zip(kus.chunks_mut(4)) {
                bajty.copy_from_slice(&slovo.to_le_bytes()[..bajty.len()]);
            }
        }
    }
}

#[derive(Clone)]
struct StavKuska {
    cv: [u32; 8],
    pocitadlo: u64,
    blok: [u8; DLZKA_BLOKU],
    dlzka_bloku: usize,
    skomprimovanych: usize,
    priznaky: u32,
}

impl StavKuska {
    fn novy(kluc: [u32; 8], pocitadlo: u64, priznaky: u32) -> Self {
        StavKuska {
            cv: kluc,
            pocitadlo,
            blok: [0; DLZKA_BLOKU],
            dlzka_bloku: 0,
            skomprimovanych: 0,
            priznaky,
        }
    }

    fn dlzka(&self) -> usize {
        DLZKA_BLOKU * self.skomprimovanych + self.dlzka_bloku
    }

    fn priznak_zaciatku(&self) -> u32 {
        if self.skomprimovanych == 0 { ZACIATOK_KUSKA } else { 0 }
    }

    fn pridaj(&mut self, mut vstup: &[u8]) {
        while !vstup.is_empty() {
            if self.dlzka_bloku == DLZKA_BLOKU {
                let w = slova(&self.blok);
                self.cv = prvych_8(komprimuj(
                    &self.cv,
                    &w,
                    self.pocitadlo,
                    DLZKA_BLOKU as u32,
                    self.priznaky | self.priznak_zaciatku(),
                ));
                self.skomprimovanych += 1;
                self.blok = [0; DLZKA_BLOKU];
                self.dlzka_bloku = 0;
            }
            let vezmi = (DLZKA_BLOKU - self.dlzka_bloku).min(vstup.len());
            self.blok[self.dlzka_bloku..self.dlzka_bloku + vezmi].copy_from_slice(&vstup[..vezmi]);
            self.dlzka_bloku += vezmi;
            vstup = &vstup[vezmi..];
        }
    }

    fn vystup(&self) -> Vystup {
        Vystup {
            cv: self.cv,
            blok: slova(&self.blok),
            pocitadlo: self.pocitadlo,
            dlzka: self.dlzka_bloku as u32,
            priznaky: self.priznaky | self.priznak_zaciatku() | KONIEC_KUSKA,
        }
    }
}

fn vystup_rodica(lavy: [u32; 8], pravy: [u32; 8], kluc: [u32; 8], priznaky: u32) -> Vystup {
    let mut blok = [0u32; 16];
    blok[..8].copy_from_slice(&lavy);
    blok[8..].copy_from_slice(&pravy);
    Vystup { cv: kluc, blok, pocitadlo: 0, dlzka: DLZKA_BLOKU as u32, priznaky: RODIC | priznaky }
}

/// Postupný výpočet odtlačku BLAKE3 (dáta možno pridávať po častiach).
#[derive(Clone)]
pub struct Hasovac {
    kusok: StavKuska,
    kluc: [u32; 8],
    zasobnik: [[u32; 8]; 54],
    vyska: usize,
    priznaky: u32,
}

impl fmt::Debug for Hasovac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Hasovac").field("pocitadlo", &self.kusok.pocitadlo).finish_non_exhaustive()
    }
}

impl Default for Hasovac {
    fn default() -> Self {
        Self::novy()
    }
}

impl Hasovac {
    fn s_klucom_a_priznakmi(kluc: [u32; 8], priznaky: u32) -> Self {
        Hasovac {
            kusok: StavKuska::novy(kluc, 0, priznaky),
            kluc,
            zasobnik: [[0; 8]; 54],
            vyska: 0,
            priznaky,
        }
    }

    /// Bežný odtlačok.
    pub fn novy() -> Self {
        Self::s_klucom_a_priznakmi(IV, 0)
    }

    /// Odtlačok s 32-bajtovým kľúčom (overenie pôvodu, overiteľné značky).
    pub fn s_klucom(kluc: &[u8; 32]) -> Self {
        let mut w = [0u32; 8];
        for (i, kus) in kluc.chunks_exact(4).enumerate() {
            w[i] = u32::from_le_bytes([kus[0], kus[1], kus[2], kus[3]]);
        }
        Self::s_klucom_a_priznakmi(w, S_KLUCOM)
    }

    /// Odvodenie kľúča z textového kontextu (napr. „orchidea 2026 denník v1“).
    pub fn odvodenie_kluca(kontext: &str) -> Self {
        let mut h = Self::s_klucom_a_priznakmi(IV, KONTEXT_ODVODENIA);
        h.pridaj(kontext.as_bytes());
        let mut kluc_bajty = [0u8; 32];
        h.dokonci_do(&mut kluc_bajty);
        let mut w = [0u32; 8];
        for (i, kus) in kluc_bajty.chunks_exact(4).enumerate() {
            w[i] = u32::from_le_bytes([kus[0], kus[1], kus[2], kus[3]]);
        }
        Self::s_klucom_a_priznakmi(w, MATERIAL_ODVODENIA)
    }

    fn pridaj_retazovu_hodnotu(&mut self, mut cv: [u32; 8], mut spolu: u64) {
        while spolu & 1 == 0 {
            self.vyska -= 1;
            cv = vystup_rodica(self.zasobnik[self.vyska], cv, self.kluc, self.priznaky).retazova_hodnota();
            spolu >>= 1;
        }
        self.zasobnik[self.vyska] = cv;
        self.vyska += 1;
    }

    /// Pridá ďalšie dáta.
    pub fn pridaj(&mut self, mut vstup: &[u8]) -> &mut Self {
        while !vstup.is_empty() {
            if self.kusok.dlzka() == DLZKA_KUSKA {
                let cv = self.kusok.vystup().retazova_hodnota();
                let spolu = self.kusok.pocitadlo + 1;
                self.pridaj_retazovu_hodnotu(cv, spolu);
                self.kusok = StavKuska::novy(self.kluc, spolu, self.priznaky);
            }
            let vezmi = (DLZKA_KUSKA - self.kusok.dlzka()).min(vstup.len());
            self.kusok.pridaj(&vstup[..vezmi]);
            vstup = &vstup[vezmi..];
        }
        self
    }

    /// Zapíše výstup ľubovoľnej dĺžky (rozšíriteľný výstup).
    pub fn dokonci_do(&self, ciel: &mut [u8]) {
        let mut vystup = self.kusok.vystup();
        let mut zostava = self.vyska;
        while zostava > 0 {
            zostava -= 1;
            vystup =
                vystup_rodica(self.zasobnik[zostava], vystup.retazova_hodnota(), self.kluc, self.priznaky);
        }
        vystup.korenove_bajty(ciel);
    }

    /// 32-bajtový odtlačok.
    pub fn dokonci(&self) -> Odtlacok {
        let mut b = [0u8; 32];
        self.dokonci_do(&mut b);
        Odtlacok(b)
    }
}

/// 32-bajtový odtlačok BLAKE3.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Odtlacok(pub [u8; 32]);

impl Odtlacok {
    /// Odtlačok bajtov naraz.
    pub fn z(data: &[u8]) -> Odtlacok {
        Hasovac::novy().pridaj(data).dokonci()
    }

    /// Zápis v šestnástkovej sústave (64 znakov).
    pub fn sestnastkovo(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            s.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
            s.push(char::from_digit((b & 15) as u32, 16).unwrap_or('0'));
        }
        s
    }

    /// Načíta zápis v šestnástkovej sústave.
    pub fn zo_sestnastkovej(text: &str) -> Option<Odtlacok> {
        let t = text.as_bytes();
        if t.len() != 64 {
            return None;
        }
        let mut b = [0u8; 32];
        for i in 0..32 {
            let h = (t[2 * i] as char).to_digit(16)?;
            let l = (t[2 * i + 1] as char).to_digit(16)?;
            b[i] = (h * 16 + l) as u8;
        }
        Some(Odtlacok(b))
    }
}

impl fmt::Debug for Odtlacok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Odtlacok({})", &self.sestnastkovo()[..16])
    }
}

impl fmt::Display for Odtlacok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.sestnastkovo())
    }
}
