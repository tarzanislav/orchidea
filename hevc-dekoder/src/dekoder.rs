//! Dekodér toku HEVC Main 8-bit: jednotky NAL → obrazy v poradí zobrazenia. Správa obrazov je spoločná
//! s hardvérovou cestou (`orchidea_hevc::SpravaObrazov`); každý obraz sa overí odtlačkom MD5 zo SEI, ak ho tok má.

use crate::filtre::RezFiltra;
use crate::geometria::Geometria;
use crate::obraz::{ObrazPasov, Zdroj};
use crate::syntax::{DekoderRezu, DekodovanyObraz, ReferencieRezu, StavObrazu};
use crate::transformacia::FaktorySkalovania;
use orchidea_hevc::nal::typ;
use orchidea_hevc::obrazy::Udalost;
use orchidea_hevc::odtlacok::{crc_roviny, kontrolny_sucet_roviny, md5_roviny};
use orchidea_hevc::sei::{self, OdtlacokObrazu};
use orchidea_hevc::{HlavickaNal, HlavickaRezu, Pps, Sady, SpravaObrazov, Sps, rbsp};
use orchidea_jadro::{Chyba, Vysledok};
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

/// Výsledky overenia bitovej zhody.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Statistika {
    /// dekódované obrazy
    pub obrazov: usize,
    /// obrazy s odtlačkom MD5 v SEI, ktorý sa zhoduje
    pub zhoda: usize,
    /// obrazy, ktorých odtlačok sa nezhoduje (poradie v dekódovaní, POC)
    pub nezhoda: Vec<(usize, i32)>,
    /// obrazy bez odtlačku MD5
    pub bez_odtlacku: usize,
    /// chyby dekódovania (poškodený tok, nepodporovaná vlastnosť)
    pub chyby: Vec<String>,
    /// preskočené obrazy RASL po náhodnom prístupe
    pub preskocene: usize,
    /// opis rezov prvých obrazov s nezhodou (ladenie na diaľku – záznam CI)
    pub opis_nezhody: Vec<String>,
}

/// Segment rezu pripravený na dekódovanie: hlavička, dáta (RBSP) a referencie.
struct SegmentUlohy {
    rez: HlavickaRezu,
    data: Vec<u8>,
    refs: ReferencieRezu,
    kol: Option<Arc<DekodovanyObraz>>,
}

/// Rozbieraný obraz (hlavné vlákno zbiera segmenty rezov, kým obraz neskončí).
struct Aktualny {
    ciel: Arc<DekodovanyObraz>,
    sps: Arc<Sps>,
    pps: Arc<Pps>,
    geometria: Arc<Geometria>,
    faktory: Option<Arc<FaktorySkalovania>>,
    odtlacky: Vec<OdtlacokObrazu>,
    poradie: usize,
    /// stručný opis segmentov rezov (pre ladenie)
    opis: Vec<String>,
    segmenty: Vec<SegmentUlohy>,
}

/// Dekódovanie jedného obrazu – beží v pracovnom vlákne (pri jednom vlákne priamo).
struct Uloha {
    ciel: Arc<DekodovanyObraz>,
    segmenty: Vec<SegmentUlohy>,
    sps: Arc<Sps>,
    pps: Arc<Pps>,
    geometria: Arc<Geometria>,
    faktory: Option<Arc<FaktorySkalovania>>,
    odtlacky: Vec<OdtlacokObrazu>,
    poradie: usize,
    opis: Vec<String>,
    overovat: bool,
}

/// Spoločné pre hlavné a pracovné vlákna: štatistika a počet dokončených úloh.
#[derive(Default)]
struct Spolocne {
    statistika: Mutex<Statistika>,
    dokoncene: Mutex<usize>,
    zmena: Condvar,
}

fn zamkni<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Pracovné vlákna dekódovania obrazov (súbežnosť po obrazoch).
struct Vlakna {
    odoslat: Option<SyncSender<Uloha>>,
    vlakna: Vec<JoinHandle<()>>,
}

impl Drop for Vlakna {
    fn drop(&mut self) {
        self.odoslat = None;
        for v in self.vlakna.drain(..) {
            let _ = v.join();
        }
    }
}

/// Softvérový dekodér HEVC.
///
/// **Súbežnosť po obrazoch:** hlavné vlákno rozoberá jednotky NAL, hlavičky rezov a spravuje pamäť obrazov;
/// každý obraz sa dekóduje v pracovnom vlákne. Obraz sa zverejňuje po riadkoch CTB (aj s filtrami v slučke)
/// a ďalší obraz, ktorý ho má ako referenciu, môže bežať súčasne – čaká len na riadky, ktoré naozaj číta.
/// Úlohy sa berú v poradí dekódovania a každá čaká len na skoršie, takže najstaršia rozpracovaná úloha vždy
/// pokračuje (nikdy sa nezasekne). Výsledok je bitovo rovnaký pri ľubovoľnom počte vlákien.
#[allow(missing_debug_implementations)]
pub struct Dekoder {
    sady: Sady,
    sprava: SpravaObrazov,
    dpb: HashMap<u32, Arc<DekodovanyObraz>>,
    aktualny: Option<Aktualny>,
    nezavisla: Option<HlavickaRezu>,
    cakajuce_odtlacky: Vec<OdtlacokObrazu>,
    preskakuje: bool,
    vystup: Vec<Arc<DekodovanyObraz>>,
    geometria: Option<(Pps, Sps, Arc<Geometria>)>,
    faktory: Option<(Pps, Sps, Arc<FaktorySkalovania>)>,
    /// stav obrazu na opätovné použitie pri dekódovaní v tomto vlákne (chladný systém – bez alokácií na obraz)
    rezerva_stavu: Option<StavObrazu>,
    spolocne: Arc<Spolocne>,
    vlakien: usize,
    vlakna: Option<Vlakna>,
    odoslane: usize,
    /// overovať odtlačky MD5 zo SEI (skúšky); pri prehrávaní sa vypne a ušetrí sa práca
    pub overovat_odtlacky: bool,
    /// výsledky overenia – úplné po [`Dekoder::koniec`]
    pub statistika: Statistika,
}

impl Default for Dekoder {
    fn default() -> Self {
        Self::novy()
    }
}

fn nepodporovane(t: impl Into<String>) -> Chyba {
    Chyba::MimoRozsahu(format!("HEVC dekodér: mimo profilu Main 8-bit: {}", t.into()))
}

/// Overí, že tok používa len nástroje profilu Main 8-bit 4:2:0.
fn over_profil(sps: &Sps, pps: &Pps) -> Vysledok<()> {
    if sps.chroma_format_idc != 1 {
        return Err(nepodporovane(format!("chroma_format_idc = {}", sps.chroma_format_idc)));
    }
    if sps.bity_jasu != 8 || sps.bity_farby != 8 {
        return Err(nepodporovane(format!("bitová hĺbka {}/{}", sps.bity_jasu, sps.bity_farby)));
    }
    let r = &sps.rozsirenie;
    if r.transform_skip_rotation
        || r.transform_skip_context
        || r.implicit_rdpcm
        || r.explicit_rdpcm
        || r.extended_precision_processing
        || r.intra_smoothing_disabled
        || r.high_precision_offsets
        || r.persistent_rice_adaptation
        || r.cabac_bypass_alignment
    {
        return Err(nepodporovane("nástroje rozšírenia rozsahu (RExt) v SPS"));
    }
    if let Some(r) = &pps.rozsirenie
        && (r.log2_max_transform_skip > 2
            || r.cross_component_prediction
            || r.chroma_qp_offset.is_some()
            || r.log2_sao_offset_scale_luma != 0
            || r.log2_sao_offset_scale_chroma != 0)
    {
        return Err(nepodporovane("nástroje rozšírenia rozsahu (RExt) v PPS"));
    }
    Ok(())
}

impl Dekoder {
    pub fn novy() -> Dekoder {
        Dekoder {
            sady: Sady::nove(),
            sprava: SpravaObrazov::nova(),
            dpb: HashMap::new(),
            aktualny: None,
            nezavisla: None,
            cakajuce_odtlacky: Vec::new(),
            preskakuje: false,
            vystup: Vec::new(),
            geometria: None,
            faktory: None,
            rezerva_stavu: None,
            spolocne: Arc::new(Spolocne::default()),
            vlakien: std::thread::available_parallelism().map_or(1, |n| n.get()).min(8),
            vlakna: None,
            odoslane: 0,
            overovat_odtlacky: true,
            statistika: Statistika::default(),
        }
    }

    /// Dekodér na prehrávanie: bez overovania odtlačkov MD5.
    pub fn na_prehravanie() -> Dekoder {
        Dekoder { overovat_odtlacky: false, ..Self::novy() }
    }

    /// Počet vlákien dekódovania (predvolene počet jadier, najviac 8; 1 = všetko v hlavnom vlákne).
    pub fn s_vlaknami(mut self, n: usize) -> Dekoder {
        self.vlakien = n.max(1);
        self
    }

    /// Spracuje jednotku NAL (bez štartovacieho kódu) a vráti obrazy pripravené na výstup (v poradí zobrazenia).
    /// Vrátený obraz sa môže ešte dekódovať – čítanie jeho riadkov počká, kým nie sú hotové.
    pub fn nal(&mut self, nal: &[u8]) -> Vec<Arc<DekodovanyObraz>> {
        if let Err(e) = self.nal_vnutorne(nal) {
            zamkni(&self.spolocne.statistika).chyby.push(e.to_string());
        }
        std::mem::take(&mut self.vystup)
    }

    /// Koniec toku: dokončí posledný obraz, počká na všetky rozpracované a vydá všetky zostávajúce.
    pub fn koniec(&mut self) -> Vec<Arc<DekodovanyObraz>> {
        self.dokonci_obraz();
        let u = self.sprava.vyprazdni();
        self.udalosti(&u, (0, 0));
        self.cakaj_na_vsetky();
        self.statistika = zamkni(&self.spolocne.statistika).clone();
        std::mem::take(&mut self.vystup)
    }

    fn cakaj_na_vsetky(&self) {
        let mut n = zamkni(&self.spolocne.dokoncene);
        while *n < self.odoslane {
            n = self.spolocne.zmena.wait(n).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Odovzdá obraz na dekódovanie: pracovnému vláknu, alebo (jedno vlákno, chyba vlákien) dekóduje hneď.
    fn odosli(&mut self, u: Uloha) {
        self.odoslane += 1;
        if self.vlakien > 1 && self.vlakna.is_none() {
            self.vlakna = spusti_vlakna(self.vlakien, &self.spolocne);
        }
        let u = match self.vlakna.as_ref().and_then(|v| v.odoslat.as_ref()) {
            Some(o) => match o.send(u) {
                Ok(()) => return,
                Err(e) => e.0,
            },
            None => u,
        };
        vykonaj(u, &mut self.rezerva_stavu, &self.spolocne);
    }

    fn nal_vnutorne(&mut self, nal: &[u8]) -> Vysledok<()> {
        let h = HlavickaNal::citaj(nal)?;
        if h.vrstva != 0 {
            return Ok(());
        }
        match h.typ {
            typ::VPS | typ::SPS | typ::PPS => {
                self.sady.pridaj(&h, &rbsp(&nal[2..]))?;
            }
            typ::PREFIX_SEI | typ::SUFFIX_SEI => {
                let r = rbsp(&nal[2..]);
                for (t, obsah) in sei::spravy(&r)? {
                    if t == sei::ODTLACOK_OBRAZU
                        && let Ok(o) = OdtlacokObrazu::citaj(obsah, 3)
                    {
                        match (h.typ, &mut self.aktualny) {
                            (typ::SUFFIX_SEI, Some(a)) => a.odtlacky.push(o),
                            (typ::SUFFIX_SEI, None) => {}
                            _ => self.cakajuce_odtlacky.push(o),
                        }
                    }
                }
            }
            typ::EOS => {
                self.dokonci_obraz();
                self.sprava.koniec_sekvencie();
            }
            typ::AUD | typ::EOB => self.dokonci_obraz(),
            t if t <= 9 || (16..=21).contains(&t) => self.rez(h, nal)?,
            _ => {}
        }
        Ok(())
    }

    fn rez(&mut self, h: HlavickaNal, nal: &[u8]) -> Vysledok<()> {
        let data = rbsp(&nal[2..]);
        let prvy = data.first().is_some_and(|b| b & 0x80 != 0);
        if prvy {
            self.dokonci_obraz();
        }
        let rez = HlavickaRezu::citaj(&data, h, &self.sady, self.nezavisla.as_ref())?;
        if !rez.zavisly {
            self.nezavisla = Some(rez.clone());
        }
        if prvy {
            let (sps, pps) = self.sady.pre_rez(rez.pps_id)?;
            let (sps, pps) = (sps.clone(), pps.clone());
            let z = self.sprava.zacni_obraz(&rez, &sps)?;
            let rozmery = (sps.sirka as usize, sps.vyska as usize);
            self.udalosti(&z.udalosti, rozmery);
            self.preskakuje = z.preskocit;
            if z.preskocit {
                zamkni(&self.spolocne.statistika).preskocene += 1;
                self.cakajuce_odtlacky.clear();
                return Ok(());
            }
            let (sx, sy) = sps.delenie_farby();
            let okno = [
                (sps.okno[0] * sx) as usize,
                (sps.okno[1] * sx) as usize,
                (sps.okno[2] * sy) as usize,
                (sps.okno[3] * sy) as usize,
            ];
            // obraz je v pamäti dekodéra hneď – ďalšie obrazy ho môžu mať ako referenciu už počas dekódovania
            let ciel = Arc::new(DekodovanyObraz::novy(z.id, z.poc, rozmery.0, rozmery.1, sps.log2_ctb, okno));
            self.dpb.insert(z.id, ciel.clone());
            if let Err(e) = over_profil(&sps, &pps) {
                // obraz sa nedekóduje – v pamäti ostane sivý, aby referencie ostali konzistentné
                self.preskakuje = true;
                ciel.dopln();
                let u = self.sprava.ukonci_obraz(&sps);
                self.udalosti(&u, rozmery);
                return Err(e);
            }
            let geometria = self.geometria_pre(&sps, &pps);
            let faktory = self.faktory_pre(&sps, &pps);
            let poradie = {
                let mut s = zamkni(&self.spolocne.statistika);
                s.obrazov += 1;
                s.obrazov - 1
            };
            self.aktualny = Some(Aktualny {
                ciel,
                sps: Arc::new(sps),
                pps: Arc::new(pps),
                geometria,
                faktory,
                odtlacky: std::mem::take(&mut self.cakajuce_odtlacky),
                poradie,
                opis: Vec::new(),
                segmenty: Vec::new(),
            });
        }
        if self.preskakuje {
            return Ok(());
        }
        let Some(a) = self.aktualny.as_mut() else {
            return Err(Chyba::PoskodeneData("HEVC dekodér: segment rezu bez začiatku obrazu".into()));
        };
        if rez.pps_id != a.pps.id {
            return Err(Chyba::PoskodeneData("HEVC dekodér: rez obrazu s iným PPS".into()));
        }
        // zoznamy referencií
        let zoznamy = self.sprava.zoznamy(&rez)?;
        let mut refs = ReferencieRezu::default();
        for (lx, zoznam) in zoznamy.iter().enumerate() {
            for &id in zoznam {
                let o =
                    self.dpb.get(&id).ok_or_else(|| Chyba::Chybajuce(format!("HEVC: referencia {id}")))?;
                refs.poc[lx].push(o.poc);
                refs.lt[lx].push(self.sprava.je_dlhodoby(id));
                refs.obrazy[lx].push(o.clone());
            }
        }
        let kol = if rez.tmvp && rez.typ != orchidea_hevc::TypRezu::I {
            let lx = if rez.typ == orchidea_hevc::TypRezu::B && !rez.kolokovany_z_l0 { 1 } else { 0 };
            refs.obrazy[lx].get(rez.kolokovany_ref as usize).cloned()
        } else {
            None
        };
        if a.opis.len() < 12 {
            a.opis.push(format!(
                "{:?}{} nal {} adr {} qp {} ref {:?} tmvp {} sao {}{} dbk {}{} cez {} body {} vahy {} merge {}",
                rez.typ,
                if rez.zavisly { "z" } else { "" },
                rez.nal.typ,
                rez.adresa,
                rez.qp,
                rez.aktivne,
                rez.tmvp as u8,
                rez.sao_jas as u8,
                rez.sao_farba as u8,
                rez.odblokovanie_vypnute as u8,
                if rez.beta_div2 != 0 || rez.tc_div2 != 0 { format!("({},{})", rez.beta_div2, rez.tc_div2) } else { String::new() },
                rez.filter_cez_rezy as u8,
                rez.vstupne_body.len(),
                rez.vahy.is_some() as u8,
                rez.max_zlucenie,
            ));
        }
        a.segmenty.push(SegmentUlohy { rez, data, refs, kol });
        Ok(())
    }

    fn geometria_pre(&mut self, sps: &Sps, pps: &Pps) -> Arc<Geometria> {
        if let Some((p, s, g)) = &self.geometria
            && p == pps
            && s == sps
        {
            return g.clone();
        }
        let g = Arc::new(Geometria::nova(sps, pps));
        self.geometria = Some((pps.clone(), sps.clone(), g.clone()));
        g
    }

    fn faktory_pre(&mut self, sps: &Sps, pps: &Pps) -> Option<Arc<FaktorySkalovania>> {
        sps.matice.as_ref()?;
        let m = pps.matice.as_ref().or(sps.matice.as_ref())?;
        if let Some((p, s, f)) = &self.faktory
            && p == pps
            && s == sps
        {
            return Some(f.clone());
        }
        let f = Arc::new(FaktorySkalovania::z_matic(m));
        self.faktory = Some((pps.clone(), sps.clone(), f.clone()));
        Some(f)
    }

    fn udalosti(&mut self, udalosti: &[Udalost], rozmery: (usize, usize)) {
        for u in udalosti {
            match *u {
                Udalost::Vytvor { id, poc } => {
                    // chýbajúci referenčný obraz (8.3.3.2): stredná hodnota vzoriek, bez pohybu
                    let o = DekodovanyObraz::novy(id, poc, rozmery.0, rozmery.1, 6, [0; 4]);
                    o.dopln();
                    self.dpb.insert(id, Arc::new(o));
                }
                Udalost::Vystup(id) => {
                    if let Some(o) = self.dpb.get(&id) {
                        self.vystup.push(o.clone());
                    }
                }
                Udalost::Uvolni(id) => {
                    self.dpb.remove(&id);
                }
            }
        }
    }

    /// Koniec obrazu: odovzdá ho na dekódovanie (filtre v slučke a odtlačok sú súčasťou úlohy) a pokračuje
    /// výstupom podľa C.5.2.3.
    fn dokonci_obraz(&mut self) {
        self.nezavisla = None;
        if self.preskakuje {
            self.preskakuje = false;
            return;
        }
        let Some(a) = self.aktualny.take() else { return };
        let sps = a.sps.clone();
        self.odosli(Uloha {
            ciel: a.ciel,
            segmenty: a.segmenty,
            sps: a.sps,
            pps: a.pps,
            geometria: a.geometria,
            faktory: a.faktory,
            odtlacky: a.odtlacky,
            poradie: a.poradie,
            opis: a.opis,
            overovat: self.overovat_odtlacky,
        });
        let u = self.sprava.ukonci_obraz(&sps);
        self.udalosti(&u, (0, 0));
    }
}

fn spusti_vlakna(n: usize, spolocne: &Arc<Spolocne>) -> Option<Vlakna> {
    let (odoslat, prijat) = sync_channel::<Uloha>(n);
    let prijat: Arc<Mutex<Receiver<Uloha>>> = Arc::new(Mutex::new(prijat));
    let mut vlakna = Vec::with_capacity(n);
    for i in 0..n {
        let (prijat, spolocne) = (prijat.clone(), spolocne.clone());
        let v = std::thread::Builder::new().name(format!("hevc-{i}")).spawn(move || {
            let mut rezerva = None;
            loop {
                // úlohy v poradí dekódovania – každá čaká len na skoršie, ktoré už niekto dekóduje
                let u = zamkni(&prijat).recv();
                match u {
                    Ok(u) => vykonaj(u, &mut rezerva, &spolocne),
                    Err(_) => break,
                }
            }
        });
        match v {
            Ok(v) => vlakna.push(v),
            Err(_) => break,
        }
    }
    (!vlakna.is_empty()).then(|| Vlakna { odoslat: Some(odoslat), vlakna })
}

/// Zaručí zverejnenie celého obrazu a započítanie úlohy aj pri chybe či panike – nikto nesmie čakať naveky.
struct Strazca<'a> {
    ciel: &'a DekodovanyObraz,
    spolocne: &'a Spolocne,
}

impl Drop for Strazca<'_> {
    fn drop(&mut self) {
        self.ciel.dopln();
        *zamkni(&self.spolocne.dokoncene) += 1;
        self.spolocne.zmena.notify_all();
    }
}

/// Dekódovanie obrazu (všetky segmenty rezov, filtre, zverejnenie) a overenie odtlačku.
fn vykonaj(u: Uloha, rezerva: &mut Option<StavObrazu>, spolocne: &Spolocne) {
    let strazca = Strazca { ciel: &u.ciel, spolocne };
    let chyby = match std::panic::catch_unwind(AssertUnwindSafe(|| dekoduj_obraz(&u, rezerva))) {
        Ok(chyby) => chyby,
        Err(_) => {
            *rezerva = None;
            vec![format!("HEVC dekodér: vnútorná chyba pri dekódovaní obrazu POC {}", u.ciel.poc)]
        }
    };
    u.ciel.dopln();
    // odtlačok obrazu (D.3.19): MD5, CRC alebo kontrolný súčet každej zložky
    let zhoda = if u.overovat { u.odtlacky.first().map(|o| over_odtlacok(o, &u.ciel.obraz)) } else { None };
    let mut s = zamkni(&spolocne.statistika);
    s.chyby.extend(chyby);
    match zhoda {
        Some(true) => s.zhoda += 1,
        Some(false) => {
            s.nezhoda.push((u.poradie, u.ciel.poc));
            if s.opis_nezhody.len() < 2 {
                s.opis_nezhody.push(opis_obrazu(&u));
            }
        }
        None => s.bez_odtlacku += 1,
    }
    drop(s);
    drop(strazca);
}

fn dekoduj_obraz(u: &Uloha, rezerva: &mut Option<StavObrazu>) -> Vec<String> {
    let g = &*u.geometria;
    let mut stav = match rezerva.take() {
        Some(mut s) => {
            if s.obnov(g, u.ciel.poc) {
                s
            } else {
                StavObrazu::novy(g, u.ciel.poc)
            }
        }
        None => StavObrazu::novy(g, u.ciel.poc),
    };
    stav.info.cb_qp_offset = u.pps.cb_qp_offset;
    stav.info.cr_qp_offset = u.pps.cr_qp_offset;
    stav.info.cez_dlazdice = u.pps.dlazdice.as_ref().is_none_or(|d| d.filter_cez_dlazdice);
    stav.ciel = Some(u.ciel.clone());
    stav.po_riadkoch = u.pps.dlazdice.is_none();
    let mut chyby = Vec::new();
    for (i, s) in u.segmenty.iter().enumerate() {
        stav.info.rezy.push(RezFiltra {
            adresa: s.rez.adresa_rezu,
            cez_rezy: s.rez.filter_cez_rezy,
            beta_div2: s.rez.beta_div2 as i8,
            tc_div2: s.rez.tc_div2 as i8,
        });
        let mut d = DekoderRezu::novy(
            &s.data,
            &mut stav,
            g,
            &u.sps,
            &u.pps,
            &s.rez,
            &s.refs,
            s.kol.as_ref().map(|k| &k.kol),
            u.faktory.as_deref(),
            i as u16,
        );
        if let Err(e) = d.dekoduj() {
            chyby.push(e.to_string());
        }
    }
    stav.dokonci();
    stav.ciel = None;
    *rezerva = Some(stav);
    chyby
}

fn over_odtlacok(o: &OdtlacokObrazu, obraz: &ObrazPasov) -> bool {
    (0..3).all(|c| {
        let r = &obraz.roviny[c];
        // pásy majú krok = šírka, za sebou dajú súvislú rovinu
        let mut d = Vec::with_capacity(r.sirka * r.vyska);
        for i in 0..r.pocet_pasov() {
            d.extend_from_slice(r.pas(i));
        }
        let (w, h) = (r.sirka, r.vyska);
        match o {
            OdtlacokObrazu::Md5(m) => m.get(c).is_some_and(|x| md5_roviny(&d, w, h, w) == *x),
            OdtlacokObrazu::Crc(m) => m.get(c).is_some_and(|x| crc_roviny(&d, w, h, w) == *x),
            OdtlacokObrazu::Kontrolny(m) => {
                m.get(c).is_some_and(|x| kontrolny_sucet_roviny(&d, w, h, w) == *x)
            }
        }
    })
}

fn opis_obrazu(u: &Uloha) -> String {
    let (sps, pps) = (&u.sps, &u.pps);
    format!(
        "obraz {} POC {} {}×{} CTB {} PPS {} (dlaždice {:?}, wpp {}, závislé {}, pcm {:?}, tskip {}, bypass {}, \
         cip {}, sdh {}, matice {}): {}",
        u.poradie,
        u.ciel.poc,
        sps.sirka,
        sps.vyska,
        1 << sps.log2_ctb,
        pps.id,
        pps.dlazdice.as_ref().map(|d| (d.stlpcov, d.riadkov, d.sirky.is_none(), d.filter_cez_dlazdice)),
        pps.wpp as u8,
        pps.zavisle_rezy as u8,
        sps.pcm.map(|p| (p.log2_min, p.log2_max, p.bez_filtra)),
        pps.transform_skip as u8,
        pps.transquant_bypass as u8,
        pps.obmedzena_intra as u8,
        pps.skryvanie_znamienok as u8,
        sps.matice.is_some() as u8,
        u.opis.join(" | ")
    )
}

impl DekodovanyObraz {
    /// Zapíše orezaný obraz (okno SPS) ako surové YUV 4:2:0 (8 bitov); počká na nehotové riadky.
    pub fn zapis_yuv(&self, ciel: &mut Vec<u8>) {
        let [l, p, h, d] = self.okno;
        for c in 0..3 {
            let r = &self.obraz.roviny[c];
            let s = usize::from(c > 0);
            let (x0, x1) = (l >> s, r.sirka - (p >> s));
            let (y0, y1) = (h >> s, r.vyska - (d >> s));
            for y in y0..y1 {
                ciel.extend_from_slice(&r.riadok(y)[x0..x1]);
            }
        }
    }
}
