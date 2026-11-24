//! Odtlačok dekódovaného obrazu (decoded picture hash, H.265 D.3.19) – vlastné MD5 (RFC 1321) a výpočet
//! odtlačku rovín obrazu. Slúži vlastnému softvérovému dekodéru aj hardvérovej ceste (RPi 4/5) na dôkaz bitovej
//! zhody so skúšobnými tokmi normy.

/// Posuny rotácie pre 64 krokov MD5.
const POSUNY: [u32; 64] = {
    let r = [[7, 12, 17, 22], [5, 9, 14, 20], [4, 11, 16, 23], [6, 10, 15, 21]];
    let mut p = [0u32; 64];
    let mut i = 0;
    while i < 64 {
        p[i] = r[i / 16][i % 4];
        i += 1;
    }
    p
};

/// Konštanty K[i] = ⌊|sin(i + 1)| · 2³²⌋ (RFC 1321, kap. 3.4).
fn konstanty() -> [u32; 64] {
    std::array::from_fn(|i| (((i + 1) as f64).sin().abs() * 4_294_967_296.0) as u32)
}

/// Postupný výpočet MD5.
#[derive(Debug, Clone)]
pub struct Md5 {
    stav: [u32; 4],
    k: [u32; 64],
    blok: [u8; 64],
    v_bloku: usize,
    dlzka: u64,
}

impl Default for Md5 {
    fn default() -> Self {
        Self::novy()
    }
}

impl Md5 {
    pub fn novy() -> Md5 {
        Md5 {
            stav: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476],
            k: konstanty(),
            blok: [0; 64],
            v_bloku: 0,
            dlzka: 0,
        }
    }

    fn spracuj_blok(&mut self, blok: &[u8; 64]) {
        let mut m = [0u32; 16];
        for (mi, kus) in m.iter_mut().zip(blok.chunks_exact(4)) {
            *mi = u32::from_le_bytes([kus[0], kus[1], kus[2], kus[3]]);
        }
        let [mut a, mut b, mut c, mut d] = self.stav;
        let k = &self.k;
        // štyri kolá po 16 krokoch (RFC 1321, kap. 3.4) – bez vetvenia vnútri kola
        macro_rules! krok {
            ($f:expr, $i:expr, $g:expr) => {{
                let f = $f.wrapping_add(a).wrapping_add(k[$i]).wrapping_add(m[$g]);
                a = d;
                d = c;
                c = b;
                b = b.wrapping_add(f.rotate_left(POSUNY[$i]));
            }};
        }
        for i in 0..16 {
            krok!((b & c) | (!b & d), i, i);
        }
        for i in 16..32 {
            krok!((d & b) | (!d & c), i, (5 * i + 1) % 16);
        }
        for i in 32..48 {
            krok!(b ^ c ^ d, i, (3 * i + 5) % 16);
        }
        for i in 48..64 {
            krok!(c ^ (b | !d), i, (7 * i) % 16);
        }
        for (s, v) in self.stav.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }

    pub fn pridaj(&mut self, mut data: &[u8]) {
        self.dlzka = self.dlzka.wrapping_add(data.len() as u64);
        if self.v_bloku > 0 {
            let n = (64 - self.v_bloku).min(data.len());
            self.blok[self.v_bloku..self.v_bloku + n].copy_from_slice(&data[..n]);
            self.v_bloku += n;
            data = &data[n..];
            if self.v_bloku == 64 {
                let b = self.blok;
                self.spracuj_blok(&b);
                self.v_bloku = 0;
            }
        }
        while data.len() >= 64 {
            let b: [u8; 64] = data[..64].try_into().unwrap();
            self.spracuj_blok(&b);
            data = &data[64..];
        }
        self.blok[..data.len()].copy_from_slice(data);
        self.v_bloku += data.len();
    }

    pub fn vysledok(mut self) -> [u8; 16] {
        let bity = self.dlzka.wrapping_mul(8);
        self.pridaj(&[0x80]);
        while self.v_bloku != 56 {
            self.pridaj(&[0]);
        }
        self.pridaj(&bity.to_le_bytes());
        let mut v = [0u8; 16];
        for (i, s) in self.stav.iter().enumerate() {
            v[4 * i..4 * i + 4].copy_from_slice(&s.to_le_bytes());
        }
        v
    }
}

/// MD5 jednej roviny obrazu (pictureData z D.3.19): vzorky riadok po riadku, pri bitovej hĺbke ≤ 8 jeden bajt
/// na vzorku. `data` má riadok `y` na `data[y * krok..]`.
pub fn md5_roviny(data: &[u8], sirka: usize, vyska: usize, krok: usize) -> [u8; 16] {
    let mut m = Md5::novy();
    for y in 0..vyska {
        m.pridaj(&data[y * krok..y * krok + sirka]);
    }
    m.vysledok()
}

/// MD5 roviny s 16-bitovými vzorkami (bitová hĺbka > 8): dva bajty na vzorku, nižší prvý.
pub fn md5_roviny16(data: &[u16], sirka: usize, vyska: usize, krok: usize) -> [u8; 16] {
    let mut m = Md5::novy();
    let mut riadok = Vec::with_capacity(2 * sirka);
    for y in 0..vyska {
        riadok.clear();
        for &v in &data[y * krok..y * krok + sirka] {
            riadok.extend_from_slice(&v.to_le_bytes());
        }
        m.pridaj(&riadok);
    }
    m.vysledok()
}

/// CRC jednej roviny (D.3.19, picture_crc): polynóm 0x1021, počiatočná hodnota 0xFFFF, bity od najvyššieho,
/// na konci 16 nulových bitov. Pri bitovej hĺbke ≤ 8 jeden bajt na vzorku.
pub fn crc_roviny(data: &[u8], sirka: usize, vyska: usize, krok: usize) -> u16 {
    let mut crc: u32 = 0xFFFF;
    for y in 0..vyska {
        for &bajt in &data[y * krok..y * krok + sirka] {
            for i in (0..8).rev() {
                let msb = (crc >> 15) & 1;
                let bit = (bajt as u32 >> i) & 1;
                crc = (((crc << 1) + bit) & 0xFFFF) ^ (msb * 0x1021);
            }
        }
    }
    for _ in 0..16 {
        let msb = (crc >> 15) & 1;
        crc = ((crc << 1) & 0xFFFF) ^ (msb * 0x1021);
    }
    crc as u16
}

/// Kontrolný súčet jednej roviny (D.3.19, picture_checksum): súčet vzoriek XOR maska z polohy, modulo 2³².
pub fn kontrolny_sucet_roviny(data: &[u8], sirka: usize, vyska: usize, krok: usize) -> u32 {
    let mut sucet: u32 = 0;
    for y in 0..vyska {
        for (x, &v) in data[y * krok..y * krok + sirka].iter().enumerate() {
            let maska = ((x & 0xFF) ^ (y & 0xFF) ^ (x >> 8) ^ (y >> 8)) as u32;
            sucet = sucet.wrapping_add((v as u32 & 0xFF) ^ maska);
        }
    }
    sucet
}

/// Odtlačok ako šestnástkový text.
pub fn hex(o: &[u8; 16]) -> String {
    o.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod testy {
    use super::*;

    fn md5(t: &[u8]) -> String {
        let mut m = Md5::novy();
        m.pridaj(t);
        hex(&m.vysledok())
    }

    /// Skúšobné vektory RFC 1321, príloha A.5.
    #[test]
    fn vektory_rfc_1321() {
        assert_eq!(md5(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5(b"a"), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(md5(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(md5(b"message digest"), "f96b697d7cb7938d525a2f31aaf161d0");
        assert_eq!(md5(b"abcdefghijklmnopqrstuvwxyz"), "c3fcd3d76192e4007dfb496cca67e13b");
        assert_eq!(
            md5(b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
        // po kúskoch rovnako ako naraz
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
        let mut m = Md5::novy();
        for k in data.chunks(37) {
            m.pridaj(k);
        }
        assert_eq!(hex(&m.vysledok()), md5(&data));
        // posun so začiatkom 0xFFFF a doplnením 16 nulových bitov je katalógový CRC-16/AUG-CCITT (priama
        // počiatočná hodnota 0x1D0F) – kontrolná hodnota pre „123456789“ je 0xE5CC
        assert_eq!(crc_roviny(b"123456789", 9, 1, 9), 0xE5CC);
        assert_eq!(kontrolny_sucet_roviny(&[10, 20, 30, 40], 2, 2, 2), 10 + (20 ^ 1) + (30 ^ 1) + 40);
        assert_eq!(md5_roviny(&[1, 2, 9, 3, 4, 9], 2, 2, 3), {
            let mut m = Md5::novy();
            m.pridaj(&[1, 2, 3, 4]);
            m.vysledok()
        });
    }
}
