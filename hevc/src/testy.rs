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
