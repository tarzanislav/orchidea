//! Tabuľka nameraného kontrastu všetkých tém (WCAG 2.2) – `cargo run -p orchidea-vzhlad --example kontrast`.
//! Vypíše riadky v Markdowne do dokumentu návrhu; nesplnený pár označí „NESPĹŇA“.

use orchidea_vzhlad::kontrast::{Poziadavka, kontrola};
use orchidea_vzhlad::tema::TEMY;

fn main() {
    println!(
        "| Téma | Najnižší kontrast textu | Najnižší kontrast grafiky | Text na akcente | Akcent v texte | Spolu párov |"
    );
    println!("|---|---|---|---|---|---|");
    let mut chyby = 0;
    for t in TEMY {
        let pary = kontrola(t);
        let min = |p: Poziadavka| {
            pary.iter().filter(|x| x.poziadavka == p).min_by(|a, b| a.pomer.total_cmp(&b.pomer)).copied()
        };
        let najmensi = |nazov: &str| {
            pary.iter().filter(|x| x.popredie == nazov).map(|x| x.pomer).fold(f64::MAX, f64::min)
        };
        let (tx, gr) = (min(Poziadavka::Text).unwrap(), min(Poziadavka::Grafika).unwrap());
        println!(
            "| {} ({}) | {:.2} : 1 ({} na {}) | {:.2} : 1 ({} na {}) | {:.2} : 1 | {:.2} : 1 | {} |",
            t.druh,
            t.meno,
            tx.pomer,
            tx.popredie,
            tx.pozadie,
            gr.pomer,
            gr.popredie,
            gr.pozadie,
            najmensi("text na akcente"),
            najmensi("akcent v texte"),
            pary.iter().filter(|p| p.poziadavka != Poziadavka::Vynimka).count()
        );
        for p in pary.iter().filter(|p| !p.splna()) {
            chyby += 1;
            eprintln!(
                "NESPĹŇA {}: {} na {} = {:.2}, treba {:.1}",
                t.meno, p.popredie, p.pozadie, p.pomer, p.minimum
            );
        }
    }
    std::process::exit(if chyby == 0 { 0 } else { 1 });
}
