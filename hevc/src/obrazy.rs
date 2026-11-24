//! Správa obrazov dekodéra (decoded picture buffer, DPB) – H.265, kap. 8.1.3, 8.3.1–8.3.4 a príloha C.5.2
//! (krok H1).
//!
//! Spoločná pre hardvérový dekodér *stateless* (RPi 4/400/5) aj pre vlastný softvérový dekodér: počíta POC,
//! sady referenčných obrazov (RPS) a ich označovanie, vytvára chýbajúce referencie, zostavuje zoznamy
//! referencií rezu a určuje poradie výstupu („bumping“). Obrazy sú len identifikátory – pamäť obrazov
//! (vyrovnávacie pamäte V4L2 alebo vlastné roviny) spravuje volajúci podľa udalostí [`Udalost`].

use crate::nal::{HlavickaNal, typ};
use crate::parametre::Sps;
use crate::rez::HlavickaRezu;
use orchidea_jadro::{Chyba, Vysledok};

/// Označenie obrazu ako referencie (8.3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Referencia {
    Nie,
    Kratkodoba,
    Dlhodoba,
}

/// Obraz v pamäti dekodéra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObrazDpb {
    /// identifikátor obrazu (jedinečný v rámci toku)
    pub id: u32,
    /// PicOrderCntVal
    pub poc: i32,
    pub referencia: Referencia,
    /// „needed for output“
    pub na_vystup: bool,
    /// PicLatencyCount
    pub oneskorenie: u32,
    /// vytvorený ako chýbajúca referencia (8.3.3) – nedekódoval sa, nikdy sa nevydá
    pub generovany: bool,
}

/// Udalosť pre volajúceho.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Udalost {
    /// vytvoriť chýbajúci referenčný obraz (8.3.3.2): vzorky 1 << (BitDepth − 1), vnútrosnímkový, bez pohybu
    Vytvor { id: u32, poc: i32 },
    /// vydať obraz (v poradí zobrazenia, orezaný oknom SPS)
    Vystup(u32),
    /// obraz už nie je potrebný – jeho pamäť sa smie použiť znova
    Uvolni(u32),
}

/// Sady referenčných obrazov aktuálneho obrazu (RefPicSetStCurrBefore, …, 8.3.2) ako identifikátory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SadyObrazu {
    pub st_pred: Vec<u32>,
    pub st_po: Vec<u32>,
    pub lt: Vec<u32>,
    /// obrazy ponechané pre neskoršie obrazy (StFoll a LtFoll), ktoré v pamäti sú
    pub nasledujuce: Vec<u32>,
}

/// Výsledok začiatku obrazu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZaciatokObrazu {
    /// identifikátor aktuálneho obrazu
    pub id: u32,
    /// PicOrderCntVal
    pub poc: i32,
    /// obraz RASL patriaci k IRAP s NoRaslOutputFlag = 1 – nedekóduje sa ani nevydá (8.1.3)
    pub preskocit: bool,
    /// PicOutputFlag
    pub vystup: bool,
    /// IRAP s NoRaslOutputFlag = 1
    pub irap_bez_rasl: bool,
    /// NoOutputOfPriorPicsFlag (len pri IRAP s NoRaslOutputFlag = 1, ktorý nie je prvým obrazom toku)
    pub bez_vystupu_predoslych: bool,
    /// udalosti pred dekódovaním obrazu (C.5.2.2), v poradí
    pub udalosti: Vec<Udalost>,
}

/// Správa obrazov dekodéra.
#[derive(Debug, Clone)]
pub struct SpravaObrazov {
    obrazy: Vec<ObrazDpb>,
    dalsie_id: u32,
    /// PicOrderCntVal obrazu prevTid0Pic
    prev_tid0_poc: i32,
    /// ešte nebol dekódovaný žiadny obraz
    zaciatok_toku: bool,
    /// prišla jednotka konca sekvencie (EOS) – nasledujúci IRAP má NoRaslOutputFlag = 1
    po_konci_sekvencie: bool,
    /// NoRaslOutputFlag posledného obrazu IRAP
    irap_bez_rasl: bool,
    /// aktuálny obraz: (id, POC, PicOutputFlag)
    aktualny: Option<(u32, i32, bool)>,
    /// sady referencií aktuálneho obrazu
    pub sady: SadyObrazu,
}

impl Default for SpravaObrazov {
    fn default() -> Self {
        Self::nova()
    }
}

fn je_rasl(t: u8) -> bool {
    t == typ::RASL_N || t == typ::RASL_R
}

fn je_radl(t: u8) -> bool {
    t == typ::RADL_N || t == typ::RADL_R
}

/// Neodkazovaný obraz podvrstvy (sub-layer non-reference): TRAIL_N, TSA_N, STSA_N, RADL_N, RASL_N, RSV_VCL_N1x.
fn je_slnr(t: u8) -> bool {
    t <= 14 && t.is_multiple_of(2)
}

impl SpravaObrazov {
    pub fn nova() -> SpravaObrazov {
        SpravaObrazov {
            obrazy: Vec::new(),
            dalsie_id: 0,
            prev_tid0_poc: 0,
            zaciatok_toku: true,
            po_konci_sekvencie: false,
            irap_bez_rasl: true,
            aktualny: None,
            sady: SadyObrazu::default(),
        }
    }

    /// Obraz s identifikátorom `id`, ak je v pamäti dekodéra.
    pub fn obraz(&self, id: u32) -> Option<&ObrazDpb> {
        self.obrazy.iter().find(|o| o.id == id)
    }

    /// Všetky obrazy v pamäti dekodéra (bez aktuálneho).
    pub fn obrazy(&self) -> &[ObrazDpb] {
        &self.obrazy
    }

    /// Identifikátor a POC aktuálneho obrazu.
    pub fn aktualny(&self) -> Option<(u32, i32)> {
        self.aktualny.map(|(id, poc, _)| (id, poc))
    }

    /// Jednotka konca sekvencie (end_of_seq_rbsp): nasledujúci IRAP začne novú kódovanú sekvenciu.
    pub fn koniec_sekvencie(&mut self) {
        self.po_konci_sekvencie = true;
    }

    fn nove_id(&mut self) -> u32 {
        let id = self.dalsie_id;
        self.dalsie_id = self.dalsie_id.wrapping_add(1);
        id
    }

    /// Začiatok obrazu – volá sa s hlavičkou prvého segmentu rezu obrazu: POC (8.3.1), RPS a označenie referencií
    /// (8.3.2), chýbajúce referencie (8.3.3) a odstránenie či vydanie obrazov pred dekódovaním (C.5.2.2).
    pub fn zacni_obraz(&mut self, rez: &HlavickaRezu, sps: &Sps) -> Vysledok<ZaciatokObrazu> {
        if self.aktualny.is_some() {
            return Err(Chyba::PoskodeneData("HEVC: nový obraz pred ukončením predošlého".into()));
        }
        let nal: HlavickaNal = rez.nal;
        let irap_bez_rasl = nal.je_irap()
            && (nal.je_idr()
                || (typ::BLA_W_LP..=18).contains(&nal.typ)
                || self.zaciatok_toku
                || self.po_konci_sekvencie);
        if nal.je_irap() {
            self.irap_bez_rasl = irap_bez_rasl;
        }
        if je_rasl(nal.typ) && self.irap_bez_rasl {
            return Ok(ZaciatokObrazu {
                id: u32::MAX,
                poc: 0,
                preskocit: true,
                vystup: false,
                irap_bez_rasl: false,
                bez_vystupu_predoslych: false,
                udalosti: Vec::new(),
            });
        }

        // 8.3.1 – POC
        let max_lsb = 1i32 << sps.log2_max_poc_lsb;
        let lsb = rez.poc_lsb as i32;
        let msb = if irap_bez_rasl {
            0
        } else {
            let prev_lsb = self.prev_tid0_poc & (max_lsb - 1);
            let prev_msb = self.prev_tid0_poc - prev_lsb;
            if lsb < prev_lsb && prev_lsb - lsb >= max_lsb / 2 {
                prev_msb + max_lsb
            } else if lsb > prev_lsb && lsb - prev_lsb > max_lsb / 2 {
                prev_msb - max_lsb
            } else {
                prev_msb
            }
        };
        let poc = msb.wrapping_add(lsb);
        if nal.casova_vrstva == 0 && !je_rasl(nal.typ) && !je_radl(nal.typ) && !je_slnr(nal.typ) {
            self.prev_tid0_poc = poc;
        }

        let mut udalosti = Vec::new();
        // 8.3.2 – RPS a označenie referencií
        if irap_bez_rasl {
            for o in &mut self.obrazy {
                o.referencia = Referencia::Nie;
            }
        }
        let mut poc_st_pred = Vec::new();
        let mut poc_st_po = Vec::new();
        let mut poc_st_nasl = Vec::new();
        for &(d, pouzity) in &rez.sada.dozadu {
            if pouzity { &mut poc_st_pred } else { &mut poc_st_nasl }.push(poc.wrapping_add(d));
        }
        for &(d, pouzity) in &rez.sada.dopredu {
            if pouzity { &mut poc_st_po } else { &mut poc_st_nasl }.push(poc.wrapping_add(d));
        }
        // dlhodobé: (POC alebo jeho lsb, či je známe celé POC)
        let mut lt_akt = Vec::new();
        let mut lt_nasl = Vec::new();
        for lt in &rez.dlhodobe {
            let mut p = lt.poc_lsb as i32;
            if let Some(cyklus) = lt.msb_cyklus {
                p = p
                    .wrapping_add(poc)
                    .wrapping_sub((cyklus as i32).wrapping_mul(max_lsb))
                    .wrapping_sub(poc & (max_lsb - 1));
            }
            if lt.pouzity { &mut lt_akt } else { &mut lt_nasl }.push((p, lt.msb_cyklus.is_some()));
        }
        let najdi_lt = |obrazy: &[ObrazDpb], (p, cele): (i32, bool)| -> Option<usize> {
            obrazy.iter().position(|o| {
                o.referencia != Referencia::Nie && if cele { o.poc == p } else { o.poc & (max_lsb - 1) == p }
            })
        };
        let mut v_sade = vec![false; self.obrazy.len()];
        let mut lt_akt_idx = Vec::new();
        for &l in &lt_akt {
            let i = najdi_lt(&self.obrazy, l);
            if let Some(i) = i {
                v_sade[i] = true;
            }
            lt_akt_idx.push(i);
        }
        let mut lt_nasl_idx = Vec::new();
        for &l in &lt_nasl {
            let i = najdi_lt(&self.obrazy, l);
            if let Some(i) = i {
                v_sade[i] = true;
            }
            lt_nasl_idx.push(i);
        }
        for i in lt_akt_idx.iter().chain(&lt_nasl_idx).flatten() {
            self.obrazy[*i].referencia = Referencia::Dlhodoba;
        }
        let najdi_st = |obrazy: &[ObrazDpb], p: i32| -> Option<usize> {
            obrazy.iter().position(|o| o.referencia == Referencia::Kratkodoba && o.poc == p)
        };
        let mut st_idx: [Vec<Option<usize>>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for (k, zoznam) in [&poc_st_pred, &poc_st_po, &poc_st_nasl].into_iter().enumerate() {
            for &p in zoznam {
                let i = najdi_st(&self.obrazy, p);
                if let Some(i) = i {
                    v_sade[i] = true;
                }
                st_idx[k].push(i);
            }
        }
        for (i, o) in self.obrazy.iter_mut().enumerate() {
            if !v_sade[i] {
                o.referencia = Referencia::Nie;
            }
        }

        // identifikátory nájdených obrazov (indexy sa pri odstraňovaní menia)
        let id_z = |obrazy: &[ObrazDpb], v: &[Option<usize>]| -> Vec<Option<u32>> {
            v.iter().map(|i| i.map(|i| obrazy[i].id)).collect()
        };
        let st_id =
            [id_z(&self.obrazy, &st_idx[0]), id_z(&self.obrazy, &st_idx[1]), id_z(&self.obrazy, &st_idx[2])];
        let lt_akt_id = id_z(&self.obrazy, &lt_akt_idx);
        let lt_nasl_id = id_z(&self.obrazy, &lt_nasl_idx);

        // C.5.2.2 – odstránenie a vydanie obrazov pred dekódovaním aktuálneho obrazu
        let podvrstva = sps.podvrstvy.last().copied().unwrap_or_default();
        let mut bez_vystupu_predoslych = false;
        if irap_bez_rasl && !self.zaciatok_toku {
            // CRA s NoRaslOutputFlag = 1 nevydá predošlé obrazy nikdy (C.5.2.2)
            bez_vystupu_predoslych = nal.typ == typ::CRA_NUT || rez.bez_vystupu_predoslych;
            if bez_vystupu_predoslych {
                for o in self.obrazy.drain(..) {
                    udalosti.push(Udalost::Uvolni(o.id));
                }
            } else {
                self.odstran_nepotrebne(&mut udalosti);
                while self.obrazy.iter().any(|o| o.na_vystup) {
                    self.vydaj(&mut udalosti);
                }
                for o in self.obrazy.drain(..) {
                    udalosti.push(Udalost::Uvolni(o.id));
                }
            }
        } else {
            self.odstran_nepotrebne(&mut udalosti);
            loop {
                let na_vystup = self.obrazy.iter().filter(|o| o.na_vystup).count() as u32;
                let plne = self.obrazy.len() as u32 > podvrstva.max_dec_pic_buffering_minus1;
                if (na_vystup > podvrstva.max_num_reorder_pics || self.oneskorenie_prekrocene(sps) || plne)
                    && na_vystup > 0
                {
                    self.vydaj(&mut udalosti);
                } else {
                    break;
                }
            }
        }

        // 8.3.3 – chýbajúce referencie: pri IRAP s NoRaslOutputFlag = 1 pre sady Foll, inak (poškodený tok
        // alebo chýbajúci začiatok) aj pre sady Curr – obraz sa tak dá dekódovať a chyba sa nerozšíri
        let mut sady = SadyObrazu::default();
        let vytvor =
            |sprava: &mut SpravaObrazov, p: i32, r: Referencia, udalosti: &mut Vec<Udalost>| -> u32 {
                let id = sprava.nove_id();
                sprava.obrazy.push(ObrazDpb {
                    id,
                    poc: p,
                    referencia: r,
                    na_vystup: false,
                    oneskorenie: 0,
                    generovany: true,
                });
                udalosti.push(Udalost::Vytvor { id, poc: p });
                id
            };
        for (k, (ciel, poc_zoznam)) in
            [(&mut sady.st_pred, &poc_st_pred), (&mut sady.st_po, &poc_st_po)].into_iter().enumerate()
        {
            for (j, i) in st_id[k].iter().enumerate() {
                let id = match i {
                    Some(id) => *id,
                    None => vytvor(self, poc_zoznam[j], Referencia::Kratkodoba, &mut udalosti),
                };
                ciel.push(id);
            }
        }
        for (j, i) in lt_akt_id.iter().enumerate() {
            let id = match i {
                Some(id) => *id,
                None => vytvor(self, lt_akt[j].0, Referencia::Dlhodoba, &mut udalosti),
            };
            sady.lt.push(id);
        }
        for (j, i) in st_id[2].iter().enumerate() {
            match i {
                Some(id) => sady.nasledujuce.push(*id),
                None if irap_bez_rasl => {
                    let id = vytvor(self, poc_st_nasl[j], Referencia::Kratkodoba, &mut udalosti);
                    sady.nasledujuce.push(id);
                }
                None => {}
            }
        }
        for (j, i) in lt_nasl_id.iter().enumerate() {
            match i {
                Some(id) => sady.nasledujuce.push(*id),
                None if irap_bez_rasl => {
                    let id = vytvor(self, lt_nasl[j].0, Referencia::Dlhodoba, &mut udalosti);
                    sady.nasledujuce.push(id);
                }
                None => {}
            }
        }
        self.sady = sady;
        self.zaciatok_toku = false;
        self.po_konci_sekvencie = false;

        let vystup = rez.vystup && !(je_rasl(nal.typ) && self.irap_bez_rasl);
        let id = self.nove_id();
        self.aktualny = Some((id, poc, vystup));
        Ok(ZaciatokObrazu {
            id,
            poc,
            preskocit: false,
            vystup,
            irap_bez_rasl,
            bez_vystupu_predoslych,
            udalosti,
        })
    }

    fn oneskorenie_prekrocene(&self, sps: &Sps) -> bool {
        let p = sps.podvrstvy.last().copied().unwrap_or_default();
        if p.max_latency_increase_plus1 == 0 {
            return false;
        }
        let max = p.max_num_reorder_pics + p.max_latency_increase_plus1 - 1;
        self.obrazy.iter().any(|o| o.na_vystup && o.oneskorenie >= max)
    }

    /// Odstráni obrazy, ktoré nie sú na výstup ani referenciou.
    fn odstran_nepotrebne(&mut self, udalosti: &mut Vec<Udalost>) {
        self.obrazy.retain(|o| {
            let ponechat = o.na_vystup || o.referencia != Referencia::Nie;
            if !ponechat {
                udalosti.push(Udalost::Uvolni(o.id));
            }
            ponechat
        });
    }

    /// „Bumping“ (C.5.2.4): vydá obraz s najmenším POC spomedzi obrazov na výstup.
    fn vydaj(&mut self, udalosti: &mut Vec<Udalost>) {
        let Some(i) = self
            .obrazy
            .iter()
            .enumerate()
            .filter(|(_, o)| o.na_vystup)
            .min_by_key(|(_, o)| o.poc)
            .map(|x| x.0)
        else {
            return;
        };
        self.obrazy[i].na_vystup = false;
        udalosti.push(Udalost::Vystup(self.obrazy[i].id));
        if self.obrazy[i].referencia == Referencia::Nie {
            let o = self.obrazy.remove(i);
            udalosti.push(Udalost::Uvolni(o.id));
        }
    }

    /// Zoznamy referencií RefPicList0 a RefPicList1 rezu (8.3.4) ako identifikátory obrazov.
    pub fn zoznamy(&self, rez: &HlavickaRezu) -> Vysledok<[Vec<u32>; 2]> {
        let mut vysledok = [Vec::new(), Vec::new()];
        let s = &self.sady;
        let spolu = s.st_pred.len() + s.st_po.len() + s.lt.len();
        for (lx, ciel) in vysledok.iter_mut().enumerate().take(rez.zoznamov()) {
            if spolu == 0 {
                return Err(Chyba::PoskodeneData("HEVC: rez P/B bez referencií".into()));
            }
            let aktivne = rez.aktivne[lx] as usize;
            let poradie: [&[u32]; 3] =
                if lx == 0 { [&s.st_pred, &s.st_po, &s.lt] } else { [&s.st_po, &s.st_pred, &s.lt] };
            let dlzka = aktivne.max(spolu);
            let mut docasny = Vec::with_capacity(dlzka);
            while docasny.len() < dlzka {
                for cast in poradie {
                    for &id in cast {
                        if docasny.len() < dlzka {
                            docasny.push(id);
                        }
                    }
                }
            }
            *ciel = match &rez.uprava_zoznamu[lx] {
                Some(u) => u.iter().map(|&e| docasny[e as usize]).collect(),
                None => docasny[..aktivne].to_vec(),
            };
        }
        Ok(vysledok)
    }

    /// Je obraz dlhodobou referenciou (pre aktuálny obraz)?
    pub fn je_dlhodoby(&self, id: u32) -> bool {
        self.obraz(id).is_some_and(|o| o.referencia == Referencia::Dlhodoba)
    }

    /// Koniec dekódovania aktuálneho obrazu (C.5.2.3): obraz ide do pamäte ako krátkodobá referencia, prípadne
    /// na výstup, a vydajú sa obrazy podľa počtu na preusporiadanie a oneskorenia.
    pub fn ukonci_obraz(&mut self, sps: &Sps) -> Vec<Udalost> {
        let mut udalosti = Vec::new();
        let Some((id, poc, vystup)) = self.aktualny.take() else { return udalosti };
        if vystup {
            for o in &mut self.obrazy {
                if o.na_vystup && o.poc > poc {
                    o.oneskorenie += 1;
                }
            }
        }
        self.obrazy.push(ObrazDpb {
            id,
            poc,
            referencia: Referencia::Kratkodoba,
            na_vystup: vystup,
            oneskorenie: 0,
            generovany: false,
        });
        let p = sps.podvrstvy.last().copied().unwrap_or_default();
        loop {
            let na_vystup = self.obrazy.iter().filter(|o| o.na_vystup).count() as u32;
            if na_vystup > 0 && (na_vystup > p.max_num_reorder_pics || self.oneskorenie_prekrocene(sps)) {
                self.vydaj(&mut udalosti);
            } else {
                break;
            }
        }
        udalosti
    }

    /// Koniec toku: vydá všetky zostávajúce obrazy v poradí POC a uvoľní pamäť.
    pub fn vyprazdni(&mut self) -> Vec<Udalost> {
        let mut udalosti = Vec::new();
        while self.obrazy.iter().any(|o| o.na_vystup) {
            self.vydaj(&mut udalosti);
        }
        for o in self.obrazy.drain(..) {
            udalosti.push(Udalost::Uvolni(o.id));
        }
        udalosti
    }
}
