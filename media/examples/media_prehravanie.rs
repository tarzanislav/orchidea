//! Prehranie súboru jadrom prehrávania LÝRY (R5d, `media::prehravanie`) so simulovaným systémom – nástroj rozhodcu
//! `media/nastroje/over_prehravanie.py` a vzor pre udalostnú slučku KLÍČA (prúd A).
//!
//! Použitie: `media_prehravanie [--hz N] [--priechod] [--kanalov N] [--posun SEKUND] <súbor>`
//! - `--hz N` – obnovovacia frekvencia displeja (predvolene 60; kryštál displeja −60 ppm),
//! - `--priechod` – televízor vie AC-3 a E-AC-3 (priechod IEC 61937),
//! - `--kanalov N` – najviac kanálov LPCM televízora (predvolene 2 – viac kanálov sa zmieša na stereo),
//! - `--posun S` – po 2 s presný posun na S sekúnd a prehranie do konca.
//!
//! Simulovaný systém: dekodér videa so 4 buffermi OUTPUT a 8 CAPTURE, oneskorenie 2 obnovenia, vracia snímky
//! **v poradí zobrazenia** (preusporiadanie do 4 snímok – snímky B); zariadenie zvuku 0,5 s s kryštálom +120 ppm,
//! dopĺňanie pri polovici, meranie po každom zápise; časy hlásení s chvením ±2 ms.
//!
//! Výstup (polia `meno=hodnota`):
//! ```text
//! PREHRAVANIE kontajner=<f> video=<tok|-> zvuk=<kodek|-> cesta=<cesta|-> frekvencia=<Hz|-> kanalov=<n|->
//! PV pts=<ns>                                   (každý paket videa v poradí odovzdania dekodéru)
//! SNIMKY prijatych=<n> zobrazenych=<n> vynechanych=<n> zahodenych=<n> poradie=ok|chyba odchylka_max_us=<n> odchylka_priemer_us=<n>
//! ZVUK blokov=<n> ramcov=<n> bajtov=<n> md5=<32 hex> prvy_pts=<ns|->
//! POSUN ciel_ns=<n> cas_ns=<n> prva_snimka_pts=<ns|-> prvy_zvuk_pts=<ns|->   (len s --posun)
//! VYSLEDOK stav=ok|chyba
//! ```

use orchidea_hevc::odtlacok::{Md5, hex};
use orchidea_media::hdmi::{CestaZvuku, VolbyZvuku};
use orchidea_media::prehravanie::{Prehravanie, PrijatieSnimky, VolbyPrehravania};
use orchidea_media::r5::{FormatVzoriekHdmi, MeranieZvuku, Obnova, SchopnostiHdmi};
use orchidea_media::{Limity, Posun, SuborovyZdroj, otvor};
use std::collections::{HashMap, VecDeque};

fn pouzitie() -> ! {
    eprintln!("použitie: media_prehravanie [--hz N] [--priechod] [--kanalov N] [--posun SEKUND] <súbor>");
    std::process::exit(2);
}

struct Sim {
    p: Prehravanie,
    t: i64,
    perioda: i64,
    // dekodér videa: (id, pts, hotové v čase) v poradí dekódovania, hotové snímky čakajúce na preusporiadanie
    v_dekoderi: VecDeque<(u64, i64, i64)>,
    hotove: Vec<(i64, u64)>,
    drzane: usize,
    pts_id: HashMap<u64, i64>,
    koniec_paketov: bool,
    // štatistika obrazu
    prijatych: u64,
    zahodenych: u64,
    zobrazene: Vec<i64>,
    vynechanych: u64,
    // zvuk
    odovzdanych: u64,
    zahranych: f64,
    frekvencia_linky: f64,
    blokov: u64,
    ramcov: u64,
    bajtov: u64,
    md5: Md5,
    prvy_zvuk: Option<i64>,
    koniec_zvuku: bool,
    zvuk_ide: bool,
}

impl Sim {
    /// Jedno obnovenie displeja.
    fn krok(&mut self) {
        // dekodér: hotové pakety → vnútorná fronta, výstup v poradí zobrazenia (preusporiadanie do 4 snímok)
        while self.v_dekoderi.front().is_some_and(|&(_, _, h)| h <= self.t) {
            let (id, pts, _) = self.v_dekoderi.pop_front().unwrap_or_default();
            self.hotove.push((pts, id));
        }
        self.hotove.sort();
        while self.hotove.len() > 4
            || (self.koniec_paketov && self.v_dekoderi.is_empty() && !self.hotove.is_empty())
        {
            let (_, id) = self.hotove.remove(0);
            match self.p.snimka(id) {
                PrijatieSnimky::Caka => {
                    self.prijatych += 1;
                    self.drzane += 1;
                }
                PrijatieSnimky::Zahod => self.zahodenych += 1,
            }
        }
        while self.v_dekoderi.len() < 4 && self.v_dekoderi.len() + self.hotove.len() + self.drzane < 12 {
            match self.p.dalsi_paket_videa() {
                Ok(Some(pv)) => {
                    let pts = pv.pts_ns.unwrap_or(i64::MIN);
                    println!("PV pts={pts}");
                    self.pts_id.insert(pv.id, pts);
                    self.v_dekoderi.push_back((pv.id, pts, self.t + 2 * self.perioda));
                }
                Ok(None) => {
                    self.koniec_paketov = self.p.stav(self.t).koniec_videa;
                    break;
                }
                Err(e) => {
                    eprintln!("video: {e}");
                    break;
                }
            }
        }
        // zvuk: hrá, kým má čo; budenie pri polovici 0,5 s, doplnenie, meranie po zápise
        if self.zvuk_ide {
            self.zahranych = (self.zahranych
                + self.perioda as f64 * self.frekvencia_linky * (1.0 + 120e-6) / 1e9)
                .min(self.odovzdanych as f64);
        }
        let pol = self.frekvencia_linky * 0.25;
        if !self.koniec_zvuku && (self.odovzdanych as f64 - self.zahranych) < pol {
            let pred = self.odovzdanych;
            while (self.odovzdanych as f64 - self.zahranych) < 2.0 * pol {
                match self.p.dalsi_zvuk() {
                    Ok(Some(z)) => {
                        if self.prvy_zvuk.is_none() {
                            self.prvy_zvuk = z.pts_ns;
                        }
                        self.blokov += 1;
                        self.ramcov += z.ramcov as u64;
                        self.bajtov += z.data.len() as u64;
                        self.md5.pridaj(&z.data);
                        self.odovzdanych += z.ramcov as u64;
                    }
                    Ok(None) => {
                        self.koniec_zvuku = self.p.stav(self.t).koniec_zvuku;
                        break;
                    }
                    Err(e) => {
                        eprintln!("zvuk: {e}");
                        self.koniec_zvuku = true;
                        break;
                    }
                }
            }
            if self.odovzdanych > pred {
                self.zvuk_ide = true;
                let chvenie = ((self.t / self.perioda) % 5 - 2) * 1_000_000;
                self.p.meranie_zvuku(MeranieZvuku {
                    odovzdanych_ramcov: self.odovzdanych,
                    oneskorenie_ramcov: (self.odovzdanych as f64 - self.zahranych) as u64,
                    monotonny_ns: self.t + chvenie,
                });
            }
        }
        let v = self.p.obnova(Obnova { monotonny_ns: self.t, perioda_ns: self.perioda });
        self.vynechanych += v.vynechane.len() as u64;
        self.drzane = self.drzane.saturating_sub(v.vynechane.len() + v.nahradena.is_some() as usize);
        if let Some(id) = v.zobraz {
            self.zobrazene.push(self.pts_id.get(&id).copied().unwrap_or(i64::MIN));
        }
        self.t += self.perioda;
    }

    fn hotovo(&self) -> bool {
        // všetky snímky odovzdané stroju a zobrazené (drží sa len posledná zobrazená)
        let video_hotove =
            self.koniec_paketov && self.v_dekoderi.is_empty() && self.hotove.is_empty() && self.drzane <= 1;
        let zvuk_hotovy = self.koniec_zvuku && self.zahranych >= self.odovzdanych as f64;
        video_hotove && zvuk_hotovy
    }

    fn po_posune(&mut self) {
        self.v_dekoderi.clear();
        self.hotove.clear();
        self.koniec_paketov = false;
        self.odovzdanych = 0;
        self.zahranych = 0.0;
        self.koniec_zvuku = false;
        self.zvuk_ide = false;
    }
}

fn main() {
    let (mut hz, mut priechod, mut kanalov, mut posun, mut subor) = (60.0f64, false, 2u8, None, None);
    let mut a = std::env::args().skip(1);
    while let Some(x) = a.next() {
        match x.as_str() {
            "--hz" => {
                hz = a
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|&h: &f64| h >= 10.0)
                    .unwrap_or_else(|| pouzitie())
            }
            "--priechod" => priechod = true,
            "--kanalov" => {
                kanalov = a
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|&k| (1..=8).contains(&k))
                    .unwrap_or_else(|| pouzitie())
            }
            "--posun" => {
                posun = Some(a.next().and_then(|v| v.parse::<f64>().ok()).unwrap_or_else(|| pouzitie()))
            }
            _ if subor.is_none() && !x.starts_with("--") => subor = Some(x),
            _ => pouzitie(),
        }
    }
    let Some(subor) = subor else { pouzitie() };
    let k = match SuborovyZdroj::otvor(&subor).and_then(|z| otvor(z, Limity::default())) {
        Ok(k) => k,
        Err(e) => {
            println!("VYSLEDOK stav=chyba");
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let schopnosti = SchopnostiHdmi {
        formaty: vec![FormatVzoriekHdmi::S16Le],
        max_kanalov: kanalov,
        frekvencie: vec![32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000],
        max_bitov: 16,
        priechod_ac3: priechod,
        priechod_eac3: priechod,
        oneskorenie_tv_ns: None,
    };
    let volby =
        VolbyPrehravania { zvuk: VolbyZvuku { priechod, stereo: false }, ..VolbyPrehravania::default() };
    let p = match Prehravanie::otvor(k, &schopnosti, &volby) {
        Ok(p) => p,
        Err(e) => {
            println!("VYSLEDOK stav=chyba");
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let i = p.info().clone();
    let cesta = |c: CestaZvuku| match c {
        CestaZvuku::Priechod(_) => "priechod",
        CestaZvuku::BitovoPresne => "bitovo_presne",
        CestaZvuku::Pcm => "pcm",
        CestaZvuku::ZmiesanieNaStereo => "zmiesanie",
    };
    println!(
        "PREHRAVANIE kontajner={} video={} zvuk={} cesta={} frekvencia={} kanalov={}",
        i.format.meno().replace(' ', "_"),
        i.video.as_ref().map_or("-".into(), |v| format!("{:?}", v.tok)),
        i.zvuk.as_ref().map_or("-".into(), |z| z.kodek.meno().replace(' ', "_")),
        i.zvuk.as_ref().map_or("-", |z| cesta(z.cesta)),
        i.zvuk.as_ref().map_or("-".into(), |z| z.nastavenie.frekvencia.to_string()),
        i.zvuk.as_ref().map_or("-".into(), |z| z.nastavenie.kanalov.to_string()),
    );
    let frekvencia_linky = i.zvuk.as_ref().map_or(48_000.0, |z| z.nastavenie.frekvencia as f64);
    let mut s = Sim {
        p,
        t: 1_000_000_000,
        perioda: (1e9 / hz * (1.0 + 60e-6)) as i64,
        v_dekoderi: VecDeque::new(),
        hotove: Vec::new(),
        drzane: 0,
        pts_id: HashMap::new(),
        koniec_paketov: i.video.is_none(),
        prijatych: 0,
        zahodenych: 0,
        zobrazene: Vec::new(),
        vynechanych: 0,
        odovzdanych: 0,
        zahranych: 0.0,
        frekvencia_linky,
        blokov: 0,
        ramcov: 0,
        bajtov: 0,
        md5: Md5::novy(),
        prvy_zvuk: None,
        koniec_zvuku: i.zvuk.is_none(),
        zvuk_ide: false,
    };
    let strop = s.t + i.trvanie_ns.unwrap_or(600_000_000_000).clamp(1, 7_200_000_000_000) + 30_000_000_000;
    let mut posun_riadok = None;
    if let Some(ciel) = posun {
        for _ in 0..(2.0 * hz) as usize {
            s.krok();
        }
        let ciel_ns = (ciel * 1e9) as i64;
        match s.p.posun(ciel_ns, Posun::Presny) {
            Ok(r) => {
                s.drzane = s.drzane.saturating_sub(r.uvolni.len());
                s.po_posune();
                let pred = s.zobrazene.len();
                s.prvy_zvuk = None;
                while !s.hotovo() && s.t < strop {
                    s.krok();
                }
                posun_riadok = Some(format!(
                    "POSUN ciel_ns={ciel_ns} cas_ns={} prva_snimka_pts={} prvy_zvuk_pts={}",
                    r.cas_ns,
                    s.zobrazene.get(pred).map_or("-".into(), |t| t.to_string()),
                    s.prvy_zvuk.map_or("-".into(), |t| t.to_string()),
                ));
                // štatistika poradia len po posune
                s.zobrazene.drain(..pred);
            }
            Err(e) => {
                println!("VYSLEDOK stav=chyba");
                eprintln!("posun: {e}");
                std::process::exit(1);
            }
        }
    } else {
        while !s.hotovo() && s.t < strop {
            s.krok();
        }
    }
    let poradie = s.zobrazene.windows(2).all(|w| w[0] < w[1]);
    let st = s.p.stav(s.t).synchronizacia;
    println!(
        "SNIMKY prijatych={} zobrazenych={} vynechanych={} zahodenych={} poradie={} odchylka_max_us={} odchylka_priemer_us={}",
        s.prijatych,
        s.zobrazene.len(),
        s.vynechanych,
        s.zahodenych,
        if poradie { "ok" } else { "chyba" },
        st.najvacsia_odchylka_ns / 1000,
        st.priemerna_odchylka_ns() / 1000
    );
    println!(
        "ZVUK blokov={} ramcov={} bajtov={} md5={} prvy_pts={}",
        s.blokov,
        s.ramcov,
        s.bajtov,
        hex(&s.md5.clone().vysledok()),
        s.prvy_zvuk.map_or("-".into(), |t| t.to_string())
    );
    if let Some(r) = posun_riadok {
        println!("{r}");
    }
    let ok = s.hotovo() && poradie;
    println!("VYSLEDOK stav={}", if ok { "ok" } else { "chyba" });
    std::process::exit(if ok { 0 } else { 1 });
}
