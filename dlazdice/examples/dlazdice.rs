//! Meranie dlaždicového rastra a pyramídy náhľadu (R70, R71) na obraze 4K.
//!
//! Spustenie: `cargo run --release -p orchidea-dlazdice --example dlazdice`
//! Výstup je tabuľka v Markdowne – CI ju pripojí k súhrnu behu.

use orchidea_dlazdice::{Obdlznik, Pyramida, Rastr, strana_pre};
use orchidea_jadro::nahoda::Nahoda;
use orchidea_jadro::os::Os;
use orchidea_jadro::paralel;
use orchidea_jadro::signal::Signal;
use std::time::Instant;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn main() {
    let profil = orchidea_zariadenie::tohto_zariadenia();
    let strana = strana_pre(&profil, 4);
    let (w, h) = (3840usize, 2160usize);

    // obraz 4K RGBA: plynulý prechod s jemným šumom (ako fotografia, nie jednoliate plochy)
    let mut n = Nahoda::zo_semienka(42);
    let mut data = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let sum = (n.dalsie() % 9) as usize;
            data.extend_from_slice(&[
                ((x * 255 / w + sum) % 256) as u8,
                ((y * 255 / h + sum) % 256) as u8,
                (((x + y) / 32 + sum) % 256) as u8,
                255,
            ]);
        }
    }
    let osi = vec![
        Os::poradova("y", h).unwrap(),
        Os::poradova("x", w).unwrap(),
        Os::oznacena("kanál", &["R", "G", "B", "A"]).unwrap(),
    ];
    let signal = Signal::novy(osi, data).unwrap();

    println!("| Dlaždice a pyramída (4K RGBA, 8 bitov) | Výsledok |");
    println!("|---|---|");
    println!("| vlákna | {} |", paralel::pocet_vlakien());
    println!(
        "| L2 na jadro → strana dlaždice | {} → {strana} px |",
        profil.l2_na_jadro_kb.map_or("neznáma".into(), |k| format!("{k} kB"))
    );

    let t = Instant::now();
    let mut obraz = Rastr::<u8>::zo_signalu(&signal, strana).unwrap();
    println!("| signál → dlaždice | {:.1} ms |", ms(t));

    let t = Instant::now();
    let mut pyramida = Pyramida::postav(&obraz);
    let cela_stavba = ms(t);
    println!(
        "| pyramída celá ({} úrovní, {} dlaždíc) | {cela_stavba:.1} ms |",
        pyramida.urovne().len(),
        pyramida.prepocitane()
    );

    // 100 ťahov štetcom 32×32 px, po každom nová verzia v histórii a obnova pyramídy
    let mut historia = vec![obraz.clone()];
    let t = Instant::now();
    let mut prepocitane = 0;
    for i in 0..100usize {
        let (x, y) = ((i * 397) % (w - 32), (i * 211) % (h - 32));
        obraz.uprav(Obdlznik::novy(x, y, 32, 32), |_, _, p| p.copy_from_slice(&[255, 0, 0, 255]));
        pyramida = pyramida.obnov(&obraz);
        prepocitane += pyramida.prepocitane();
        historia.push(obraz.clone());
    }
    let tah = ms(t) / 100.0;
    println!(
        "| ťah 32×32 + obnova pyramídy | {tah:.2} ms ({:.0}× rýchlejšie ako celá pyramída; {:.1} dlaždice na ťah) |",
        cela_stavba / tah.max(1e-6),
        prepocitane as f64 / 100.0
    );

    let verzie: Vec<&Rastr<u8>> = historia.iter().collect();
    let spolu = Rastr::bajtov_spolu(&verzie) as f64 / 1e6;
    let bez = (w * h * 4 * historia.len()) as f64 / 1e6;
    println!("| história 101 verzií | {spolu:.1} MB (bez zdieľania {bez:.0} MB) |");

    // bitová zhoda: obnovená pyramída = postavená znova
    let znova = Pyramida::postav(&obraz);
    let zhoda = znova.urovne().iter().zip(pyramida.urovne()).all(|(a, b)| a.odtlacok() == b.odtlacok());
    println!("| obnovená = postavená znova | {} |", if zhoda { "áno, bitovo" } else { "NIE" });

    // náhľad celého obrazu na obrazovku Full HD a výrez 100 % pri priblížení
    let t = Instant::now();
    let (k, nahlad) = pyramida.nahlad(obraz.cely(), 1920.0 / w as f64).unwrap();
    println!("| náhľad na Full HD | úroveň {k}, {:?}, {:.1} ms |", nahlad.tvar(), ms(t));
    let t = Instant::now();
    let (_, vyrez) = pyramida.nahlad(Obdlznik::novy(1000, 500, 1920, 1080), 1.0).unwrap();
    println!("| výrez 1920×1080 pri 100 % | {:?}, {:.1} ms |", vyrez.tvar(), ms(t));

    let platno = Rastr::<u8>::novy(7680, 4320, strana, &[255, 255, 255, 255]).unwrap();
    let pp = Pyramida::postav(&platno);
    let vsetky: Vec<&Rastr<u8>> = pp.urovne().iter().collect();
    println!("| prázdne plátno 8K s pyramídou | {} B |", Rastr::bajtov_spolu(&vsetky));
    assert!(zhoda, "obnovená pyramída sa líši od novej");
}
