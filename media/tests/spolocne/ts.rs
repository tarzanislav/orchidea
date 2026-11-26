//! Vlastný zapisovač MPEG-TS a M2TS pre skúšky (ISO/IEC 13818-1) – PAT, PMT, PES s PTS/DTS, výplň
//! v adaptačnom poli, počítadlá kontinuity; pakety 188, 192 (M2TS) a 204 B.

/// CRC-32 sekcie PSI (polynóm 0x04C11DB7).
pub fn crc32(d: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in d {
        c ^= (b as u32) << 24;
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
        }
    }
    c
}

/// 5 bajtov časovej značky PES s prefixom `prefix` (0x2 = len PTS, 0x3 = PTS pri PTS+DTS, 0x1 = DTS).
pub fn znacka(prefix: u8, v: u64) -> [u8; 5] {
    let v = v & ((1 << 33) - 1);
    [
        (prefix << 4) | (((v >> 30) & 7) << 1) as u8 | 1,
        (v >> 22) as u8,
        (((v >> 15) & 0x7F) << 1) as u8 | 1,
        (v >> 7) as u8,
        ((v & 0x7F) << 1) as u8 | 1,
    ]
}

pub struct Zapisovac {
    pub velkost: usize,
    cc: Vec<u8>,
    pub data: Vec<u8>,
}

impl Zapisovac {
    pub fn novy(velkost: usize) -> Zapisovac {
        Zapisovac { velkost, cc: vec![0; 8192], data: Vec::new() }
    }

    /// Jeden transportný paket; vráti počet zapísaných bajtov `udaje`.
    pub fn paket(&mut self, pid: u16, zaciatok: bool, nahodny: bool, udaje: &[u8]) -> usize {
        let mut p = Vec::with_capacity(self.velkost);
        if self.velkost == 192 {
            p.extend_from_slice(&[0, 0, 0, 0]);
        }
        p.push(0x47);
        p.push(((zaciatok as u8) << 6) | (pid >> 8) as u8);
        p.push(pid as u8);
        let n = udaje.len().min(184);
        let s_polom = nahodny || n < 184;
        let n = if s_polom { udaje.len().min(182) } else { n };
        let cc = self.cc[pid as usize];
        self.cc[pid as usize] = (cc + 1) & 0x0F;
        p.push(if s_polom { 0x30 } else { 0x10 } | cc);
        if s_polom {
            let dlzka_pola = 183 - n;
            p.push(dlzka_pola as u8);
            if dlzka_pola > 0 {
                p.push(if nahodny { 0x40 } else { 0 });
                p.extend(std::iter::repeat_n(0xFF, dlzka_pola - 1));
            }
        }
        p.extend_from_slice(&udaje[..n]);
        if self.velkost == 204 {
            p.extend_from_slice(&[0xAB; 16]);
        }
        assert_eq!(p.len(), self.velkost);
        self.data.extend_from_slice(&p);
        n
    }

    /// Sekcia PSI (bez CRC – doplní sa) v jednom alebo viacerých paketoch.
    pub fn sekcia(&mut self, pid: u16, telo: &[u8]) {
        let mut s = telo.to_vec();
        s.extend_from_slice(&crc32(telo).to_be_bytes());
        let mut d = vec![0u8]; // pointer_field
        d.extend_from_slice(&s);
        let mut o = 0;
        let mut prvy = true;
        while o < d.len() {
            o += self.paket(pid, prvy, false, &d[o..]);
            prvy = false;
        }
    }

    pub fn pat(&mut self, programy: &[(u16, u16)]) {
        let mut t = vec![0x00, 0xB0, 0, 0x00, 0x01, 0xC1, 0x00, 0x00];
        for &(cislo, pid) in programy {
            t.extend_from_slice(&cislo.to_be_bytes());
            t.extend_from_slice(&(0xE000 | pid).to_be_bytes());
        }
        let dlzka = t.len() - 3 + 4;
        t[1] = 0xB0 | (dlzka >> 8) as u8;
        t[2] = dlzka as u8;
        self.sekcia(0, &t);
    }

    /// PMT: toky (typ toku, PID, deskriptory).
    pub fn pmt(&mut self, pid: u16, program: u16, info: &[u8], toky: &[(u8, u16, Vec<u8>)]) {
        let mut t = vec![0x02, 0xB0, 0];
        t.extend_from_slice(&program.to_be_bytes());
        t.extend_from_slice(&[0xC1, 0, 0]);
        let pcr = toky.first().map_or(0x1FFF, |x| x.1);
        t.extend_from_slice(&(0xE000 | pcr).to_be_bytes());
        t.extend_from_slice(&(0xF000 | info.len() as u16).to_be_bytes());
        t.extend_from_slice(info);
        for (typ, tok_pid, d) in toky {
            t.push(*typ);
            t.extend_from_slice(&(0xE000 | tok_pid).to_be_bytes());
            t.extend_from_slice(&(0xF000 | d.len() as u16).to_be_bytes());
            t.extend_from_slice(d);
        }
        let dlzka = t.len() - 3 + 4;
        t[1] = 0xB0 | (dlzka >> 8) as u8;
        t[2] = dlzka as u8;
        self.sekcia(pid, &t);
    }

    /// PES s PTS (a DTS, ak sa líši); `s_dlzkou` = PES_packet_length vyplnená (inak 0 – video).
    pub fn pes(
        &mut self,
        pid: u16,
        id: u8,
        pts: Option<u64>,
        dts: Option<u64>,
        nahodny: bool,
        udaje: &[u8],
        s_dlzkou: bool,
    ) {
        let mut h = Vec::new();
        match (pts, dts) {
            (Some(p), Some(d)) if p != d => {
                h.extend_from_slice(&[0x80, 0xC0, 10]);
                h.extend_from_slice(&znacka(3, p));
                h.extend_from_slice(&znacka(1, d));
            }
            (Some(p), _) => {
                h.extend_from_slice(&[0x80, 0x80, 5]);
                h.extend_from_slice(&znacka(2, p));
            }
            _ => h.extend_from_slice(&[0x80, 0x00, 0]),
        }
        let dlzka = if s_dlzkou { h.len() + udaje.len() } else { 0 };
        let mut d = vec![0, 0, 1, id, (dlzka >> 8) as u8, dlzka as u8];
        d.extend_from_slice(&h);
        d.extend_from_slice(udaje);
        let mut o = 0;
        let mut prvy = true;
        while o < d.len() {
            o += self.paket(pid, prvy, prvy && nahodny, &d[o..]);
            prvy = false;
        }
    }
}

/// Rámec ADTS (AAC LC, 48 kHz, stereo) dĺžky `n` s výplňou `v`.
pub fn adts(n: usize, v: u8) -> Vec<u8> {
    let mut r =
        vec![0xFF, 0xF1, 0x4C, 0x80 | (n >> 11) as u8, (n >> 3) as u8, ((n & 7) << 5) as u8 | 0x1F, 0xFC];
    r.resize(n, v);
    r
}

/// Rámec AC-3 48 kHz, 192 kb/s, stereo (768 B).
pub fn ac3(v: u8) -> Vec<u8> {
    let mut r = vec![0x0B, 0x77, 0, 0, 0x14, 0x40, 0x40, 0x00];
    r.resize(768, v);
    r
}

/// Rámec E-AC-3 48 kHz stereo, 6 blokov, `n` B (párne); `zavisly` = závislý podtok.
pub fn eac3(n: usize, zavisly: bool, v: u8) -> Vec<u8> {
    let frmsiz = n / 2 - 1;
    let mut r = vec![
        0x0B,
        0x77,
        if zavisly { 0x40 } else { 0 } | (frmsiz >> 8) as u8,
        frmsiz as u8,
        0x34,
        0x80,
        0,
        0,
    ];
    r.resize(n, v);
    r
}

/// Rámec MPEG-1 vrstvy II, 192 kb/s, 48 kHz (576 B).
pub fn mp2(v: u8) -> Vec<u8> {
    let mut r = vec![0xFF, 0xFD, 0xA4, 0x00];
    r.resize(576, v);
    r
}

/// Prístupová jednotka H.264: AUD, rez IDR alebo P.
pub fn h264(idr: bool, n: usize, v: u8) -> Vec<u8> {
    let mut r = vec![0, 0, 0, 1, 0x09, 0xF0, 0, 0, 1, if idr { 0x65 } else { 0x41 }, 0x88];
    r.resize(n, v);
    r
}

/// Typický súbor: H.264 25 snímok/s (IDR každých 25) a ADTS 48 kHz v PES po ≈ 700 B (rámce cez hranice PES).
/// Vráti súbor, začiatok PTS videa a dĺžky rámcov zvuku.
pub fn typicky(velkost: usize, sekund: u64, zaciatok: u64) -> (Vec<u8>, Vec<usize>) {
    let mut z = Zapisovac::novy(velkost);
    z.pat(&[(1, 0x1000)]);
    z.pmt(0x1000, 1, &[], &[(0x1B, 0x100, vec![]), (0x0F, 0x101, vec![0x0A, 4, b's', b'l', b'k', 0])]);
    let snimok = 25 * sekund;
    let ramcov = (sekund * 48_000).div_ceil(1024);
    let dlzky: Vec<usize> = (0..ramcov).map(|k| 200 + (k as usize * 37) % 300).collect();
    let mut tok = Vec::new();
    let mut zaciatky = Vec::new();
    for (k, &n) in dlzky.iter().enumerate() {
        zaciatky.push(tok.len());
        tok.extend(adts(n, k as u8));
    }
    let mut o = 0usize;
    for s in 0..snimok {
        let dts = zaciatok + s * 3600;
        let idr = s % 25 == 0;
        z.pes(
            0x100,
            0xE0,
            Some(dts + 3600),
            Some(dts),
            idr,
            &h264(idr, 300 + (s as usize % 7) * 50, s as u8),
            false,
        );
        // zvuk do času snímky (index rámca, v ktorom je bajt `o`)
        while o < tok.len() && (zaciatky.partition_point(|&x| x <= o) as u64 - 1) * 1920 <= s * 3600 {
            zvuk_pes(&mut z, &tok, &zaciatky, &mut o, zaciatok);
        }
        if s % 50 == 49 {
            z.pat(&[(1, 0x1000)]);
        }
    }
    while o < tok.len() {
        zvuk_pes(&mut z, &tok, &zaciatky, &mut o, zaciatok);
    }
    (z.data, dlzky)
}

/// PES zvuku s ≈ 700 B toku od `o`; PTS = čas prvého rámca, ktorý v PES začína (ISO/IEC 13818-1 §2.4.3.7).
fn zvuk_pes(z: &mut Zapisovac, tok: &[u8], zaciatky: &[usize], o: &mut usize, zaciatok: u64) {
    let n = 700.min(tok.len() - *o);
    let prvy = zaciatky.partition_point(|&x| x < *o);
    let pts =
        (prvy < zaciatky.len() && zaciatky[prvy] < *o + n).then(|| zaciatok + 3600 + prvy as u64 * 1920);
    z.pes(0x101, 0xC0, pts, None, false, &tok[*o..*o + n], true);
    *o += n;
}
