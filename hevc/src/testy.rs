//! Skúšky rozboru: zapisovač bitov zostaví parametrické sady s takmer všetkými voľbami, rozbor ich musí prečítať
//! presne; poškodené a skrátené dáta musia vrátiť chybu, nikdy nesmú spôsobiť pád (R211).

use crate::parametre::*;
use crate::sei;
use orchidea_jadro::nahoda::Nahoda;

/// Zapisovač bitov s kódmi Exp-Golomb (len pre skúšky).
#[derive(Default)]
struct Zapis {
    data: Vec<u8>,
    obsadene: u32,
}

impl Zapis {
    fn bit(&mut self, b: bool) {
        if self.obsadene == 0 {
            self.data.push(0);
        }
        if b {
            *self.data.last_mut().unwrap() |= 0x80 >> self.obsadene;
        }
        self.obsadene = (self.obsadene + 1) % 8;
    }
    fn u(&mut self, n: u32, v: u64) {
        for i in (0..n).rev() {
            self.bit(i < 64 && (v >> i) & 1 == 1);
        }
    }
    fn ue(&mut self, v: u64) {
        let x = v + 1;
        let n = 64 - x.leading_zeros();
        self.u(n - 1, 0);
        self.u(n, x);
    }
    fn se(&mut self, v: i64) {
        self.ue(if v > 0 { (2 * v - 1) as u64 } else { (-2 * v) as u64 });
    }
    fn koniec(mut self) -> Vec<u8> {
        self.bit(true);
        while self.obsadene != 0 {
            self.bit(false);
        }
        self.data
    }
}

/// profile_tier_level: Main 10, úroveň 4, jedna podvrstva s profilom aj úrovňou.
fn ptl(z: &mut Zapis, podvrstiev_minus1: u32) {
    z.u(2, 0);
    z.u(1, 0);
    z.u(5, 2);
    z.u(32, 0x2000_0000);
    z.u(4, 0b1001); // progresívny, nie prekladaný, non_packed 0, len snímky
    z.u(44, 0);
    z.u(8, 120);
    for _ in 0..podvrstiev_minus1 {
        z.u(2, 0b11);
    }
    if podvrstiev_minus1 > 0 {
        z.u(2 * (8 - podvrstiev_minus1), 0);
    }
    for _ in 0..podvrstiev_minus1 {
        z.u(88, 0);
        z.u(8, 90);
    }
}

fn vps() -> Vec<u8> {
    let mut z = Zapis::default();
    z.u(4, 1);
    z.u(2, 0b11);
    z.u(6, 0);
    z.u(3, 1);
    z.u(1, 1);
    z.u(16, 0xffff);
    ptl(&mut z, 1);
    z.u(1, 0); // len posledná podvrstva
    z.ue(4);
    z.ue(2);
    z.ue(0);
    z.u(6, 0);
    z.ue(0);
    z.u(1, 1);
    z.u(32, 1001);
    z.u(32, 60000);
    z.u(1, 0);
    z.koniec()
}

fn sps() -> Vec<u8> {
    let mut z = Zapis::default();
    z.u(4, 1); // vps
    z.u(3, 1); // dve podvrstvy
    z.u(1, 1);
    ptl(&mut z, 1);
    z.ue(3); // sps_id
    z.ue(1); // 4:2:0
    z.ue(1920);
    z.ue(1088);
    z.u(1, 1); // conformance window: 8 riadkov jasu dole
    for v in [0, 0, 0, 4] {
        z.ue(v);
    }
    z.ue(2);
    z.ue(2); // 10 bitov
    z.ue(4); // POC lsb 8 bitov
    z.u(1, 1);
    for (a, b) in [(3, 2), (4, 2)] {
        z.ue(a);
        z.ue(b);
        z.ue(0);
    }
    z.ue(0); // CB 8
    z.ue(3); // CTB 64
    z.ue(0); // TB 4
    z.ue(3); // TB 32
    z.ue(2);
    z.ue(1);
    // matice kvantovania
    z.u(1, 1);
    z.u(1, 1);
    for velkost in 0..4 {
        let krok = if velkost == 3 { 3 } else { 1 };
        let mut m = 0;
        while m < 6 {
            match (velkost, m) {
                (0, 0) => {
                    z.u(1, 1);
                    (0..16).for_each(|_| z.se(0));
                }
                (2, 0) => {
                    z.u(1, 1);
                    z.se(4); // DC 12
                    z.se(2);
                    (1..64).for_each(|_| z.se(0));
                }
                (1, 0) | (3, 0) => {
                    z.u(1, 0);
                    z.ue(0); // predvolená
                }
                _ => {
                    z.u(1, 0);
                    z.ue(1); // kópia predošlej
                }
            }
            m += krok;
        }
    }
    z.u(1, 1); // amp
    z.u(1, 1); // sao
    z.u(1, 1); // pcm
    z.u(4, 7);
    z.u(4, 7);
    z.ue(0);
    z.ue(2);
    z.u(1, 1);
    // sady referencií
    z.ue(2);
    z.ue(2);
    z.ue(1);
    z.ue(0);
    z.u(1, 1);
    z.ue(1);
    z.u(1, 1);
    z.ue(1);
    z.u(1, 1);
    z.u(1, 1); // predikovaná zo sady 0
    z.u(1, 1); // znamienko –
    z.ue(0); // deltaRps = −1
    z.u(4, 0b1111);
    // dlhodobé referencie
    z.u(1, 1);
    z.ue(2);
    z.u(8, 5);
    z.u(1, 1);
    z.u(8, 200);
    z.u(1, 0);
    z.u(1, 1); // temporal mvp
    z.u(1, 1); // strong intra smoothing
    // VUI
    z.u(1, 1);
    z.u(1, 1);
    z.u(8, 1);
    z.u(1, 0);
    z.u(1, 1);
    z.u(3, 5);
    z.u(1, 0);
    z.u(1, 1);
    z.u(24, 0x09_10_09);
    z.u(1, 1);
    z.ue(2);
    z.ue(2);
    z.u(3, 0);
    z.u(1, 0);
    z.u(1, 1);
    z.u(32, 1001);
    z.u(32, 60000);
    z.u(1, 0);
    z.u(1, 1); // HRD
    z.u(1, 1);
    z.u(1, 0);
    z.u(1, 0);
    z.u(8, 0x33);
    z.u(15, 0x1234);
    for _ in 0..2 {
        z.u(1, 1);
        z.ue(0);
        z.ue(0);
        z.ue(1000);
        z.ue(2000);
        z.u(1, 1);
    }
    z.u(1, 1); // bitstream_restriction
    z.u(3, 0b010);
    z.ue(0);
    z.ue(2);
    z.ue(1);
    z.ue(15);
    z.ue(15);
    // rozšírenie rozsahu
    z.u(1, 1);
    z.u(1, 1);
    z.u(7, 0);
    z.u(9, 0b1_0100_0001); // rotation, implicit RDPCM, bypass alignment
    z.koniec()
}

fn pps() -> Vec<u8> {
    let mut z = Zapis::default();
    z.ue(5);
    z.ue(3);
    z.u(1, 1);
    z.u(1, 0);
    z.u(3, 2);
    z.u(1, 1);
    z.u(1, 1);
    z.ue(3);
    z.ue(1);
    z.se(-4);
    z.u(1, 0);
    z.u(1, 1);
    z.u(1, 1);
    z.ue(2);
    z.se(-2);
    z.se(3);
    z.u(1, 1);
    z.u(1, 1);
    z.u(1, 0);
    z.u(1, 0);
    z.u(1, 1); // dlaždice
    z.u(1, 1); // WPP
    z.ue(2);
    z.ue(1);
    z.u(1, 0);
    z.ue(8);
    z.ue(9);
    z.ue(6);
    z.u(1, 1);
    z.u(1, 0);
    z.u(1, 1); // odblokovanie
    z.u(1, 1);
    z.u(1, 0);
    z.se(2);
    z.se(-3);
    z.u(1, 0);
    z.u(1, 1);
    z.ue(1);
    z.u(1, 0);
    z.u(1, 1); // rozšírenia
    z.u(1, 1);
    z.u(7, 0);
    z.ue(1);
    z.u(1, 1);
    z.u(1, 1);
    z.ue(1);
    z.ue(1);
    z.se(1);
    z.se(-1);
    z.se(-2);
    z.se(2);
    z.ue(0);
    z.ue(1);
    z.koniec()
}

#[test]
fn vps_presne() {
    let v = Vps::citaj(&vps()).unwrap();
    assert_eq!((v.id, v.max_podvrstiev_minus1), (1, 1));
    assert_eq!(v.profil.meno(), "Main 10");
    assert_eq!(v.profil.uroven, 120);
    assert_eq!(v.podvrstvy[0], v.podvrstvy[1]);
    assert_eq!(v.podvrstvy[1].max_dec_pic_buffering_minus1, 4);
    assert_eq!(v.casovanie, Some(Casovanie { jednotiek_na_takt: 1001, mierka_casu: 60000 }));
}

#[test]
fn sps_presne() {
    let s = Sps::citaj(&sps()).unwrap();
    assert_eq!((s.vps_id, s.id, s.chroma_format_idc), (1, 3, 1));
    assert_eq!((s.sirka, s.vyska, s.zobrazene_rozmery()), (1920, 1088, (1920, 1080)));
    assert_eq!((s.bity_jasu, s.bity_farby, s.log2_max_poc_lsb), (10, 10, 8));
    assert_eq!(s.podvrstvy[0].max_dec_pic_buffering_minus1, 3);
    assert_eq!(s.podvrstvy[1].max_dec_pic_buffering_minus1, 4);
    assert_eq!((s.log2_min_cb, s.log2_ctb, s.log2_min_tb, s.log2_max_tb), (3, 6, 2, 5));
    assert_eq!((s.max_hlbka_inter, s.max_hlbka_intra), (2, 1));
    assert_eq!(s.ctb_mriezka(), (30, 17));
    let m = s.matice.as_ref().unwrap();
    assert!(m.zoznamy[0].iter().all(|z| z == &vec![8; 16]));
    assert_eq!(m.zoznamy[1][3][63], 115, "kópia vnútrosnímkovej, nie predvolená medzisnímková");
    assert!(m.zoznamy[2].iter().all(|z| z == &vec![14; 64]));
    assert_eq!(m.dc[0], [12; 6]);
    assert_eq!((m.zoznamy[3][3][63], m.dc[1][3]), (115, 16));
    assert_eq!((m.zoznamy[3][1][0], m.dc[1][1]), (14, 12));
    assert!(s.amp && s.sao);
    assert_eq!(s.pcm, Some(Pcm { bity_jasu: 8, bity_farby: 8, log2_min: 3, log2_max: 5, bez_filtra: true }));
    assert_eq!(s.sady_referencii[0].dozadu, vec![(-1, true), (-3, true)]);
    assert_eq!(s.sady_referencii[0].dopredu, vec![(2, true)]);
    // predikovaná sada: deltaRps = −1 (7-61, 7-62)
    assert_eq!(s.sady_referencii[1].dozadu, vec![(-1, true), (-2, true), (-4, true)]);
    assert_eq!(s.sady_referencii[1].dopredu, vec![(1, true)]);
    assert_eq!(s.dlhodobe, Some(vec![(5, true), (200, false)]));
    assert!(s.temporal_mvp && s.strong_intra_smoothing);
    let vui = s.vui.as_ref().unwrap();
    assert_eq!(vui.pomer_stran, Some((1, 1)));
    assert_eq!(vui.farby, Some((9, 16, 9)));
    assert_eq!(vui.poloha_farby, Some((2, 2)));
    assert_eq!(vui.casovanie.unwrap().mierka_casu, 60000);
    assert!(s.rozsirenie.transform_skip_rotation && s.rozsirenie.implicit_rdpcm);
    assert!(s.rozsirenie.cabac_bypass_alignment && !s.rozsirenie.explicit_rdpcm);
    assert!(!s.necitane_rozsirenia);
}

#[test]
fn pps_presne() {
    let p = Pps::citaj(&pps()).unwrap();
    assert_eq!((p.id, p.sps_id, p.extra_bity_hlavicky), (5, 3, 2));
    assert!(p.zavisle_rezy && p.skryvanie_znamienok && p.cabac_init && p.transform_skip);
    assert_eq!((p.ref_l0_minus1, p.ref_l1_minus1, p.init_qp), (3, 1, 22));
    assert_eq!((p.cu_qp_delta, p.cb_qp_offset, p.cr_qp_offset), (Some(2), -2, 3));
    assert!(p.chroma_qp_v_reze && p.vazena_pred && !p.vazena_bipred && p.wpp);
    let d = p.dlazdice.as_ref().unwrap();
    assert_eq!((d.stlpcov, d.riadkov, d.filter_cez_dlazdice), (3, 2, true));
    let s = Sps::citaj(&sps()).unwrap();
    assert_eq!(d.rozdelenie(&s), (vec![9, 10, 11], vec![7, 10]));
    assert_eq!(
        p.odblokovanie,
        Some(Odblokovanie { prepis_v_reze: true, vypnute: false, beta_div2: 2, tc_div2: -3 })
    );
    assert!(p.matice.is_none() && p.uprava_zoznamov);
    assert_eq!(p.log2_parallel_merge_level, 3);
    let r = p.rozsirenie.as_ref().unwrap();
    assert_eq!(r.log2_max_transform_skip, 3);
    assert!(r.cross_component_prediction);
    assert_eq!(r.chroma_qp_offset, Some((1, vec![(1, -1), (-2, 2)])));
    assert_eq!((r.log2_sao_offset_scale_luma, r.log2_sao_offset_scale_chroma), (0, 1));
}

#[test]
fn rovnomerne_dlazdice() {
    let s = Sps::citaj(&sps()).unwrap();
    let d = DlazdiceHevc { stlpcov: 4, riadkov: 3, sirky: None, vysky: None, filter_cez_dlazdice: true };
    let (st, ri) = d.rozdelenie(&s);
    assert_eq!(st, vec![7, 8, 7, 8]);
    assert_eq!(ri, vec![5, 6, 6]);
    assert_eq!(st.iter().sum::<u32>(), 30);
}

/// Poškodené a skrátené parametrické sady nesmú spôsobiť pád (R211).
#[test]
fn poskodene_data_bez_padu() {
    let vzory = [vps(), sps(), pps()];
    let mut n = Nahoda::zo_semienka(214);
    let mut chyb = 0;
    for kolo in 0..30_000 {
        let mut d = vzory[kolo % 3].clone();
        match n.dalsie() % 3 {
            0 => {
                let k = (n.dalsie() as usize) % d.len();
                d.truncate(k);
            }
            1 => {
                for _ in 0..1 + n.dalsie() % 4 {
                    let i = (n.dalsie() as usize) % d.len();
                    d[i] ^= 1 << (n.dalsie() % 8);
                }
            }
            _ => {
                for _ in 0..1 + n.dalsie() % 3 {
                    let i = (n.dalsie() as usize) % d.len();
                    d[i] = n.dalsie() as u8;
                }
            }
        }
        let ok = match kolo % 3 {
            0 => Vps::citaj(&d).is_ok(),
            1 => Sps::citaj(&d).is_ok(),
            _ => Pps::citaj(&d).is_ok(),
        };
        chyb += usize::from(!ok);
        if let Ok(spravy) = sei::spravy(&d) {
            for (_, obsah) in spravy {
                let _ = sei::OdtlacokObrazu::citaj(obsah, 3);
            }
        }
    }
    // väčšina poškodení sa musí odhaliť
    assert!(chyb > 10_000, "odhalených len {chyb}");
}

// ---------------------------------------------------------------------------------------------------------
// H1 – hlavička rezu, správa obrazov dekodéra, parametre V4L2
// ---------------------------------------------------------------------------------------------------------

use crate::nal::{HlavickaNal, typ};
use crate::obrazy::{SpravaObrazov, Udalost};
use crate::rez::{HlavickaRezu, TypRezu};
use crate::sady::Sady;

fn sady() -> Sady {
    let mut s = Sady::nove();
    s.pridaj(&HlavickaNal { typ: typ::SPS, vrstva: 0, casova_vrstva: 0 }, &sps()).unwrap();
    s.pridaj(&HlavickaNal { typ: typ::PPS, vrstva: 0, casova_vrstva: 0 }, &pps()).unwrap();
    s
}

/// Rez P (nie prvý v obraze) so všetkými voliteľnými časťami pre PPS 5 / SPS 3 z týchto skúšok.
fn rez_p() -> Vec<u8> {
    let mut z = Zapis::default();
    z.u(1, 0); // first_slice_segment_in_pic_flag
    z.ue(5); // PPS
    z.u(1, 0); // dependent_slice_segment_flag
    z.u(9, 31); // slice_segment_address (510 CTB → 9 bitov)
    z.u(2, 0b10); // slice_reserved_flag
    z.ue(1); // P
    z.u(8, 7); // POC lsb
    z.u(1, 1); // sada zo SPS
    z.u(1, 1); // short_term_ref_pic_set_idx = 1
    z.ue(1); // num_long_term_sps
    z.ue(1); // num_long_term_pics
    z.u(1, 0); // lt_idx_sps
    z.u(1, 1);
    z.ue(2); // delta_poc_msb_cycle_lt
    z.u(8, 99);
    z.u(1, 1);
    z.u(1, 0);
    z.u(1, 1); // slice_temporal_mvp_enabled_flag
    z.u(1, 1); // SAO jas
    z.u(1, 0); // SAO farba
    z.u(1, 1); // num_ref_idx_active_override_flag
    z.ue(2);
    z.u(1, 1); // ref_pic_list_modification_flag_l0
    for e in [5, 0, 2] {
        z.u(3, e);
    }
    z.u(1, 1); // cabac_init_flag
    z.ue(2); // collocated_ref_idx
    // pred_weight_table
    z.ue(3);
    z.se(-1);
    z.u(3, 0b101);
    z.u(3, 0b010);
    z.se(-5);
    z.se(7);
    z.se(3);
    z.se(-20);
    z.se(0);
    z.se(100);
    z.se(10);
    z.se(-128);
    z.ue(2); // five_minus_max_num_merge_cand
    z.se(4); // slice_qp_delta
    z.se(1);
    z.se(-2);
    z.u(1, 0); // cu_chroma_qp_offset_enabled_flag (PPS má zoznam posunov QP farby)
    z.u(1, 1); // deblocking_filter_override_flag
    z.u(1, 0);
    z.se(-1);
    z.se(4);
    // vstupné body
    z.ue(2);
    z.ue(10);
    z.u(11, 999);
    z.u(11, 1999);
    z.koniec()
}

fn rez_idr() -> Vec<u8> {
    let mut z = Zapis::default();
    z.u(1, 1);
    z.u(1, 1); // no_output_of_prior_pics_flag
    z.ue(5);
    z.u(2, 0);
    z.ue(2); // I
    z.u(1, 1);
    z.u(1, 1);
    z.se(-3);
    z.se(0);
    z.se(0);
    z.u(1, 1); // cu_chroma_qp_offset_enabled_flag
    z.u(1, 0);
    z.ue(0);
    z.koniec()
}

fn rez_zavisly() -> Vec<u8> {
    let mut z = Zapis::default();
    z.u(1, 0);
    z.u(1, 0); // no_output_of_prior_pics_flag (segment obrazu IDR)
    z.ue(5);
    z.u(1, 1); // závislý
    z.u(9, 100);
    z.ue(1);
    z.ue(3);
    z.u(4, 5);
    z.koniec()
}

const NAL_TRAIL_R: HlavickaNal = HlavickaNal { typ: 1, vrstva: 0, casova_vrstva: 0 };
const NAL_IDR: HlavickaNal = HlavickaNal { typ: typ::IDR_W_RADL, vrstva: 0, casova_vrstva: 0 };

#[test]
fn hlavicka_rezu_p_presne() {
    let s = sady();
    let r = HlavickaRezu::citaj(&rez_p(), NAL_TRAIL_R, &s, None).unwrap();
    assert_eq!((r.prvy_v_obraze, r.zavisly, r.adresa, r.adresa_rezu), (false, false, 31, 31));
    assert_eq!((r.typ, r.poc_lsb, r.sada_zo_sps), (TypRezu::P, 7, Some(1)));
    assert_eq!(r.sada.dozadu, vec![(-1, true), (-2, true), (-4, true)]);
    assert_eq!(r.dlhodobe.len(), 2);
    assert_eq!((r.dlhodobe[0].poc_lsb, r.dlhodobe[0].pouzity, r.dlhodobe[0].msb_cyklus), (5, true, Some(2)));
    assert_eq!((r.dlhodobe[1].poc_lsb, r.dlhodobe[1].pouzity, r.dlhodobe[1].msb_cyklus), (99, true, None));
    assert_eq!(r.dlhodobe_zo_sps, 1);
    assert!(r.bity_dlhodobych > 0 && r.bity_sady == 0);
    assert_eq!(r.pocet_aktualnych, 6);
    assert!(r.tmvp && r.sao_jas && !r.sao_farba);
    assert_eq!(r.aktivne, [3, 0]);
    assert_eq!(r.uprava_zoznamu[0], Some(vec![5, 0, 2]));
    assert!(r.cabac_init && r.kolokovany_z_l0);
    assert_eq!(r.kolokovany_ref, 2);
    let v = r.vahy.as_ref().unwrap();
    assert_eq!((v.log2_jas, v.log2_farba), (3, 2));
    assert_eq!(v.jas(0, 0), (8 - 5, 7));
    assert_eq!(v.jas(0, 1), (8, 0));
    assert_eq!(v.jas(0, 2), (18, -128));
    // ChromaOffset = Clip3(−128, 127, 128 + (−20) − ((128 · 7) >> 2)) = −116
    assert_eq!(v.farba(0, 1, 0), (7, -116));
    assert_eq!(v.farba(0, 1, 1), (4, 100));
    assert_eq!(v.farba(0, 0, 0), (4, 0));
    assert_eq!(r.max_zlucenie, 3);
    assert_eq!((r.qp_delta, r.qp, r.cb_qp_offset, r.cr_qp_offset), (4, 26, 1, -2));
    assert!(!r.odblokovanie_vypnute);
    assert_eq!((r.beta_div2, r.tc_div2), (-1, 4));
    assert!(!r.filter_cez_rezy);
    assert_eq!(r.vstupne_body, vec![1000, 2000]);
    assert_eq!(r.bity_vstupnych_bodov, 11);
    assert_eq!(r.dlzka_rbsp, rez_p().len());
}

#[test]
fn hlavicka_rezu_idr_a_zavisly_segment() {
    let s = sady();
    let i = HlavickaRezu::citaj(&rez_idr(), NAL_IDR, &s, None).unwrap();
    assert!(i.prvy_v_obraze && i.bez_vystupu_predoslych);
    assert_eq!((i.typ, i.poc_lsb, i.qp), (TypRezu::I, 0, 19));
    assert!(i.sada.dozadu.is_empty() && i.dlhodobe.is_empty());
    assert_eq!((i.beta_div2, i.tc_div2), (2, -3), "z PPS");
    assert!(i.cu_chroma_qp_offset);
    // závislý segment bez nezávislého je chyba
    assert!(HlavickaRezu::citaj(&rez_zavisly(), NAL_IDR, &s, None).is_err());
    let z = HlavickaRezu::citaj(&rez_zavisly(), NAL_IDR, &s, Some(&i)).unwrap();
    assert!(z.zavisly && !z.prvy_v_obraze);
    assert_eq!((z.adresa, z.adresa_rezu), (100, 0));
    assert_eq!(z.vstupne_body, vec![6]);
    assert_eq!((z.typ, z.qp, z.sao_farba), (TypRezu::I, 19, true));
    // IRAP s rezom P je chyba
    assert!(HlavickaRezu::citaj(&rez_p(), NAL_IDR, &s, None).is_err());
    // neznáme PPS
    assert!(HlavickaRezu::citaj(&rez_p(), NAL_TRAIL_R, &Sady::nove(), None).is_err());
}

/// Poškodené hlavičky rezov nesmú spôsobiť pád (R211).
#[test]
fn poskodene_hlavicky_rezov_bez_padu() {
    let s = sady();
    let vzory = [rez_p(), rez_idr(), rez_zavisly()];
    let nezavisla = HlavickaRezu::citaj(&rez_idr(), NAL_IDR, &s, None).unwrap();
    let mut n = Nahoda::zo_semienka(1214);
    let mut chyb = 0;
    for kolo in 0..30_000 {
        let mut d = vzory[kolo % 3].clone();
        let k = (n.dalsie() as usize) % d.len();
        match n.dalsie() % 3 {
            0 => d.truncate(k),
            1 => d[k] ^= 1 << (n.dalsie() % 8),
            _ => d[k] = n.dalsie() as u8,
        }
        let nal = if kolo % 3 == 0 { NAL_TRAIL_R } else { NAL_IDR };
        chyb += usize::from(HlavickaRezu::citaj(&d, nal, &s, Some(&nezavisla)).is_err());
    }
    assert!(chyb > 8_000, "odhalených len {chyb}");
}

/// Hlavička rezu na skúšky správy obrazov: typ NAL, typ rezu, POC lsb a krátkodobá sada.
fn hlavicka(nal_typ: u8, typ_rezu: TypRezu, poc_lsb: u32, dozadu: &[i32], dopredu: &[i32]) -> HlavickaRezu {
    let s = sady();
    let mut h = HlavickaRezu::citaj(&rez_p(), NAL_TRAIL_R, &s, None).unwrap();
    h.nal = HlavickaNal { typ: nal_typ, vrstva: 0, casova_vrstva: 0 };
    h.prvy_v_obraze = true;
    h.typ = typ_rezu;
    h.poc_lsb = poc_lsb;
    h.sada = SadaReferencii {
        dozadu: dozadu.iter().map(|&d| (d, true)).collect(),
        dopredu: dopredu.iter().map(|&d| (d, true)).collect(),
    };
    h.dlhodobe.clear();
    h.uprava_zoznamu = [None, None];
    h.aktivne = match typ_rezu {
        TypRezu::I => [0, 0],
        TypRezu::P => [1, 0],
        TypRezu::B => [2, 2],
    };
    h.vystup = true;
    h.bez_vystupu_predoslych = false;
    h
}

fn sps_dpb() -> Sps {
    let mut s = Sps::citaj(&sps()).unwrap();
    for p in s.podvrstvy.iter_mut() {
        p.max_dec_pic_buffering_minus1 = 4;
        p.max_num_reorder_pics = 2;
        p.max_latency_increase_plus1 = 0;
    }
    s
}

fn vystupy(u: &[Udalost], poc: &std::collections::HashMap<u32, i32>) -> Vec<i32> {
    u.iter().filter_map(|u| if let Udalost::Vystup(id) = u { Some(poc[id]) } else { None }).collect()
}

/// Hierarchické B: dekódovanie 0, 8, 4, 2, 6 → výstup 0, 2, 4, 6, 8; zoznamy referencií podľa 8.3.4.
#[test]
fn sprava_obrazov_poradie_a_zoznamy() {
    let sps = sps_dpb();
    let mut m = SpravaObrazov::nova();
    let mut poc = std::collections::HashMap::new();
    let mut vystup = Vec::new();
    type Krok<'a> = (u8, TypRezu, u32, &'a [i32], &'a [i32]);
    let postup: [Krok; 5] = [
        (typ::IDR_W_RADL, TypRezu::I, 0, &[], &[]),
        (1, TypRezu::P, 8, &[-8], &[]),
        (1, TypRezu::B, 4, &[-4], &[4]),
        (0, TypRezu::B, 2, &[-2], &[2, 6]),
        (0, TypRezu::B, 6, &[-2, -6], &[2]),
    ];
    for (t, tr, lsb, d, p) in postup {
        let h = hlavicka(t, tr, lsb, d, p);
        let z = m.zacni_obraz(&h, &sps).unwrap();
        assert!(!z.preskocit);
        assert_eq!(z.poc, lsb as i32);
        poc.insert(z.id, z.poc);
        vystup.extend(vystupy(&z.udalosti, &poc));
        if lsb == 4 {
            let [l0, l1] = m.zoznamy(&h).unwrap();
            assert_eq!(l0.iter().map(|i| poc[i]).collect::<Vec<_>>(), vec![0, 8]);
            assert_eq!(l1.iter().map(|i| poc[i]).collect::<Vec<_>>(), vec![8, 0]);
        }
        if lsb == 2 {
            let mut h3 = h.clone();
            h3.aktivne = [4, 4];
            let [l0, l1] = m.zoznamy(&h3).unwrap();
            assert_eq!(l0.iter().map(|i| poc[i]).collect::<Vec<_>>(), vec![0, 4, 8, 0], "opakovanie zoznamu");
            assert_eq!(l1.iter().map(|i| poc[i]).collect::<Vec<_>>(), vec![4, 8, 0, 4]);
        }
        vystup.extend(vystupy(&m.ukonci_obraz(&sps), &poc));
    }
    vystup.extend(vystupy(&m.vyprazdni(), &poc));
    assert_eq!(vystup, vec![0, 2, 4, 6, 8]);
}

/// Prechod POC cez 2^8 (log2_max_poc_lsb = 8), CRA na začiatku toku a preskočenie RASL.
#[test]
fn sprava_obrazov_poc_a_rasl() {
    let sps = sps_dpb();
    let mut m = SpravaObrazov::nova();
    // CRA s POC 250 na začiatku toku: NoRaslOutputFlag = 1
    let z = m.zacni_obraz(&hlavicka(typ::CRA_NUT, TypRezu::I, 250, &[], &[]), &sps).unwrap();
    assert!(z.irap_bez_rasl && z.poc == 250);
    m.ukonci_obraz(&sps);
    // RASL pred CRA v poradí zobrazenia – preskočí sa
    let z = m.zacni_obraz(&hlavicka(typ::RASL_N, TypRezu::B, 248, &[-2], &[2]), &sps).unwrap();
    assert!(z.preskocit);
    // ďalší obraz: lsb 2 → POC 258
    let z = m.zacni_obraz(&hlavicka(1, TypRezu::P, 2, &[-8], &[]), &sps).unwrap();
    assert_eq!(z.poc, 258);
    assert_eq!(m.sady.st_pred.len(), 1);
    m.ukonci_obraz(&sps);
    let z = m.zacni_obraz(&hlavicka(1, TypRezu::P, 10, &[-8], &[]), &sps).unwrap();
    assert_eq!(z.poc, 266);
    // chýbajúca referencia (poškodený tok) sa vytvorí, aby sa obraz dal dekódovať
    m.ukonci_obraz(&sps);
    let z = m.zacni_obraz(&hlavicka(1, TypRezu::P, 20, &[-3], &[]), &sps).unwrap();
    assert!(z.udalosti.iter().any(|u| matches!(u, Udalost::Vytvor { poc: 273, .. })));
}

#[test]
fn v4l2_matica_v_rastri_a_posun_dat() {
    use crate::v4l2::*;
    assert_eq!(&diagonalny_prechod(4)[..6], &[(0, 0), (0, 1), (1, 0), (0, 2), (1, 1), (2, 0)]);
    let s = Sps::citaj(&sps()).unwrap();
    let p = Pps::citaj(&pps()).unwrap();
    let m = matica_v4l2(&s, &p).unwrap();
    assert_eq!(m.scaling_list_4x4[0], [8; 16]);
    assert_eq!(m.scaling_list_dc_coef_16x16, [12; 6]);
    // predvolená vnútrosnímková matica 8×8: posledná hodnota v poradí prechodu (115) je v rastri vpravo dole
    assert_eq!(m.scaling_list_8x8[0][63], 115);
    assert_eq!(m.scaling_list_8x8[0][7 * 8], PREDVOLENA_INTRA_RASTER_7_0);
    // posun s ochrannými bajtmi: 00 00 03 01 → RBSP 00 00 01
    assert_eq!(posun_s_ochranou(&[0xAA, 0, 0, 3, 1, 0xBB], 3), 4);
    assert_eq!(posun_s_ochranou(&[0xAA, 0, 0, 3, 1, 0xBB], 1), 1);
    let v = sps_v4l2(&s);
    assert_eq!((v.pic_width_in_luma_samples, v.pic_height_in_luma_samples), (1920, 1088));
    assert_ne!(v.flags & SPS_FLAG_SCALING_LIST_ENABLED, 0);
    let w = pps_v4l2(&p);
    assert_eq!((w.num_tile_columns_minus1, w.column_width_minus1[0], w.column_width_minus1[1]), (2, 8, 9));
    assert_eq!(w.flags & PPS_FLAG_UNIFORM_SPACING, 0);
    assert_ne!(w.flags & PPS_FLAG_ENTROPY_CODING_SYNC_ENABLED, 0);
}

/// Hodnota predvolenej vnútrosnímkovej matice 8×8 (tab. 7-6) na polohe x = 0, y = 7.
const PREDVOLENA_INTRA_RASTER_7_0: u8 = 24;

/// Koniec toku (EOB, 7.4.3.6): vydajú sa všetky obrazy a CRA za ním je prvým obrazom nového toku – NoRaslOutputFlag
/// = 1, RASL sa preskočí (8.1.3).
#[test]
fn sprava_obrazov_koniec_toku() {
    let sps = sps_dpb();
    let mut m = SpravaObrazov::nova();
    let mut poc = std::collections::HashMap::new();
    let mut vystup = Vec::new();
    for (t, tr, lsb, d) in [(typ::IDR_W_RADL, TypRezu::I, 0, &[][..]), (1, TypRezu::P, 4, &[-4][..])] {
        let z = m.zacni_obraz(&hlavicka(t, tr, lsb, d, &[]), &sps).unwrap();
        poc.insert(z.id, z.poc);
        vystup.extend(vystupy(&z.udalosti, &poc));
        vystup.extend(vystupy(&m.ukonci_obraz(&sps), &poc));
    }
    assert!(vystup.is_empty(), "preusporiadanie 2 – zatiaľ nič");
    vystup.extend(vystupy(&m.koniec_toku(), &poc));
    assert_eq!(vystup, vec![0, 4]);
    assert!(m.obrazy().is_empty());
    let z = m.zacni_obraz(&hlavicka(typ::CRA_NUT, TypRezu::I, 8, &[-4], &[]), &sps).unwrap();
    assert!(z.irap_bez_rasl && !z.bez_vystupu_predoslych);
    assert_eq!(z.poc, 8);
    m.ukonci_obraz(&sps);
    let z = m.zacni_obraz(&hlavicka(typ::RASL_N, TypRezu::B, 2, &[-2], &[2]), &sps).unwrap();
    assert!(z.preskocit);
}
