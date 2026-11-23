//! MERAČ PAMÄTE a MERAČ BUDENÍ (R162, R163) – veda namiesto odhadu.
//!
//! **Merač pamäte:** radič pamäte DDR v čipoch Amlogic G12/SM1 počíta prenesené bajty a jadro Linux ho
//! sprístupňuje ako počítadlo perf `meson_ddr_bw`. Celkový tok plus 4 kanály, každý s filtrom až 4 pánov
//! zbernice. KLÍČ ich rozdelí tak, aby bolo vidno, **kto s pamäťou komunikuje**:
//! procesor · obraz (čítanie a zápis obvodu obrazu VPU) · video a GPU (dekodér, HEVC, Mali) · periférie
//! (USB, Ethernet). Jedno započítanie = 16 bajtov.
//!
//! **Merač budení:** z `/proc/interrupts` – ktoré zdroje prerušení budia čip a koľkokrát za sekundu
//! (každé budenie = procesor sa prebudí, siahne do pamäte, zohreje sa).

#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::io::Read;

/// Kanál merača: názov a filter pánov zbernice (bity `config1` podľa ovládača jadra).
pub const KANALY: [(&str, u64); 4] = [
    ("procesor", 1 << 0),
    // vpu_read1, vpu_read2, vpu_read3, vpu_write1
    ("obraz", (1 << 16) | (1 << 17) | (1 << 18) | (1 << 19)),
    // vdec, hevc_front, hevc_back, gpu
    ("video a GPU", (1 << 21) | (1 << 4) | (1 << 8) | (1 << 1)),
    // usb0, usb1, usb3_0, ethernet
    ("periférie", (1 << 33) | (1 << 37) | (1 << 6) | (1 << 45)),
];

/// Bajtov na jedno započítanie radiča.
pub const BAJTOV_NA_ZAPOCET: u64 = 16;

/// R176: merač pamäte len v krátkych oknách. Ovládač jadra ho počas merania 10× za sekundu znova spúšťa
/// v radiči pamäte (prerušenie časovača) – pri pôvodnom obraze (≈ 500 MB/s) to spôsobovalo výpadky HDMI
/// každých pár sekúnd (pokus 23. 11. 2026, fázy 1, 2 a 8) a zbytočne budilo čip. Preto: okno 2 s raz
/// za minútu a nikdy pri obraze s veľkým tokom.
pub const OKNO_S: f64 = 2.0;
pub const OKNO_KAZDYCH_S: f64 = 60.0;

/// Priemer tokov z viacerých okien.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SumaToku {
    suma: Tok,
    pub okien: u32,
}

impl SumaToku {
    pub fn pridaj(&mut self, t: &Tok) {
        self.suma.celkom += t.celkom;
        for (s, v) in self.suma.kanaly.iter_mut().zip(t.kanaly) {
            *s += v;
        }
        self.okien += 1;
    }

    pub fn priemer(&self) -> Option<Tok> {
        (self.okien > 0).then(|| {
            let n = self.okien as f64;
            Tok { celkom: self.suma.celkom / n, kanaly: self.suma.kanaly.map(|v| v / n) }
        })
    }
}

/// Tok pamäte v MB/s: celkom a po kanáloch.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Tok {
    pub celkom: f64,
    pub kanaly: [f64; 4],
}

impl Tok {
    /// Tok z rozdielu dvoch stavov počítadiel (započty) za `s` sekúnd.
    pub fn z_rozdielu(pred: &[u64; 5], teraz: &[u64; 5], s: f64) -> Tok {
        let mb = |a: u64, b: u64| b.saturating_sub(a) as f64 * BAJTOV_NA_ZAPOCET as f64 / 1e6 / s.max(1e-3);
        Tok {
            celkom: mb(pred[0], teraz[0]),
            kanaly: [
                mb(pred[1], teraz[1]),
                mb(pred[2], teraz[2]),
                mb(pred[3], teraz[3]),
                mb(pred[4], teraz[4]),
            ],
        }
    }

    /// Krátky popis pre obrazovku: „pamäť 31 MB/s (obraz 25 · procesor 4)“ – kanály nad 0,5 MB/s.
    pub fn popis(&self) -> String {
        let mut casti: Vec<(f64, &str)> =
            KANALY.iter().zip(self.kanaly).filter(|(_, v)| *v >= 0.5).map(|(k, v)| (v, k.0)).collect();
        casti.sort_by(|a, b| b.0.total_cmp(&a.0));
        let detail: Vec<String> = casti.iter().map(|(v, n)| format!("{n} {}", mb(*v))).collect();
        if detail.is_empty() {
            format!("pamäť {} MB/s", mb(self.celkom))
        } else {
            format!("pamäť {} MB/s ({})", mb(self.celkom), detail.join(" · "))
        }
    }
}

fn mb(v: f64) -> String {
    if v < 10.0 { format!("{v:.1}").replace('.', ",") } else { format!("{v:.0}") }
}

/// Otvorený merač pamäte (počítadlá jadra).
#[cfg(target_os = "linux")]
pub struct MeracPamate {
    pocitadla: Vec<fs::File>,
}

#[cfg(target_os = "linux")]
impl MeracPamate {
    /// Otvorí merač, ak ho jadro má (Amlogic G12/SM1). `None` na ostatných doskách.
    pub fn otvor() -> Option<MeracPamate> {
        let d = "/sys/bus/event_source/devices/meson_ddr_bw";
        let typ: u32 = fs::read_to_string(format!("{d}/type")).ok()?.trim().parse().ok()?;
        let cpu: i32 =
            fs::read_to_string(format!("{d}/cpumask")).ok()?.trim().split([',', '-']).next()?.parse().ok()?;
        let mut pocitadla = vec![crate::sys::perf_pocitadlo(typ, 0, 0, cpu).ok()?];
        for (i, (_, filter)) in KANALY.iter().enumerate() {
            pocitadla.push(crate::sys::perf_pocitadlo(typ, i as u64 + 1, *filter, cpu).ok()?);
        }
        Some(MeracPamate { pocitadla })
    }

    /// Stav počítadiel (započty od otvorenia).
    pub fn stav(&mut self) -> Option<[u64; 5]> {
        let mut v = [0u64; 5];
        for (i, f) in self.pocitadla.iter_mut().enumerate() {
            let mut b = [0u8; 8];
            f.read_exact(&mut b).ok()?;
            v[i] = u64::from_ne_bytes(b);
        }
        Some(v)
    }
}

/// Prerušenia podľa zdroja z `/proc/interrupts`: (meno, súčet cez všetky procesory).
pub fn prerusenia(text: &str) -> Vec<(String, u64)> {
    let mut lines = text.lines();
    let Some(hlavicka) = lines.next() else { return Vec::new() };
    let cpus = hlavicka.split_whitespace().filter(|s| s.starts_with("CPU")).count();
    let mut v = Vec::new();
    for l in lines {
        let Some((cislo, zvysok)) = l.split_once(':') else { continue };
        let casti: Vec<&str> = zvysok.split_whitespace().collect();
        let pocty: Vec<u64> = casti.iter().take(cpus).map_while(|c| c.parse().ok()).collect();
        if pocty.is_empty() {
            continue;
        }
        let sucet: u64 = pocty.iter().sum();
        // meno: posledné slová riadku (ovládač/zariadenie); pri IPI a pod. popis za číslami
        let popis: Vec<&str> = casti.iter().skip(pocty.len()).copied().collect();
        let meno = match popis.last() {
            Some(posledne) if cislo.trim().parse::<u32>().is_ok() => (*posledne).to_string(),
            _ => format!("{} {}", cislo.trim(), popis.join(" ")).trim().to_string(),
        };
        v.push((meno, sucet));
    }
    v
}

/// Najčastejšie budenia za interval: (meno, za sekundu), zoradené, najviac `n`.
pub fn budenia(pred: &[(String, u64)], teraz: &[(String, u64)], s: f64, n: usize) -> Vec<(String, f64)> {
    let mut v: Vec<(String, f64)> = teraz
        .iter()
        .map(|(m, c)| {
            let p = pred.iter().find(|x| &x.0 == m).map_or(0, |x| x.1);
            (m.clone(), c.saturating_sub(p) as f64 / s.max(1e-3))
        })
        .filter(|x| x.1 >= 0.05)
        .collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v.truncate(n);
    v
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn priemer_okien() {
        let mut s = SumaToku::default();
        assert_eq!(s.priemer(), None);
        s.pridaj(&Tok { celkom: 10.0, kanaly: [2.0, 8.0, 0.0, 0.0] });
        s.pridaj(&Tok { celkom: 30.0, kanaly: [4.0, 26.0, 0.0, 0.0] });
        assert_eq!(s.priemer(), Some(Tok { celkom: 20.0, kanaly: [3.0, 17.0, 0.0, 0.0] }));
    }

    #[test]
    fn tok_pamate_z_pocitadiel() {
        // 25 MB/s obraz za 10 s = 15 625 000 započtov (16 B), procesor 3 MB/s
        let pred = [0, 0, 0, 0, 0];
        let teraz = [18_750_000 + 100, 1_875_000, 15_625_000, 0, 1000];
        let t = Tok::z_rozdielu(&pred, &teraz, 10.0);
        assert!((t.kanaly[1] - 25.0).abs() < 1e-9 && (t.kanaly[0] - 3.0).abs() < 1e-9);
        assert_eq!(t.popis(), "pamäť 30 MB/s (obraz 25 · procesor 3,0)");
        assert_eq!(Tok::default().popis(), "pamäť 0,0 MB/s");
        // pretečenie/reštart počítadla nedá záporný tok
        assert_eq!(Tok::z_rozdielu(&teraz, &pred, 1.0).celkom, 0.0);
    }

    #[test]
    fn budenia_z_proc_interrupts() {
        let a = "           CPU0       CPU1       CPU2       CPU3\n\
                  11:      1000        200          0          0     GICv2  30 Level     arch_timer\n\
                  25:        50          0          0          0     GICv2  40 Level     eth0\n\
                 IPI0:       10         20          0          0       Rescheduling interrupts\n";
        let b = "           CPU0       CPU1       CPU2       CPU3\n\
                  11:      1400        200          0          0     GICv2  30 Level     arch_timer\n\
                  25:       150          0          0          0     GICv2  40 Level     eth0\n\
                 IPI0:       12         20          0          0       Rescheduling interrupts\n";
        let pa = prerusenia(a);
        assert_eq!(pa[0], ("arch_timer".to_string(), 1200));
        assert_eq!(pa[2].0, "IPI0 Rescheduling interrupts");
        let v = budenia(&pa, &prerusenia(b), 100.0, 5);
        assert_eq!(v[0], ("arch_timer".to_string(), 4.0));
        assert_eq!(v[1], ("eth0".to_string(), 1.0));
        assert_eq!(v.len(), 2, "{v:?}");
    }
}
