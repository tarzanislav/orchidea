//! Vypíše dostupné výpočtové zariadenia a dôvody, prečo niektoré nie sú dostupné.
//! Spustenie: `cargo run -p orchidea-gpu --example zariadenia`

fn main() {
    let (zariadenia, dovody) = orchidea_gpu::vsetky_zariadenia();
    println!("| Zariadenie | Typ | Ovládač |");
    println!("|---|---|---|");
    for z in &zariadenia {
        let p = z.popis();
        println!("| {:?} – {} | {:?} | {} |", p.druh, p.meno, p.typ, p.ovladac);
    }
    for d in &dovody {
        println!("| nedostupné | – | {d} |");
    }
}
