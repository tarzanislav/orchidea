use super::*;

#[test]
fn crc_a_tabulky() {
    // mnohočlen x^16 + x^15 + x^2 + 1 (0x8005), register od nuly, bez zrkadlenia: kontrolná hodnota „123456789“
    // pre tieto parametre (katalóg CRC: CRC-16/UMTS) je 0xFEE8
    assert_eq!(crc16(b"123456789"), 0xFEE8);
    // správa nasledovaná vlastným CRC dá nulu (tak sa kontroluje crc2)
    let mut d = b"AC-3 ramec".to_vec();
    let c = crc16(&d);
    d.extend(c.to_be_bytes());
    assert_eq!(crc16(&d), 0);
    // tabuľka 5.18: 48 kHz, 192 kb/s = 384 slov; 44,1 kHz nepárny kód o slovo viac
    assert_eq!(VELKOST_RAMCA[20][0], 384);
    assert_eq!(VELKOST_RAMCA[21][1], VELKOST_RAMCA[20][1] + 1);
    assert_eq!(FLOORTAB[7], -2048);
}

#[test]
fn hlavicka_a_poradie() {
    // fscod 0, frmsizecod 0x14 (192 kb/s), bsid 8, bsmod 0, acmod 7, cmixlev, surmixlev, lfeon (najnižší bit)
    let h = Hlavicka::z_bajtov(&[0x0B, 0x77, 0, 0, 0x14, 0x40, 0xE0, 0x00]).expect("hlavička");
    assert_eq!((h.frekvencia, h.dlzka, h.acmod, h.lfeon), (48_000, 768, 7, false));
    let h = Hlavicka::z_bajtov(&[0x0B, 0x77, 0, 0, 0x14, 0x40, 0xE1, 0x00]).expect("hlavička");
    assert!(h.lfeon && h.kanalov() == 6);
    assert_eq!(h.rozlozenie(), RozlozenieKanalov::S5_1);
    // L C R SL SR LFE → FL FR FC LFE SL SR
    assert_eq!(h.poradie_vystupu(), vec![0, 2, 1, 5, 3, 4]);
    // E-AC-3 (bsid 16) nie je AC-3
    assert!(Hlavicka::z_bajtov(&[0x0B, 0x77, 0, 0, 0x14, 0x80, 0, 0]).is_none());
}

#[test]
fn dynrng_a_mocniny() {
    assert_eq!(dynrng_zisk(0), 1.0);
    // 0x7F: X = 3, Y = 31 → 2^4 · 63/64 (+23,95 dB)
    assert_eq!(dynrng_zisk(0x7F), 16.0 * 63.0 / 64.0);
    // 0x80: X = −4, Y = 0 → 2^−3 · 1/2 (−24,08 dB)
    assert_eq!(dynrng_zisk(0x80), 1.0 / 16.0);
    assert_eq!(mocnina2(-24), 1.0 / 16_777_216.0);
    assert_eq!(mocnina2(3), 8.0);
}

#[test]
fn rozdelenie_bitov_nulova_maska() {
    // pri psd = 3072 (exponent 0) a nulovom maskovaní dá adresa 63 → bap 15
    let mut s = StavRamca::novy();
    s.rozdelenie =
        ParametreRozdelenia { sdcycod: 2, fdcycod: 1, sgaincod: 1, dbpbcod: 2, floorcod: 7, csnroffst: 63 };
    s.fsnroffst[0] = 15;
    s.fgaincod[0] = 4;
    let h = Hlavicka::z_bajtov(&[0x0B, 0x77, 0, 0, 0x14, 0x40, 0x40, 0x00]).expect("hlavička");
    let mut bap = [0u8; 256];
    rozdel_bity(&s, &h, 0, 0, 253, &mut bap).expect("rozdelenie");
    assert!(bap[..253].iter().all(|&b| b > 0));
    // exponent 24 (ticho) dostane menej bitov ako exponent 0
    s.exp[0] = [24; 256];
    let mut bap2 = [0u8; 256];
    rozdel_bity(&s, &h, 0, 0, 253, &mut bap2).expect("rozdelenie");
    assert!(bap2[..253].iter().zip(&bap[..253]).all(|(a, b)| a <= b));
}
