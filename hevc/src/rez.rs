//! Hlavička rezu (slice_segment_header) – H.265, kap. 7.3.6 a 7.4.7 (krok H1).
//!
//! Spoločná pre hardvérové dekodéry *stateless* (RPi 4/400/5 – ovládacie prvky V4L2 sa z nej odvodia v module
//! [`crate::v4l2`]) aj pre vlastný softvérový dekodér. Závislý segment rezu preberá hodnoty z predošlého
//! nezávislého segmentu. Každá hodnota sa kontroluje voči hraniciam normy – poškodený tok vráti chybu (R211).

use crate::bity::Citac;
use crate::nal::{HlavickaNal, typ};
use crate::parametre::{Pps, SadaReferencii, Sps};
use crate::sady::Sady;
use orchidea_jadro::{Chyba, Vysledok};

fn chyba(text: impl Into<String>) -> Chyba {
    Chyba::PoskodeneData(format!("HEVC: {}", text.into()))
}

/// Ceil(Log2(n)) pre n ≥ 1 (počet bitov polí u(v) s rozsahom 0 … n − 1).
pub fn bity_pre(n: u32) -> u32 {
    if n <= 1 { 0 } else { 32 - (n - 1).leading_zeros() }
}

/// Typ rezu (slice_type).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypRezu {
    B = 0,
    P = 1,
    I = 2,
}

/// Dlhodobý referenčný obraz uvedený v hlavičke rezu (po odvodení podľa 7.4.7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DlhodobyRef {
    /// PocLsbLt[i]
    pub poc_lsb: u32,
    /// UsedByCurrPicLt[i]
    pub pouzity: bool,
    /// DeltaPocMsbCycleLt[i], ak delta_poc_msb_present_flag[i] = 1
    pub msb_cyklus: Option<u32>,
}

/// Váhy jedného referenčného obrazu z pred_weight_table (hodnoty syntaxe).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VahaReferencie {
    /// (delta_luma_weight, luma_offset), ak luma_weight_flag = 1
    pub jas: Option<(i32, i32)>,
    /// (delta_chroma_weight, delta_chroma_offset) pre Cb a Cr, ak chroma_weight_flag = 1
    pub farba: Option<[(i32, i32); 2]>,
}

/// pred_weight_table() (7.3.6.3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabulkaVah {
    /// luma_log2_weight_denom
    pub log2_jas: u32,
    /// ChromaLog2WeightDenom = luma_log2_weight_denom + delta_chroma_log2_weight_denom
    pub log2_farba: u32,
    /// váhy pre zoznamy L0 a L1 (dĺžka = počet aktívnych referencií)
    pub zoznamy: [Vec<VahaReferencie>; 2],
}

impl TabulkaVah {
    /// LumaWeightLX[i] a luma_offset_lX[i] (7.4.7.3) – posun pre 8 bitov; pri inej hĺbke ho treba posunúť
    /// o BitDepth − 8 (WpOffsetBdShiftY).
    pub fn jas(&self, lx: usize, i: usize) -> (i32, i32) {
        match self.zoznamy[lx].get(i).and_then(|v| v.jas) {
            Some((dw, o)) => ((1 << self.log2_jas) + dw, o),
            None => (1 << self.log2_jas, 0),
        }
    }

    /// ChromaWeightLX[i][j] a ChromaOffsetLX[i][j] (7.4.7.3, wpOffsetHalfRangeC = 128 – bez
    /// high_precision_offsets).
    pub fn farba(&self, lx: usize, i: usize, j: usize) -> (i32, i32) {
        match self.zoznamy[lx].get(i).and_then(|v| v.farba) {
            Some(f) => {
                let (dw, doff) = f[j];
                let w = (1 << self.log2_farba) + dw;
                let polovica = 128;
                let o =
                    (polovica + doff - ((polovica * w) >> self.log2_farba)).clamp(-polovica, polovica - 1);
                (w, o)
            }
            None => (1 << self.log2_farba, 0),
        }
    }
}

/// Hlavička segmentu rezu. Pri závislom segmente sú polia za `adresa` prevzaté z nezávislého segmentu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlavickaRezu {
    /// hlavička jednotky NAL, z ktorej segment pochádza
    pub nal: HlavickaNal,
    /// first_slice_segment_in_pic_flag
    pub prvy_v_obraze: bool,
    /// no_output_of_prior_pics_flag (len IRAP)
    pub bez_vystupu_predoslych: bool,
    /// slice_pic_parameter_set_id
    pub pps_id: u32,
    /// dependent_slice_segment_flag
    pub zavisly: bool,
    /// slice_segment_address – adresa prvého CTB segmentu v rastri obrazu
    pub adresa: u32,
    /// SliceAddrRs – adresa prvého CTB rezu (nezávislého segmentu)
    pub adresa_rezu: u32,
    pub typ: TypRezu,
    /// pic_output_flag
    pub vystup: bool,
    /// colour_plane_id
    pub rovina_farby: u8,
    /// slice_pic_order_cnt_lsb (0 pri IDR)
    pub poc_lsb: u32,
    /// short_term_ref_pic_set_idx, ak sa použila sada zo SPS (short_term_ref_pic_set_sps_flag = 1)
    pub sada_zo_sps: Option<usize>,
    /// platná krátkodobá sada referencií (zo SPS alebo z hlavičky); prázdna pri IDR
    pub sada: SadaReferencii,
    /// počet bitov st_ref_pic_set() v hlavičke (0, ak je sada zo SPS)
    pub bity_sady: u32,
    /// NumDeltaPocs[RefRpsIdx] – počet obrazov sady SPS, z ktorej sa sada v hlavičke predikovala (inak 0)
    pub delty_zdrojovej_sady: u32,
    /// dlhodobé referencie (najprv num_long_term_sps zo SPS, potom num_long_term_pics)
    pub dlhodobe: Vec<DlhodobyRef>,
    /// num_long_term_sps
    pub dlhodobe_zo_sps: u32,
    /// počet bitov bloku dlhodobých referencií
    pub bity_dlhodobych: u32,
    /// slice_temporal_mvp_enabled_flag
    pub tmvp: bool,
    /// slice_sao_luma_flag, slice_sao_chroma_flag
    pub sao_jas: bool,
    pub sao_farba: bool,
    /// num_ref_idx_l0/l1_active_minus1 + 1 (0 pre nepoužitý zoznam)
    pub aktivne: [u32; 2],
    /// list_entry_lX, ak ref_pic_list_modification_flag_lX = 1
    pub uprava_zoznamu: [Option<Vec<u32>>; 2],
    /// mvd_l1_zero_flag
    pub mvd_l1_nula: bool,
    /// cabac_init_flag
    pub cabac_init: bool,
    /// collocated_from_l0_flag
    pub kolokovany_z_l0: bool,
    /// collocated_ref_idx
    pub kolokovany_ref: u32,
    /// pred_weight_table(), ak je vážená predikcia zapnutá pre tento typ rezu
    pub vahy: Option<TabulkaVah>,
    /// MaxNumMergeCand = 5 − five_minus_max_num_merge_cand
    pub max_zlucenie: u32,
    /// slice_qp_delta
    pub qp_delta: i32,
    /// SliceQpY = 26 + init_qp_minus26 + slice_qp_delta
    pub qp: i32,
    /// slice_cb_qp_offset, slice_cr_qp_offset
    pub cb_qp_offset: i32,
    pub cr_qp_offset: i32,
    /// cu_chroma_qp_offset_enabled_flag
    pub cu_chroma_qp_offset: bool,
    /// slice_deblocking_filter_disabled_flag
    pub odblokovanie_vypnute: bool,
    /// slice_beta_offset_div2, slice_tc_offset_div2
    pub beta_div2: i32,
    pub tc_div2: i32,
    /// slice_loop_filter_across_slices_enabled_flag
    pub filter_cez_rezy: bool,
    /// entry_point_offset_minus1[i] + 1 – dĺžky podprúdov v bajtoch **vrátane** ochranných bajtov (7.4.7.1)
    pub vstupne_body: Vec<u32>,
    /// offset_len_minus1 + 1 (0, ak nie sú vstupné body)
    pub bity_vstupnych_bodov: u32,
    /// dĺžka hlavičky v bajtoch RBSP (po byte_alignment) – tu začína slice_segment_data()
    pub dlzka_rbsp: usize,
    /// NumPicTotalCurr – počet obrazov RPS použitých aktuálnym obrazom
    pub pocet_aktualnych: u32,
}

impl HlavickaRezu {
    /// slice_segment_header() z RBSP jednotky NAL rezu (`rbsp` **bez** 2-bajtovej hlavičky NAL).
    /// `nezavisla` je hlavička posledného nezávislého segmentu toho istého obrazu (pre závislé segmenty).
    pub fn citaj(
        rbsp: &[u8],
        nal: HlavickaNal,
        sady: &Sady,
        nezavisla: Option<&HlavickaRezu>,
    ) -> Vysledok<HlavickaRezu> {
        if !nal.je_vcl() || nal.typ > typ::RSV_IRAP_23 || (10..=15).contains(&nal.typ) {
            return Err(chyba(format!("jednotka NAL typu {} nie je kódovaný rez", nal.typ)));
        }
        let mut c = Citac::novy(rbsp);
        let prvy_v_obraze = c.bit()?;
        let bez_vystupu_predoslych = nal.je_irap() && c.bit()?;
        let pps_id = c.ue_najviac(63, "slice_pic_parameter_set_id")?;
        let (sps, pps) = sady.pre_rez(pps_id)?;
        let (sirka_ctb, vyska_ctb) = sps.ctb_mriezka();
        let ctb_spolu = sirka_ctb * vyska_ctb;

        let mut zavisly = false;
        let mut adresa = 0;
        if !prvy_v_obraze {
            if pps.zavisle_rezy {
                zavisly = c.bit()?;
            }
            adresa = c.u(bity_pre(ctb_spolu))?;
            if adresa >= ctb_spolu {
                return Err(chyba(format!("slice_segment_address {adresa} ≥ {ctb_spolu}")));
            }
        }

        let mut h = if zavisly {
            let n = nezavisla.ok_or_else(|| chyba("závislý segment rezu bez nezávislého segmentu"))?;
            if n.pps_id != pps_id {
                return Err(chyba("závislý segment rezu s iným PPS"));
            }
            HlavickaRezu { nal, prvy_v_obraze, zavisly, adresa, vstupne_body: Vec::new(), ..n.clone() }
        } else {
            citaj_nezavislu(&mut c, nal, sps, pps, prvy_v_obraze, adresa)?
        };
        h.bez_vystupu_predoslych = bez_vystupu_predoslych;
        h.pps_id = pps_id;

        // vstupné body podprúdov (dlaždice, WPP)
        h.bity_vstupnych_bodov = 0;
        if pps.dlazdice.is_some() || pps.wpp {
            let max = match (&pps.dlazdice, pps.wpp) {
                (Some(d), false) => d.stlpcov * d.riadkov - 1,
                (None, true) => vyska_ctb - 1,
                (Some(d), true) => d.stlpcov * vyska_ctb - 1,
                (None, false) => 0,
            };
            let n = c.ue_najviac(max, "num_entry_point_offsets")?;
            if n > 0 {
                let bity = c.ue_najviac(31, "offset_len_minus1")? + 1;
                h.bity_vstupnych_bodov = bity;
                h.vstupne_body = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    let v = c.u(bity)? as u64 + 1;
                    h.vstupne_body
                        .push(u32::try_from(v).map_err(|_| chyba("entry_point_offset mimo rozsahu"))?);
                }
            }
        }
        if pps.rozsirenie_hlavicky_rezu {
            let n = c.ue_najviac(256, "slice_segment_header_extension_length")?;
            c.preskoc(8 * n as usize)?;
        }
        // byte_alignment(): alignment_bit_equal_to_one a nuly
        if !c.bit()? {
            return Err(chyba("alignment_bit_equal_to_one = 0"));
        }
        while !c.je_zarovnany() {
            if c.bit()? {
                return Err(chyba("alignment_bit_equal_to_zero = 1"));
            }
        }
        h.dlzka_rbsp = c.pozicia() / 8;
        Ok(h)
    }

    /// Je rez I, P alebo B a koľko zoznamov referencií používa.
    pub fn zoznamov(&self) -> usize {
        match self.typ {
            TypRezu::I => 0,
            TypRezu::P => 1,
            TypRezu::B => 2,
        }
    }
}

/// Časť hlavičky nezávislého segmentu (od slice_reserved_flag po slice_loop_filter_across_slices_enabled_flag).
fn citaj_nezavislu(
    c: &mut Citac,
    nal: HlavickaNal,
    sps: &Sps,
    pps: &Pps,
    prvy_v_obraze: bool,
    adresa: u32,
) -> Vysledok<HlavickaRezu> {
    c.preskoc(pps.extra_bity_hlavicky as usize)?;
    let typ_rezu = match c.ue_najviac(2, "slice_type")? {
        0 => TypRezu::B,
        1 => TypRezu::P,
        _ => TypRezu::I,
    };
    if nal.je_irap() && typ_rezu != TypRezu::I {
        return Err(chyba("obraz IRAP s rezom P alebo B"));
    }
    let vystup = if pps.priznak_vystupu { c.bit()? } else { true };
    let rovina_farby = if sps.oddelene_roviny_farby { c.u(2)? as u8 } else { 0 };
    if rovina_farby > 2 {
        return Err(chyba("colour_plane_id > 2"));
    }

    let mut poc_lsb = 0;
    let mut sada_zo_sps = None;
    let mut sada = SadaReferencii::default();
    let mut bity_sady = 0;
    let mut delty_zdrojovej_sady = 0;
    let mut dlhodobe = Vec::new();
    let mut dlhodobe_zo_sps = 0;
    let mut bity_dlhodobych = 0;
    let mut tmvp = false;
    let max_obrazov = sps.podvrstvy.last().map_or(0, |p| p.max_dec_pic_buffering_minus1);
    if !nal.je_idr() {
        poc_lsb = c.u(sps.log2_max_poc_lsb)?;
        let pocet_sad = sps.sady_referencii.len();
        if !c.bit()? {
            let zaciatok = c.pozicia();
            let (s, zdroj) =
                SadaReferencii::citaj_so_zdrojom(c, pocet_sad, &sps.sady_referencii, pocet_sad, max_obrazov)?;
            bity_sady = (c.pozicia() - zaciatok) as u32;
            delty_zdrojovej_sady = zdroj.map_or(0, |i| sps.sady_referencii[i].pocet() as u32);
            sada = s;
        } else {
            if pocet_sad == 0 {
                return Err(chyba("short_term_ref_pic_set_sps_flag bez sád v SPS"));
            }
            let idx = c.u(bity_pre(pocet_sad as u32))? as usize;
            let s = sps.sady_referencii.get(idx).ok_or_else(|| chyba("short_term_ref_pic_set_idx mimo"))?;
            sada = s.clone();
            sada_zo_sps = Some(idx);
        }
        if let Some(lt_sps) = &sps.dlhodobe {
            let zaciatok = c.pozicia();
            let n_sps =
                if !lt_sps.is_empty() { c.ue_najviac(lt_sps.len() as u32, "num_long_term_sps")? } else { 0 };
            let n_pics = c.ue_najviac(16, "num_long_term_pics")?;
            if sada.pocet() as u32 + n_sps + n_pics > 16 {
                return Err(chyba("viac ako 16 referenčných obrazov"));
            }
            let mut cyklus = 0u32;
            for i in 0..n_sps + n_pics {
                let (poc_lsb_lt, pouzity) = if i < n_sps {
                    let idx = if lt_sps.len() > 1 { c.u(bity_pre(lt_sps.len() as u32))? } else { 0 };
                    *lt_sps.get(idx as usize).ok_or_else(|| chyba("lt_idx_sps mimo rozsahu"))?
                } else {
                    (c.u(sps.log2_max_poc_lsb)?, c.bit()?)
                };
                let msb_cyklus = if c.bit()? {
                    let d = c.ue_najviac(1 << (32 - sps.log2_max_poc_lsb), "delta_poc_msb_cycle_lt")?;
                    // DeltaPocMsbCycleLt sa sčítava v rámci skupiny zo SPS a skupiny z hlavičky (7-52)
                    cyklus = if i == 0 || i == n_sps { d } else { cyklus.saturating_add(d) };
                    Some(cyklus)
                } else {
                    if i == 0 || i == n_sps {
                        cyklus = 0;
                    }
                    None
                };
                dlhodobe.push(DlhodobyRef { poc_lsb: poc_lsb_lt, pouzity, msb_cyklus });
            }
            dlhodobe_zo_sps = n_sps;
            bity_dlhodobych = (c.pozicia() - zaciatok) as u32;
        }
        if sps.temporal_mvp {
            tmvp = c.bit()?;
        }
    }
    let (sao_jas, sao_farba) = if sps.sao {
        let j = c.bit()?;
        (j, sps.chroma_format_idc != 0 && c.bit()?)
    } else {
        (false, false)
    };

    let pocet_aktualnych = sada.dozadu.iter().filter(|r| r.1).count() as u32
        + sada.dopredu.iter().filter(|r| r.1).count() as u32
        + dlhodobe.iter().filter(|r| r.pouzity).count() as u32;

    let mut aktivne = [0u32; 2];
    let mut uprava_zoznamu = [None, None];
    let mut mvd_l1_nula = false;
    let mut cabac_init = false;
    let mut kolokovany_z_l0 = true;
    let mut kolokovany_ref = 0;
    let mut vahy = None;
    let mut max_zlucenie = 5;
    if typ_rezu != TypRezu::I {
        if pocet_aktualnych == 0 {
            return Err(chyba("rez P alebo B bez referenčných obrazov (NumPicTotalCurr = 0)"));
        }
        aktivne = [pps.ref_l0_minus1 + 1, if typ_rezu == TypRezu::B { pps.ref_l1_minus1 + 1 } else { 0 }];
        if c.bit()? {
            aktivne[0] = c.ue_najviac(14, "num_ref_idx_l0_active_minus1")? + 1;
            if typ_rezu == TypRezu::B {
                aktivne[1] = c.ue_najviac(14, "num_ref_idx_l1_active_minus1")? + 1;
            }
        }
        if pps.uprava_zoznamov && pocet_aktualnych > 1 {
            let bity = bity_pre(pocet_aktualnych);
            for (lx, uprava) in
                uprava_zoznamu.iter_mut().enumerate().take(if typ_rezu == TypRezu::B { 2 } else { 1 })
            {
                if c.bit()? {
                    let mut v = Vec::with_capacity(aktivne[lx] as usize);
                    for _ in 0..aktivne[lx] {
                        let e = c.u(bity)?;
                        if e >= pocet_aktualnych {
                            return Err(chyba("list_entry mimo rozsahu"));
                        }
                        v.push(e);
                    }
                    *uprava = Some(v);
                }
            }
        }
        if typ_rezu == TypRezu::B {
            mvd_l1_nula = c.bit()?;
        }
        if pps.cabac_init {
            cabac_init = c.bit()?;
        }
        if tmvp {
            if typ_rezu == TypRezu::B {
                kolokovany_z_l0 = c.bit()?;
            }
            let lx = if kolokovany_z_l0 { 0 } else { 1 };
            if aktivne[lx] > 1 {
                kolokovany_ref = c.ue_najviac(aktivne[lx] - 1, "collocated_ref_idx")?;
            }
        }
        if (pps.vazena_pred && typ_rezu == TypRezu::P) || (pps.vazena_bipred && typ_rezu == TypRezu::B) {
            vahy = Some(citaj_vahy(c, sps, &aktivne)?);
        }
        max_zlucenie = 5 - c.ue_najviac(4, "five_minus_max_num_merge_cand")?;
    }
    let qp_bd = 6 * (sps.bity_jasu as i32 - 8);
    let qp_delta = c.se_rozsah(-(26 + qp_bd + 51), 51 + 26 + qp_bd, "slice_qp_delta")?;
    let qp = pps.init_qp + qp_delta;
    if qp < -qp_bd || qp > 51 {
        return Err(chyba(format!("SliceQpY = {qp} mimo rozsahu")));
    }
    let (mut cb_qp_offset, mut cr_qp_offset) = (0, 0);
    if pps.chroma_qp_v_reze {
        cb_qp_offset = c.se_rozsah(-12, 12, "slice_cb_qp_offset")?;
        cr_qp_offset = c.se_rozsah(-12, 12, "slice_cr_qp_offset")?;
        if !(-12..=12).contains(&(pps.cb_qp_offset + cb_qp_offset))
            || !(-12..=12).contains(&(pps.cr_qp_offset + cr_qp_offset))
        {
            return Err(chyba("súčet posunov QP farby mimo −12 … 12"));
        }
    }
    let cu_chroma_qp_offset =
        pps.rozsirenie.as_ref().is_some_and(|r| r.chroma_qp_offset.is_some()) && c.bit()?;
    let pps_odb = pps.odblokovanie.unwrap_or_default();
    let (mut odblokovanie_vypnute, mut beta_div2, mut tc_div2) =
        (pps_odb.vypnute, pps_odb.beta_div2, pps_odb.tc_div2);
    if pps_odb.prepis_v_reze && c.bit()? {
        odblokovanie_vypnute = c.bit()?;
        if !odblokovanie_vypnute {
            beta_div2 = c.se_rozsah(-6, 6, "slice_beta_offset_div2")?;
            tc_div2 = c.se_rozsah(-6, 6, "slice_tc_offset_div2")?;
        } else {
            (beta_div2, tc_div2) = (0, 0);
        }
    }
    let filter_cez_rezy = if pps.filter_cez_rezy && (sao_jas || sao_farba || !odblokovanie_vypnute) {
        c.bit()?
    } else {
        pps.filter_cez_rezy
    };

    Ok(HlavickaRezu {
        nal,
        prvy_v_obraze,
        bez_vystupu_predoslych: false,
        pps_id: pps.id,
        zavisly: false,
        adresa,
        adresa_rezu: adresa,
        typ: typ_rezu,
        vystup,
        rovina_farby,
        poc_lsb,
        sada_zo_sps,
        sada,
        bity_sady,
        delty_zdrojovej_sady,
        dlhodobe,
        dlhodobe_zo_sps,
        bity_dlhodobych,
        tmvp,
        sao_jas,
        sao_farba,
        aktivne,
        uprava_zoznamu,
        mvd_l1_nula,
        cabac_init,
        kolokovany_z_l0,
        kolokovany_ref,
        vahy,
        max_zlucenie,
        qp_delta,
        qp,
        cb_qp_offset,
        cr_qp_offset,
        cu_chroma_qp_offset,
        odblokovanie_vypnute,
        beta_div2,
        tc_div2,
        filter_cez_rezy,
        vstupne_body: Vec::new(),
        bity_vstupnych_bodov: 0,
        dlzka_rbsp: 0,
        pocet_aktualnych,
    })
}

/// pred_weight_table() (7.3.6.3). Pri jednej vrstve bez obsahu obrazovky (SCC) nemá referenčný obraz rovnaké POC
/// ako aktuálny, preto sú príznaky vždy prítomné.
fn citaj_vahy(c: &mut Citac, sps: &Sps, aktivne: &[u32; 2]) -> Vysledok<TabulkaVah> {
    let log2_jas = c.ue_najviac(7, "luma_log2_weight_denom")?;
    let farba = sps.chroma_format_idc != 0;
    let log2_farba = if farba {
        let d = c.se_rozsah(-(log2_jas as i32), 7 - log2_jas as i32, "delta_chroma_log2_weight_denom")?;
        (log2_jas as i32 + d) as u32
    } else {
        0
    };
    let mut t = TabulkaVah { log2_jas, log2_farba, zoznamy: [Vec::new(), Vec::new()] };
    for (lx, &aktivnych) in aktivne.iter().enumerate() {
        let n = aktivnych as usize;
        if n == 0 {
            continue;
        }
        let mut priznak_jas = [false; 16];
        let mut priznak_farba = [false; 16];
        for p in priznak_jas.iter_mut().take(n) {
            *p = c.bit()?;
        }
        if farba {
            for p in priznak_farba.iter_mut().take(n) {
                *p = c.bit()?;
            }
        }
        for i in 0..n {
            let mut v = VahaReferencie::default();
            if priznak_jas[i] {
                v.jas = Some((
                    c.se_rozsah(-128, 127, "delta_luma_weight")?,
                    c.se_rozsah(-128, 127, "luma_offset")?,
                ));
            }
            if priznak_farba[i] {
                let mut f = [(0, 0); 2];
                for fj in f.iter_mut() {
                    *fj = (
                        c.se_rozsah(-128, 127, "delta_chroma_weight")?,
                        c.se_rozsah(-512, 511, "delta_chroma_offset")?,
                    );
                }
                v.farba = Some(f);
            }
            t.zoznamy[lx].push(v);
        }
    }
    Ok(t)
}
