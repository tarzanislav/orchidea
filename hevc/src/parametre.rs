//! Parametrické sady HEVC: VPS, SPS, PPS a ich časti (H.265, kap. 7.3.2–7.3.7, príloha E).
//!
//! Rozbor je úplný pre profily Main, Main 10 a rozšírenie rozsahu (RExt); rozšírenia viacerých vrstiev, 3D
//! a obsahu obrazovky (SCC) sa len zaznamenajú. Každá hodnota sa kontroluje voči hraniciam normy – poškodený
//! alebo podvrhnutý tok vráti chybu, nikdy nespôsobí pád ani obrovskú alokáciu (R211).

use crate::bity::Citac;
use orchidea_jadro::{Chyba, Vysledok};

fn chyba(text: impl Into<String>) -> Chyba {
    Chyba::PoskodeneData(format!("HEVC: {}", text.into()))
}

/// Profil, stupeň a úroveň (profile_tier_level) – len hlavná vrstva; podvrstvy sa prečítajú a preskočia.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfilUroven {
    pub priestor_profilu: u8,
    pub stupen_vysoky: bool,
    pub profil: u8,
    pub kompatibilita: u32,
    pub progresivny: bool,
    pub prekladany: bool,
    pub len_snimky: bool,
    /// general_level_idc = 30 × úroveň (93 = 3.1, 120 = 4, 153 = 5.1)
    pub uroven: u8,
}

impl ProfilUroven {
    pub fn citaj(c: &mut Citac, max_podvrstiev_minus1: u32) -> Vysledok<ProfilUroven> {
        let mut p = ProfilUroven {
            priestor_profilu: c.u(2)? as u8,
            stupen_vysoky: c.bit()?,
            profil: c.u(5)? as u8,
            kompatibilita: c.u(32)?,
            progresivny: c.bit()?,
            prekladany: c.bit()?,
            ..Default::default()
        };
        c.bit()?; // general_non_packed_constraint_flag
        p.len_snimky = c.bit()?;
        c.preskoc(43 + 1)?; // obmedzenia profilu + general_inbld_flag / rezerva
        p.uroven = c.u(8)? as u8;
        let n = max_podvrstiev_minus1 as usize;
        let mut profil_podvrstvy = [false; 8];
        let mut uroven_podvrstvy = [false; 8];
        for i in 0..n {
            profil_podvrstvy[i] = c.bit()?;
            uroven_podvrstvy[i] = c.bit()?;
        }
        if n > 0 {
            c.preskoc(2 * (8 - n))?;
        }
        for i in 0..n {
            if profil_podvrstvy[i] {
                c.preskoc(88)?;
            }
            if uroven_podvrstvy[i] {
                c.preskoc(8)?;
            }
        }
        Ok(p)
    }

    /// Meno profilu podľa general_profile_idc alebo príznakov kompatibility.
    pub fn meno(&self) -> &'static str {
        let ma = |i: u32| self.profil as u32 == i || (self.kompatibilita >> (31 - i)) & 1 == 1;
        if self.profil == 1 || (self.profil == 0 && ma(1)) {
            "Main"
        } else if self.profil == 2 || (self.profil == 0 && ma(2)) {
            "Main 10"
        } else if self.profil == 3 {
            "Main Still Picture"
        } else if self.profil == 4 {
            "RExt"
        } else if self.profil == 9 {
            "SCC"
        } else {
            "iný"
        }
    }
}

/// Usporiadanie podvrstiev: najväčší počet obrazov v pamäti dekodéra, preusporiadanie, oneskorenie.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Podvrstva {
    pub max_dec_pic_buffering_minus1: u32,
    pub max_num_reorder_pics: u32,
    pub max_latency_increase_plus1: u32,
}

fn citaj_podvrstvy(c: &mut Citac, max_podvrstiev_minus1: u32) -> Vysledok<Vec<Podvrstva>> {
    let vsetky = c.bit()?;
    let n = max_podvrstiev_minus1 as usize + 1;
    let mut v = vec![Podvrstva::default(); n];
    for p in v.iter_mut().skip(if vsetky { 0 } else { n - 1 }) {
        *p = Podvrstva {
            max_dec_pic_buffering_minus1: c.ue_najviac(15, "max_dec_pic_buffering_minus1")?,
            max_num_reorder_pics: c.ue_najviac(15, "max_num_reorder_pics")?,
            max_latency_increase_plus1: c.ue()?,
        };
    }
    if !vsetky {
        let posledna = v[n - 1];
        v.iter_mut().for_each(|p| *p = posledna);
    }
    Ok(v)
}

/// Časovanie (vui/vps_timing_info): `cas_jednotky / mierka` sekundy na takt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Casovanie {
    pub jednotiek_na_takt: u32,
    pub mierka_casu: u32,
}

/// Video Parameter Set – základ (rozšírenia viacerých vrstiev sa nečítajú).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vps {
    pub id: u8,
    pub max_podvrstiev_minus1: u32,
    pub profil: ProfilUroven,
    pub podvrstvy: Vec<Podvrstva>,
    pub casovanie: Option<Casovanie>,
}

impl Vps {
    pub fn citaj(rbsp: &[u8]) -> Vysledok<Vps> {
        let mut c = Citac::novy(rbsp);
        let id = c.u(4)? as u8;
        c.preskoc(2)?; // vps_base_layer_internal_flag, vps_base_layer_available_flag
        c.u(6)?; // vps_max_layers_minus1
        let max_podvrstiev_minus1 = c.u(3)?;
        if max_podvrstiev_minus1 > 6 {
            return Err(chyba("vps_max_sub_layers_minus1 > 6"));
        }
        c.bit()?; // vps_temporal_id_nesting_flag
        if c.u(16)? != 0xffff {
            return Err(chyba("vps_reserved_0xffff_16bits"));
        }
        let profil = ProfilUroven::citaj(&mut c, max_podvrstiev_minus1)?;
        let podvrstvy = citaj_podvrstvy(&mut c, max_podvrstiev_minus1)?;
        let max_vrstva = c.u(6)?;
        let sady = c.ue_najviac(1023, "vps_num_layer_sets_minus1")?;
        c.preskoc(sady as usize * (max_vrstva as usize + 1))?;
        let casovanie = if c.bit()? {
            Some(Casovanie { jednotiek_na_takt: c.u(32)?, mierka_casu: c.u(32)? })
        } else {
            None
        };
        Ok(Vps { id, max_podvrstiev_minus1, profil, podvrstvy, casovanie })
    }
}

/// Matice kvantovania (scaling_list_data) rozbalené na hodnoty v poradí prechodu (diagonálne nahor doprava).
/// `zoznamy[veľkosť][matica]` má 16 (4×4) alebo 64 hodnôt; `dc[veľkosť − 2][matica]` pre 16×16 a 32×32.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaticeKvantovania {
    pub zoznamy: [[Vec<u8>; 6]; 4],
    pub dc: [[u8; 6]; 2],
}

/// Predvolená matica 8×8 pre vnútrosnímkovú predikciu (H.265, tab. 7-6), v poradí prechodu.
const PREDVOLENA_INTRA: [u8; 64] = [
    16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 17, 16, 17, 16, 17, 18, 17, 18, 18, 17, 18, 21, 19, 20, 21, 20,
    19, 21, 24, 22, 22, 24, 24, 22, 22, 24, 25, 25, 27, 30, 27, 25, 25, 29, 31, 35, 35, 31, 29, 36, 41, 44,
    41, 36, 47, 54, 54, 47, 65, 70, 65, 88, 88, 115,
];
/// Predvolená matica 8×8 pre medzisnímkovú predikciu (H.265, tab. 7-6).
const PREDVOLENA_INTER: [u8; 64] = [
    16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 17, 17, 17, 17, 17, 18, 18, 18, 18, 18, 18, 20, 20, 20, 20, 20,
    20, 20, 24, 24, 24, 24, 24, 24, 24, 24, 25, 25, 25, 25, 25, 25, 25, 28, 28, 28, 28, 28, 28, 33, 33, 33,
    33, 33, 41, 41, 41, 41, 54, 54, 54, 71, 71, 91,
];

fn predvoleny_zoznam(velkost: usize, matica: usize) -> Vec<u8> {
    if velkost == 0 {
        vec![16; 16]
    } else if matica < 3 {
        PREDVOLENA_INTRA.to_vec()
    } else {
        PREDVOLENA_INTER.to_vec()
    }
}

impl MaticeKvantovania {
    /// Predvolené matice (scaling_list_enabled_flag bez vlastných dát).
    pub fn predvolene() -> MaticeKvantovania {
        MaticeKvantovania {
            zoznamy: std::array::from_fn(|v| std::array::from_fn(|m| predvoleny_zoznam(v, m))),
            dc: [[16; 6]; 2],
        }
    }

    /// scaling_list_data() (7.3.4).
    pub fn citaj(c: &mut Citac) -> Vysledok<MaticeKvantovania> {
        let mut s = MaticeKvantovania::predvolene();
        for velkost in 0..4 {
            let krok = if velkost == 3 { 3 } else { 1 };
            let mut matica = 0;
            while matica < 6 {
                if !c.bit()? {
                    // kópia inej matice alebo predvolená
                    let delta = c
                        .ue_najviac(matica as u32 / krok as u32, "scaling_list_pred_matrix_id_delta")?
                        as usize;
                    if delta == 0 {
                        s.zoznamy[velkost][matica] = predvoleny_zoznam(velkost, matica);
                        if velkost > 1 {
                            s.dc[velkost - 2][matica] = 16;
                        }
                    } else {
                        let zdroj = matica - delta * krok;
                        s.zoznamy[velkost][matica] = s.zoznamy[velkost][zdroj].clone();
                        if velkost > 1 {
                            s.dc[velkost - 2][matica] = s.dc[velkost - 2][zdroj];
                        }
                    }
                } else {
                    let pocet = 64.min(1 << (4 + (velkost << 1)));
                    let mut dalsi: i32 = 8;
                    if velkost > 1 {
                        let dc = c.se_rozsah(-7, 247, "scaling_list_dc_coef_minus8")?;
                        dalsi = dc + 8;
                        s.dc[velkost - 2][matica] = dalsi as u8;
                    }
                    let mut z = Vec::with_capacity(pocet);
                    for _ in 0..pocet {
                        let d = c.se_rozsah(-128, 127, "scaling_list_delta_coef")?;
                        dalsi = (dalsi + d + 256) % 256;
                        z.push(dalsi as u8);
                    }
                    if z.contains(&0) {
                        return Err(chyba("hodnota matice kvantovania 0"));
                    }
                    s.zoznamy[velkost][matica] = z;
                }
                matica += krok;
            }
        }
        // 32×32 pre farbu (4:4:4) sa pri krok 3 neprenáša – odvodí sa zo 16×16 (7.4.5)
        for m in [1, 2, 4, 5] {
            s.zoznamy[3][m] = s.zoznamy[2][m].clone();
            s.dc[1][m] = s.dc[0][m];
        }
        Ok(s)
    }
}

/// Krátkodobá sada referenčných obrazov (st_ref_pic_set) – rozdiely POC dopredu a dozadu.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SadaReferencii {
    /// DeltaPocS0 (záporné, od najbližšieho) a UsedByCurrPicS0
    pub dozadu: Vec<(i32, bool)>,
    /// DeltaPocS1 (kladné, od najbližšieho) a UsedByCurrPicS1
    pub dopredu: Vec<(i32, bool)>,
}

impl SadaReferencii {
    pub fn pocet(&self) -> usize {
        self.dozadu.len() + self.dopredu.len()
    }

    /// st_ref_pic_set(idx) (7.3.7, 7.4.8). `predosle` sú už prečítané sady (v hlavičke rezu všetky sady SPS),
    /// `pocet_v_sps` = num_short_term_ref_pic_sets – v hlavičke rezu je `idx` rovné tomuto počtu.
    /// `max_obrazov` = sps_max_dec_pic_buffering_minus1 najvyššej podvrstvy.
    pub fn citaj(
        c: &mut Citac,
        idx: usize,
        predosle: &[SadaReferencii],
        pocet_v_sps: usize,
        max_obrazov: u32,
    ) -> Vysledok<SadaReferencii> {
        Ok(Self::citaj_so_zdrojom(c, idx, predosle, pocet_v_sps, max_obrazov)?.0)
    }

    /// Ako [`SadaReferencii::citaj`], navyše vráti RefRpsIdx – index sady, z ktorej sa táto sada predikovala
    /// (inter_ref_pic_set_prediction_flag = 1), inak `None`.
    pub fn citaj_so_zdrojom(
        c: &mut Citac,
        idx: usize,
        predosle: &[SadaReferencii],
        pocet_v_sps: usize,
        max_obrazov: u32,
    ) -> Vysledok<(SadaReferencii, Option<usize>)> {
        let predikovana = idx != 0 && c.bit()?;
        if predikovana {
            let delta_idx = if idx == pocet_v_sps {
                c.ue_najviac(idx as u32 - 1, "delta_idx_minus1")? as usize + 1
            } else {
                1
            };
            let zdroj_idx = idx - delta_idx;
            let zdroj = predosle.get(zdroj_idx).ok_or_else(|| chyba("st_ref_pic_set bez zdroja"))?;
            let znamienko = c.bit()?;
            let abs = c.ue_najviac((1 << 15) - 1, "abs_delta_rps_minus1")? as i32 + 1;
            let delta_rps = if znamienko { -abs } else { abs };
            let n = zdroj.pocet();
            let mut pouzity = vec![false; n + 1];
            let mut pouzit_deltu = vec![true; n + 1];
            for j in 0..=n {
                pouzity[j] = c.bit()?;
                if !pouzity[j] {
                    pouzit_deltu[j] = c.bit()?;
                }
            }
            let (neg, poz) = (zdroj.dozadu.len(), zdroj.dopredu.len());
            let mut s = SadaReferencii::default();
            // (7-61)
            for j in (0..poz).rev() {
                let d = zdroj.dopredu[j].0 + delta_rps;
                if d < 0 && pouzit_deltu[neg + j] {
                    s.dozadu.push((d, pouzity[neg + j]));
                }
            }
            if delta_rps < 0 && pouzit_deltu[n] {
                s.dozadu.push((delta_rps, pouzity[n]));
            }
            for j in 0..neg {
                let d = zdroj.dozadu[j].0 + delta_rps;
                if d < 0 && pouzit_deltu[j] {
                    s.dozadu.push((d, pouzity[j]));
                }
            }
            // (7-62)
            for j in (0..neg).rev() {
                let d = zdroj.dozadu[j].0 + delta_rps;
                if d > 0 && pouzit_deltu[j] {
                    s.dopredu.push((d, pouzity[j]));
                }
            }
            if delta_rps > 0 && pouzit_deltu[n] {
                s.dopredu.push((delta_rps, pouzity[n]));
            }
            for j in 0..poz {
                let d = zdroj.dopredu[j].0 + delta_rps;
                if d > 0 && pouzit_deltu[neg + j] {
                    s.dopredu.push((d, pouzity[neg + j]));
                }
            }
            if s.pocet() > 16 {
                return Err(chyba("st_ref_pic_set má viac ako 16 obrazov"));
            }
            Ok((s, Some(zdroj_idx)))
        } else {
            let neg = c.ue_najviac(max_obrazov.min(16), "num_negative_pics")? as usize;
            let poz = c.ue_najviac(max_obrazov.min(16) - neg as u32, "num_positive_pics")? as usize;
            let mut s = SadaReferencii::default();
            let mut poc = 0i32;
            for _ in 0..neg {
                poc -= c.ue_najviac((1 << 15) - 1, "delta_poc_s0_minus1")? as i32 + 1;
                s.dozadu.push((poc, c.bit()?));
            }
            poc = 0;
            for _ in 0..poz {
                poc += c.ue_najviac((1 << 15) - 1, "delta_poc_s1_minus1")? as i32 + 1;
                s.dopredu.push((poc, c.bit()?));
            }
            Ok((s, None))
        }
    }
}

/// Užitočné údaje VUI (príloha E): pomer strán, farby, časovanie, okno zobrazenia.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vui {
    pub pomer_stran: Option<(u16, u16)>,
    pub plny_rozsah: bool,
    /// colour_primaries, transfer_characteristics, matrix_coeffs (2 = neurčené)
    pub farby: Option<(u8, u8, u8)>,
    pub poloha_farby: Option<(u32, u32)>,
    pub polia: bool,
    pub okno_zobrazenia: Option<[u32; 4]>,
    pub casovanie: Option<Casovanie>,
}

/// Pomery strán pixela podľa aspect_ratio_idc 1–16 (tab. E-1).
const POMERY: [(u16, u16); 17] = [
    (0, 0),
    (1, 1),
    (12, 11),
    (10, 11),
    (16, 11),
    (40, 33),
    (24, 11),
    (20, 11),
    (32, 11),
    (80, 33),
    (18, 11),
    (15, 11),
    (64, 33),
    (160, 99),
    (4, 3),
    (3, 2),
    (2, 1),
];

/// hrd_parameters() – len prečíta (hodnoty pre prehrávanie netreba).
fn preskoc_hrd(c: &mut Citac, spolocne: bool, max_podvrstiev_minus1: u32) -> Vysledok<()> {
    let (mut nal, mut vcl, mut podobraz) = (false, false, false);
    if spolocne {
        nal = c.bit()?;
        vcl = c.bit()?;
        if nal || vcl {
            podobraz = c.bit()?;
            if podobraz {
                c.preskoc(8 + 5 + 1 + 5)?;
            }
            c.preskoc(4 + 4)?;
            if podobraz {
                c.preskoc(4)?;
            }
            c.preskoc(5 + 5 + 5)?;
        }
    }
    for _ in 0..=max_podvrstiev_minus1 {
        let pevna_vseobecne = c.bit()?;
        let pevna = pevna_vseobecne || c.bit()?;
        let mut nizke_oneskorenie = false;
        if pevna {
            c.ue_najviac(2047, "elemental_duration_in_tc_minus1")?;
        } else {
            nizke_oneskorenie = c.bit()?;
        }
        let cpb = if nizke_oneskorenie { 0 } else { c.ue_najviac(31, "cpb_cnt_minus1")? };
        for pritomne in [nal, vcl] {
            if pritomne {
                for _ in 0..=cpb {
                    c.ue()?;
                    c.ue()?;
                    if podobraz {
                        c.ue()?;
                        c.ue()?;
                    }
                    c.bit()?;
                }
            }
        }
    }
    Ok(())
}

impl Vui {
    pub fn citaj(c: &mut Citac, max_podvrstiev_minus1: u32) -> Vysledok<Vui> {
        let mut v = Vui::default();
        if c.bit()? {
            let idc = c.u(8)? as usize;
            v.pomer_stran = if idc == 255 {
                Some((c.u(16)? as u16, c.u(16)? as u16))
            } else {
                POMERY.get(idc).copied().filter(|p| p.0 != 0)
            };
        }
        if c.bit()? {
            c.bit()?; // overscan_appropriate_flag
        }
        if c.bit()? {
            c.u(3)?; // video_format
            v.plny_rozsah = c.bit()?;
            if c.bit()? {
                v.farby = Some((c.u(8)? as u8, c.u(8)? as u8, c.u(8)? as u8));
            }
        }
        if c.bit()? {
            v.poloha_farby = Some((
                c.ue_najviac(5, "chroma_sample_loc_type_top_field")?,
                c.ue_najviac(5, "chroma_sample_loc_type_bottom_field")?,
            ));
        }
        c.bit()?; // neutral_chroma_indication_flag
        v.polia = c.bit()?;
        c.bit()?; // frame_field_info_present_flag
        if c.bit()? {
            v.okno_zobrazenia = Some([c.ue()?, c.ue()?, c.ue()?, c.ue()?]);
        }
        if c.bit()? {
            v.casovanie = Some(Casovanie { jednotiek_na_takt: c.u(32)?, mierka_casu: c.u(32)? });
            if c.bit()? {
                c.ue()?; // vui_num_ticks_poc_diff_one_minus1
            }
            if c.bit()? {
                preskoc_hrd(c, true, max_podvrstiev_minus1)?;
            }
        }
        if c.bit()? {
            c.preskoc(3)?; // tiles_fixed_structure, motion_vectors_over_pic_boundaries, restricted_ref_pic_lists
            c.ue_najviac(4095, "min_spatial_segmentation_idc")?;
            c.ue_najviac(16, "max_bytes_per_pic_denom")?;
            c.ue_najviac(16, "max_bits_per_min_cu_denom")?;
            c.ue_najviac(15, "log2_max_mv_length_horizontal")?;
            c.ue_najviac(15, "log2_max_mv_length_vertical")?;
        }
        Ok(v)
    }
}

/// Rozšírenie rozsahu SPS (sps_range_extension).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RozsirenieSps {
    pub transform_skip_rotation: bool,
    pub transform_skip_context: bool,
    pub implicit_rdpcm: bool,
    pub explicit_rdpcm: bool,
    pub extended_precision_processing: bool,
    pub intra_smoothing_disabled: bool,
    pub high_precision_offsets: bool,
    pub persistent_rice_adaptation: bool,
    pub cabac_bypass_alignment: bool,
}

/// Bloky PCM (nekomprimované vzorky).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pcm {
    pub bity_jasu: u8,
    pub bity_farby: u8,
    pub log2_min: u32,
    pub log2_max: u32,
    pub bez_filtra: bool,
}

/// Sequence Parameter Set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sps {
    pub vps_id: u8,
    pub max_podvrstiev_minus1: u32,
    pub profil: ProfilUroven,
    pub id: u32,
    /// 0 = len jas, 1 = 4:2:0, 2 = 4:2:2, 3 = 4:4:4
    pub chroma_format_idc: u32,
    pub oddelene_roviny_farby: bool,
    pub sirka: u32,
    pub vyska: u32,
    /// orezanie (conformance window) v jednotkách vzoriek farby: vľavo, vpravo, hore, dole
    pub okno: [u32; 4],
    pub bity_jasu: u32,
    pub bity_farby: u32,
    pub log2_max_poc_lsb: u32,
    pub podvrstvy: Vec<Podvrstva>,
    pub log2_min_cb: u32,
    pub log2_ctb: u32,
    pub log2_min_tb: u32,
    pub log2_max_tb: u32,
    pub max_hlbka_inter: u32,
    pub max_hlbka_intra: u32,
    /// `None` = matice vypnuté; inak predvolené alebo prenesené
    pub matice: Option<MaticeKvantovania>,
    pub amp: bool,
    pub sao: bool,
    pub pcm: Option<Pcm>,
    pub sady_referencii: Vec<SadaReferencii>,
    /// dlhodobé referencie v SPS: (lt_ref_pic_poc_lsb_sps, used_by_curr_pic_lt_sps_flag); `None` = vypnuté
    pub dlhodobe: Option<Vec<(u32, bool)>>,
    pub temporal_mvp: bool,
    pub strong_intra_smoothing: bool,
    pub vui: Option<Vui>,
    pub rozsirenie: RozsirenieSps,
    /// prítomné rozšírenia, ktoré sa nečítajú (viac vrstiev, 3D, SCC, ďalšie)
    pub necitane_rozsirenia: bool,
}

impl Sps {
    pub fn citaj(rbsp: &[u8]) -> Vysledok<Sps> {
        let mut c = Citac::novy(rbsp);
        let vps_id = c.u(4)? as u8;
        let max_podvrstiev_minus1 = c.u(3)?;
        if max_podvrstiev_minus1 > 6 {
            return Err(chyba("sps_max_sub_layers_minus1 > 6"));
        }
        c.bit()?; // sps_temporal_id_nesting_flag
        let profil = ProfilUroven::citaj(&mut c, max_podvrstiev_minus1)?;
        let id = c.ue_najviac(15, "sps_seq_parameter_set_id")?;
        let chroma_format_idc = c.ue_najviac(3, "chroma_format_idc")?;
        let oddelene_roviny_farby = chroma_format_idc == 3 && c.bit()?;
        let sirka = c.ue_najviac(16888, "pic_width_in_luma_samples")?;
        let vyska = c.ue_najviac(16888, "pic_height_in_luma_samples")?;
        if sirka == 0 || vyska == 0 {
            return Err(chyba("nulové rozmery obrazu"));
        }
        let okno = if c.bit()? { [c.ue()?, c.ue()?, c.ue()?, c.ue()?] } else { [0; 4] };
        let bity_jasu = c.ue_najviac(8, "bit_depth_luma_minus8")? + 8;
        let bity_farby = c.ue_najviac(8, "bit_depth_chroma_minus8")? + 8;
        let log2_max_poc_lsb = c.ue_najviac(12, "log2_max_pic_order_cnt_lsb_minus4")? + 4;
        let podvrstvy = citaj_podvrstvy(&mut c, max_podvrstiev_minus1)?;
        let log2_min_cb = c.ue_najviac(3, "log2_min_luma_coding_block_size_minus3")? + 3;
        let log2_ctb = log2_min_cb + c.ue_najviac(3, "log2_diff_max_min_luma_coding_block_size")?;
        let log2_min_tb = c.ue_najviac(3, "log2_min_luma_transform_block_size_minus2")? + 2;
        let log2_max_tb = log2_min_tb + c.ue_najviac(3, "log2_diff_max_min_luma_transform_block_size")?;
        if log2_ctb > 6 || log2_max_tb > 5 || log2_min_tb >= log2_min_cb || log2_max_tb > log2_ctb {
            return Err(chyba(format!(
                "veľkosti blokov: CTB 2^{log2_ctb}, CB 2^{log2_min_cb}, TB 2^{log2_min_tb}..2^{log2_max_tb}"
            )));
        }
        if sirka % (1 << log2_min_cb) != 0 || vyska % (1 << log2_min_cb) != 0 {
            return Err(chyba("rozmery obrazu nie sú násobkom najmenšieho bloku"));
        }
        let max_hlbka_inter = c.ue_najviac(log2_ctb - log2_min_tb, "max_transform_hierarchy_depth_inter")?;
        let max_hlbka_intra = c.ue_najviac(log2_ctb - log2_min_tb, "max_transform_hierarchy_depth_intra")?;
        let matice = if c.bit()? {
            Some(if c.bit()? { MaticeKvantovania::citaj(&mut c)? } else { MaticeKvantovania::predvolene() })
        } else {
            None
        };
        let amp = c.bit()?;
        let sao = c.bit()?;
        let pcm = if c.bit()? {
            let bity_jasu_pcm = c.u(4)? as u8 + 1;
            let bity_farby_pcm = c.u(4)? as u8 + 1;
            let log2_min = c.ue_najviac(2, "log2_min_pcm_luma_coding_block_size_minus3")? + 3;
            let log2_max =
                log2_min + c.ue_najviac(5 - log2_min, "log2_diff_max_min_pcm_luma_coding_block_size")?;
            if bity_jasu_pcm as u32 > bity_jasu || bity_farby_pcm as u32 > bity_farby {
                return Err(chyba("PCM má viac bitov ako obraz"));
            }
            Some(Pcm {
                bity_jasu: bity_jasu_pcm,
                bity_farby: bity_farby_pcm,
                log2_min,
                log2_max,
                bez_filtra: c.bit()?,
            })
        } else {
            None
        };
        let max_obrazov = podvrstvy.last().map_or(0, |p| p.max_dec_pic_buffering_minus1);
        let pocet_sad = c.ue_najviac(64, "num_short_term_ref_pic_sets")? as usize;
        let mut sady_referencii = Vec::with_capacity(pocet_sad);
        for i in 0..pocet_sad {
            let s = SadaReferencii::citaj(&mut c, i, &sady_referencii, pocet_sad, max_obrazov)?;
            sady_referencii.push(s);
        }
        let dlhodobe = if c.bit()? {
            let n = c.ue_najviac(32, "num_long_term_ref_pics_sps")?;
            let mut v = Vec::with_capacity(n as usize);
            for _ in 0..n {
                v.push((c.u(log2_max_poc_lsb)?, c.bit()?));
            }
            Some(v)
        } else {
            None
        };
        let temporal_mvp = c.bit()?;
        let strong_intra_smoothing = c.bit()?;
        let vui = if c.bit()? { Some(Vui::citaj(&mut c, max_podvrstiev_minus1)?) } else { None };
        let mut rozsirenie = RozsirenieSps::default();
        let mut necitane_rozsirenia = false;
        if c.bit()? {
            let rozsah = c.bit()?;
            let ostatne = c.u(7)?; // viac vrstiev, 3D, SCC, sps_extension_4bits
            if rozsah {
                rozsirenie = RozsirenieSps {
                    transform_skip_rotation: c.bit()?,
                    transform_skip_context: c.bit()?,
                    implicit_rdpcm: c.bit()?,
                    explicit_rdpcm: c.bit()?,
                    extended_precision_processing: c.bit()?,
                    intra_smoothing_disabled: c.bit()?,
                    high_precision_offsets: c.bit()?,
                    persistent_rice_adaptation: c.bit()?,
                    cabac_bypass_alignment: c.bit()?,
                };
            }
            necitane_rozsirenia = ostatne != 0;
        }
        Ok(Sps {
            vps_id,
            max_podvrstiev_minus1,
            profil,
            id,
            chroma_format_idc,
            oddelene_roviny_farby,
            sirka,
            vyska,
            okno,
            bity_jasu,
            bity_farby,
            log2_max_poc_lsb,
            podvrstvy,
            log2_min_cb,
            log2_ctb,
            log2_min_tb,
            log2_max_tb,
            max_hlbka_inter,
            max_hlbka_intra,
            matice,
            amp,
            sao,
            pcm,
            sady_referencii,
            dlhodobe,
            temporal_mvp,
            strong_intra_smoothing,
            vui,
            rozsirenie,
            necitane_rozsirenia,
        })
    }

    /// Delenie vzoriek farby (SubWidthC, SubHeightC).
    pub fn delenie_farby(&self) -> (u32, u32) {
        match (self.chroma_format_idc, self.oddelene_roviny_farby) {
            (1, _) => (2, 2),
            (2, _) => (2, 1),
            _ => (1, 1),
        }
    }

    /// Rozmery po orezaní (conformance window) – to, čo sa zobrazí.
    pub fn zobrazene_rozmery(&self) -> (u32, u32) {
        let (sx, sy) = self.delenie_farby();
        let w = self.sirka.saturating_sub(sx * (self.okno[0] + self.okno[1]));
        let h = self.vyska.saturating_sub(sy * (self.okno[2] + self.okno[3]));
        (w, h)
    }

    /// Počet CTB vo vodorovnom a zvislom smere.
    pub fn ctb_mriezka(&self) -> (u32, u32) {
        (self.sirka.div_ceil(1 << self.log2_ctb), self.vyska.div_ceil(1 << self.log2_ctb))
    }
}

/// Dlaždice obrazu (tiles) – šírky stĺpcov a výšky riadkov v CTB; `None` pri rovnomernom rozdelení.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DlazdiceHevc {
    pub stlpcov: u32,
    pub riadkov: u32,
    pub sirky: Option<Vec<u32>>,
    pub vysky: Option<Vec<u32>>,
    pub filter_cez_dlazdice: bool,
}

impl DlazdiceHevc {
    /// Šírky stĺpcov a výšky riadkov v CTB pre daný obraz (6-3, 6-4).
    pub fn rozdelenie(&self, sps: &Sps) -> (Vec<u32>, Vec<u32>) {
        let (sc, sr) = sps.ctb_mriezka();
        let rovnomerne = |n: u32, celok: u32| -> Vec<u32> {
            (0..n).map(|i| ((i + 1) * celok) / n - (i * celok) / n).collect()
        };
        let doplnene = |v: &Vec<u32>, celok: u32| -> Vec<u32> {
            let mut v = v.clone();
            let sucet: u32 = v.iter().sum();
            v.push(celok.saturating_sub(sucet));
            v
        };
        let s = match &self.sirky {
            Some(v) => doplnene(v, sc),
            None => rovnomerne(self.stlpcov, sc),
        };
        let r = match &self.vysky {
            Some(v) => doplnene(v, sr),
            None => rovnomerne(self.riadkov, sr),
        };
        (s, r)
    }
}

/// Odblokovací filter v PPS.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Odblokovanie {
    pub prepis_v_reze: bool,
    pub vypnute: bool,
    pub beta_div2: i32,
    pub tc_div2: i32,
}

/// Rozšírenie rozsahu PPS (pps_range_extension).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RozsireniePps {
    pub log2_max_transform_skip: u32,
    pub cross_component_prediction: bool,
    /// (diff_cu_chroma_qp_offset_depth, zoznam (cb, cr))
    pub chroma_qp_offset: Option<(u32, Vec<(i32, i32)>)>,
    pub log2_sao_offset_scale_luma: u32,
    pub log2_sao_offset_scale_chroma: u32,
}

/// Picture Parameter Set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pps {
    pub id: u32,
    pub sps_id: u32,
    pub zavisle_rezy: bool,
    pub priznak_vystupu: bool,
    pub extra_bity_hlavicky: u32,
    pub skryvanie_znamienok: bool,
    pub cabac_init: bool,
    pub ref_l0_minus1: u32,
    pub ref_l1_minus1: u32,
    pub init_qp: i32,
    pub obmedzena_intra: bool,
    pub transform_skip: bool,
    /// diff_cu_qp_delta_depth, ak je cu_qp_delta zapnutá
    pub cu_qp_delta: Option<u32>,
    pub cb_qp_offset: i32,
    pub cr_qp_offset: i32,
    pub chroma_qp_v_reze: bool,
    pub vazena_pred: bool,
    pub vazena_bipred: bool,
    pub transquant_bypass: bool,
    pub dlazdice: Option<DlazdiceHevc>,
    pub wpp: bool,
    pub filter_cez_rezy: bool,
    pub odblokovanie: Option<Odblokovanie>,
    pub matice: Option<MaticeKvantovania>,
    pub uprava_zoznamov: bool,
    pub log2_parallel_merge_level: u32,
    pub rozsirenie_hlavicky_rezu: bool,
    pub rozsirenie: Option<RozsireniePps>,
    pub necitane_rozsirenia: bool,
}

impl Pps {
    pub fn citaj(rbsp: &[u8]) -> Vysledok<Pps> {
        let mut c = Citac::novy(rbsp);
        let id = c.ue_najviac(63, "pps_pic_parameter_set_id")?;
        let sps_id = c.ue_najviac(15, "pps_seq_parameter_set_id")?;
        let zavisle_rezy = c.bit()?;
        let priznak_vystupu = c.bit()?;
        let extra_bity_hlavicky = c.u(3)?;
        let skryvanie_znamienok = c.bit()?;
        let cabac_init = c.bit()?;
        let ref_l0_minus1 = c.ue_najviac(14, "num_ref_idx_l0_default_active_minus1")?;
        let ref_l1_minus1 = c.ue_najviac(14, "num_ref_idx_l1_default_active_minus1")?;
        // init_qp_minus26: −(26 + QpBdOffsetY) .. 25; QpBdOffsetY ≤ 48 pri 16 bitoch
        let init_qp = 26 + c.se_rozsah(-(26 + 48), 25, "init_qp_minus26")?;
        let obmedzena_intra = c.bit()?;
        let transform_skip = c.bit()?;
        let cu_qp_delta = if c.bit()? { Some(c.ue_najviac(3, "diff_cu_qp_delta_depth")?) } else { None };
        let cb_qp_offset = c.se_rozsah(-12, 12, "pps_cb_qp_offset")?;
        let cr_qp_offset = c.se_rozsah(-12, 12, "pps_cr_qp_offset")?;
        let chroma_qp_v_reze = c.bit()?;
        let vazena_pred = c.bit()?;
        let vazena_bipred = c.bit()?;
        let transquant_bypass = c.bit()?;
        let ma_dlazdice = c.bit()?;
        let wpp = c.bit()?;
        let dlazdice = if ma_dlazdice {
            let stlpcov = c.ue_najviac(19, "num_tile_columns_minus1")? + 1;
            let riadkov = c.ue_najviac(21, "num_tile_rows_minus1")? + 1;
            if stlpcov == 1 && riadkov == 1 {
                return Err(chyba("tiles_enabled_flag s jednou dlaždicou"));
            }
            let rovnomerne = c.bit()?;
            let (sirky, vysky) = if rovnomerne {
                (None, None)
            } else {
                let mut s = Vec::with_capacity(stlpcov as usize - 1);
                for _ in 1..stlpcov {
                    s.push(c.ue_najviac(1 << 15, "column_width_minus1")? + 1);
                }
                let mut r = Vec::with_capacity(riadkov as usize - 1);
                for _ in 1..riadkov {
                    r.push(c.ue_najviac(1 << 15, "row_height_minus1")? + 1);
                }
                (Some(s), Some(r))
            };
            Some(DlazdiceHevc { stlpcov, riadkov, sirky, vysky, filter_cez_dlazdice: c.bit()? })
        } else {
            None
        };
        let filter_cez_rezy = c.bit()?;
        let odblokovanie = if c.bit()? {
            let prepis_v_reze = c.bit()?;
            let vypnute = c.bit()?;
            let (beta_div2, tc_div2) = if vypnute {
                (0, 0)
            } else {
                (c.se_rozsah(-6, 6, "pps_beta_offset_div2")?, c.se_rozsah(-6, 6, "pps_tc_offset_div2")?)
            };
            Some(Odblokovanie { prepis_v_reze, vypnute, beta_div2, tc_div2 })
        } else {
            None
        };
        let matice = if c.bit()? { Some(MaticeKvantovania::citaj(&mut c)?) } else { None };
        let uprava_zoznamov = c.bit()?;
        let log2_parallel_merge_level = c.ue_najviac(4, "log2_parallel_merge_level_minus2")? + 2;
        let rozsirenie_hlavicky_rezu = c.bit()?;
        let mut rozsirenie = None;
        let mut necitane_rozsirenia = false;
        if c.bit()? {
            let rozsah = c.bit()?;
            let ostatne = c.u(7)?;
            if rozsah {
                let log2_max_transform_skip = if transform_skip {
                    c.ue_najviac(3, "log2_max_transform_skip_block_size_minus2")? + 2
                } else {
                    2
                };
                let cross_component_prediction = c.bit()?;
                let chroma_qp_offset = if c.bit()? {
                    let hlbka = c.ue_najviac(3, "diff_cu_chroma_qp_offset_depth")?;
                    let n = c.ue_najviac(5, "chroma_qp_offset_list_len_minus1")? + 1;
                    let mut v = Vec::with_capacity(n as usize);
                    for _ in 0..n {
                        v.push((
                            c.se_rozsah(-12, 12, "cb_qp_offset_list")?,
                            c.se_rozsah(-12, 12, "cr_qp_offset_list")?,
                        ));
                    }
                    Some((hlbka, v))
                } else {
                    None
                };
                rozsirenie = Some(RozsireniePps {
                    log2_max_transform_skip,
                    cross_component_prediction,
                    chroma_qp_offset,
                    log2_sao_offset_scale_luma: c.ue_najviac(6, "log2_sao_offset_scale_luma")?,
                    log2_sao_offset_scale_chroma: c.ue_najviac(6, "log2_sao_offset_scale_chroma")?,
                });
            }
            necitane_rozsirenia = ostatne != 0;
        }
        Ok(Pps {
            id,
            sps_id,
            zavisle_rezy,
            priznak_vystupu,
            extra_bity_hlavicky,
            skryvanie_znamienok,
            cabac_init,
            ref_l0_minus1,
            ref_l1_minus1,
            init_qp,
            obmedzena_intra,
            transform_skip,
            cu_qp_delta,
            cb_qp_offset,
            cr_qp_offset,
            chroma_qp_v_reze,
            vazena_pred,
            vazena_bipred,
            transquant_bypass,
            dlazdice,
            wpp,
            filter_cez_rezy,
            odblokovanie,
            matice,
            uprava_zoznamov,
            log2_parallel_merge_level,
            rozsirenie_hlavicky_rezu,
            rozsirenie,
            necitane_rozsirenia,
        })
    }
}
