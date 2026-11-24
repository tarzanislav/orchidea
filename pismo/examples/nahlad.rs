//! Náhľad písma STONKA v termináli (znaky ako odtiene): `cargo run -p orchidea-pismo --example nahlad -- "Text" 24`
use orchidea_pismo::{Pismo, Platno};

fn main() {
    let mut a = std::env::args().skip(1);
    let text = a.next().unwrap_or_else(|| "ORCHIDEA OS – Žltý kôň úpel ďábelské ódy".into());
    let velkost: f32 = a.next().and_then(|v| v.parse().ok()).unwrap_or(20.0);
    let hrubka = (velkost * 0.12).max(1.2);
    let pismo = Pismo::stonka();
    let sirka = pismo.sirka(&text, velkost, hrubka).ceil() as usize + 8;
    let vyska = (velkost * 1.8).ceil() as usize;
    let mut p = Platno::nove(sirka, vyska, 0);
    p.text(&pismo, &text, 4.0, velkost * 1.35, velkost, hrubka, 0xFFFFFF);
    let odtiene = [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];
    for y in 0..vyska {
        let riadok: String =
            (0..sirka).map(|x| odtiene[((p.body[y * sirka + x] & 0xFF) as usize * 9) / 255]).collect();
        println!("{}", riadok.trim_end());
    }
}
