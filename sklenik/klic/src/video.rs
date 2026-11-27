//! Skúšobné video s presne známym obrazom (R165) – vlastný generátor tokov H.264 a HEVC.
//!
//! Hardvérový dekodér sa dá overiť len tak, že sa mu dá tok, ktorého výsledok vieme **na bod presne**. Bežné
//! video to nedovolí (kompresia so stratou – výsledok závisí od dekodéra). Preto KLÍČ skladá vlastné toky,
//! v ktorých je každý blok **PCM** – nekomprimované vzorky priamo v toku (H.264 makrobloky I_PCM, HEVC
//! kódovacie jednotky s pcm_flag). Dekodér ich musí vrátiť bit po bite; každá odchýlka je chyba dekodéra
//! alebo ovládača a jej poloha (riadok, stĺpec, rovina) ukáže, kde.
//!
//! * **H.264** – entropické kódovanie CAVLC: I_PCM je len číslo typu makrobloku (ue 25) a zarovnanie.
//! * **HEVC** – rez je vždy CABAC (aritmetické kódovanie): pre každú jednotku 16 × 16 sa zakóduje
//!   `part_mode` (jeden binárny symbol s kontextom), `pcm_flag` (ukončovací symbol a vyprázdnenie kódera),
//!   zarovnanie, vzorky a znovuspustenie kódera – presne podľa ITU-T H.265, kap. 9.3.
//!
//! Vzor obrazu mení každý bod inak (vodorovne, zvisle aj po snímkach), takže zámena riadkov, rovín, poradia
//! snímok alebo posun o bod sa nedá skryť.
//!
//! Všetko je vlastný kód bez cudzích knižníc; generátor sa v CI overuje porovnaním s nezávislým dekodérom.

/// Obraz 4:2:0 v troch rovinách (Y, Cb, Cr) bez zarovnania riadkov.
#[derive(Debug, Clone, PartialEq)]
pub struct Obraz {
    pub sirka: usize,
    pub vyska: usize,
    pub y: Vec<u8>,
    pub cb: Vec<u8>,
    pub cr: Vec<u8>,
}

/// Vzorový obraz snímky `n` – každý bod, riadok, rovina a snímka má inú hodnotu (aj nulu a 255, aby sa
/// v toku objavili postupnosti, ktoré treba chrániť pred zámenou so štartovacím kódom 00 00 01).
pub fn vzor(sirka: usize, vyska: usize, n: usize) -> Obraz {
    let mut y = Vec::with_capacity(sirka * vyska);
    for r in 0..vyska {
        for s in 0..sirka {
            // pomalý vodorovný a zvislý prechod + jemná šachovnica + posun po snímkach
            let v = (s * 3 + r * 5 + n * 17 + ((s ^ r) & 4) * 9) & 0xFF;
            y.push(if (s / 16 + r / 16 + n).is_multiple_of(7) { 0 } else { v as u8 });
        }
    }
    let (cs, cv) = (sirka / 2, vyska / 2);
    let mut cb = Vec::with_capacity(cs * cv);
    let mut cr = Vec::with_capacity(cs * cv);
    for r in 0..cv {
        for s in 0..cs {
            cb.push(((s * 7 + r * 2 + n * 29 + 40) & 0xFF) as u8);
            cr.push(((255 + s + r * 11 + n * 13) & 0xFF) as u8);
        }
    }
    Obraz { sirka, vyska, y, cb, cr }
}

/// Odchýlka bodu: (rovina, riadok, stĺpec, čakané, dekódované).
pub type Odchylka = (&'static str, usize, usize, u8, u8);

/// Prvá odchýlka dekódovanej snímky od vzoru a počet chýb.
pub fn porovnaj(cakany: &Obraz, y: &[u8], cb: &[u8], cr: &[u8]) -> (usize, Option<Odchylka>) {
    let mut chyb = 0;
    let mut prva = None;
    for (meno, ocakavane, dekodovane, sirka) in [
        ("Y", &cakany.y, y, cakany.sirka),
        ("Cb", &cakany.cb, cb, cakany.sirka / 2),
        ("Cr", &cakany.cr, cr, cakany.sirka / 2),
    ] {
        for (i, (a, b)) in ocakavane.iter().zip(dekodovane).enumerate() {
            if a != b {
                chyb += 1;
                prva.get_or_insert((meno, i / sirka, i % sirka, *a, *b));
            }
        }
        chyb += ocakavane.len().saturating_sub(dekodovane.len());
    }
    (chyb, prva)
}

/// R237: riadky v rámci bloku (0..vyska_bloku), ktoré sú chybné aspoň v polovici radov blokov – napr. „12–15“
/// ukáže, že dekodér zle zapisuje spodok každého radu makroblokov; „žiadne“ = chyby nie sú viazané na riadok bloku.
pub fn chybne_riadky_bloku(ziadane: &[u8], skutocne: &[u8], sirka: usize, vyska_bloku: usize) -> String {
    if sirka == 0 || vyska_bloku == 0 {
        return "žiadne".into();
    }
    let riadkov = ziadane.len().min(skutocne.len()) / sirka;
    let mut zle = vec![0usize; vyska_bloku];
    let mut vsetky = vec![0usize; vyska_bloku];
    for r in 0..riadkov {
        vsetky[r % vyska_bloku] += 1;
        if ziadane[r * sirka..(r + 1) * sirka] != skutocne[r * sirka..(r + 1) * sirka] {
            zle[r % vyska_bloku] += 1;
        }
    }
    let k: Vec<usize> = (0..vyska_bloku).filter(|&k| vsetky[k] > 0 && zle[k] * 2 >= vsetky[k]).collect();
    if k.is_empty() {
        return "žiadne".into();
    }
    // súvislé úseky ako „a–b“
    let mut casti = Vec::new();
    let mut i = 0;
    while i < k.len() {
        let mut j = i;
        while j + 1 < k.len() && k[j + 1] == k[j] + 1 {
            j += 1;
        }
        casti.push(if i == j { k[i].to_string() } else { format!("{}–{}", k[i], k[j]) });
        i = j + 1;
    }
    casti.join(",")
}

/// R237: odkiaľ je chybný riadok jasu – presne zhodný riadok niektorej čakanej snímky („s2 r12“ = 2. snímka,
/// riadok 12; stará snímka = zápis do iného bufra, iný riadok = posunutý zápis), stála hodnota, alebo „?“.
pub fn povod_riadku(cakane: &[Obraz], riadok: &[u8]) -> String {
    if riadok.is_empty() {
        return "?".into();
    }
    for (k, o) in cakane.iter().enumerate() {
        if o.sirka != riadok.len() {
            continue;
        }
        if let Some(m) = o.y.chunks(o.sirka).position(|r| r == riadok) {
            return format!("s{} r{m}", k + 1);
        }
    }
    if riadok.iter().all(|&b| b == riadok[0]) {
        return format!("všade {}", riadok[0]);
    }
    "?".into()
}

/// R242: na koniec posledného prístupového bloku jednotka NAL „koniec toku“ (typ 11). Dekodér SM1 pri
/// V4L2_DEC_CMD_STOP posledné 2 snímky z vyrovnávacej pamäte obrazov neodovzdá (6 z 8) – overí, či ich vydá
/// po výslovnom konci toku (ak áno, ovládač ho môže pri vyprázdnení pridať sám, ako HEVC prúdu B).
pub fn s_koncom_toku(mut toky: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    if let Some(posledny) = toky.last_mut() {
        posledny.extend(nal(&[0x0B], &[]));
    }
    toky
}

/// Zapisovač bitov (najvyšší bit bajtu prvý) s kódmi Exp-Golomb.
#[derive(Default)]
struct Bity {
    data: Vec<u8>,
    /// koľko bitov posledného bajtu je obsadených (0 = zarovnané)
    obsadene: u32,
}

impl Bity {
    fn bit(&mut self, b: u32) {
        if self.obsadene == 0 {
            self.data.push(0);
        }
        if b & 1 != 0 {
            *self.data.last_mut().expect("bajt") |= 0x80 >> self.obsadene;
        }
        self.obsadene = (self.obsadene + 1) % 8;
    }

    fn u(&mut self, n: u32, v: u64) {
        for i in (0..n).rev() {
            self.bit((v >> i) as u32 & 1);
        }
    }

    fn ue(&mut self, v: u64) {
        let x = v + 1;
        let n = 64 - x.leading_zeros();
        self.u(n - 1, 0);
        self.u(n, x);
    }

    fn se(&mut self, v: i64) {
        self.ue(if v > 0 { (2 * v - 1) as u64 } else { (-2 * v) as u64 });
    }

    fn zarovnane(&self) -> bool {
        self.obsadene == 0
    }

    fn nuly_do_zarovnania(&mut self) {
        while !self.zarovnane() {
            self.bit(0);
        }
    }

    /// rbsp_trailing_bits: bit 1 a nuly do konca bajtu
    fn koniec(&mut self) {
        self.bit(1);
        self.nuly_do_zarovnania();
    }

    fn bajty(&mut self, b: &[u8]) {
        debug_assert!(self.zarovnane());
        self.data.extend_from_slice(b);
    }
}

/// Jednotka NAL so štartovacím kódom: hlavička + RBSP s ochranou proti emulácii (00 00 0x → 00 00 03 0x).
fn nal(hlavicka: &[u8], rbsp: &[u8]) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 1];
    v.extend_from_slice(hlavicka);
    let mut nuly = 0;
    for &b in rbsp {
        if nuly >= 2 && b <= 3 {
            v.push(3);
            nuly = 0;
        }
        v.push(b);
        nuly = if b == 0 { nuly + 1 } else { 0 };
    }
    v
}

fn bloky(o: &Obraz, x: usize, y: usize, velkost: usize, ciel: &mut Vec<u8>) {
    for r in 0..velkost {
        ciel.extend_from_slice(&o.y[(y + r) * o.sirka + x..][..velkost]);
    }
    let (cx, cy, cv, cs) = (x / 2, y / 2, velkost / 2, o.sirka / 2);
    for rovina in [&o.cb, &o.cr] {
        for r in 0..cv {
            ciel.extend_from_slice(&rovina[(cy + r) * cs + cx..][..cv]);
        }
    }
}

/// SPS a PPS H.264 (Constrained Baseline, CAVLC, QP 26, 25 snímok/s, okamžitý výstup snímky).
fn h264_sps_pps(sirka: usize, vyska: usize) -> (Bity, Bity) {
    let mut sps = Bity::default();
    sps.u(8, 66); // Baseline
    sps.u(8, 0xC0); // constraint_set0/1 – Constrained Baseline
    sps.u(8, 40); // úroveň 4.0
    sps.ue(0); // seq_parameter_set_id
    sps.ue(0); // log2_max_frame_num_minus4
    sps.ue(2); // pic_order_cnt_type
    sps.ue(1); // max_num_ref_frames
    sps.u(1, 0); // gaps_in_frame_num_value_allowed_flag
    sps.ue((sirka / 16 - 1) as u64);
    sps.ue((vyska / 16 - 1) as u64);
    sps.u(1, 1); // frame_mbs_only_flag
    sps.u(1, 1); // direct_8x8_inference_flag
    sps.u(1, 0); // frame_cropping_flag
    sps.u(1, 1); // vui_parameters_present_flag
    sps.u(4, 0); // aspect_ratio, overscan, video_signal_type, chroma_loc – nie
    sps.u(1, 1); // timing_info_present_flag
    sps.u(32, 1); // num_units_in_tick
    sps.u(32, 50); // time_scale → 25 snímok/s
    sps.u(1, 1); // fixed_frame_rate_flag
    sps.u(2, 0); // nal/vcl_hrd_parameters_present_flag
    sps.u(1, 0); // pic_struct_present_flag
    sps.u(1, 1); // bitstream_restriction_flag
    sps.u(1, 1); // motion_vectors_over_pic_boundaries_flag
    sps.ue(0); // max_bytes_per_pic_denom
    sps.ue(0); // max_bits_per_mb_denom
    sps.ue(15); // log2_max_mv_length_horizontal
    sps.ue(15); // log2_max_mv_length_vertical
    sps.ue(0); // max_num_reorder_frames – snímka von hneď
    sps.ue(1); // max_dec_frame_buffering
    sps.koniec();

    let mut pps = Bity::default();
    pps.ue(0); // pic_parameter_set_id
    pps.ue(0); // seq_parameter_set_id
    pps.u(1, 0); // entropy_coding_mode_flag – CAVLC
    pps.u(1, 0); // bottom_field_pic_order_in_frame_present_flag
    pps.ue(0); // num_slice_groups_minus1
    pps.ue(0); // num_ref_idx_l0_default_active_minus1
    pps.ue(0); // num_ref_idx_l1_default_active_minus1
    pps.u(1, 0); // weighted_pred_flag
    pps.u(2, 0); // weighted_bipred_idc
    pps.se(0); // pic_init_qp_minus26
    pps.se(0); // pic_init_qs_minus26
    pps.se(0); // chroma_qp_index_offset
    pps.u(1, 1); // deblocking_filter_control_present_flag
    pps.u(1, 0); // constrained_intra_pred_flag
    pps.u(1, 0); // redundant_pic_cnt_present_flag
    pps.koniec();
    (sps, pps)
}

/// Tok H.264 (Annex B) zo snímok – po jednej prístupovej jednotke na snímku (prvá má aj SPS a PPS). Každá
/// snímka je IDR, všetky makrobloky I_PCM. Rozmery musia byť násobkom 16.
pub fn h264_pcm(snimky: &[Obraz]) -> Vec<Vec<u8>> {
    h264_pcm_odblokovanie(snimky, false)
}

/// R239: tie isté bloky PCM, len s **zapnutým odblokovaním** (disable_deblocking_filter_idc = 0, posuny 0).
/// Pri I_PCM je QP 0 (norma 8.7.2.2) → α = 0 → filter nezmení ani jeden bod, výstup musí byť presne vstup.
/// Líši sa teda len príznak filtra – na SM1 overí hypotézu, že chybné spodné 4 riadky každého radu makroblokov
/// (26. 11.: aj pri vypnutom filtri, žiadny z registrových pokusov R237 nepomohol) spôsobuje práve vypnutý filter:
/// skutočné videá ho majú takmer vždy zapnutý.
pub fn h264_pcm_odblokovanie(snimky: &[Obraz], odblokovanie: bool) -> Vec<Vec<u8>> {
    let (sirka, vyska) = (snimky[0].sirka, snimky[0].vyska);
    assert!(sirka % 16 == 0 && vyska % 16 == 0);
    let (sps, pps) = h264_sps_pps(sirka, vyska);

    snimky
        .iter()
        .enumerate()
        .map(|(n, o)| {
            let mut au = Vec::new();
            if n == 0 {
                au.extend(nal(&[0x67], &sps.data));
                au.extend(nal(&[0x68], &pps.data));
            }
            let mut rez = Bity::default();
            rez.ue(0); // first_mb_in_slice
            rez.ue(7); // slice_type I (všetky rezy snímky)
            rez.ue(0); // pic_parameter_set_id
            rez.u(4, 0); // frame_num
            rez.ue((n % 2) as u64); // idr_pic_id – susedné IDR sa líšia
            rez.u(1, 0); // no_output_of_prior_pics_flag
            rez.u(1, 0); // long_term_reference_flag
            rez.se(0); // slice_qp_delta
            if odblokovanie {
                rez.ue(0); // disable_deblocking_filter_idc – filter zapnutý (pri PCM nič nezmení)
                rez.se(0); // slice_alpha_c0_offset_div2
                rez.se(0); // slice_beta_offset_div2
            } else {
                rez.ue(1); // disable_deblocking_filter_idc – filter vypnutý
            }
            let mut vzorky = Vec::with_capacity(384);
            for my in (0..vyska).step_by(16) {
                for mx in (0..sirka).step_by(16) {
                    rez.ue(25); // mb_type I_PCM
                    rez.nuly_do_zarovnania(); // pcm_alignment_zero_bit
                    vzorky.clear();
                    bloky(o, mx, my, 16, &mut vzorky);
                    rez.bajty(&vzorky);
                }
            }
            rez.koniec();
            au.extend(nal(&[0x65], &rez.data));
            au
        })
        .collect::<Vec<_>>()
        .s_koncom_h264()
}

/// R202: koniec toku H.264 – end_of_seq a end_of_stream: dekodér dostane štartovací kód za posledným rezom a
/// vie, že posledná snímka je celá (inak ju parser drží a čaká na ďalšie dáta).
trait KoniecToku {
    fn s_koncom_h264(self) -> Self;
    fn s_koncom_hevc(self) -> Self;
}

impl KoniecToku for Vec<Vec<u8>> {
    fn s_koncom_h264(mut self) -> Self {
        if let Some(posledna) = self.last_mut() {
            posledna.extend_from_slice(&[0, 0, 0, 1, 0x0A, 0, 0, 0, 1, 0x0B]);
        }
        self
    }

    fn s_koncom_hevc(mut self) -> Self {
        // EOS_NUT (36) a EOB_NUT (37): ovládač pri EOS vydá všetky snímky (sklenik_hevc.c), EOB ukončí EOS
        if let Some(posledna) = self.last_mut() {
            posledna.extend_from_slice(&[0, 0, 0, 1, 0x48, 0x01, 0, 0, 0, 1, 0x4A, 0x01]);
        }
        self
    }
}

/// R202: koeficient DC jasu bloku Intra 16×16 (CAVLC, jediný koeficient na pozícii 0), −8 až 8.
/// R255: pre nC (ITU-T H.264 9.2.1): 0 ≤ nC < 2 tabuľka 9-5 prvý stĺpec, nC ≥ 8 pevná dĺžka 6
/// bitov (TotalCoeff − 1 a TrailingOnes; „000011“ = žiadny koeficient). Iné nC tok R255 nepotrebuje (susedia sú
/// len Intra 16×16 bez AC – nN 0 – alebo I_PCM – nN 16).
fn dc_koeficient_nc(b: &mut Bity, c: i32, nc: u32) {
    assert!(!(2..8).contains(&nc), "nC {nc}");
    let pevna = nc >= 8;
    match c.unsigned_abs() {
        0 if pevna => b.u(6, 0b000011),
        0 => b.u(1, 1), // coeff_token: TotalCoeff 0
        1 => {
            if pevna {
                b.u(6, 0b000001); // TotalCoeff 1 (0000), TrailingOnes 1 (01)
            } else {
                b.u(2, 0b01); // coeff_token: TotalCoeff 1, TrailingOnes 1
            }
            b.u(1, u64::from(c < 0)); // trailing_ones_sign_flag
            b.u(1, 1); // total_zeros 0
        }
        a => {
            assert!(a <= 8);
            if pevna {
                b.u(6, 0b000000); // TotalCoeff 1, TrailingOnes 0
            } else {
                b.u(6, 0b000101); // coeff_token: TotalCoeff 1, TrailingOnes 0
            }
            // levelCode bez úpravy o 2 (prvá úroveň pri TrailingOnes < 3), suffixLength 0 → len level_prefix
            let kod = if c > 0 { 2 * c - 2 } else { -2 * c - 1 } - 2;
            b.u(kod as u32, 0);
            b.u(1, 1);
            b.u(1, 1); // total_zeros 0
        }
    }
}

/// R202: o koľko zmení bod koeficient DC `c` bloku Intra 16×16 pri QP 26 (ITU-T H.264 8.5.10 a 8.5.12):
/// Hadamard rozloží c do všetkých 16 blokov, dcY = (c·208 + 2) >> 2, potom (dcY + 32) >> 6 v každom bode.
fn posun_dc(c: i32) -> i32 {
    (((c * 208 + 2) >> 2) + 32) >> 6
}

/// R202: tok H.264 **bez PCM** – makrobloky Intra 16×16 s predikciou DC a jediným koeficientom DC (každý blok
/// inej jasnosti), farba bez zvyšku (128). Overí bežnú cestu dekodéra (predikcia, CAVLC, Hadamard,
/// inverzná transformácia) – dekodér hlavného jadra I_PCM nevie (0/8), tento tok ukáže, či vie aspoň toto.
/// Vráti toky a čakané obrazy (bit po bite, overuje FFmpeg v CI).
pub fn h264_dc(sirka: usize, vyska: usize, pocet: usize) -> (Vec<Vec<u8>>, Vec<Obraz>) {
    h264_dc_z(sirka, vyska, &(0..pocet).collect::<Vec<_>>(), false)
}

/// R249 (návrh B, 27. 11.): tok Intra 16×16 DC so snímkami s obsahom `obsah[i]` (rovnaké číslo = rovnaký obraz)
/// a voliteľne so SPS a PPS v samostatnej jednotke pred prvou snímkou. Rozlíši, či je na SM1 zlá vždy prvá
/// dekódovaná snímka, prvá po SPS, alebo prvá v buffri spolu so SPS a PPS.
pub fn h264_dc_z(
    sirka: usize,
    vyska: usize,
    obsah: &[usize],
    sps_zvlast: bool,
) -> (Vec<Vec<u8>>, Vec<Obraz>) {
    h264_dc_pcm(sirka, vyska, obsah, sps_zvlast, &|_, _| false)
}

/// R255 (návrh A, B súhlasí – #21): tok Intra 16×16 DC, v ktorom sú niektoré rady makroblokov I_PCM podľa
/// `pcm(poradie_snimky, rad_mb)`. Makroblok PCM má jednotný jas (28–227) a farbu 128; susedný blok DC ho
/// predikuje ako každého suseda. Rozlíši dve hypotézy o zlom prvom rade MB prvého obrazu na SM1:
/// (a) 1. obraz celý PCM, ďalšie DC – stačí, aby dekodér „prešiel“ jedným obrazom;
/// (b) 1. rad MB v PCM – chyba je v ceste Intra 16×16 horného radu pri prvom obraze.
/// Filter zapnutý s posunmi −12: pri hranách PCM–DC qPav = 13 → indexA 1 → α = 0, nič sa nezmení.
pub fn h264_dc_pcm(
    sirka: usize,
    vyska: usize,
    obsah: &[usize],
    sps_zvlast: bool,
    pcm: &dyn Fn(usize, usize) -> bool,
) -> (Vec<Vec<u8>>, Vec<Obraz>) {
    assert!(sirka.is_multiple_of(16) && vyska.is_multiple_of(16));
    let (sps, pps) = h264_sps_pps(sirka, vyska);
    let (sx, sy) = (sirka / 16, vyska / 16);
    let mut toky = Vec::new();
    let mut cakane = Vec::new();
    if sps_zvlast {
        let mut au = nal(&[0x67], &sps.data);
        au.extend(nal(&[0x68], &pps.data));
        toky.push(au);
    }
    for (poradie, &n) in obsah.iter().enumerate() {
        let mut au = Vec::new();
        if poradie == 0 && !sps_zvlast {
            au.extend(nal(&[0x67], &sps.data));
            au.extend(nal(&[0x68], &pps.data));
        }
        let mut rez = Bity::default();
        rez.ue(0); // first_mb_in_slice
        rez.ue(7); // slice_type I
        rez.ue(0); // pic_parameter_set_id
        rez.u(4, 0); // frame_num
        rez.ue((poradie % 2) as u64); // idr_pic_id – po sebe idúce IDR sa líšia (norma 7.4.3)
        rez.u(1, 0); // no_output_of_prior_pics_flag
        rez.u(1, 0); // long_term_reference_flag
        rez.se(0); // slice_qp_delta – QP 26
        // R242: filter zapnutý, posuny −12 → indexA = indexB = 26 − 12 = 14 < 16 → α = β = 0, filter nezmení nič
        // (norma 8.7.2.2, tabuľka 8-16). Vypnutý filter (idc 1) dekodér SM1 nezvláda – Odroid 27. 11.: s filtrom
        // bloky PCM presne, bez neho spodné 4 riadky každého radu makroblokov zlé.
        rez.ue(0); // disable_deblocking_filter_idc – filter zapnutý
        rez.se(-6); // slice_alpha_c0_offset_div2
        rez.se(-6); // slice_beta_offset_div2
        let mut jas = vec![0i32; sx * sy];
        for my in 0..sy {
            let rad_pcm = pcm(poradie, my);
            for mx in 0..sx {
                if rad_pcm {
                    let v = ((mx * 11 + my * 13 + n * 5) % 200 + 28) as u8;
                    rez.ue(25); // mb_type I_PCM
                    rez.nuly_do_zarovnania(); // pcm_alignment_zero_bit
                    let mut vzorky = vec![v; 256];
                    vzorky.extend_from_slice(&[128; 128]);
                    rez.bajty(&vzorky);
                    jas[my * sx + mx] = i32::from(v);
                    continue;
                }
                // nC (9.2.1): sused I_PCM má nN 16, Intra 16×16 bez AC 0; obaja → (nA + nB + 1) >> 1
                let n_radu = |y: usize| if pcm(poradie, y) { 16u32 } else { 0 };
                let na = (mx > 0).then(|| n_radu(my));
                let nb = (my > 0).then(|| n_radu(my - 1));
                let nc = match (na, nb) {
                    (Some(a), Some(b)) => (a + b + 1) >> 1,
                    (Some(x), None) | (None, Some(x)) => x,
                    (None, None) => 0,
                };
                let c = ((mx * 3 + my * 5 + n * 7) % 17) as i32 - 8;
                rez.ue(3); // mb_type I_16x16_2_0_0 – predikcia DC, bez AC a farby
                rez.ue(0); // intra_chroma_pred_mode DC
                rez.se(0); // mb_qp_delta
                dc_koeficient_nc(&mut rez, c, nc);
                let hore = (my > 0).then(|| jas[(my - 1) * sx + mx]);
                let vlavo = (mx > 0).then(|| jas[my * sx + mx - 1]);
                let pred = match (hore, vlavo) {
                    (Some(h), Some(v)) => (h + v + 1) >> 1,
                    (Some(x), None) | (None, Some(x)) => x,
                    (None, None) => 128,
                };
                jas[my * sx + mx] = (pred + posun_dc(c)).clamp(0, 255);
            }
        }
        rez.koniec();
        au.extend(nal(&[0x65], &rez.data));
        toky.push(au);
        let mut y = vec![0u8; sirka * vyska];
        for (i, b) in y.iter_mut().enumerate() {
            *b = jas[(i / sirka / 16) * sx + (i % sirka) / 16] as u8;
        }
        let farba = vec![128u8; sirka * vyska / 4];
        cakane.push(Obraz { sirka, vyska, y, cb: farba.clone(), cr: farba });
    }
    (toky.s_koncom_h264(), cakane)
}

/// Pravdepodobnostné kódovanie CABAC (ITU-T H.265, kap. 9.3.4.3 a 9.3.5) – len kóder, len to, čo tok PCM
/// potrebuje: binárne symboly s kontextom a ukončovací symbol s vyprázdnením.
struct Cabac {
    bity: Bity,
    low: u32,
    range: u32,
    prvy_bit: bool,
    odlozene: u32,
}

/// rangeTabLps[pStateIdx][qRangeIdx] (Tabuľka 9-46).
const RANGE_LPS: [[u8; 4]; 64] = [
    [128, 176, 208, 240],
    [128, 167, 197, 227],
    [128, 158, 187, 216],
    [123, 150, 178, 205],
    [116, 142, 169, 195],
    [111, 135, 160, 185],
    [105, 128, 152, 175],
    [100, 122, 144, 166],
    [95, 116, 137, 158],
    [90, 110, 130, 150],
    [85, 104, 123, 142],
    [81, 99, 117, 135],
    [77, 94, 111, 128],
    [73, 89, 105, 122],
    [69, 85, 100, 116],
    [66, 80, 95, 110],
    [62, 76, 90, 104],
    [59, 72, 86, 99],
    [56, 69, 81, 94],
    [53, 65, 77, 89],
    [51, 62, 73, 85],
    [48, 59, 69, 80],
    [46, 56, 66, 76],
    [43, 53, 63, 72],
    [41, 50, 59, 69],
    [39, 48, 56, 65],
    [37, 45, 54, 62],
    [35, 43, 51, 59],
    [33, 41, 48, 56],
    [32, 39, 46, 53],
    [30, 37, 43, 50],
    [29, 35, 41, 48],
    [27, 33, 39, 45],
    [26, 31, 37, 43],
    [24, 30, 35, 41],
    [23, 28, 33, 39],
    [22, 27, 32, 37],
    [21, 26, 30, 35],
    [20, 24, 29, 33],
    [19, 23, 27, 31],
    [18, 22, 26, 30],
    [17, 21, 25, 28],
    [16, 20, 23, 27],
    [15, 19, 22, 25],
    [14, 18, 21, 24],
    [14, 17, 20, 23],
    [13, 16, 19, 22],
    [12, 15, 18, 21],
    [12, 14, 17, 20],
    [11, 14, 16, 19],
    [11, 13, 15, 18],
    [10, 12, 15, 17],
    [10, 12, 14, 16],
    [9, 11, 13, 15],
    [9, 11, 12, 14],
    [8, 10, 12, 14],
    [8, 9, 11, 13],
    [7, 9, 11, 12],
    [7, 9, 10, 12],
    [7, 8, 10, 11],
    [6, 8, 9, 11],
    [6, 7, 9, 10],
    [6, 7, 8, 9],
    [2, 2, 2, 2],
];

/// transIdxLps (Tabuľka 9-47); transIdxMps je min(stav + 1, 62).
const PRECHOD_LPS: [u8; 64] = [
    0, 0, 1, 2, 2, 4, 4, 5, 6, 7, 8, 9, 9, 11, 11, 12, 13, 13, 15, 15, 16, 16, 18, 18, 19, 19, 21, 21, 22,
    22, 23, 24, 24, 25, 26, 26, 27, 27, 28, 29, 29, 30, 30, 30, 31, 32, 32, 33, 33, 33, 34, 34, 35, 35, 35,
    36, 36, 36, 37, 37, 37, 38, 38, 63,
];

/// Kontext: (stav pravdepodobnosti, hodnota pravdepodobnejšieho symbolu).
#[derive(Clone, Copy)]
struct Kontext {
    stav: u8,
    mps: u8,
}

impl Kontext {
    /// Počiatočný stav z hodnoty initValue a QP rezu (9.3.2.2).
    fn novy(init: u32, qp: i32) -> Kontext {
        let m = (init >> 4) as i32 * 5 - 45;
        let n = (((init & 15) << 3) as i32) - 16;
        let pre = (((m * qp.clamp(0, 51)) >> 4) + n).clamp(1, 126);
        if pre <= 63 {
            Kontext { stav: (63 - pre) as u8, mps: 0 }
        } else {
            Kontext { stav: (pre - 64) as u8, mps: 1 }
        }
    }
}

impl Cabac {
    fn novy(bity: Bity) -> Cabac {
        Cabac { bity, low: 0, range: 510, prvy_bit: true, odlozene: 0 }
    }

    fn put_bit(&mut self, b: u32) {
        if self.prvy_bit {
            self.prvy_bit = false;
        } else {
            self.bity.bit(b);
        }
        while self.odlozene > 0 {
            self.bity.bit(1 - b);
            self.odlozene -= 1;
        }
    }

    fn renorm(&mut self) {
        while self.range < 256 {
            if self.low < 256 {
                self.put_bit(0);
            } else if self.low >= 512 {
                self.low -= 512;
                self.put_bit(1);
            } else {
                self.low -= 256;
                self.odlozene += 1;
            }
            self.range <<= 1;
            self.low <<= 1;
        }
    }

    fn symbol(&mut self, k: &mut Kontext, b: u8) {
        let lps = RANGE_LPS[k.stav as usize][((self.range >> 6) & 3) as usize] as u32;
        self.range -= lps;
        if b != k.mps {
            self.low += self.range;
            self.range = lps;
            if k.stav == 0 {
                k.mps = 1 - k.mps;
            }
            k.stav = PRECHOD_LPS[k.stav as usize];
        } else {
            k.stav = (k.stav + 1).min(62);
        }
        self.renorm();
    }

    /// Ukončovací symbol (end_of_slice_segment_flag, pcm_flag); pri 1 sa kóder vyprázdni – posledný zapísaný
    /// bit je 1 a čítanie dekodéra skončí presne za ním.
    fn ukoncovaci(&mut self, b: bool) {
        self.range -= 2;
        if b {
            self.low += self.range;
            self.range = 2;
            self.renorm();
            self.put_bit((self.low >> 9) & 1);
            let posledne = ((self.low >> 7) & 3) | 1;
            self.bity.u(2, posledne as u64);
        } else {
            self.renorm();
        }
    }

    /// Znovuspustenie aritmetického kódera po vzorkách PCM (kontexty ostávajú).
    fn znova(&mut self) {
        self.low = 0;
        self.range = 510;
        self.prvy_bit = true;
        self.odlozene = 0;
    }
}

/// profile_tier_level pre profil Main, úroveň 3.1, jedna časová vrstva.
fn hevc_profil(b: &mut Bity) {
    b.u(2, 0); // general_profile_space
    b.u(1, 0); // general_tier_flag
    b.u(5, 1); // general_profile_idc – Main
    b.u(32, 0x6000_0000); // kompatibilita s Main a Main 10
    b.u(1, 1); // general_progressive_source_flag
    b.u(1, 0); // general_interlaced_source_flag
    b.u(1, 0); // general_non_packed_constraint_flag
    b.u(1, 1); // general_frame_only_constraint_flag
    b.u(44, 0); // rezervované
    b.u(8, 93); // general_level_idc – 3.1
}

/// Tok HEVC (Annex B) – po jednej prístupovej jednotke na snímku (prvá má aj VPS, SPS a PPS). Každá snímka je
/// IDR, kódovacie jednotky 16 × 16 s pcm_flag. Rozmery musia byť násobkom 16.
pub fn hevc_pcm(snimky: &[Obraz]) -> Vec<Vec<u8>> {
    hevc_gop(snimky, 1, None).0
}

/// Tok HEVC so skupinami snímok (R177): každá `gop`-tá snímka je IDR s blokmi PCM (obraz zo `snimky`), ostatné
/// sú **P-snímky, v ktorých je každá jednotka „preskočená“** (cu_skip_flag, zlučovací kandidát s nulovým
/// pohybom) – dekodér musí skopírovať predošlú snímku cez referenciu. Overí tak celú cestu referencií:
/// sadu RPS, zoznam L0, kompenzáciu pohybu, predikciu vektorov a správu snímok. Vráti toky a čakané obrazy.
/// `snimok_za_s` pridá do SPS časovanie (VUI) – dekodér podľa neho riadi takt.
pub fn hevc_gop(snimky: &[Obraz], gop: usize, snimok_za_s: Option<u32>) -> (Vec<Vec<u8>>, Vec<Obraz>) {
    let (sirka, vyska) = (snimky[0].sirka, snimky[0].vyska);
    assert!(sirka % 16 == 0 && vyska % 16 == 0);
    let mut vps = Bity::default();
    vps.u(4, 0); // vps_video_parameter_set_id
    vps.u(1, 1); // vps_base_layer_internal_flag
    vps.u(1, 1); // vps_base_layer_available_flag
    vps.u(6, 0); // vps_max_layers_minus1
    vps.u(3, 0); // vps_max_sub_layers_minus1
    vps.u(1, 1); // vps_temporal_id_nesting_flag
    vps.u(16, 0xFFFF);
    hevc_profil(&mut vps);
    vps.u(1, 1); // vps_sub_layer_ordering_info_present_flag
    vps.ue(1); // vps_max_dec_pic_buffering_minus1
    vps.ue(0); // vps_max_num_reorder_pics
    vps.ue(0); // vps_max_latency_increase_plus1
    vps.u(6, 0); // vps_max_layer_id
    vps.ue(0); // vps_num_layer_sets_minus1
    vps.u(1, 0); // vps_timing_info_present_flag
    vps.u(1, 0); // vps_extension_flag
    vps.koniec();

    let mut sps = Bity::default();
    sps.u(4, 0); // sps_video_parameter_set_id
    sps.u(3, 0); // sps_max_sub_layers_minus1
    sps.u(1, 1); // sps_temporal_id_nesting_flag
    hevc_profil(&mut sps);
    sps.ue(0); // sps_seq_parameter_set_id
    sps.ue(1); // chroma_format_idc – 4:2:0
    sps.ue(sirka as u64);
    sps.ue(vyska as u64);
    sps.u(1, 0); // conformance_window_flag
    sps.ue(0); // bit_depth_luma_minus8
    sps.ue(0); // bit_depth_chroma_minus8
    sps.ue(4); // log2_max_pic_order_cnt_lsb_minus4
    sps.u(1, 1); // sps_sub_layer_ordering_info_present_flag
    sps.ue(1); // sps_max_dec_pic_buffering_minus1
    sps.ue(0); // sps_max_num_reorder_pics
    sps.ue(0); // sps_max_latency_increase_plus1
    sps.ue(1); // log2_min_luma_coding_block_size_minus3 – 16
    sps.ue(0); // log2_diff_max_min_luma_coding_block_size – CTB 16
    sps.ue(0); // log2_min_luma_transform_block_size_minus2
    sps.ue(2); // log2_diff_max_min_luma_transform_block_size
    sps.ue(0); // max_transform_hierarchy_depth_inter
    sps.ue(0); // max_transform_hierarchy_depth_intra
    sps.u(1, 0); // scaling_list_enabled_flag
    sps.u(1, 0); // amp_enabled_flag
    sps.u(1, 0); // sample_adaptive_offset_enabled_flag
    sps.u(1, 1); // pcm_enabled_flag
    sps.u(4, 7); // pcm_sample_bit_depth_luma_minus1
    sps.u(4, 7); // pcm_sample_bit_depth_chroma_minus1
    sps.ue(1); // log2_min_pcm_luma_coding_block_size_minus3 – 16
    sps.ue(0); // log2_diff_max_min_pcm_luma_coding_block_size
    sps.u(1, 1); // pcm_loop_filter_disabled_flag
    sps.ue(0); // num_short_term_ref_pic_sets
    sps.u(1, 0); // long_term_ref_pics_present_flag
    sps.u(1, 0); // sps_temporal_mvp_enabled_flag
    sps.u(1, 0); // strong_intra_smoothing_enabled_flag
    match snimok_za_s {
        Some(f) => {
            sps.u(1, 1); // vui_parameters_present_flag
            sps.u(4, 0); // aspect_ratio, overscan, video_signal_type, chroma_loc – nie
            sps.u(3, 0); // neutral_chroma_indication, field_seq, frame_field_info
            sps.u(1, 0); // default_display_window_flag
            sps.u(1, 1); // vui_timing_info_present_flag
            sps.u(32, 1); // vui_num_units_in_tick
            sps.u(32, f as u64); // vui_time_scale
            sps.u(1, 0); // vui_poc_proportional_to_timing_flag
            sps.u(1, 0); // vui_hrd_parameters_present_flag
            sps.u(1, 0); // bitstream_restriction_flag
        }
        None => sps.u(1, 0), // vui_parameters_present_flag
    }
    sps.u(1, 0); // sps_extension_present_flag
    sps.koniec();

    let mut pps = Bity::default();
    pps.ue(0); // pps_pic_parameter_set_id
    pps.ue(0); // pps_seq_parameter_set_id
    pps.u(1, 0); // dependent_slice_segments_enabled_flag
    pps.u(1, 0); // output_flag_present_flag
    pps.u(3, 0); // num_extra_slice_header_bits
    pps.u(1, 0); // sign_data_hiding_enabled_flag
    pps.u(1, 0); // cabac_init_present_flag
    pps.ue(0); // num_ref_idx_l0_default_active_minus1
    pps.ue(0); // num_ref_idx_l1_default_active_minus1
    pps.se(0); // init_qp_minus26
    pps.u(1, 0); // constrained_intra_pred_flag
    pps.u(1, 0); // transform_skip_enabled_flag
    pps.u(1, 0); // cu_qp_delta_enabled_flag
    pps.se(0); // pps_cb_qp_offset
    pps.se(0); // pps_cr_qp_offset
    pps.u(1, 0); // pps_slice_chroma_qp_offsets_present_flag
    pps.u(1, 0); // weighted_pred_flag
    pps.u(1, 0); // weighted_bipred_flag
    pps.u(1, 0); // transquant_bypass_enabled_flag
    pps.u(1, 0); // tiles_enabled_flag
    pps.u(1, 0); // entropy_coding_sync_enabled_flag
    pps.u(1, 0); // pps_loop_filter_across_slices_enabled_flag
    pps.u(1, 1); // deblocking_filter_control_present_flag
    pps.u(1, 0); // deblocking_filter_override_enabled_flag
    pps.u(1, 1); // pps_deblocking_filter_disabled_flag
    pps.u(1, 0); // pps_scaling_list_data_present_flag
    pps.u(1, 0); // lists_modification_present_flag
    pps.ue(0); // log2_parallel_merge_level_minus2
    pps.u(1, 0); // slice_segment_header_extension_present_flag
    pps.u(1, 0); // pps_extension_present_flag
    pps.koniec();

    // QP rezu 26; part_mode (rez I, initType 0): initValue 184; cu_skip_flag (rez P, initType 1): 197, 185, 201
    let qp = 26;
    let (sx, sy) = (sirka / 16, vyska / 16);
    let mut toky = Vec::with_capacity(snimky.len());
    let mut cakane = Vec::with_capacity(snimky.len());
    for (n, o) in snimky.iter().enumerate() {
        let mut au = Vec::new();
        if n == 0 {
            au.extend(nal(&[0x40, 0x01], &vps.data));
            au.extend(nal(&[0x42, 0x01], &sps.data));
            au.extend(nal(&[0x44, 0x01], &pps.data));
        }
        let poc = n % gop.max(1);
        let mut rez = Bity::default();
        rez.u(1, 1); // first_slice_segment_in_pic_flag
        if poc == 0 {
            rez.u(1, 0); // no_output_of_prior_pics_flag
            rez.ue(0); // slice_pic_parameter_set_id
            rez.ue(2); // slice_type I
        } else {
            rez.ue(0); // slice_pic_parameter_set_id
            rez.ue(1); // slice_type P
            rez.u(8, poc as u64); // slice_pic_order_cnt_lsb
            rez.u(1, 0); // short_term_ref_pic_set_sps_flag – sada priamo v reze:
            rez.ue(1); // num_negative_pics
            rez.ue(0); // num_positive_pics
            rez.ue(0); // delta_poc_s0_minus1 – predošlá snímka
            rez.u(1, 1); // used_by_curr_pic_s0_flag
            rez.u(1, 0); // num_ref_idx_active_override_flag
            rez.ue(4); // five_minus_max_num_merge_cand – jeden kandidát (merge_idx sa nekóduje)
        }
        rez.se(0); // slice_qp_delta
        rez.koniec(); // byte_alignment(): bit 1 a nuly
        let mut c = Cabac::novy(rez);
        if poc == 0 {
            let mut part_mode = Kontext::novy(184, qp);
            let mut vzorky = Vec::with_capacity(384);
            for i in 0..sx * sy {
                let (x, y) = (i % sx * 16, i / sx * 16);
                c.symbol(&mut part_mode, 1); // PART_2Nx2N
                c.ukoncovaci(true); // pcm_flag
                c.bity.nuly_do_zarovnania(); // pcm_alignment_zero_bit
                vzorky.clear();
                bloky(o, x, y, 16, &mut vzorky);
                c.bity.bajty(&vzorky);
                c.znova();
                c.ukoncovaci(i + 1 == sx * sy); // end_of_slice_segment_flag
            }
            cakane.push(o.clone());
        } else {
            let mut skip = [Kontext::novy(197, qp), Kontext::novy(185, qp), Kontext::novy(201, qp)];
            for i in 0..sx * sy {
                // kontext podľa susedov vľavo a hore (obaja sú preskočení, ak existujú)
                let ctx = usize::from(i % sx > 0) + usize::from(i / sx > 0);
                c.symbol(&mut skip[ctx], 1); // cu_skip_flag
                c.ukoncovaci(i + 1 == sx * sy); // end_of_slice_segment_flag
            }
            cakane.push(cakane[n - 1].clone());
        }
        // posledný bit vyprázdnenia je rbsp_stop_one_bit, ďalej len zarovnanie nulami
        c.bity.nuly_do_zarovnania();
        let hlavicka = if poc == 0 { [0x28, 0x01] } else { [0x02, 0x01] }; // IDR_N_LP / TRAIL_R
        au.extend(nal(&hlavicka, &c.bity.data));
        toky.push(au);
    }
    (toky.s_koncom_hevc(), cakane)
}

#[cfg(test)]
mod testy {
    #[test]
    fn r237_povod_chybneho_riadku() {
        let snimky: Vec<Obraz> = (0..3).map(|n| vzor(64, 48, n)).collect();
        // riadok 12 3. snímky v skutočnosti z 2. snímky (zápis do starého bufra)
        let riadok = &snimky[1].y[12 * 64..13 * 64];
        assert_eq!(povod_riadku(&snimky, riadok), "s2 r12");
        assert_eq!(povod_riadku(&snimky, &[0u8; 64]), "všade 0");
        let mut iny = riadok.to_vec();
        iny[3] ^= 0x55;
        assert_eq!(povod_riadku(&snimky, &iny), "?");
    }

    #[test]
    fn r237_chybne_riadky_makroblokov() {
        // 320×240: spodné 4 riadky každého radu makroblokov zlé (ako X96 26. 11.)
        let dobry = vzor(320, 240, 0);
        let mut zly = dobry.y.clone();
        for r in (0..240).filter(|r| r % 16 >= 12) {
            zly[r * 320 + 5] ^= 0x40;
        }
        assert_eq!(chybne_riadky_bloku(&dobry.y, &zly, 320, 16), "12–15");
        assert_eq!(chybne_riadky_bloku(&dobry.y, &dobry.y, 320, 16), "žiadne");
        // jediný chybný riadok obrazu nie je vzorec
        let mut jeden = dobry.y.clone();
        jeden[100 * 320] ^= 1;
        assert_eq!(chybne_riadky_bloku(&dobry.y, &jeden, 320, 16), "žiadne");
        let mut dva = dobry.y.clone();
        for r in (0..240).filter(|r| r % 16 == 0 || r % 16 == 7) {
            dva[r * 320] ^= 1;
        }
        assert_eq!(chybne_riadky_bloku(&dobry.y, &dva, 320, 16), "0,7");
    }

    use super::*;

    #[test]
    fn exp_golomb_a_ochrana_pred_emulaciou() {
        let mut b = Bity::default();
        b.ue(0);
        b.ue(1);
        b.ue(2);
        b.ue(25);
        b.se(-1);
        b.koniec();
        // 1 010 011 000011010 011 1(stop) → 1010 0110 0001 1010 0111 + zarovnanie
        assert_eq!(b.data, vec![0b1010_0110, 0b0001_1010, 0b0111_0000]);
        assert_eq!(
            nal(&[0x65], &[0, 0, 1, 0, 0, 0, 0, 0, 3]),
            vec![0, 0, 0, 1, 0x65, 0, 0, 3, 1, 0, 0, 3, 0, 0, 3, 0, 3]
        );
    }

    #[test]
    fn kontext_part_mode_pre_rez_i() {
        let k = Kontext::novy(184, 26);
        assert_eq!((k.stav, k.mps), (0, 1));
        // initValue 154 je neutrálny (m = 0, n = 64) – stav 0 s MPS 1 pri každom QP
        let k = Kontext::novy(154, 26);
        assert_eq!((k.stav, k.mps), (0, 1));
    }

    #[test]
    fn vzor_je_rozny_pre_body_riadky_a_snimky() {
        let a = vzor(64, 32, 0);
        let b = vzor(64, 32, 1);
        assert_ne!(a.y, b.y);
        assert_ne!(a.cb, b.cb);
        assert_ne!(a.cr, b.cr);
        assert!(a.y.contains(&0) && a.y.iter().any(|&v| v > 200));
        let (chyb, prva) = porovnaj(&a, &a.y, &a.cb, &a.cr);
        assert_eq!((chyb, prva), (0, None));
        let mut y = a.y.clone();
        y[64 * 3 + 5] ^= 1;
        let (chyb, prva) = porovnaj(&a, &y, &a.cb, &a.cr);
        assert_eq!(chyb, 1);
        assert_eq!(prva.map(|p| (p.0, p.1, p.2)), Some(("Y", 3, 5)));
    }

    #[test]
    fn toky_maju_spravnu_stavbu() {
        let snimky: Vec<Obraz> = (0..2).map(|n| vzor(64, 48, n)).collect();
        let h = h264_pcm(&snimky);
        assert_eq!(h.len(), 2);
        assert_eq!(&h[0][..5], &[0, 0, 0, 1, 0x67]);
        let e = hevc_pcm(&snimky);
        assert_eq!(e.len(), 2);
        assert_eq!(&e[0][..6], &[0, 0, 0, 1, 0x40, 0x01]);
        // druhá snímka je len rez (bez VPS/SPS/PPS)
        assert_eq!(&e[1][..6], &[0, 0, 0, 1, 0x28, 0x01]);
        // PCM: aspoň vzorky všetkých bodov (1,5 B na bod)
        assert!(e[1].len() > 64 * 48 * 3 / 2);
    }

    /// Ručné overenie nezávislým dekodérom: `cargo test -p sklenik-klic -- --ignored zapis_skusobne_toky`
    /// zapíše toky a čakané obrazy do adresára z premennej SKLENIK_VIDEO (inak do dočasného adresára).
    #[test]
    #[ignore]
    fn zapis_skusobne_toky() {
        let dir =
            std::env::var("SKLENIK_VIDEO").unwrap_or_else(|_| std::env::temp_dir().display().to_string());
        let snimky: Vec<Obraz> = (0..3).map(|n| vzor(320, 240, n)).collect();
        std::fs::write(format!("{dir}/pcm.h264"), h264_pcm(&snimky).concat()).unwrap();
        std::fs::write(
            format!("{dir}/pcm_filter_koniec.h264"),
            s_koncom_toku(h264_pcm_odblokovanie(&snimky, true)).concat(),
        )
        .unwrap();
        std::fs::write(format!("{dir}/pcm_filter.h264"), h264_pcm_odblokovanie(&snimky, true).concat())
            .unwrap();
        std::fs::write(format!("{dir}/pcm.hevc"), hevc_pcm(&snimky).concat()).unwrap();
        let mut yuv = Vec::new();
        for o in &snimky {
            yuv.extend_from_slice(&o.y);
            yuv.extend_from_slice(&o.cb);
            yuv.extend_from_slice(&o.cr);
        }
        std::fs::write(format!("{dir}/pcm.yuv"), yuv).unwrap();
        // R202: H.264 bez PCM – Intra 16×16, predikcia DC a koeficienty DC
        let (toky, cakane) = h264_dc(320, 240, 3);
        std::fs::write(format!("{dir}/dc.h264"), toky.concat()).unwrap();
        let mut yuv = Vec::new();
        for o in &cakane {
            yuv.extend_from_slice(&o.y);
            yuv.extend_from_slice(&o.cb);
            yuv.extend_from_slice(&o.cr);
        }
        std::fs::write(format!("{dir}/dc.yuv"), yuv).unwrap();
        // R249: SPS+PPS zvlášť a prvá snímka dvakrát
        for (meno, obsah, zvlast) in
            [("dc_sps_zvlast", vec![0, 1, 2], true), ("dc_prva_dvakrat", vec![0, 0, 1, 2], false)]
        {
            let (toky, cakane) = h264_dc_z(320, 240, &obsah, zvlast);
            std::fs::write(format!("{dir}/{meno}.h264"), toky.concat()).unwrap();
            let mut yuv = Vec::new();
            for o in &cakane {
                yuv.extend_from_slice(&o.y);
                yuv.extend_from_slice(&o.cb);
                yuv.extend_from_slice(&o.cr);
            }
            std::fs::write(format!("{dir}/{meno}.yuv"), yuv).unwrap();
        }
        // R255: 1. obraz celý PCM, ďalšie DC; 1. rad makroblokov PCM (susedia DC s nC 8 a 16 – kód pevnej dĺžky)
        let prvy_pcm: &dyn Fn(usize, usize) -> bool = &|p, _| p == 0;
        let rad_pcm: &dyn Fn(usize, usize) -> bool = &|_, r| r == 0;
        for (meno, maska) in [("dc_prvy_pcm", prvy_pcm), ("dc_rad_pcm", rad_pcm)] {
            let (toky, cakane) = h264_dc_pcm(320, 240, &[0, 1, 2], false, maska);
            std::fs::write(format!("{dir}/{meno}.h264"), toky.concat()).unwrap();
            let mut yuv = Vec::new();
            for o in &cakane {
                yuv.extend_from_slice(&o.y);
                yuv.extend_from_slice(&o.cb);
                yuv.extend_from_slice(&o.cr);
            }
            std::fs::write(format!("{dir}/{meno}.yuv"), yuv).unwrap();
        }
        // R177: skupiny po 4 snímkach – IDR s PCM a tri P-snímky s preskočenými jednotkami, 25 snímok/s
        let obrazy: Vec<Obraz> = (0..10).map(|n| vzor(320, 240, n)).collect();
        let (toky, cakane) = hevc_gop(&obrazy, 4, Some(25));
        std::fs::write(format!("{dir}/gop.hevc"), toky.concat()).unwrap();
        let mut yuv = Vec::new();
        for o in &cakane {
            yuv.extend_from_slice(&o.y);
            yuv.extend_from_slice(&o.cb);
            yuv.extend_from_slice(&o.cr);
        }
        std::fs::write(format!("{dir}/gop.yuv"), yuv).unwrap();
    }
}
