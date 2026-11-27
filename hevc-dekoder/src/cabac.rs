//! Aritmetické dekódovanie CABAC – H.265, kap. 9.3: dekodér (9.3.4.3) a kontextové premenné s inicializáciou
//! (9.3.2.2, tabuľky 9-5 … 9-37).
//!
//! Dekodér presne sleduje model normy (ivlCurrRange 9 bitov, ivlOffset sa dopĺňa po bitoch), len bity číta
//! z 64-bitového okna. Vďaka tomu po bine „terminate“ s hodnotou 1 vie presne určiť bajt, kde začína ďalší
//! podprúd (dlaždica, riadok WPP) alebo vzorky PCM.

/// Tabuľka rangeTabLps[pStateIdx][qRangeIdx] (9.3.4.3.2, tab. 9-46).
const ROZSAH_LPS: [[u8; 4]; 64] = [
    [128, 176, 208, 240],
    [128, 167, 197, 227],
    [128, 158, 187, 216],
    [123, 150, 178, 205],
    [116, 142, 169, 195],
    [111, 135, 160, 185],
    [105, 128, 152, 175],
    [100, 122, 144, 166],
    [95, 116, 137, 158],
    [90, 110, 130, 150],
    [85, 104, 123, 142],
    [81, 99, 117, 135],
    [77, 94, 111, 128],
    [73, 89, 105, 122],
    [69, 85, 100, 116],
    [66, 80, 95, 110],
    [62, 76, 90, 104],
    [59, 72, 86, 99],
    [56, 69, 81, 94],
    [53, 65, 77, 89],
    [51, 62, 73, 85],
    [48, 59, 69, 80],
    [46, 56, 66, 76],
    [43, 53, 63, 72],
    [41, 50, 59, 69],
    [39, 48, 56, 65],
    [37, 45, 54, 62],
    [35, 43, 51, 59],
    [33, 41, 48, 56],
    [32, 39, 46, 53],
    [30, 37, 43, 50],
    [29, 35, 41, 48],
    [27, 33, 39, 45],
    [26, 31, 37, 43],
    [24, 30, 35, 41],
    [23, 28, 33, 39],
    [22, 27, 32, 37],
    [21, 26, 30, 35],
    [20, 24, 29, 33],
    [19, 23, 27, 31],
    [18, 22, 26, 30],
    [17, 21, 25, 28],
    [16, 20, 23, 27],
    [15, 19, 22, 25],
    [14, 18, 21, 24],
    [14, 17, 20, 23],
    [13, 16, 19, 22],
    [12, 15, 18, 21],
    [12, 14, 17, 20],
    [11, 14, 16, 19],
    [11, 13, 15, 18],
    [10, 12, 15, 17],
    [10, 12, 14, 16],
    [9, 11, 13, 15],
    [9, 11, 12, 14],
    [8, 10, 12, 14],
    [8, 9, 11, 13],
    [7, 9, 11, 12],
    [7, 9, 10, 12],
    [7, 8, 10, 11],
    [6, 8, 9, 11],
    [6, 7, 9, 10],
    [6, 7, 8, 9],
    [2, 2, 2, 2],
];

/// transIdxLps (tab. 9-47); transIdxMps = Min(pStateIdx + 1, 62).
const PRECHOD_LPS: [u8; 64] = [
    0, 0, 1, 2, 2, 4, 4, 5, 6, 7, 8, 9, 9, 11, 11, 12, 13, 13, 15, 15, 16, 16, 18, 18, 19, 19, 21, 21, 22,
    22, 23, 24, 24, 25, 26, 26, 27, 27, 28, 29, 29, 30, 30, 30, 31, 32, 32, 33, 33, 33, 34, 34, 35, 35, 35,
    36, 36, 36, 37, 37, 37, 38, 38, 63,
];

/// Prechod stavu po bine MPS a LPS pre stav (pStateIdx << 1) | valMps – vopred spočítané.
const fn prechody() -> ([u8; 128], [u8; 128]) {
    let mut mps = [0u8; 128];
    let mut lps = [0u8; 128];
    let mut s = 0;
    while s < 128 {
        let p = s >> 1;
        let m = s & 1;
        let p_mps = if p < 62 { p + 1 } else { p };
        mps[s] = ((p_mps << 1) | m) as u8;
        let m_lps = if p == 0 { 1 - m } else { m };
        lps[s] = ((PRECHOD_LPS[p] as usize) << 1 | m_lps) as u8;
        s += 1;
    }
    (mps, lps)
}
const PRECHODY: ([u8; 128], [u8; 128]) = prechody();

/// Aritmetický dekodér nad RBSP dát segmentu rezu.
#[derive(Debug, Clone)]
pub struct Cabac<'a> {
    data: &'a [u8],
    /// ďalší bajt na načítanie do okna
    poz: usize,
    /// neprečítané bity zarovnané k najvyššiemu bitu
    okno: u64,
    bitov: u32,
    /// ivlCurrRange
    rozsah: u32,
    /// ivlOffset
    hodnota: u32,
}

impl<'a> Cabac<'a> {
    /// Inicializácia dekodéra na bajte `zaciatok` (9.3.2.5): ivlCurrRange = 510, ivlOffset = read_bits(9).
    pub fn novy(data: &'a [u8], zaciatok: usize) -> Cabac<'a> {
        let mut c = Cabac { data, poz: zaciatok, okno: 0, bitov: 0, rozsah: 510, hodnota: 0 };
        c.hodnota = c.bity(9);
        c
    }

    #[inline]
    fn dopln(&mut self) {
        while self.bitov <= 56 {
            // za koncom dát nuly (čítanie dopredu; poškodený tok odhalí `poskodeny`)
            let b = self.data.get(self.poz).copied().unwrap_or(0);
            self.poz += 1;
            self.okno |= (b as u64) << (56 - self.bitov);
            self.bitov += 8;
        }
    }

    /// `n` bitov (n ≤ 32) z prúdu.
    #[inline]
    fn bity(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if self.bitov < n {
            self.dopln();
        }
        let v = (self.okno >> (64 - n)) as u32;
        self.okno <<= n;
        self.bitov -= n;
        v
    }

    /// Bity spotrebované dekodérom od začiatku dát.
    fn spotrebovane(&self) -> usize {
        self.poz * 8 - self.bitov as usize
    }

    /// Stav na ladenie: dĺžka dát a spotrebované bity.
    pub fn ladenie(&self) -> String {
        format!("dĺžka {} B, spotrebovaných {} bitov", self.data.len(), self.spotrebovane())
    }

    /// Je prúd poškodený? Dekodér spotreboval viac bitov, ako dáta majú (v platnom toku nikdy – posledný
    /// spotrebovaný bit je záverečný bit 1).
    pub fn poskodeny(&self) -> bool {
        self.spotrebovane() > self.data.len() * 8
    }

    /// DecodeDecision (9.3.4.3.2) s kontextovou premennou `stav` = (pStateIdx << 1) | valMps.
    #[inline]
    pub fn bin(&mut self, stav: &mut u8) -> u32 {
        let s = *stav as usize;
        let lps = ROZSAH_LPS[s >> 1][((self.rozsah >> 6) & 3) as usize] as u32;
        self.rozsah -= lps;
        let bin;
        if self.hodnota >= self.rozsah {
            bin = (s & 1) as u32 ^ 1;
            self.hodnota -= self.rozsah;
            self.rozsah = lps;
            *stav = PRECHODY.1[s];
        } else {
            bin = (s & 1) as u32;
            *stav = PRECHODY.0[s];
        }
        if self.rozsah < 256 {
            let n = self.rozsah.leading_zeros() - 23;
            self.rozsah <<= n;
            self.hodnota = (self.hodnota << n) | self.bity(n);
        }
        bin
    }

    /// DecodeBypass (9.3.4.3.4) – bez vetvenia (biny obchádzania sú náhodné, vetva by sa často mýlila).
    #[inline]
    pub fn obchadzanie(&mut self) -> u32 {
        let v = (self.hodnota << 1) | self.bity(1);
        let bin = (v >= self.rozsah) as u32;
        self.hodnota = v - (self.rozsah & bin.wrapping_neg());
        bin
    }

    /// `n` binov obchádzania (n ≤ 32) ako číslo s pevnou dĺžkou (najvyšší bit prvý) – naraz jedným delením.
    ///
    /// n krokov 9.3.4.3.4 (`h = 2h + b`, pri `h ≥ R` bin 1 a `h −= R`) je presne dlhé delenie čísla
    /// `h·2ⁿ + B` (B = ďalších n bitov) rozsahom R: podiel sú biny, zvyšok nové ivlOffset. Keďže h < R, každá
    /// číslica podielu je 0 alebo 1. V poškodenom toku (h ≥ R) dá postupný výpočet samé jednotky a zvyšok
    /// `v − (2ⁿ − 1)·R` – rovnako ako orezaný podiel nižšie, výsledok je teda totožný v každom prípade.
    #[inline]
    pub fn obchadzanie_n(&mut self, n: u32) -> u32 {
        if n <= 1 {
            return if n == 1 { self.obchadzanie() } else { 0 };
        }
        let v = ((self.hodnota as u64) << n) | self.bity(n) as u64;
        let r = self.rozsah as u64;
        let q = (v / r).min((1u64 << n) - 1);
        self.hodnota = (v - q * r) as u32;
        q as u32
    }

    /// DecodeTerminate (9.3.4.3.5).
    pub fn ukoncenie(&mut self) -> u32 {
        self.rozsah -= 2;
        if self.hodnota >= self.rozsah {
            1
        } else {
            if self.rozsah < 256 {
                self.rozsah <<= 1;
                self.hodnota = (self.hodnota << 1) | self.bity(1);
            }
            0
        }
    }

    /// Bajt, kde po bine „terminate“ s hodnotou 1 začínajú ďalšie dáta. Posledný bit, ktorý kodér zapíše pri
    /// ukončení (9.3.5.? EncodeFlush), je 1 a dekodér ho už má v 9-bitovom okne ivlOffset – je to
    /// rbsp_stop_one_bit, alignment_bit_equal_to_one, resp. bit pred pcm_alignment_zero_bit. Za ním sú už len
    /// nuly po hranicu bajtu, takže ďalšie dáta začínajú na najbližšej hranici bajtu.
    pub fn bajt_po_ukonceni(&self) -> usize {
        self.spotrebovane().div_ceil(8)
    }

    /// Znova inicializuje dekodér na bajte `zaciatok` (nový podprúd, koniec vzoriek PCM).
    pub fn znova(&mut self, zaciatok: usize) {
        self.poz = zaciatok;
        self.okno = 0;
        self.bitov = 0;
        self.rozsah = 510;
        self.hodnota = self.bity(9);
    }

    /// Prístup k dátam (vzorky PCM sa čítajú priamo z RBSP).
    pub fn data(&self) -> &'a [u8] {
        self.data
    }
}

/// Indexy kontextových premenných (posun prvej premennej každého prvku syntaxe).
pub mod k {
    pub const SAO_MERGE: usize = 0;
    pub const SAO_TYPE: usize = 1;
    pub const SPLIT_CU: usize = 2;
    pub const TRANSQUANT_BYPASS: usize = 5;
    pub const CU_SKIP: usize = 6;
    pub const PRED_MODE: usize = 9;
    pub const PART_MODE: usize = 10;
    pub const PREV_INTRA_LUMA: usize = 14;
    pub const INTRA_CHROMA: usize = 15;
    pub const RQT_ROOT_CBF: usize = 16;
    pub const MERGE_FLAG: usize = 17;
    pub const MERGE_IDX: usize = 18;
    pub const INTER_PRED_IDC: usize = 19;
    pub const REF_IDX: usize = 24;
    pub const MVP_FLAG: usize = 26;
    pub const SPLIT_TRANSFORM: usize = 27;
    pub const CBF_LUMA: usize = 30;
    pub const CBF_CHROMA: usize = 32;
    pub const MVD_GT0: usize = 37;
    pub const MVD_GT1: usize = 38;
    pub const CU_QP_DELTA: usize = 39;
    pub const TRANSFORM_SKIP: usize = 41;
    pub const LAST_X: usize = 43;
    pub const LAST_Y: usize = 61;
    pub const CODED_SUB_BLOCK: usize = 79;
    pub const SIG_COEFF: usize = 83;
    pub const GT1: usize = 127;
    pub const GT2: usize = 151;
    pub const POCET: usize = 157;
}

/// Hodnota „nepoužité“ pre prvky, ktoré sa v danom type rezu nevyskytujú.
const NA: u8 = 154;

/// initValue všetkých kontextových premenných pre initType 0, 1, 2 v poradí modulu [`k`]
/// (tab. 9-5 … 9-37 normy).
const POCIATOCNE: [[u8; k::POCET]; 3] = {
    let mut t = [[NA; k::POCET]; 3];
    let mut it = 0;
    while it < 3 {
        let r = &mut t[it];
        r[k::SAO_MERGE] = 153;
        r[k::SAO_TYPE] = [200, 185, 160][it];
        let split = [[139, 141, 157], [107, 139, 126], [107, 139, 126]][it];
        let mut i = 0;
        while i < 3 {
            r[k::SPLIT_CU + i] = split[i];
            i += 1;
        }
        r[k::TRANSQUANT_BYPASS] = 154;
        let skip = [[NA, NA, NA], [197, 185, 201], [197, 185, 201]][it];
        let mut i = 0;
        while i < 3 {
            r[k::CU_SKIP + i] = skip[i];
            i += 1;
        }
        r[k::PRED_MODE] = [NA, 149, 134][it];
        let part = [[184, NA, NA, NA], [154, 139, 154, 154], [154, 139, 154, 154]][it];
        let mut i = 0;
        while i < 4 {
            r[k::PART_MODE + i] = part[i];
            i += 1;
        }
        r[k::PREV_INTRA_LUMA] = [184, 154, 183][it];
        r[k::INTRA_CHROMA] = [63, 152, 152][it];
        r[k::RQT_ROOT_CBF] = [NA, 79, 79][it];
        r[k::MERGE_FLAG] = [NA, 110, 154][it];
        r[k::MERGE_IDX] = [NA, 122, 137][it];
        let ipi = [95, 79, 63, 31, 31];
        let mut i = 0;
        while i < 5 {
            r[k::INTER_PRED_IDC + i] = if it == 0 { NA } else { ipi[i] };
            i += 1;
        }
        r[k::REF_IDX] = if it == 0 { NA } else { 153 };
        r[k::REF_IDX + 1] = if it == 0 { NA } else { 153 };
        r[k::MVP_FLAG] = [NA, 168, 168][it];
        let st = [[153, 138, 138], [124, 138, 94], [224, 167, 122]][it];
        let mut i = 0;
        while i < 3 {
            r[k::SPLIT_TRANSFORM + i] = st[i];
            i += 1;
        }
        let cl = [[111, 141], [153, 111], [153, 111]][it];
        r[k::CBF_LUMA] = cl[0];
        r[k::CBF_LUMA + 1] = cl[1];
        let cc = [[94, 138, 182, 154, 154], [149, 107, 167, 154, 154], [149, 92, 167, 154, 154]][it];
        let mut i = 0;
        while i < 5 {
            r[k::CBF_CHROMA + i] = cc[i];
            i += 1;
        }
        r[k::MVD_GT0] = [NA, 140, 169][it];
        r[k::MVD_GT1] = [NA, 198, 198][it];
        r[k::CU_QP_DELTA] = 154;
        r[k::CU_QP_DELTA + 1] = 154;
        r[k::TRANSFORM_SKIP] = 139;
        r[k::TRANSFORM_SKIP + 1] = 139;
        let last = [
            [110, 110, 124, 125, 140, 153, 125, 127, 140, 109, 111, 143, 127, 111, 79, 108, 123, 63],
            [125, 110, 94, 110, 95, 79, 125, 111, 110, 78, 110, 111, 111, 95, 94, 108, 123, 108],
            [125, 110, 124, 110, 95, 94, 125, 111, 111, 79, 125, 126, 111, 111, 79, 108, 123, 93],
        ][it];
        let mut i = 0;
        while i < 18 {
            r[k::LAST_X + i] = last[i];
            r[k::LAST_Y + i] = last[i];
            i += 1;
        }
        let csb = [[91, 171, 134, 141], [121, 140, 61, 154], [121, 140, 61, 154]][it];
        let mut i = 0;
        while i < 4 {
            r[k::CODED_SUB_BLOCK + i] = csb[i];
            i += 1;
        }
        let sig = [
            [
                111, 111, 125, 110, 110, 94, 124, 108, 124, 107, 125, 141, 179, 153, 125, 107, 125, 141, 179,
                153, 125, 107, 125, 141, 179, 153, 125, 140, 139, 182, 182, 152, 136, 152, 136, 153, 136,
                139, 111, 136, 139, 111, 141, 111,
            ],
            [
                155, 154, 139, 153, 139, 123, 123, 63, 153, 166, 183, 140, 136, 153, 154, 166, 183, 140, 136,
                153, 154, 166, 183, 140, 136, 153, 154, 170, 153, 123, 123, 107, 121, 107, 121, 167, 151,
                183, 140, 151, 183, 140, 140, 140,
            ],
            [
                170, 154, 139, 153, 139, 123, 123, 63, 124, 166, 183, 140, 136, 153, 154, 166, 183, 140, 136,
                153, 154, 166, 183, 140, 136, 153, 154, 170, 153, 138, 138, 122, 121, 122, 121, 167, 151,
                183, 140, 151, 183, 140, 140, 140,
            ],
        ][it];
        let mut i = 0;
        while i < 44 {
            r[k::SIG_COEFF + i] = sig[i];
            i += 1;
        }
        let g1 = [
            [
                140, 92, 137, 138, 140, 152, 138, 139, 153, 74, 149, 92, 139, 107, 122, 152, 140, 179, 166,
                182, 140, 227, 122, 197,
            ],
            [
                154, 196, 196, 167, 154, 152, 167, 182, 182, 134, 149, 136, 153, 121, 136, 137, 169, 194,
                166, 167, 154, 167, 137, 182,
            ],
            [
                154, 196, 167, 167, 154, 152, 167, 182, 182, 134, 149, 136, 153, 121, 136, 122, 169, 208,
                166, 167, 154, 152, 167, 182,
            ],
        ][it];
        let mut i = 0;
        while i < 24 {
            r[k::GT1 + i] = g1[i];
            i += 1;
        }
        let g2 =
            [[138, 153, 136, 167, 152, 152], [107, 167, 91, 122, 107, 167], [107, 167, 91, 107, 107, 167]]
                [it];
        let mut i = 0;
        while i < 6 {
            r[k::GT2 + i] = g2[i];
            i += 1;
        }
        it += 1;
    }
    t
};

/// Kontextové premenné (stav (pStateIdx << 1) | valMps pre každú).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kontexty(pub [u8; k::POCET]);

impl Kontexty {
    /// Inicializácia (9.3.2.2) pre initType a SliceQpY.
    pub fn nove(init_typ: usize, qp: i32) -> Kontexty {
        let qp = qp.clamp(0, 51);
        let mut s = [0u8; k::POCET];
        for (stav, &init) in s.iter_mut().zip(POCIATOCNE[init_typ].iter()) {
            let sklon = (init >> 4) as i32 * 5 - 45;
            let posun = (((init & 15) as i32) << 3) - 16;
            let pre = (((sklon * qp) >> 4) + posun).clamp(1, 126);
            *stav = if pre <= 63 { ((63 - pre) << 1) as u8 } else { (((pre - 64) << 1) | 1) as u8 };
        }
        Kontexty(s)
    }
}

#[cfg(test)]
mod testy {
    use super::*;

    /// Kodér CABAC podľa 9.3.5 (len pre skúšky) – dekodér musí vrátiť presne zakódované biny.
    struct Koder {
        nizka: u32,
        rozsah: u32,
        prve: bool,
        cakajuce: u32,
        bity: Vec<bool>,
    }

    impl Koder {
        fn novy() -> Koder {
            Koder { nizka: 0, rozsah: 510, prve: true, cakajuce: 0, bity: Vec::new() }
        }
        fn daj_bit(&mut self, b: bool) {
            if self.prve {
                self.prve = false;
            } else {
                self.bity.push(b);
            }
            while self.cakajuce > 0 {
                self.bity.push(!b);
                self.cakajuce -= 1;
            }
        }
        fn renorm(&mut self) {
            while self.rozsah < 256 {
                if self.nizka < 256 {
                    self.daj_bit(false);
                } else if self.nizka >= 512 {
                    self.nizka -= 512;
                    self.daj_bit(true);
                } else {
                    self.nizka -= 256;
                    self.cakajuce += 1;
                }
                self.rozsah <<= 1;
                self.nizka <<= 1;
            }
        }
        fn bin(&mut self, stav: &mut u8, bin: u32) {
            let s = *stav as usize;
            let lps = ROZSAH_LPS[s >> 1][((self.rozsah >> 6) & 3) as usize] as u32;
            self.rozsah -= lps;
            if bin != (s & 1) as u32 {
                self.nizka += self.rozsah;
                self.rozsah = lps;
                *stav = PRECHODY.1[s];
            } else {
                *stav = PRECHODY.0[s];
            }
            self.renorm();
        }
        fn obchadzanie(&mut self, bin: u32) {
            self.nizka <<= 1;
            if bin != 0 {
                self.nizka += self.rozsah;
            }
            if self.nizka >= 1024 {
                self.daj_bit(true);
                self.nizka -= 1024;
            } else if self.nizka < 512 {
                self.daj_bit(false);
            } else {
                self.nizka -= 512;
                self.cakajuce += 1;
            }
        }
        fn ukoncenie(&mut self, bin: u32) {
            self.rozsah -= 2;
            if bin != 0 {
                self.nizka += self.rozsah;
                self.rozsah = 2;
                self.renorm();
                self.daj_bit((self.nizka >> 9) & 1 == 1);
                let posledne = ((self.nizka >> 7) & 3) | 1;
                self.bity.push(posledne & 2 != 0);
                self.bity.push(true);
            } else {
                self.renorm();
            }
        }
        fn bajty(&self) -> Vec<u8> {
            let mut v = vec![0u8; self.bity.len().div_ceil(8)];
            for (i, &b) in self.bity.iter().enumerate() {
                if b {
                    v[i / 8] |= 0x80 >> (i % 8);
                }
            }
            v
        }
    }

    /// n binov obchádzania naraz (delenie) = n binov po jednom – aj v poškodenom stave (ivlOffset ≥ ivlCurrRange)
    /// a pri rôznom počte bitov v okne.
    #[test]
    fn obchadzanie_naraz_ako_po_jednom() {
        let mut n = orchidea_jadro::nahoda::Nahoda::zo_semienka(2711);
        for _ in 0..2000 {
            let data: Vec<u8> = (0..64).map(|_| n.dalsie() as u8).collect();
            let mut a = Cabac::novy(&data, 0);
            let mut ks = Kontexty::nove(1, 30);
            // náhodný stav: niekoľko rozhodnutí a bitov
            for _ in 0..(n.dalsie() % 12) {
                a.bin(&mut ks.0[(n.dalsie() % 8) as usize]);
            }
            if n.dalsie().is_multiple_of(8) {
                a.hodnota = a.rozsah + (n.dalsie() % 300) as u32; // poškodený tok
            }
            let mut b = a.clone();
            for _ in 0..6 {
                let k = (n.dalsie() % 33) as u32;
                let mut po_jednom = 0u32;
                for _ in 0..k {
                    po_jednom = (po_jednom << 1) | b.obchadzanie();
                }
                assert_eq!(a.obchadzanie_n(k), po_jednom, "n = {k}");
                assert_eq!((a.hodnota, a.rozsah, a.spotrebovane()), (b.hodnota, b.rozsah, b.spotrebovane()));
            }
        }
    }

    /// Náhodné biny zakódované kodérom podľa normy musí dekodér vrátiť presne a po ukončení nájsť nasledujúci
    /// bajt – pre všetky zvyšky dĺžky po delení 8 (koniec podprúdu na hranici bajtu aj uprostred).
    #[test]
    fn kodovanie_a_dekodovanie_binov() {
        let mut zvysky = [false; 8];
        for semienko in 0..64u64 {
            let mut n = orchidea_jadro::nahoda::Nahoda::zo_semienka(93 + semienko);
            let mut biny = Vec::new();
            let mut kod = Koder::novy();
            let mut ks = Kontexty::nove(1, 30);
            let pocet = 200 + (n.dalsie() % 3000) as usize;
            for _ in 0..pocet {
                let druh = n.dalsie() % 4;
                let ctx = (n.dalsie() % 8) as usize;
                // skreslené biny, aby sa kontexty učili
                let bin = if n.dalsie() % 10 < 8 { (ctx % 2) as u32 } else { (n.dalsie() % 2) as u32 };
                match druh {
                    0 | 1 => kod.bin(&mut ks.0[ctx], bin),
                    2 => kod.obchadzanie(bin),
                    _ => kod.ukoncenie(0),
                }
                biny.push((druh, ctx, bin));
            }
            kod.ukoncenie(1);
            let mut data = kod.bajty();
            zvysky[kod.bity.len() % 8] = true;
            let koniec = data.len();
            data.extend_from_slice(&[0xAB, 0xCD, 0, 0, 0, 0, 0, 0, 0, 0]);
            let mut d = Cabac::novy(&data, 0);
            let mut ks = Kontexty::nove(1, 30);
            for (i, &(druh, ctx, bin)) in biny.iter().enumerate() {
                let b = match druh {
                    0 | 1 => d.bin(&mut ks.0[ctx]),
                    2 => d.obchadzanie(),
                    _ => d.ukoncenie(),
                };
                let ocakavany = if druh == 3 { 0 } else { bin };
                assert_eq!(b, ocakavany, "semienko {semienko}, bin {i}");
            }
            assert_eq!(d.ukoncenie(), 1);
            assert!(!d.poskodeny(), "platné dáta nesmú vyzerať ako poškodené");
            assert_eq!(d.bajt_po_ukonceni(), koniec, "semienko {semienko}, {} bitov", kod.bity.len());
            assert_eq!(data[d.bajt_po_ukonceni()], 0xAB);
            // nový podprúd za ukončením
            d.znova(d.bajt_po_ukonceni());
        }
        assert!(zvysky.iter().all(|&z| z), "pokryté zvyšky dĺžky po delení 8: {zvysky:?}");
    }

    /// Krátke podprúdy (dlaždice, riadky WPP, koniec vzoriek PCM) tesne pred koncom dát rezu: okno sa pri každej
    /// inicializácii naplní vopred za koniec dát, ale to nie je poškodený tok.
    #[test]
    fn kratke_podprudy_na_konci_dat() {
        let mut data = Vec::new();
        let mut zaciatky = Vec::new();
        let mut biny = Vec::new();
        let mut n = orchidea_jadro::nahoda::Nahoda::zo_semienka(7);
        for _ in 0..6 {
            zaciatky.push(data.len());
            let mut kod = Koder::novy();
            let mut ks = Kontexty::nove(1, 30);
            let b: Vec<u32> = (0..3).map(|_| (n.dalsie() % 2) as u32).collect();
            for &x in &b {
                kod.bin(&mut ks.0[k::CU_SKIP], x);
            }
            kod.ukoncenie(1);
            data.extend(kod.bajty());
            biny.push(b);
        }
        let mut d = Cabac::novy(&data, 0);
        for (i, b) in biny.iter().enumerate() {
            if i > 0 {
                assert_eq!(d.bajt_po_ukonceni(), zaciatky[i]);
                d.znova(zaciatky[i]);
            }
            let mut ks = Kontexty::nove(1, 30);
            for &x in b {
                assert_eq!(d.bin(&mut ks.0[k::CU_SKIP]), x, "podprúd {i}");
            }
            assert_eq!(d.ukoncenie(), 1);
            assert!(!d.poskodeny(), "podprúd {i} z {} bajtov nie je poškodený", data.len());
        }
    }

    #[test]
    fn inicializacia_kontextov() {
        // initValue 154 (m = 0, n = 0) pri ľubovoľnom QP → preCtxState 64 → pStateIdx 0, valMps 1
        let k = Kontexty::nove(0, 30);
        assert_eq!(k.0[k::TRANSQUANT_BYPASS], 1);
        // initValue 63: m = 3 · 5 − 45 = −30, n = (15 << 3) − 16 = 104; QP 30: (−900 >> 4) + 104 = 47 → pState 16
        assert_eq!(k.0[k::INTRA_CHROMA], 16 << 1);
        // QP 26: (−780 >> 4) + 104 = 55 → pState 8, MPS 0
        assert_eq!(Kontexty::nove(0, 26).0[k::INTRA_CHROMA], 8 << 1);
        // initValue 224 (m = 25, n = −16) pri QP 40: (1000 >> 4) − 16 = 46 → pState 17, MPS 0
        assert_eq!(Kontexty::nove(2, 40).0[k::SPLIT_TRANSFORM], 17 << 1);
        // initValue 197 (m = 15, n = 24) pri QP 51: (765 >> 4) + 24 = 71 → pState 7, MPS 1
        assert_eq!(Kontexty::nove(1, 51).0[k::CU_SKIP], (7 << 1) | 1);
    }
}
