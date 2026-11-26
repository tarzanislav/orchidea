//! **Rozhranie R5 – médiá → ORCHIDEA OS** (prúd D → prúd A; dohoda v dokumente projektu
//! `ORCHIDEA/rozhrania.md`, časť R5). Neutrálne – žiadne ioctl ani štruktúry jadra; tie robí prúd A
//! v `media-hw`.
//!
//! - **R5a – video pre hardvérové dekodéry (V4L2):** [`PrevodnikVidea`] urobí z paketu kontajnera
//!   [`PaketVidea`] – tok, ktorý dekodér prijme do buffera OUTPUT (H.264 a HEVC v Annex B so sadami parametrov
//!   pred kľúčovými celkami, AV1 ako časová jednotka OBU s oddeľovačom). `id` je stúpajúce číslo paketu na časovú
//!   značku buffera (ako `dpb[i].timestamp` v R1), `pts_ns` slúži synchronizácii.
//! - **R5b – zvuk pre HDMI:** prúd A opíše, čo vie ovládač a televízor ([`SchopnostiHdmi`]); prevodník zvuku
//!   (etapa L5) zvolí bitovo presné PCM, priechod IEC 61937 alebo PCM s prevodom a dá [`NastavenieHdmi`]
//!   a [`ZvukHdmi`]. Základ – prekladanie a prevod hĺbky so zaokrúhlením – je [`prekladaj`]. Zápis do ALSA
//!   a balenie do podrámcov IEC 60958 robí prúd A (R226).
//! - **R5c – merania (A → D):** [`MeranieZvuku`] po zápise do zvukového zariadenia a [`Obnova`] pri každom
//!   obnovení displeja – vstupy hodín prehrávania (etapa L6).

use crate::cas::na_ns;
use crate::paket::Paket;
use crate::stopa::{Kodek, Stopa};
use crate::zvuk::{BlokZvuku, VzorkyZvuku};
use orchidea_jadro::{Chyba, Vysledok, Zlomok};

// ---------------------------------------------------------------- R5a – video

/// Štartovací kód Annex B (4 bajty – dekodéry ho prijímajú pred každou jednotkou NAL).
pub const STARTOVACI_KOD: [u8; 4] = [0, 0, 0, 1];

/// Oddeľovač časovej jednotky AV1 (OBU_TEMPORAL_DELIMITER s poľom veľkosti 0).
const AV1_ODDELOVAC: [u8; 2] = [0x12, 0x00];

/// Formát toku pre hardvérový dekodér (prúd A ho mapuje na štvorznak V4L2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokVidea {
    /// H.264 – Annex B, SPS a PPS pred prvým a každým kľúčovým celkom.
    H264,
    /// HEVC – Annex B, VPS, SPS a PPS pred prvým a každým kľúčovým celkom.
    Hevc,
    /// AV1 – časová jednotka OBU v nízkoréžijnom formáte s oddeľovačom; hlavička sekvencie pred kľúčovým rámcom.
    Av1,
    /// VP9 – rámec alebo superrámec, ako v kontajneri.
    Vp9,
    /// VP8 – rámec.
    Vp8,
    /// MPEG-2 video – elementárny tok, sekvenčná hlavička pred kľúčovým snímkom.
    Mpeg2,
}

impl TokVidea {
    /// Tok pre kodek; `None` – kodek hardvérovým dekodérom cez R5 nejde.
    pub fn z_kodeku(kodek: &Kodek) -> Option<TokVidea> {
        Some(match kodek {
            Kodek::H264 => TokVidea::H264,
            Kodek::Hevc => TokVidea::Hevc,
            Kodek::Av1 => TokVidea::Av1,
            Kodek::Vp9 => TokVidea::Vp9,
            Kodek::Vp8 => TokVidea::Vp8,
            Kodek::Mpeg2Video => TokVidea::Mpeg2,
            _ => return None,
        })
    }
}

/// Jeden celok pripravený pre buffer OUTPUT hardvérového dekodéra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaketVidea {
    pub tok: TokVidea,
    /// Hotové bajty pre dekodér.
    pub data: Vec<u8>,
    /// Stúpajúce číslo paketu (od 1) – časová značka buffera OUTPUT; dekodér ju skopíruje k dekódovanej snímke.
    pub id: u64,
    /// Čas zobrazenia na časovej osi média (ns).
    pub pts_ns: Option<i64>,
    pub trvanie_ns: Option<i64>,
    /// Kľúčový celok – od neho sa dá začať dekódovať.
    pub klucovy: bool,
    /// Na začiatku sú vložené sady parametrov (dekodér sa z nich (pre)nastaví).
    pub parametre: bool,
    /// Prvý paket po prerušení alebo posune – dekodér treba pred ním vyprázdniť.
    pub prerusenie: bool,
}

/// Konfigurácia toku jednotiek NAL (H.264 avcC, HEVC hvcC – ISO/IEC 14496-15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KonfiguraciaNal {
    /// Dĺžka poľa dĺžky pred každou jednotkou NAL v paketoch (1–4 B); 0 = pakety sú už v Annex B.
    pub dlzka_pola: u8,
    /// Sady parametrov v Annex B (so štartovacími kódmi).
    pub sady: Vec<u8>,
}

struct Citac<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Citac<'a> {
    fn bajty(&mut self, n: usize) -> Vysledok<&'a [u8]> {
        let k = self.p.checked_add(n).filter(|&k| k <= self.d.len());
        let k = k.ok_or_else(|| Chyba::PoskodeneData("konfigurácia kodeku je skrátená".into()))?;
        let s = &self.d[self.p..k];
        self.p = k;
        Ok(s)
    }
    fn u8(&mut self) -> Vysledok<u8> {
        Ok(self.bajty(1)?[0])
    }
    fn u16(&mut self) -> Vysledok<u16> {
        let b = self.bajty(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn nal(&mut self, sady: &mut Vec<u8>) -> Vysledok<()> {
        let n = self.u16()? as usize;
        let nal = self.bajty(n)?;
        if !nal.is_empty() {
            sady.extend_from_slice(&STARTOVACI_KOD);
            sady.extend_from_slice(nal);
        }
        Ok(())
    }
}

/// Je to už tok Annex B (začína štartovacím kódom 00 00 01 alebo 00 00 00 01)?
fn je_annex_b(d: &[u8]) -> bool {
    d.starts_with(&[0, 0, 1]) || d.starts_with(&[0, 0, 0, 1])
}

/// Rozbor `AVCDecoderConfigurationRecord` (avcC): dĺžka poľa NAL a SPS + PPS v Annex B.
/// Konfigurácia, ktorá je už v Annex B (niektoré súbory Matroska), sa prevezme tak, ako je.
pub fn avcc(konfiguracia: &[u8]) -> Vysledok<KonfiguraciaNal> {
    if je_annex_b(konfiguracia) {
        return Ok(KonfiguraciaNal { dlzka_pola: 0, sady: konfiguracia.to_vec() });
    }
    let mut c = Citac { d: konfiguracia, p: 0 };
    if c.u8()? != 1 {
        return Err(Chyba::PoskodeneData("avcC: neznáma verzia konfigurácie".into()));
    }
    c.bajty(3)?; // profil, kompatibilita, úroveň
    let dlzka_pola = (c.u8()? & 3) + 1;
    let mut sady = Vec::new();
    for _ in 0..(c.u8()? & 0x1F) {
        c.nal(&mut sady)?;
    }
    for _ in 0..c.u8()? {
        c.nal(&mut sady)?;
    }
    Ok(KonfiguraciaNal { dlzka_pola, sady })
}

/// Rozbor `HEVCDecoderConfigurationRecord` (hvcC): dĺžka poľa NAL a polia VPS, SPS, PPS (a SEI) v Annex B.
pub fn hvcc(konfiguracia: &[u8]) -> Vysledok<KonfiguraciaNal> {
    if je_annex_b(konfiguracia) {
        return Ok(KonfiguraciaNal { dlzka_pola: 0, sady: konfiguracia.to_vec() });
    }
    let mut c = Citac { d: konfiguracia, p: 0 };
    if c.u8()? != 1 {
        return Err(Chyba::PoskodeneData("hvcC: neznáma verzia konfigurácie".into()));
    }
    c.bajty(20)?; // profil, úroveň, segmentácia, formát chrominancie, hĺbky, frekvencia snímok
    let dlzka_pola = (c.u8()? & 3) + 1;
    let mut sady = Vec::new();
    for _ in 0..c.u8()? {
        c.u8()?; // úplnosť poľa a typ jednotky NAL
        for _ in 0..c.u16()? {
            c.nal(&mut sady)?;
        }
    }
    Ok(KonfiguraciaNal { dlzka_pola, sady })
}

/// Prevod jednotiek NAL ohraničených dĺžkou (big-endian pole `dlzka_pola` bajtov – MP4, Matroska) na Annex B so
/// 4-bajtovými štartovacími kódmi; pridá na koniec `ciel`. Prázdne jednotky sa vynechajú. Vráti počet jednotiek.
pub fn annex_b_z_dlzok(data: &[u8], dlzka_pola: u8, ciel: &mut Vec<u8>) -> Vysledok<usize> {
    let k = dlzka_pola as usize;
    if !(1..=4).contains(&k) {
        return Err(Chyba::MimoRozsahu(format!("dĺžka poľa NAL {k} B")));
    }
    let mut p = 0usize;
    let mut pocet = 0;
    while p < data.len() {
        let pole =
            data.get(p..p + k).ok_or_else(|| Chyba::PoskodeneData("skrátené pole dĺžky NAL".into()))?;
        let n = pole.iter().fold(0usize, |a, &b| (a << 8) | b as usize);
        p += k;
        let nal = data
            .get(p..p.saturating_add(n))
            .ok_or_else(|| Chyba::PoskodeneData(format!("jednotka NAL dĺžky {n} B presahuje paket")))?;
        if !nal.is_empty() {
            ciel.extend_from_slice(&STARTOVACI_KOD);
            ciel.extend_from_slice(nal);
            pocet += 1;
        }
        p += n;
    }
    Ok(pocet)
}

/// Prevodník paketov jednej videostopy na pakety pre hardvérový dekodér (R5a).
#[derive(Debug, Clone)]
pub struct PrevodnikVidea {
    tok: TokVidea,
    nal: Option<KonfiguraciaNal>,
    /// Sady parametrov alebo hlavička sekvencie na vloženie pred kľúčový celok.
    hlavicka: Vec<u8>,
    zakladna: Zlomok,
    dalsie_id: u64,
    prerusenie: bool,
}

impl PrevodnikVidea {
    pub fn novy(stopa: &Stopa) -> Vysledok<PrevodnikVidea> {
        let tok = TokVidea::z_kodeku(&stopa.kodek).ok_or_else(|| {
            Chyba::MimoRozsahu(format!("kodek {} nejde do hardvérového dekodéra", stopa.kodek.meno()))
        })?;
        let k = &stopa.konfiguracia;
        let (nal, hlavicka) = match tok {
            TokVidea::H264 | TokVidea::Hevc if k.is_empty() => {
                (Some(KonfiguraciaNal { dlzka_pola: 0, sady: vec![] }), vec![])
            }
            TokVidea::H264 => {
                let n = avcc(k)?;
                let s = n.sady.clone();
                (Some(n), s)
            }
            TokVidea::Hevc => {
                let n = hvcc(k)?;
                let s = n.sady.clone();
                (Some(n), s)
            }
            // av1C: 4 bajty hlavičky, potom konfiguračné OBU (hlavička sekvencie) s poľom veľkosti
            TokVidea::Av1 => (None, k.get(4..).unwrap_or_default().to_vec()),
            TokVidea::Mpeg2 => (None, k.clone()),
            TokVidea::Vp9 | TokVidea::Vp8 => (None, vec![]),
        };
        Ok(PrevodnikVidea { tok, nal, hlavicka, zakladna: stopa.zakladna, dalsie_id: 1, prerusenie: true })
    }

    pub fn tok(&self) -> TokVidea {
        self.tok
    }

    /// Po posune v súbore: ďalší paket bude mať príznak prerušenia a sady parametrov.
    pub fn prerusenie(&mut self) {
        self.prerusenie = true;
    }

    /// Prevedie paket kontajnera na paket pre dekodér.
    pub fn prevod(&mut self, paket: &Paket) -> Vysledok<PaketVidea> {
        let prerusenie = std::mem::take(&mut self.prerusenie) || paket.prerusenie;
        let vloz = !self.hlavicka.is_empty() && (paket.klucovy || prerusenie);
        let mut data = Vec::with_capacity(paket.data.len() + self.hlavicka.len() + 16);
        if self.tok == TokVidea::Av1 {
            data.extend_from_slice(&AV1_ODDELOVAC);
        }
        if vloz {
            data.extend_from_slice(&self.hlavicka);
        }
        match &self.nal {
            Some(n) if n.dlzka_pola > 0 => {
                annex_b_z_dlzok(&paket.data, n.dlzka_pola, &mut data)?;
            }
            _ if self.tok == TokVidea::Av1 && paket.data.starts_with(&AV1_ODDELOVAC) => {
                data.extend_from_slice(&paket.data[AV1_ODDELOVAC.len()..]);
            }
            _ => data.extend_from_slice(&paket.data),
        }
        let id = self.dalsie_id;
        self.dalsie_id += 1;
        let ns = |t: Option<i64>| t.and_then(|t| na_ns(t, self.zakladna));
        Ok(PaketVidea {
            tok: self.tok,
            data,
            id,
            pts_ns: ns(paket.pts),
            trvanie_ns: ns(paket.trvanie),
            klucovy: paket.klucovy,
            parametre: vloz,
            prerusenie,
        })
    }
}

// ---------------------------------------------------------------- R5b – zvuk pre HDMI

/// Formát vzoriek, ktorý prijíma ovládač zvuku HDMI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FormatVzoriekHdmi {
    /// 16 bitov so znamienkom, malokoncové.
    S16Le,
    /// 32 bitov so znamienkom, malokoncové (24 bitov zvuku v hornej časti slova).
    S32Le,
    /// Podrámce IEC 60958 v 32-bitových slovách (ovládač vc4 HDMI na Raspberry Pi prijíma len tento formát).
    /// Prevodník zvuku vtedy dá `S32Le` a bajty stavu kanála; do podrámcov ich zabalí zapisovač prúdu A
    /// (R226, `sklenik/klic/src/zvuk.rs`) – jedno balenie v systéme, nie dve.
    Iec958PodramecLe,
}

/// Komprimovaný zvuk, ktorý ide priechodom do televízora (IEC 61937) – procesor ho nedekóduje.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DruhPriechodu {
    Ac3,
    Eac3,
}

/// Čo vie ovládač a televízor (prúd A zistí z EDID – krátke popisy zvuku, HDMI VSDB – a z ovládača).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SchopnostiHdmi {
    /// Formáty vzoriek ovládača v poradí preferencie.
    pub formaty: Vec<FormatVzoriekHdmi>,
    /// Najviac kanálov LPCM, ktoré televízor prijme.
    pub max_kanalov: u8,
    /// Frekvencie LPCM (Hz), ktoré televízor prijme.
    pub frekvencie: Vec<u32>,
    /// Najväčšia bitová hĺbka LPCM televízora (16, 20, 24).
    pub max_bitov: u8,
    /// Televízor dekóduje AC-3, resp. E-AC-3 (a používateľ priechod dovolil).
    pub priechod_ac3: bool,
    pub priechod_eac3: bool,
    /// Oneskorenie zvuku televízora (ns) z HDMI VSDB, ak ho udáva.
    pub oneskorenie_tv_ns: Option<i64>,
}

/// Nastavenie výstupu, ktoré prúd A použije pri otvorení zariadenia (parametre hardvéru, InfoFrame zvuku,
/// stav kanála IEC 60958).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NastavenieHdmi {
    pub format: FormatVzoriekHdmi,
    pub frekvencia: u32,
    pub kanalov: u8,
    /// Kód rozloženia reproduktorov CTA-861 (CA) pre InfoFrame zvuku.
    pub pridelenie_cta: u8,
    /// Bajty stavu kanála IEC 60958 (pri priechode príznak „nie zvuk PCM“).
    pub stav_kanala: [u8; 24],
    pub priechod: Option<DruhPriechodu>,
}

/// Zvuk pripravený na zápis do zariadenia HDMI (prekladané rámce vo formáte z `NastavenieHdmi`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ZvukHdmi {
    pub data: Vec<u8>,
    pub ramcov: usize,
    /// Čas prvého rámca na časovej osi média (ns).
    pub pts_ns: Option<i64>,
}

/// Hĺbka prekladaného PCM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HlbkaPcm {
    S16,
    S32,
}

/// Dolná celá časť bez systémovej knižnice (|y| < 2^62) – rovnaký výsledok na každej platforme.
fn podlaha(y: f64) -> i64 {
    let t = y as i64;
    if (t as f64) > y { t - 1 } else { t }
}

/// Plávajúca vzorka (±1,0) na celé číslo s `bitov` bitmi: násobok 2^(bitov−1), zaokrúhlenie polovicou nahor
/// (⌊y + ½⌋), orezanie na rozsah, NaN → 0. Výpočet v `f64` je presný.
fn f32_na_cele(x: f32, bitov: u32) -> i64 {
    let mierka = (1u64 << (bitov - 1)) as f64;
    let y = x as f64 * mierka;
    if y.is_nan() {
        return 0;
    }
    let y = y.clamp(-mierka, mierka); // ±nekonečno aj veľké hodnoty
    podlaha(y + 0.5).clamp(-(mierka as i64), mierka as i64 - 1)
}

/// Celá vzorka s `z` bitmi na `na` bitov: rozšírenie posunom doľava, zúženie zaokrúhlením polovicou nahor
/// (⌊v / 2^s + ½⌋) a orezaním (bez rozptylu – ten patrí zvukovému enginu).
fn cele_na_cele(v: i32, z: u32, na: u32) -> i64 {
    let v = (v as i64).clamp(-(1i64 << (z - 1)), (1i64 << (z - 1)) - 1);
    if z <= na {
        v << (na - z)
    } else {
        let s = z - na;
        ((v + (1i64 << (s - 1))) >> s).clamp(-(1i64 << (na - 1)), (1i64 << (na - 1)) - 1)
    }
}

/// Preloží planárny blok na prekladané malokoncové PCM (rámec po rámci, kanály v poradí bloku) a pridá ho na
/// koniec `ciel`. Poradie kanálov podľa CTA-861 a podrámce IEC 60958 robí prevodník zvuku (L5) nad touto funkciou.
pub fn prekladaj(blok: &BlokZvuku, hlbka: HlbkaPcm, ciel: &mut Vec<u8>) -> Vysledok<()> {
    blok.over()?;
    let k = blok.kanalov();
    let n = blok.ramcov;
    let (na, bajtov) = match hlbka {
        HlbkaPcm::S16 => (16u32, 2usize),
        HlbkaPcm::S32 => (32u32, 4usize),
    };
    ciel.reserve(k * n * bajtov);
    let mut zapis = |v: i64| match hlbka {
        HlbkaPcm::S16 => ciel.extend_from_slice(&(v as i16).to_le_bytes()),
        HlbkaPcm::S32 => ciel.extend_from_slice(&(v as i32).to_le_bytes()),
    };
    match &blok.vzorky {
        VzorkyZvuku::Cele { bitov, data } => {
            for i in 0..n {
                for c in 0..k {
                    zapis(cele_na_cele(data[c * n + i], *bitov as u32, na));
                }
            }
        }
        VzorkyZvuku::F32(data) => {
            for i in 0..n {
                for c in 0..k {
                    zapis(f32_na_cele(data[c * n + i], na));
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- R5c – merania (A → D)

/// Meranie zvukového zariadenia po zápise (stačí 4× za sekundu).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeranieZvuku {
    /// Rámcov odovzdaných zariadeniu od začiatku toku (od posledného posunu).
    pub odovzdanych_ramcov: u64,
    /// Rámcov v zariadení, ktoré ešte nezazneli (oneskorenie zariadenia).
    pub oneskorenie_ramcov: u64,
    /// Monotónny čas merania (ns) – ideálne časová značka ovládača.
    pub monotonny_ns: i64,
}

/// Obnovenie displeja (udalosť DRM): čas najbližšieho obnovenia a perióda.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Obnova {
    pub monotonny_ns: i64,
    pub perioda_ns: i64,
}

#[cfg(test)]
mod skusky {
    use super::*;
    use crate::cas::zakladna_z_ns;
    use crate::zvuk::RozlozenieKanalov;

    fn sps() -> Vec<u8> {
        vec![0x67, 0x64, 0x00, 0x28, 0xAC]
    }
    fn pps() -> Vec<u8> {
        vec![0x68, 0xEE, 0x3C, 0x80]
    }

    fn avcc_zaznam(dlzka_pola: u8) -> Vec<u8> {
        let mut d = vec![1, 0x64, 0x00, 0x28, 0xFC | (dlzka_pola - 1), 0xE1];
        d.extend_from_slice(&(sps().len() as u16).to_be_bytes());
        d.extend(sps());
        d.push(1);
        d.extend_from_slice(&(pps().len() as u16).to_be_bytes());
        d.extend(pps());
        d.extend_from_slice(&[0xFD, 0xF8, 0xF8, 0]); // rozšírenie High profilu – ignoruje sa
        d
    }

    fn s_kodom(nal: &[u8]) -> Vec<u8> {
        let mut v = STARTOVACI_KOD.to_vec();
        v.extend_from_slice(nal);
        v
    }

    #[test]
    fn avcc_rozbor() {
        let k = avcc(&avcc_zaznam(4)).unwrap();
        assert_eq!(k.dlzka_pola, 4);
        assert_eq!(k.sady, [s_kodom(&sps()), s_kodom(&pps())].concat());
        assert_eq!(avcc(&avcc_zaznam(2)).unwrap().dlzka_pola, 2);
        let z = avcc_zaznam(4);
        for n in 0..z.len() - 4 {
            assert!(avcc(&z[..n]).is_err(), "skrátené na {n}");
        }
        assert!(avcc(&[2, 0, 0, 0, 0xFF, 0xE0, 0]).is_err());
        let ab = [0, 0, 0, 1, 0x67, 1, 2];
        assert_eq!(avcc(&ab).unwrap(), KonfiguraciaNal { dlzka_pola: 0, sady: ab.to_vec() });
    }

    #[test]
    fn hvcc_rozbor() {
        let mut d = vec![1u8];
        d.extend_from_slice(&[
            0x01, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 0x5D, 0xF0, 0, 0xFC, 0xFD, 0xF8, 0xF8, 0, 0,
        ]);
        d.push(0x0F); // dĺžka poľa 4
        d.push(3);
        for (typ, nal) in
            [(32u8, vec![0x40, 0x01, 0x0C]), (33, vec![0x42, 0x01, 0x01, 0x60]), (34, vec![0x44, 0x01, 0xC1])]
        {
            d.push(0x80 | typ);
            d.extend_from_slice(&1u16.to_be_bytes());
            d.extend_from_slice(&(nal.len() as u16).to_be_bytes());
            d.extend(nal);
        }
        let k = hvcc(&d).unwrap();
        assert_eq!(k.dlzka_pola, 4);
        assert_eq!(
            k.sady,
            [s_kodom(&[0x40, 0x01, 0x0C]), s_kodom(&[0x42, 0x01, 0x01, 0x60]), s_kodom(&[0x44, 0x01, 0xC1])]
                .concat()
        );
        for n in 0..d.len() {
            assert!(hvcc(&d[..n]).is_err(), "skrátené na {n}");
        }
    }

    #[test]
    fn dlzky_na_annex_b() {
        let mut p = Vec::new();
        for nal in [&[0x65u8, 1, 2, 3][..], &[][..], &[0x06, 9][..]] {
            p.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            p.extend_from_slice(nal);
        }
        let mut v = Vec::new();
        assert_eq!(annex_b_z_dlzok(&p, 4, &mut v).unwrap(), 2);
        assert_eq!(v, [s_kodom(&[0x65, 1, 2, 3]), s_kodom(&[0x06, 9])].concat());
        // 2- a 1-bajtové polia
        let mut v = Vec::new();
        annex_b_z_dlzok(&[0, 2, 0x41, 7, 0, 1, 0x01], 2, &mut v).unwrap();
        assert_eq!(v, [s_kodom(&[0x41, 7]), s_kodom(&[0x01])].concat());
        let mut v = Vec::new();
        assert_eq!(annex_b_z_dlzok(&[2, 0x41, 7], 1, &mut v).unwrap(), 1);
        // poškodené: dĺžka presahuje, skrátené pole, zlá dĺžka poľa
        assert!(annex_b_z_dlzok(&[0, 0, 0, 9, 1], 4, &mut Vec::new()).is_err());
        assert!(annex_b_z_dlzok(&[0, 0, 0, 1, 5, 0, 0], 4, &mut Vec::new()).is_err());
        assert!(annex_b_z_dlzok(&p, 0, &mut Vec::new()).is_err());
        assert!(annex_b_z_dlzok(&p, 5, &mut Vec::new()).is_err());
    }

    #[test]
    fn prevodnik_h264() {
        let mut s = Stopa::nova(1, Kodek::H264, zakladna_z_ns(1_000_000).unwrap());
        s.konfiguracia = avcc_zaznam(4);
        let mut pv = PrevodnikVidea::novy(&s).unwrap();
        let telo = [0u8, 0, 0, 3, 0x65, 0xAA, 0xBB];
        let mut paket = Paket {
            data: telo.to_vec(),
            pts: Some(40),
            trvanie: Some(40),
            klucovy: true,
            ..Paket::default()
        };
        let a = pv.prevod(&paket).unwrap();
        let sady = [s_kodom(&sps()), s_kodom(&pps())].concat();
        assert_eq!(a.data, [sady.clone(), s_kodom(&[0x65, 0xAA, 0xBB])].concat());
        assert!(a.parametre && a.prerusenie && a.klucovy);
        assert_eq!((a.id, a.pts_ns, a.trvanie_ns), (1, Some(40_000_000), Some(40_000_000)));
        // nekľúčový paket bez sád parametrov
        paket.klucovy = false;
        paket.data = vec![0, 0, 0, 2, 0x41, 0x01];
        let b = pv.prevod(&paket).unwrap();
        assert_eq!(b.data, s_kodom(&[0x41, 0x01]));
        assert!(!b.parametre && !b.prerusenie);
        assert_eq!(b.id, 2);
        // po posune znova sady a prerušenie
        pv.prerusenie();
        let c = pv.prevod(&paket).unwrap();
        assert!(c.parametre && c.prerusenie);
        assert!(c.data.starts_with(&sady));
        // poškodený paket je chyba, nie pád
        paket.data = vec![0, 0, 1, 0];
        assert!(pv.prevod(&paket).is_err());
    }

    #[test]
    fn prevodnik_av1_a_ine() {
        let mut s = Stopa::nova(1, Kodek::Av1, zakladna_z_ns(1_000_000).unwrap());
        s.konfiguracia = vec![0x81, 0x08, 0x0C, 0x00, 0x0A, 0x02, 0xAB, 0xCD];
        let mut pv = PrevodnikVidea::novy(&s).unwrap();
        let k =
            pv.prevod(&Paket { data: vec![0x32, 0x01, 0x77], klucovy: true, ..Paket::default() }).unwrap();
        assert_eq!(k.data, [0x12, 0x00, 0x0A, 0x02, 0xAB, 0xCD, 0x32, 0x01, 0x77]);
        // oddeľovač, ktorý už v pakete je, sa nezdvojí
        let n = pv.prevod(&Paket { data: vec![0x12, 0x00, 0x32, 0x01, 0x78], ..Paket::default() }).unwrap();
        assert_eq!(n.data, [0x12, 0x00, 0x32, 0x01, 0x78]);
        let vp9 = Stopa::nova(2, Kodek::Vp9, zakladna_z_ns(1_000_000).unwrap());
        let mut pv = PrevodnikVidea::novy(&vp9).unwrap();
        assert_eq!(
            pv.prevod(&Paket { data: vec![1, 2, 3], klucovy: true, ..Paket::default() }).unwrap().data,
            [1, 2, 3]
        );
        assert!(PrevodnikVidea::novy(&Stopa::nova(3, Kodek::Flac, zakladna_z_ns(1).unwrap())).is_err());
    }

    fn blok(vzorky: VzorkyZvuku, ramcov: usize) -> BlokZvuku {
        BlokZvuku { frekvencia: 48_000, rozlozenie: RozlozenieKanalov::STEREO, ramcov, vzorky, pts_ns: None }
    }

    fn s16(d: &[u8]) -> Vec<i16> {
        d.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()
    }
    fn s32(d: &[u8]) -> Vec<i32> {
        d.chunks_exact(4).map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
    }

    #[test]
    fn prekladanie_celych() {
        // planárne: ľavý [1, 2], pravý [-1, -2] → prekladané L R L R
        let b = blok(VzorkyZvuku::Cele { bitov: 16, data: vec![1, 2, -1, -2] }, 2);
        let mut v = Vec::new();
        prekladaj(&b, HlbkaPcm::S16, &mut v).unwrap();
        assert_eq!(s16(&v), [1, -1, 2, -2]);
        let mut v = Vec::new();
        prekladaj(&b, HlbkaPcm::S32, &mut v).unwrap();
        assert_eq!(s32(&v), [1 << 16, -1 << 16, 2 << 16, -2 << 16]);
        // 24 bitov na 16: zaokrúhlenie polovicou nahor a orezanie
        let b = blok(VzorkyZvuku::Cele { bitov: 24, data: vec![0x7FFFFF, 128, -129, -0x800000] }, 2);
        let mut v = Vec::new();
        prekladaj(&b, HlbkaPcm::S16, &mut v).unwrap();
        assert_eq!(s16(&v), [32767, -1, 1, -32768]);
        let mut v = Vec::new();
        prekladaj(&b, HlbkaPcm::S32, &mut v).unwrap();
        assert_eq!(s32(&v), [0x7FFFFF00, -129 << 8, 128 << 8, i32::MIN]);
        // zlý tvar je chyba
        assert!(
            prekladaj(&blok(VzorkyZvuku::Cele { bitov: 16, data: vec![1] }, 1), HlbkaPcm::S16, &mut v)
                .is_err()
        );
    }

    #[test]
    fn prekladanie_plavajucich() {
        let x = 0.5 / 32768.0;
        let b = blok(VzorkyZvuku::F32(vec![1.0, x, f32::NAN, -x, -1.0, -1.5, f32::INFINITY, 0.25]), 4);
        let mut v = Vec::new();
        prekladaj(&b, HlbkaPcm::S16, &mut v).unwrap();
        // ľavý [1, x, NaN, −x], pravý [−1, −1,5, ∞, 0,25]
        assert_eq!(s16(&v), [32767, -32768, 1, -32768, 0, 32767, 0, 8192]);
        let mut v = Vec::new();
        prekladaj(&b, HlbkaPcm::S32, &mut v).unwrap();
        assert_eq!(s32(&v), [i32::MAX, i32::MIN, 1 << 15, i32::MIN, 0, i32::MAX, -(1 << 15), 1 << 29]);
    }
}
