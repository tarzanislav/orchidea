//! Parametre pre hardvérové dekodéry HEVC *stateless* cez V4L2 (RPi 4/400/5 – ovládač `rpi-hevc-dec`) –
//! krok H1, rozhranie R1 (`ORCHIDEA/rozhrania.md`).
//!
//! Štruktúry sú **neutrálne** (bez `#[repr(C)]` a bez závislosti od Linuxu): polia majú presne mená polí
//! ovládacích prvkov jadra Linux (`include/uapi/linux/v4l2-controls.h`):
//! `V4L2_CID_STATELESS_HEVC_SPS` → [`SpsV4l2`], `_PPS` → [`PpsV4l2`], `_SCALING_MATRIX` → [`MaticaV4l2`],
//! `_DECODE_PARAMS` → [`DekodovanieV4l2`], `_SLICE_PARAMS` → [`RezV4l2`], `_ENTRY_POINT_OFFSETS` →
//! [`RezV4l2::entry_point_offset_minus1`]. Klient V4L2 (relácia A, balík `media-hw`) ich prevedie na štruktúry
//! jadra 1 : 1 a overí rozloženie voči hlavičke jadra, s ktorým sa zostavuje.
//!
//! [`PripravaV4l2`] prijíma jednotky NAL v poradí toku a vracia obrazy pripravené na dekódovanie a udalosti
//! výstupu – klient už nerobí žiadny rozbor syntaxe ani správu referencií.

use crate::nal::{HlavickaNal, rbsp, typ};
use crate::obrazy::{Referencia, SpravaObrazov, Udalost};
use crate::parametre::{MaticeKvantovania, Pps, Sps};
use crate::rez::{HlavickaRezu, TypRezu};
use crate::sady::Sady;
use orchidea_jadro::{Chyba, Vysledok};

/// Najviac obrazov v pamäti dekodéra (V4L2_HEVC_DPB_ENTRIES_NUM_MAX).
pub const DPB_MAX: usize = 16;

// Príznaky V4L2_HEVC_SPS_FLAG_*
pub const SPS_FLAG_SEPARATE_COLOUR_PLANE: u64 = 1 << 0;
pub const SPS_FLAG_SCALING_LIST_ENABLED: u64 = 1 << 1;
pub const SPS_FLAG_AMP_ENABLED: u64 = 1 << 2;
pub const SPS_FLAG_SAMPLE_ADAPTIVE_OFFSET: u64 = 1 << 3;
pub const SPS_FLAG_PCM_ENABLED: u64 = 1 << 4;
pub const SPS_FLAG_PCM_LOOP_FILTER_DISABLED: u64 = 1 << 5;
pub const SPS_FLAG_LONG_TERM_REF_PICS_PRESENT: u64 = 1 << 6;
pub const SPS_FLAG_SPS_TEMPORAL_MVP_ENABLED: u64 = 1 << 7;
pub const SPS_FLAG_STRONG_INTRA_SMOOTHING_ENABLED: u64 = 1 << 8;

// Príznaky V4L2_HEVC_PPS_FLAG_*
pub const PPS_FLAG_DEPENDENT_SLICE_SEGMENT_ENABLED: u64 = 1 << 0;
pub const PPS_FLAG_OUTPUT_FLAG_PRESENT: u64 = 1 << 1;
pub const PPS_FLAG_SIGN_DATA_HIDING_ENABLED: u64 = 1 << 2;
pub const PPS_FLAG_CABAC_INIT_PRESENT: u64 = 1 << 3;
pub const PPS_FLAG_CONSTRAINED_INTRA_PRED: u64 = 1 << 4;
pub const PPS_FLAG_TRANSFORM_SKIP_ENABLED: u64 = 1 << 5;
pub const PPS_FLAG_CU_QP_DELTA_ENABLED: u64 = 1 << 6;
pub const PPS_FLAG_PPS_SLICE_CHROMA_QP_OFFSETS_PRESENT: u64 = 1 << 7;
pub const PPS_FLAG_WEIGHTED_PRED: u64 = 1 << 8;
pub const PPS_FLAG_WEIGHTED_BIPRED: u64 = 1 << 9;
pub const PPS_FLAG_TRANSQUANT_BYPASS_ENABLED: u64 = 1 << 10;
pub const PPS_FLAG_TILES_ENABLED: u64 = 1 << 11;
pub const PPS_FLAG_ENTROPY_CODING_SYNC_ENABLED: u64 = 1 << 12;
pub const PPS_FLAG_LOOP_FILTER_ACROSS_TILES_ENABLED: u64 = 1 << 13;
pub const PPS_FLAG_PPS_LOOP_FILTER_ACROSS_SLICES_ENABLED: u64 = 1 << 14;
pub const PPS_FLAG_DEBLOCKING_FILTER_OVERRIDE_ENABLED: u64 = 1 << 15;
pub const PPS_FLAG_PPS_DISABLE_DEBLOCKING_FILTER: u64 = 1 << 16;
pub const PPS_FLAG_LISTS_MODIFICATION_PRESENT: u64 = 1 << 17;
pub const PPS_FLAG_SLICE_SEGMENT_HEADER_EXTENSION_PRESENT: u64 = 1 << 18;
pub const PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT: u64 = 1 << 19;
pub const PPS_FLAG_UNIFORM_SPACING: u64 = 1 << 20;

// Príznaky V4L2_HEVC_SLICE_PARAMS_FLAG_*
pub const SLICE_FLAG_SLICE_SAO_LUMA: u64 = 1 << 0;
pub const SLICE_FLAG_SLICE_SAO_CHROMA: u64 = 1 << 1;
pub const SLICE_FLAG_SLICE_TEMPORAL_MVP_ENABLED: u64 = 1 << 2;
pub const SLICE_FLAG_MVD_L1_ZERO: u64 = 1 << 3;
pub const SLICE_FLAG_CABAC_INIT: u64 = 1 << 4;
pub const SLICE_FLAG_COLLOCATED_FROM_L0: u64 = 1 << 5;
pub const SLICE_FLAG_USE_INTEGER_MV: u64 = 1 << 6;
pub const SLICE_FLAG_SLICE_DEBLOCKING_FILTER_DISABLED: u64 = 1 << 7;
pub const SLICE_FLAG_SLICE_LOOP_FILTER_ACROSS_SLICES_ENABLED: u64 = 1 << 8;
pub const SLICE_FLAG_DEPENDENT_SLICE_SEGMENT: u64 = 1 << 9;

// Príznaky V4L2_HEVC_DECODE_PARAM_FLAG_* a V4L2_HEVC_DPB_ENTRY_*
pub const DECODE_FLAG_IRAP_PIC: u64 = 0x1;
pub const DECODE_FLAG_IDR_PIC: u64 = 0x2;
pub const DECODE_FLAG_NO_OUTPUT_OF_PRIOR: u64 = 0x4;
pub const DPB_ENTRY_LONG_TERM_REFERENCE: u8 = 0x01;

/// `struct v4l2_ctrl_hevc_sps` – hodnoty zo SPS (najvyššia podvrstva pre sps_max_*).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SpsV4l2 {
    pub video_parameter_set_id: u8,
    pub seq_parameter_set_id: u8,
    pub pic_width_in_luma_samples: u16,
    pub pic_height_in_luma_samples: u16,
    pub bit_depth_luma_minus8: u8,
    pub bit_depth_chroma_minus8: u8,
    pub log2_max_pic_order_cnt_lsb_minus4: u8,
    pub sps_max_dec_pic_buffering_minus1: u8,
    pub sps_max_num_reorder_pics: u8,
    pub sps_max_latency_increase_plus1: u8,
    pub log2_min_luma_coding_block_size_minus3: u8,
    pub log2_diff_max_min_luma_coding_block_size: u8,
    pub log2_min_luma_transform_block_size_minus2: u8,
    pub log2_diff_max_min_luma_transform_block_size: u8,
    pub max_transform_hierarchy_depth_inter: u8,
    pub max_transform_hierarchy_depth_intra: u8,
    pub pcm_sample_bit_depth_luma_minus1: u8,
    pub pcm_sample_bit_depth_chroma_minus1: u8,
    pub log2_min_pcm_luma_coding_block_size_minus3: u8,
    pub log2_diff_max_min_pcm_luma_coding_block_size: u8,
    pub num_short_term_ref_pic_sets: u8,
    pub num_long_term_ref_pics_sps: u8,
    pub chroma_format_idc: u8,
    pub sps_max_sub_layers_minus1: u8,
    pub flags: u64,
}

/// `struct v4l2_ctrl_hevc_pps`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PpsV4l2 {
    pub pic_parameter_set_id: u8,
    pub num_extra_slice_header_bits: u8,
    pub num_ref_idx_l0_default_active_minus1: u8,
    pub num_ref_idx_l1_default_active_minus1: u8,
    pub init_qp_minus26: i8,
    pub diff_cu_qp_delta_depth: u8,
    pub pps_cb_qp_offset: i8,
    pub pps_cr_qp_offset: i8,
    pub num_tile_columns_minus1: u8,
    pub num_tile_rows_minus1: u8,
    pub column_width_minus1: [u8; 20],
    pub row_height_minus1: [u8; 22],
    pub pps_beta_offset_div2: i8,
    pub pps_tc_offset_div2: i8,
    pub log2_parallel_merge_level_minus2: u8,
    pub flags: u64,
}

/// `struct v4l2_ctrl_hevc_scaling_matrix` – hodnoty v **rastri** (V4L2 ich žiada v rastri, nie v poradí prechodu);
/// 32×32: index 0 = vnútrosnímková jas (matrixId 0), 1 = medzisnímková jas (matrixId 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaticaV4l2 {
    pub scaling_list_4x4: [[u8; 16]; 6],
    pub scaling_list_8x8: [[u8; 64]; 6],
    pub scaling_list_16x16: [[u8; 64]; 6],
    pub scaling_list_32x32: [[u8; 64]; 2],
    pub scaling_list_dc_coef_16x16: [u8; 6],
    pub scaling_list_dc_coef_32x32: [u8; 2],
}

/// `struct v4l2_hevc_dpb_entry`. `timestamp` = identifikátor obrazu zo [`SpravaObrazov`]; klient ho môže priamo
/// použiť ako časovú značku vyrovnávacej pamäte (časová značka OUTPUT buffera sa prenesie na CAPTURE buffer).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DpbV4l2 {
    pub timestamp: u64,
    pub flags: u8,
    pub field_pic: u8,
    pub pic_order_cnt_val: i32,
}

/// `struct v4l2_ctrl_hevc_decode_params`. `poc_st_curr_before` a ďalšie sú **indexy do `dpb`**.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DekodovanieV4l2 {
    pub pic_order_cnt_val: i32,
    pub short_term_ref_pic_set_size: u16,
    pub long_term_ref_pic_set_size: u16,
    pub num_active_dpb_entries: u8,
    pub num_poc_st_curr_before: u8,
    pub num_poc_st_curr_after: u8,
    pub num_poc_lt_curr: u8,
    pub poc_st_curr_before: [u8; DPB_MAX],
    pub poc_st_curr_after: [u8; DPB_MAX],
    pub poc_lt_curr: [u8; DPB_MAX],
    pub num_delta_pocs_of_ref_rps_idx: u8,
    pub dpb: [DpbV4l2; DPB_MAX],
    pub flags: u64,
}

/// `struct v4l2_hevc_pred_weight_table`: delta váh ako v syntaxi, `luma_offset_lX` ako v syntaxi,
/// `chroma_offset_lX` = odvodené ChromaOffsetLX (7.4.7.3, rozsah −128 … 127).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VahyV4l2 {
    pub delta_luma_weight_l0: [i8; DPB_MAX],
    pub luma_offset_l0: [i8; DPB_MAX],
    pub delta_chroma_weight_l0: [[i8; 2]; DPB_MAX],
    pub chroma_offset_l0: [[i8; 2]; DPB_MAX],
    pub delta_luma_weight_l1: [i8; DPB_MAX],
    pub luma_offset_l1: [i8; DPB_MAX],
    pub delta_chroma_weight_l1: [[i8; 2]; DPB_MAX],
    pub chroma_offset_l1: [[i8; 2]; DPB_MAX],
    pub luma_log2_weight_denom: u8,
    pub delta_chroma_log2_weight_denom: i8,
}

/// `struct v4l2_ctrl_hevc_slice_params` a vstupné body segmentu.
///
/// - `bit_size` = veľkosť celej jednotky NAL segmentu v bitoch (vrátane 2-bajtovej hlavičky NAL a ochranných
///   bajtov, bez štartovacieho kódu),
/// - `data_byte_offset` = posun od začiatku jednotky NAL (hlavičky NAL) po prvý bajt slice_segment_data(),
///   v bajtoch vrátane ochranných bajtov; ak klient pred NAL vloží štartovací kód (Annex B), pripočíta jeho dĺžku,
/// - `slice_pic_order_cnt` = PicOrderCntVal aktuálneho obrazu,
/// - `ref_idx_l0/l1` = indexy do `DekodovanieV4l2::dpb`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RezV4l2 {
    pub bit_size: u32,
    pub data_byte_offset: u32,
    pub num_entry_point_offsets: u32,
    pub nal_unit_type: u8,
    pub nuh_temporal_id_plus1: u8,
    pub slice_type: u8,
    pub colour_plane_id: u8,
    pub slice_pic_order_cnt: i32,
    pub num_ref_idx_l0_active_minus1: u8,
    pub num_ref_idx_l1_active_minus1: u8,
    pub collocated_ref_idx: u8,
    pub five_minus_max_num_merge_cand: u8,
    pub slice_qp_delta: i8,
    pub slice_cb_qp_offset: i8,
    pub slice_cr_qp_offset: i8,
    pub slice_act_y_qp_offset: i8,
    pub slice_act_cb_qp_offset: i8,
    pub slice_act_cr_qp_offset: i8,
    pub slice_beta_offset_div2: i8,
    pub slice_tc_offset_div2: i8,
    pub pic_struct: u8,
    pub slice_segment_addr: u32,
    pub ref_idx_l0: [u8; DPB_MAX],
    pub ref_idx_l1: [u8; DPB_MAX],
    pub short_term_ref_pic_set_size: u16,
    pub long_term_ref_pic_set_size: u16,
    pub pred_weight_table: VahyV4l2,
    pub flags: u64,
    /// V4L2_CID_STATELESS_HEVC_ENTRY_POINT_OFFSETS: entry_point_offset_minus1[i]
    pub entry_point_offset_minus1: Vec<u32>,
}

/// Všetko, čo hardvérový dekodér potrebuje na jeden obraz (rozhranie R1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParametreDekodovania {
    pub sps: SpsV4l2,
    pub pps: PpsV4l2,
    /// `None` = scaling_list_enabled_flag = 0 (ovládací prvok sa neposiela)
    pub matica: Option<MaticaV4l2>,
    pub dekodovanie: DekodovanieV4l2,
    /// segmenty rezov obrazu v poradí toku
    pub rezy: Vec<RezV4l2>,
}

fn u8_z(v: u32) -> u8 {
    v.min(255) as u8
}

/// SPS → `v4l2_ctrl_hevc_sps`.
pub fn sps_v4l2(s: &Sps) -> SpsV4l2 {
    let p = s.podvrstvy.last().copied().unwrap_or_default();
    let mut f = 0;
    let mut priznak = |podmienka: bool, b: u64| {
        if podmienka {
            f |= b;
        }
    };
    priznak(s.oddelene_roviny_farby, SPS_FLAG_SEPARATE_COLOUR_PLANE);
    priznak(s.matice.is_some(), SPS_FLAG_SCALING_LIST_ENABLED);
    priznak(s.amp, SPS_FLAG_AMP_ENABLED);
    priznak(s.sao, SPS_FLAG_SAMPLE_ADAPTIVE_OFFSET);
    priznak(s.pcm.is_some(), SPS_FLAG_PCM_ENABLED);
    priznak(s.pcm.is_some_and(|p| p.bez_filtra), SPS_FLAG_PCM_LOOP_FILTER_DISABLED);
    priznak(s.dlhodobe.is_some(), SPS_FLAG_LONG_TERM_REF_PICS_PRESENT);
    priznak(s.temporal_mvp, SPS_FLAG_SPS_TEMPORAL_MVP_ENABLED);
    priznak(s.strong_intra_smoothing, SPS_FLAG_STRONG_INTRA_SMOOTHING_ENABLED);
    let pcm = s.pcm.unwrap_or(crate::parametre::Pcm {
        bity_jasu: 1,
        bity_farby: 1,
        log2_min: 3,
        log2_max: 3,
        bez_filtra: false,
    });
    SpsV4l2 {
        video_parameter_set_id: s.vps_id,
        seq_parameter_set_id: u8_z(s.id),
        pic_width_in_luma_samples: s.sirka as u16,
        pic_height_in_luma_samples: s.vyska as u16,
        bit_depth_luma_minus8: u8_z(s.bity_jasu - 8),
        bit_depth_chroma_minus8: u8_z(s.bity_farby - 8),
        log2_max_pic_order_cnt_lsb_minus4: u8_z(s.log2_max_poc_lsb - 4),
        sps_max_dec_pic_buffering_minus1: u8_z(p.max_dec_pic_buffering_minus1),
        sps_max_num_reorder_pics: u8_z(p.max_num_reorder_pics),
        sps_max_latency_increase_plus1: u8_z(p.max_latency_increase_plus1),
        log2_min_luma_coding_block_size_minus3: u8_z(s.log2_min_cb - 3),
        log2_diff_max_min_luma_coding_block_size: u8_z(s.log2_ctb - s.log2_min_cb),
        log2_min_luma_transform_block_size_minus2: u8_z(s.log2_min_tb - 2),
        log2_diff_max_min_luma_transform_block_size: u8_z(s.log2_max_tb - s.log2_min_tb),
        max_transform_hierarchy_depth_inter: u8_z(s.max_hlbka_inter),
        max_transform_hierarchy_depth_intra: u8_z(s.max_hlbka_intra),
        pcm_sample_bit_depth_luma_minus1: if s.pcm.is_some() { pcm.bity_jasu - 1 } else { 0 },
        pcm_sample_bit_depth_chroma_minus1: if s.pcm.is_some() { pcm.bity_farby - 1 } else { 0 },
        log2_min_pcm_luma_coding_block_size_minus3: if s.pcm.is_some() { u8_z(pcm.log2_min - 3) } else { 0 },
        log2_diff_max_min_pcm_luma_coding_block_size: if s.pcm.is_some() {
            u8_z(pcm.log2_max - pcm.log2_min)
        } else {
            0
        },
        num_short_term_ref_pic_sets: u8_z(s.sady_referencii.len() as u32),
        num_long_term_ref_pics_sps: u8_z(s.dlhodobe.as_ref().map_or(0, |v| v.len() as u32)),
        chroma_format_idc: u8_z(s.chroma_format_idc),
        sps_max_sub_layers_minus1: u8_z(s.max_podvrstiev_minus1),
        flags: f,
    }
}

/// PPS → `v4l2_ctrl_hevc_pps` (šírky stĺpcov a výšky riadkov dlaždíc len pri nerovnomernom rozdelení).
pub fn pps_v4l2(p: &Pps) -> PpsV4l2 {
    let mut f = 0;
    let mut priznak = |podmienka: bool, b: u64| {
        if podmienka {
            f |= b;
        }
    };
    let odb = p.odblokovanie.unwrap_or_default();
    priznak(p.zavisle_rezy, PPS_FLAG_DEPENDENT_SLICE_SEGMENT_ENABLED);
    priznak(p.priznak_vystupu, PPS_FLAG_OUTPUT_FLAG_PRESENT);
    priznak(p.skryvanie_znamienok, PPS_FLAG_SIGN_DATA_HIDING_ENABLED);
    priznak(p.cabac_init, PPS_FLAG_CABAC_INIT_PRESENT);
    priznak(p.obmedzena_intra, PPS_FLAG_CONSTRAINED_INTRA_PRED);
    priznak(p.transform_skip, PPS_FLAG_TRANSFORM_SKIP_ENABLED);
    priznak(p.cu_qp_delta.is_some(), PPS_FLAG_CU_QP_DELTA_ENABLED);
    priznak(p.chroma_qp_v_reze, PPS_FLAG_PPS_SLICE_CHROMA_QP_OFFSETS_PRESENT);
    priznak(p.vazena_pred, PPS_FLAG_WEIGHTED_PRED);
    priznak(p.vazena_bipred, PPS_FLAG_WEIGHTED_BIPRED);
    priznak(p.transquant_bypass, PPS_FLAG_TRANSQUANT_BYPASS_ENABLED);
    priznak(p.dlazdice.is_some(), PPS_FLAG_TILES_ENABLED);
    priznak(p.wpp, PPS_FLAG_ENTROPY_CODING_SYNC_ENABLED);
    priznak(
        p.dlazdice.as_ref().is_some_and(|d| d.filter_cez_dlazdice),
        PPS_FLAG_LOOP_FILTER_ACROSS_TILES_ENABLED,
    );
    priznak(p.filter_cez_rezy, PPS_FLAG_PPS_LOOP_FILTER_ACROSS_SLICES_ENABLED);
    priznak(odb.prepis_v_reze, PPS_FLAG_DEBLOCKING_FILTER_OVERRIDE_ENABLED);
    priznak(odb.vypnute, PPS_FLAG_PPS_DISABLE_DEBLOCKING_FILTER);
    priznak(p.uprava_zoznamov, PPS_FLAG_LISTS_MODIFICATION_PRESENT);
    priznak(p.rozsirenie_hlavicky_rezu, PPS_FLAG_SLICE_SEGMENT_HEADER_EXTENSION_PRESENT);
    priznak(p.odblokovanie.is_some(), PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT);
    let mut v = PpsV4l2 {
        pic_parameter_set_id: u8_z(p.id),
        num_extra_slice_header_bits: u8_z(p.extra_bity_hlavicky),
        num_ref_idx_l0_default_active_minus1: u8_z(p.ref_l0_minus1),
        num_ref_idx_l1_default_active_minus1: u8_z(p.ref_l1_minus1),
        init_qp_minus26: (p.init_qp - 26) as i8,
        diff_cu_qp_delta_depth: u8_z(p.cu_qp_delta.unwrap_or(0)),
        pps_cb_qp_offset: p.cb_qp_offset as i8,
        pps_cr_qp_offset: p.cr_qp_offset as i8,
        pps_beta_offset_div2: odb.beta_div2 as i8,
        pps_tc_offset_div2: odb.tc_div2 as i8,
        log2_parallel_merge_level_minus2: u8_z(p.log2_parallel_merge_level - 2),
        ..Default::default()
    };
    if let Some(d) = &p.dlazdice {
        v.num_tile_columns_minus1 = u8_z(d.stlpcov - 1);
        v.num_tile_rows_minus1 = u8_z(d.riadkov - 1);
        match (&d.sirky, &d.vysky) {
            (Some(s), Some(r)) => {
                for (i, w) in s.iter().enumerate().take(20) {
                    v.column_width_minus1[i] = u8_z(w - 1);
                }
                for (i, h) in r.iter().enumerate().take(22) {
                    v.row_height_minus1[i] = u8_z(h - 1);
                }
            }
            _ => f |= PPS_FLAG_UNIFORM_SPACING,
        }
    }
    v.flags = f;
    v
}

/// Diagonálny prechod nahor doprava pre blok `n` × `n` (6.5.3): poloha i → (x, y).
pub fn diagonalny_prechod(n: usize) -> Vec<(usize, usize)> {
    let mut v = Vec::with_capacity(n * n);
    let (mut x, mut y) = (0i32, 0i32);
    while v.len() < n * n {
        while y >= 0 {
            if (x as usize) < n && (y as usize) < n {
                v.push((x as usize, y as usize));
            }
            y -= 1;
            x += 1;
        }
        y = x;
        x = 0;
    }
    v
}

/// Matice kvantovania (platné pre obraz: PPS má prednosť pred SPS) → `v4l2_ctrl_hevc_scaling_matrix`.
pub fn matica_v4l2(sps: &Sps, pps: &Pps) -> Option<MaticaV4l2> {
    sps.matice.as_ref()?;
    let m: &MaticeKvantovania = pps.matice.as_ref().or(sps.matice.as_ref())?;
    let d4 = diagonalny_prechod(4);
    let d8 = diagonalny_prechod(8);
    let rozbal4 = |z: &[u8]| -> [u8; 16] {
        let mut r = [16u8; 16];
        for (i, &(x, y)) in d4.iter().enumerate() {
            r[y * 4 + x] = z[i];
        }
        r
    };
    let rozbal8 = |z: &[u8]| -> [u8; 64] {
        let mut r = [16u8; 64];
        for (i, &(x, y)) in d8.iter().enumerate() {
            r[y * 8 + x] = z[i];
        }
        r
    };
    Some(MaticaV4l2 {
        scaling_list_4x4: std::array::from_fn(|i| rozbal4(&m.zoznamy[0][i])),
        scaling_list_8x8: std::array::from_fn(|i| rozbal8(&m.zoznamy[1][i])),
        scaling_list_16x16: std::array::from_fn(|i| rozbal8(&m.zoznamy[2][i])),
        scaling_list_32x32: [rozbal8(&m.zoznamy[3][0]), rozbal8(&m.zoznamy[3][3])],
        scaling_list_dc_coef_16x16: m.dc[0],
        scaling_list_dc_coef_32x32: [m.dc[1][0], m.dc[1][3]],
    })
}

/// Posun v bajtoch s ochrannými bajtmi (escaped) pre posun `rbsp_posun` v RBSP; `telo` je obsah jednotky NAL
/// za 2-bajtovou hlavičkou (s ochrannými bajtmi).
pub fn posun_s_ochranou(telo: &[u8], rbsp_posun: usize) -> usize {
    let mut nuly = 0;
    let mut r = 0;
    for (i, &b) in telo.iter().enumerate() {
        if nuly >= 2 && b == 3 {
            nuly = 0;
            continue;
        }
        if r == rbsp_posun {
            return i;
        }
        nuly = if b == 0 { nuly + 1 } else { 0 };
        r += 1;
    }
    telo.len()
}

/// Obraz pripravený na hardvérové dekódovanie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObrazV4l2 {
    /// identifikátor obrazu (= `timestamp` v `dpb` neskorších obrazov)
    pub id: u32,
    pub parametre: ParametreDekodovania,
    /// jednotky NAL segmentov rezov (2-bajtová hlavička NAL + telo s ochrannými bajtmi, bez štartovacieho kódu)
    pub rezy_nal: Vec<Vec<u8>>,
}

/// Udalosť pre klienta V4L2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UdalostV4l2 {
    /// dekódovať obraz (poslať ovládacie prvky a rezy jednou požiadavkou request API)
    Dekoduj(Box<ObrazV4l2>),
    /// chýbajúci referenčný obraz (poškodený tok) – klient pripraví vyrovnávaciu pamäť (sivý obraz) s týmto id
    Vytvor { id: u32, poc: i32 },
    /// vydať obraz `id` na displej (v poradí zobrazenia)
    Vystup(u32),
    /// vyrovnávacia pamäť obrazu `id` sa smie použiť znova
    Uvolni(u32),
}

impl From<Udalost> for UdalostV4l2 {
    fn from(u: Udalost) -> Self {
        match u {
            Udalost::Vytvor { id, poc } => UdalostV4l2::Vytvor { id, poc },
            Udalost::Vystup(id) => UdalostV4l2::Vystup(id),
            Udalost::Uvolni(id) => UdalostV4l2::Uvolni(id),
        }
    }
}

/// Príprava ovládacích prvkov V4L2 z toku jednotiek NAL.
#[derive(Debug, Clone, Default)]
pub struct PripravaV4l2 {
    pub sady: Sady,
    pub sprava: SpravaObrazov,
    aktualny: Option<ObrazV4l2>,
    /// SPS aktuálneho obrazu (pre ukončenie obrazu)
    sps_obrazu: Option<Sps>,
    nezavisla: Option<HlavickaRezu>,
    dpb_id: Vec<u32>,
    /// obraz sa preskakuje (RASL po náhodnom prístupe)
    preskakuje: bool,
}

impl PripravaV4l2 {
    pub fn nova() -> PripravaV4l2 {
        PripravaV4l2 { sady: Sady::nove(), ..Default::default() }
    }

    fn ukonci(&mut self, udalosti: &mut Vec<UdalostV4l2>) {
        if let Some(o) = self.aktualny.take() {
            udalosti.push(UdalostV4l2::Dekoduj(Box::new(o)));
            if let Some(sps) = &self.sps_obrazu {
                udalosti.extend(self.sprava.ukonci_obraz(sps).into_iter().map(UdalostV4l2::from));
            }
        }
        self.nezavisla = None;
    }

    /// Spracuje jednu jednotku NAL (bez štartovacieho kódu, s hlavičkou NAL).
    pub fn nal(&mut self, nal: &[u8]) -> Vysledok<Vec<UdalostV4l2>> {
        let mut udalosti = Vec::new();
        let h = HlavickaNal::citaj(nal)?;
        if h.vrstva != 0 {
            return Ok(udalosti);
        }
        let telo = &nal[2..];
        match h.typ {
            typ::VPS | typ::SPS | typ::PPS => {
                self.sady.pridaj(&h, &rbsp(telo))?;
            }
            typ::EOS => {
                self.ukonci(&mut udalosti);
                self.sprava.koniec_sekvencie();
            }
            typ::EOB => {
                self.ukonci(&mut udalosti);
                udalosti.extend(self.sprava.koniec_toku().into_iter().map(UdalostV4l2::from));
            }
            typ::AUD => self.ukonci(&mut udalosti),
            t if t <= 9 || (16..=21).contains(&t) => {
                // RBSP stačí po koniec hlavičky (aj s tisíckami vstupných bodov je kratšia ako 64 KiB)
                let r = rbsp(&telo[..telo.len().min(1 << 16)]);
                let prvy = r.first().is_some_and(|b| b & 0x80 != 0);
                if prvy {
                    self.ukonci(&mut udalosti);
                }
                let rez = HlavickaRezu::citaj(&r, h, &self.sady, self.nezavisla.as_ref())?;
                let (sps, pps) = self.sady.pre_rez(rez.pps_id)?;
                let (sps, pps) = (sps.clone(), pps.clone());
                if prvy {
                    let z = self.sprava.zacni_obraz(&rez, &sps)?;
                    udalosti.extend(z.udalosti.iter().copied().map(UdalostV4l2::from));
                    self.preskakuje = z.preskocit;
                    if !z.preskocit {
                        self.zacni_obraz(
                            &rez,
                            &sps,
                            &pps,
                            z.id,
                            z.poc,
                            z.irap_bez_rasl,
                            z.bez_vystupu_predoslych,
                        );
                        self.sps_obrazu = Some(sps.clone());
                    }
                } else if self.aktualny.is_none() && !self.preskakuje {
                    return Err(Chyba::PoskodeneData("HEVC: segment rezu bez začiatku obrazu".into()));
                }
                if !self.preskakuje {
                    let v = self.rez_v4l2(&rez, telo, nal.len())?;
                    if let Some(o) = &mut self.aktualny {
                        o.parametre.rezy.push(v);
                        o.rezy_nal.push(nal.to_vec());
                    }
                }
                if !rez.zavisly {
                    self.nezavisla = Some(rez);
                }
            }
            _ => {}
        }
        Ok(udalosti)
    }

    /// Koniec toku: posledný obraz na dekódovanie a vydanie všetkých zostávajúcich obrazov.
    pub fn koniec(&mut self) -> Vec<UdalostV4l2> {
        let mut udalosti = Vec::new();
        self.ukonci(&mut udalosti);
        udalosti.extend(self.sprava.vyprazdni().into_iter().map(UdalostV4l2::from));
        udalosti
    }

    #[allow(clippy::too_many_arguments)]
    fn zacni_obraz(
        &mut self,
        rez: &HlavickaRezu,
        sps: &Sps,
        pps: &Pps,
        id: u32,
        poc: i32,
        irap_bez_rasl: bool,
        bez_vystupu: bool,
    ) {
        let mut d = DekodovanieV4l2 {
            pic_order_cnt_val: poc,
            short_term_ref_pic_set_size: rez.bity_sady as u16,
            long_term_ref_pic_set_size: rez.bity_dlhodobych as u16,
            num_delta_pocs_of_ref_rps_idx: if rez.sada_zo_sps.is_none() {
                rez.delty_zdrojovej_sady as u8
            } else {
                0
            },
            ..Default::default()
        };
        if rez.nal.je_irap() {
            d.flags |= DECODE_FLAG_IRAP_PIC;
        }
        if rez.nal.je_idr() {
            d.flags |= DECODE_FLAG_IDR_PIC;
        }
        if irap_bez_rasl && bez_vystupu {
            d.flags |= DECODE_FLAG_NO_OUTPUT_OF_PRIOR;
        }
        // DPB: všetky referenčné obrazy v pamäti
        self.dpb_id.clear();
        for o in self.sprava.obrazy().iter().filter(|o| o.referencia != Referencia::Nie).take(DPB_MAX) {
            let i = self.dpb_id.len();
            d.dpb[i] = DpbV4l2 {
                timestamp: o.id as u64,
                flags: if o.referencia == Referencia::Dlhodoba { DPB_ENTRY_LONG_TERM_REFERENCE } else { 0 },
                field_pic: 0,
                pic_order_cnt_val: o.poc,
            };
            self.dpb_id.push(o.id);
        }
        d.num_active_dpb_entries = self.dpb_id.len() as u8;
        let index = |dpb: &[u32], id: u32| dpb.iter().position(|&x| x == id).unwrap_or(0) as u8;
        let s = &self.sprava.sady;
        for (i, &id) in s.st_pred.iter().enumerate().take(DPB_MAX) {
            d.poc_st_curr_before[i] = index(&self.dpb_id, id);
        }
        for (i, &id) in s.st_po.iter().enumerate().take(DPB_MAX) {
            d.poc_st_curr_after[i] = index(&self.dpb_id, id);
        }
        for (i, &id) in s.lt.iter().enumerate().take(DPB_MAX) {
            d.poc_lt_curr[i] = index(&self.dpb_id, id);
        }
        d.num_poc_st_curr_before = s.st_pred.len().min(DPB_MAX) as u8;
        d.num_poc_st_curr_after = s.st_po.len().min(DPB_MAX) as u8;
        d.num_poc_lt_curr = s.lt.len().min(DPB_MAX) as u8;
        self.aktualny = Some(ObrazV4l2 {
            id,
            parametre: ParametreDekodovania {
                sps: sps_v4l2(sps),
                pps: pps_v4l2(pps),
                matica: matica_v4l2(sps, pps),
                dekodovanie: d,
                rezy: Vec::new(),
            },
            rezy_nal: Vec::new(),
        });
    }

    fn rez_v4l2(&self, rez: &HlavickaRezu, telo: &[u8], dlzka_nal: usize) -> Vysledok<RezV4l2> {
        let poc = self.aktualny.as_ref().map_or(0, |o| o.parametre.dekodovanie.pic_order_cnt_val);
        let zoznamy = self.sprava.zoznamy(rez)?;
        let mut ref_idx = [[0u8; DPB_MAX]; 2];
        for lx in 0..2 {
            for (i, &id) in zoznamy[lx].iter().enumerate().take(DPB_MAX) {
                ref_idx[lx][i] = self.dpb_id.iter().position(|&x| x == id).unwrap_or(0) as u8;
            }
        }
        let mut w = VahyV4l2::default();
        if let Some(t) = &rez.vahy {
            w.luma_log2_weight_denom = t.log2_jas as u8;
            w.delta_chroma_log2_weight_denom = (t.log2_farba as i32 - t.log2_jas as i32) as i8;
            for lx in 0..2 {
                for (i, v) in t.zoznamy[lx].iter().enumerate().take(DPB_MAX) {
                    let (dw, o) = v.jas.unwrap_or((0, 0));
                    let mut dc = [0i8; 2];
                    let mut oc = [0i8; 2];
                    for j in 0..2 {
                        dc[j] = v.farba.map_or(0, |f| f[j].0) as i8;
                        oc[j] = t.farba(lx, i, j).1 as i8;
                    }
                    if lx == 0 {
                        w.delta_luma_weight_l0[i] = dw as i8;
                        w.luma_offset_l0[i] = o as i8;
                        w.delta_chroma_weight_l0[i] = dc;
                        w.chroma_offset_l0[i] = oc;
                    } else {
                        w.delta_luma_weight_l1[i] = dw as i8;
                        w.luma_offset_l1[i] = o as i8;
                        w.delta_chroma_weight_l1[i] = dc;
                        w.chroma_offset_l1[i] = oc;
                    }
                }
            }
        }
        let mut f = 0;
        let mut priznak = |podmienka: bool, b: u64| {
            if podmienka {
                f |= b;
            }
        };
        priznak(rez.sao_jas, SLICE_FLAG_SLICE_SAO_LUMA);
        priznak(rez.sao_farba, SLICE_FLAG_SLICE_SAO_CHROMA);
        priznak(rez.tmvp, SLICE_FLAG_SLICE_TEMPORAL_MVP_ENABLED);
        priznak(rez.mvd_l1_nula, SLICE_FLAG_MVD_L1_ZERO);
        priznak(rez.cabac_init, SLICE_FLAG_CABAC_INIT);
        priznak(rez.kolokovany_z_l0, SLICE_FLAG_COLLOCATED_FROM_L0);
        priznak(rez.odblokovanie_vypnute, SLICE_FLAG_SLICE_DEBLOCKING_FILTER_DISABLED);
        priznak(rez.filter_cez_rezy, SLICE_FLAG_SLICE_LOOP_FILTER_ACROSS_SLICES_ENABLED);
        priznak(rez.zavisly, SLICE_FLAG_DEPENDENT_SLICE_SEGMENT);
        Ok(RezV4l2 {
            bit_size: (dlzka_nal * 8) as u32,
            data_byte_offset: (2 + posun_s_ochranou(telo, rez.dlzka_rbsp)) as u32,
            num_entry_point_offsets: rez.vstupne_body.len() as u32,
            nal_unit_type: rez.nal.typ,
            nuh_temporal_id_plus1: rez.nal.casova_vrstva + 1,
            slice_type: rez.typ as u8,
            colour_plane_id: rez.rovina_farby,
            slice_pic_order_cnt: poc,
            num_ref_idx_l0_active_minus1: rez.aktivne[0].saturating_sub(1) as u8,
            num_ref_idx_l1_active_minus1: rez.aktivne[1].saturating_sub(1) as u8,
            collocated_ref_idx: rez.kolokovany_ref as u8,
            five_minus_max_num_merge_cand: if rez.typ == TypRezu::I {
                0
            } else {
                (5 - rez.max_zlucenie) as u8
            },
            slice_qp_delta: rez.qp_delta as i8,
            slice_cb_qp_offset: rez.cb_qp_offset as i8,
            slice_cr_qp_offset: rez.cr_qp_offset as i8,
            slice_act_y_qp_offset: 0,
            slice_act_cb_qp_offset: 0,
            slice_act_cr_qp_offset: 0,
            slice_beta_offset_div2: rez.beta_div2 as i8,
            slice_tc_offset_div2: rez.tc_div2 as i8,
            pic_struct: 0,
            slice_segment_addr: rez.adresa,
            ref_idx_l0: ref_idx[0],
            ref_idx_l1: ref_idx[1],
            short_term_ref_pic_set_size: rez.bity_sady as u16,
            long_term_ref_pic_set_size: rez.bity_dlhodobych as u16,
            pred_weight_table: w,
            flags: f,
            entry_point_offset_minus1: rez.vstupne_body.iter().map(|v| v - 1).collect(),
        })
    }
}
