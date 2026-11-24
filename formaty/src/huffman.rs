//! Huffmanove kódy s obmedzenou dĺžkou – spoločné pre kodér DEFLATE (PNG, TIFF, OpenEXR) a kodér JPEG.
//!
//! Dĺžky sa najprv spočítajú klasicky (dve fronty nad zoradenými početnosťami – lineárny čas po zoradení).
//! Kódy dlhšie ako dovolené sa skrátia na limit a Kraftova nerovnosť sa potom vyrovná dvoma prechodmi:
//! najprv sa predĺžia najhlbšie kódy (najmenšia strata), kým súčet neklesne pod 1, potom sa skrátia kódy
//! najčastejších znakov, kým nie je strom úplný. Úplný strom je podmienka dekodérov (zlib odmietne neúplný).

/// Dĺžky kódov pre početnosti `pocty` (0 = znak sa nepoužije) s najväčšou dĺžkou `limit` (1–24).
///
/// Jeden použitý znak dostane dĺžku 1 (jediný kód je dovolený neúplný). Pri dvoch a viac znakoch je výsledný
/// kód vždy úplný (Kraftov súčet presne 1).
pub fn dlzky_kodov(pocty: &[u32], limit: u8) -> Vec<u8> {
    let n = pocty.len();
    let mut dlzky = vec![0u8; n];
    let mut pouzite: Vec<usize> = (0..n).filter(|&i| pocty[i] > 0).collect();
    match pouzite.len() {
        0 => return dlzky,
        1 => {
            dlzky[pouzite[0]] = 1;
            return dlzky;
        }
        _ => {}
    }
    assert!((1..=24).contains(&limit) && (1usize << limit) >= pouzite.len(), "limit dĺžky kódu je malý");
    // zoradenie podľa početnosti, pri zhode podľa znaku – výsledok je rovnaký na každej platforme
    pouzite.sort_by_key(|&i| (pocty[i], i));
    let m = pouzite.len();
    // uzly 0..m sú listy, m.. vnútorné; rodic[] a váhy
    let mut vaha: Vec<u64> = pouzite.iter().map(|&i| pocty[i] as u64).collect();
    vaha.reserve(m);
    let mut rodic = vec![usize::MAX; 2 * m - 1];
    let (mut list, mut vnutorny) = (0usize, m);
    // menší z dvoch čiel front: listy (zoradené) a vnútorné uzly (vznikajú v neklesajúcom poradí)
    fn vyber(vaha: &[u64], m: usize, list: &mut usize, vnutorny: &mut usize) -> usize {
        if *list < m && (*vnutorny >= vaha.len() || vaha[*list] <= vaha[*vnutorny]) {
            *list += 1;
            *list - 1
        } else {
            *vnutorny += 1;
            *vnutorny - 1
        }
    }
    for _ in 0..m - 1 {
        let a = vyber(&vaha, m, &mut list, &mut vnutorny);
        let b = vyber(&vaha, m, &mut list, &mut vnutorny);
        let novy = vaha.len();
        vaha.push(vaha[a] + vaha[b]);
        rodic[a] = novy;
        rodic[b] = novy;
    }
    // hĺbky od koreňa: vnútorné uzly vznikali postupne, rodič má vždy vyšší index
    let mut hlbka = vec![0u32; 2 * m - 1];
    for u in (0..2 * m - 2).rev() {
        hlbka[u] = hlbka[rodic[u]] + 1;
    }
    let mut d: Vec<u32> = (0..m).map(|j| hlbka[j]).collect();
    obmedz(&mut d, limit as u32);
    for (j, &i) in pouzite.iter().enumerate() {
        dlzky[i] = d[j] as u8;
    }
    dlzky
}

/// Vyrovná dĺžky `d` (listy zoradené vzostupne podľa početnosti) na najviac `limit` a úplný kód.
fn obmedz(d: &mut [u32], limit: u32) {
    if d.iter().all(|&x| x <= limit) {
        return;
    }
    let ciel: u64 = 1 << limit;
    for x in d.iter_mut() {
        *x = (*x).min(limit);
    }
    let mut sucet: u64 = d.iter().map(|&x| 1u64 << (limit - x)).sum();
    // 1. prečerpané: predĺž najhlbší kód kratší ako limit (najmenší úbytok), pri zhode najmenej častý
    while sucet > ciel {
        let mut najlepsi = None;
        for (j, &x) in d.iter().enumerate() {
            if x < limit && najlepsi.is_none_or(|k: usize| x > d[k]) {
                najlepsi = Some(j);
            }
        }
        let j = najlepsi.expect("vždy existuje kód kratší ako limit");
        sucet -= 1 << (limit - d[j] - 1);
        d[j] += 1;
    }
    // 2. neúplné: skráť kód najčastejšieho znaku, ktorého prírastok sa ešte zmestí
    while sucet < ciel {
        let zvysok = ciel - sucet;
        let mut najlepsi = None;
        for j in (0..d.len()).rev() {
            if d[j] > 1 && (1u64 << (limit - d[j])) <= zvysok {
                najlepsi = Some(j);
                break;
            }
        }
        let j = najlepsi.expect("neúplný kód sa dá vždy doplniť");
        sucet += 1 << (limit - d[j]);
        d[j] -= 1;
    }
}

/// Kanonické kódy z dĺžok (norma DEFLATE aj JPEG): kratšie kódy skôr, v rámci dĺžky podľa poradia znaku.
/// Vráti kód každého znaku (bity od najvyššieho, platí len `dlzky[i]` najnižších bitov).
pub fn kanonicke_kody(dlzky: &[u8]) -> Vec<u32> {
    let mut pocet = [0u32; 33];
    for &l in dlzky {
        pocet[l as usize] += 1;
    }
    pocet[0] = 0;
    let mut dalsi = [0u32; 33];
    let mut kod = 0u32;
    for l in 1..33 {
        kod = (kod + pocet[l - 1]) << 1;
        dalsi[l] = kod;
    }
    dlzky
        .iter()
        .map(|&l| {
            if l == 0 {
                0
            } else {
                let k = dalsi[l as usize];
                dalsi[l as usize] += 1;
                k
            }
        })
        .collect()
}

/// Obráti poradie najnižších `n` bitov (DEFLATE posiela kódy od najnižšieho bitu).
pub fn obrat_bity(mut x: u32, n: u32) -> u32 {
    let mut r = 0;
    for _ in 0..n {
        r = (r << 1) | (x & 1);
        x >>= 1;
    }
    r
}

#[cfg(test)]
mod testy {
    use super::*;
    use orchidea_jadro::nahoda::Nahoda;

    fn kraft(d: &[u8]) -> f64 {
        d.iter().filter(|&&x| x > 0).map(|&x| 0.5f64.powi(x as i32)).sum()
    }

    #[test]
    fn klasicky_huffman() {
        // a:45 b:13 c:12 d:16 e:9 f:5 – učebnicový príklad, cena 224
        let p = [45, 13, 12, 16, 9, 5];
        let d = dlzky_kodov(&p, 15);
        let cena: u32 = p.iter().zip(&d).map(|(&a, &b)| a * b as u32).sum();
        assert_eq!(cena, 224);
        assert_eq!(kraft(&d), 1.0);
    }

    #[test]
    fn obmedzenie_dlzky_dava_uplny_kod() {
        let mut n = Nahoda::zo_semienka(7);
        for pokus in 0..300 {
            let pocet = 2 + n.mensie_ako(300) as usize;
            // Fibonacciho početnosti vytvoria veľmi hlboký strom
            let p: Vec<u32> = if pokus % 3 == 0 {
                let mut f = vec![1u32, 1];
                while f.len() < pocet.min(40) {
                    let k = f.len();
                    f.push(f[k - 1].saturating_add(f[k - 2]));
                }
                f
            } else {
                (0..pocet)
                    .map(|_| if n.mensie_ako(4) == 0 { 0 } else { 1 + n.mensie_ako(100_000) as u32 })
                    .collect()
            };
            if p.iter().filter(|&&x| x > 0).count() < 2 {
                continue;
            }
            for limit in [7u8, 9, 15, 16] {
                if (1usize << limit) < p.len() {
                    continue;
                }
                let d = dlzky_kodov(&p, limit);
                assert!(d.iter().all(|&x| x <= limit));
                assert_eq!(kraft(&d), 1.0, "pokus {pokus}, limit {limit}");
                for (i, &x) in p.iter().enumerate() {
                    assert_eq!(x > 0, d[i] > 0);
                }
            }
        }
    }

    #[test]
    fn kanonicke_kody_z_normy() {
        // príklad z RFC 1951, 3.2.2: dĺžky (3,3,3,3,3,2,4,4) → 010 011 100 101 110 00 1110 1111
        let k = kanonicke_kody(&[3, 3, 3, 3, 3, 2, 4, 4]);
        assert_eq!(k, vec![0b010, 0b011, 0b100, 0b101, 0b110, 0b00, 0b1110, 0b1111]);
        assert_eq!(obrat_bity(0b1101, 4), 0b1011);
    }
}
