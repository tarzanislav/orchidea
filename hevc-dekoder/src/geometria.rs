//! Geometria obrazu: poradie CTB v dlaždiciach (6.5.1), adresy najmenších transformačných blokov v poradí z
//! (6.5.2) a dostupnosť susedov (6.4.1).

use orchidea_hevc::{Pps, Sps};

/// Prevody adries CTB a mapa MinTbAddrZs pre jednu dvojicu SPS a PPS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Geometria {
    pub sirka: usize,
    pub vyska: usize,
    pub log2_ctb: u32,
    pub sirka_ctb: usize,
    pub vyska_ctb: usize,
    pub log2_min_tb: u32,
    pub sirka_tb: usize,
    /// CtbAddrRsToTs a CtbAddrTsToRs
    pub rs_na_ts: Vec<u32>,
    pub ts_na_rs: Vec<u32>,
    /// TileId podľa adresy v rastri
    pub dlazdica_rs: Vec<u16>,
    /// hranice stĺpcov a riadkov dlaždíc v CTB (colBd, rowBd) – posledná hodnota je šírka/výška v CTB
    pub stlpce: Vec<usize>,
    pub riadky: Vec<usize>,
    /// MinTbAddrZs v rastri najmenších transformačných blokov
    pub zs: Vec<u32>,
}

impl Geometria {
    pub fn nova(sps: &Sps, pps: &Pps) -> Geometria {
        let (sc, sr) = sps.ctb_mriezka();
        let (sc, sr) = (sc as usize, sr as usize);
        let (sirky, vysky) = match &pps.dlazdice {
            Some(d) => d.rozdelenie(sps),
            None => (vec![sc as u32], vec![sr as u32]),
        };
        let mut stlpce = vec![0usize];
        for w in &sirky {
            stlpce.push(stlpce.last().unwrap() + *w as usize);
        }
        let mut riadky = vec![0usize];
        for h in &vysky {
            riadky.push(riadky.last().unwrap() + *h as usize);
        }
        let n = sc * sr;
        let mut rs_na_ts = vec![0u32; n];
        let mut dlazdica_rs = vec![0u16; n];
        for rs in 0..n {
            let (x, y) = (rs % sc, rs / sc);
            let tx = (0..sirky.len()).rfind(|&i| stlpce[i] <= x).unwrap_or(0);
            let ty = (0..vysky.len()).rfind(|&j| riadky[j] <= y).unwrap_or(0);
            let mut ts: usize = sirky[..tx].iter().map(|&w| vysky[ty] as usize * w as usize).sum();
            ts += vysky[..ty].iter().map(|&h| sc * h as usize).sum::<usize>();
            ts += (y - riadky[ty]) * sirky[tx] as usize + x - stlpce[tx];
            rs_na_ts[rs] = ts as u32;
            dlazdica_rs[rs] = (ty * sirky.len() + tx) as u16;
        }
        let mut ts_na_rs = vec![0u32; n];
        for (rs, &ts) in rs_na_ts.iter().enumerate() {
            ts_na_rs[ts as usize] = rs as u32;
        }
        let log2_ctb = sps.log2_ctb;
        let log2_min_tb = sps.log2_min_tb;
        let d = log2_ctb - log2_min_tb;
        let sirka_tb = sc << d;
        let vyska_tb = sr << d;
        let mut zs = vec![0u32; sirka_tb * vyska_tb];
        for y in 0..vyska_tb {
            for x in 0..sirka_tb {
                let rs = (y >> d) * sc + (x >> d);
                let mut p = rs_na_ts[rs] << (2 * d);
                for i in 0..d {
                    let m = 1usize << i;
                    if x & m != 0 {
                        p += (m * m) as u32;
                    }
                    if y & m != 0 {
                        p += (2 * m * m) as u32;
                    }
                }
                zs[y * sirka_tb + x] = p;
            }
        }
        Geometria {
            sirka: sps.sirka as usize,
            vyska: sps.vyska as usize,
            log2_ctb,
            sirka_ctb: sc,
            vyska_ctb: sr,
            log2_min_tb,
            sirka_tb,
            rs_na_ts,
            ts_na_rs,
            dlazdica_rs,
            stlpce,
            riadky,
            zs,
        }
    }

    /// MinTbAddrZs vzorky jasu (x, y).
    #[inline]
    pub fn z(&self, x: usize, y: usize) -> u32 {
        self.zs[(y >> self.log2_min_tb) * self.sirka_tb + (x >> self.log2_min_tb)]
    }

    /// Adresa CTB (raster), ktorý obsahuje vzorku jasu (x, y).
    #[inline]
    pub fn ctb(&self, x: usize, y: usize) -> usize {
        (y >> self.log2_ctb) * self.sirka_ctb + (x >> self.log2_ctb)
    }

    /// Prvý stĺpec CTB dlaždice, ktorá obsahuje stĺpec CTB `x`.
    pub fn zaciatok_stlpca(&self, x: usize) -> usize {
        *self.stlpce.iter().rev().find(|&&s| s <= x).unwrap_or(&0)
    }

    /// Šírka dlaždice v CTB pre stĺpec CTB `x`.
    pub fn sirka_dlazdice(&self, x: usize) -> usize {
        let i = self.stlpce.iter().rposition(|&s| s <= x).unwrap_or(0);
        self.stlpce.get(i + 1).copied().unwrap_or(self.sirka_ctb) - self.stlpce[i]
    }

    /// Je CTB (raster) prvý v svojej dlaždici?
    pub fn prvy_v_dlazdici(&self, rs: usize) -> bool {
        let (x, y) = (rs % self.sirka_ctb, rs / self.sirka_ctb);
        self.stlpce.contains(&x) && self.riadky.contains(&y)
    }

    /// Dostupnosť v poradí z (6.4.1): sused (xn, yn) voči aktuálnemu bloku (xc, yc); `adresa_rezu` je SliceAddrRs
    /// každého CTB (u32::MAX = ešte nedekódovaný).
    #[inline]
    pub fn dostupny(&self, adresa_rezu: &[u32], xc: i32, yc: i32, xn: i32, yn: i32) -> bool {
        if xn < 0 || yn < 0 || xn as usize >= self.sirka || yn as usize >= self.vyska {
            return false;
        }
        let (xn, yn, xc, yc) = (xn as usize, yn as usize, xc as usize, yc as usize);
        if self.z(xn, yn) > self.z(xc, yc) {
            return false;
        }
        let (cn, cc) = (self.ctb(xn, yn), self.ctb(xc, yc));
        adresa_rezu[cn] == adresa_rezu[cc] && self.dlazdica_rs[cn] == self.dlazdica_rs[cc]
    }
}
