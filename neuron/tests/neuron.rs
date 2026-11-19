//! Skúšky motora Neurón: zhoda jadier, správnosť gradientov, učenie a bitová opakovateľnosť (MY0).

use orchidea_jadro::matematika as mat;
use orchidea_jadro::nahoda::Nahoda;
use orchidea_jadro::paralel;
use orchidea_neuron::jadra::{self, Rezim};
use orchidea_neuron::{AdamW, Matica, ParamId, Parametre, Paska, Premenna};

fn nahodne(n: &mut Nahoda, dlzka: usize) -> Vec<f32> {
    (0..dlzka).map(|_| n.f32() * 2.0 - 1.0).collect()
}

#[test]
fn rychle_jadro_sa_zhoduje_s_presnym() {
    let mut n = Nahoda::zo_semienka(11);
    for &(m, k, p) in
        &[(1, 1, 1), (3, 5, 7), (4, 16, 16), (5, 17, 33), (17, 64, 9), (64, 128, 40), (37, 300, 129)]
    {
        let a = nahodne(&mut n, m * k);
        let b = nahodne(&mut n, k * p);
        let mut c1 = vec![0.0; m * p];
        let mut c2 = vec![0.0; m * p];
        jadra::nasob(Rezim::Presny, m, p, k, &a, &b, &mut c1);
        jadra::nasob(Rezim::Rychly, m, p, k, &a, &b, &mut c2);
        for (x, y) in c1.iter().zip(&c2) {
            assert!((x - y).abs() <= 1e-5 * (k as f32).sqrt() * (1.0 + x.abs()), "{m}×{k}×{p}: {x} vs {y}");
        }
        // presný výpočet s odlišne zapísaným poradím: obyčajná trojitá slučka v rovnakom poradí sčítania
        for i in 0..m {
            for j in 0..p {
                let mut s = 0.0f32;
                for q in 0..k {
                    s += a[i * k + q] * b[q * p + j];
                }
                assert_eq!(s.to_bits(), c1[i * p + j].to_bits(), "presný režim musí byť bitovo rovnaký");
            }
        }
    }
}

#[test]
fn transpozicie_a_celociselne_jadro() {
    let mut n = Nahoda::zo_semienka(12);
    let (m, k, p) = (13, 29, 11);
    let a = Matica::nova(m, k, nahodne(&mut n, m * k));
    let b = Matica::nova(k, p, nahodne(&mut n, k * p));
    let c = a.krat(Rezim::Presny, &b);
    let at = a.transponovana();
    let bt = b.transponovana();
    let mut c2 = vec![0.0; m * p];
    jadra::nasob_t(Rezim::Presny, true, true, m, p, k, &at.data, &bt.data, &mut c2);
    assert_eq!(c.data, c2);
    for dlzka in [0usize, 1, 15, 16, 17, 100, 1000, 4097] {
        let x: Vec<i8> = (0..dlzka).map(|_| n.dalsie() as i8).collect();
        let y: Vec<i8> = (0..dlzka).map(|_| n.dalsie() as i8).collect();
        assert_eq!(
            jadra::sucin_i8(Rezim::Rychly, &x, &y),
            jadra::sucin_i8(Rezim::Presny, &x, &y),
            "dĺžka {dlzka}"
        );
    }
    let extremy = vec![-128i8; 1 << 16];
    assert_eq!(jadra::sucin_i8(Rezim::Rychly, &extremy, &extremy), 128 * 128 * (1 << 16));
}

#[test]
fn presny_rezim_nezavisi_od_poctu_vlakien() {
    let mut n = Nahoda::zo_semienka(13);
    let (m, k, p) = (200, 150, 90);
    let a = nahodne(&mut n, m * k);
    let b = nahodne(&mut n, k * p);
    let mut c1 = vec![0.0; m * p];
    let mut c2 = vec![0.0; m * p];
    paralel::obmedz_vlakna(1);
    jadra::nasob(Rezim::Presny, m, p, k, &a, &b, &mut c1);
    paralel::obmedz_vlakna(0);
    jadra::nasob(Rezim::Presny, m, p, k, &a, &b, &mut c2);
    assert_eq!(c1, c2);
}

/// Malá sieť pre kontrolu gradientov: x → Linear → RMSNorm → GELU → Linear → krížová entropia.
fn siet(p: &Parametre, paska: &mut Paska, x: &Matica, triedy: &[usize], ids: &[ParamId]) -> Premenna {
    let v = paska.vstup(x.clone());
    let h = paska.linear(p, v, ids[0], Some(ids[1]));
    let h = paska.rms_norm(p, h, ids[2]);
    let h = paska.gelu(h);
    let o = paska.linear(p, h, ids[3], Some(ids[4]));
    paska.krizova_entropia(o, triedy)
}

#[test]
fn gradienty_sedia_s_numerickou_deriváciou() {
    let mut n = Nahoda::zo_semienka(14);
    let mut p = Parametre::nove();
    let ids = vec![
        p.pridaj_vahy("w1", 6, 4, &mut n),
        p.pridaj_nuly("b1", 1, 6),
        p.pridaj_jednotky("g", 1, 6),
        p.pridaj_vahy("w2", 3, 6, &mut n),
        p.pridaj_nuly("b2", 1, 3),
    ];
    for b in [1usize, 4] {
        p.hodnoty[b].data.iter_mut().for_each(|v| *v = n.f32() * 0.2 - 0.1);
    }
    let x = Matica::nova(5, 4, nahodne(&mut n, 20));
    let triedy = [0, 2, 1, 1, 0];
    let mut paska = Paska::nova(Rezim::Presny);
    let s = siet(&p, &mut paska, &x, &triedy, &ids);
    p.vynuluj_gradienty();
    paska.spatne(s, &mut p);
    let analyticke = p.gradienty.clone();
    let strata = |p: &Parametre| {
        let mut t = Paska::nova(Rezim::Presny);
        let s = siet(p, &mut t, &x, &triedy, &ids);
        t.hodnota(s).data[0] as f64
    };
    let mut kontrol = 0;
    for (pi, g) in analyticke.iter().enumerate() {
        for j in 0..g.data.len() {
            let h = 1e-2f32;
            let mut plus = p.clone();
            plus.hodnoty[pi].data[j] += h;
            let mut minus = p.clone();
            minus.hodnoty[pi].data[j] -= h;
            let num = (strata(&plus) - strata(&minus)) / (2.0 * h as f64);
            let an = g.data[j] as f64;
            assert!(
                (num - an).abs() <= 2e-3 + 2e-2 * an.abs(),
                "{}[{j}]: numericky {num}, analyticky {an}",
                p.mena[pi]
            );
            kontrol += 1;
        }
    }
    assert_eq!(kontrol, p.pocet_hodnot());
}

/// Dve prepletené špirály – klasická úloha, ktorú lineárny model nevyrieši.
fn spiraly(n: &mut Nahoda, na_triedu: usize) -> (Matica, Vec<usize>) {
    let mut x = Vec::with_capacity(na_triedu * 4);
    let mut t = Vec::with_capacity(na_triedu * 2);
    for trieda in 0..2 {
        for i in 0..na_triedu {
            let r = i as f64 / na_triedu as f64 * 5.0 + 0.4;
            let uhol = 1.75 * r + trieda as f64 * std::f64::consts::PI + n.normalne() * 0.08;
            x.push((r * mat::cos(uhol) / 5.0) as f32);
            x.push((r * mat::sin(uhol) / 5.0) as f32);
            t.push(trieda);
        }
    }
    (Matica::nova(na_triedu * 2, 2, x), t)
}

struct Model {
    p: Parametre,
    ids: Vec<ParamId>,
    trojhodnotovy: bool,
}

impl Model {
    fn novy(semienko: u64, skryte: usize, trojhodnotovy: bool) -> Model {
        let mut n = Nahoda::zo_semienka(semienko);
        let mut p = Parametre::nove();
        let ids = vec![
            p.pridaj_vahy("vstup.w", skryte, 2, &mut n),
            p.pridaj_nuly("vstup.b", 1, skryte),
            p.pridaj_jednotky("norma1.g", 1, skryte),
            p.pridaj_vahy("skrytá.w", skryte, skryte, &mut n),
            p.pridaj_nuly("skrytá.b", 1, skryte),
            p.pridaj_jednotky("norma2.g", 1, skryte),
            p.pridaj_vahy("výstup.w", 2, skryte, &mut n),
            p.pridaj_nuly("výstup.b", 1, 2),
        ];
        Model { p, ids, trojhodnotovy }
    }

    fn dopredu(&self, paska: &mut Paska, x: &Matica) -> Premenna {
        let i = &self.ids;
        let v = paska.vstup(x.clone());
        let h = paska.linear(&self.p, v, i[0], Some(i[1]));
        let h = paska.relu(h);
        let h = paska.rms_norm(&self.p, h, i[2]);
        let h = if self.trojhodnotovy {
            paska.bitlinear(&self.p, h, i[3], Some(i[4]))
        } else {
            paska.linear(&self.p, h, i[3], Some(i[4]))
        };
        let h = paska.relu(h);
        let h = paska.rms_norm(&self.p, h, i[5]);
        paska.linear(&self.p, h, i[6], Some(i[7]))
    }

    fn presnost(&self, rezim: Rezim, x: &Matica, t: &[usize]) -> f64 {
        let mut paska = Paska::nova(rezim);
        let o = self.dopredu(&mut paska, x);
        let o = paska.hodnota(o);
        let spravne = (0..o.riadky).filter(|&r| (o.hodnota(r, 1) > o.hodnota(r, 0)) as usize == t[r]).count();
        spravne as f64 / o.riadky as f64
    }

    fn uc(&mut self, rezim: Rezim, x: &Matica, t: &[usize], krokov: usize) -> f32 {
        let mut opt = AdamW::novy(0.02);
        let mut posledna = 0.0;
        for _ in 0..krokov {
            let mut paska = Paska::nova(rezim);
            let o = self.dopredu(&mut paska, x);
            let s = paska.krizova_entropia(o, t);
            posledna = paska.hodnota(s).data[0];
            self.p.vynuluj_gradienty();
            paska.spatne(s, &mut self.p);
            opt.krok(&mut self.p);
        }
        posledna
    }
}

#[test]
fn ucenie_spiral_f32_aj_trojhodnotove() {
    let mut n = Nahoda::zo_semienka(15);
    let (x, t) = spiraly(&mut n, 150);
    for (trojhodnotovy, prah) in [(false, 0.97), (true, 0.93)] {
        let mut model = Model::novy(16, 48, trojhodnotovy);
        let pred = model.presnost(Rezim::Rychly, &x, &t);
        let strata = model.uc(Rezim::Rychly, &x, &t, 600);
        let po = model.presnost(Rezim::Rychly, &x, &t);
        assert!(
            po >= prah,
            "trojhodnotový={trojhodnotovy}: presnosť {pred:.3} → {po:.3}, strata {strata:.4}"
        );
    }
}

#[test]
fn ucenie_je_bitovo_opakovatelne_na_kazdej_platforme() {
    let mut n = Nahoda::zo_semienka(17);
    let (x, t) = spiraly(&mut n, 60);
    let mut odtlacky = Vec::new();
    for vlakna in [1usize, 0] {
        paralel::obmedz_vlakna(vlakna);
        let mut model = Model::novy(18, 24, true);
        model.uc(Rezim::Presny, &x, &t, 40);
        odtlacky.push(model.p.odtlacok().sestnastkovo());
    }
    paralel::obmedz_vlakna(0);
    assert_eq!(odtlacky[0], odtlacky[1], "výsledok učenia nesmie závisieť od počtu vlákien");
    assert_eq!(odtlacky[0], ODTLACOK_UCENIA, "výsledok učenia sa líši od referenčnej platformy");
}

const ODTLACOK_UCENIA: &str = "ee8ef42cbe36935d7f991312d836484b70a0903b8d7908c41c7b00682777dcf0";

#[test]
fn gpu_sa_zhoduje_s_presnym_rezimom_bit_po_bite() {
    let (zariadenia, dovody) = orchidea_gpu::vsetky_zariadenia();
    for d in &dovody {
        eprintln!("nedostupné: {d}");
    }
    let mut n = Nahoda::zo_semienka(19);
    let (m, k, p) = (64, 96, 48);
    let a = nahodne(&mut n, m * k);
    let b = nahodne(&mut n, k * p);
    let mut presne = vec![0.0; m * p];
    jadra::nasob(Rezim::Presny, m, p, k, &a, &b, &mut presne);
    for z in &zariadenia {
        let mut c = vec![f32::NAN; m * p];
        jadra::nasob_na_zariadeni(z.as_ref(), m, p, k, &a, &b, &mut c).unwrap();
        assert_eq!(
            c.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
            presne.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
            "{}: výsledok sa musí zhodovať s presným režimom procesora",
            z.popis()
        );
        eprintln!("zhoda bit po bite: {}", z.popis());
    }
}
