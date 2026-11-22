//! PNG – uloženie plátna do obrázka PNG vlastným kódom, bez cudzích knižníc (R122).
//!
//! Obrázok je RGB s 8 bitmi na farbu. Každý riadok dostane filter (žiadny, „Sub“ alebo „Up“ – vyberie sa
//! ten s najmenším súčtom, bežný odhad z normy PNG). Údaje sa stlačia metódou DEFLATE (RFC 1951) s pevnými
//! Huffmanovými kódmi; opakovania (LZ77) sa hľadajú cez rozptylovú tabuľku s reťazou predchodcov.
//! Obrazovka SKLENÍKA – plochy jednej farby a text – sa tak z niekoľkých MB stlačí na desiatky kB.
//!
//! Norma: PNG (ISO/IEC 15948), zlib (RFC 1950), DEFLATE (RFC 1951). Celé je to pôvodný kód projektu.

use crate::Platno;

const PODPIS: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Tabuľka CRC-32 (mnohočlen 0xEDB88320), spočítaná pri preklade.
const CRC: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[n] = c;
        n += 1;
    }
    t
};

/// Kontrolný súčet CRC-32 (ten istý ako v PNG, ZIP a Ethernete).
pub fn crc32(data: &[u8]) -> u32 {
    crc32_pokracuj(0xFFFF_FFFF, data) ^ 0xFFFF_FFFF
}

fn crc32_pokracuj(mut c: u32, data: &[u8]) -> u32 {
    for &b in data {
        c = CRC[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c
}

/// Kontrolný súčet Adler-32 (koniec údajov zlib).
pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    // po 5552 bajtoch sa súčty ešte nepretečú – modulo stačí raz za kus
    for kus in data.chunks(5552) {
        for &x in kus {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

/// Zápis bitov od najnižšieho (poradie DEFLATE).
struct Bity {
    von: Vec<u8>,
    akum: u64,
    pocet: u32,
}

impl Bity {
    fn nove(kapacita: usize) -> Bity {
        Bity { von: Vec::with_capacity(kapacita), akum: 0, pocet: 0 }
    }

    /// Zapíše `bitov` najnižších bitov hodnoty (najnižší prvý) – hlavičky a doplnkové bity.
    fn zapis(&mut self, hodnota: u32, bitov: u32) {
        self.akum |= (hodnota as u64) << self.pocet;
        self.pocet += bitov;
        while self.pocet >= 8 {
            self.von.push(self.akum as u8);
            self.akum >>= 8;
            self.pocet -= 8;
        }
    }

    /// Zapíše Huffmanov kód – ten sa podľa normy zapisuje od najvyššieho bitu.
    fn kod(&mut self, kod: u32, bitov: u32) {
        self.zapis(kod.reverse_bits() >> (32 - bitov), bitov);
    }

    fn dokonci(mut self) -> Vec<u8> {
        if self.pocet > 0 {
            self.von.push(self.akum as u8);
        }
        self.von
    }
}

/// Pevné Huffmanove kódy DEFLATE pre znak 0–287 (literál, koniec bloku, dĺžka).
fn znak(b: &mut Bity, z: u32) {
    match z {
        0..=143 => b.kod(0x30 + z, 8),
        144..=255 => b.kod(0x190 + (z - 144), 9),
        256..=279 => b.kod(z - 256, 7),
        _ => b.kod(0xC0 + (z - 280), 8),
    }
}

const ZAKLAD_DLZKY: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195,
    227, 258,
];
const BITY_DLZKY: [u8; 29] =
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const ZAKLAD_VZDIALENOSTI: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073,
    4097, 6145, 8193, 12289, 16385, 24577,
];
const BITY_VZDIALENOSTI: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// Posledný index tabuľky so základom ≤ hodnota.
fn index_zakladu(tabulka: &[u16], hodnota: usize) -> usize {
    tabulka.partition_point(|&z| z as usize <= hodnota) - 1
}

/// Zapíše opakovanie: `dlzka` bajtov (3–258) spred `vzdialenost` bajtov (1–32768).
fn opakovanie(b: &mut Bity, dlzka: usize, vzdialenost: usize) {
    let i = index_zakladu(&ZAKLAD_DLZKY, dlzka);
    znak(b, 257 + i as u32);
    b.zapis((dlzka - ZAKLAD_DLZKY[i] as usize) as u32, BITY_DLZKY[i] as u32);
    let d = index_zakladu(&ZAKLAD_VZDIALENOSTI, vzdialenost);
    b.kod(d as u32, 5);
    b.zapis((vzdialenost - ZAKLAD_VZDIALENOSTI[d] as usize) as u32, BITY_VZDIALENOSTI[d] as u32);
}

const OKNO: usize = 32768;
const ROZPTYL_BITOV: u32 = 15;
const NIC: u32 = u32::MAX;
/// Koľko predchodcov s rovnakým začiatkom sa najviac porovná – rozumný pomer rýchlosti a stlačenia.
const HLBKA: usize = 24;

/// Stlačí údaje metódou DEFLATE: jeden posledný blok s pevnými Huffmanovými kódmi.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    let n = data.len();
    let mut b = Bity::nove(n / 8 + 64);
    b.zapis(1, 1); // posledný blok
    b.zapis(1, 2); // pevné kódy
    let mut hlavy = vec![NIC; 1 << ROZPTYL_BITOV];
    let mut predchodca = vec![NIC; OKNO];
    let rozptyl = |i: usize| {
        let v = data[i] as u32 | (data[i + 1] as u32) << 8 | (data[i + 2] as u32) << 16;
        (v.wrapping_mul(0x9E37_79B1) >> (32 - ROZPTYL_BITOV)) as usize
    };
    let vloz = |i: usize, hlavy: &mut [u32], predchodca: &mut [u32]| {
        if i + 3 <= n {
            let h = rozptyl(i);
            predchodca[i % OKNO] = hlavy[h];
            hlavy[h] = i as u32;
        }
    };
    let mut i = 0;
    while i < n {
        let (mut dlzka, mut vzdialenost) = (0, 0);
        if i + 3 <= n {
            let najviac = (n - i).min(258);
            let mut kandidat = hlavy[rozptyl(i)];
            let mut hlbka = 0;
            while kandidat != NIC && hlbka < HLBKA {
                let k = kandidat as usize;
                if k >= i || i - k > OKNO {
                    break;
                }
                // najprv bajt, ktorý by predĺžil doterajšiu najlepšiu zhodu – väčšina kandidátov tu skončí
                if data[k + dlzka.min(najviac - 1)] == data[i + dlzka.min(najviac - 1)] {
                    let l = data[k..k + najviac]
                        .iter()
                        .zip(&data[i..i + najviac])
                        .take_while(|(a, b)| a == b)
                        .count();
                    if l > dlzka {
                        (dlzka, vzdialenost) = (l, i - k);
                        if l == najviac {
                            break;
                        }
                    }
                }
                let dalsi = predchodca[k % OKNO];
                if dalsi == NIC || dalsi as usize >= k {
                    break;
                }
                kandidat = dalsi;
                hlbka += 1;
            }
        }
        if dlzka >= 3 {
            opakovanie(&mut b, dlzka, vzdialenost);
            for j in i..i + dlzka {
                vloz(j, &mut hlavy, &mut predchodca);
            }
            i += dlzka;
        } else {
            znak(&mut b, data[i] as u32);
            vloz(i, &mut hlavy, &mut predchodca);
            i += 1;
        }
    }
    znak(&mut b, 256); // koniec bloku
    b.dokonci()
}

/// Údaje vo formáte zlib (hlavička, DEFLATE, Adler-32).
pub fn zlib(data: &[u8]) -> Vec<u8> {
    // 0x78 = DEFLATE s oknom 32 KB; 0x01 = bez slovníka, (0x7801 % 31 == 0)
    let mut v = vec![0x78, 0x01];
    v.extend(deflate(data));
    v.extend(adler32(data).to_be_bytes());
    v
}

fn kus(von: &mut Vec<u8>, typ: &[u8; 4], data: &[u8]) {
    von.extend((data.len() as u32).to_be_bytes());
    von.extend(typ);
    von.extend(data);
    von.extend(crc32(&crc32_pokracuj_hotove(typ, data)).to_be_bytes());
}

/// Typ a údaje kusu za sebou – z nich sa počíta CRC kusu.
fn crc32_pokracuj_hotove(typ: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(4 + data.len());
    v.extend(typ);
    v.extend(data);
    v
}

/// Riadky obrázka s filtrami PNG (pred každým riadkom bajt typu filtra).
fn filtrovane(p: &Platno) -> Vec<u8> {
    let w = p.sirka * 3;
    let mut von = Vec::with_capacity((w + 1) * p.vyska);
    let mut predosly = vec![0u8; w];
    let mut riadok = vec![0u8; w];
    let mut kandidati = [vec![0u8; w], vec![0u8; w], vec![0u8; w]];
    for y in 0..p.vyska {
        for (x, &f) in p.body[y * p.sirka..(y + 1) * p.sirka].iter().enumerate() {
            riadok[3 * x..3 * x + 3].copy_from_slice(&[(f >> 16) as u8, (f >> 8) as u8, f as u8]);
        }
        // 0 = žiadny, 1 = Sub (rozdiel od bodu vľavo), 2 = Up (rozdiel od bodu nad ním)
        for i in 0..w {
            kandidati[0][i] = riadok[i];
            kandidati[1][i] = riadok[i].wrapping_sub(if i >= 3 { riadok[i - 3] } else { 0 });
            kandidati[2][i] = riadok[i].wrapping_sub(predosly[i]);
        }
        let sucet = |k: &[u8]| k.iter().map(|&x| (x as i8).unsigned_abs() as u64).sum::<u64>();
        let (typ, najlepsi) =
            kandidati.iter().enumerate().min_by_key(|(_, k)| sucet(k)).unwrap_or((0, &kandidati[0]));
        von.push(typ as u8);
        von.extend_from_slice(najlepsi);
        std::mem::swap(&mut predosly, &mut riadok);
    }
    von
}

impl Platno {
    /// Obrázok PNG (RGB, 8 bitov na farbu) – napr. snímka stavovej obrazovky.
    pub fn png(&self) -> Vec<u8> {
        let mut von = PODPIS.to_vec();
        let mut hlavicka = Vec::with_capacity(13);
        hlavicka.extend((self.sirka as u32).to_be_bytes());
        hlavicka.extend((self.vyska as u32).to_be_bytes());
        hlavicka.extend([8, 2, 0, 0, 0]); // 8 bitov, RGB, DEFLATE, filtre PNG, bez prekladania
        kus(&mut von, b"IHDR", &hlavicka);
        kus(&mut von, b"tEXt", b"Software\0ORCHIDEA - SKLENIK");
        kus(&mut von, b"IDAT", &zlib(&filtrovane(self)));
        kus(&mut von, b"IEND", &[]);
        von
    }
}

#[cfg(test)]
mod skusky {
    use super::*;

    /// Čítanie bitov od najnižšieho (len pre skúšky).
    struct Citac<'a> {
        d: &'a [u8],
        poz: usize,
    }

    impl Citac<'_> {
        fn bit(&mut self) -> u32 {
            let b = (self.d[self.poz / 8] >> (self.poz % 8)) & 1;
            self.poz += 1;
            b as u32
        }
        fn bity(&mut self, n: u32) -> u32 {
            (0..n).fold(0, |v, k| v | self.bit() << k)
        }
        /// Znak pevného Huffmanovho kódu (kód sa číta od najvyššieho bitu).
        fn znak(&mut self) -> u32 {
            let mut kod = 0;
            for dlzka in 1..=9 {
                kod = (kod << 1) | self.bit();
                match dlzka {
                    7 if kod <= 23 => return 256 + kod,
                    8 if (0x30..=0xBF).contains(&kod) => return kod - 0x30,
                    8 if (0xC0..=0xC7).contains(&kod) => return 280 + kod - 0xC0,
                    9 if (0x190..=0x1FF).contains(&kod) => return 144 + kod - 0x190,
                    _ => {}
                }
            }
            panic!("neplatný kód");
        }
    }

    /// Nezávislé rozbalenie DEFLATE s pevnými kódmi – overí, že stlačené údaje sú správne.
    fn inflate(d: &[u8]) -> Vec<u8> {
        let mut c = Citac { d, poz: 0 };
        assert_eq!(c.bity(1), 1, "posledný blok");
        assert_eq!(c.bity(2), 1, "pevné kódy");
        let mut v: Vec<u8> = Vec::new();
        loop {
            let z = c.znak();
            match z {
                0..=255 => v.push(z as u8),
                256 => return v,
                _ => {
                    let i = (z - 257) as usize;
                    let dlzka = ZAKLAD_DLZKY[i] as usize + c.bity(BITY_DLZKY[i] as u32) as usize;
                    let kod = (0..5).fold(0, |k, _| (k << 1) | c.bit()) as usize;
                    let vzd =
                        ZAKLAD_VZDIALENOSTI[kod] as usize + c.bity(BITY_VZDIALENOSTI[kod] as u32) as usize;
                    assert!(vzd <= v.len() && vzd <= OKNO);
                    for _ in 0..dlzka {
                        v.push(v[v.len() - vzd]);
                    }
                }
            }
        }
    }

    /// Rozbalí PNG z `png()` späť na plátno (kontroluje podpis, CRC kusov, Adler-32 a filtre).
    fn precitaj(png: &[u8]) -> Platno {
        assert_eq!(&png[..8], &PODPIS);
        let (mut i, mut w, mut h, mut idat) = (8, 0, 0, Vec::new());
        loop {
            let dlzka = u32::from_be_bytes(png[i..i + 4].try_into().unwrap()) as usize;
            let typ = &png[i + 4..i + 8];
            let data = &png[i + 8..i + 8 + dlzka];
            let crc = u32::from_be_bytes(png[i + 8 + dlzka..i + 12 + dlzka].try_into().unwrap());
            assert_eq!(crc, crc32(&png[i + 4..i + 8 + dlzka]), "CRC kusu");
            match typ {
                b"IHDR" => {
                    w = u32::from_be_bytes(data[0..4].try_into().unwrap()) as usize;
                    h = u32::from_be_bytes(data[4..8].try_into().unwrap()) as usize;
                    assert_eq!(&data[8..], &[8, 2, 0, 0, 0]);
                }
                b"IDAT" => idat.extend_from_slice(data),
                b"IEND" => break,
                _ => {}
            }
            i += 12 + dlzka;
        }
        assert_eq!((idat[0], idat[1]), (0x78, 0x01));
        let surove = inflate(&idat[2..idat.len() - 4]);
        assert_eq!(u32::from_be_bytes(idat[idat.len() - 4..].try_into().unwrap()), adler32(&surove));
        let r = w * 3;
        assert_eq!(surove.len(), (r + 1) * h);
        let mut p = Platno::nove(w, h, 0);
        let mut predosly = vec![0u8; r];
        for y in 0..h {
            let typ = surove[y * (r + 1)];
            let mut riadok = surove[y * (r + 1) + 1..(y + 1) * (r + 1)].to_vec();
            for x in 0..r {
                riadok[x] = match typ {
                    0 => riadok[x],
                    1 => riadok[x].wrapping_add(if x >= 3 { riadok[x - 3] } else { 0 }),
                    2 => riadok[x].wrapping_add(predosly[x]),
                    _ => panic!("nečakaný filter {typ}"),
                };
            }
            for x in 0..w {
                p.body[y * w + x] =
                    (riadok[3 * x] as u32) << 16 | (riadok[3 * x + 1] as u32) << 8 | riadok[3 * x + 2] as u32;
            }
            predosly = riadok;
        }
        p
    }

    #[test]
    fn kontrolne_sucty_podla_normy() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(&vec![0xFF; 100_000]), {
            let (mut a, mut b) = (1u64, 0u64);
            for _ in 0..100_000 {
                a = (a + 255) % 65521;
                b = (b + a) % 65521;
            }
            (b << 16 | a) as u32
        });
        assert_eq!((0x78u32 * 256 + 0x01) % 31, 0, "hlavička zlib");
    }

    #[test]
    fn deflate_sa_da_rozbalit_aj_pri_dlhych_opakovaniach() {
        let mut data = b"SKLENIK SKLENIK SKLENIK ".repeat(50);
        data.extend(std::iter::repeat_n(7u8, 70_000)); // dlhšie ako okno 32 KB
        let mut x = 12345u32;
        data.extend((0..50_000).map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x % 7) as u8 // náhodné, ale s opakovaniami
        }));
        data.extend_from_slice(&data.clone()[1000..40_000]); // opakovanie spred viac ako 32 KB
        for d in [&data[..], b"", b"a", b"ab", b"abc", b"aaaa"] {
            assert_eq!(inflate(&deflate(d)), d);
        }
        // najdlhšie opakovanie DEFLATE je 258 B a pevný kód má 13 bitov – milión rovnakých bajtov ≈ 6,3 kB
        assert!(deflate(&vec![0u8; 1_000_000]).len() < 6500, "plocha jednej farby sa stlačí");
    }

    #[test]
    fn obrazovka_do_png_a_spat() {
        let pismo = crate::Pismo::stonka();
        let mut p = Platno::nove(1280, 720, 0x0F1C15);
        p.vypln(50, 120, 1180, 2, 0x2A4535);
        for (i, t) in
            ["SKLENÍK", "Teplota 45,1 °C – chladné · plný výkon", "Pamäť 1 GB · overená ✓"].iter().enumerate()
        {
            p.text(&pismo, t, 60.0, 80.0 + 60.0 * i as f32, 32.0, 3.5, 0xE8F0EA - i as u32 * 0x101010);
        }
        p.body[0] = 0xFF0000; // roh – overí poradie farieb
        let png = p.png();
        assert!(png.len() < 120_000, "obrazovka má mať desiatky kB, má {} B", png.len());
        assert_eq!(precitaj(&png), p);
        // malé a nepárne rozmery
        let mut q = Platno::nove(3, 2, 0x123456);
        q.body[4] = 0xABCDEF;
        assert_eq!(precitaj(&q.png()), q);
    }
}
