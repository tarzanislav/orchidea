//! **Vrstvy I a II** MPEG audio (MPEG-1 a MPEG-2 LSF) – etapa L8.
//!
//! Zdroje: syntax CD 11172-3 2.4.1.5 (vrstva I) a 2.4.1.6 (vrstva II), postup dekódovania 2.4.3.2 a 2.4.3.3
//! (rozdelenie bitov v troch krokoch, scfsi, škálovacie faktory, rozbalenie skupín `s[i] = c mod nlevels`,
//! obrátenie prvého bitu, `s'' = C·(s''' + D)`, `s' = mierka · s''`), tabuľky ETSI TS 103 466 (`tabulky_l2`).
//! Polyfázová syntéza a okno D[i] sú spoločné s vrstvou III (`synteza`).
//!
//! **Odvodené z textu:** konštanty C a D (tabuľka 3-B.4 v dodaných dokumentoch nie je). ETSI §5.2.8 opisuje
//! kvantovanie ako `A·X + B`, n najvyšších bitov, obrátený prvý bit; spätne je to stred kvantizačného kroku:
//! `C = 1/A = 2^n/kroky`, `D = 2^−n − B`. Pre vrstvu I to dáva presne vzorec CD 2.4.3.2 (D = 2^(1−nb)).
//!
//! **Hypotézy (text ich nemá) – dokázané meraním:** tabuľky 3-B.2b (44,1 a 32 kHz, vyššie toky, sblimit 30)
//! a 3-B.2d (32 kHz, nízke toky, sblimit 12) ako rozšírenie 3-B.2a a 3-B.2c o pásma s rovnakým rozdelením ako
//! posledná skupina; výber tabuľky pri 44,1 a 32 kHz podľa toku na kanál (do 48 kb/s nízke, 56–80 kb/s 3-B.2a,
//! od 96 kb/s 3-B.2b). Rozhodca: kodéry `mp2` a twolame pri 44,1/32 kHz – plná presnosť 11172-4 (RMS ≈ 1,5·10⁻⁷);
//! bez rozšírenia tabuliek RMS 0,24–0,66 alebo rámec nejde rozobrať – skúška hypotézy rozlišuje.

use super::synteza::StavSyntezy;
use super::{Bity, Hlavicka, tabulky_l2};
use orchidea_jadro::{Chyba, Vysledok};
use std::sync::OnceLock;

/// Riadok tabuľky rozdelenia bitov: (nbal, počty krokov podľa indexu 1..).
type Riadok = (u8, &'static [u16]);
type Rozdelenie = [Riadok];

/// Tabuľky 3-B.2b a 3-B.2d (hypotéza – pozri hlavičku modulu; dokázané meraním).
fn odvodene() -> &'static (Vec<Riadok>, Vec<Riadok>) {
    static T: OnceLock<(Vec<Riadok>, Vec<Riadok>)> = OnceLock::new();
    T.get_or_init(|| {
        let a = &tabulky_l2::ROZDELENIE_A;
        let c = &tabulky_l2::ROZDELENIE_C;
        let b: Vec<_> = a.iter().copied().chain(std::iter::repeat_n(a[26], 3)).collect();
        let d: Vec<_> = c.iter().copied().chain(std::iter::repeat_n(c[7], 4)).collect();
        (b, d)
    })
}

/// Tabuľka rozdelenia bitov vrstvy II podľa verzie, frekvencie a toku na kanál (0 = voľný formát).
pub(crate) fn rozdelenie(h: &Hlavicka, tok_na_kanal: u32) -> &'static Rozdelenie {
    if h.verzia != 1 {
        return &tabulky_l2::ROZDELENIE_LSF;
    }
    let (b, d) = odvodene();
    let nizky = tok_na_kanal != 0 && tok_na_kanal <= 48_000;
    let stredny = tok_na_kanal != 0 && tok_na_kanal <= 80_000;
    match h.frekvencia {
        48_000 if nizky => &tabulky_l2::ROZDELENIE_C,
        48_000 => &tabulky_l2::ROZDELENIE_A,
        44_100 if nizky => &tabulky_l2::ROZDELENIE_C,
        32_000 if nizky => d,
        _ if stredny => &tabulky_l2::ROZDELENIE_A,
        _ => b,
    }
}

/// Spätná kvantizácia kódu `c` s `n` bitmi a `kroky` úrovňami: obrátený prvý bit, dvojkový doplnok ako zlomok
/// (s''' ∈ [−1, 1)), `s'' = C·(s''' + D)` s `C = 2^n/kroky`, `D = 2^−n + 1 − kroky/2^n`.
pub(crate) fn rekvantuj(c: u32, n: u8, kroky: u32) -> f32 {
    let polovica = 1u32 << (n - 1);
    let obrateny = c ^ polovica;
    let cele = if obrateny >= polovica { obrateny as i64 - 2 * polovica as i64 } else { obrateny as i64 };
    let s3 = cele as f64 / polovica as f64;
    let dva_n = (2 * polovica) as f64;
    let c_ = dva_n / kroky as f64;
    let d = 1.0 / dva_n + 1.0 - kroky as f64 / dva_n;
    (c_ * (s3 + d)) as f32
}

fn mierka(index: u32) -> f32 {
    // index 63 nie je v tabuľke 1 (6 bitov, 63 mierok) – neplatný, stlmí pásmo
    tabulky_l2::MIERKY.get(index as usize).map_or(0.0, |&m| m as f32)
}

/// Trieda kvantovania podľa počtu krokov: (zoskupenie, bitov kódového slova, bitov vzorky).
fn trieda(kroky: u16) -> Option<(bool, u8, u8)> {
    tabulky_l2::TRIEDY.iter().find(|t| t.0 == kroky).map(|&(_, z, b, n)| (z, b, n))
}

/// Dekóduje rámec vrstvy I alebo II do `von[kanál]` (384 alebo 1152 vzoriek ±1,0).
pub(crate) fn dekoduj(
    h: &Hlavicka,
    ramec: &[u8],
    stav: &mut [StavSyntezy; 2],
    von: &mut [Vec<f32>],
) -> Vysledok<()> {
    let nch = h.kanalov();
    let zac = 4 + if h.crc { 2 } else { 0 };
    let telo = ramec.get(zac..).ok_or_else(|| Chyba::PoskodeneData("MPEG audio: krátky rámec".into()))?;
    let koniec = telo.len() * 8;
    let mut b = Bity::nove(telo);
    let intenzita = h.rezim == 1;
    // pásma[t][ch][sb] po spätnej kvantizácii a mierke
    let casov = if h.vrstva == 1 { 12 } else { 36 };
    let mut pasma = vec![[[0f32; 32]; 2]; casov];
    if h.vrstva == 1 {
        let hranica =
            if intenzita { tabulky_l2::HRANICA_INTENZITY[h.rozsirenie as usize] as usize } else { 32 };
        let mut alok = [[0u8; 32]; 2];
        for sb in 0..32 {
            for ch in 0..if sb < hranica { nch } else { 1 } {
                alok[ch][sb] = b.citaj(4) as u8;
            }
            if sb >= hranica {
                alok[1][sb] = alok[0][sb];
            }
        }
        if alok.iter().flatten().any(|&a| a == 15) {
            return Err(Chyba::PoskodeneData("MPEG audio vrstva I: neplatné rozdelenie bitov 15".into()));
        }
        let mut sf = [[0f32; 32]; 2];
        for sb in 0..32 {
            for ch in 0..nch {
                if alok[ch][sb] != 0 {
                    sf[ch][sb] = mierka(b.citaj(6));
                }
            }
        }
        for t in 0..12 {
            for sb in 0..32 {
                let spolocne = sb >= hranica;
                for ch in 0..if spolocne { 1 } else { nch } {
                    let a = alok[ch][sb];
                    if a == 0 {
                        continue;
                    }
                    let nb = a + 1;
                    let v = rekvantuj(b.citaj(nb as u32), nb, (1u32 << nb) - 1);
                    for k in if spolocne { 0..nch } else { ch..ch + 1 } {
                        pasma[t][k][sb] = v * sf[k][sb];
                    }
                }
            }
        }
    } else {
        let tok = crate::ramce::mpeg_audio_tok(ramec).unwrap_or(0);
        let tabulka = rozdelenie(h, tok / nch as u32);
        let sblimit = tabulka.len();
        let hranica = if intenzita {
            (tabulky_l2::HRANICA_INTENZITY[h.rozsirenie as usize] as usize).min(sblimit)
        } else {
            sblimit
        };
        let mut kroky = [[0u16; 32]; 2];
        for sb in 0..sblimit {
            let (nbal, k) = tabulka[sb];
            for ch in 0..if sb < hranica { nch } else { 1 } {
                let index = b.citaj(nbal as u32) as usize;
                kroky[ch][sb] = if index == 0 { 0 } else { k.get(index - 1).copied().unwrap_or(0) };
            }
            if sb >= hranica {
                kroky[1][sb] = kroky[0][sb];
            }
        }
        let mut scfsi = [[0u8; 32]; 2];
        for sb in 0..sblimit {
            for ch in 0..nch {
                if kroky[ch][sb] != 0 {
                    scfsi[ch][sb] = b.citaj(2) as u8;
                }
            }
        }
        let mut sf = [[[0f32; 3]; 32]; 2];
        for sb in 0..sblimit {
            for ch in 0..nch {
                if kroky[ch][sb] == 0 {
                    continue;
                }
                let s = &mut sf[ch][sb];
                match scfsi[ch][sb] {
                    0 => *s = [mierka(b.citaj(6)), mierka(b.citaj(6)), mierka(b.citaj(6))],
                    1 => {
                        let (x, z) = (mierka(b.citaj(6)), mierka(b.citaj(6)));
                        *s = [x, x, z];
                    }
                    2 => *s = [mierka(b.citaj(6)); 3],
                    _ => {
                        let (x, z) = (mierka(b.citaj(6)), mierka(b.citaj(6)));
                        *s = [x, z, z];
                    }
                }
            }
        }
        for gr in 0..12 {
            for sb in 0..sblimit {
                let spolocne = sb >= hranica;
                for ch in 0..if spolocne { 1 } else { nch } {
                    let k = kroky[ch][sb];
                    if k == 0 {
                        continue;
                    }
                    let (zoskupenie, bitov, n) = trieda(k)
                        .ok_or_else(|| Chyba::PoskodeneData("MPEG audio vrstva II: trieda".into()))?;
                    let mut kody = [0u32; 3];
                    if zoskupenie {
                        let mut c = b.citaj(bitov as u32);
                        for kod in &mut kody {
                            *kod = c % k as u32;
                            c /= k as u32;
                        }
                    } else {
                        for kod in &mut kody {
                            *kod = b.citaj(n as u32);
                        }
                    }
                    for (s, &kod) in kody.iter().enumerate() {
                        let v = rekvantuj(kod, n, k as u32);
                        for c in if spolocne { 0..nch } else { ch..ch + 1 } {
                            pasma[3 * gr + s][c][sb] = v * sf[c][sb][gr / 4];
                        }
                    }
                }
            }
        }
    }
    if b.poloha > koniec {
        return Err(Chyba::PoskodeneData(
            "MPEG audio vrstva I/II: rámec je kratší, než žiada rozdelenie bitov".into(),
        ));
    }
    for ch in 0..nch {
        von[ch].resize(32 * casov, 0.0);
        for t in 0..casov {
            stav[ch].polyfaza(&pasma[t][ch], &mut von[ch][32 * t..32 * t + 32]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod skusky {
    use super::*;

    #[test]
    fn spatna_kvantizacia_je_stred_kroku() {
        // s'' = (2c + 1 − kroky)/kroky pre všetky triedy aj vrstvu I
        for &(k, _, _, n) in &tabulky_l2::TRIEDY {
            for c in [0u32, 1, (k as u32 - 1) / 2, k as u32 - 1] {
                let ocak = (2.0 * c as f64 + 1.0 - k as f64) / k as f64;
                assert!((rekvantuj(c, n, k as u32) as f64 - ocak).abs() < 1e-6, "{k} {c}");
            }
        }
        // 3 kroky: −2/3, 0, 2/3
        assert!((rekvantuj(0, 2, 3) + 2.0 / 3.0).abs() < 1e-7 && rekvantuj(1, 2, 3) == 0.0);
    }

    #[test]
    fn odvodene_tabulky() {
        let (b, d) = odvodene();
        assert_eq!((b.len(), d.len()), (30, 12));
        assert_eq!(b[29], (2, &[3u16, 5, 65535][..]));
        assert_eq!(d[11], (3, &[3u16, 5, 9, 15, 31, 63, 127][..]));
        // súčty nbal: 3-B.2b mono 88 + 3·2 = 94, 3-B.2d mono 26 + 4·3 = 38
        assert_eq!(b.iter().map(|x| x.0 as u32).sum::<u32>(), 94);
        assert_eq!(d.iter().map(|x| x.0 as u32).sum::<u32>(), 38);
    }
}
