//! Spoločné údaje pre filtre v slučke (odblokovací filter a adaptívny posun vzoriek) – vyplní ich dekodér počas
//! dekódovania obrazu, filtre ich len čítajú.
//!
//! ROZHRANIE (pevné): [`PohybBloku`], [`RezFiltra`], [`InfoObrazu`], [`MapyFiltra`].

/// Pohyb bloku 4×4 jasu pre odblokovací filter (8.7.2.4): vektory a identita referenčných obrazov.
/// `obraz[i]` = PicOrderCntVal referenčného obrazu použitého zo zoznamu L`i`, `None` = zoznam sa nepoužíva.
/// Dva rôzne obrazy v pamäti dekodéra nemajú rovnaké POC, preto POC jednoznačne určuje obraz.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PohybBloku {
    pub mv: [[i16; 2]; 2],
    pub obraz: [Option<i32>; 2],
}

/// Údaje rezu (slice) potrebné pre filtre.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RezFiltra {
    /// SliceAddrRs – adresa prvého CTB rezu (nezávislého segmentu)
    pub adresa: u32,
    /// slice_loop_filter_across_slices_enabled_flag
    pub cez_rezy: bool,
    /// slice_beta_offset_div2
    pub beta_div2: i8,
    /// slice_tc_offset_div2
    pub tc_div2: i8,
}

/// Geometria obrazu, rezy a dlaždice po CTB.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InfoObrazu {
    /// rozmery obrazu v jase (pic_width/height_in_luma_samples)
    pub sirka: usize,
    pub vyska: usize,
    /// CtbLog2SizeY
    pub log2_ctb: u32,
    pub sirka_ctb: usize,
    pub vyska_ctb: usize,
    /// index do `rezy` pre každý CTB (raster, index `ry * sirka_ctb + rx`)
    pub ctb_rez: Vec<u16>,
    /// TileId každého CTB (raster)
    pub ctb_dlazdica: Vec<u16>,
    /// CtbAddrRsToTs – poradie CTB v dekódovaní (raster → poradie v dlaždiciach)
    pub ctb_ts: Vec<u32>,
    /// loop_filter_across_tiles_enabled_flag
    pub cez_dlazdice: bool,
    pub rezy: Vec<RezFiltra>,
    /// pps_cb_qp_offset, pps_cr_qp_offset (cQpPicOffset odblokovania farby)
    pub cb_qp_offset: i32,
    pub cr_qp_offset: i32,
    pub bitova_hlbka: u32,
}

impl InfoObrazu {
    /// Rez CTB, ktorý obsahuje vzorku jasu (x, y).
    #[inline]
    pub fn rez_vzorky(&self, x: usize, y: usize) -> &RezFiltra {
        let i = (y >> self.log2_ctb) * self.sirka_ctb + (x >> self.log2_ctb);
        &self.rezy[self.ctb_rez[i] as usize]
    }
}

/// Mapy obrazu s jemnosťou blokov 4×4 vzoriek jasu: index bloku `(y / 4) * sirka4 + x / 4`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MapyFiltra {
    pub sirka4: usize,
    pub vyska4: usize,
    /// Zvislá hrana na **ľavej** strane bloku: 0 = nefiltruje sa, 1 = hrana predikčného bloku,
    /// 2 = hrana transformačného bloku (môže byť zároveň hranou predikčného bloku). Dekodér už započítal
    /// filterEdgeFlag (okraj obrazu, dlaždice, rezu) aj slice_deblocking_filter_disabled_flag; hrany sú
    /// zapísané len na mriežke 8 × 8 (x deliteľné 8).
    pub hrana_v: Vec<u8>,
    /// Vodorovná hrana na **hornej** strane bloku – rovnaký význam, y deliteľné 8.
    pub hrana_h: Vec<u8>,
    /// blok patrí vnútrosnímkovej kódovacej jednotke (CuPredMode = MODE_INTRA)
    pub intra: Vec<bool>,
    /// transformačný blok jasu, ktorý obsahuje tento blok, má aspoň jeden nenulový koeficient
    pub nenulove: Vec<bool>,
    /// pohyb (len medzisnímkové bloky)
    pub pohyb: Vec<PohybBloku>,
    /// QpY kódovacej jednotky
    pub qp: Vec<i8>,
    /// vzorky bloku filtre nemenia: pcm_flag pri pcm_loop_filter_disabled_flag, alebo cu_transquant_bypass_flag
    pub bez_filtra: Vec<bool>,
}

impl MapyFiltra {
    /// Prázdne mapy pre obraz `sirka` × `vyska` vzoriek jasu.
    pub fn nove(sirka: usize, vyska: usize) -> MapyFiltra {
        let (s4, v4) = (sirka.div_ceil(4), vyska.div_ceil(4));
        let n = s4 * v4;
        MapyFiltra {
            sirka4: s4,
            vyska4: v4,
            hrana_v: vec![0; n],
            hrana_h: vec![0; n],
            intra: vec![false; n],
            nenulove: vec![false; n],
            pohyb: vec![PohybBloku::default(); n],
            qp: vec![0; n],
            bez_filtra: vec![false; n],
        }
    }

    /// Index bloku 4×4, ktorý obsahuje vzorku jasu (x, y).
    #[inline]
    pub fn index(&self, x: usize, y: usize) -> usize {
        (y >> 2) * self.sirka4 + (x >> 2)
    }
}
