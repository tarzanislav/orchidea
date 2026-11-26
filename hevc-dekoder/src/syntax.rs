//! Dekódovanie dát segmentu rezu (slice_segment_data, H.265 kap. 7.3.8) s rekonštrukciou: CTU, SAO, strom
//! kódovania, kódovacie a predikčné jednotky, strom transformácií, rezíduá (residual_coding) a PCM.
//! Každá jednotka sa rekonštruuje hneď po rozbore (vnútrosnímková predikcia potrebuje susedov).

use super::interpolacia::{self, Vahy};
use super::intra::{self, Susedia};
use super::obraz::{Obraz, ObrazPasov, RovinaPasov, Vzorka};
use super::odblokovanie;
use super::sao::{self, CielSao, SaoCtb};
use super::transformacia::{self, FaktorySkalovania, ParametreTb};
use crate::cabac::{Cabac, Kontexty, k};
use crate::filtre::{InfoObrazu, MapyFiltra, PohybBloku};
use crate::geometria::Geometria;
use crate::pohyb::{Blok, Okolie, PohybKol, PohybPu, PoleKol, PoleKolPasov, Rozdelenie};
use orchidea_hevc::bity::Citac;
use orchidea_hevc::rez::TypRezu;
use orchidea_hevc::{HlavickaRezu, Pps, Sps};
use orchidea_jadro::{Chyba, Vysledok};
use std::sync::Arc;

fn chyba(t: impl Into<String>) -> Chyba {
    Chyba::PoskodeneData(format!("HEVC dekodér: {}", t.into()))
}

/// Dekódovaný obraz v pamäti dekodéra (referencia a výstup). Vzorky aj pohyb sa zverejňujú po pásoch (riadkoch
/// CTB), takže obraz môže byť referenciou (aj výstupom) už počas dekódovania – čítanie nehotového pásu počká.
#[derive(Debug)]
pub struct DekodovanyObraz {
    pub id: u32,
    pub poc: i32,
    pub obraz: ObrazPasov,
    /// pohyb pre časovú predikciu (TMVP)
    pub kol: PoleKolPasov,
    /// okno orezania (conformance window) v jase: vľavo, vpravo, hore, dole
    pub okno: [usize; 4],
}

impl DekodovanyObraz {
    /// Nový rozpracovaný obraz: pásy výšky CTB `1 << log2_ctb`.
    pub fn novy(
        id: u32,
        poc: i32,
        sirka: usize,
        vyska: usize,
        log2_ctb: u32,
        okno: [usize; 4],
        (hlbka, hlbka_farby): (u32, u32),
    ) -> DekodovanyObraz {
        let ctb = 1usize << log2_ctb;
        let mut obraz = ObrazPasov::novy(sirka, vyska, ctb);
        (obraz.bitova_hlbka, obraz.bitova_hlbka_farby) = (hlbka, hlbka_farby);
        DekodovanyObraz {
            id,
            poc,
            obraz,
            kol: PoleKolPasov::nove(sirka.div_ceil(16), vyska.div_ceil(16), ctb / 16, poc),
            okno,
        }
    }

    /// Doplní nezverejnené pásy (obraz s chybou), aby naň nikto nečakal naveky.
    pub fn dopln(&self) {
        self.obraz.dopln();
        self.kol.dopln();
    }
}

/// Stav rozpracovaného obrazu.
#[derive(Debug, Clone)]
pub struct StavObrazu {
    pub obraz: Obraz,
    pub mapy: MapyFiltra,
    pub info: InfoObrazu,
    pub sao: Vec<SaoCtb>,
    /// SliceAddrRs každého CTB (u32::MAX = ešte nedekódovaný)
    pub adresa_rezu: Vec<u32>,
    /// po blokoch 4×4: CtDepth, cu_skip_flag, IntraPredModeY, pcm_flag
    pub hlbka: Vec<u8>,
    pub skip: Vec<bool>,
    pub rezim_y: Vec<u8>,
    pub pcm: Vec<bool>,
    pub pohyb: Vec<PohybPu>,
    pub kol: PoleKol,
    /// kontexty uložené po 2. CTB riadka (WPP) a na konci segmentu (závislé segmenty)
    pub ulozene_wpp: Option<Kontexty>,
    pub ulozene_ds: Option<Kontexty>,
    /// QpY poslednej kódovacej jednotky (qPY_PREV pre ďalšiu skupinu kvantovania)
    pub qp_posledny: i32,
    /// zverejňovaný obraz a postup: rekonštruované, odblokované a zverejnené riadky CTB, zverejnený pohyb
    pub ciel: Option<Arc<DekodovanyObraz>>,
    /// dekódovaných CTB v každom riadku CTB a počet úvodných celých riadkov (aj s dlaždicami – riadok je celý,
    /// keď ho dokončí posledný stĺpec dlaždíc)
    hotove_v_riadku: Vec<u16>,
    celych_riadkov: usize,
    odblokovanych: usize,
    zverejnenych: usize,
    pohyb_zverejnenych: usize,
}

impl StavObrazu {
    /// Pripraví stav na nový obraz s rovnakými rozmermi a CTB bez novej alokácie: vymažú sa len mapy, ktoré
    /// dekódovanie nevyplní celé (hrany, nenulové koeficienty, pohyb pre TMVP, rezy CTB, SAO). Vráti `false`, ak
    /// sa rozmery líšia (treba nový stav).
    pub fn obnov(&mut self, g: &Geometria, poc: i32) -> bool {
        let n_ctb = g.sirka_ctb * g.vyska_ctb;
        if self.info.sirka != g.sirka || self.info.vyska != g.vyska || self.adresa_rezu.len() != n_ctb {
            return false;
        }
        self.mapy.hrana_v.fill(0);
        self.mapy.hrana_h.fill(0);
        self.mapy.nenulove.fill(false);
        self.adresa_rezu.fill(u32::MAX);
        self.sao.fill(SaoCtb::default());
        self.info.log2_ctb = g.log2_ctb;
        self.info.sirka_ctb = g.sirka_ctb;
        self.info.vyska_ctb = g.vyska_ctb;
        self.info.ctb_dlazdica.clone_from(&g.dlazdica_rs);
        self.info.ctb_ts.clone_from(&g.rs_na_ts);
        self.info.rezy.clear();
        let (s16, v16) = (g.sirka.div_ceil(16), g.vyska.div_ceil(16));
        self.kol.sirka16 = s16;
        self.kol.vyska16 = v16;
        self.kol.poc = poc;
        self.kol.bloky.clear();
        self.kol.bloky.resize(s16 * v16, PohybKol::default());
        self.ulozene_wpp = None;
        self.ulozene_ds = None;
        self.qp_posledny = 26;
        self.ciel = None;
        self.hotove_v_riadku.clear();
        self.hotove_v_riadku.resize(g.vyska_ctb, 0);
        (self.celych_riadkov, self.odblokovanych, self.zverejnenych, self.pohyb_zverejnenych) = (0, 0, 0, 0);
        true
    }

    pub fn novy(g: &Geometria, poc: i32) -> StavObrazu {
        let mapy = MapyFiltra::nove(g.sirka, g.vyska);
        let n4 = mapy.sirka4 * mapy.vyska4;
        let (s16, v16) = (g.sirka.div_ceil(16), g.vyska.div_ceil(16));
        let n_ctb = g.sirka_ctb * g.vyska_ctb;
        StavObrazu {
            obraz: Obraz::novy(g.sirka, g.vyska),
            info: InfoObrazu {
                sirka: g.sirka,
                vyska: g.vyska,
                log2_ctb: g.log2_ctb,
                sirka_ctb: g.sirka_ctb,
                vyska_ctb: g.vyska_ctb,
                ctb_rez: vec![0; n_ctb],
                ctb_dlazdica: g.dlazdica_rs.clone(),
                ctb_ts: g.rs_na_ts.clone(),
                bitova_hlbka: 8,
                ..Default::default()
            },
            mapy,
            sao: vec![SaoCtb::default(); n_ctb],
            adresa_rezu: vec![u32::MAX; n_ctb],
            hlbka: vec![0; n4],
            skip: vec![false; n4],
            rezim_y: vec![1; n4],
            pcm: vec![false; n4],
            pohyb: vec![PohybPu::ZIADNY; n4],
            kol: PoleKol { sirka16: s16, vyska16: v16, bloky: vec![PohybKol::default(); s16 * v16], poc },
            ulozene_wpp: None,
            ulozene_ds: None,
            qp_posledny: 26,
            ciel: None,
            hotove_v_riadku: vec![0; g.vyska_ctb],
            celych_riadkov: 0,
            odblokovanych: 0,
            zverejnenych: 0,
            pohyb_zverejnenych: 0,
        }
    }

    /// CTB v riadku `ry` je zrekonštruovaný. Keď je celých prvých `n` riadkov CTB, zverejní sa pohyb týchto
    /// riadkov, odblokujú sa úseky do `n − 1` a po SAO sa zverejnia riadky CTB do `n − 2` – obraz zaostáva za
    /// rekonštrukciou len o jeden riadok CTB (pozri [`StavObrazu::posun`]).
    pub fn ctb_hotovy(&mut self, ry: usize) {
        let Some(h) = self.hotove_v_riadku.get_mut(ry) else { return };
        *h = h.saturating_add(1);
        let sirka = self.info.sirka_ctb;
        let n0 = self.celych_riadkov;
        while self.hotove_v_riadku.get(self.celych_riadkov).is_some_and(|&h| usize::from(h) >= sirka) {
            self.celych_riadkov += 1;
        }
        if self.celych_riadkov > n0 {
            self.posun(self.celych_riadkov, false);
        }
    }

    /// Koniec obrazu: filtre a zverejnenie všetkých zostávajúcich riadkov CTB.
    pub fn dokonci(&mut self) {
        let n = self.info.vyska_ctb;
        self.posun(n, true);
    }

    fn posun(&mut self, n: usize, koniec: bool) {
        let Some(ciel) = self.ciel.clone() else { return };
        let vyska_ctb = self.info.vyska_ctb;
        let n = n.min(vyska_ctb);
        while self.pohyb_zverejnenych < n {
            ciel.kol.zverejni_z(self.pohyb_zverejnenych, &self.kol);
            self.pohyb_zverejnenych += 1;
        }
        // Odblokovanie po úsekoch posunutých o 8 riadkov hore: úsek u sú riadky u·CTB − 8 … (u + 1)·CTB − 9 (prvý
        // od 0, posledný po koniec obrazu). Úsek u stačí rekonštrukcia riadka CTB u – spodných 8 riadkov vzoriek
        // ostáva nefiltrovaných, takže vnútrosnímková predikcia riadka u + 1 (číta riadok (u + 1)·CTB − 1) ich vidí
        // pred filtrom. Hranice sú na mriežke 8, preto je výsledok rovnaký ako pre celý obraz (odblokuj_pas).
        let ctb = 1usize << self.info.log2_ctb;
        let odblokovat = if koniec { vyska_ctb } else { n };
        while self.odblokovanych < odblokovat {
            let u = self.odblokovanych;
            let y0 = (u * ctb).saturating_sub(8);
            let y1 = if u + 1 == vyska_ctb { self.info.vyska } else { (u + 1) * ctb - 8 };
            odblokovanie::odblokuj_pas(&mut self.obraz, &self.mapy, &self.info, y0, y1);
            self.odblokovanych += 1;
        }
        // SAO riadka CTB r číta odblokované vzorky o riadok nad ním a pod ním: treba úseky do r + 1
        // (teda rekonštrukciu riadka CTB r + 1)
        let zverejnit = if koniec { vyska_ctb } else { self.odblokovanych.saturating_sub(1) };
        while self.zverejnenych < zverejnit {
            self.zverejni_riadok(&ciel.obraz, self.zverejnenych);
            self.zverejnenych += 1;
        }
    }

    /// SAO riadka CTB `r` priamo do nového pásu zverejňovaného obrazu (bez SAO len kópia odblokovaných vzoriek).
    fn zverejni_riadok(&self, ciel: &ObrazPasov, r: usize) {
        let s = self.info.sirka_ctb;
        if !self.sao[r * s..(r + 1) * s].iter().any(|p| p.typ != [0; 3]) {
            for c in 0..3 {
                ciel.zverejni_z(c, r, &self.obraz.roviny[c]);
            }
            return;
        }
        let pas = |rp: &RovinaPasov| {
            if r < rp.pocet_pasov() { vec![0; rp.riadky_pasu(r).len() * rp.sirka] } else { Vec::new() }
        };
        let [mut a, mut b, mut c] = [pas(&ciel.roviny[0]), pas(&ciel.roviny[1]), pas(&ciel.roviny[2])];
        {
            let ciel_sao =
                |data, rp: &RovinaPasov| CielSao { data, krok: rp.sirka, od_riadka: r * rp.vyska_pasu };
            let mut ciele = [
                ciel_sao(&mut a[..], &ciel.roviny[0]),
                ciel_sao(&mut b[..], &ciel.roviny[1]),
                ciel_sao(&mut c[..], &ciel.roviny[2]),
            ];
            sao::sao_riadok(&self.obraz, &mut ciele, &self.sao, &self.mapy, &self.info, r);
        }
        for (i, d) in [a, b, c].into_iter().enumerate() {
            if r < ciel.roviny[i].pocet_pasov() {
                ciel.roviny[i].zverejni(r, d.into_boxed_slice());
            }
        }
    }
}

/// Referenčné obrazy rezu: RefPicList0/1 ako obrazy, ich POC a dlhodobosť.
#[derive(Debug, Clone, Default)]
pub struct ReferencieRezu {
    pub obrazy: [Vec<Arc<DekodovanyObraz>>; 2],
    pub poc: [Vec<i32>; 2],
    pub lt: [Vec<bool>; 2],
}

/// Poradia prechodu (6.5.3–6.5.5): `[log2 veľkosti 0..=3][typ: 0 diagonálne, 1 vodorovne, 2 zvislo][i] → (x, y)`.
const fn prechody() -> [[[(u8, u8); 64]; 3]; 4] {
    let mut t = [[[(0u8, 0u8); 64]; 3]; 4];
    let mut l = 0;
    while l < 4 {
        let s = 1usize << l;
        // diagonálne nahor doprava
        let (mut i, mut x, mut y) = (0usize, 0i32, 0i32);
        while i < s * s {
            while y >= 0 {
                if (x as usize) < s && (y as usize) < s {
                    t[l][0][i] = (x as u8, y as u8);
                    i += 1;
                }
                y -= 1;
                x += 1;
            }
            y = x;
            x = 0;
        }
        let mut j = 0;
        while j < s * s {
            t[l][1][j] = ((j % s) as u8, (j / s) as u8);
            t[l][2][j] = ((j / s) as u8, (j % s) as u8);
            j += 1;
        }
        l += 1;
    }
    t
}
const PRECHOD: [[[(u8, u8); 64]; 3]; 4] = prechody();

/// ctxIdxMap pre sig_coeff_flag bloku 4×4 (9.3.4.2.5).
const MAPA_SIG_4X4: [u8; 16] = [0, 1, 4, 5, 2, 3, 4, 5, 6, 6, 8, 8, 7, 7, 8, 8];

/// QpC podľa qPi pre ChromaArrayType = 1 (tab. 8-10).
fn qp_farby(qpi: i32) -> i32 {
    const T: [i32; 14] = [29, 30, 31, 32, 33, 33, 34, 34, 35, 35, 36, 36, 37, 37];
    if qpi < 30 {
        qpi
    } else if qpi > 43 {
        qpi - 6
    } else {
        T[(qpi - 30) as usize]
    }
}

/// Údaje kódovacej jednotky potrebné v strome transformácií.
#[derive(Debug, Clone, Copy)]
struct Jednotka {
    intra: bool,
    rezim_c: u32,
    /// max_transform_hierarchy_depth + IntraSplitFlag
    max_hlbka: u32,
    intra_split: bool,
}

/// Dekodér jedného segmentu rezu.
#[allow(missing_debug_implementations)]
pub struct DekoderRezu<'a> {
    c: Cabac<'a>,
    kx: Kontexty,
    s: &'a mut StavObrazu,
    g: &'a Geometria,
    sps: &'a Sps,
    pps: &'a Pps,
    h: &'a HlavickaRezu,
    refs: &'a ReferencieRezu,
    kol: Option<&'a PoleKolPasov>,
    faktory: Option<&'a FaktorySkalovania>,
    rez_index: u16,
    init_typ: usize,
    // kvantovanie
    qp_y: i32,
    qp_pred: i32,
    qp_delta: i32,
    qp_delta_kodovany: bool,
    log2_min_qg: u32,
    qp_bd: i32,
    // kódovacia jednotka
    obchadzanie: bool,
    koef: [i16; 1024],
    bez_spatnej: bool,
}

impl<'a> DekoderRezu<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn novy(
        data: &'a [u8],
        s: &'a mut StavObrazu,
        g: &'a Geometria,
        sps: &'a Sps,
        pps: &'a Pps,
        h: &'a HlavickaRezu,
        refs: &'a ReferencieRezu,
        kol: Option<&'a PoleKolPasov>,
        faktory: Option<&'a FaktorySkalovania>,
        rez_index: u16,
    ) -> DekoderRezu<'a> {
        let init_typ = match h.typ {
            TypRezu::I => 0,
            TypRezu::P => {
                if h.cabac_init {
                    2
                } else {
                    1
                }
            }
            TypRezu::B => {
                if h.cabac_init {
                    1
                } else {
                    2
                }
            }
        };
        let poc = s.kol.poc;
        let bez_spatnej = refs.poc.iter().flatten().all(|&p| p <= poc);
        DekoderRezu {
            c: Cabac::novy(data, h.dlzka_rbsp),
            kx: Kontexty::nove(init_typ, h.qp),
            s,
            g,
            sps,
            pps,
            h,
            refs,
            kol,
            faktory,
            rez_index,
            init_typ,
            qp_y: h.qp,
            qp_pred: h.qp,
            qp_delta: 0,
            qp_delta_kodovany: false,
            log2_min_qg: sps.log2_ctb - pps.cu_qp_delta.unwrap_or(0),
            qp_bd: 6 * (sps.bity_jasu as i32 - 8),
            obchadzanie: false,
            koef: [0; 1024],
            bez_spatnej,
        }
    }

    #[inline]
    fn bin(&mut self, ctx: usize) -> u32 {
        self.c.bin(&mut self.kx.0[ctx])
    }

    #[inline]
    fn i4(&self, x: usize, y: usize) -> usize {
        (y >> 2) * self.s.mapy.sirka4 + (x >> 2)
    }

    #[inline]
    fn dostupny(&self, xc: i32, yc: i32, xn: i32, yn: i32) -> bool {
        self.g.dostupny(&self.s.adresa_rezu, xc, yc, xn, yn)
    }

    /// Inicializácia kontextov na začiatku podprúdu (9.3.1, 9.3.2.?): dlaždica, riadok WPP, závislý segment.
    fn kontexty_podprudu(&mut self, rs: usize, prvy_v_segmente: bool) {
        // aktuálny CTB patrí rezu už pri určovaní dostupnosti horného pravého CTB
        self.s.adresa_rezu[rs] = self.h.adresa_rezu;
        let (x, y) = (rs % self.g.sirka_ctb, rs / self.g.sirka_ctb);
        let ctb = 1i32 << self.g.log2_ctb;
        let (x0, y0) = ((x as i32) << self.g.log2_ctb, (y as i32) << self.g.log2_ctb);
        let nove = Kontexty::nove(self.init_typ, self.h.qp);
        self.kx = if self.g.prvy_v_dlazdici(rs) {
            nove
        } else if self.pps.wpp && x == self.g.zaciatok_stlpca(x) {
            match self.s.ulozene_wpp {
                Some(u) if self.dostupny(x0, y0, x0 + ctb, y0 - ctb) => u,
                _ => nove,
            }
        } else if prvy_v_segmente && self.h.zavisly {
            self.s.ulozene_ds.unwrap_or(nove)
        } else {
            nove
        };
    }

    /// Dekóduje všetky CTU segmentu rezu.
    pub fn dekoduj(&mut self) -> Vysledok<()> {
        let g = self.g;
        let n_ctb = g.sirka_ctb * g.vyska_ctb;
        let mut ts = g.rs_na_ts[self.h.adresa as usize] as usize;
        if !self.h.zavisly {
            self.s.qp_posledny = self.h.qp;
        }
        self.kontexty_podprudu(g.ts_na_rs[ts] as usize, true);
        if self.g.prvy_v_dlazdici(g.ts_na_rs[ts] as usize)
            || (self.pps.wpp && {
                let x = g.ts_na_rs[ts] as usize % g.sirka_ctb;
                x == g.zaciatok_stlpca(x)
            })
        {
            self.s.qp_posledny = self.h.qp;
        }
        loop {
            let rs = g.ts_na_rs[ts] as usize;
            self.s.adresa_rezu[rs] = self.h.adresa_rezu;
            self.s.info.ctb_rez[rs] = self.rez_index;
            let (cx, cy) = (rs % g.sirka_ctb, rs / g.sirka_ctb);
            self.ctu(cx << g.log2_ctb, cy << g.log2_ctb, rs)
                .map_err(|e| chyba(format!("CTB ({cx}, {cy}), POC {}: {e}", self.s.kol.poc)))?;
            let koniec = self.c.ukoncenie();
            if self.pps.wpp {
                let od = cx - g.zaciatok_stlpca(cx);
                if od == 1 || (od == 0 && g.sirka_dlazdice(cx) == 1) {
                    self.s.ulozene_wpp = Some(self.kx);
                }
            }
            if self.c.poskodeny() {
                return Err(chyba(format!(
                    "dáta rezu sa skončili predčasne (CTB {cx}, {cy}; {})",
                    self.c.ladenie()
                )));
            }
            // po celom riadku CTB filtre a zverejnenie predošlých riadkov (iné vlákna na ne môžu čakať)
            self.s.ctb_hotovy(cy);
            if koniec == 1 {
                if self.pps.zavisle_rezy {
                    self.s.ulozene_ds = Some(self.kx);
                }
                return Ok(());
            }
            ts += 1;
            if ts >= n_ctb {
                return Err(chyba("rez pokračuje za posledný CTB"));
            }
            let rs2 = g.ts_na_rs[ts] as usize;
            let x2 = rs2 % g.sirka_ctb;
            let nova_dlazdica = self.pps.dlazdice.is_some() && g.dlazdica_rs[rs2] != g.dlazdica_rs[rs];
            let novy_riadok = self.pps.wpp && x2 == g.zaciatok_stlpca(x2);
            if nova_dlazdica || novy_riadok {
                if self.c.ukoncenie() != 1 {
                    return Err(chyba(format!(
                        "end_of_subset_one_bit = 0 po CTB ({cx}, {cy}), POC {}",
                        self.s.kol.poc
                    )));
                }
                let b = self.c.bajt_po_ukonceni();
                self.c.znova(b);
                self.kontexty_podprudu(rs2, false);
                self.s.qp_posledny = self.h.qp;
            }
        }
    }

    // -----------------------------------------------------------------------------------------------------
    // SAO (7.3.8.3)
    // -----------------------------------------------------------------------------------------------------

    fn sao(&mut self, rs: usize) {
        let g = self.g;
        let (rx, ry) = (rs % g.sirka_ctb, rs / g.sirka_ctb);
        let adr = self.h.adresa_rezu as usize;
        let mut zlucit = None;
        if rx > 0 {
            let vlavo = rs - 1;
            if rs > adr && g.dlazdica_rs[vlavo] == g.dlazdica_rs[rs] && self.bin(k::SAO_MERGE) == 1 {
                zlucit = Some(vlavo);
            }
        }
        if zlucit.is_none() && ry > 0 {
            let hore = rs - g.sirka_ctb;
            if hore >= adr && g.dlazdica_rs[hore] == g.dlazdica_rs[rs] && self.bin(k::SAO_MERGE) == 1 {
                zlucit = Some(hore);
            }
        }
        if let Some(z) = zlucit {
            self.s.sao[rs] = self.s.sao[z];
            return;
        }
        let mut p = SaoCtb::default();
        for c in 0..3 {
            if !((self.h.sao_jas && c == 0) || (self.h.sao_farba && c > 0)) {
                continue;
            }
            if c < 2 {
                p.typ[c] = if self.bin(k::SAO_TYPE) == 0 {
                    0
                } else if self.c.obchadzanie() == 0 {
                    1
                } else {
                    2
                };
            } else {
                p.typ[2] = p.typ[1];
            }
            if p.typ[c] == 0 {
                continue;
            }
            // cMax sao_offset_abs = (1 << (Min(bitDepth, 10) − 5)) − 1, bitDepth podľa zložky
            let max = (1u32 << (self.hlbka(c).min(10) - 5)) - 1;
            let mut abs = [0i16; 4];
            for a in abs.iter_mut() {
                let mut v = 0;
                while v < max && self.c.obchadzanie() == 1 {
                    v += 1;
                }
                *a = v as i16;
            }
            if p.typ[c] == 1 {
                for a in abs.iter_mut() {
                    if *a != 0 && self.c.obchadzanie() == 1 {
                        *a = -*a;
                    }
                }
                p.pasmo[c] = self.c.obchadzanie_n(5) as u8;
                p.posuny[c] = abs;
            } else {
                if c < 2 {
                    p.trieda[c] = self.c.obchadzanie_n(2) as u8;
                } else {
                    p.trieda[2] = p.trieda[1];
                }
                p.posuny[c] = [abs[0], abs[1], -abs[2], -abs[3]];
            }
        }
        self.s.sao[rs] = p;
    }

    fn ctu(&mut self, x0: usize, y0: usize, rs: usize) -> Vysledok<()> {
        if self.h.sao_jas || self.h.sao_farba {
            self.sao(rs);
        }
        self.strom(x0, y0, self.g.log2_ctb, 0)
    }

    // -----------------------------------------------------------------------------------------------------
    // Strom kódovania a kódovacia jednotka (7.3.8.4, 7.3.8.5)
    // -----------------------------------------------------------------------------------------------------

    fn strom(&mut self, x0: usize, y0: usize, log2: u32, hlbka: u32) -> Vysledok<()> {
        let n = 1usize << log2;
        let split = if x0 + n <= self.g.sirka && y0 + n <= self.g.vyska && log2 > self.sps.log2_min_cb {
            let mut ctx = 0;
            if self.dostupny(x0 as i32, y0 as i32, x0 as i32 - 1, y0 as i32)
                && self.s.hlbka[self.i4(x0 - 1, y0)] as u32 > hlbka
            {
                ctx += 1;
            }
            if self.dostupny(x0 as i32, y0 as i32, x0 as i32, y0 as i32 - 1)
                && self.s.hlbka[self.i4(x0, y0 - 1)] as u32 > hlbka
            {
                ctx += 1;
            }
            self.bin(k::SPLIT_CU + ctx) == 1
        } else {
            log2 > self.sps.log2_min_cb
        };
        if log2 >= self.log2_min_qg {
            self.qp_delta_kodovany = false;
            self.qp_delta = 0;
            self.zaciatok_skupiny_qp(x0, y0);
        }
        if split {
            let x1 = x0 + n / 2;
            let y1 = y0 + n / 2;
            self.strom(x0, y0, log2 - 1, hlbka + 1)?;
            if x1 < self.g.sirka {
                self.strom(x1, y0, log2 - 1, hlbka + 1)?;
            }
            if y1 < self.g.vyska {
                self.strom(x0, y1, log2 - 1, hlbka + 1)?;
            }
            if x1 < self.g.sirka && y1 < self.g.vyska {
                self.strom(x1, y1, log2 - 1, hlbka + 1)?;
            }
            Ok(())
        } else {
            self.jednotka(x0, y0, log2, hlbka)
        }
    }

    /// qPY_PRED skupiny kvantovania (8.6.1).
    fn zaciatok_skupiny_qp(&mut self, x: usize, y: usize) {
        let maska = (1usize << self.g.log2_ctb) - 1;
        let pred = self.s.qp_posledny;
        let qa = if x & maska != 0 { self.s.mapy.qp[self.i4(x - 1, y)] as i32 } else { pred };
        let qb = if y & maska != 0 { self.s.mapy.qp[self.i4(x, y - 1)] as i32 } else { pred };
        self.qp_pred = (qa + qb + 1) >> 1;
        self.qp_y = self.qp_pred;
    }

    fn nastav_qp(&mut self) {
        let bd = self.qp_bd;
        self.qp_y = ((self.qp_pred + self.qp_delta + 52 + 2 * bd) % (52 + bd)) - bd;
    }

    /// Vyplní mapu po blokoch 4×4 v obdĺžniku.
    fn vypln<T: Copy>(mapa: &mut [T], sirka4: usize, x: usize, y: usize, w: usize, h: usize, v: T) {
        for by in (y >> 2)..((y + h) >> 2) {
            mapa[by * sirka4 + (x >> 2)..by * sirka4 + ((x + w) >> 2)].fill(v);
        }
    }

    fn jednotka(&mut self, x0: usize, y0: usize, log2: u32, hlbka: u32) -> Vysledok<()> {
        let n = 1usize << log2;
        let s4 = self.s.mapy.sirka4;
        self.obchadzanie = self.pps.transquant_bypass && self.bin(k::TRANSQUANT_BYPASS) == 1;
        let mut skip = false;
        if self.h.typ != TypRezu::I {
            let mut ctx = 0;
            if self.dostupny(x0 as i32, y0 as i32, x0 as i32 - 1, y0 as i32)
                && self.s.skip[self.i4(x0 - 1, y0)]
            {
                ctx += 1;
            }
            if self.dostupny(x0 as i32, y0 as i32, x0 as i32, y0 as i32 - 1)
                && self.s.skip[self.i4(x0, y0 - 1)]
            {
                ctx += 1;
            }
            skip = self.bin(k::CU_SKIP + ctx) == 1;
        }
        Self::vypln(&mut self.s.hlbka, s4, x0, y0, n, n, hlbka as u8);
        Self::vypln(&mut self.s.skip, s4, x0, y0, n, n, skip);
        let mut pcm = false;
        let mut rozdelenie = Rozdelenie::P2Nx2N;
        if skip {
            Self::vypln(&mut self.s.mapy.intra, s4, x0, y0, n, n, false);
            Self::vypln(&mut self.s.pcm, s4, x0, y0, n, n, false);
            self.predikcna(x0, y0, n, x0, y0, n, n, 0, rozdelenie, true)?;
        } else {
            let intra = self.h.typ == TypRezu::I || self.bin(k::PRED_MODE) == 1;
            Self::vypln(&mut self.s.mapy.intra, s4, x0, y0, n, n, intra);
            if !intra || log2 == self.sps.log2_min_cb {
                rozdelenie = self.part_mode(intra, log2);
            }
            if intra {
                if rozdelenie == Rozdelenie::P2Nx2N
                    && let Some(p) = self.sps.pcm
                    && log2 >= p.log2_min
                    && log2 <= p.log2_max
                {
                    pcm = self.c.ukoncenie() == 1;
                }
                Self::vypln(&mut self.s.pcm, s4, x0, y0, n, n, pcm);
                Self::vypln(&mut self.s.pohyb, s4, x0, y0, n, n, PohybPu::ZIADNY);
                if pcm {
                    self.pcm(x0, y0, log2)?;
                } else {
                    let rezim_c = self.rezimy_intra(x0, y0, log2, rozdelenie == Rozdelenie::PNxN);
                    let intra_split = rozdelenie == Rozdelenie::PNxN;
                    let ju = Jednotka {
                        intra: true,
                        rezim_c,
                        max_hlbka: self.sps.max_hlbka_intra + intra_split as u32,
                        intra_split,
                    };
                    self.strom_transformacii(&ju, x0, y0, x0, y0, log2, 0, 0, [false, false])?;
                }
            } else {
                Self::vypln(&mut self.s.pcm, s4, x0, y0, n, n, false);
                let (n2, n4) = (n / 2, n / 4);
                let bloky: &[(usize, usize, usize, usize)] = match rozdelenie {
                    Rozdelenie::P2Nx2N => &[(0, 0, n, n)],
                    Rozdelenie::P2NxN => &[(0, 0, n, n2), (0, n2, n, n2)],
                    Rozdelenie::PNx2N => &[(0, 0, n2, n), (n2, 0, n2, n)],
                    Rozdelenie::P2NxnU => &[(0, 0, n, n4), (0, n4, n, n - n4)],
                    Rozdelenie::P2NxnD => &[(0, 0, n, n - n4), (0, n - n4, n, n4)],
                    Rozdelenie::PnLx2N => &[(0, 0, n4, n), (n4, 0, n - n4, n)],
                    Rozdelenie::PnRx2N => &[(0, 0, n - n4, n), (n - n4, 0, n4, n)],
                    Rozdelenie::PNxN => &[(0, 0, n2, n2), (n2, 0, n2, n2), (0, n2, n2, n2), (n2, n2, n2, n2)],
                };
                let bloky = bloky.to_vec();
                let mut zlucenie_2nx2n = false;
                for (i, &(bx, by, bw, bh)) in bloky.iter().enumerate() {
                    let z =
                        self.predikcna(x0, y0, n, x0 + bx, y0 + by, bw, bh, i as u32, rozdelenie, false)?;
                    if i == 0 && rozdelenie == Rozdelenie::P2Nx2N {
                        zlucenie_2nx2n = z;
                    }
                }
                // hrany predikčných blokov vnútri jednotky
                self.hrany_pu(x0, y0, n, rozdelenie);
                let rqt = zlucenie_2nx2n || self.bin(k::RQT_ROOT_CBF) == 1;
                if rqt {
                    let inter_split = self.sps.max_hlbka_inter == 0 && rozdelenie != Rozdelenie::P2Nx2N;
                    let ju = Jednotka {
                        intra: false,
                        rezim_c: 0,
                        max_hlbka: self.sps.max_hlbka_inter,
                        intra_split: inter_split,
                    };
                    self.strom_transformacii(&ju, x0, y0, x0, y0, log2, 0, 0, [false, false])?;
                }
            }
        }
        // hrany kódovacej jednotky sú vždy hranami transformačného bloku (8.7.2.3) – aj bez stromu transformácií
        self.hrany_tu(x0, y0, n);
        self.nastav_qp();
        let qp = self.qp_y as i8;
        Self::vypln(&mut self.s.mapy.qp, s4, x0, y0, n, n, qp);
        let bez = (pcm && self.sps.pcm.is_some_and(|p| p.bez_filtra)) || self.obchadzanie;
        Self::vypln(&mut self.s.mapy.bez_filtra, s4, x0, y0, n, n, bez);
        self.s.qp_posledny = self.qp_y;
        Ok(())
    }

    fn part_mode(&mut self, intra: bool, log2: u32) -> Rozdelenie {
        if self.bin(k::PART_MODE) == 1 {
            return Rozdelenie::P2Nx2N;
        }
        if intra {
            return Rozdelenie::PNxN;
        }
        let min = self.sps.log2_min_cb;
        if log2 == min {
            if self.bin(k::PART_MODE + 1) == 1 {
                return Rozdelenie::P2NxN;
            }
            if log2 == 3 {
                return Rozdelenie::PNx2N;
            }
            return if self.bin(k::PART_MODE + 2) == 1 { Rozdelenie::PNx2N } else { Rozdelenie::PNxN };
        }
        if !self.sps.amp {
            return if self.bin(k::PART_MODE + 1) == 1 { Rozdelenie::P2NxN } else { Rozdelenie::PNx2N };
        }
        if self.bin(k::PART_MODE + 1) == 1 {
            if self.bin(k::PART_MODE + 3) == 1 {
                return Rozdelenie::P2NxN;
            }
            return if self.c.obchadzanie() == 1 { Rozdelenie::P2NxnD } else { Rozdelenie::P2NxnU };
        }
        if self.bin(k::PART_MODE + 3) == 1 {
            return Rozdelenie::PNx2N;
        }
        if self.c.obchadzanie() == 1 { Rozdelenie::PnRx2N } else { Rozdelenie::PnLx2N }
    }

    /// Režimy vnútrosnímkovej predikcie jasu (8.4.2) a farby (8.4.3); vráti IntraPredModeC.
    fn rezimy_intra(&mut self, x0: usize, y0: usize, log2: u32, nxn: bool) -> u32 {
        let n = 1usize << log2;
        let pb = if nxn { n / 2 } else { n };
        let pocet = if nxn { 4 } else { 1 };
        let mut priznak = [false; 4];
        for p in priznak.iter_mut().take(pocet) {
            *p = self.bin(k::PREV_INTRA_LUMA) == 1;
        }
        let mut prvy = 0;
        for (j, &pr) in priznak.iter().enumerate().take(pocet) {
            let (x, y) = (x0 + (j % 2) * pb, y0 + (j / 2) * pb);
            let kand = self.kandidati_mpm(x, y);
            let rezim = if pr {
                let mut idx = 0;
                while idx < 2 && self.c.obchadzanie() == 1 {
                    idx += 1;
                }
                kand[idx]
            } else {
                let mut r = self.c.obchadzanie_n(5);
                let mut z = kand;
                z.sort_unstable();
                for &kz in &z {
                    if r >= kz {
                        r += 1;
                    }
                }
                r
            };
            if j == 0 {
                prvy = rezim;
            }
            let s4 = self.s.mapy.sirka4;
            Self::vypln(&mut self.s.rezim_y, s4, x, y, pb, pb, rezim as u8);
        }
        // intra_chroma_pred_mode
        let m = if self.bin(k::INTRA_CHROMA) == 0 { 4 } else { self.c.obchadzanie_n(2) };
        if m == 4 {
            prvy
        } else {
            let r = [0, 26, 10, 1][m as usize];
            if r == prvy { 34 } else { r }
        }
    }

    fn kandidati_mpm(&self, x: usize, y: usize) -> [u32; 3] {
        let kand = |xn: i32, yn: i32, hore: bool| -> u32 {
            if !self.dostupny(x as i32, y as i32, xn, yn) {
                return 1;
            }
            let i = self.i4(xn as usize, yn as usize);
            if !self.s.mapy.intra[i] || self.s.pcm[i] {
                return 1;
            }
            if hore && (yn as usize) < ((y >> self.g.log2_ctb) << self.g.log2_ctb) {
                return 1;
            }
            self.s.rezim_y[i] as u32
        };
        let a = kand(x as i32 - 1, y as i32, false);
        let b = kand(x as i32, y as i32 - 1, true);
        if a == b {
            if a < 2 { [0, 1, 26] } else { [a, 2 + ((a + 29) % 32), 2 + ((a - 2 + 1) % 32)] }
        } else {
            let c = if a != 0 && b != 0 {
                0
            } else if a != 1 && b != 1 {
                1
            } else {
                26
            };
            [a, b, c]
        }
    }

    // -----------------------------------------------------------------------------------------------------
    // PCM (7.3.8.7)
    // -----------------------------------------------------------------------------------------------------

    fn pcm(&mut self, x0: usize, y0: usize, log2: u32) -> Vysledok<()> {
        let p = self.sps.pcm.ok_or_else(|| chyba("PCM bez parametrov v SPS"))?;
        let zaciatok = self.c.bajt_po_ukonceni();
        let data = self.c.data();
        let mut r = Citac::novy(data.get(zaciatok..).ok_or_else(|| chyba("PCM za koncom dát"))?);
        let n = 1usize << log2;
        let posun_y = self.sps.bity_jasu - p.bity_jasu as u32;
        let posun_c = self.sps.bity_farby - p.bity_farby as u32;
        for y in 0..n {
            for x in 0..n {
                let v = r.u(p.bity_jasu as u32)? << posun_y;
                let rov = &mut self.s.obraz.roviny[0];
                rov.data[(y0 + y) * rov.krok + x0 + x] = v as Vzorka;
            }
        }
        for c in 1..3 {
            for y in 0..n / 2 {
                for x in 0..n / 2 {
                    let v = r.u(p.bity_farby as u32)? << posun_c;
                    let rov = &mut self.s.obraz.roviny[c];
                    rov.data[(y0 / 2 + y) * rov.krok + x0 / 2 + x] = v as Vzorka;
                }
            }
        }
        let koniec = zaciatok + r.pozicia().div_ceil(8);
        self.c.znova(koniec);
        Ok(())
    }

    // -----------------------------------------------------------------------------------------------------
    // Predikčná jednotka (7.3.8.6, 8.5)
    // -----------------------------------------------------------------------------------------------------

    /// Rozbor a predikcia jedného PB; vráti merge_flag.
    #[allow(clippy::too_many_arguments)]
    fn predikcna(
        &mut self,
        x_cb: usize,
        y_cb: usize,
        n_cb: usize,
        x: usize,
        y: usize,
        w: usize,
        hh: usize,
        index: u32,
        rozdelenie: Rozdelenie,
        skip: bool,
    ) -> Vysledok<bool> {
        let max_merge = self.h.max_zlucenie;
        let zlucenie = skip || self.bin(k::MERGE_FLAG) == 1;
        let blok = Blok {
            x_cb: x_cb as i32,
            y_cb: y_cb as i32,
            n_cb: n_cb as i32,
            x: x as i32,
            y: y as i32,
            w: w as i32,
            h: hh as i32,
            index,
            rozdelenie,
        };
        let mut p = PohybPu::ZIADNY;
        let mut mvd = [[0i32; 2]; 2];
        let mut mvp_priznak = [0u32; 2];
        let mut merge_idx = 0;
        if zlucenie {
            if max_merge > 1 && self.bin(k::MERGE_IDX) == 1 {
                merge_idx = 1;
                while merge_idx < max_merge - 1 && self.c.obchadzanie() == 1 {
                    merge_idx += 1;
                }
            }
        } else {
            let smer = if self.h.typ == TypRezu::B {
                if w + hh != 12 {
                    let hlbka = self.s.hlbka[self.i4(x_cb, y_cb)] as usize;
                    if self.bin(k::INTER_PRED_IDC + hlbka) == 1 { 2 } else { self.bin(k::INTER_PRED_IDC + 4) }
                } else {
                    self.bin(k::INTER_PRED_IDC + 4)
                }
            } else {
                0
            };
            for lx in 0..2 {
                if (lx == 0 && smer == 1) || (lx == 1 && smer == 0) {
                    continue;
                }
                let aktivne = self.h.aktivne[lx];
                let mut r = 0u32;
                if aktivne > 1 && self.bin(k::REF_IDX) == 1 {
                    r = 1;
                    if aktivne > 2 && self.bin(k::REF_IDX + 1) == 1 {
                        r = 2;
                        while r < aktivne - 1 && self.c.obchadzanie() == 1 {
                            r += 1;
                        }
                    }
                }
                p.ref_idx[lx] = r as i8;
                if lx == 1 && self.h.mvd_l1_nula && smer == 2 {
                    mvd[1] = [0, 0];
                } else {
                    mvd[lx] = self.mvd();
                }
                mvp_priznak[lx] = self.bin(k::MVP_FLAG);
            }
        }
        // odvodenie pohybu
        let pohyb = {
            let g = self.g;
            let adresy = &self.s.adresa_rezu;
            let dostupny = |xc: i32, yc: i32, xn: i32, yn: i32| g.dostupny(adresy, xc, yc, xn, yn);
            let ok = Okolie {
                mapa: &self.s.pohyb,
                sirka4: self.s.mapy.sirka4,
                dostupny: &dostupny,
                sirka: g.sirka as i32,
                vyska: g.vyska as i32,
                log2_ctb: g.log2_ctb,
                poc: self.s.kol.poc,
                ref_poc: [&self.refs.poc[0], &self.refs.poc[1]],
                ref_lt: [&self.refs.lt[0], &self.refs.lt[1]],
                kol: if self.h.tmvp { self.kol } else { None },
                b_rez: self.h.typ == TypRezu::B,
                kol_z_l0: self.h.kolokovany_z_l0,
                bez_spatnej: self.bez_spatnej,
                log2_par_mrg: self.pps.log2_parallel_merge_level,
                max_zlucenie: max_merge,
                aktivne: self.h.aktivne,
            };
            if zlucenie {
                ok.zlucenie(&blok, merge_idx)
            } else {
                for lx in 0..2 {
                    if p.pred(lx) {
                        let mvp = ok.amvp(&blok, lx, p.ref_idx[lx] as usize, mvp_priznak[lx]);
                        p.mv[lx] = [
                            (mvp[0] as i32 + mvd[lx][0]) as u16 as i16,
                            (mvp[1] as i32 + mvd[lx][1]) as u16 as i16,
                        ];
                    }
                }
                p
            }
        };
        for lx in 0..2 {
            if pohyb.pred(lx) && pohyb.ref_idx[lx] as usize >= self.refs.obrazy[lx].len() {
                return Err(chyba("referenčný index mimo zoznamu"));
            }
        }
        self.uloz_pohyb(x, y, w, hh, pohyb);
        self.medzisnimkova(x, y, w, hh, pohyb);
        Ok(zlucenie)
    }

    fn mvd(&mut self) -> [i32; 2] {
        let g0 = [self.bin(k::MVD_GT0), self.bin(k::MVD_GT0)];
        let mut g1 = [0, 0];
        for i in 0..2 {
            if g0[i] == 1 {
                g1[i] = self.bin(k::MVD_GT1);
            }
        }
        let mut v = [0i32; 2];
        for i in 0..2 {
            if g0[i] == 1 {
                let mut abs = 1i64;
                if g1[i] == 1 {
                    abs = 2 + self.exp_golomb(1) as i64;
                }
                let abs = abs.min(1 << 15) as i32;
                v[i] = if self.c.obchadzanie() == 1 { -abs } else { abs };
            }
        }
        v
    }

    /// Exp-Golomb k-tého rádu z binov obchádzania (9.3.3.5).
    fn exp_golomb(&mut self, mut kk: u32) -> u32 {
        let mut v = 0u32;
        while self.c.obchadzanie() == 1 {
            v = v.saturating_add(1 << kk);
            kk += 1;
            if kk > 31 {
                return v;
            }
        }
        v.saturating_add(self.c.obchadzanie_n(kk))
    }

    fn uloz_pohyb(&mut self, x: usize, y: usize, w: usize, h: usize, p: PohybPu) {
        let s4 = self.s.mapy.sirka4;
        Self::vypln(&mut self.s.pohyb, s4, x, y, w, h, p);
        let poc_z = |lx: usize| (p.pred(lx)).then(|| self.refs.poc[lx][p.ref_idx[lx] as usize]);
        let pb = PohybBloku { mv: p.mv, obraz: [poc_z(0), poc_z(1)] };
        Self::vypln(&mut self.s.mapy.pohyb, s4, x, y, w, h, pb);
        // pohyb pre TMVP na mriežke 16 × 16
        let kol = PohybKol {
            mv: p.mv,
            poc: [pb.obraz[0].unwrap_or(0), pb.obraz[1].unwrap_or(0)],
            dlhodoba: [
                p.pred(0) && self.refs.lt[0][p.ref_idx[0] as usize],
                p.pred(1) && self.refs.lt[1][p.ref_idx[1] as usize],
            ],
            pred: [p.pred(0), p.pred(1)],
        };
        let s16 = self.s.kol.sirka16;
        for by in (y.div_ceil(16) * 16..y + h).step_by(16) {
            for bx in (x.div_ceil(16) * 16..x + w).step_by(16) {
                self.s.kol.bloky[(by >> 4) * s16 + (bx >> 4)] = kol;
            }
        }
    }

    /// Bitová hĺbka zložky `c` (BitDepthY alebo BitDepthC).
    #[inline]
    fn hlbka(&self, c: usize) -> u32 {
        if c == 0 { self.sps.bity_jasu } else { self.sps.bity_farby }
    }

    fn vahy(&self, p: &PohybPu, c: usize) -> Option<Vahy> {
        let t = self.h.vahy.as_ref()?;
        let mut v = Vahy { log2_denom: if c == 0 { t.log2_jas } else { t.log2_farba }, w: [0; 2], o: [0; 2] };
        // o = luma_offset << (BitDepthY − 8), ChromaOffset << (BitDepthC − 8)
        let posun = self.hlbka(c) - 8;
        for lx in 0..2 {
            if p.pred(lx) {
                let (w, o) = if c == 0 {
                    t.jas(lx, p.ref_idx[lx] as usize)
                } else {
                    t.farba(lx, p.ref_idx[lx] as usize, c - 1)
                };
                v.w[lx] = w;
                v.o[lx] = o << posun;
            }
        }
        Some(v)
    }

    /// Medzisnímková predikcia vzoriek PB (8.5.3.3).
    fn medzisnimkova(&mut self, x: usize, y: usize, w: usize, h: usize, p: PohybPu) {
        for c in 0..3 {
            let (sx, sy, sw, sh) = if c == 0 { (x, y, w, h) } else { (x / 2, y / 2, w / 2, h / 2) };
            let vahy = self.vahy(&p, c);
            let ref_z = |lx: usize| -> Option<(&RovinaPasov, [i32; 2])> {
                p.pred(lx).then(|| {
                    (
                        &self.refs.obrazy[lx][p.ref_idx[lx] as usize].obraz.roviny[c],
                        [p.mv[lx][0] as i32, p.mv[lx][1] as i32],
                    )
                })
            };
            let (r0, r1) = (ref_z(0), ref_z(1));
            let hlbka = self.hlbka(c);
            let rov = &mut self.s.obraz.roviny[c];
            let krok = rov.krok;
            let ciel = &mut rov.data[sy * krok + sx..];
            interpolacia::predikuj(
                c == 0,
                r0,
                r1,
                sx as i32,
                sy as i32,
                sw,
                sh,
                vahy.as_ref(),
                hlbka,
                ciel,
                krok,
            );
        }
    }

    // -----------------------------------------------------------------------------------------------------
    // Hrany pre odblokovací filter
    // -----------------------------------------------------------------------------------------------------

    /// Smie sa filtrovať ľavá (zvislá) alebo horná hrana bloku na (x, y)? (filterEdgeFlag, 8.7.2)
    fn filtrovat_hranu(&self, x: usize, y: usize, zvisla: bool) -> bool {
        if self.h.odblokovanie_vypnute {
            return false;
        }
        let (xn, yn) = if zvisla { (x as i32 - 1, y as i32) } else { (x as i32, y as i32 - 1) };
        if xn < 0 || yn < 0 {
            return false;
        }
        let (cn, cc) = (self.g.ctb(xn as usize, yn as usize), self.g.ctb(x, y));
        if cn == cc {
            return true;
        }
        if self.g.dlazdica_rs[cn] != self.g.dlazdica_rs[cc]
            && !self.pps.dlazdice.as_ref().is_some_and(|d| d.filter_cez_dlazdice)
        {
            return false;
        }
        if self.s.adresa_rezu[cn] != self.s.adresa_rezu[cc] && !self.h.filter_cez_rezy {
            return false;
        }
        true
    }

    /// Označí ľavú a hornú hranu bloku n × n ako hranu TU (hodnota 2).
    fn hrany_tu(&mut self, x0: usize, y0: usize, n: usize) {
        if self.h.odblokovanie_vypnute {
            return;
        }
        let s4 = self.s.mapy.sirka4;
        if x0.is_multiple_of(8) && self.filtrovat_hranu(x0, y0, true) {
            for y in (y0..(y0 + n).min(self.g.vyska)).step_by(4) {
                self.s.mapy.hrana_v[(y >> 2) * s4 + (x0 >> 2)] = 2;
            }
        }
        if y0.is_multiple_of(8) && self.filtrovat_hranu(x0, y0, false) {
            for x in (x0..(x0 + n).min(self.g.sirka)).step_by(4) {
                self.s.mapy.hrana_h[(y0 >> 2) * s4 + (x >> 2)] = 2;
            }
        }
    }

    /// Vnútorné hrany predikčných blokov jednotky (hodnota 1, ak tam nie je hrana TU).
    fn hrany_pu(&mut self, x0: usize, y0: usize, n: usize, r: Rozdelenie) {
        if self.h.odblokovanie_vypnute {
            return;
        }
        let s4 = self.s.mapy.sirka4;
        let (n2, n4) = (n / 2, n / 4);
        let zvisle: &[usize] = match r {
            Rozdelenie::PNx2N | Rozdelenie::PNxN => &[n2],
            Rozdelenie::PnLx2N => &[n4],
            Rozdelenie::PnRx2N => &[n - n4],
            _ => &[],
        };
        let vodorovne: &[usize] = match r {
            Rozdelenie::P2NxN | Rozdelenie::PNxN => &[n2],
            Rozdelenie::P2NxnU => &[n4],
            Rozdelenie::P2NxnD => &[n - n4],
            _ => &[],
        };
        for &dx in zvisle {
            let x = x0 + dx;
            if x.is_multiple_of(8) {
                for y in (y0..y0 + n).step_by(4) {
                    let i = (y >> 2) * s4 + (x >> 2);
                    if self.s.mapy.hrana_v[i] == 0 {
                        self.s.mapy.hrana_v[i] = 1;
                    }
                }
            }
        }
        for &dy in vodorovne {
            let y = y0 + dy;
            if y.is_multiple_of(8) {
                for x in (x0..x0 + n).step_by(4) {
                    let i = (y >> 2) * s4 + (x >> 2);
                    if self.s.mapy.hrana_h[i] == 0 {
                        self.s.mapy.hrana_h[i] = 1;
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------------------------------------
    // Strom transformácií a transformačná jednotka (7.3.8.8, 7.3.8.10)
    // -----------------------------------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn strom_transformacii(
        &mut self,
        ju: &Jednotka,
        x0: usize,
        y0: usize,
        xb: usize,
        yb: usize,
        log2: u32,
        hlbka: u32,
        blk: u32,
        cbf_rodic: [bool; 2],
    ) -> Vysledok<()> {
        let sps = self.sps;
        let split = if log2 <= sps.log2_max_tb
            && log2 > sps.log2_min_tb
            && hlbka < ju.max_hlbka
            && !(ju.intra_split && hlbka == 0)
        {
            self.bin(k::SPLIT_TRANSFORM + 5 - log2 as usize) == 1
        } else {
            log2 > sps.log2_max_tb || (ju.intra_split && hlbka == 0)
        };
        let cbf_c = if log2 > 2 {
            let mut v = [false; 2];
            for (c, vc) in v.iter_mut().enumerate() {
                if hlbka == 0 || cbf_rodic[c] {
                    *vc = self.bin(k::CBF_CHROMA + hlbka as usize) == 1;
                }
            }
            v
        } else {
            cbf_rodic
        };
        if split {
            let h = 1usize << (log2 - 1);
            for (i, (dx, dy)) in [(0, 0), (h, 0), (0, h), (h, h)].into_iter().enumerate() {
                self.strom_transformacii(ju, x0 + dx, y0 + dy, x0, y0, log2 - 1, hlbka + 1, i as u32, cbf_c)?;
            }
            Ok(())
        } else {
            let cbf_y = if ju.intra || hlbka != 0 || cbf_c[0] || cbf_c[1] {
                self.bin(k::CBF_LUMA + (hlbka == 0) as usize) == 1
            } else {
                true
            };
            self.transformacna(ju, x0, y0, xb, yb, log2, blk, cbf_y, cbf_c)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn transformacna(
        &mut self,
        ju: &Jednotka,
        x0: usize,
        y0: usize,
        xb: usize,
        yb: usize,
        log2: u32,
        blk: u32,
        cbf_y: bool,
        cbf_c: [bool; 2],
    ) -> Vysledok<()> {
        let n = 1usize << log2;
        if (cbf_y || cbf_c[0] || cbf_c[1]) && self.pps.cu_qp_delta.is_some() && !self.qp_delta_kodovany {
            let mut pref = 0;
            while pref < 5 && self.bin(k::CU_QP_DELTA + (pref > 0) as usize) == 1 {
                pref += 1;
            }
            let mut abs = pref;
            if pref == 5 {
                abs += self.exp_golomb(0).min(1 << 10) as i32;
            }
            if abs > 0 && self.c.obchadzanie() == 1 {
                abs = -abs;
            }
            let hranica = 26 + self.qp_bd / 2;
            if abs < -hranica || abs > hranica - 1 {
                return Err(chyba(format!("CuQpDeltaVal = {abs} mimo rozsahu")));
            }
            self.qp_delta = abs;
            self.qp_delta_kodovany = true;
            self.nastav_qp();
        }
        // hrana TU (jas)
        self.hrany_tu(x0, y0, n);
        // jas
        let rezim_y = self.s.rezim_y[self.i4(x0, y0)] as u32;
        if ju.intra {
            self.predikcia_intra(0, x0, y0, log2, rezim_y);
        }
        if cbf_y {
            let s4 = self.s.mapy.sirka4;
            Self::vypln(&mut self.s.mapy.nenulove, s4, x0, y0, n, n, true);
            self.rezidua_a_rekonstrukcia(0, x0, y0, log2, ju, rezim_y)?;
        }
        // farba 4:2:0
        let (xc, yc, log2c) = if log2 > 2 {
            (x0 / 2, y0 / 2, log2 - 1)
        } else if blk == 3 {
            (xb / 2, yb / 2, 2)
        } else {
            return Ok(());
        };
        for c in 1..3 {
            if ju.intra {
                self.predikcia_intra(c, xc, yc, log2c, ju.rezim_c);
            }
            if cbf_c[c - 1] {
                self.rezidua_a_rekonstrukcia(c, xc, yc, log2c, ju, ju.rezim_c)?;
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------------------------------------
    // Vnútrosnímková predikcia – susedia (8.4.4.2.1)
    // -----------------------------------------------------------------------------------------------------

    fn predikcia_intra(&mut self, c: usize, x: usize, y: usize, log2: u32, rezim: u32) {
        let n = 1usize << log2;
        let sx = (c > 0) as usize;
        let (xl, yl) = ((x << sx) as i32, (y << sx) as i32);
        let jednotka = 4 >> sx;
        let mut sus = Susedia::default();
        let rov = &self.s.obraz.roviny[c];
        let (sirka_c, vyska_c) = (rov.sirka, rov.vyska);
        let obmedzena = self.pps.obmedzena_intra;
        let dost = |xn: i32, yn: i32| -> bool {
            if xn < 0 || yn < 0 || xn as usize >= sirka_c || yn as usize >= vyska_c {
                return false;
            }
            let (xnl, ynl) = (xn << sx, yn << sx);
            self.dostupny(xl, yl, xnl, ynl)
                && (!obmedzena || self.s.mapy.intra[self.i4(xnl as usize, ynl as usize)])
        };
        // ľavý stĺpec zdola nahor: index i ↔ p[−1][2n − 1 − i]
        let xn = x as i32 - 1;
        let mut i = 0;
        while i < 2 * n {
            let yn = (y + 2 * n - 1 - i) as i32;
            let d = dost(xn, yn - (yn % jednotka as i32));
            for j in 0..jednotka.min(2 * n - i) {
                let yy = yn - j as i32;
                sus.dostupne[i + j] = d;
                if d {
                    sus.vzorky[i + j] = rov.data[yy as usize * rov.krok + xn as usize] as u16;
                }
            }
            i += jednotka;
        }
        // roh
        let d = dost(x as i32 - 1, y as i32 - 1);
        sus.dostupne[2 * n] = d;
        if d {
            sus.vzorky[2 * n] = rov.data[(y - 1) * rov.krok + x - 1] as u16;
        }
        // horný riadok zľava doprava
        let yn = y as i32 - 1;
        let mut i = 0;
        while i < 2 * n {
            let xx = (x + i) as i32;
            let d = dost(xx, yn);
            for j in 0..jednotka.min(2 * n - i) {
                sus.dostupne[2 * n + 1 + i + j] = d;
                if d {
                    sus.vzorky[2 * n + 1 + i + j] = rov.data[yn as usize * rov.krok + x + i + j] as u16;
                }
            }
            i += jednotka;
        }
        let silne = self.sps.strong_intra_smoothing;
        let hlbka = self.hlbka(c);
        let rov = &mut self.s.obraz.roviny[c];
        let krok = rov.krok;
        intra::predikuj(&sus, log2, rezim, c == 0, hlbka, silne, &mut rov.data[y * krok + x..], krok);
    }

    // -----------------------------------------------------------------------------------------------------
    // Rezíduá (7.3.8.11) a rekonštrukcia
    // -----------------------------------------------------------------------------------------------------

    fn rezidua_a_rekonstrukcia(
        &mut self,
        c: usize,
        x: usize,
        y: usize,
        log2: u32,
        ju: &Jednotka,
        rezim: u32,
    ) -> Vysledok<()> {
        let skenovanie = if ju.intra && (log2 == 2 || (log2 == 3 && c == 0)) {
            if (6..=14).contains(&rezim) {
                2
            } else if (22..=30).contains(&rezim) {
                1
            } else {
                0
            }
        } else {
            0
        };
        let (preskocenie, max_x, max_y) = self.rezidua(c, log2, skenovanie)?;
        let qp = if c == 0 {
            self.qp_y + self.qp_bd
        } else {
            let posun = if c == 1 {
                self.pps.cb_qp_offset + self.h.cb_qp_offset
            } else {
                self.pps.cr_qp_offset + self.h.cr_qp_offset
            };
            // QpBdOffsetC = 6 · (BitDepthC − 8) (8.6.1)
            let bd_c = 6 * (self.sps.bity_farby as i32 - 8);
            let qpi = (self.qp_y + posun).clamp(-bd_c, 57);
            qp_farby(qpi) + bd_c
        };
        let matica = (c + if ju.intra { 0 } else { 3 }).min(5);
        let p = ParametreTb {
            log2_n: log2,
            jas: c == 0,
            intra: ju.intra,
            qp,
            preskocenie,
            obchadzanie: self.obchadzanie,
            faktory: self.faktory.map(|f| f.faktor(log2, matica)),
            bitova_hlbka: self.hlbka(c),
        };
        let n = 1usize << log2;
        let rov = &mut self.s.obraz.roviny[c];
        let krok = rov.krok;
        transformacia::rekonstruuj(
            &mut self.koef[..n * n],
            max_x,
            max_y,
            &p,
            &mut rov.data[y * krok + x..],
            krok,
        );
        for yy in 0..=max_y {
            self.koef[yy * n..yy * n + max_x + 1].fill(0);
        }
        Ok(())
    }

    /// residual_coding(): TransCoeffLevel do `self.koef` (raster n × n); vráti (transform_skip_flag, max_x, max_y).
    fn rezidua(&mut self, c: usize, log2: u32, sken: usize) -> Vysledok<(bool, usize, usize)> {
        let n = 1usize << log2;
        let preskocenie = self.pps.transform_skip
            && !self.obchadzanie
            && log2 <= 2
            && self.bin(k::TRANSFORM_SKIP + (c > 0) as usize) == 1;
        // posledná významná poloha
        let (posun, shift) = if c == 0 {
            (3 * (log2 as usize - 2) + ((log2 as usize - 1) >> 2), (log2 as usize + 1) >> 2)
        } else {
            (15, log2 as usize - 2)
        };
        let max_pref = (log2 as usize) * 2 - 1;
        let mut pref = [0usize; 2];
        for (i, zaklad) in [k::LAST_X, k::LAST_Y].into_iter().enumerate() {
            while pref[i] < max_pref && self.bin(zaklad + posun + (pref[i] >> shift)) == 1 {
                pref[i] += 1;
            }
        }
        let mut posl = [0usize; 2];
        for i in 0..2 {
            let p = pref[i];
            posl[i] = if p > 3 {
                let nb = (p >> 1) - 1;
                (1 << nb) * (2 + (p & 1)) + self.c.obchadzanie_n(nb as u32) as usize
            } else {
                p
            };
        }
        if sken == 2 {
            posl.swap(0, 1);
        }
        let (lx, ly) = (posl[0], posl[1]);
        if lx >= n || ly >= n {
            return Err(chyba("posledná významná poloha mimo bloku"));
        }
        let log2_sb = log2 as usize - 2;
        let nsb = 1usize << log2_sb;
        let prechod_sb = &PRECHOD[log2_sb][sken];
        let prechod_4 = &PRECHOD[2][sken];
        let posledny_sb = (0..nsb * nsb)
            .find(|&i| prechod_sb[i] == ((lx >> 2) as u8, (ly >> 2) as u8))
            .ok_or_else(|| chyba("poloha podbloku"))?;
        let posledna_poz = (0..16).find(|&i| prechod_4[i] == ((lx & 3) as u8, (ly & 3) as u8)).unwrap_or(0);
        let mut csbf = [false; 64];
        let mut c1 = 1u32;
        let (mut max_x, mut max_y) = (0usize, 0usize);
        let skryvanie = self.pps.skryvanie_znamienok && !self.obchadzanie;
        for i in (0..=posledny_sb).rev() {
            let (xs, ys) = (prechod_sb[i].0 as usize, prechod_sb[i].1 as usize);
            let mut odvod_dc = false;
            if i < posledny_sb && i > 0 {
                let mut ctx = 0;
                if xs + 1 < nsb && csbf[ys * 8 + xs + 1] {
                    ctx = 1;
                }
                if ys + 1 < nsb && csbf[(ys + 1) * 8 + xs] {
                    ctx = 1;
                }
                csbf[ys * 8 + xs] = self.bin(k::CODED_SUB_BLOCK + ctx + if c > 0 { 2 } else { 0 }) == 1;
                odvod_dc = true;
            } else {
                csbf[ys * 8 + xs] = true;
            }
            if !csbf[ys * 8 + xs] {
                continue;
            }
            let predosly = (xs + 1 < nsb && csbf[ys * 8 + xs + 1]) as usize
                + 2 * (ys + 1 < nsb && csbf[(ys + 1) * 8 + xs]) as usize;
            // významné koeficienty v poradí n = 15 … 0
            let mut vyznamne = [0u8; 16];
            let mut pocet = 0usize;
            let zaciatok = if i == posledny_sb {
                vyznamne[0] = posledna_poz as u8;
                pocet = 1;
                posledna_poz as i32 - 1
            } else {
                15
            };
            let mut nn = zaciatok;
            while nn >= 0 {
                let np = nn as usize;
                let (xp, yp) = (prechod_4[np].0 as usize, prechod_4[np].1 as usize);
                let (xc, yc) = ((xs << 2) + xp, (ys << 2) + yp);
                let sig = if np > 0 || !odvod_dc {
                    let s = if log2 == 2 {
                        MAPA_SIG_4X4[(yc << 2) + xc] as usize
                    } else if xc + yc == 0 {
                        0
                    } else {
                        let mut s = match predosly {
                            0 => {
                                if xp + yp == 0 {
                                    2
                                } else if xp + yp < 3 {
                                    1
                                } else {
                                    0
                                }
                            }
                            1 => {
                                if yp == 0 {
                                    2
                                } else if yp == 1 {
                                    1
                                } else {
                                    0
                                }
                            }
                            2 => {
                                if xp == 0 {
                                    2
                                } else if xp == 1 {
                                    1
                                } else {
                                    0
                                }
                            }
                            _ => 2,
                        };
                        if c == 0 {
                            if xs + ys > 0 {
                                s += 3;
                            }
                            s += if log2 == 3 { if sken == 0 { 9 } else { 15 } } else { 21 };
                        } else {
                            s += if log2 == 3 { 9 } else { 12 };
                        }
                        s
                    };
                    let v = self.bin(k::SIG_COEFF + if c == 0 { s } else { 27 + s }) == 1;
                    if v {
                        odvod_dc = false;
                    }
                    v
                } else {
                    // DC podbloku sa odvodí
                    true
                };
                if sig {
                    vyznamne[pocet] = np as u8;
                    pocet += 1;
                }
                nn -= 1;
            }
            if pocet == 0 {
                continue;
            }
            // úrovne
            let mut ctx_sada = if i == 0 || c > 0 { 0 } else { 2 };
            if c1 == 0 {
                ctx_sada += 1;
            }
            c1 = 1;
            let mut g1 = [0u8; 16];
            let mut prvy_g1 = usize::MAX;
            for (j, g) in g1.iter_mut().enumerate().take(pocet.min(8)) {
                let ctx = ctx_sada * 4 + c1 as usize + if c > 0 { 16 } else { 0 };
                *g = self.bin(k::GT1 + ctx) as u8;
                if *g == 1 {
                    c1 = 0;
                    if prvy_g1 == usize::MAX {
                        prvy_g1 = j;
                    }
                } else if c1 > 0 && c1 < 3 {
                    c1 += 1;
                }
            }
            let mut g2 = 0u8;
            if prvy_g1 != usize::MAX {
                g2 = self.bin(k::GT2 + ctx_sada + if c > 0 { 4 } else { 0 }) as u8;
            }
            let skryte = skryvanie && (vyznamne[0] as i32 - vyznamne[pocet - 1] as i32) > 3;
            let mut znamienka = 0u32;
            let pocet_znamienok = if skryte { pocet - 1 } else { pocet };
            for _ in 0..pocet_znamienok {
                znamienka = (znamienka << 1) | self.c.obchadzanie();
            }
            if skryte {
                znamienka <<= 1;
            }
            // coeff_abs_level_remaining
            let mut rice = 0u32;
            let mut suma = 0i32;
            for j in 0..pocet {
                let zaklad = 1 + g1[j] as i32 + if j == prvy_g1 { g2 as i32 } else { 0 };
                let prah = if j < 8 { if j == prvy_g1 { 3 } else { 2 } } else { 1 };
                let mut uroven = zaklad;
                if zaklad == prah {
                    let zv = self.zvysok(rice)? as i32;
                    uroven = zaklad.saturating_add(zv);
                    if uroven > 3 * (1 << rice) {
                        rice = (rice + 1).min(4);
                    }
                }
                let np = vyznamne[j] as usize;
                let (xc, yc) = ((xs << 2) + prechod_4[np].0 as usize, (ys << 2) + prechod_4[np].1 as usize);
                let zaporne = (znamienka >> (pocet - 1 - j)) & 1 == 1;
                let mut v = if zaporne { -uroven } else { uroven };
                if skryte {
                    suma = suma.saturating_add(uroven);
                    if j == pocet - 1 && suma % 2 == 1 {
                        v = -v;
                    }
                }
                self.koef[yc * n + xc] = v.clamp(-32768, 32767) as i16;
                max_x = max_x.max(xc);
                max_y = max_y.max(yc);
            }
        }
        Ok((preskocenie, max_x, max_y))
    }

    /// coeff_abs_level_remaining (9.3.3.11) s parametrom Rice.
    fn zvysok(&mut self, rice: u32) -> Vysledok<u32> {
        let mut pref = 0u32;
        while self.c.obchadzanie() == 1 {
            pref += 1;
            if pref > 32 {
                return Err(chyba("coeff_abs_level_remaining – príliš dlhá predpona"));
            }
        }
        if pref <= 3 {
            Ok((pref << rice) + self.c.obchadzanie_n(rice))
        } else {
            let dlzka = pref - 3 + rice;
            let suffix = if dlzka > 24 {
                let hore = self.c.obchadzanie_n(dlzka - 24);
                (hore << 24) | self.c.obchadzanie_n(24)
            } else {
                self.c.obchadzanie_n(dlzka)
            };
            Ok((((1u32 << (pref - 3)) + 2) << rice).wrapping_add(suffix).min(1 << 16))
        }
    }
}
