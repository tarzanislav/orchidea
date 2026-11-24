//! R193 – vlastný klient schránky firmvéru Raspberry Pi (`/dev/vcio`, náš kód bez knižníc). Grafický čip
//! VideoCore riadi takty a napätia sám (aj takt procesora), Linux ich nevidí. Cez schránku vlastností sa dá
//! spýtať na **skutočne namerané takty** (jadro VideoCore, 3D, H.264, ISP, pamäť, obraz), **napätia** jadra a
//! pamäte a na **škrtenie** (podpätie, tepelné obmedzenie) – merania, nie odhady. Zápis (zníženie taktov
//! nepoužívaných blokov) príde až podľa týchto meraní.

use std::fs;
use std::io;
use std::os::fd::AsRawFd;

use crate::sys;

/// `_IOWR(100, 0, char *)` – veľkosť ukazovateľa podľa architektúry.
const IOCTL_MBOX_PROPERTY: std::ffi::c_ulong =
    (3 << 30) | ((std::mem::size_of::<usize>() as std::ffi::c_ulong) << 16) | (100 << 8);
const ODPOVED_OK: u32 = 0x8000_0000;

const TAG_TAKT: u32 = 0x0003_0002;
const TAG_TAKT_NAMERANY: u32 = 0x0003_0047;
const TAG_NAPATIE: u32 = 0x0003_0003;
const TAG_SKRTENIE: u32 = 0x0003_0046;

/// Takty VideoCore (id podľa firmvéru) – (id, meno do záznamu).
pub const TAKTY: [(u32, &str); 8] = [
    (3, "ARM"),
    (4, "jadro VC"),
    (5, "3D"),
    (6, "H.264"),
    (7, "ISP"),
    (8, "pamäť"),
    (9, "obraz"),
    (11, "HEVC"),
];
/// Napätia – (id, meno).
pub const NAPATIA: [(u32, &str); 4] = [(1, "jadro"), (2, "pamäť C"), (3, "pamäť P"), (4, "pamäť I")];

/// Správa schránky: [veľkosť, požiadavka, značka, miesto na hodnoty, dĺžka požiadavky, hodnoty…, koniec].
fn sprava(tag: u32, poziadavka: &[u32], slov_odpovede: usize) -> Vec<u32> {
    let slov = poziadavka.len().max(slov_odpovede);
    let mut b = Vec::with_capacity(6 + slov);
    b.push(((6 + slov) * 4) as u32);
    b.push(0);
    b.push(tag);
    b.push((slov * 4) as u32);
    b.push((poziadavka.len() * 4) as u32);
    b.extend_from_slice(poziadavka);
    b.resize(5 + slov, 0);
    b.push(0);
    b
}

/// Hodnoty z odpovede, ak firmvér správu prijal a značku pozná.
fn odpoved(b: &[u32], slov: usize) -> Option<Vec<u32>> {
    (b.len() >= 5 + slov && b[1] == ODPOVED_OK && b[4] & ODPOVED_OK != 0).then(|| b[5..5 + slov].to_vec())
}

pub struct Schranka {
    subor: fs::File,
}

impl Schranka {
    pub fn otvor() -> io::Result<Schranka> {
        Ok(Schranka { subor: fs::OpenOptions::new().read(true).write(true).open("/dev/vcio")? })
    }

    fn vlastnost(&self, tag: u32, poziadavka: &[u32], slov: usize) -> Option<Vec<u32>> {
        let mut b = sprava(tag, poziadavka, slov);
        sys::ioctl_raz(self.subor.as_raw_fd(), IOCTL_MBOX_PROPERTY, &mut b[0]).ok()?;
        odpoved(&b, slov)
    }

    /// Takt v Hz – nameraný (novší firmvér), inak nastavený.
    pub fn takt(&self, id: u32) -> Option<u32> {
        self.vlastnost(TAG_TAKT_NAMERANY, &[id, 0], 2)
            .filter(|v| v[0] == id && v[1] > 0)
            .or_else(|| self.vlastnost(TAG_TAKT, &[id, 0], 2).filter(|v| v[0] == id))
            .map(|v| v[1])
    }

    /// Napätie v µV.
    pub fn napatie(&self, id: u32) -> Option<u32> {
        self.vlastnost(TAG_NAPATIE, &[id, 0], 2).filter(|v| v[0] == id && v[1] > 0).map(|v| v[1])
    }

    /// Príznaky škrtenia (bit 0 podpätie teraz, 1 strop taktu, 2 škrtenie, 3 mäkký tepelný limit; 16–19 odvtedy).
    pub fn skrtenie(&self) -> Option<u32> {
        self.vlastnost(TAG_SKRTENIE, &[0xffff], 1).map(|v| v[0])
    }

    /// Riadok do záznamu: takty, napätia, škrtenie.
    pub fn popis(&self) -> String {
        popis(
            &TAKTY.iter().map(|&(id, m)| (m, self.takt(id))).collect::<Vec<_>>(),
            &NAPATIA.iter().map(|&(id, m)| (m, self.napatie(id))).collect::<Vec<_>>(),
            self.skrtenie(),
        )
    }
}

/// Text riadka (čistá funkcia kvôli skúškam).
pub fn popis(
    takty: &[(&str, Option<u32>)],
    napatia: &[(&str, Option<u32>)],
    skrtenie: Option<u32>,
) -> String {
    let t: Vec<String> = takty
        .iter()
        .filter_map(|(m, hz)| hz.map(|hz| format!("{m} {} MHz", (hz + 500_000) / 1_000_000)))
        .collect();
    let n: Vec<String> = napatia
        .iter()
        .filter_map(|(m, uv)| uv.map(|uv| format!("{m} {:.4} V", uv as f64 / 1e6).replace('.', ",")))
        .collect();
    let s = match skrtenie {
        Some(0) => "bez škrtenia".to_string(),
        Some(x) => format!("škrtenie 0x{x:05x}"),
        None => "škrtenie ?".to_string(),
    };
    format!("takty: {} · napätia: {} · {s}", t.join(", "), n.join(", "))
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn sprava_a_odpoved() {
        let b = sprava(TAG_TAKT_NAMERANY, &[4, 0], 2);
        assert_eq!(b, vec![32, 0, 0x0003_0047, 8, 8, 4, 0, 0]);
        let mut o = b.clone();
        o[1] = ODPOVED_OK;
        o[4] = ODPOVED_OK | 8;
        o[6] = 250_000_000;
        assert_eq!(odpoved(&o, 2), Some(vec![4, 250_000_000]));
        // firmvér značku nepozná – bit odpovede chýba
        let mut z = b.clone();
        z[1] = ODPOVED_OK;
        assert_eq!(odpoved(&z, 2), None);
        // škrtenie: požiadavka 1 slovo
        assert_eq!(sprava(TAG_SKRTENIE, &[0xffff], 1), vec![28, 0, 0x0003_0046, 4, 4, 0xffff, 0]);
        if std::mem::size_of::<usize>() == 8 {
            assert_eq!(IOCTL_MBOX_PROPERTY, 0xC008_6400);
        }
    }

    #[test]
    fn riadok() {
        assert_eq!(
            popis(
                &[("ARM", Some(600_000_000)), ("3D", None), ("jadro VC", Some(250_000_123))],
                &[("jadro", Some(1_200_000))],
                Some(0)
            ),
            "takty: ARM 600 MHz, jadro VC 250 MHz · napätia: jadro 1,2000 V · bez škrtenia"
        );
        assert!(popis(&[], &[], Some(0x50005)).ends_with("škrtenie 0x50005"));
    }
}
