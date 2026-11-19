//! Skúšky GPU vrstvy: interpret, prekladače a zhoda každého dostupného zariadenia s referenciou.
//!
//! Premenná prostredia `ORCHIDEA_VYZADUJ_GPU` (napr. „Vulkan“ alebo „D3d12“) v CI zaručí, že sa skúšky
//! naozaj spustili na danom rozhraní – ak zariadenie chýba, skúška zlyhá namiesto tichého preskočenia.

use orchidea_gpu::ir::*;
use orchidea_gpu::kernely;
use orchidea_gpu::zariadenie::{DataVazby, Hodnota32, Procesor, Zariadenie, vsetky_zariadenia};
use orchidea_jadro::nahoda::Nahoda;

fn nahodne(n: &mut Nahoda, dlzka: usize) -> Vec<f32> {
    (0..dlzka).map(|_| n.f32() * 2.0 - 1.0).collect()
}

fn nasob(z: &dyn Zariadenie, m: usize, n: usize, k: usize, a: &[f32], b: &[f32]) -> Vec<f32> {
    let mut a = a.to_vec();
    let mut b = b.to_vec();
    let mut c = vec![f32::NAN; m * n];
    let kern = kernely::nasobenie_presne();
    let skupiny = [(n as u32).div_ceil(16), (m as u32).div_ceil(16), 1];
    z.spusti(
        &kern,
        &mut [DataVazby::F32(&mut a), DataVazby::F32(&mut b), DataVazby::F32(&mut c)],
        &[Hodnota32::U32(m as u32), Hodnota32::U32(n as u32), Hodnota32::U32(k as u32)],
        skupiny,
    )
    .unwrap_or_else(|e| panic!("{}: {e}", z.popis()));
    c
}

#[test]
fn interpret_pocita_spravne_a_odhali_chyby() {
    let p = Procesor::default();
    let mut n = Nahoda::zo_semienka(1);
    let (m, nn, k) = (5, 7, 9);
    let a = nahodne(&mut n, m * k);
    let b = nahodne(&mut n, k * nn);
    let c = nasob(&p, m, nn, k, &a, &b);
    for i in 0..m {
        for j in 0..nn {
            let mut s = 0.0f32;
            for q in 0..k {
                s += a[i * k + q] * b[q * nn + j];
            }
            assert_eq!(s.to_bits(), c[i * nn + j].to_bits());
        }
    }
    // čítanie mimo bufferu je chyba (na GPU nedefinované)
    let mut kr = Kernel::novy("mimo", [1, 1, 1]);
    let x = kr.vazba("x", Typ::F32, Pristup::CitanieZapis);
    kr.telo = vec![zapis(x, u(0), citaj(x, u(5)))];
    let mut d = vec![1.0f32; 2];
    assert!(p.spusti(&kr, &mut [DataVazby::F32(&mut d)], &[], [1, 1, 1]).is_err());
    // typy sa kontrolujú
    let mut zly = Kernel::novy("zlý", [1, 1, 1]);
    let y = zly.vazba("y", Typ::F32, Pristup::Zapis);
    zly.telo = vec![zapis(y, u(0), u(1))];
    assert!(zly.over().is_err());
    // nesprávne argumenty
    let mut d = vec![1u32; 2];
    assert!(p.spusti(&kr, &mut [DataVazby::U32(&mut d)], &[], [1, 1, 1]).is_err());
}

fn spusti_skusku_jazyka(z: &dyn Zariadenie) -> (Vec<f32>, Vec<u32>, Vec<i32>) {
    let kern = kernely::skuska_jazyka();
    let mut n = Nahoda::zo_semienka(2);
    let pocet = 1000usize;
    let mut vf: Vec<f32> = nahodne(&mut n, pocet);
    vf[3] = -0.25;
    vf[4] = 0.5;
    vf[5] = 0.0;
    let mut vu: Vec<u32> = (0..pocet).map(|_| n.dalsie() as u32 % 1000).collect();
    let mut of = vec![0.0f32; pocet];
    let mut ou = vec![0u32; pocet];
    let mut oi: Vec<i32> = (0..pocet).map(|i| i as i32 - 500).collect();
    z.spusti(
        &kern,
        &mut [
            DataVazby::F32(&mut vf),
            DataVazby::U32(&mut vu),
            DataVazby::F32(&mut of),
            DataVazby::U32(&mut ou),
            DataVazby::I32(&mut oi),
        ],
        &[Hodnota32::U32(pocet as u32 - 3), Hodnota32::F32(0.375), Hodnota32::I32(-11)],
        [(pocet as u32).div_ceil(32), 1, 1],
    )
    .unwrap_or_else(|e| panic!("{}: {e}", z.popis()));
    (of, ou, oi)
}

fn gpu_zariadenia() -> Vec<std::sync::Arc<dyn Zariadenie>> {
    let (vsetky, dovody) = vsetky_zariadenia();
    for d in &dovody {
        eprintln!("nedostupné: {d}");
    }
    let gpu: Vec<_> =
        vsetky.into_iter().filter(|z| z.popis().druh != orchidea_gpu::zariadenie::Druh::Procesor).collect();
    for z in &gpu {
        eprintln!("zariadenie: {}", z.popis());
    }
    if let Ok(druh) = std::env::var("ORCHIDEA_VYZADUJ_GPU")
        && !druh.is_empty()
    {
        assert!(
            gpu.iter().any(|z| format!("{:?}", z.popis().druh) == druh),
            "CI vyžaduje zariadenie {druh}, ale nenašlo sa (dôvody: {dovody:?})"
        );
    }
    gpu
}

#[test]
fn kazde_zariadenie_sa_zhoduje_s_referenciou() {
    let referencia = spusti_skusku_jazyka(&Procesor::default());
    for z in gpu_zariadenia() {
        let (of, ou, oi) = spusti_skusku_jazyka(z.as_ref());
        assert_eq!(ou, referencia.1, "{}: celočíselné výsledky (u32) sa musia zhodovať presne", z.popis());
        assert_eq!(oi, referencia.2, "{}: celočíselné výsledky (i32) sa musia zhodovať presne", z.popis());
        for (i, (a, b)) in of.iter().zip(&referencia.0).enumerate() {
            assert!((a - b).abs() <= 1e-4 * (1.0 + b.abs()), "{}: f32 [{i}] {a} vs {b}", z.popis());
        }
    }
}

#[test]
fn nasobenie_matic_na_gpu_je_bitovo_rovnake_ako_na_procesore() {
    let mut n = Nahoda::zo_semienka(3);
    for &(m, nn, k) in &[(1, 1, 1), (16, 16, 16), (33, 17, 65), (100, 70, 129)] {
        let a = nahodne(&mut n, m * k);
        let b = nahodne(&mut n, k * nn);
        let referencia = nasob(&Procesor::default(), m, nn, k, &a, &b);
        for z in gpu_zariadenia() {
            let c = nasob(z.as_ref(), m, nn, k, &a, &b);
            let rozdielne = c.iter().zip(&referencia).filter(|(x, y)| x.to_bits() != y.to_bits()).count();
            assert_eq!(rozdielne, 0, "{}: {m}×{k}×{nn} – {rozdielne} prvkov sa líši v bitoch", z.popis());
        }
    }
}
