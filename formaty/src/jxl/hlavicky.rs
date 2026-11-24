//! Hlavičky JPEG XL: veľkosť, metadáta obrazu (hĺbka, dodatočné kanály, farba, tónovanie) a hlavička snímky.

use super::bity::{Citac, D, chyba, so_znamienkom};
use orchidea_jadro::Vysledok;

const ROZMER: [D; 4] = [D::B(9, 1), D::B(13, 1), D::B(18, 1), D::B(30, 1)];

/// Rozmery (SizeHeader alebo PreviewHeader).
pub(super) fn velkost(c: &mut Citac) -> Vysledok<(u32, u32)> {
    let div8 = c.bool();
    let vyska = if div8 { 8 * (1 + c.u(5)) } else { c.u32(ROZMER) };
    let pomer = c.u(3);
    let sirka = podla_pomeru(c, pomer, vyska, div8, ROZMER)?;
    Ok((sirka, vyska))
}

fn podla_pomeru(c: &mut Citac, pomer: u32, vyska: u32, div8: bool, d: [D; 4]) -> Vysledok<u32> {
    let v = vyska as u64;
    Ok(match pomer {
        0 => {
            if div8 {
                8 * (1 + c.u(5))
            } else {
                c.u32(d)
            }
        }
        1 => vyska,
        2 => (v * 12 / 10) as u32,
        3 => (v * 4 / 3) as u32,
        4 => (v * 3 / 2) as u32,
        5 => (v * 16 / 9) as u32,
        6 => (v * 5 / 4) as u32,
        _ => (v * 2) as u32,
    })
}

fn nahlad(c: &mut Citac) -> Vysledok<(u32, u32)> {
    let d: [D; 4] = [D::B(6, 1), D::B(8, 65), D::B(10, 321), D::B(12, 1345)];
    let div8 = c.bool();
    let vyska = if div8 { 8 * c.u32([D::V(16), D::V(32), D::B(5, 1), D::B(9, 33)]) } else { c.u32(d) };
    let pomer = c.u(3);
    let sirka = if pomer == 0 && div8 {
        8 * c.u32([D::V(16), D::V(32), D::B(5, 1), D::B(9, 33)])
    } else {
        podla_pomeru(c, pomer, vyska, false, d)?
    };
    Ok((sirka, vyska))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Hlbka {
    pub plavajuca: bool,
    pub bitov: u32,
    pub exp_bitov: u32,
}

fn hlbka(c: &mut Citac) -> Vysledok<Hlbka> {
    let plavajuca = c.bool();
    let (bitov, exp_bitov) = if plavajuca {
        (c.u32([D::V(32), D::V(16), D::V(24), D::B(6, 1)]), 1 + c.u(4))
    } else {
        (c.u32([D::V(8), D::V(10), D::V(12), D::B(6, 1)]), 0)
    };
    if bitov == 0 || bitov > 31 && !(plavajuca && bitov == 32) {
        return Err(chyba("neplatná bitová hĺbka"));
    }
    Ok(Hlbka { plavajuca, bitov, exp_bitov })
}

#[derive(Debug, Clone)]
pub(super) struct DodatocnyKanal {
    pub typ: u32,
    pub hlbka: Hlbka,
    pub posun_rozmeru: u32,
    pub alfa_nasobena: bool,
}

/// Typ dodatočného kanála „alfa“.
pub(super) const KANAL_ALFA: u32 = 0;

fn dodatocny(c: &mut Citac) -> Vysledok<DodatocnyKanal> {
    if c.bool() {
        return Ok(DodatocnyKanal {
            typ: KANAL_ALFA,
            hlbka: Hlbka { plavajuca: false, bitov: 8, exp_bitov: 0 },
            posun_rozmeru: 0,
            alfa_nasobena: false,
        });
    }
    let typ = c.enum_()?;
    let h = hlbka(c)?;
    let posun_rozmeru = c.u32([D::V(0), D::V(3), D::V(4), D::B(3, 1)]);
    let dlzka_mena = c.u32([D::V(0), D::B(4, 0), D::B(5, 16), D::B(10, 48)]);
    // meno kanála sa zatiaľ nepoužije
    c.preskoc(8 * dlzka_mena as u64);
    let alfa_nasobena = if typ == KANAL_ALFA { c.bool() } else { false };
    if typ == 2 {
        for _ in 0..4 {
            c.f16()?;
        }
    }
    if typ == 5 {
        c.u32([D::V(1), D::B(2, 0), D::B(4, 3), D::B(8, 19)]);
    }
    c.over()?;
    Ok(DodatocnyKanal { typ, hlbka: h, posun_rozmeru, alfa_nasobena })
}

/// Kódovanie farby (ColourEncoding).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Farba {
    pub chce_icc: bool,
    /// 0 RGB, 1 sivá, 2 XYB, 3 neznámy
    pub priestor: u32,
    /// biely bod: 1 D65, 2 vlastný, 10 E, 11 DCI
    pub biela: u32,
    pub biela_xy: (f64, f64),
    /// základné farby: 1 sRGB, 2 vlastné, 9 BT.2100, 11 P3
    pub zakladne: u32,
    pub zakladne_xy: [(f64, f64); 3],
    /// gama (ak je), inak prenosová funkcia: 1 BT.709, 8 lineárna, 13 sRGB, 16 PQ, 17 DCI, 18 HLG
    pub gama: Option<f64>,
    pub prenos: u32,
    pub zamer: u32,
}

impl Default for Farba {
    fn default() -> Self {
        Farba {
            chce_icc: false,
            priestor: 0,
            biela: 1,
            biela_xy: (0.3127, 0.3290),
            zakladne: 1,
            zakladne_xy: [(0.64, 0.33), (0.30, 0.60), (0.15, 0.06)],
            gama: None,
            prenos: 13,
            zamer: 1,
        }
    }
}

fn xy(c: &mut Citac) -> (f64, f64) {
    let d = [D::B(19, 0), D::B(19, 524_288), D::B(20, 1_048_576), D::B(21, 2_097_152)];
    let x = so_znamienkom(c.u32(d)) as f64 / 1e6;
    let y = so_znamienkom(c.u32(d)) as f64 / 1e6;
    (x, y)
}

fn farba(c: &mut Citac) -> Vysledok<Farba> {
    let mut f = Farba::default();
    if c.bool() {
        return Ok(f);
    }
    f.chce_icc = c.bool();
    f.priestor = c.enum_()?;
    if !f.chce_icc && f.priestor != 2 {
        f.biela = c.enum_()?;
        match f.biela {
            2 => f.biela_xy = xy(c),
            10 => f.biela_xy = (1.0 / 3.0, 1.0 / 3.0),
            11 => f.biela_xy = (0.314, 0.351),
            _ => {}
        }
        if f.priestor != 1 {
            f.zakladne = c.enum_()?;
            match f.zakladne {
                2 => f.zakladne_xy = [xy(c), xy(c), xy(c)],
                9 => f.zakladne_xy = [(0.708, 0.292), (0.170, 0.797), (0.131, 0.046)],
                11 => f.zakladne_xy = [(0.680, 0.320), (0.265, 0.690), (0.150, 0.060)],
                _ => {}
            }
        }
    }
    if !f.chce_icc {
        if c.bool() {
            f.gama = Some(c.u(24) as f64 / 1e7);
        } else {
            f.prenos = c.enum_()?;
        }
        f.zamer = c.enum_()?;
    }
    Ok(f)
}

fn rozsirenia(c: &mut Citac) -> Vysledok<()> {
    let e = c.u64();
    let mut bity = 0u64;
    for i in 0..64 {
        if e >> i & 1 == 1 {
            bity = bity.saturating_add(c.u64());
        }
    }
    if bity > 1 << 40 {
        return Err(chyba("priveľké rozšírenia"));
    }
    c.preskoc(bity);
    Ok(())
}

/// Metadáta obrazu (ImageMetadata) s dátami transformácie (CustomTransformData).
#[derive(Debug, Clone)]
pub(super) struct Metadata {
    pub natocenie: u32,
    pub nahlad: Option<(u32, u32)>,
    pub animacia: Option<(bool,)>,
    pub hlbka: Hlbka,
    pub dodatocne: Vec<DodatocnyKanal>,
    pub xyb: bool,
    pub farba: Farba,
    pub cielovy_jas: f32,
    /// Obrátená matica opsin (XYB → lineárne RGB) a posuny.
    pub opsin: [[f32; 3]; 3],
    pub opsin_bias: [f32; 3],
    pub kvant_bias: [f32; 3],
    pub kvant_bias_citatel: f32,
    /// Váhy zväčšenia 2×, 4×, 8× (ak sú vlastné).
    pub vahy_zvacsenia: [Option<Vec<f32>>; 3],
}

pub(super) const OPSIN_PREDVOLENA: [[f32; 3]; 3] = [
    [11.031_567, -9.866_944, -0.164_623],
    [-3.254_147_4, 4.418_770_3, -0.164_623],
    [-3.658_851_4, 2.712_923, 1.945_928_2],
];

pub(super) fn metadata(c: &mut Citac) -> Vysledok<Metadata> {
    let mut m = Metadata {
        natocenie: 1,
        nahlad: None,
        animacia: None,
        hlbka: Hlbka { plavajuca: false, bitov: 8, exp_bitov: 0 },
        dodatocne: Vec::new(),
        xyb: true,
        farba: Farba::default(),
        cielovy_jas: 255.0,
        opsin: OPSIN_PREDVOLENA,
        opsin_bias: [-0.003_793_073_3; 3],
        kvant_bias: [1.0 - 0.054_650_073, 1.0 - 0.070_054_5, 1.0 - 0.049_935_103],
        kvant_bias_citatel: 0.145,
        vahy_zvacsenia: [None, None, None],
    };
    if !c.bool() {
        let extra = c.bool();
        if extra {
            m.natocenie = 1 + c.u(3);
            if c.bool() {
                velkost(c)?;
            }
            if c.bool() {
                m.nahlad = Some(nahlad(c)?);
            }
            if c.bool() {
                c.u32([D::V(100), D::V(1000), D::B(10, 1), D::B(30, 1)]);
                c.u32([D::V(1), D::V(1001), D::B(8, 1), D::B(10, 1)]);
                c.u32([D::V(0), D::B(3, 0), D::B(16, 0), D::B(32, 0)]);
                m.animacia = Some((c.bool(),));
            }
        }
        m.hlbka = hlbka(c)?;
        let _modular_16 = c.bool();
        let n = c.u32([D::V(0), D::V(1), D::B(4, 2), D::B(12, 1)]);
        if n > 256 {
            return Err(chyba("priveľa dodatočných kanálov"));
        }
        for _ in 0..n {
            m.dodatocne.push(dodatocny(c)?);
        }
        m.xyb = c.bool();
        m.farba = farba(c)?;
        if extra && !c.bool() {
            m.cielovy_jas = c.f16()?;
            c.f16()?;
            c.bool();
            c.f16()?;
        }
        rozsirenia(c)?;
    }
    // CustomTransformData
    if !c.bool() {
        if m.xyb && !c.bool() {
            for r in m.opsin.iter_mut() {
                for x in r.iter_mut() {
                    *x = c.f16()?;
                }
            }
            for x in m.opsin_bias.iter_mut() {
                *x = c.f16()?;
            }
            for x in m.kvant_bias.iter_mut() {
                *x = c.f16()?;
            }
            m.kvant_bias_citatel = c.f16()?;
        }
        let maska = c.u(3);
        for (i, n) in [15usize, 55, 210].iter().enumerate() {
            if maska >> i & 1 == 1 {
                let mut v = Vec::with_capacity(*n);
                for _ in 0..*n {
                    v.push(c.f16()?);
                }
                m.vahy_zvacsenia[i] = Some(v);
            }
        }
    }
    c.over()?;
    Ok(m)
}

/// Filtre obnovy (RestorationFilter).
#[derive(Debug, Clone)]
pub(super) struct Filtre {
    pub gabor: bool,
    pub gabor_vahy: [(f32, f32); 3],
    pub epf_iteracie: u32,
    pub epf_ostrost: [f32; 8],
    pub epf_kanaly: [f32; 3],
    pub epf_kvant_nas: f32,
    pub epf_sigma0: f32,
    pub epf_sigma2: f32,
    pub epf_okraj: f32,
    pub epf_sigma_modular: f32,
}

fn filtre(c: &mut Citac, modular: bool) -> Vysledok<Filtre> {
    let mut f = Filtre {
        gabor: true,
        gabor_vahy: [(0.115_169_525, 0.061_248_592); 3],
        epf_iteracie: 1,
        epf_ostrost: [0.0, 1.0 / 7.0, 2.0 / 7.0, 3.0 / 7.0, 4.0 / 7.0, 5.0 / 7.0, 6.0 / 7.0, 1.0],
        epf_kanaly: [40.0, 5.0, 3.5],
        epf_kvant_nas: 0.46,
        epf_sigma0: 0.9,
        epf_sigma2: 6.5,
        epf_okraj: 2.0 / 3.0,
        epf_sigma_modular: 1.0,
    };
    if c.bool() {
        return Ok(f);
    }
    f.gabor = c.bool();
    if f.gabor && c.bool() {
        for v in f.gabor_vahy.iter_mut() {
            *v = (c.f16()?, c.f16()?);
        }
    }
    f.epf_iteracie = c.u(2);
    if f.epf_iteracie > 0 {
        if !modular && c.bool() {
            for x in f.epf_ostrost.iter_mut() {
                *x = c.f16()?;
            }
        }
        if c.bool() {
            for x in f.epf_kanaly.iter_mut() {
                *x = c.f16()?;
            }
            c.f16()?;
            c.f16()?;
        }
        if c.bool() {
            if !modular {
                f.epf_kvant_nas = c.f16()?;
            }
            f.epf_sigma0 = c.f16()?;
            f.epf_sigma2 = c.f16()?;
            f.epf_okraj = c.f16()?;
        }
        if modular {
            f.epf_sigma_modular = c.f16()?;
        }
    }
    rozsirenia(c)?;
    Ok(f)
}

/// Prechody progresívneho obrazu.
#[derive(Debug, Clone)]
pub(super) struct Prechody {
    pub pocet: u32,
    pub posun: Vec<u32>,
    pub zmensenie: Vec<u32>,
    pub posledny: Vec<u32>,
}

/// Spôsob prekrytia snímok (BlendingInfo).
#[derive(Debug, Clone, Copy)]
pub(super) struct Prekrytie {
    pub rezim: u32,
}

/// Hlavička snímky (FrameHeader).
#[derive(Debug, Clone)]
pub(super) struct HlavickaSnimky {
    pub typ: u32,
    pub modular: bool,
    pub priznaky: u64,
    pub ycbcr: bool,
    pub jpeg_podvzorkovanie: [u32; 3],
    pub zvacsenie: u32,
    pub posun_skupiny: u32,
    pub x_qm: u32,
    pub b_qm: u32,
    pub prechody: Prechody,
    pub lf_uroven: u32,
    pub x0: i32,
    pub y0: i32,
    pub sirka: u32,
    pub vyska: u32,
    pub prekrytie: Prekrytie,
    pub posledna: bool,
    pub uloz_ako: u32,
    pub filtre: Filtre,
}

pub(super) const PRIZNAK_SUM: u64 = 1;
pub(super) const PRIZNAK_ZAPLATY: u64 = 2;
pub(super) const PRIZNAK_KRIVKY: u64 = 16;
pub(super) const PRIZNAK_LF_SNIMKA: u64 = 32;

pub(super) fn hlavicka_snimky(
    c: &mut Citac,
    m: &Metadata,
    sirka: u32,
    vyska: u32,
) -> Vysledok<HlavickaSnimky> {
    let pred_prechody = Prechody { pocet: 1, posun: vec![], zmensenie: vec![], posledny: vec![] };
    let mut h = HlavickaSnimky {
        typ: 0,
        modular: false,
        priznaky: 0,
        ycbcr: false,
        jpeg_podvzorkovanie: [0; 3],
        zvacsenie: 1,
        posun_skupiny: 1,
        x_qm: 3,
        b_qm: 2,
        prechody: pred_prechody,
        lf_uroven: 0,
        x0: 0,
        y0: 0,
        sirka,
        vyska,
        prekrytie: Prekrytie { rezim: 0 },
        posledna: true,
        uloz_ako: 0,
        filtre: filtre_predvolene(),
    };
    if c.bool() {
        return Ok(h);
    }
    h.typ = c.u(2);
    h.modular = c.u(1) == 1;
    h.priznaky = c.u64();
    if !m.xyb {
        h.ycbcr = c.bool();
    }
    if h.priznaky & PRIZNAK_LF_SNIMKA == 0 {
        if h.ycbcr {
            for x in h.jpeg_podvzorkovanie.iter_mut() {
                *x = c.u(2);
            }
        }
        h.zvacsenie = c.u32([D::V(1), D::V(2), D::V(4), D::V(8)]);
        for _ in &m.dodatocne {
            c.u32([D::V(1), D::V(2), D::V(4), D::V(8)]);
        }
    }
    if h.modular {
        h.posun_skupiny = c.u(2);
    }
    if m.xyb && !h.modular {
        h.x_qm = c.u(3);
        h.b_qm = c.u(3);
    }
    if h.typ != 2 {
        let pocet = c.u32([D::V(1), D::V(2), D::V(3), D::B(3, 4)]);
        let mut p = Prechody { pocet, posun: vec![], zmensenie: vec![], posledny: vec![] };
        if pocet != 1 {
            let n = c.u32([D::V(0), D::V(1), D::V(2), D::B(1, 3)]);
            for _ in 0..pocet - 1 {
                p.posun.push(c.u(2));
            }
            for _ in 0..n {
                p.zmensenie.push(c.u32([D::V(1), D::V(2), D::V(4), D::V(8)]));
            }
            for _ in 0..n {
                p.posledny.push(c.u32([D::V(0), D::V(1), D::V(2), D::B(3, 0)]));
            }
        }
        h.prechody = p;
    }
    if h.typ == 1 {
        h.lf_uroven = 1 + c.u(2);
    }
    let mut vyrez = false;
    if h.typ != 1 {
        vyrez = c.bool();
    }
    if vyrez {
        let d = [D::B(8, 0), D::B(11, 256), D::B(14, 2304), D::B(30, 18_688)];
        if h.typ != 2 {
            h.x0 = so_znamienkom(c.u32(d));
            h.y0 = so_znamienkom(c.u32(d));
        }
        h.sirka = c.u32(d);
        h.vyska = c.u32(d);
    }
    let cela = !vyrez
        || (h.x0 <= 0
            && h.y0 <= 0
            && h.x0 as i64 + h.sirka as i64 >= sirka as i64
            && h.y0 as i64 + h.vyska as i64 >= vyska as i64);
    let mut trvanie = 0;
    if h.typ == 0 || h.typ == 3 {
        h.prekrytie = prekrytie(c, m, cela)?;
        for _ in &m.dodatocne {
            prekrytie(c, m, cela)?;
        }
        if let Some((casove_kody,)) = m.animacia {
            trvanie = c.u32([D::V(0), D::V(1), D::B(8, 0), D::B(32, 0)]);
            if casove_kody {
                c.u(32);
            }
        }
        h.posledna = c.bool();
    } else {
        h.posledna = false;
    }
    if h.typ != 1 && !h.posledna {
        h.uloz_ako = c.u(2);
    }
    let pred_ct = h.typ == 2
        || (cela
            && (h.typ == 0 || h.typ == 3)
            && h.prekrytie.rezim == 0
            && (trvanie == 0 || h.uloz_ako != 0)
            && !h.posledna);
    if pred_ct {
        c.bool();
    }
    let dlzka_mena = c.u32([D::V(0), D::B(4, 0), D::B(5, 16), D::B(10, 48)]);
    c.preskoc(8 * dlzka_mena as u64);
    h.filtre = filtre(c, h.modular)?;
    rozsirenia(c)?;
    c.over()?;
    Ok(h)
}

fn filtre_predvolene() -> Filtre {
    let mut c = Citac::novy(&[1]);
    filtre(&mut c, false).unwrap_or_else(|_| unreachable!("predvolené filtre"))
}

fn prekrytie(c: &mut Citac, m: &Metadata, cela: bool) -> Vysledok<Prekrytie> {
    let rezim = c.u32([D::V(0), D::V(1), D::V(2), D::B(2, 3)]);
    if !m.dodatocne.is_empty() && (rezim == 2 || rezim == 3) {
        c.u32([D::V(0), D::V(1), D::V(2), D::B(3, 3)]);
    }
    if !m.dodatocne.is_empty() && (rezim == 2 || rezim == 3 || rezim == 4) {
        c.bool();
    }
    if rezim != 0 || !cela {
        c.u(2); // zdroj prekrytia
    }
    Ok(Prekrytie { rezim })
}
