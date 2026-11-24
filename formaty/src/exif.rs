//! Exif – len to, čo potrebuje zobrazenie: natočenie (značka 0x0112) z prvého adresára (IFD0).
//!
//! Exif je štruktúra TIFF: poradie bajtov „II“ alebo „MM“, číslo 42, posun prvého adresára a položky po 12
//! bajtoch. Čítanie je ohraničené dĺžkou údajov, poškodený Exif vráti `None`.

fn u16_v(d: &[u8], i: usize, le: bool) -> Option<u16> {
    let b = d.get(i..i + 2)?;
    Some(if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) })
}

fn u32_v(d: &[u8], i: usize, le: bool) -> Option<u32> {
    let b = d.get(i..i + 4)?;
    let p = [b[0], b[1], b[2], b[3]];
    Some(if le { u32::from_le_bytes(p) } else { u32::from_be_bytes(p) })
}

/// Natočenie 1–8 podľa Exif (1 = bez zmeny), ak je v údajoch.
pub fn natocenie(exif: &[u8]) -> Option<u8> {
    // niektoré programy nechajú na začiatku hlavičku „Exif\0\0“
    let d = exif.strip_prefix(b"Exif\0\0").unwrap_or(exif);
    let le = match d.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    if u16_v(d, 2, le)? != 42 {
        return None;
    }
    let ifd = u32_v(d, 4, le)? as usize;
    let pocet = u16_v(d, ifd, le)? as usize;
    for k in 0..pocet.min(1000) {
        let e = ifd + 2 + 12 * k;
        if u16_v(d, e, le)? == 0x0112 {
            let v = u16_v(d, e + 8, le)?;
            return (1..=8).contains(&v).then_some(v as u8);
        }
    }
    None
}

/// Exif s jedinou položkou natočenia (pre zápis a skúšky).
pub fn s_natocenim(n: u8) -> Vec<u8> {
    let mut d = b"II\x2A\x00\x08\x00\x00\x00\x01\x00".to_vec();
    d.extend_from_slice(&0x0112u16.to_le_bytes());
    d.extend_from_slice(&3u16.to_le_bytes());
    d.extend_from_slice(&1u32.to_le_bytes());
    d.extend_from_slice(&(n as u16).to_le_bytes());
    d.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    d
}

#[cfg(test)]
mod testy {
    use super::*;

    #[test]
    fn natocenie_tam_a_spat() {
        for n in 1..=8 {
            assert_eq!(natocenie(&s_natocenim(n)), Some(n));
        }
        let mut s = b"Exif\0\0".to_vec();
        s.extend_from_slice(&s_natocenim(6));
        assert_eq!(natocenie(&s), Some(6));
        assert_eq!(natocenie(b"MM\x00\x2A\x00\x00\x00\x08\xFF\xFF"), None);
        assert_eq!(natocenie(b"xx"), None);
    }
}
