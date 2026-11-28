//! **Jadro prehrávania LÝRY (R5d, úloha D2)** – neutrálny stroj, ktorý spojí kontajner, prevod videa pre hardvérový
//! dekodér (R5a), dekódovanie a prevod zvuku pre HDMI (R5b), hodiny prehrávania z meraní zvuku (R5c) a výber snímky
//! pri každom obnovení displeja (L6).
//!
//! **Kto ho volá:** udalostná slučka ORCHIDEA OS (prúd A – KLÍČ; neskôr LÝRA na PC). Stroj nemá vlákna, nečaká
//! a nič nezisťuje sám – každé volanie je krátke a odpovedá na udalosť:
//!
//! | udalosť v systéme | volanie |
//! |---|---|
//! | V4L2 má voľný buffer OUTPUT | [`Prehravanie::dalsi_paket_videa`] |
//! | V4L2 vrátil dekódovanú snímku (CAPTURE, časová značka = `id`) | [`Prehravanie::snimka`] |
//! | blíži sa obnovenie displeja (udalosť DRM) | [`Prehravanie::obnova`] → ktorú snímku zobraziť, ktoré uvoľniť |
//! | ALSA má miesto (dopĺňanie pri polovici vyrovnávacej pamäte – R306) | [`Prehravanie::dalsi_zvuk`] (opakovane) |
//! | po zápise do zariadenia zvuku (stačí 4× za sekundu) | [`Prehravanie::meranie_zvuku`] |
//! | používateľ: pauza, pokračovanie, posun | [`Prehravanie::pauza`], [`Prehravanie::pokracuj`], [`Prehravanie::posun`] |
//!
//! **Chladne:** pakety sa čítajú v poradí súboru (čítačky čítajú dopredu po dávkach – úložisko medzi dávkami spí);
//! fronty stôp sú ohraničené, takže čítanie nepredbehne prehrávanie o viac, ako treba; zvuk sa dekóduje len vtedy,
//! keď má zariadenie miesto (≈ 4 budenia za sekundu), pri priechode AC-3/E-AC-3 sa nedekóduje vôbec.
//!
//! **Čas:** všetko na časovej osi média v ns. Hodiny prehrávania vzniknú z prvého bloku zvuku po začiatku alebo po
//! posune (`HodinyPrehravania::zo_zvuku`); súbor bez zvuku má monotónne hodiny od prvého obnovenia displeja.

use crate::cas::na_ns;
use crate::hdmi::{CestaZvuku, PrevodnikZvuku, VolbyZvuku};
use crate::hodiny::{HodinyPrehravania, StatistikaSynchronizacie, Synchronizacia};
use crate::kontajner::{FormatKontajnera, Kontajner, Posun};
use crate::paket::Paket;
use crate::r5::{
    MeranieZvuku, NastavenieHdmi, Obnova, PaketVidea, PrevodnikVidea, SchopnostiHdmi, TokVidea, ZvukHdmi,
};
use crate::stopa::{DruhStopy, Kodek, ParametreVidea, Stopa};
use crate::zvuk::{BlokZvuku, DekoderZvuku, dekoder_zvuku, orez};
use orchidea_jadro::{Chyba, Vysledok};
use std::collections::VecDeque;

/// Voľby prehrávania.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolbyPrehravania {
    pub zvuk: VolbyZvuku,
    /// Výslovne zvolené stopy (index do `Kontajner::stopy()`); inak predvolená, potom prvá použiteľná.
    pub stopa_videa: Option<usize>,
    pub stopa_zvuku: Option<usize>,
    /// Uprednostnený jazyk zvuku (napr. „sk“, „slk“, „slo“, „sk-SK“ – ISO 639-1, 639-2/B a /T aj BCP 47).
    pub jazyk_zvuku: Option<String>,
    /// Oneskorenie obrazu voči zvuku na výstupe (rovina + televízor), ns – pozri `Synchronizacia`.
    pub oneskorenie_obrazu_ns: i64,
    /// Najviac bajtov paketov čakajúcich vo fronte jednej stopy (čítanie dopredu kvôli prekladaniu stôp).
    pub max_fronta_bajtov: usize,
}

impl Default for VolbyPrehravania {
    fn default() -> Self {
        VolbyPrehravania {
            zvuk: VolbyZvuku::default(),
            stopa_videa: None,
            stopa_zvuku: None,
            jazyk_zvuku: None,
            oneskorenie_obrazu_ns: 0,
            max_fronta_bajtov: 64 << 20,
        }
    }
}

/// Zvolené video.
#[derive(Debug, Clone, PartialEq)]
pub struct InfoVidea {
    pub stopa: usize,
    pub tok: TokVidea,
    /// Rozmery, snímková frekvencia (pre výber obnovy displeja podľa videa) a farba.
    pub parametre: Option<ParametreVidea>,
}

/// Zvolený zvuk.
#[derive(Debug, Clone, PartialEq)]
pub struct InfoZvuku {
    pub stopa: usize,
    pub kodek: Kodek,
    pub cesta: CestaZvuku,
    /// Nastavenie zariadenia HDMI (formát, frekvencia linky, kanály, CA, stav kanála IEC 60958).
    pub nastavenie: NastavenieHdmi,
}

/// Čo sa prehráva.
#[derive(Debug, Clone, PartialEq)]
pub struct InfoPrehravania {
    pub format: FormatKontajnera,
    pub trvanie_ns: Option<i64>,
    pub video: Option<InfoVidea>,
    pub zvuk: Option<InfoZvuku>,
    /// Prečo sa zvuk neprehrá (stopa je, ale kodek alebo výstup nejde) – na zobrazenie v LÝRE.
    pub chyba_zvuku: Option<String>,
    pub chyba_videa: Option<String>,
}

/// Čo urobiť so snímkou, ktorú dekodér práve vrátil.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrijatieSnimky {
    /// Snímka čaká na svoje obnovenie displeja (stroj ju vráti v `VyberSnimky`).
    Caka,
    /// Snímku hneď uvoľniť (je pred cieľom presného posunu alebo patrí k toku pred posunom).
    Zahod,
}

/// Rozhodnutie pri obnovení displeja.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VyberSnimky {
    /// Snímka (`id`), ktorá sa má zobraziť od tohto obnovenia; `None` = ostáva doterajšia.
    pub zobraz: Option<u64>,
    /// Snímky uvoľnené bez zobrazenia (dobiehanie zvuku).
    pub vynechane: Vec<u64>,
    /// Doterajšia zobrazená snímka – uvoľniť až po dokončení prepnutia na `zobraz`.
    pub nahradena: Option<u64>,
}

/// Stav na zobrazenie a do štatistiky.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StavPrehravania {
    /// Čas média v ns v okamihu `t` (podľa hodín prehrávania), ak sú hodiny už v chode.
    pub cas_ns: Option<i64>,
    pub pauza: bool,
    /// Súbor nemá ďalšie pakety videa, resp. zvuku (fronty sú prázdne).
    pub koniec_videa: bool,
    pub koniec_zvuku: bool,
    pub synchronizacia: StatistikaSynchronizacie,
    /// Skoky hodín (podtečenie zvuku, chyba) – pozri `HodinyPrehravania`.
    pub skokov_hodin: u64,
    pub chybnych_paketov_zvuku: u64,
    pub chybnych_paketov_videa: u64,
    /// Snímky, ktoré dekodér vrátil s neznámou časovou značkou (pred posunom) – uvoľnené.
    pub neznamych_snimok: u64,
}

/// Výsledok posunu.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Posunute {
    /// Čas, od ktorého sa číta (prvý kľúčový paket); pri presnom posune sa zobrazí a zahrá až od cieľa.
    pub cas_ns: i64,
    /// Čakajúce dekódované snímky – uvoľniť (dekodér treba vyprázdniť: prvý ďalší paket má `prerusenie`).
    pub uvolni: Vec<u64>,
}

#[derive(Debug)]
struct Video {
    index: usize,
    prevodnik: PrevodnikVidea,
    fronta: VecDeque<Paket>,
    bajtov: usize,
    /// `id` odoslaných paketov a ich čas zobrazenia (najviac `MAX_ODOSLANYCH`).
    odoslane: VecDeque<(u64, Option<i64>)>,
    /// Dekódované snímky čakajúce na obnovenie: (pts, id) vzostupne podľa pts.
    cakajuce: Vec<(i64, u64)>,
    zobrazena: Option<u64>,
}

#[derive(Debug)]
struct Zvuk {
    index: usize,
    stopa: Stopa,
    dekoder: Option<Box<dyn DekoderZvuku>>,
    prevodnik: PrevodnikZvuku,
    fronta: VecDeque<Paket>,
    bajtov: usize,
    blok: BlokZvuku,
}

/// Dekodér HEVC/H.264 drží len niekoľko snímok; viac odoslaných `id` netreba pamätať.
const MAX_ODOSLANYCH: usize = 512;
/// Najviac čakajúcich dekódovaných snímok (dekodér ich viac nemá – CAPTURE buffery).
const MAX_CAKAJUCICH: usize = 64;
/// Ak dekodér obrazu do 2 s nevráti snímku, zvuk sa spustí aj tak.
const NAJDLHSIE_CAKANIE_NA_OBRAZ_NS: i64 = 2_000_000_000;

/// Jadro prehrávania jedného súboru.
#[derive(Debug)]
pub struct Prehravanie {
    k: Box<dyn Kontajner>,
    info: InfoPrehravania,
    video: Option<Video>,
    zvuk: Option<Zvuk>,
    max_fronta: usize,
    koniec_suboru: bool,
    hodiny: Option<HodinyPrehravania>,
    sync: Synchronizacia,
    pauza: bool,
    /// Presný posun: snímky a vzorky pred týmto časom sa nezobrazia ani nezahrajú.
    ciel_ns: Option<i64>,
    /// Po začiatku a po posune zvuk čaká na prvú dekódovanú snímku (inak by hodiny zo zvuku utiekli obrazu);
    /// poistka: najviac `NAJDLHSIE_CAKANIE_NA_OBRAZ_NS` podľa obnovení displeja.
    caka_na_obraz: bool,
    caka_od: Option<i64>,
    /// Prvá snímka po začiatku alebo posune sa ukáže hneď (ešte pred hodinami) – používateľ vidí cieľ posunu.
    prva_snimka: bool,
    stav: StavPrehravania,
}

/// Kód jazyka na porovnanie: BCP 47 bez oblasti („sk-SK“ → „sk“), ISO 639-1 a 639-2/B → 639-2/T
/// (Matroska a MP4 používajú ISO 639-2 v oboch tvaroch, BCP 47 dvojpísmenové kódy).
fn jazyk_kod(j: &str) -> String {
    let j = j.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    const TABULKA: [(&str, &str, &str); 24] = [
        ("sk", "slo", "slk"),
        ("cs", "cze", "ces"),
        ("en", "eng", "eng"),
        ("de", "ger", "deu"),
        ("fr", "fre", "fra"),
        ("hu", "hun", "hun"),
        ("pl", "pol", "pol"),
        ("uk", "ukr", "ukr"),
        ("ru", "rus", "rus"),
        ("es", "spa", "spa"),
        ("it", "ita", "ita"),
        ("pt", "por", "por"),
        ("nl", "dut", "nld"),
        ("hr", "hrv", "hrv"),
        ("sl", "slv", "slv"),
        ("sr", "srp", "srp"),
        ("ro", "rum", "ron"),
        ("bg", "bul", "bul"),
        ("el", "gre", "ell"),
        ("tr", "tur", "tur"),
        ("sv", "swe", "swe"),
        ("fi", "fin", "fin"),
        ("ja", "jpn", "jpn"),
        ("zh", "chi", "zho"),
    ];
    TABULKA
        .iter()
        .find(|(a, b, t)| j == *a || j == *b || j == *t)
        .map_or(j.clone(), |(_, _, t)| (*t).to_string())
}

fn index_stopy(stopy: &[Stopa], druh: DruhStopy, zvolena: Option<usize>, jazyk: Option<&str>) -> Vec<usize> {
    if let Some(i) = zvolena {
        return if stopy.get(i).is_some_and(|s| s.druh == druh) { vec![i] } else { vec![] };
    }
    let mut kandidati: Vec<usize> = (0..stopy.len()).filter(|&i| stopy[i].druh == druh).collect();
    // poradie: jazyk, predvolená, poradie v súbore
    let jazyk = jazyk.map(jazyk_kod);
    kandidati.sort_by_key(|&i| {
        let s = &stopy[i];
        let zhoda = jazyk.as_ref().is_some_and(|j| s.jazyk.as_deref().is_some_and(|l| jazyk_kod(l) == *j));
        (!zhoda, !s.predvolena, i)
    });
    kandidati
}

impl Prehravanie {
    /// Otvorí prehrávanie kontajnera: vyberie video (ak ide do hardvérového dekodéra) a zvuk (dekodér alebo priechod
    /// podľa schopností HDMI). Chyba len vtedy, keď sa nedá prehrať nič.
    pub fn otvor(
        k: Box<dyn Kontajner>,
        schopnosti: &SchopnostiHdmi,
        volby: &VolbyPrehravania,
    ) -> Vysledok<Prehravanie> {
        let stopy = k.stopy().to_vec();
        let mut info = InfoPrehravania {
            format: k.format(),
            trvanie_ns: k.trvanie_ns(),
            video: None,
            zvuk: None,
            chyba_zvuku: None,
            chyba_videa: None,
        };
        let mut video = None;
        for i in index_stopy(&stopy, DruhStopy::Video, volby.stopa_videa, None) {
            match PrevodnikVidea::novy(&stopy[i]) {
                Ok(p) => {
                    info.video = Some(InfoVidea { stopa: i, tok: p.tok(), parametre: stopy[i].video });
                    video = Some(Video {
                        index: i,
                        prevodnik: p,
                        fronta: VecDeque::new(),
                        bajtov: 0,
                        odoslane: VecDeque::new(),
                        cakajuce: Vec::new(),
                        zobrazena: None,
                    });
                    break;
                }
                Err(e) => {
                    info.chyba_videa.get_or_insert_with(|| e.to_string());
                }
            }
        }
        let mut zvuk = None;
        for i in index_stopy(&stopy, DruhStopy::Zvuk, volby.stopa_zvuku, volby.jazyk_zvuku.as_deref()) {
            let s = &stopy[i];
            let pokus = PrevodnikZvuku::novy(s, schopnosti, volby.zvuk).and_then(|prevodnik| {
                let dekoder = match prevodnik.cesta() {
                    CestaZvuku::Priechod(_) => None,
                    _ => Some(dekoder_zvuku(s)?),
                };
                Ok((prevodnik, dekoder))
            });
            match pokus {
                Ok((prevodnik, dekoder)) => {
                    let p = dekoder.as_ref().map(|d| *d.parametre()).or(s.zvuk);
                    let (f, r) = p.map_or((48_000, crate::zvuk::RozlozenieKanalov::STEREO), |p| {
                        (p.frekvencia, p.rozlozenie)
                    });
                    info.zvuk = Some(InfoZvuku {
                        stopa: i,
                        kodek: s.kodek.clone(),
                        cesta: prevodnik.cesta(),
                        nastavenie: prevodnik.nastavenie().clone(),
                    });
                    info.chyba_zvuku = None;
                    zvuk = Some(Zvuk {
                        index: i,
                        stopa: s.clone(),
                        dekoder,
                        prevodnik,
                        fronta: VecDeque::new(),
                        bajtov: 0,
                        blok: BlokZvuku::f32(f, r),
                    });
                    break;
                }
                Err(e) => {
                    info.chyba_zvuku.get_or_insert_with(|| format!("stopa {}: {e}", s.kodek.meno()));
                }
            }
        }
        let caka_na_obraz = video.is_some();
        let (ma_video, ma_zvuk) = (video.is_some(), zvuk.is_some());
        if video.is_none() && zvuk.is_none() {
            let dovod =
                info.chyba_zvuku.clone().or(info.chyba_videa.clone()).unwrap_or_else(|| "bez stôp".into());
            return Err(Chyba::MimoRozsahu(format!("súbor sa nedá prehrať: {dovod}")));
        }
        Ok(Prehravanie {
            k,
            info,
            video,
            zvuk,
            max_fronta: volby.max_fronta_bajtov.max(1 << 20),
            koniec_suboru: false,
            hodiny: None,
            sync: Synchronizacia::nova(volby.oneskorenie_obrazu_ns),
            pauza: false,
            ciel_ns: None,
            caka_na_obraz,
            caka_od: None,
            prva_snimka: true,
            // stopa, ktorá v súbore nie je (alebo nejde), je hneď na konci
            stav: StavPrehravania {
                koniec_videa: !ma_video,
                koniec_zvuku: !ma_zvuk,
                ..StavPrehravania::default()
            },
        })
    }

    pub fn info(&self) -> &InfoPrehravania {
        &self.info
    }

    /// Číta pakety kontajnera, kým fronta žiadanej stopy nie je neprázdna. Pakety druhej stopy idú do jej fronty;
    /// ak by tá prerástla limit (zlé prekladanie v súbore), čítanie sa zastaví – druhá stopa sa najprv odoberie.
    fn napln(&mut self, chce_video: bool) -> Vysledok<()> {
        loop {
            let (moja, ina) = if chce_video {
                (self.video.as_ref().map(|v| v.fronta.len()), self.zvuk.as_ref().map(|z| z.bajtov))
            } else {
                (self.zvuk.as_ref().map(|z| z.fronta.len()), self.video.as_ref().map(|v| v.bajtov))
            };
            if moja.is_none_or(|n| n > 0) || self.koniec_suboru || ina.is_some_and(|b| b > self.max_fronta) {
                return Ok(());
            }
            let Some(p) = self.k.dalsi_paket()? else {
                self.koniec_suboru = true;
                return Ok(());
            };
            if let Some(v) = self.video.as_mut().filter(|v| v.index == p.stopa) {
                v.bajtov += p.data.len();
                v.fronta.push_back(p);
            } else if let Some(z) = self.zvuk.as_mut().filter(|z| z.index == p.stopa) {
                z.bajtov += p.data.len();
                z.fronta.push_back(p);
            }
        }
    }

    /// Ďalší paket pre hardvérový dekodér (R5a); `None` = koniec videa alebo súbor nemá video (alebo čítanie čaká,
    /// kým sa odoberie zvuk – potom treba volať znova).
    pub fn dalsi_paket_videa(&mut self) -> Vysledok<Option<PaketVidea>> {
        self.napln(true)?;
        let Some(v) = self.video.as_mut() else { return Ok(None) };
        let Some(p) = v.fronta.pop_front() else {
            self.stav.koniec_videa = self.koniec_suboru;
            if self.koniec_suboru {
                self.caka_na_obraz = false;
            }
            return Ok(None);
        };
        v.bajtov -= p.data.len();
        match v.prevodnik.prevod(&p) {
            Ok(pv) => {
                if v.odoslane.len() >= MAX_ODOSLANYCH {
                    v.odoslane.pop_front();
                }
                v.odoslane.push_back((pv.id, pv.pts_ns));
                Ok(Some(pv))
            }
            Err(e) => {
                self.stav.chybnych_paketov_videa += 1;
                Err(e)
            }
        }
    }

    /// Dekodér vrátil snímku s časovou značkou `id` (z `PaketVidea::id`).
    pub fn snimka(&mut self, id: u64) -> PrijatieSnimky {
        let Some(v) = self.video.as_mut() else { return PrijatieSnimky::Zahod };
        let Some(pts) = v.odoslane.iter().find(|(i, _)| *i == id).map(|(_, t)| t.unwrap_or(i64::MIN)) else {
            self.stav.neznamych_snimok += 1;
            return PrijatieSnimky::Zahod;
        };
        // pred cieľom presného posunu, plná fronta, alebo druhá snímka s tým istým `id` (dve polia v jednom pakete)
        if self.ciel_ns.is_some_and(|c| pts < c)
            || v.cakajuce.len() >= MAX_CAKAJUCICH
            || v.zobrazena == Some(id)
            || v.cakajuce.iter().any(|&(_, i)| i == id)
        {
            return PrijatieSnimky::Zahod;
        }
        let i = v.cakajuce.partition_point(|&(t, _)| t <= pts);
        v.cakajuce.insert(i, (pts, id));
        self.caka_na_obraz = false;
        PrijatieSnimky::Caka
    }

    /// Obnovenie displeja: ktorú snímku zobraziť (L6 – `Synchronizacia`).
    pub fn obnova(&mut self, o: Obnova) -> VyberSnimky {
        let Some(v) = self.video.as_mut() else { return VyberSnimky::default() };
        if self.caka_na_obraz {
            let od = *self.caka_od.get_or_insert(o.monotonny_ns);
            if o.monotonny_ns.saturating_sub(od) > NAJDLHSIE_CAKANIE_NA_OBRAZ_NS {
                self.caka_na_obraz = false;
            }
        }
        if self.hodiny.is_none() && self.zvuk.is_none() {
            // bez zvuku: monotónne hodiny od prvej snímky
            if let Some(&(pts, _)) = v.cakajuce.first() {
                self.hodiny = Some(HodinyPrehravania::monotonne(pts, o.monotonny_ns));
            }
        }
        let Some(h) = self.hodiny.as_ref() else {
            // hodiny ešte nebežia (zvuk sa spúšťa): prvá snímka sa ukáže hneď, ďalšie čakajú na hodiny
            if self.prva_snimka
                && let Some(&(_, id)) = v.cakajuce.first()
            {
                v.cakajuce.remove(0);
                self.prva_snimka = false;
                let nahradena = v.zobrazena.replace(id);
                return VyberSnimky { zobraz: Some(id), vynechane: Vec::new(), nahradena };
            }
            return VyberSnimky::default();
        };
        if self.pauza {
            return VyberSnimky::default();
        }
        let fronta: Vec<i64> = v.cakajuce.iter().map(|&(t, _)| t).collect();
        let r = self.sync.obnova(h, o, &fronta);
        let Some(i) = r.zobraz else { return VyberSnimky::default() };
        self.prva_snimka = false;
        let vybrane: Vec<u64> = v.cakajuce.drain(..=i).map(|(_, id)| id).collect();
        let (vynechane, zobraz) = vybrane.split_at(r.vynechat.min(vybrane.len() - 1));
        let nahradena = v.zobrazena.replace(zobraz[0]);
        VyberSnimky { zobraz: Some(zobraz[0]), vynechane: vynechane.to_vec(), nahradena }
    }

    /// Ďalší blok zvuku pre zariadenie HDMI (R5b); `None` = koniec zvuku (alebo čítanie čaká na odobratie videa).
    /// Pri priechode E-AC-3 sa pakety zbierajú do dávok po 6 blokoch – vráti sa až celá dávka.
    pub fn dalsi_zvuk(&mut self) -> Vysledok<Option<ZvukHdmi>> {
        if self.caka_na_obraz && self.video.is_some() {
            return Ok(None);
        }
        loop {
            self.napln(false)?;
            let Some(z) = self.zvuk.as_mut() else { return Ok(None) };
            let Some(p) = z.fronta.pop_front() else {
                self.stav.koniec_zvuku = self.koniec_suboru;
                return Ok(None);
            };
            z.bajtov -= p.data.len();
            let von = match z.dekoder.as_mut() {
                None => match z.prevodnik.priechod(&p) {
                    Ok(v) => v,
                    Err(_) => {
                        self.stav.chybnych_paketov_zvuku += 1;
                        continue;
                    }
                },
                Some(d) => {
                    if d.dekoduj(&p, &mut z.blok).is_err() {
                        self.stav.chybnych_paketov_zvuku += 1;
                        continue;
                    }
                    // čas prvej vzorky po orezaní začiatku (dekodér orezanie už uplatnil)
                    let f = z.blok.frekvencia.max(1) as i64;
                    z.blok.pts_ns = p
                        .pts
                        .and_then(|t| na_ns(t, z.stopa.zakladna))
                        .map(|t| t + p.orezat_zaciatok as i64 * 1_000_000_000 / f);
                    if let (Some(c), Some(t)) = (self.ciel_ns, z.blok.pts_ns)
                        && t < c
                    {
                        let zahod = ((c - t) as u128 * f as u128).div_ceil(1_000_000_000) as usize;
                        let n = z.blok.ramcov;
                        orez(&mut z.blok, zahod.min(n), 0);
                    }
                    if z.blok.ramcov == 0 {
                        continue;
                    }
                    z.prevodnik.pcm(&z.blok)?
                }
            };
            if von.ramcov == 0 {
                continue;
            }
            // priechod: dávky pred cieľom presného posunu sa nezahrajú
            if z.dekoder.is_none() && self.ciel_ns.zip(von.pts_ns).is_some_and(|(c, t)| t < c) {
                continue;
            }
            if self.hodiny.is_none() {
                self.hodiny = Some(HodinyPrehravania::zo_zvuku(
                    z.prevodnik.nastavenie().frekvencia,
                    von.pts_ns.unwrap_or(0),
                ));
            }
            return Ok(Some(von));
        }
    }

    /// Meranie zvukového zariadenia (R5c) – rámce od začiatku prehrávania alebo od posledného posunu.
    pub fn meranie_zvuku(&mut self, m: MeranieZvuku) {
        if let Some(h) = self.hodiny.as_mut()
            && self.zvuk.is_some()
        {
            h.meranie(&m);
        }
    }

    pub fn pauza(&mut self, t: i64) {
        self.pauza = true;
        if let Some(h) = self.hodiny.as_mut() {
            h.pauza(t);
        }
    }

    pub fn pokracuj(&mut self, t: i64) {
        self.pauza = false;
        if let Some(h) = self.hodiny.as_mut() {
            h.pokracuj(t);
        }
    }

    /// Posun na `ciel_ns` (časová os média). Systém potom vyprázdni dekodér videa (prvý ďalší paket má
    /// `prerusenie`) a zariadenie zvuku (zahodí nezahraté rámce; ďalšie merania rátajú rámce od nuly).
    pub fn posun(&mut self, ciel_ns: i64, sposob: Posun) -> Vysledok<Posunute> {
        let cas = self.k.posun(ciel_ns, sposob)?;
        self.koniec_suboru = false;
        self.stav.koniec_videa = self.video.is_none();
        self.stav.koniec_zvuku = self.zvuk.is_none();
        self.ciel_ns = (sposob == Posun::Presny).then_some(ciel_ns);
        self.sync.prerusenie();
        self.caka_na_obraz = self.video.is_some();
        self.caka_od = None;
        self.prva_snimka = true;
        // nové hodiny: zo zvuku z prvého bloku po posune, bez zvuku od prvej snímky
        self.hodiny = None;
        let mut uvolni = Vec::new();
        if let Some(v) = self.video.as_mut() {
            v.fronta.clear();
            v.bajtov = 0;
            v.odoslane.clear();
            v.prevodnik.prerusenie();
            uvolni.extend(v.cakajuce.drain(..).map(|(_, id)| id));
        }
        if let Some(z) = self.zvuk.as_mut() {
            z.fronta.clear();
            z.bajtov = 0;
            z.prevodnik.prerusenie();
            if let Some(d) = z.dekoder.as_mut() {
                d.reset();
            }
        }
        Ok(Posunute { cas_ns: cas, uvolni })
    }

    /// Stav v okamihu `t` (monotónny čas systému, ns).
    pub fn stav(&self, t: i64) -> StavPrehravania {
        let mut s = self.stav;
        s.cas_ns = self.hodiny.as_ref().and_then(|h| h.cas_ns(t));
        s.pauza = self.pauza;
        s.synchronizacia = *self.sync.statistika();
        s.skokov_hodin = self.hodiny.as_ref().map_or(0, |h| h.skokov());
        s
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::cas::zakladna_z_frekvencie;
    use crate::r5::FormatVzoriekHdmi;
    use crate::stopa::{CisloPcm, ParametreZvuku, PopisPcm, PoradieBajtov};
    use crate::zvuk::RozlozenieKanalov;
    use orchidea_jadro::Zlomok;

    /// Syntetický kontajner: H.264 24000/1001 snímok/s (čas v ms) a PCM 48 kHz stereo po 1024 rámcoch, prekladané
    /// v poradí času ako v skutočnom súbore. Obsah snímky nesie jej poradové číslo.
    #[derive(Debug)]
    struct Umely {
        stopy: Vec<Stopa>,
        pakety: Vec<Paket>,
        i: usize,
    }

    const SNIMOK: i64 = 240;
    const RAMCOV_ZVUKU: i64 = 1024;

    fn ns_snimky(n: i64) -> i64 {
        n * 1_001_000_000_000 / 24_000
    }

    impl Umely {
        fn novy(so_zvukom: bool, s_videom: bool) -> Umely {
            let mut v = Stopa::nova(1, Kodek::H264, Zlomok::novy(1, 1_000_000_000).unwrap());
            v.predvolena = true;
            let mut z = Stopa::nova(
                2,
                Kodek::Pcm(PopisPcm {
                    bitov: 16,
                    bajtov: 2,
                    poradie: PoradieBajtov::MaloKoncove,
                    cislo: CisloPcm::SoZnamienkom,
                }),
                zakladna_z_frekvencie(48_000).unwrap(),
            );
            z.zvuk = Some(ParametreZvuku {
                frekvencia: 48_000,
                kanalov: 2,
                rozlozenie: RozlozenieKanalov::STEREO,
                bitov: Some(16),
            });
            let mut stopy = Vec::new();
            let (iv, iz) = match (s_videom, so_zvukom) {
                (true, true) => (0, 1),
                (true, false) => (0, usize::MAX),
                _ => (usize::MAX, 0),
            };
            if s_videom {
                stopy.push(v);
            }
            if so_zvukom {
                stopy.push(z);
            }
            let mut pakety = Vec::new();
            let koniec = ns_snimky(SNIMOK);
            let (mut n, mut a) = (0i64, 0i64);
            loop {
                let tv = if s_videom && n < SNIMOK { ns_snimky(n) } else { i64::MAX };
                let ta = if so_zvukom && a * RAMCOV_ZVUKU * 1_000_000_000 / 48_000 < koniec {
                    a * RAMCOV_ZVUKU * 1_000_000_000 / 48_000
                } else {
                    i64::MAX
                };
                if tv == i64::MAX && ta == i64::MAX {
                    break;
                }
                if tv <= ta {
                    let mut d = vec![0, 0, 0, 1, 0x65];
                    d.extend_from_slice(&(n as u32).to_be_bytes());
                    pakety.push(paket(iv, d, tv, n % 24 == 0));
                    n += 1;
                } else {
                    // zvuk: rampa – hodnota vzorky = číslo rámca mod 30 000 (kontrola súvislosti)
                    let mut d = Vec::new();
                    for r in 0..RAMCOV_ZVUKU {
                        let x = (((a * RAMCOV_ZVUKU + r) % 30_000) as i16).to_le_bytes();
                        d.extend_from_slice(&x);
                        d.extend_from_slice(&x);
                    }
                    pakety.push(paket(iz, d, a * RAMCOV_ZVUKU, true));
                    a += 1;
                }
            }
            Umely { stopy, pakety, i: 0 }
        }
    }

    fn paket(stopa: usize, data: Vec<u8>, pts: i64, klucovy: bool) -> Paket {
        Paket {
            stopa,
            data,
            pts: Some(pts),
            dts: None,
            trvanie: None,
            klucovy,
            prerusenie: false,
            orezat_zaciatok: 0,
            orezat_koniec: 0,
            pozicia: 0,
        }
    }

    impl Kontajner for Umely {
        fn format(&self) -> FormatKontajnera {
            FormatKontajnera::Matroska
        }
        fn stopy(&self) -> &[Stopa] {
            &self.stopy
        }
        fn trvanie_ns(&self) -> Option<i64> {
            Some(ns_snimky(SNIMOK))
        }
        fn dalsi_paket(&mut self) -> Vysledok<Option<Paket>> {
            self.i += 1;
            Ok(self.pakety.get(self.i - 1).cloned())
        }
        fn posun(&mut self, ciel_ns: i64, _sposob: Posun) -> Vysledok<i64> {
            // kľúčový paket videa ≤ cieľ (každá 24. snímka); zvuk od toho istého času
            let kluc =
                self.pakety.iter().filter(|p| p.klucovy && self.stopy[p.stopa].druh == DruhStopy::Video);
            let t = kluc.map(|p| p.pts.unwrap()).filter(|&t| t <= ciel_ns).max().unwrap_or(0);
            self.i = self
                .pakety
                .iter()
                .position(|p| {
                    let ns = if self.stopy[p.stopa].druh == DruhStopy::Video {
                        p.pts.unwrap()
                    } else {
                        p.pts.unwrap() * 1_000_000_000 / 48_000
                    };
                    ns >= t
                })
                .unwrap_or(self.pakety.len());
            let mut p = self.pakety.clone();
            if let Some(x) = p.get_mut(self.i) {
                x.prerusenie = true;
            }
            self.pakety = p;
            Ok(t)
        }
    }

    fn hdmi() -> SchopnostiHdmi {
        SchopnostiHdmi {
            formaty: vec![FormatVzoriekHdmi::S16Le],
            max_kanalov: 2,
            frekvencie: vec![44_100, 48_000],
            max_bitov: 16,
            ..SchopnostiHdmi::default()
        }
    }

    /// Simulácia systému: dekodér videa (4 buffery OUTPUT, 8 CAPTURE, 1 snímka na paket, oneskorenie 2 obnovenia),
    /// zvukové zariadenie (0,5 s, kryštál +120 ppm, dopĺňanie pri polovici, meranie 4×/s), displej 60 Hz
    /// (−60 ppm). Vráti (zobrazené snímky v poradí, rámcov zvuku, budení zvuku, prehrávanie).
    struct Simulacia {
        p: Prehravanie,
        t: i64,
        zobrazene: Vec<u64>,
        obsah: std::collections::HashMap<u64, u32>,
        v_dekoderi: VecDeque<(u64, i64)>,
        odovzdanych: u64,
        zahranych: f64,
        budeni: u64,
        prvy_ramec_zvuku: Vec<i16>,
        koniec_zvuku: bool,
        drzane: usize,
        najviac_drzanych: usize,
    }

    impl Simulacia {
        fn nova(p: Prehravanie) -> Simulacia {
            Simulacia {
                p,
                t: 1_000_000_000,
                zobrazene: vec![],
                obsah: Default::default(),
                v_dekoderi: VecDeque::new(),
                odovzdanych: 0,
                zahranych: 0.0,
                budeni: 0,
                prvy_ramec_zvuku: vec![],
                koniec_zvuku: false,
                drzane: 0,
                najviac_drzanych: 0,
            }
        }

        /// Beh `sekund` simulovaného času po krokoch obnovenia displeja.
        fn bez(&mut self, sekund: f64, pauza_od: Option<f64>) {
            let perioda = (1e9 / 60.0 * (1.0 + 60e-6)) as i64;
            let start = self.t;
            let mut pauza = false;
            while ((self.t - start) as f64) < sekund * 1e9 {
                if let Some(od) = pauza_od
                    && !pauza
                    && (self.t - start) as f64 >= od * 1e9
                {
                    pauza = true;
                    self.p.pauza(self.t);
                }
                // dekodér: snímky hotové po 2 obnoveniach
                while self.v_dekoderi.front().is_some_and(|&(_, hotovo)| hotovo <= self.t) {
                    let (id, _) = self.v_dekoderi.pop_front().unwrap();
                    match self.p.snimka(id) {
                        PrijatieSnimky::Caka => {
                            self.drzane += 1;
                            self.najviac_drzanych = self.najviac_drzanych.max(self.drzane);
                        }
                        PrijatieSnimky::Zahod => {}
                    }
                }
                // 8 bufferov CAPTURE: rozpracované + držané snímky
                while self.v_dekoderi.len() < 4 && self.v_dekoderi.len() + self.drzane < 8 {
                    match self.p.dalsi_paket_videa().unwrap() {
                        Some(pv) => {
                            let n = u32::from_be_bytes(pv.data[pv.data.len() - 4..].try_into().unwrap());
                            self.obsah.insert(pv.id, n);
                            self.v_dekoderi.push_back((pv.id, self.t + 2 * perioda));
                        }
                        None => break,
                    }
                }
                // zvuk: zariadenie hrá, ak nie je pauza
                if !pauza {
                    self.zahranych = (self.zahranych + perioda as f64 * 48_000.0 * (1.0 + 120e-6) / 1e9)
                        .min(self.odovzdanych as f64);
                }
                let vo_fronte = self.odovzdanych as f64 - self.zahranych;
                // zvuk: budenie pri polovici (0,25 s), doplnenie na 0,5 s, po zápise meranie (R5c)
                if !pauza && !self.koniec_zvuku && vo_fronte < 12_000.0 {
                    let pred = self.odovzdanych;
                    while (self.odovzdanych as f64 - self.zahranych) < 24_000.0 {
                        match self.p.dalsi_zvuk().unwrap() {
                            Some(z) => {
                                if self.prvy_ramec_zvuku.is_empty() {
                                    self.prvy_ramec_zvuku = z.data[..4]
                                        .chunks(2)
                                        .map(|c| i16::from_le_bytes([c[0], c[1]]))
                                        .collect();
                                }
                                self.odovzdanych += z.ramcov as u64;
                            }
                            None => {
                                self.koniec_zvuku = self.p.stav(self.t).koniec_zvuku;
                                break;
                            }
                        }
                    }
                    if self.odovzdanych > pred {
                        self.budeni += 1;
                        // chvenie hlásenia ±2 ms (pevná postupnosť)
                        let chvenie = ((self.t / perioda) % 5 - 2) * 1_000_000;
                        self.p.meranie_zvuku(MeranieZvuku {
                            odovzdanych_ramcov: self.odovzdanych,
                            oneskorenie_ramcov: (self.odovzdanych as f64 - self.zahranych) as u64,
                            monotonny_ns: self.t + chvenie,
                        });
                    }
                }
                let v = self.p.obnova(Obnova { monotonny_ns: self.t, perioda_ns: perioda });
                self.drzane -= v.vynechane.len() + v.nahradena.is_some() as usize;
                if let Some(id) = v.zobraz {
                    self.zobrazene.push(id);
                }
                self.t += perioda;
            }
        }

        fn poradove(&self) -> Vec<u32> {
            self.zobrazene.iter().map(|id| self.obsah[id]).collect()
        }
    }

    #[test]
    fn film_so_zvukom_synchronne() {
        let p = Prehravanie::otvor(Box::new(Umely::novy(true, true)), &hdmi(), &VolbyPrehravania::default())
            .unwrap();
        assert_eq!(p.info().video.as_ref().unwrap().tok, TokVidea::H264);
        assert_eq!(p.info().zvuk.as_ref().unwrap().cesta, CestaZvuku::BitovoPresne);
        let mut s = Simulacia::nova(p);
        s.bez(11.0, None);
        let poradie = s.poradove();
        // každá snímka práve raz a v poradí (24 fps na 60 Hz – kadencia 3:2 bez vynechania)
        assert_eq!(poradie, (0..SNIMOK as u32).collect::<Vec<_>>(), "zobrazené {}", poradie.len());
        let st = s.p.stav(s.t);
        assert!(st.koniec_videa && st.koniec_zvuku);
        assert_eq!(st.synchronizacia.vynechanych, 0);
        // odchýlka nástupu: hodiny zo zvuku, obraz v cieli (≤ P/2 + P/8 + 2 ms ≈ 12,4 ms pri 60 Hz)
        let sy = st.synchronizacia;
        assert!(sy.najvacsia_odchylka_ns < 12_500_000, "{sy:?}");
        assert!(sy.priemerna_odchylka_ns().abs() < 4_000_000, "{sy:?}");
        // zvuk celý, bitovo (rampa od 0), budenia ≈ 4/s (dopĺňanie pri polovici 0,5 s)
        assert_eq!(
            s.odovzdanych,
            (ns_snimky(SNIMOK) as u64 * 48_000).div_ceil(1_000_000_000 * RAMCOV_ZVUKU as u64) * 1024
        );
        assert_eq!(s.prvy_ramec_zvuku, vec![0, 0]);
        assert!((34..=44).contains(&s.budeni), "budení {}", s.budeni);
        assert!(s.najviac_drzanych <= 8);
    }

    #[test]
    fn presny_posun_a_pauza() {
        let p = Prehravanie::otvor(Box::new(Umely::novy(true, true)), &hdmi(), &VolbyPrehravania::default())
            .unwrap();
        let mut s = Simulacia::nova(p);
        s.bez(2.0, None);
        // posun na 6,0 s: kľúčová snímka 144 (6,006 s) je za cieľom → číta sa od 120 (5,005 s)
        let r = s.p.posun(6_000_000_000, Posun::Presny).unwrap();
        assert_eq!(r.cas_ns, ns_snimky(120));
        s.drzane -= r.uvolni.len();
        s.v_dekoderi.clear();
        s.odovzdanych = 0;
        s.zahranych = 0.0;
        s.prvy_ramec_zvuku.clear();
        s.koniec_zvuku = false;
        let pred = s.zobrazene.len();
        s.bez(2.0, Some(1.0));
        let po: Vec<u32> = s.zobrazene[pred..].iter().map(|id| s.obsah[id]).collect();
        // prvá zobrazená snímka po posune je prvá s pts ≥ 6 s (144 = 6,006 s), ďalej v poradí bez medzier
        assert_eq!(po[0], 144, "{po:?}");
        assert!(po.windows(2).all(|w| w[1] == w[0] + 1), "{po:?}");
        // zvuk začína presne v cieli: rámec 288 000 (6 s × 48 kHz) mod 30 000 = 18 000
        assert_eq!(s.prvy_ramec_zvuku, vec![18_000, 18_000]);
        // pauza od 1 s: čas stojí, ďalšie snímky sa nezobrazia (≈ 1 s × 24 snímok, menej o čas, kým dekodér
        // prejde 24 snímok pred cieľom)
        assert!((15..=27).contains(&po.len()), "po posune {} snímok", po.len());
        let t1 = s.p.stav(s.t).cas_ns.unwrap();
        s.bez(0.5, Some(0.0));
        assert_eq!(s.p.stav(s.t).cas_ns.unwrap(), t1);
        assert!(s.p.stav(s.t).pauza);
    }

    #[test]
    fn rovnake_id_dvakrat() {
        let mut p =
            Prehravanie::otvor(Box::new(Umely::novy(false, true)), &hdmi(), &VolbyPrehravania::default())
                .unwrap();
        let id = p.dalsi_paket_videa().unwrap().unwrap().id;
        assert_eq!(p.snimka(id), PrijatieSnimky::Caka);
        assert_eq!(p.snimka(id), PrijatieSnimky::Zahod);
        assert_eq!(p.snimka(999), PrijatieSnimky::Zahod);
        assert_eq!(p.stav(0).neznamych_snimok, 1);
    }

    #[test]
    fn len_video_a_len_zvuk() {
        // bez zvuku: monotónne hodiny od prvej snímky
        let p = Prehravanie::otvor(Box::new(Umely::novy(false, true)), &hdmi(), &VolbyPrehravania::default())
            .unwrap();
        assert!(p.info().zvuk.is_none());
        assert!(p.stav(0).koniec_zvuku);
        let mut s = Simulacia::nova(p);
        s.bez(11.0, None);
        assert_eq!(s.poradove(), (0..SNIMOK as u32).collect::<Vec<_>>());
        // bez videa (hudba): zvuk a hodiny
        let p = Prehravanie::otvor(Box::new(Umely::novy(true, false)), &hdmi(), &VolbyPrehravania::default())
            .unwrap();
        assert!(p.info().video.is_none());
        assert!(p.stav(0).koniec_videa && !p.stav(0).koniec_zvuku);
        let mut s = Simulacia::nova(p);
        s.bez(3.0, None);
        assert!(s.p.stav(s.t).cas_ns.unwrap() > 2_400_000_000);
    }

    #[test]
    fn zvuk_ktory_nejde_a_vyber_jazyka() {
        // televízor bez 48 kHz: zvuk sa neprehrá, video áno, dôvod pre LÝRU
        let mut h = hdmi();
        h.frekvencie = vec![44_100];
        let p =
            Prehravanie::otvor(Box::new(Umely::novy(true, true)), &h, &VolbyPrehravania::default()).unwrap();
        assert!(p.info().zvuk.is_none());
        assert!(p.info().chyba_zvuku.is_some());
        // bez videa a bez zvuku: chyba
        assert!(
            Prehravanie::otvor(Box::new(Umely::novy(true, false)), &h, &VolbyPrehravania::default()).is_err()
        );
        // jazyk: zo dvoch stôp sa vyberie „slk“ aj pri požiadavke „sk“
        let mut u = Umely::novy(true, true);
        let mut druha = u.stopy[1].clone();
        druha.jazyk = Some("slk".into());
        u.stopy[1].jazyk = Some("eng".into());
        u.stopy[1].predvolena = true;
        u.stopy.push(druha);
        let volby = VolbyPrehravania { jazyk_zvuku: Some("sk".into()), ..VolbyPrehravania::default() };
        let p = Prehravanie::otvor(Box::new(u), &hdmi(), &volby).unwrap();
        assert_eq!(p.info().zvuk.as_ref().unwrap().stopa, 2);
        assert_eq!(jazyk_kod("slo"), "slk");
        assert_eq!(jazyk_kod("sk-SK"), "slk");
        assert_eq!(jazyk_kod("CZE"), "ces");
        assert_eq!(jazyk_kod("tlh"), "tlh");
    }
}
