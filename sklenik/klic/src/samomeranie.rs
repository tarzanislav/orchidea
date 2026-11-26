//! SAMOMERANIE (R122) – KLÍČ sám zaznamená skúšku zariadenia, aby sa nemuselo fotiť obrazovku.
//!
//! Od chvíle, keď je systém pripravený (dobehla skúška pamäte a meranie karty), sa v 0., 5., 10. a 30. minúte
//! uloží snímka obrazovky (PNG) a textová správa a každých 30 s bod krivky teploty. Na konci pribudne
//! tabuľka krivky a jej súhrn (najvyššia teplota, časová konštanta ohrevu, ustálenie). Všetko sa ukladá do
//! priečinka `MERANIA` na oddiele karty, ktorý vidí aj Windows; každé zariadenie dostane poradové číslo
//! (`01-…`, `02-…`), nič sa neprepisuje, a všetky správy sa navyše pripájajú do jedného súboru `SPOLU.TXT`.
//!
//! Tu sú časti nezávislé od systému (mená súborov, poradie, krivka) – skúšajú sa na každom počítači.
//! Zápis na kartu je v `zaznam.rs`, riadenie v hlavnej slučke v `linux.rs`.

/// Minúty merania (od pripravenosti systému), v ktorých sa uloží snímka obrazovky a správa.
pub const MILNIKY_MIN: [u32; 4] = [0, 5, 10, 30];
/// Bod krivky teploty každých 30 s.
pub const KROK_KRIVKY_S: u64 = 30;
/// Priečinok na oddiele FAT karty.
pub const PRIECINOK: &str = "MERANIA";
/// Súbor, do ktorého sa pripájajú všetky správy všetkých zariadení.
pub const SPOLU: &str = "SPOLU.TXT";
/// Koniec riadku v správach – Windows (Poznámkový blok) ho zobrazí správne vždy.
pub const NL: &str = "\r\n";
/// Na mieste týchto značiek doplní zápis poradové číslo a nosič (zistia sa až pri pripojení karty).
pub const ZNACKA_PORADIA: &str = "{PORADIE}";
pub const ZNACKA_NOSICA: &str = "{NOSIC}";

/// Bod krivky teploty.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodKrivky {
    /// Sekundy od začiatku merania.
    pub s: u64,
    pub teplota: Option<f64>,
    /// Výkon, ktorý strážca dovoľuje (0–1).
    pub vykon: f64,
    /// Strop taktu od strážcu (MHz).
    pub strop_mhz: u32,
    /// Skutočný takt procesora (MHz).
    pub takt_mhz: Option<u32>,
    /// Vyťaženie procesora (0–1).
    pub vytazenie: Option<f64>,
    /// Odhad teploty okolia (R123).
    pub okolie: Option<f64>,
}

/// Nadpis oddielu správy.
pub fn nadpis(nazov: &str) -> String {
    let n = nazov.chars().count();
    format!("── {nazov} {}{NL}", "─".repeat(62usize.saturating_sub(n)))
}

/// Časť mena súboru: písmená bez diakritiky, číslice, bodka a pomlčka (ostatné sa zlúči do pomlčky).
pub fn bezpecne(text: &str) -> String {
    let mut s = String::new();
    for c in text.chars() {
        let c = match c {
            'á' | 'ä' => 'a',
            'Á' | 'Ä' => 'A',
            'č' => 'c',
            'Č' => 'C',
            'ď' => 'd',
            'ľ' | 'ĺ' => 'l',
            'ň' => 'n',
            'ŕ' | 'ř' => 'r',
            'ť' => 't',
            'é' | 'ě' => 'e',
            'É' => 'E',
            'í' => 'i',
            'ó' | 'ô' => 'o',
            'š' => 's',
            'Š' => 'S',
            'ú' | 'ů' => 'u',
            'ý' => 'y',
            'ž' => 'z',
            'Ž' => 'Z',
            c => c,
        };
        if c.is_ascii_alphanumeric() || c == '.' {
            s.push(c);
        } else if !s.is_empty() && !s.ends_with('-') {
            s.push('-');
        }
    }
    let s = s.trim_end_matches(['-', '.']);
    s.chars().take(48).collect::<String>().trim_end_matches(['-', '.']).to_string()
}

/// Meno súborov zariadenia (bez poradového čísla), napr. `RPi-3-B-Plus-Rev-1.3-SD-3f2a1c`.
pub fn predpona(model: &str, nosic: &str, seriove: Option<&str>) -> String {
    let model = model.replace("Raspberry Pi", "RPi").replace(" Model ", " ");
    let mut p = bezpecne(&model);
    if p.is_empty() {
        p = "zariadenie".into();
    }
    if !nosic.is_empty() {
        p = format!("{p}-{}", bezpecne(nosic));
    }
    if let Some(s) = seriove.map(|s| s.trim().trim_start_matches('0').to_ascii_lowercase())
        && !s.is_empty()
    {
        let s: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let koniec: String = s.chars().skip(s.chars().count().saturating_sub(6)).collect();
        if !koniec.is_empty() {
            p = format!("{p}-{koniec}");
        }
    }
    p
}

/// Ďalšie poradové číslo podľa mien v priečinku `MERANIA` (`07-…` → 8). Prázdny priečinok → 1.
pub fn dalsie_poradie<'a>(mena: impl IntoIterator<Item = &'a str>) -> u32 {
    mena.into_iter()
        .filter_map(|m| {
            let (cislo, _) = m.split_once('-')?;
            (!cislo.is_empty() && cislo.len() <= 4 && cislo.bytes().all(|b| b.is_ascii_digit()))
                .then(|| cislo.parse::<u32>().ok())
                .flatten()
        })
        .max()
        .map_or(1, |n| n + 1)
}

/// Čas merania `m:ss` (od hodiny `h:mm:ss`).
pub fn cas(s: u64) -> String {
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn stupne(t: Option<f64>) -> String {
    t.map_or("–".into(), |t| format!("{t:.1} °C"))
}

/// Tabuľka krivky teploty (jeden riadok na bod).
pub fn tabulka(body: &[BodKrivky]) -> String {
    let mut t = format!("    čas     teplota   výkon  takt (strop/skutočný)  vyťaženie   okolie{NL}");
    for b in body {
        t += &format!(
            "  {:>6}  {:>9}  {:>4.0} %  {:>5} / {:>5} MHz      {:>4}  {:>9}{NL}",
            cas(b.s),
            stupne(b.teplota),
            b.vykon * 100.0,
            b.strop_mhz,
            b.takt_mhz.map_or("–".into(), |m| m.to_string()),
            b.vytazenie.map_or("–".into(), |v| format!("{:.0} %", v * 100.0)),
            stupne(b.okolie),
        );
    }
    t
}

/// Teplota v bode najbližšom k času `s` (len ak je bod do 45 s od neho).
fn teplota_okolo(body: &[BodKrivky], s: u64) -> Option<f64> {
    let b = body.iter().filter(|b| b.teplota.is_some()).min_by_key(|b| b.s.abs_diff(s))?;
    (b.s.abs_diff(s) <= 45).then_some(b.teplota).flatten()
}

/// Súhrn krivky: teploty v míľnikoch, najvyššia, ohrev a jeho časová konštanta, ustálenie, zásahy strážcu.
pub fn suhrn(body: &[BodKrivky], milniky_min: &[u32]) -> String {
    let s_teplotou: Vec<(u64, f64)> = body.iter().filter_map(|b| Some((b.s, b.teplota?))).collect();
    let mut t = String::new();
    let Some(&(_, prva)) = s_teplotou.first() else {
        return format!("  teplota sa nemerala (doska nemá snímač){NL}");
    };
    let &(s_konca, posledna) = s_teplotou.last().unwrap_or(&(0, prva));
    let milniky: Vec<String> = milniky_min
        .iter()
        .filter_map(|&m| teplota_okolo(body, m as u64 * 60).map(|v| format!("{m}. min {v:.1}")))
        .collect();
    t += &format!("  teplota: {} °C{NL}", milniky.join(" · "));
    let (s_max, max) = s_teplotou.iter().fold((0, f64::MIN), |a, &(s, v)| if v > a.1 { (s, v) } else { a });
    let min = s_teplotou.iter().map(|x| x.1).fold(f64::MAX, f64::min);
    t += &format!("  najvyššia {max:.1} °C (v {}) · najnižšia {min:.1} °C{NL}", cas(s_max));
    let ohrev = posledna - prva;
    if ohrev.abs() < 1.0 {
        t += &format!("  ohrev: {ohrev:+.1} °C – teplota sa takmer nemenila{NL}");
    } else {
        // časová konštanta: kedy teplota prešla 63 % zmeny (pri ohreve podľa jednoduchého tepelného modelu)
        let hranica = prva + 0.632 * ohrev;
        let tau = s_teplotou
            .iter()
            .find(|&&(_, v)| if ohrev > 0.0 { v >= hranica } else { v <= hranica })
            .map(|x| x.0);
        t += &format!(
            "  ohrev: {ohrev:+.1} °C za {}{}{NL}",
            cas(s_konca),
            tau.map_or(String::new(), |s| format!(" · 63 % zmeny za {} (časová konštanta ≈ {s} s)", cas(s)))
        );
    }
    // ustálenie: od kedy sa teplota drží do ±0,5 °C od konečnej
    let ustalena_od = s_teplotou
        .iter()
        .rposition(|&(_, v)| (v - posledna).abs() > 0.5)
        .map_or(Some(s_teplotou[0].0), |i| s_teplotou.get(i + 1).map(|x| x.0));
    match ustalena_od {
        Some(s) if s < s_konca => {
            t += &format!("  ustálená od {} (±0,5 °C okolo {posledna:.1} °C){NL}", cas(s))
        }
        _ => t += &format!("  ešte sa neustálila – na konci sa stále menila{NL}"),
    }
    let (s_min_vykon, min_vykon) =
        body.iter().fold((0, f64::MAX), |a, b| if b.vykon < a.1 { (b.s, b.vykon) } else { a });
    if min_vykon >= 0.995 {
        t += &format!("  výkon: plný celý čas (tepelný regulátor nemusel zasiahnuť){NL}");
    } else {
        t += &format!(
            "  výkon: tepelný regulátor znížil výkon najviac na {:.0} % (v {}){NL}",
            min_vykon * 100.0,
            cas(s_min_vykon)
        );
    }
    // okolie na začiatku a na konci (R123) – stúplo, ak je zariadenie pri zdroji tepla
    let okolia: Vec<f64> = body.iter().filter_map(|b| b.okolie).collect();
    if let (Some(a), Some(z)) = (okolia.first(), okolia.last()) {
        t += &format!("  okolie: na začiatku {a:.1} °C, na konci {z:.1} °C ({:+.1} °C){NL}", z - a);
    }
    let vytazenie: Vec<f64> = body.iter().filter_map(|b| b.vytazenie).collect();
    if !vytazenie.is_empty() {
        t += &format!(
            "  vyťaženie procesora: priemer {:.0} %, najviac {:.0} %{NL}",
            100.0 * vytazenie.iter().sum::<f64>() / vytazenie.len() as f64,
            100.0 * vytazenie.iter().cloned().fold(0.0, f64::max)
        );
    }
    t
}

/// Je zavádzací sektor oddielu súborovým systémom FAT (12, 16 alebo 32)?
pub fn je_fat(sektor: &[u8]) -> bool {
    sektor.len() >= 512
        && sektor[510] == 0x55
        && sektor[511] == 0xAA
        && (&sektor[82..87] == b"FAT32" || &sektor[54..57] == b"FAT")
}

/// Výrobca karty SD alebo pamäte eMMC podľa čísla výrobcu v jej identifikácii (CID).
pub fn vyrobca_karty(typ: &str, manfid: u32) -> Option<&'static str> {
    let sd = [
        (0x01, "Panasonic"),
        (0x02, "Toshiba/Kioxia"),
        (0x03, "SanDisk"),
        (0x1B, "Samsung"),
        (0x1D, "ADATA"),
        (0x27, "Phison (Verbatim, PNY, …)"),
        (0x28, "Lexar"),
        (0x31, "Silicon Power"),
        (0x41, "Kingston"),
        (0x6F, "STEC/Netac"),
        (0x74, "Transcend"),
        (0x76, "Patriot"),
        (0x82, "Sony"),
        (0x9F, "Kingston (Canvas; ovládač 0x9F)"),
    ];
    let emmc = [
        (0x11, "Toshiba/Kioxia"),
        (0x13, "Micron"),
        (0x15, "Samsung"),
        (0x45, "SanDisk"),
        (0x90, "SK hynix"),
        (0xD6, "Foresee"),
    ];
    let tabulka: &[(u32, &str)] = if typ == "MMC" { &emmc } else { &sd };
    tabulka.iter().find(|v| v.0 == manfid).map(|v| v.1)
}

/// Údaje monitora z jeho popisu EDID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edid {
    /// Trojpísmenový kód výrobcu (napr. SAM, GSM, DEL).
    pub vyrobca: String,
    pub kod_vyrobku: u16,
    pub nazov: Option<String>,
    pub seriove: Option<String>,
    /// Rok výroby.
    pub rok: u16,
    /// Odporúčané (najlepšie) rozlíšenie monitora.
    pub rozlisenie: Option<(u32, u32)>,
}

/// Rozloží popis monitora EDID (prvých 128 bajtov; kontroluje hlavičku a kontrolný súčet).
pub fn rozloz_edid(d: &[u8]) -> Option<Edid> {
    if d.len() < 128 || d[..8] != [0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0] {
        return None;
    }
    if d[..128].iter().fold(0u8, |a, &b| a.wrapping_add(b)) != 0 {
        return None;
    }
    let k = u16::from_be_bytes([d[8], d[9]]);
    let pismeno = |x: u16| (b'A' - 1 + (x & 0x1F) as u8) as char;
    let vyrobca: String = [pismeno(k >> 10), pismeno(k >> 5), pismeno(k)].iter().collect();
    let mut e = Edid {
        vyrobca,
        kod_vyrobku: u16::from_le_bytes([d[10], d[11]]),
        nazov: None,
        seriove: None,
        rok: 1990 + d[17] as u16,
        rozlisenie: None,
    };
    for i in 0..4 {
        let b = &d[54 + 18 * i..72 + 18 * i];
        if b[0] != 0 || b[1] != 0 {
            // podrobné časovanie – prvé je odporúčaný režim
            if e.rozlisenie.is_none() {
                let w = b[2] as u32 | ((b[4] as u32 >> 4) << 8);
                let h = b[5] as u32 | ((b[7] as u32 >> 4) << 8);
                e.rozlisenie = Some((w, h));
            }
        } else {
            let text = || {
                let t: String = b[5..18].iter().take_while(|&&c| c != 0x0A).map(|&c| c as char).collect();
                Some(t.trim().to_string()).filter(|t| !t.is_empty())
            };
            match b[3] {
                0xFC => e.nazov = text(),
                0xFF => e.seriove = text(),
                _ => {}
            }
        }
    }
    Some(e)
}

/// Popis rozhrania USB podľa triedy (a protokolu pri HID).
pub fn rozhranie_usb(trieda: u8, podtrieda: u8, protokol: u8) -> &'static str {
    match (trieda, podtrieda, protokol) {
        (0x03, 0x01, 0x01) => "klávesnica",
        (0x03, 0x01, 0x02) => "myš",
        (0x03, _, _) => "HID",
        (0x01, _, _) => "zvuk",
        (0x02, _, _) | (0x0A, _, _) => "sieť/sériová linka",
        (0x07, _, _) => "tlačiareň",
        (0x08, _, _) => "úložisko",
        (0x09, _, _) => "rozbočovač",
        (0x0E, _, _) => "kamera",
        (0xE0, _, _) => "bezdrôtové (Bluetooth)",
        (0xFF, _, _) => "vlastné rozhranie výrobcu",
        _ => "iné",
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    fn bod(s: u64, t: f64) -> BodKrivky {
        BodKrivky {
            s,
            teplota: Some(t),
            vykon: 1.0,
            strop_mhz: 1400,
            takt_mhz: Some(600),
            vytazenie: Some(0.02),
            okolie: Some(22.0 + s as f64 / 1800.0),
        }
    }

    #[test]
    fn fat_karta_monitor_a_usb() {
        let mut sektor = [0u8; 512];
        assert!(!je_fat(&sektor));
        sektor[510..512].copy_from_slice(&[0x55, 0xAA]);
        sektor[82..90].copy_from_slice(b"FAT32   ");
        assert!(je_fat(&sektor));
        let mut f16 = [0u8; 512];
        f16[510..512].copy_from_slice(&[0x55, 0xAA]);
        f16[54..62].copy_from_slice(b"FAT16   ");
        assert!(je_fat(&f16) && !je_fat(&f16[..100]));
        assert_eq!(vyrobca_karty("SD", 0x03), Some("SanDisk"));
        assert_eq!(vyrobca_karty("MMC", 0x15), Some("Samsung"));
        assert_eq!(vyrobca_karty("SD", 0x15), None);
        // EDID: výrobca SAM, názov, sériové číslo, 1920×1080, rok 2019
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&[0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0]);
        // „SAM“ – každé písmeno 5 bitmi, A = 1
        let pismeno = |c: u8| (c - b'A' + 1) as u16;
        let k: u16 = pismeno(b'S') << 10 | pismeno(b'A') << 5 | pismeno(b'M');
        e[8..10].copy_from_slice(&k.to_be_bytes());
        e[10..12].copy_from_slice(&0x0F99u16.to_le_bytes());
        e[17] = 29;
        let t = &mut e[54..72];
        t[0] = 0x02;
        t[1] = 0x3A;
        t[2] = 0x80; // 1920 = 0x780
        t[4] = 0x70;
        t[5] = 0x38; // 1080 = 0x438
        t[7] = 0x40;
        e[72 + 3] = 0xFC;
        e[77..77 + 13].copy_from_slice(b"SAMSUNG\n     ");
        e[90 + 3] = 0xFF;
        e[95..95 + 13].copy_from_slice(b"H4ZN100123\n  ");
        let suma = e[..127].iter().fold(0u8, |a, &b| a.wrapping_add(b));
        e[127] = 0u8.wrapping_sub(suma);
        let m = rozloz_edid(&e).unwrap();
        assert_eq!(
            m,
            Edid {
                vyrobca: "SAM".into(),
                kod_vyrobku: 0x0F99,
                nazov: Some("SAMSUNG".into()),
                seriove: Some("H4ZN100123".into()),
                rok: 2019,
                rozlisenie: Some((1920, 1080)),
            }
        );
        e[127] ^= 1;
        assert_eq!(rozloz_edid(&e), None, "zlý kontrolný súčet");
        assert_eq!(rozhranie_usb(3, 1, 1), "klávesnica");
        assert_eq!(rozhranie_usb(3, 0, 0), "HID");
        assert_eq!(rozhranie_usb(8, 6, 0x50), "úložisko");
    }

    #[test]
    fn mena_suborov() {
        assert_eq!(bezpecne("Raspberry Pi 3 Model B Plus Rev 1.3"), "Raspberry-Pi-3-Model-B-Plus-Rev-1.3");
        assert_eq!(bezpecne("  Dielňa č. 1 / Tanix?"), "Dielna-c.-1-Tanix");
        assert_eq!(
            predpona("Raspberry Pi 3 Model B Plus Rev 1.3", "SD", Some("00000000a1b2c3d4")),
            "RPi-3-B-Plus-Rev-1.3-SD-b2c3d4"
        );
        assert_eq!(predpona("Hardkernel ODROID-C4", "eMMC", None), "Hardkernel-ODROID-C4-eMMC");
        assert_eq!(predpona("", "", Some("0000")), "zariadenie");
        assert!(predpona(&"x".repeat(300), "USB", None).len() <= 52);
    }

    #[test]
    fn poradie_pokracuje_za_najvyssim() {
        assert_eq!(dalsie_poradie([]), 1);
        assert_eq!(
            dalsie_poradie(["SPOLU.TXT", "01-RPi-3.txt", "01-RPi-3-00min.png", "07-X96.txt", "x-1"]),
            8
        );
        assert_eq!(dalsie_poradie(["12345-príliš-dlhé"]), 1);
    }

    #[test]
    fn cas_merania() {
        assert_eq!(cas(0), "0:00");
        assert_eq!(cas(1830), "30:30");
        assert_eq!(cas(3725), "1:02:05");
    }

    #[test]
    fn suhrn_ohrevu_ako_pri_rpi3() {
        // ohrev 40 → 50 °C s časovou konštantou 300 s
        let body: Vec<BodKrivky> =
            (0..=60).map(|i| bod(i * 30, 40.0 + 10.0 * (1.0 - (-((i * 30) as f64) / 300.0).exp()))).collect();
        let s = suhrn(&body, &MILNIKY_MIN);
        assert!(s.contains("0. min 40.0 · 5. min 46.3 · 10. min 48.6 · 30. min 50.0"), "{s}");
        assert!(s.contains("časová konštanta ≈ 300 s"), "{s}");
        assert!(s.contains("ustálená od"), "{s}");
        assert!(s.contains("plný celý čas"), "{s}");
        assert!(s.contains("priemer 2 %"), "{s}");
        assert!(s.contains("okolie: na začiatku 22.0 °C, na konci 23.0 °C (+1.0 °C)"), "{s}");
        let t = tabulka(&body[..2]);
        assert_eq!(t.lines().count(), 3);
        assert!(t.contains("40.0 °C") && t.contains("100 %") && t.contains("1400 /   600 MHz"), "{t}");
    }

    #[test]
    fn suhrn_bez_zmeny_so_strazcom_a_bez_snimaca() {
        let mut body: Vec<BodKrivky> = (0..=10).map(|i| bod(i * 30, 35.0 + (i % 2) as f64 * 0.2)).collect();
        body[4].vykon = 0.75;
        let s = suhrn(&body, &MILNIKY_MIN);
        assert!(s.contains("takmer nemenila") && s.contains("najviac na 75 % (v 2:00)"), "{s}");
        // stále stúpa – neustálila sa
        let rastie: Vec<BodKrivky> = (0..=10).map(|i| bod(i * 30, 40.0 + i as f64)).collect();
        assert!(suhrn(&rastie, &MILNIKY_MIN).contains("ešte sa neustálila"));
        let bez: Vec<BodKrivky> = (0..3).map(|i| BodKrivky { teplota: None, ..bod(i * 30, 0.0) }).collect();
        assert!(
            suhrn(&bez, &MILNIKY_MIN).contains("nemala") || suhrn(&bez, &MILNIKY_MIN).contains("nemerala")
        );
    }
}
