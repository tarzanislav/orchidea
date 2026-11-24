//! Dekodér toku HEVC Main 8-bit: jednotky NAL → obrazy v poradí zobrazenia. Správa obrazov je spoločná
//! s hardvérovou cestou (`orchidea_hevc::SpravaObrazov`); každý obraz sa overí odtlačkom MD5 zo SEI, ak ho tok má.

use crate::filtre::RezFiltra;
use crate::geometria::Geometria;
use crate::obraz::Obraz;
use crate::odblokovanie;
use crate::pohyb::PoleKol;
use crate::sao;
use crate::syntax::{DekoderRezu, DekodovanyObraz, ReferencieRezu, StavObrazu};
use crate::transformacia::FaktorySkalovania;
use orchidea_hevc::nal::typ;
use orchidea_hevc::obrazy::Udalost;
use orchidea_hevc::odtlacok::md5_roviny;
use orchidea_hevc::sei::{self, OdtlacokObrazu};
use orchidea_hevc::{HlavickaNal, HlavickaRezu, Pps, Sady, SpravaObrazov, Sps, rbsp};
use orchidea_jadro::{Chyba, Vysledok};
use std::collections::HashMap;
use std::sync::Arc;

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
}

struct Aktualny {
    id: u32,
    poc: i32,
    stav: StavObrazu,
    sps: Arc<Sps>,
    pps: Arc<Pps>,
    geometria: Arc<Geometria>,
    faktory: Option<Arc<FaktorySkalovania>>,
    odtlacky: Vec<OdtlacokObrazu>,
    poradie: usize,
}

/// Softvérový dekodér HEVC.
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
            statistika: Statistika::default(),
        }
    }

    /// Spracuje jednotku NAL (bez štartovacieho kódu) a vráti obrazy pripravené na výstup (v poradí zobrazenia).
    pub fn nal(&mut self, nal: &[u8]) -> Vec<Arc<DekodovanyObraz>> {
        if let Err(e) = self.nal_vnutorne(nal) {
            self.statistika.chyby.push(e.to_string());
        }
        std::mem::take(&mut self.vystup)
    }

    /// Koniec toku: dokončí posledný obraz a vydá všetky zostávajúce.
    pub fn koniec(&mut self) -> Vec<Arc<DekodovanyObraz>> {
        self.dokonci_obraz();
        let u = self.sprava.vyprazdni();
        self.udalosti(&u, (0, 0));
        std::mem::take(&mut self.vystup)
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
                self.statistika.preskocene += 1;
                self.cakajuce_odtlacky.clear();
                return Ok(());
            }
            if let Err(e) = over_profil(&sps, &pps) {
                // obraz sa nedekóduje – do pamäte ide sivý obraz, aby referencie ostali konzistentné
                self.preskakuje = true;
                self.udalosti(&[Udalost::Vytvor { id: z.id, poc: z.poc }], rozmery);
                let u = self.sprava.ukonci_obraz(&sps);
                self.udalosti(&u, rozmery);
                return Err(e);
            }
            let geometria = self.geometria_pre(&sps, &pps);
            let faktory = self.faktory_pre(&sps, &pps);
            let mut stav = StavObrazu::novy(&geometria, z.poc);
            stav.info.cb_qp_offset = pps.cb_qp_offset;
            stav.info.cr_qp_offset = pps.cr_qp_offset;
            stav.info.cez_dlazdice = pps.dlazdice.as_ref().is_none_or(|d| d.filter_cez_dlazdice);
            let poradie = self.statistika.obrazov;
            self.statistika.obrazov += 1;
            self.aktualny = Some(Aktualny {
                id: z.id,
                poc: z.poc,
                stav,
                sps: Arc::new(sps),
                pps: Arc::new(pps),
                geometria,
                faktory,
                odtlacky: std::mem::take(&mut self.cakajuce_odtlacky),
                poradie,
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
        let rez_index = a.stav.info.rezy.len() as u16;
        a.stav.info.rezy.push(RezFiltra {
            adresa: rez.adresa_rezu,
            cez_rezy: rez.filter_cez_rezy,
            beta_div2: rez.beta_div2 as i8,
            tc_div2: rez.tc_div2 as i8,
        });
        let (sps, pps, g, f) = (a.sps.clone(), a.pps.clone(), a.geometria.clone(), a.faktory.clone());
        let mut d = DekoderRezu::novy(
            &data,
            &mut a.stav,
            &g,
            &sps,
            &pps,
            &rez,
            &refs,
            kol.as_ref().map(|k| &k.kol),
            f.as_deref(),
            rez_index,
        );
        d.dekoduj()
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
                    let o = DekodovanyObraz {
                        id,
                        poc,
                        obraz: Obraz::novy(rozmery.0, rozmery.1),
                        kol: PoleKol { sirka16: 0, vyska16: 0, bloky: Vec::new(), poc },
                        okno: [0; 4],
                    };
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

    /// Dokončenie obrazu: filtre v slučke, odtlačok MD5, uloženie do pamäte dekodéra a výstup (C.5.2.3).
    fn dokonci_obraz(&mut self) {
        self.nezavisla = None;
        if self.preskakuje {
            self.preskakuje = false;
            return;
        }
        let Some(a) = self.aktualny.take() else { return };
        let Aktualny { id, poc, mut stav, sps, odtlacky, poradie, .. } = a;
        odblokovanie::odblokuj(&mut stav.obraz, &stav.mapy, &stav.info);
        let obraz = if stav.sao.iter().any(|s| s.typ != [0; 3]) {
            let mut vystup = Obraz::novy(stav.obraz.sirka(), stav.obraz.vyska());
            sao::sao(&stav.obraz, &mut vystup, &stav.sao, &stav.mapy, &stav.info);
            vystup
        } else {
            stav.obraz
        };
        // odtlačok
        match odtlacky.iter().find(|o| matches!(o, OdtlacokObrazu::Md5(_))) {
            Some(OdtlacokObrazu::Md5(m)) => {
                let zhoda = (0..3).all(|c| {
                    let r = &obraz.roviny[c];
                    m.get(c).is_some_and(|ocak| md5_roviny(&r.data, r.sirka, r.vyska, r.krok) == *ocak)
                });
                if zhoda {
                    self.statistika.zhoda += 1;
                } else {
                    self.statistika.nezhoda.push((poradie, poc));
                }
            }
            _ => self.statistika.bez_odtlacku += 1,
        }
        let (sx, sy) = sps.delenie_farby();
        let okno = [
            (sps.okno[0] * sx) as usize,
            (sps.okno[1] * sx) as usize,
            (sps.okno[2] * sy) as usize,
            (sps.okno[3] * sy) as usize,
        ];
        let d = DekodovanyObraz { id, poc, obraz, kol: stav.kol, okno };
        self.dpb.insert(id, Arc::new(d));
        let u = self.sprava.ukonci_obraz(&sps);
        self.udalosti(&u, (0, 0));
    }
}

impl DekodovanyObraz {
    /// Zapíše orezaný obraz (okno SPS) ako surové YUV 4:2:0 (8 bitov).
    pub fn zapis_yuv(&self, ciel: &mut Vec<u8>) {
        let [l, p, h, d] = self.okno;
        for c in 0..3 {
            let r = &self.obraz.roviny[c];
            let s = if c == 0 { 0 } else { 1 };
            let (x0, x1) = (l >> s, r.sirka - (p >> s));
            let (y0, y1) = (h >> s, r.vyska - (d >> s));
            for y in y0..y1 {
                ciel.extend_from_slice(&r.data[y * r.krok + x0..y * r.krok + x1]);
            }
        }
    }
}
