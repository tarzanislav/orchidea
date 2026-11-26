//! Meranie vlastného dekodéra HEVC pre iný program (rozhranie R4 v `ORCHIDEA/rozhrania.md`): jeden súbor .hevc
//! (Annex B) → na štandardný výstup riadky v pevnom tvare, ktoré sa dajú rozobrať bez knižníc.
//!
//! Použitie: `hevc_meranie [--vlakna N] [--rychlost] [--etapy] <súbor.hevc>`
//! - `--vlakna N` – počet vlákien dekódovania (predvolene počet jadier, najviac 8),
//! - `--rychlost` – bez MD5 obrazov a bez overovania SEI (čisté dekódovanie ako pri prehrávaní),
//! - `--etapy` – rozpis času po etapách (riadky `CAS`); meranie samo stojí niekoľko % času.
//!
//! Výstup (každý riadok začína kľúčovým slovom, polia `meno=hodnota` oddelené medzerou):
//! ```text
//! HEVC_MERANIE verzia=1 subor=<meno> vlakna=<n> rezim=md5|rychlost
//! OBRAZ poradie=<i> poc=<POC> sirka=<w> vyska=<h> md5=<32 hex>       (len v režime md5, v poradí výstupu)
//! VYSLEDOK obrazov=<n> cas_ms=<ms> obr_s=<x.y> sei_zhoda=<n> sei_nezhoda=<n> sei_bez=<n> chyby=<n> stav=<stav>
//! CHYBA <text>                                                         (0 až 5 riadkov)
//! CAS etapa=<meno> ms=<x.y> podiel=<x.y>                               (len s --etapy, 13 riadkov)
//! CAS_SPOLU stena_ms=<x> praca_ms=<x> vlakna=<n> vyuzitie=<x.y>        (len s --etapy)
//! ```
//! Etapy (vlastný čas, súčet cez všetky vlákna): `rozbor` (hlavné vlákno: NAL, hlavičky, správa obrazov),
//! `cabac` (CABAC a syntax CTU vrátane odvodenia pohybu), `intra`, `inter` (interpolácia a váženie),
//! `transformacia` (dekvantizácia, spätná transformácia, rezíduum), `odblokovanie`, `sao`, `zverejnenie`
//! (kopírovanie hotových riadkov a pohybu), `odtlacok` (overenie SEI), `ostatne`, a čakania: `cakanie_ref`
//! (na riadky referencie z iného vlákna), `cakanie_hlavne` (hlavné vlákno na rad úloh či koniec),
//! `necinnost` (pracovné vlákno bez úlohy). `podiel` = % z `praca_ms` (súčet etáp okrem čakaní); `vyuzitie` =
//! praca_ms / (stena_ms × vlákna) v %.
//! `md5` je MD5 orezaného obrazu YUV 4:2:0 (rovina Y, U, V za sebou; Main 10 po 2 bajtoch, malý koniec –
//! ako `ffmpeg -pix_fmt yuv420p10le -f framemd5`). `stav`: `ok` (bez chyby a bez nezhody so
//! SEI), `nezhoda` (odtlačok SEI nesedí), `chyba` (chyba dekódovania alebo mimo profilu). Návratový kód: 0 = ok,
//! 1 = nezhoda alebo chyba, 2 = zlé argumenty alebo súbor sa nedá prečítať.

use orchidea_hevc::jednotky_annex_b;
use orchidea_hevc::odtlacok::{Md5, hex};
use orchidea_hevc_dekoder::{Dekoder, Vystup};
use std::time::Instant;

fn pouzitie() -> ! {
    eprintln!("použitie: hevc_meranie [--vlakna N] [--rychlost] [--etapy] <súbor.hevc>");
    std::process::exit(2);
}

fn main() {
    let (mut vlakna, mut rychlost, mut etapy, mut subor) = (None, false, false, None);
    let mut argumenty = std::env::args().skip(1);
    while let Some(a) = argumenty.next() {
        match a.as_str() {
            "--vlakna" => {
                let n = argumenty.next().and_then(|v| v.parse::<usize>().ok()).filter(|&n| n > 0);
                vlakna = Some(n.unwrap_or_else(|| pouzitie()));
            }
            "--rychlost" => rychlost = true,
            "--etapy" => etapy = true,
            _ if subor.is_none() && !a.starts_with("--") => subor = Some(a),
            _ => pouzitie(),
        }
    }
    let Some(subor) = subor else { pouzitie() };
    let data = match std::fs::read(&subor) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("súbor {subor} sa nedá prečítať: {e}");
            std::process::exit(2);
        }
    };
    let meno =
        std::path::Path::new(&subor).file_name().map_or(subor.clone(), |m| m.to_string_lossy().into_owned());

    let mut d = if rychlost { Dekoder::na_prehravanie() } else { Dekoder::novy() };
    if let Some(n) = vlakna {
        d = d.s_vlaknami(n);
    }
    let n_vlakien =
        vlakna.unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()).min(8));
    println!(
        "HEVC_MERANIE verzia=1 subor={} vlakna={n_vlakien} rezim={}",
        meno.replace(' ', "_"),
        if rychlost { "rychlost" } else { "md5" }
    );

    orchidea_hevc_dekoder::etapy::zapni(etapy);
    let _ = orchidea_hevc_dekoder::etapy::vezmi();
    let start = Instant::now();
    let (mut poradie, mut yuv) = (0usize, Vec::new());
    let mut vypis = |obrazy: Vec<Vystup>| {
        for o in obrazy {
            if !rychlost {
                yuv.clear();
                o.zapis_yuv(&mut yuv);
                let mut m = Md5::novy();
                m.pridaj(&yuv);
                let [l, p, h, dl] = o.okno();
                println!(
                    "OBRAZ poradie={poradie} poc={} sirka={} vyska={} md5={}",
                    o.poc(),
                    o.sirka() - l - p,
                    o.vyska() - h - dl,
                    hex(&m.vysledok())
                );
            }
            poradie += 1;
        }
    };
    for j in jednotky_annex_b(&data) {
        vypis(d.nal(j));
    }
    vypis(d.koniec());
    let cas = start.elapsed().as_secs_f64();

    let s = &d.statistika;
    let stav = if !s.chyby.is_empty() {
        "chyba"
    } else if !s.nezhoda.is_empty() {
        "nezhoda"
    } else {
        "ok"
    };
    println!(
        "VYSLEDOK obrazov={poradie} cas_ms={:.0} obr_s={:.1} sei_zhoda={} sei_nezhoda={} sei_bez={} chyby={} stav={stav}",
        cas * 1000.0,
        poradie as f64 / cas.max(1e-9),
        s.zhoda,
        s.nezhoda.len(),
        s.bez_odtlacku,
        s.chyby.len()
    );
    for c in s.chyby.iter().take(5) {
        println!("CHYBA {}", c.replace('\n', " "));
    }
    if etapy {
        use orchidea_hevc_dekoder::etapy::{VSETKY, vezmi};
        let ns = vezmi();
        let praca: u64 = VSETKY.iter().filter(|e| e.je_praca()).map(|&e| ns[e as usize]).sum();
        for e in VSETKY {
            let v = ns[e as usize];
            println!(
                "CAS etapa={} ms={:.1} podiel={:.1}",
                e.meno(),
                v as f64 / 1e6,
                100.0 * v as f64 / praca.max(1) as f64
            );
        }
        println!(
            "CAS_SPOLU stena_ms={:.0} praca_ms={:.0} vlakna={n_vlakien} vyuzitie={:.1}",
            cas * 1000.0,
            praca as f64 / 1e6,
            100.0 * praca as f64 / 1e9 / (cas * n_vlakien as f64).max(1e-9)
        );
    }
    std::process::exit(if stav == "ok" { 0 } else { 1 });
}
