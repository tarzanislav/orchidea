//! Výstup prevodníka zvuku pre HDMI (R5b, etapa L5) – na porovnanie s rozhodcom: prvá zvuková stopa súboru ide
//! cez [`PrevodnikZvuku`] pre zvolené schopnosti ovládača a televízora; vypíše nastavenie a MD5 všetkých bajtov,
//! ktoré by dostal ovládač.
//!
//! Použitie: `media_hdmi [--format s16|s32|iec958] [--kanalov N] [--bitov N] [--bez-priechodu] [--stereo]
//! [--surove <výstup>] <súbor>` – na stdout:
//! ```text
//! NASTAVENIE cesta=<priechod_ac3|priechod_eac3|bitovo_presne|pcm|zmiesanie> format=<…> frekvencia=<Hz> kanalov=<n> ca=<hex> stav=<5 B hex>
//! VYSLEDOK ramcov=<n> bajtov=<n> md5=<32 hex>
//! ```
//! Návratový kód: 0 v poriadku, 1 chyba súboru alebo prevodu, 2 zlé argumenty.

use orchidea_hevc::odtlacok::{Md5, hex};
use orchidea_media::r5::{FormatVzoriekHdmi, SchopnostiHdmi};
use orchidea_media::{
    BlokZvuku, CestaZvuku, DruhStopy, Limity, PrevodnikZvuku, RozlozenieKanalov, SuborovyZdroj, VolbyZvuku,
    dekoder_zvuku, otvor,
};
use std::io::Write;

fn chyba(text: impl std::fmt::Display) -> ! {
    println!("CHYBA {text}");
    std::process::exit(1)
}

fn main() {
    let mut a: Vec<String> = std::env::args().skip(1).collect();
    let mut hodnota = |meno: &str| -> Option<String> {
        let i = a.iter().position(|x| x == meno)?;
        let v = a.get(i + 1).cloned();
        a.drain(i..(i + 2).min(a.len()));
        v
    };
    let format = hodnota("--format").unwrap_or_else(|| "s32".into());
    let kanalov: u8 = hodnota("--kanalov").and_then(|v| v.parse().ok()).unwrap_or(8);
    let bitov: u8 = hodnota("--bitov").and_then(|v| v.parse().ok()).unwrap_or(24);
    let surove = hodnota("--surove");
    let mut prepinac = |meno: &str| match a.iter().position(|x| x == meno) {
        Some(i) => {
            a.remove(i);
            true
        }
        None => false,
    };
    let volby = VolbyZvuku { priechod: !prepinac("--bez-priechodu"), stereo: prepinac("--stereo") };
    let Some(cesta) = a.first() else {
        eprintln!(
            "použitie: media_hdmi [--format s16|s32|iec958] [--kanalov N] [--bitov N] [--bez-priechodu] [--stereo] [--surove <výstup>] <súbor>"
        );
        std::process::exit(2);
    };
    let formaty = match format.as_str() {
        "s16" => vec![FormatVzoriekHdmi::S16Le],
        "s32" => vec![FormatVzoriekHdmi::S32Le, FormatVzoriekHdmi::S16Le],
        "iec958" => vec![FormatVzoriekHdmi::Iec958PodramecLe],
        _ => {
            eprintln!("neznámy formát {format}");
            std::process::exit(2)
        }
    };
    let schopnosti = SchopnostiHdmi {
        formaty,
        max_kanalov: kanalov,
        frekvencie: vec![],
        max_bitov: bitov,
        priechod_ac3: true,
        priechod_eac3: true,
        oneskorenie_tv_ns: None,
    };
    let zdroj = SuborovyZdroj::otvor(cesta).unwrap_or_else(|e| chyba(e));
    let mut k = otvor(zdroj, Limity::default()).unwrap_or_else(|e| chyba(e));
    let ix = k
        .stopy()
        .iter()
        .position(|s| s.druh == DruhStopy::Zvuk)
        .unwrap_or_else(|| chyba("bez zvukovej stopy"));
    let stopa = k.stopy()[ix].clone();
    let mut p = PrevodnikZvuku::novy(&stopa, &schopnosti, volby).unwrap_or_else(|e| chyba(e));
    let n = p.nastavenie().clone();
    let meno_cesty = match p.cesta() {
        CestaZvuku::Priechod(orchidea_media::r5::DruhPriechodu::Ac3) => "priechod_ac3",
        CestaZvuku::Priechod(_) => "priechod_eac3",
        CestaZvuku::BitovoPresne => "bitovo_presne",
        CestaZvuku::Pcm => "pcm",
        CestaZvuku::ZmiesanieNaStereo => "zmiesanie",
    };
    println!(
        "NASTAVENIE cesta={meno_cesty} format={:?} frekvencia={} kanalov={} ca=0x{:02x} stav={}",
        n.format,
        n.frekvencia,
        n.kanalov,
        n.pridelenie_cta,
        n.stav_kanala[..5].iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let priechod = matches!(p.cesta(), CestaZvuku::Priechod(_));
    let mut d = if priechod { None } else { Some(dekoder_zvuku(&stopa).unwrap_or_else(|e| chyba(e))) };
    let mut blok = BlokZvuku::cele(0, RozlozenieKanalov::STEREO, 16);
    let mut md5 = Md5::novy();
    let mut vystup =
        surove.map(|c| std::io::BufWriter::new(std::fs::File::create(c).expect("výstupný súbor")));
    let (mut ramcov, mut bajtov) = (0u64, 0u64);
    while let Some(paket) = k.dalsi_paket().unwrap_or_else(|e| chyba(e)) {
        if paket.stopa != ix {
            continue;
        }
        let z = match &mut d {
            None => p.priechod(&paket),
            Some(d) => d.dekoduj(&paket, &mut blok).and_then(|_| p.pcm(&blok)),
        }
        .unwrap_or_else(|e| chyba(e));
        md5.pridaj(&z.data);
        if let Some(v) = &mut vystup {
            v.write_all(&z.data).expect("zápis");
        }
        ramcov += z.ramcov as u64;
        bajtov += z.data.len() as u64;
    }
    if let Some(mut v) = vystup {
        v.flush().expect("zápis");
    }
    println!("VYSLEDOK ramcov={ramcov} bajtov={bajtov} md5={}", hex(&md5.vysledok()));
}
