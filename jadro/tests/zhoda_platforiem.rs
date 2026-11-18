//! Zhoda platforiem: deterministické výpočty musia dať **bitovo rovnaký** výsledok na Windows, Linuxe,
//! macOS, Androide aj webe. Test spočíta odtlačok tisícov výsledkov a porovná ho s pevnou hodnotou.
//! Ak sa na niektorej platforme líši čo i len jeden bit, CI zlyhá.

use orchidea_jadro::krivka::{Interpolacia, Kluc, Krivka, Mimo};
use orchidea_jadro::matematika as mat;
use orchidea_jadro::nahoda::Nahoda;
use orchidea_jadro::vyraz::Vyraz;
use orchidea_jadro::{Hasovac, m};

fn odtlacok_matematiky() -> String {
    let mut h = Hasovac::novy();
    let mut n = Nahoda::zo_semienka(20_261_119);
    for i in 0..20_000 {
        let x = n.v_rozsahu(-50.0, 50.0) * if i % 7 == 0 { 10.0 } else { 1.0 };
        let kladne = x.abs() + 1e-3;
        for v in [
            mat::exp(x / 4.0),
            mat::ln(kladne),
            mat::sin(x),
            mat::cos(x),
            mat::tanh(x / 8.0),
            mat::mocnina(kladne, 1.7),
        ] {
            h.pridaj(&v.to_bits().to_le_bytes());
        }
        h.pridaj(&n.normalne().to_bits().to_le_bytes());
    }
    let k = Krivka::nova(
        m("t"),
        (0..20).map(|i| Kluc::novy(i as f64, mat::sin(i as f64), Interpolacia::Monotonna)).collect(),
        Mimo::Opakuj,
    )
    .unwrap();
    let v = Vyraz::novy("exp(-t / 3) * cos(tau * t) + ln(1 + t ^ 2) - tanh(t)").unwrap();
    for i in 0..5000 {
        let t = i as f64 * 0.013;
        h.pridaj(&k.hodnota(t).to_bits().to_le_bytes());
        h.pridaj(&v.vyhodnot(&|_| Some(t)).unwrap().to_bits().to_le_bytes());
    }
    h.dokonci().sestnastkovo()
}

#[test]
fn matematika_je_bitovo_rovnaka_vsade() {
    assert_eq!(odtlacok_matematiky(), ODTLACOK, "výsledky sa líšia od referenčnej platformy");
}

const ODTLACOK: &str = "6ff4f263f1ab9c68b8f6017521d217ad28802a14a2191ec64a4053b9ed337c8c";
