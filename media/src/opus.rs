//! **Opus** – zatiaľ len rozbor balenia paketu (RFC 6716, kap. 3.1–3.2): trvanie paketu z bajtu TOC. Celý
//! dekodér pribudne v etape L7 (R302 – podľa textu RFC 6716 a 8251).

/// Trvanie jedného rámca podľa konfigurácie v TOC (stotiny ms → ns): SILK 10/20/40/60 ms, hybrid 10/20 ms,
/// CELT 2,5/5/10/20 ms (RFC 6716, tabuľka 2).
fn trvanie_ramca_ns(konfiguracia: u8) -> u64 {
    match konfiguracia {
        0..=11 => [10_000_000, 20_000_000, 40_000_000, 60_000_000][(konfiguracia % 4) as usize],
        12..=15 => [10_000_000, 20_000_000][(konfiguracia % 2) as usize],
        _ => [2_500_000, 5_000_000, 10_000_000, 20_000_000][(konfiguracia % 4) as usize],
    }
}

/// Trvanie paketu Opus v ns z TOC a počtu rámcov (kód 0: 1 rámec, 1 a 2: dva, 3: počet v druhom bajte).
/// `None` pri prázdnom alebo neplatnom pakete (paket dlhší ako 120 ms je podľa RFC 6716 neplatný).
pub fn trvanie_paketu_ns(paket: &[u8]) -> Option<u64> {
    let toc = *paket.first()?;
    let ramcov = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => (*paket.get(1)? & 0x3F) as u64,
    };
    let t = ramcov * trvanie_ramca_ns(toc >> 3);
    (ramcov > 0 && t <= 120_000_000).then_some(t)
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn trvanie_z_toc() {
        assert_eq!(trvanie_paketu_ns(&[12 << 3]), Some(10_000_000)); // hybrid 10 ms
        assert_eq!(trvanie_paketu_ns(&[15 << 3]), Some(20_000_000));
        assert_eq!(trvanie_paketu_ns(&[(31 << 3) | 1]), Some(40_000_000)); // CELT 20 ms × 2
        assert_eq!(trvanie_paketu_ns(&[(16 << 3) | 3, 0x05]), Some(12_500_000)); // CELT 2,5 ms × 5
        assert_eq!(trvanie_paketu_ns(&[(3 << 3) | 3, 0x03]), None); // 3 × 60 ms > 120 ms
        assert_eq!(trvanie_paketu_ns(&[(3 << 3) | 3, 0x00]), None);
        assert_eq!(trvanie_paketu_ns(&[(3 << 3) | 3]), None);
        assert_eq!(trvanie_paketu_ns(&[]), None);
    }
}
