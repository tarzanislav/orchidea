//! **Rozsahový dekodér Opus** (RFC 6716, §4.1) – bitovo presná celočíselná aritmetika.
//!
//! Stav (val, rng) – 32-bitové celé čísla bez znamienka; `val` = rozdiel horného okraja rozsahu a kódovanej hodnoty
//! mínus 1, `rng` = veľkosť rozsahu. Po každom symbole sa rozsah normuje, kým `rng > 2^23` (čítanie po bajtoch,
//! bit sa prenáša do ďalšieho bajtu). „Surové“ bity (len CELT) sa čítajú od konca rámca, od najnižšieho bitu
//! posledného bajtu; smú sa prekrývať s bajtmi, ktoré už prečítal rozsahový dekodér. Za koncom rámca sa číta nula.
//!
//! Odvodenia použité v kóde (z definície `ec_decode` / `ec_dec_update` v §4.1.2):
//! - `ec_dec_icdf`: s r = rng ≫ ftb je symbol k prvý s `val ≥ r · icdf[k]` (lebo fs < ft − icdf[k] ⟺
//!   ⌊val / r⌋ ≥ icdf[k]); potom val −= r · icdf[k], rng = r · (icdf[k−1] − icdf[k]), pri k = 0 rng −= r · icdf[0],
//! - `ec_dec_bit_logp`: s s = rng ≫ logp je dekódovaná „1“ práve vtedy, keď val < s.

/// Počet bitov potrebných na zápis `x` (ilog z RFC 6716 §1.1.10): ilog(0) = 0.
pub fn ilog(x: u32) -> u32 {
    32 - x.leading_zeros()
}

/// Rozsahový dekodér jedného rámca Opus.
#[derive(Debug, Clone)]
pub struct RozsahovyDekoder<'a> {
    data: &'a [u8],
    /// Ďalší bajt na čítanie zľava (rozsahový kód).
    poloha: usize,
    /// Bajtov prečítaných sprava (surové bity).
    od_konca: usize,
    okno: u32,
    bitov_v_okne: u32,
    val: u32,
    rng: u32,
    /// Posledný prečítaný bajt (jeho najnižší bit ide do ďalšieho symbolu normovania).
    zvysok: u32,
    /// Medzivýsledok rng / ft z `dekoduj` pre `aktualizuj`.
    ext: u32,
    nbits_total: i32,
    /// Poškodený rámec (napr. ec_dec_uint mimo rozsahu).
    pub chyba: bool,
}

impl<'a> RozsahovyDekoder<'a> {
    /// Inicializácia podľa §4.1.1: rng = 128, val = 127 − (b0 ≫ 1), potom normovanie.
    pub fn novy(data: &'a [u8]) -> RozsahovyDekoder<'a> {
        let b0 = data.first().copied().unwrap_or(0) as u32;
        let mut d = RozsahovyDekoder {
            data,
            poloha: 1,
            od_konca: 0,
            okno: 0,
            bitov_v_okne: 0,
            val: 127 - (b0 >> 1),
            rng: 128,
            zvysok: b0,
            ext: 0,
            nbits_total: 9,
            chyba: false,
        };
        d.normuj();
        d
    }

    /// Dĺžka rámca v bajtoch.
    pub fn dlzka(&self) -> usize {
        self.data.len()
    }

    fn citaj_bajt(&mut self) -> u32 {
        let b = self.data.get(self.poloha).copied().unwrap_or(0) as u32;
        self.poloha += 1;
        b
    }

    fn normuj(&mut self) {
        while self.rng <= 1 << 23 {
            self.nbits_total += 8;
            self.rng <<= 8;
            let predosly = self.zvysok;
            self.zvysok = self.citaj_bajt();
            let sym = ((predosly << 8) | self.zvysok) >> 1 & 0xFF;
            self.val = (self.val << 8).wrapping_add(255 - sym) & 0x7FFF_FFFF;
        }
    }

    /// `ec_decode(ft)`: hodnota fs v [0, ft), podľa ktorej volajúci určí symbol.
    pub fn dekoduj(&mut self, ft: u32) -> u32 {
        self.ext = self.rng / ft.max(1);
        let s = self.val / self.ext.max(1);
        ft - (s + 1).min(ft)
    }

    /// `ec_decode_bin(ftb)` = `ec_decode(1 << ftb)`.
    pub fn dekoduj_bin(&mut self, ftb: u32) -> u32 {
        self.ext = self.rng >> ftb;
        let s = self.val / self.ext.max(1);
        (1u32 << ftb) - (s + 1).min(1 << ftb)
    }

    /// `ec_dec_update(fl, fh, ft)`.
    pub fn aktualizuj(&mut self, fl: u32, fh: u32, ft: u32) {
        let s = self.ext.wrapping_mul(ft - fh);
        self.val = self.val.wrapping_sub(s);
        self.rng = if fl > 0 { self.ext.wrapping_mul(fh - fl) } else { self.rng.wrapping_sub(s) };
        self.normuj();
    }

    /// Binárny symbol s pravdepodobnosťou „1“ = 2^−logp.
    pub fn bit_logp(&mut self, logp: u32) -> bool {
        let s = self.rng >> logp;
        let jedna = self.val < s;
        if jedna {
            self.rng = s;
        } else {
            self.val -= s;
            self.rng -= s;
        }
        self.normuj();
        jedna
    }

    /// Symbol z tabuľky icdf (končí nulou), ft = 2^ftb.
    pub fn icdf(&mut self, icdf: &[u8], ftb: u32) -> usize {
        let r = self.rng >> ftb;
        let mut horny = self.rng;
        let mut k = 0;
        loop {
            let s = r * icdf.get(k).copied().unwrap_or(0) as u32;
            if self.val >= s || k + 1 >= icdf.len() {
                self.val -= s.min(self.val);
                self.rng = horny - s;
                break;
            }
            horny = s;
            k += 1;
        }
        self.normuj();
        k
    }

    /// Symbol z tabuľky frekvencií PDF (ako v texte RFC, napr. {4, 4, 4, 4}/16): súčet = ft.
    pub fn pdf(&mut self, pdf: &[u16]) -> usize {
        let ft: u32 = pdf.iter().map(|&x| x as u32).sum();
        let fs = self.dekoduj(ft);
        let mut fl = 0u32;
        for (k, &f) in pdf.iter().enumerate() {
            let fh = fl + f as u32;
            if fs < fh || k + 1 == pdf.len() {
                self.aktualizuj(fl, fh, ft);
                return k;
            }
            fl = fh;
        }
        0
    }

    /// `ec_dec_bits(n)`: n surových bitov od konca rámca (n ≤ 25).
    pub fn bity(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        while self.bitov_v_okne < n {
            let b = if self.od_konca < self.data.len() {
                self.data[self.data.len() - 1 - self.od_konca]
            } else {
                0
            };
            self.od_konca += 1;
            self.okno |= (b as u32) << self.bitov_v_okne;
            self.bitov_v_okne += 8;
        }
        let v = self.okno & ((1u64 << n) - 1) as u32;
        self.okno = if n >= 32 { 0 } else { self.okno >> n };
        self.bitov_v_okne -= n;
        self.nbits_total += n as i32;
        v
    }

    /// `ec_dec_uint(ft)`: rovnomerné celé číslo v [0, ft) (ft ≥ 1); pri hodnote mimo rozsahu nastaví `chyba`.
    pub fn uint(&mut self, ft: u32) -> u32 {
        let ft1 = ft.max(1) - 1;
        let ftb = ilog(ft1);
        if ftb > 8 {
            let nizsich = ftb - 8;
            let t_ft = (ft1 >> nizsich) + 1;
            let s = self.dekoduj(t_ft);
            self.aktualizuj(s, s + 1, t_ft);
            let t = (s << nizsich) | self.bity(nizsich);
            if t > ft1 {
                self.chyba = true;
                return ft1;
            }
            t
        } else {
            let ft = ft1 + 1;
            let s = self.dekoduj(ft);
            self.aktualizuj(s, s + 1, ft);
            s
        }
    }

    /// `ec_tell()`: horný odhad celých bitov použitých z rámca.
    pub fn tell(&self) -> i32 {
        self.nbits_total - ilog(self.rng) as i32
    }

    /// `ec_tell_frac()`: horný odhad v osminách bitu.
    pub fn tell_frac(&self) -> i32 {
        let mut lg = ilog(self.rng);
        let mut r = self.rng >> (lg - 16);
        for _ in 0..3 {
            r = (r * r) >> 15;
            let b = r >> 16;
            lg = 2 * lg + b;
            if b != 0 {
                r >>= 1;
            }
        }
        self.nbits_total * 8 - lg as i32
    }

    /// Stav `rng` (konečný stav rámca = `OPUS_GET_FINAL_RANGE` referenčného dekodéra).
    pub fn rng(&self) -> u32 {
        self.rng
    }

    /// Zostávajúce celé bity rámca (dĺžka · 8 − tell).
    pub fn zostava_bitov(&self) -> i32 {
        self.data.len() as i32 * 8 - self.tell()
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    /// Pomocný kodér podľa RFC 6716 §5.1 (len na skúšky dekodéra – kodér nie je normatívny).
    struct Koder {
        low: u32,
        rng: u32,
        rem: i32,
        ext: u32,
        von: Vec<u8>,
        koniec: Vec<u8>,
        okno: u32,
        bitov: u32,
    }

    impl Koder {
        fn novy() -> Koder {
            Koder {
                low: 0,
                rng: 1 << 31,
                rem: -1,
                ext: 0,
                von: Vec::new(),
                koniec: Vec::new(),
                okno: 0,
                bitov: 0,
            }
        }
        fn carry_out(&mut self, c: u32) {
            if c != 0xFF {
                let carry = c >> 8;
                if self.rem >= 0 {
                    self.von.push((self.rem as u32 + carry) as u8);
                }
                for _ in 0..self.ext {
                    self.von.push((0xFF + carry) as u8);
                }
                self.ext = 0;
                self.rem = (c & 0xFF) as i32;
            } else {
                self.ext += 1;
            }
        }
        fn normuj(&mut self) {
            while self.rng <= 1 << 23 {
                self.carry_out(self.low >> 23);
                self.low = (self.low << 8) & 0x7FFF_FFFF;
                self.rng <<= 8;
            }
        }
        fn koduj(&mut self, fl: u32, fh: u32, ft: u32) {
            let r = self.rng / ft;
            if fl > 0 {
                self.low += self.rng - r * (ft - fl);
                self.rng = r * (fh - fl);
            } else {
                self.rng -= r * (ft - fh);
            }
            self.normuj();
        }
        fn bity(&mut self, v: u32, n: u32) {
            self.okno |= v << self.bitov;
            self.bitov += n;
            while self.bitov >= 8 {
                self.koniec.push(self.okno as u8);
                self.okno >>= 8;
                self.bitov -= 8;
            }
        }
        fn hotovo(mut self, dlzka: usize) -> Vec<u8> {
            // ukončenie: najmenej bitov tak, aby dekodér dekódoval správne bez ohľadu na ďalšie bajty
            let mut l = 32 - ilog(self.rng);
            let mut maska = 0x7FFF_FFFFu32 >> l;
            let mut koniec = (self.low + maska) & !maska;
            if (koniec | maska) >= self.low + self.rng {
                l += 1;
                maska >>= 1;
                koniec = (self.low + maska) & !maska;
            }
            while l > 0 {
                self.carry_out(koniec >> 23);
                koniec = (koniec << 8) & 0x7FFF_FFFF;
                l = l.saturating_sub(8);
            }
            if self.rem >= 0 || self.ext > 0 {
                self.carry_out(0);
            }
            if self.bitov > 0 {
                self.koniec.push(self.okno as u8);
            }
            let mut d = self.von;
            assert!(d.len() + self.koniec.len() <= dlzka, "rámec príliš malý");
            d.resize(dlzka - self.koniec.len(), 0);
            d.extend(self.koniec.iter().rev());
            d
        }
    }

    #[test]
    fn kodovanie_a_dekodovanie_symbolov() {
        // náhodná postupnosť symbolov s rôznymi kontextmi, surovými bitmi a uint; dekodér ju musí vrátiť
        let mut x = 0x1234_5678u32;
        let mut nahoda = || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        let icdf: [u8; 4] = [200, 100, 30, 0];
        for pokus in 0..200 {
            let mut k = Koder::novy();
            let mut ocak = Vec::new();
            for _ in 0..(20 + pokus % 50) {
                match nahoda() % 4 {
                    0 => {
                        let ft = 2 + nahoda() % 1000;
                        let s = nahoda() % ft;
                        k.koduj(s, s + 1, ft);
                        ocak.push((0, ft, s));
                    }
                    1 => {
                        let s = nahoda() % 4;
                        let fl = if s == 0 { 0 } else { 256 - icdf[s as usize - 1] as u32 };
                        k.koduj(fl, 256 - icdf[s as usize] as u32, 256);
                        ocak.push((1, 0, s));
                    }
                    2 => {
                        let logp = 1 + nahoda() % 15;
                        let b = (nahoda() % 4 == 0) as u32;
                        let ft = 1u32 << logp;
                        if b == 1 {
                            k.koduj(ft - 1, ft, ft)
                        } else {
                            k.koduj(0, ft - 1, ft)
                        }
                        ocak.push((2, logp, b));
                    }
                    _ => {
                        let n = 1 + nahoda() % 16;
                        let v = nahoda() & ((1 << n) - 1);
                        k.bity(v, n);
                        ocak.push((3, n, v));
                    }
                }
            }
            let data = k.hotovo(400);
            let mut d = RozsahovyDekoder::novy(&data);
            for &(druh, p, v) in &ocak {
                let r = match druh {
                    0 => {
                        let s = d.dekoduj(p);
                        d.aktualizuj(s, s + 1, p);
                        s
                    }
                    1 => d.icdf(&icdf, 8) as u32,
                    2 => d.bit_logp(p) as u32,
                    _ => d.bity(p),
                };
                assert_eq!(r, v, "pokus {pokus}");
            }
            assert!(d.tell() <= 400 * 8 && d.tell_frac() <= 400 * 64);
        }
    }

    #[test]
    fn tell_po_inicializacii() {
        let d = RozsahovyDekoder::novy(&[0x80, 0, 0, 0]);
        assert_eq!(d.tell(), 1);
        assert_eq!(d.tell_frac(), 8);
        // prázdny rámec: číta nuly, nikdy pád
        let mut d = RozsahovyDekoder::novy(&[]);
        let _ = (d.uint(1000), d.icdf(&[128, 0], 8), d.bity(25), d.bit_logp(15));
    }

    #[test]
    fn uint_velke_rozsahy() {
        for &ft in &[2u32, 255, 256, 257, 1000, 65_535, 1 << 20, 0x7FFF_FFFF, u32::MAX] {
            let mut k = Koder::novy();
            let vals: Vec<u32> = [0, ft / 3, ft - 1].to_vec();
            for &v in &vals {
                let ft1 = ft - 1;
                let ftb = ilog(ft1);
                if ftb > 8 {
                    let n = ftb - 8;
                    let t_ft = (ft1 >> n) + 1;
                    k.koduj(v >> n, (v >> n) + 1, t_ft);
                    k.bity(v & ((1 << n) - 1), n);
                } else {
                    k.koduj(v, v + 1, ft);
                }
            }
            let data = k.hotovo(64);
            let mut d = RozsahovyDekoder::novy(&data);
            for &v in &vals {
                assert_eq!(d.uint(ft), v, "ft {ft}");
            }
            assert!(!d.chyba);
        }
    }
}
