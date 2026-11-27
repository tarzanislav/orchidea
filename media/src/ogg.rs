//! Čítačka kontajnera **Ogg** (RFC 3533; `framing.html` Xiph.Org) – súbory `.opus`, `.ogg`, `.oga`.
//!
//! - **Stránka:** „OggS“, verzia 0, príznaky (pokračovanie, začiatok – BOS, koniec – EOS), pozícia granule (i64),
//!   sériové číslo, poradové číslo, CRC-32 (mnohočlen 0x04C11DB7, počiatočná hodnota a záverečný XOR 0, bez
//!   zrkadlenia – `framing.html`), tabuľka segmentov; paket = segmenty po 255 B a posledný kratší (lacing).
//!   Stránka so zlým CRC sa preskočí a hľadá sa ďalšie „OggS“.
//! - **Opus v Ogg (RFC 7845):** hlavičky OpusHead (konfigurácia stopy, pre-skip) a OpusTags (značky; R128).
//!   Granule = vzorky pri 48 kHz po poslednej úplnej vzorke posledného paketu ukončeného na stránke. Počiatočná
//!   pozícia sa určí **spätne od prvej zvukovej stránky** (granule − súčet trvaní paketov podľa TOC; ak je granule
//!   menšia, 0 – §4). PTS paketu = pozícia začiatku − pre-skip (ako Matroska `CodecDelay`; pre-skip zahodí dekodér).
//!   Stránka EOS s menšou granule **oreže koniec** (`orezat_koniec`). Paket s chýbajúcou časťou (pretrhnutá
//!   postupnosť stránok) sa zahodí (§3).
//! - **FLAC v Ogg** (mapovanie FLAC → Ogg, RFC 9639): prvý paket 0x7F „FLAC“, verzia, počet hlavičkových paketov,
//!   „fLaC“ a STREAMINFO; ďalej bloky metadát (VORBIS_COMMENT, PICTURE …) a rámce FLAC. Trvanie rámca z jeho
//!   hlavičky, granule = vzorky. Bez textu mapovania (overené rozhodcom).
//! - **Vorbis:** stopa a značky (komentár „\x03vorbis“); `media` Vorbis nedekóduje – pakety bez časov (trvanie
//!   paketu by vyžadovalo rozbor hlavičky setup).
//! - **Posun:** delenie intervalu podľa polohy na stránku s najväčšou granule ≤ cieľ (RFC 7845 §6), predbeh 80 ms
//!   dá dekodér (`predbeh_ns`). Trvanie z granule poslednej stránky.
//! - Zreťazené toky (nový BOS po EOS) sa zatiaľ čítajú len po koniec prvého článku.

use crate::cas::{na_ns, z_ns, zakladna_z_frekvencie};
use crate::kontajner::{FormatKontajnera, Kontajner, Posun};
use crate::limity::Limity;
use crate::paket::Paket;
use crate::stopa::{Kodek, ParametreZvuku, Stopa};
use crate::zdroj::{Citac, ZdrojBajtov};
use crate::znacky::{self, Znacky};
use crate::zvuk::RozlozenieKanalov;
use orchidea_jadro::{Chyba, Vysledok};
use std::collections::VecDeque;
use std::sync::OnceLock;

/// Koľko bajtov najviac hľadať ďalšiu stránku (synchronizácia po poškodení).
const NAJVIAC_HLADANIA: u64 = 1 << 20;
/// Najväčšia stránka: 27 + 255 + 255 × 255 B.
const NAJVIAC_STRANKA: usize = 27 + 255 + 255 * 255;
/// Najviac stránok s hlavičkami pri otvorení.
const NAJVIAC_STRANOK_HLAVICIEK: usize = 10_000;

fn chyba(s: impl Into<String>) -> Chyba {
    Chyba::PoskodeneData(format!("Ogg: {}", s.into()))
}

/// CRC-32 stránky Ogg: mnohočlen 0x04C11DB7, počiatočná hodnota 0, bez zrkadlenia, bez záverečného XOR.
fn crc_tabulka() -> &'static [u32; 256] {
    static T: OnceLock<[u32; 256]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let mut c = (i as u32) << 24;
            for _ in 0..8 {
                c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
            }
            *v = c;
        }
        t
    })
}

fn crc_ogg(stranka: &[u8]) -> u32 {
    let t = crc_tabulka();
    let mut c = 0u32;
    for (i, &b) in stranka.iter().enumerate() {
        let b = if (22..26).contains(&i) { 0 } else { b }; // pole CRC sa počíta ako nuly
        c = (c << 8) ^ t[((c >> 24) as u8 ^ b) as usize];
    }
    c
}

/// Rozobraná stránka.
#[derive(Debug, Clone)]
struct Stranka {
    pozicia: u64,
    dlzka: usize,
    pokracovanie: bool,
    bos: bool,
    eos: bool,
    granula: i64,
    serial: u32,
    poradie: u32,
    /// dĺžky segmentov
    segmenty: Vec<u8>,
    data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Druh {
    Opus,
    Flac,
    Vorbis,
}

#[derive(Debug)]
struct StavStopy {
    serial: u32,
    druh: Druh,
    /// zostávajúce hlavičkové pakety vrátane prvého (Opus 2, FLAC 1 + počet z prvého paketu, Vorbis 3)
    hlaviciek: usize,
    /// rozpracovaný paket (pokračuje na ďalšej stránke) a poloha jeho začiatku v súbore
    rozpracovany: Option<(Vec<u8>, u64)>,
    /// poradové číslo poslednej stránky (kontrola straty)
    poradie: Option<u32>,
    /// pozícia (granule) za posledným vydaným paketom; `None` = začiatok ešte neurčený
    pozicia: Option<i64>,
    /// pre-skip (Opus) – odpočíta sa od pozície na PTS
    predskok: i64,
    /// Opus s viacerými prúdmi (samooddeľovacie balenie prvého prúdu)
    viac_prudov: bool,
    streaminfo: Option<crate::flac::StreamInfo>,
    koniec: bool,
    prerusenie: bool,
}

/// Čítačka súboru Ogg.
#[derive(Debug)]
pub struct Ogg<Z> {
    c: Citac<Z>,
    limity: Limity,
    stopy: Vec<Stopa>,
    stav: Vec<StavStopy>,
    znacky: Option<Znacky>,
    trvanie_ns: Option<i64>,
    /// poloha prvej zvukovej stránky (za hlavičkami)
    prva_zvukova: u64,
    p: u64,
    koniec: u64,
    fronta: VecDeque<Paket>,
    /// po posune: pakety ukončené na prvej stránke sa zahodia, pozícia = jej granule
    po_posune: bool,
}

/// Počet vzoriek (48 kHz) paketu Opus podľa TOC (RFC 6716 §3); pri viacerých prúdoch rozhoduje prvý prúd
/// (samooddeľovacie balenie, RFC 6716 príloha B).
fn vzoriek_opus(d: &[u8], viac_prudov: bool) -> Option<i64> {
    let p = crate::opus::balenie::rozbal(d, viac_prudov).ok()?;
    Some(p.ramce.len() as i64 * p.toc.vzoriek_48k as i64)
}

impl<Z: ZdrojBajtov> Ogg<Z> {
    pub fn otvor(zdroj: Z, limity: Limity) -> Vysledok<Ogg<Z>> {
        let c = Citac::novy(zdroj, Citac::<Z>::PREDVOLENA_DAVKA);
        let koniec = c.dlzka();
        let mut o = Ogg {
            c,
            limity,
            stopy: Vec::new(),
            stav: Vec::new(),
            znacky: None,
            trvanie_ns: None,
            prva_zvukova: 0,
            p: 0,
            koniec,
            fronta: VecDeque::new(),
            po_posune: false,
        };
        o.citaj_hlavicky()?;
        o.trvanie_ns = o.zisti_trvanie();
        Ok(o)
    }

    /// Stránka presne na polohe `p` s platným CRC; `None` = nie je tam platná stránka.
    fn stranka_na(&mut self, p: u64) -> Vysledok<Option<Stranka>> {
        let h = self.c.bajty(p, 27)?;
        if h.len() < 27 || &h[..4] != b"OggS" || h[4] != 0 {
            return Ok(None);
        }
        let n = h[26] as usize;
        let segmenty = self.c.bajty(p + 27, n)?.to_vec();
        if segmenty.len() < n {
            return Ok(None);
        }
        let dlzka = 27 + n + segmenty.iter().map(|&s| s as usize).sum::<usize>();
        let cela = self.c.bajty(p, dlzka)?;
        if cela.len() < dlzka || crc_ogg(cela) != u32::from_le_bytes([cela[22], cela[23], cela[24], cela[25]])
        {
            return Ok(None);
        }
        Ok(Some(Stranka {
            pozicia: p,
            dlzka,
            pokracovanie: cela[5] & 1 != 0,
            bos: cela[5] & 2 != 0,
            eos: cela[5] & 4 != 0,
            granula: i64::from_le_bytes(cela[6..14].try_into().unwrap_or([0xFF; 8])),
            serial: u32::from_le_bytes([cela[14], cela[15], cela[16], cela[17]]),
            poradie: u32::from_le_bytes([cela[18], cela[19], cela[20], cela[21]]),
            data: cela[27 + n..].to_vec(),
            segmenty,
        }))
    }

    /// Ďalšia platná stránka od polohy `p` (hľadanie „OggS“ s kontrolou CRC); `None` = koniec súboru.
    fn dalsia_stranka(&mut self, mut p: u64) -> Vysledok<Option<Stranka>> {
        let hranica = p.saturating_add(NAJVIAC_HLADANIA);
        while p + 27 <= self.koniec && p <= hranica {
            if let Some(s) = self.stranka_na(p)? {
                return Ok(Some(s));
            }
            let okno = self.c.bajty(p + 1, 64 * 1024)?.to_vec();
            match okno.windows(4).position(|w| w == b"OggS") {
                Some(i) => p += 1 + i as u64,
                None if okno.len() < 4 => return Ok(None),
                None => p += okno.len() as u64 - 2,
            }
        }
        Ok(None)
    }

    /// Hlavičky: stránky BOS určia stopy, potom hlavičkové pakety každej stopy. Čítanie zvuku začne prvou
    /// stránkou, ktorá nenesie hlavičku (hlavičky Opus a Vorbis končia stránku – RFC 7845 §3).
    fn citaj_hlavicky(&mut self) -> Vysledok<()> {
        let mut p = 0u64;
        for _ in 0..NAJVIAC_STRANOK_HLAVICIEK {
            let Some(s) = self.dalsia_stranka(p)? else { break };
            p = s.pozicia + s.dlzka as u64;
            if s.bos {
                self.nova_stopa(&s)?;
            }
            let Some(ix) = self.stav.iter().position(|t| t.serial == s.serial) else { continue };
            if self.stav[ix].hlaviciek == 0 {
                self.prva_zvukova = s.pozicia;
                break;
            }
            for (paket, _) in self.pakety_stranky(ix, &s) {
                if self.stav[ix].hlaviciek > 0 {
                    self.hlavickovy_paket(ix, &paket);
                }
            }
            self.prva_zvukova = p;
            if self.stav.iter().all(|t| t.hlaviciek == 0) {
                break;
            }
        }
        if self.stopy.is_empty() {
            return Err(chyba("súbor nemá stopu Opus, FLAC ani Vorbis"));
        }
        self.p = self.prva_zvukova;
        self.znovu_od_zaciatku_toku();
        Ok(())
    }

    fn znovu_od_zaciatku_toku(&mut self) {
        for s in &mut self.stav {
            s.rozpracovany = None;
            s.poradie = None;
            s.koniec = false;
            s.prerusenie = true;
        }
        self.fronta.clear();
    }

    fn nova_stopa(&mut self, s: &Stranka) -> Vysledok<()> {
        let prvy = s.data.get(..s.segmenty.first().map_or(0, |&n| n as usize)).unwrap_or_default();
        let (mut predskok, mut viac_prudov, mut streaminfo) = (0i64, false, None);
        let (druh, hlaviciek, mut stopa) = if prvy.starts_with(b"OpusHead") {
            let h = crate::opus::dekoder::HlavickaOpus::z_konfiguracie(prvy, 2)?;
            let mut st = Stopa::nova(
                s.serial as u64,
                Kodek::Opus,
                zakladna_z_frekvencie(48_000).ok_or_else(|| chyba("základňa"))?,
            );
            st.konfiguracia = prvy.to_vec();
            let (rozlozenie, _) = h.rozlozenie();
            st.zvuk =
                Some(ParametreZvuku { frekvencia: 48_000, kanalov: h.kanalov, rozlozenie, bitov: None });
            predskok = h.predskok as i64;
            viac_prudov = h.prudov > 1;
            st.oneskorenie_kodeku_ns = predskok * 1_000_000_000 / 48_000;
            st.predbeh_posunu_ns = 80_000_000;
            (Druh::Opus, 2, st)
        } else if prvy.len() >= 17 + 34 && prvy.starts_with(b"\x7FFLAC") && &prvy[9..13] == b"fLaC" {
            let pocet = u16::from_be_bytes([prvy[7], prvy[8]]) as usize;
            let si = crate::flac::StreamInfo::z_tela(&prvy[17..])?;
            let zakladna =
                zakladna_z_frekvencie(si.frekvencia as u64).ok_or_else(|| chyba("FLAC: frekvencia 0"))?;
            let mut st = Stopa::nova(s.serial as u64, Kodek::Flac, zakladna);
            // konfigurácia ako pri súbore .flac: „fLaC“, hlavička posledného bloku STREAMINFO a STREAMINFO
            st.konfiguracia = b"fLaC\x80\x00\x00\x22".to_vec();
            st.konfiguracia.extend_from_slice(&prvy[17..17 + 34]);
            st.zvuk = Some(ParametreZvuku {
                frekvencia: si.frekvencia,
                kanalov: si.kanalov,
                rozlozenie: crate::flac::rozlozenie(si.kanalov),
                bitov: Some(si.bitov),
            });
            streaminfo = Some(si);
            (Druh::Flac, 1 + pocet, st)
        } else if prvy.len() >= 30 && prvy.starts_with(b"\x01vorbis") {
            let kanalov = prvy[11];
            let frekvencia = u32::from_le_bytes([prvy[12], prvy[13], prvy[14], prvy[15]]);
            let zakladna =
                zakladna_z_frekvencie(frekvencia as u64).ok_or_else(|| chyba("Vorbis: frekvencia 0"))?;
            let mut st = Stopa::nova(s.serial as u64, Kodek::Vorbis, zakladna);
            st.zvuk = Some(ParametreZvuku {
                frekvencia,
                kanalov,
                rozlozenie: if kanalov == 1 { RozlozenieKanalov::MONO } else { RozlozenieKanalov::STEREO },
                bitov: None,
            });
            (Druh::Vorbis, 3, st)
        } else {
            return Ok(()); // iný kodek (Theora, Skeleton …) – stopa sa vynechá
        };
        self.limity.over_stopy(self.stopy.len() + 1)?;
        stopa.predvolena = self.stopy.is_empty();
        self.stopy.push(stopa);
        self.stav.push(StavStopy {
            serial: s.serial,
            druh,
            hlaviciek,
            rozpracovany: None,
            poradie: None,
            pozicia: None,
            predskok,
            viac_prudov,
            streaminfo,
            koniec: false,
            prerusenie: true,
        });
        Ok(())
    }

    /// Hlavičkový paket: značky z OpusTags, komentára Vorbis a blokov metadát FLAC (VORBIS_COMMENT, PICTURE).
    fn hlavickovy_paket(&mut self, ix: usize, d: &[u8]) {
        self.stav[ix].hlaviciek -= 1;
        let mut z = Znacky::nove();
        match self.stav[ix].druh {
            Druh::Opus if d.starts_with(b"OpusTags") => {
                z = znacky::vorbis_comment(&d[8..]).unwrap_or_default()
            }
            Druh::Vorbis if d.starts_with(b"\x03vorbis") => {
                z = znacky::vorbis_comment(&d[7..]).unwrap_or_default()
            }
            Druh::Flac if d.len() >= 4 && !d.starts_with(b"\x7FFLAC") => match d[0] & 0x7F {
                4 => z = znacky::vorbis_comment(&d[4..]).unwrap_or_default(),
                6 => znacky::flac_picture(&mut z, &d[4..]),
                _ => {}
            },
            _ => {}
        }
        if !z.je_prazdne() {
            self.znacky.get_or_insert_default().pripoj(z);
        }
    }

    /// Pakety ukončené na stránke (zlúčené segmenty, aj pokračovanie z predošlej stránky) s polohou začiatku.
    /// RFC 7845 §3: časť paketu bez predošlej časti (chýbajúca stránka, chýbajúci príznak pokračovania) sa zahodí.
    fn pakety_stranky(&mut self, ix: usize, s: &Stranka) -> Vec<(Vec<u8>, u64)> {
        let najviac = self.limity.paket as usize;
        let st = &mut self.stav[ix];
        let strata = st.poradie.is_some_and(|q| s.poradie != q.wrapping_add(1));
        st.poradie = Some(s.poradie);
        let mut zahodit_prvy = false;
        if s.pokracovanie {
            if st.rozpracovany.is_none() || strata {
                zahodit_prvy = true;
                st.rozpracovany = None;
            }
        } else {
            st.rozpracovany = None;
        }
        let (mut aktualny, mut zaciatok) = st.rozpracovany.take().unwrap_or((Vec::new(), s.pozicia));
        let (mut vysl, mut od, mut prvy) = (Vec::new(), 0usize, true);
        for &n in &s.segmenty {
            let kus = &s.data[od..od + n as usize];
            od += n as usize;
            if aktualny.len() + kus.len() <= najviac {
                aktualny.extend_from_slice(kus);
            } else {
                zahodit_prvy |= prvy; // príliš veľký paket sa zahodí
            }
            if n < 255 {
                let paket = std::mem::take(&mut aktualny);
                if !(prvy && zahodit_prvy) {
                    vysl.push((paket, zaciatok));
                }
                prvy = false;
                zaciatok = s.pozicia;
            }
        }
        if s.segmenty.last() == Some(&255) && !(prvy && zahodit_prvy) {
            st.rozpracovany = Some((aktualny, zaciatok));
        }
        vysl
    }

    /// Trvanie paketu v tikoch stopy (Opus podľa TOC, FLAC podľa hlavičky rámca); Vorbis `None`.
    fn trvanie_paketu(&self, ix: usize, d: &[u8]) -> Option<i64> {
        let st = &self.stav[ix];
        match st.druh {
            Druh::Opus => vzoriek_opus(d, st.viac_prudov),
            Druh::Flac => crate::flac::hlavicka_ramca(d, st.streaminfo.as_ref()).map(|h| h.blok as i64),
            Druh::Vorbis => None,
        }
    }

    /// Pakety jednej stránky do fronty s časmi (RFC 7845 §4).
    fn spracuj_stranku(&mut self, s: &Stranka) {
        let Some(ix) = self.stav.iter().position(|t| t.serial == s.serial) else { return };
        let pakety = self.pakety_stranky(ix, s);
        if s.eos {
            self.stav[ix].koniec = true;
        }
        if self.po_posune {
            // prvá stránka po posune: jej ukončené pakety sa zahodia, rozpracovaný začína na jej granule
            self.po_posune = false;
            if s.granula >= 0 && self.stav[ix].druh != Druh::Vorbis {
                self.stav[ix].pozicia = Some(s.granula);
            }
            return;
        }
        let pakety: Vec<_> = pakety.into_iter().filter(|(d, _)| !d.is_empty()).collect();
        let trvania: Vec<Option<i64>> = pakety.iter().map(|(d, _)| self.trvanie_paketu(ix, d)).collect();
        let sucet: Option<i64> = trvania.iter().copied().sum();
        let st = &mut self.stav[ix];
        if st.pozicia.is_none()
            && s.granula >= 0
            && let Some(sucet) = sucet
        {
            // prvá zvuková stránka: počiatočná pozícia spätne od granule; menšia granule → 0 (RFC 7845 §4)
            st.pozicia = Some(if s.granula >= sucet { s.granula - sucet } else { 0 });
        }
        // stránka EOS s granule menšou ako koniec posledného paketu: orezanie konca od posledného paketu
        let mut nadbytok = match (s.eos, st.pozicia, sucet) {
            (true, Some(p), Some(sucet)) if s.granula >= 0 => (p + sucet - s.granula).max(0),
            _ => 0,
        };
        let mut orezania = vec![0i64; pakety.len()];
        for (o, t) in orezania.iter_mut().zip(&trvania).rev() {
            let t = t.unwrap_or(0);
            *o = nadbytok.min(t);
            nadbytok -= *o;
        }
        let mut poz = st.pozicia;
        for (((data, pozicia), t), o) in pakety.into_iter().zip(trvania).zip(orezania) {
            let pts = poz.map(|p| p - st.predskok);
            self.fronta.push_back(Paket {
                stopa: ix,
                data,
                pts,
                dts: None,
                trvanie: t,
                klucovy: true,
                prerusenie: std::mem::take(&mut st.prerusenie),
                orezat_zaciatok: 0,
                orezat_koniec: o as u32,
                pozicia,
            });
            poz = poz.zip(t).map(|(p, t)| p + t);
        }
        // granule stránky je záväzná (oprava po strate paketov); pri EOS ostáva vypočítaný koniec; Vorbis bez časov
        st.pozicia = if st.druh == Druh::Vorbis {
            None
        } else if s.granula >= 0 && !s.eos {
            Some(s.granula)
        } else {
            poz
        };
    }

    /// Prvá stránka stopy `ix` s granule ≥ 0 od polohy `od` (najviac po `hranica`): (poloha, granule).
    fn stranka_s_granulou(&mut self, od: u64, ix: usize, hranica: u64) -> Vysledok<Option<(u64, i64)>> {
        let serial = self.stav[ix].serial;
        let mut p = od;
        while p < hranica {
            let Some(s) = self.dalsia_stranka(p)? else { return Ok(None) };
            if s.pozicia >= hranica {
                return Ok(None);
            }
            if s.serial == serial && s.granula >= 0 {
                return Ok(Some((s.pozicia, s.granula)));
            }
            p = s.pozicia + s.dlzka as u64;
        }
        Ok(None)
    }

    /// Trvanie z granule poslednej stránky prvej stopy (koniec súboru, posledných ≈ 128 kB).
    fn zisti_trvanie(&mut self) -> Option<i64> {
        let serial = self.stav.first()?.serial;
        let od = self.koniec.saturating_sub(2 * NAJVIAC_STRANKA as u64).max(self.prva_zvukova);
        let okno = self.c.bajty(od, (self.koniec - od) as usize).ok()?.to_vec();
        let kandidati: Vec<usize> =
            okno.windows(4).enumerate().filter(|(_, w)| *w == b"OggS").map(|(i, _)| i).collect();
        for &i in kandidati.iter().rev() {
            if let Ok(Some(s)) = self.stranka_na(od + i as u64)
                && s.serial == serial
                && s.granula >= 0
            {
                let st = &self.stav[0];
                return na_ns(s.granula - st.predskok, self.stopy[0].zakladna);
            }
        }
        None
    }
}

impl<Z: ZdrojBajtov> Kontajner for Ogg<Z> {
    fn format(&self) -> FormatKontajnera {
        FormatKontajnera::Ogg
    }

    fn stopy(&self) -> &[Stopa] {
        &self.stopy
    }

    fn trvanie_ns(&self) -> Option<i64> {
        self.trvanie_ns
    }

    fn znacky(&self) -> Option<&Znacky> {
        self.znacky.as_ref()
    }

    fn dalsi_paket(&mut self) -> Vysledok<Option<Paket>> {
        loop {
            if let Some(p) = self.fronta.pop_front() {
                return Ok(Some(p));
            }
            if self.stav.iter().all(|s| s.koniec) {
                return Ok(None);
            }
            let Some(s) = self.dalsia_stranka(self.p)? else { return Ok(None) };
            self.p = s.pozicia + s.dlzka as u64;
            if s.bos && self.stav.iter().all(|t| t.serial != s.serial) {
                return Ok(None); // ďalší článok zreťazeného toku – zatiaľ koniec
            }
            self.spracuj_stranku(&s);
        }
    }

    /// Posun delením intervalu na stránku prvej stopy s najväčšou granule ≤ cieľ (RFC 7845 §6); pri
    /// `Posun::Presny` o predbeh stopy skôr. Vráti čas prvého paketu za posunom.
    fn posun(&mut self, ciel_ns: i64, sposob: Posun) -> Vysledok<i64> {
        let ciel_ns = match sposob {
            Posun::Presny => ciel_ns - self.stopy[0].predbeh_posunu_ns,
            _ => ciel_ns,
        };
        let predskok = self.stav[0].predskok;
        let ciel = z_ns(ciel_ns.max(0), self.stopy[0].zakladna).unwrap_or(0) + predskok;
        let (mut lo, mut hi) = (self.prva_zvukova, self.koniec);
        let mut najlepsia: Option<(u64, i64)> = None;
        for _ in 0..48 {
            if hi <= lo + 1 {
                break;
            }
            let stred = lo + (hi - lo) / 2;
            match self.stranka_s_granulou(stred, 0, hi)? {
                Some((p, g)) if g <= ciel => {
                    najlepsia = Some((p, g));
                    lo = p + 1;
                }
                _ => hi = stred,
            }
        }
        self.znovu_od_zaciatku_toku();
        match najlepsia {
            Some((p, g)) => {
                self.p = p;
                self.po_posune = true;
                for s in &mut self.stav {
                    s.pozicia = None;
                }
                self.stav[0].pozicia = Some(g);
                Ok(na_ns(g - predskok, self.stopy[0].zakladna).unwrap_or(0))
            }
            None => {
                // pred prvou stránkou s granule: od začiatku zvuku
                self.p = self.prva_zvukova;
                self.po_posune = false;
                for s in &mut self.stav {
                    s.pozicia = None;
                }
                Ok(na_ns(-predskok, self.stopy[0].zakladna).unwrap_or(0))
            }
        }
    }
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::zdroj::PametovyZdroj;

    /// Stránka Ogg z dĺžok segmentov a dát (CRC dopočíta).
    fn stranka(
        priznaky: u8,
        granula: i64,
        serial: u32,
        poradie: u32,
        segmenty: &[u8],
        data: &[u8],
    ) -> Vec<u8> {
        let mut d = b"OggS\x00".to_vec();
        d.push(priznaky);
        d.extend(granula.to_le_bytes());
        d.extend(serial.to_le_bytes());
        d.extend(poradie.to_le_bytes());
        d.extend([0; 4]);
        d.push(segmenty.len() as u8);
        d.extend(segmenty);
        d.extend(data);
        let c = crc_ogg(&d);
        d[22..26].copy_from_slice(&c.to_le_bytes());
        d
    }

    /// Celé pakety (každý < 255·k + 255 B) na jednej stránke.
    fn stranka_paketov(priznaky: u8, granula: i64, poradie: u32, pakety: &[Vec<u8>]) -> Vec<u8> {
        let (mut seg, mut data) = (Vec::new(), Vec::new());
        for p in pakety {
            seg.extend(std::iter::repeat_n(255u8, p.len() / 255));
            seg.push((p.len() % 255) as u8);
            data.extend(p);
        }
        stranka(priznaky, granula, 7, poradie, &seg, &data)
    }

    fn opus_head(predskok: u16) -> Vec<u8> {
        let mut h = b"OpusHead\x01\x01".to_vec();
        h.extend(predskok.to_le_bytes());
        h.extend(48_000u32.to_le_bytes());
        h.extend([0, 0, 0]);
        h
    }

    fn opus_tags() -> Vec<u8> {
        let mut t = b"OpusTags".to_vec();
        t.extend(1u32.to_le_bytes());
        t.push(b'x');
        let polozky = ["TITLE=Skúška", "R128_TRACK_GAIN=-573"];
        t.extend((polozky.len() as u32).to_le_bytes());
        for p in polozky {
            t.extend((p.len() as u32).to_le_bytes());
            t.extend(p.as_bytes());
        }
        t
    }

    /// TOC 0xF8: CELT, plné pásmo, 20 ms, mono, kód 0 → 960 vzoriek.
    fn paket(i: u8) -> Vec<u8> {
        vec![0xF8, i, i, i]
    }

    fn subor() -> Vec<u8> {
        let mut d = stranka_paketov(2, 0, 0, &[opus_head(312)]);
        d.extend(stranka_paketov(0, 0, 1, &[opus_tags()]));
        d.extend(stranka_paketov(0, 2880, 2, &[paket(1), paket(2), paket(3)]));
        d.extend(stranka_paketov(0, 5760, 3, &[paket(4), paket(5), paket(6)]));
        // koniec: 2 pakety, granule o 460 menšia ako ich koniec → orezanie posledného paketu
        d.extend(stranka_paketov(4, 5760 + 960 + 500, 4, &[paket(7), paket(8)]));
        d
    }

    fn vsetky(o: &mut Ogg<PametovyZdroj>) -> Vec<Paket> {
        std::iter::from_fn(|| o.dalsi_paket().unwrap()).collect()
    }

    #[test]
    fn opus_casy_preskok_orezanie_a_znacky() {
        let mut o = Ogg::otvor(PametovyZdroj::novy(subor()), Limity::default()).unwrap();
        assert_eq!(o.format(), FormatKontajnera::Ogg);
        assert_eq!(o.stopy()[0].kodek, Kodek::Opus);
        assert_eq!(o.stopy()[0].oneskorenie_kodeku_ns, 6_500_000);
        // granule poslednej stránky 7220 − pre-skip 312 = 6908 vzoriek = 143 916 666,7 ns → k najbližšiemu
        assert_eq!(o.trvanie_ns(), Some(143_916_667));
        let z = o.znacky().unwrap();
        assert_eq!(z.hodnota("TITLE"), Some("Skúška"));
        assert_eq!(z.replaygain().unwrap().stopa_db, Some(-573.0 / 256.0 + 5.0));
        let p = vsetky(&mut o);
        assert_eq!(
            p.iter().map(|p| p.pts.unwrap()).collect::<Vec<_>>(),
            (0..8).map(|i| i * 960 - 312).collect::<Vec<_>>()
        );
        assert!(p[0].prerusenie && !p[1].prerusenie);
        assert_eq!((p[6].orezat_koniec, p[7].orezat_koniec), (0, 460));
        assert_eq!(p[2].data, paket(3));
    }

    #[test]
    fn paket_cez_stranky_strata_a_poskodenie() {
        let velky: Vec<u8> = std::iter::once(0xF8).chain((0..599).map(|i| i as u8)).collect();
        let mut d = stranka_paketov(2, 0, 0, &[opus_head(0)]);
        d.extend(stranka_paketov(0, 0, 1, &[opus_tags()]));
        // paket 600 B: 255 + 255 na stránke 2 (pokračuje), 90 na stránke 3
        d.extend(stranka(0, -1, 7, 2, &[255, 255], &velky[..510]));
        d.extend(stranka(1, 960, 7, 3, &[90, 4], &[&velky[510..], &paket(9)[..]].concat()));
        // stránka 4 chýba: pokračovanie na stránke 5 sa zahodí, celý paket za ním ostane
        d.extend(stranka(1, -1, 7, 5, &[10, 4], &[&[0u8; 10][..], &paket(7)[..]].concat()));
        let mut poskodena = stranka_paketov(4, 5000, 6, &[paket(8)]);
        poskodena[30] ^= 1; // zlé CRC – stránka sa preskočí
        d.extend(poskodena);
        let mut o = Ogg::otvor(PametovyZdroj::novy(d), Limity::default()).unwrap();
        let p = vsetky(&mut o);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].data, velky);
        assert_eq!(p[1].data, paket(9));
        assert_eq!(p[2].data, paket(7));
    }

    #[test]
    fn posun_delenim() {
        let mut d = stranka_paketov(2, 0, 0, &[opus_head(312)]);
        d.extend(stranka_paketov(0, 0, 1, &[opus_tags()]));
        for s in 0..50u32 {
            let pakety: Vec<_> = (0..5).map(|i| paket((s * 5 + i) as u8)).collect();
            d.extend(stranka_paketov(if s == 49 { 4 } else { 0 }, (s as i64 + 1) * 4800, s + 2, &pakety));
        }
        let mut o = Ogg::otvor(PametovyZdroj::novy(d), Limity::default()).unwrap();
        // cieľ 1 s = vzorka 48 000 + 312 → stránka s granule 48 000 (najväčšia ≤ 48 312)
        let t = o.posun(1_000_000_000, Posun::KlucovyPred).unwrap();
        assert_eq!(t, 993_500_000); // 47 688 vzoriek pri 48 kHz
        let p = o.dalsi_paket().unwrap().unwrap();
        assert_eq!((p.pts, p.prerusenie, p.data[1]), (Some(48_000 - 312), true, 50));
        // pred začiatok: od prvej zvukovej stránky
        o.posun(0, Posun::Presny).unwrap();
        assert_eq!(o.dalsi_paket().unwrap().unwrap().pts, Some(-312));
    }
}
