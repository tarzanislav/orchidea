// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * SKLENÍK – vlastný dekodér HEVC (H.265) pre jadro HEVC čipu Amlogic SM1 (S905X3), R174.
 *
 * Hlavné jadro Linuxu má v ovládači meson-vdec jadro HEVC (spoločné s VP9), ale samotný kodek HEVC v ňom
 * nie je. Tento súbor je vlastný kodek SKLENÍKA: riadi mikrokód dekodéra (sm1_hevc_mmu.bin z linux-firmware)
 * cez jeho poštové registre, vedie zoznam snímok (DPB), referencie, poradie výstupu a nastavuje bloky
 * jadra HEVC (predikcia pohybu MPRED, kompenzácia pohybu MPP, odblokovanie DBLK, SAO a zápis NV12).
 *
 * Postup mikrokódu (zistený z dokumentácie registrov a verejných zdrojov Amlogic, napísaný nanovo):
 *   1. mikrokód hľadá jednotky NAL; o každej sa pýta (HEVC_NAL_SEARCH_DONE) – odpoveď: spracuj VPS, SPS,
 *      PPS, hlavičku rezu, alebo zahoď,
 *   2. po hlavičke rezu (HEVC_SLICE_SEGMENT_DONE) zapíše jej polia do pamäte RPM; ovládač určí POC, sadu
 *      referencií, zoznamy L0/L1, kam sa zapíše snímka a nastaví bloky – potom povie „dekóduj rez“,
 *   3. koniec toku (NAL EOS) vydá všetky snímky.
 *
 * Čo robí navyše pre chlad (to, čo pôvodný ovládač výrobcu nerobí):
 *   - **takt podľa potreby** – jadro HEVC beží na 666 MHz aj pri videu, ktoré by stačilo dekódovať pri
 *     menšom takte; strom hodín tiká stále (DOS_GCLK_EN3). Ovládač meria čas dekódovania každej snímky
 *     a takt znižuje, kým je jadro vyťažené menej ako na 70 % (s rezervou); pri náročnejšej scéne hneď
 *     zvýši. Takt sa mení len medzi snímkami pri zastavených hodinách (bez zákmitov),
 *   - **žiadne budenie navyše** – žiadne vlákno, ktoré by kontrolovalo buffre (prívod toku sa riadi počtom
 *     voľných snímok v esparseri); pri chýbajúcom buffri sa skúša znova až po 10 ms,
 *   - **8-bitové snímky priamo do NV12** (dvojitý zápis bez komprimovaných kópií) – menej toku pamäte.
 *
 * Zatiaľ len 8-bitové video (Main); 10-bitové (Main 10) potrebuje komprimované referencie – ďalší krok.
 */

#include <linux/clk.h>
#include <linux/delay.h>
#include <linux/ktime.h>
#include <linux/slab.h>
#include <linux/workqueue.h>
#include <media/v4l2-mem2mem.h>
#include <media/videobuf2-dma-contig.h>

#include "vdec_helpers.h"
#include "hevc_regs.h"
#include "dos_regs.h"
#include "codec_hevc_common.h"
#include "esparser.h"
#include "sklenik_hevc.h"

/* poštové registre mikrokódu (HEVC_ASSIST_SCRATCH_x) */
#define SKL_STAV		HEVC_ASSIST_SCRATCH_0
#define SKL_RPM			HEVC_ASSIST_SCRATCH_1
#define SKL_RPS			HEVC_ASSIST_SCRATCH_2
#define SKL_VPS			HEVC_ASSIST_SCRATCH_3
#define SKL_SPS			HEVC_ASSIST_SCRATCH_4
#define SKL_PPS			HEVC_ASSIST_SCRATCH_5
#define SKL_SAO_HORE		HEVC_ASSIST_SCRATCH_6
#define SKL_SAO_JEDNOTKA	HEVC_ASSIST_SCRATCH_9
#define SKL_SAO_NAD		HEVC_ASSIST_SCRATCH_A
#define SKL_SAO_VB_VELKOST	HEVC_ASSIST_SCRATCH_B
#define SKL_SAO_VB		HEVC_ASSIST_SCRATCH_C
#define SKL_SCALELUT		HEVC_ASSIST_SCRATCH_D
#define SKL_CAKANIE		HEVC_ASSIST_SCRATCH_E
#define SKL_LMEM		HEVC_ASSIST_SCRATCH_F
#define SKL_LADENIE		HEVC_ASSIST_SCRATCH_G
#define SKL_HLADANIE_NAL	HEVC_ASSIST_SCRATCH_I
#define SKL_TYP_NAL		HEVC_ASSIST_SCRATCH_J	/* čítanie: typ NAL; pred štartom: režim */
#define SKL_STOP_POZICIA	HEVC_ASSIST_SCRATCH_K
#define SKL_VELKOST_TOKU	HEVC_ASSIST_SCRATCH_N

/* registre, ktoré hlavička jadra nemá (adresy podľa mapy registrov DOS, slovo × 4) */
#define SKL_MBOX0_CLR		0xc1c4
#define SKL_MBOX0_MASK		0xc1c8
#define AMRISC_HLAVNA_ZIADOST	0x04

/* stavy mikrokódu */
#define ST_VPS			0x01
#define ST_SPS			0x02
#define ST_PPS			0x03
#define ST_HLAVICKA_REZU	0x04
#define ST_DATA_REZU		0x05
#define ST_SEI			0x0c
#define ST_SEI_HOTOVE		0x0d
#define ST_REZ_PRECITANY	0x08
#define ST_NAL_NAJDENY		0x09
#define ST_PREDEKODOVANE	0x0f
#define ST_PRILIS_VELKY		0x23
#define ST_ZAHOD_NAL		0xf0
#define ST_HOTOVO		0xff

/* typy jednotiek NAL (ITU-T H.265, tab. 7-1) */
#define NAL_RADL_N		6
#define NAL_RASL_R		9
#define NAL_BLA_W_LP		16
#define NAL_IDR_W_RADL		19
#define NAL_IDR_N_LP		20
#define NAL_CRA			21
#define NAL_VPS			32
#define NAL_SPS			33
#define NAL_PPS			34
#define NAL_EOS			36

#define REZ_B			0
#define REZ_P			1
#define REZ_I			2

#define SKL_SNIMOK		24	/* najviac snímok (buffrov) */
#define SKL_REF			16
#define SKL_POC_ZIADNY		INT_MIN

/* pracovná pamäť mikrokódu (veľkosti pre 4K) */
#define PP_IPP			0x4000
#define PP_SAO_NAD		0x30000
#define PP_SAO_VB		0x30000
#define PP_RPS			0x800
#define PP_VPS			0x800
#define PP_SPS			0x800
#define PP_PPS			0x2000
#define PP_SAO_HORE		0x2800
#define PP_SCALELUT		0x8000
#define PP_DBLK_PARA		0x20000
#define PP_DBLK_DATA		0x80000
#define PP_DBLK_DATA2		0x80000
#define PP_MPRED_NAD		0x8000
#define PP_RPM			0x100
#define PP_LMEM			0xa00

#define O_IPP		0
#define O_SAO_NAD	(O_IPP + PP_IPP)
#define O_SAO_VB	(O_SAO_NAD + PP_SAO_NAD)
#define O_RPS		(O_SAO_VB + PP_SAO_VB)
#define O_VPS		(O_RPS + PP_RPS)
#define O_SPS		(O_VPS + PP_VPS)
#define O_PPS		(O_SPS + PP_SPS)
#define O_SAO_HORE	(O_PPS + PP_PPS)
#define O_SCALELUT	(O_SAO_HORE + PP_SAO_HORE)
#define O_DBLK_PARA	(O_SCALELUT + PP_SCALELUT)
#define O_DBLK_DATA	(O_DBLK_PARA + PP_DBLK_PARA)
#define O_DBLK_DATA2	(O_DBLK_DATA + PP_DBLK_DATA)
#define O_MPRED_NAD	(O_DBLK_DATA2 + PP_DBLK_DATA2)
#define O_RPM		(O_MPRED_NAD + PP_MPRED_NAD)
#define O_LMEM		(O_RPM + PP_RPM)
#define PP_SPOLU	ALIGN(O_LMEM + PP_LMEM, SZ_64K)

/* pohybové vektory jednej snímky (zarovnané na 64 KiB) */
#define MV_FULL_HD	(0x40000 + SZ_64K)
#define MV_4K		(0x120000 + SZ_64K)

/* takty jadra HEVC, ktoré dávajú delidlá pevných PLL čipu (MHz) */
static const u32 skl_takty[] = { 666, 500, 400, 333, 250, 200, 166 };
#define TAKT_OKNO	30	/* po koľkých snímkach sa takt prehodnotí */

/*
 * Polia hlavičky rezu, ktoré mikrokód zapíše do pamäte RPM (128 slov po 16 bitoch). Poradie je dané
 * mikrokódom – nemeniť.
 */
struct skl_rpm {
	u16 rps[16];		/* sada krátkodobých referencií: bit 15 koniec, 14 použitá, 13 záporná, 12:0 delta */
	u16 ref_l0;
	u16 ref_l1;
	u16 typ_rezu;
	u16 tmvp;
	u16 zavisly_rez;
	u16 adresa_rezu;
	u16 riadkov_dlazdic_minus1;
	u16 sirka;
	u16 vyska;
	u16 log2_min_cb_minus3;
	u16 log2_rozdiel_cb;
	u16 log2_max_poc_lsb_minus4;
	u16 poc_lsb;
	u16 col_z_l0;
	u16 col_ref;
	u16 log2_paralelny_merge;
	u16 pat_minus_merge;
	u16 preusporiadanie;
	u16 zmena_zoznamov;
	u16 dlazdice;
	u16 stlpcov_dlazdic_minus1;
	u16 riadkov_dlazdic2_minus1;
	u16 sirka_dlazdice[8];
	u16 vyska_dlazdice[8];
	u16 priznaky;		/* misc_flag0 */
	u16 pps_beta;
	u16 pps_tc;
	u16 rez_beta;
	u16 rez_tc;
	u16 pps_cb_qp;
	u16 pps_cr_qp;
	u16 prvy_rez;
	u16 docasna_vrstva;	/* TemporalId + 1 */
	u16 typ_nal;
	u16 vui_tick_hore;
	u16 vui_tick_dole;
	u16 vui_skala_hore;
	u16 vui_skala_dole;
	u16 bitova_hlbka;	/* 3:0 jas − 8, 7:4 farba − 8 */
	u16 profil;
	u16 sei_pole;
	u16 signal;
	u16 zoznam_zmien[32];
	u16 orezanie;
	u16 orez[4];
	u16 chroma;
	u16 farby;
	u16 pomer_idc;
	u16 sar_sirka;
	u16 sar_vyska;
	u16 max_dpb_minus1;
	u16 zvysok[13];
};

static_assert(sizeof(struct skl_rpm) == 128 * 2);
static_assert(offsetof(struct skl_rpm, zoznam_zmien) == 0x48 * 2);
static_assert(offsetof(struct skl_rpm, max_dpb_minus1) == 0x72 * 2);

/* príznaky v misc_flag0 */
#define F_PCM_BEZ_FILTRA	BIT(0)
#define F_PCM			BIT(1)
#define F_FILTER_CEZ_DLAZDICE	BIT(2)
#define F_PPS_FILTER_CEZ_REZY	BIT(3)
#define F_PREPIS_POVOLENY	BIT(4)
#define F_PPS_BEZ_ODBLOKOVANIA	BIT(5)
#define F_PREPIS		BIT(6)
#define F_REZ_BEZ_ODBLOKOVANIA	BIT(7)
#define F_SAO_JAS		BIT(8)
#define F_SAO_FARBA		BIT(9)
#define F_REZ_FILTER_CEZ_REZY	BIT(10)

/* snímka v zozname dekodéra (podľa indexu buffra V4L2) */
struct skl_snimka {
	struct vb2_v4l2_buffer *vbuf;	/* NULL – miesto voľné */
	int poc;
	u32 poradie;			/* poradie dekódovania */
	u32 posun;			/* poloha v toku (pre časovú značku) */
	bool referencna;
	bool na_vystup;			/* dekódovaná, čaká na výstup */
	bool vydana;			/* odovzdaná programu */
	bool dekoduje_sa;
	u32 sirka, vyska;
	dma_addr_t mv;
	void *mv_virt;
	u32 mv_velkost;
	int l0[SKL_REF], l1[SKL_REF];
	u8 typ_rezu;
};

struct skl_hevc {
	struct amvdec_session *sess;
	struct mutex zamok;	/* stav dekodéra – vlákno prerušenia, resume a opakovanie */
	struct delayed_work opakuj;

	void *pp_virt;
	dma_addr_t pp;

	struct skl_rpm rpm;
	bool rpm_cakajuci;	/* hlavička rezu čaká na buffre (zmena formátu) alebo na voľnú snímku */
	bool cakame_format;
	bool cakame_snimku;

	bool ma_vps, ma_sps, ma_pps, ma_start;
	bool zoznam_nastaveny;	/* tabuľka adries snímok (ANC2AXI) zapísaná */

	struct skl_snimka snimky[SKL_SNIMOK];
	struct skl_snimka *aktualna;
	struct skl_snimka *col;

	u32 sirka, vyska;
	u32 lcu, lcu_log2, lcu_x, lcu_y;
	int poc, poc_predch_tid0, poc_nahodny_pristup;
	u32 poradie;
	u32 preusporiadanie;
	bool nova_snimka;
	bool dalsi_segment;
	u32 adresa_rezu;
	u8 typ_rezu;
	u8 ref_l0, ref_l1;

	/* takt podľa potreby */
	int takt_index;
	int takt_ziadany;
	ktime_t dekod_od;
	bool meranie_platne;	/* ďalšia snímka už bola v toku – čas je čisté dekódovanie bez čakania */
	u64 dekod_ns;
	u32 dekod_snimok;
	u64 perioda_ns;
	u32 vytazenie;		/* posledné zhodnotené vyťaženie (%) */

	/* štatistika (vypíše sa pri zastavení) */
	u32 vydanych, dekodovanych, chyb_ref;
	u64 dekod_ns_spolu;
	/* len snímky, pri ktorých už bol v toku ďalší kus (čisté dekódovanie bez čakania na program) */
	u32 dekodovanych_platne;
	u64 dekod_ns_platne;
};

static inline struct amvdec_core *jadro(struct skl_hevc *h)
{
	return h->sess->core;
}

static void zapis(struct skl_hevc *h, u32 reg, u32 val)
{
	amvdec_write_dos(jadro(h), reg, val);
}

static u32 citaj(struct skl_hevc *h, u32 reg)
{
	return amvdec_read_dos(jadro(h), reg);
}

/* povie mikrokódu, čo ďalej */
static void odpovedz(struct skl_hevc *h, u32 stav)
{
	zapis(h, SKL_STAV, stav);
	zapis(h, HEVC_MCPU_INTR_REQ, AMRISC_HLAVNA_ZIADOST);
}

static void skl_opakuj(struct work_struct *w);
static u32 skl_cakajuce(struct amvdec_session *sess);

/* ---------- pracovná pamäť a štart ---------- */

static void skl_nastav_pracovnu_pamat(struct skl_hevc *h)
{
	dma_addr_t a = h->pp;

	zapis(h, HEVCD_IPP_LINEBUFF_BASE, a + O_IPP);
	zapis(h, SKL_RPM, a + O_RPM);
	zapis(h, SKL_RPS, a + O_RPS);
	zapis(h, SKL_VPS, a + O_VPS);
	zapis(h, SKL_SPS, a + O_SPS);
	zapis(h, SKL_PPS, a + O_PPS);
	zapis(h, SKL_SAO_HORE, a + O_SAO_HORE);
	zapis(h, SKL_SCALELUT, a + O_SCALELUT);
	zapis(h, HEVC_DBLK_CFG4, a + O_DBLK_PARA);
	zapis(h, HEVC_DBLK_CFG5, a + O_DBLK_DATA);
	zapis(h, HEVC_DBLK_CFGE, a + O_DBLK_DATA2);
	zapis(h, SKL_LMEM, a + O_LMEM);
}

static int skl_start(struct amvdec_session *sess)
{
	struct amvdec_core *core = sess->core;
	struct skl_hevc *h;
	u32 v;
	int i;

	h = kzalloc(sizeof(*h), GFP_KERNEL);
	if (!h)
		return -ENOMEM;
	h->sess = sess;
	mutex_init(&h->zamok);
	INIT_DELAYED_WORK(&h->opakuj, skl_opakuj);
	h->pp_virt = dma_alloc_coherent(core->dev, PP_SPOLU, &h->pp, GFP_KERNEL);
	if (!h->pp_virt) {
		kfree(h);
		return -ENOMEM;
	}
	h->poc = SKL_POC_ZIADNY;
	h->poc_nahodny_pristup = INT_MAX;
	h->takt_index = 0;
	h->takt_ziadany = 0;
	for (i = 0; i < SKL_SNIMOK; i++)
		h->snimky[i].poc = SKL_POC_ZIADNY;

	skl_nastav_pracovnu_pamat(h);

	/* parser a posúvač toku: hľadanie štartovacích kódov a ochrany proti emulácii */
	zapis(h, DOS_SW_RESET3, BIT(14));
	zapis(h, DOS_SW_RESET3, 0);
	amvdec_clear_dos_bits(core, HEVC_CABAC_CONTROL, BIT(0));
	amvdec_clear_dos_bits(core, HEVC_PARSER_CORE_CONTROL, BIT(0));
	v = citaj(h, HEVC_STREAM_CONTROL) | BIT(0) | (0xf << 25);
	zapis(h, HEVC_STREAM_CONTROL, v);
	amvdec_write_dos_bits(core, HEVC_STREAM_FIFO_CTL, BIT(29));
	zapis(h, HEVC_SHIFT_STARTCODE, 0x00000100);
	zapis(h, HEVC_SHIFT_EMULATECODE, 0x00000300);
	v = citaj(h, HEVC_PARSER_INT_CONTROL) & 0x03ffffff;
	v |= (3 << 29) | (2 << 26) | BIT(24) | BIT(22) | BIT(7) | BIT(4) | BIT(0);
	zapis(h, HEVC_PARSER_INT_CONTROL, v);
	amvdec_write_dos_bits(core, HEVC_SHIFT_STATUS, BIT(1) | BIT(0));
	zapis(h, HEVC_SHIFT_CONTROL, (3 << 6) | (2 << 4) | (2 << 1) | BIT(0));
	zapis(h, HEVC_CABAC_CONTROL, BIT(0));
	zapis(h, HEVC_PARSER_CORE_CONTROL, BIT(0));
	zapis(h, SKL_STAV, 0);

	zapis(h, HEVC_IQIT_SCALELUT_WR_ADDR, 0);
	for (i = 0; i < 1024; i++)
		zapis(h, HEVC_IQIT_SCALELUT_DATA, 0);

	zapis(h, SKL_VELKOST_TOKU, 0);
	zapis(h, HEVC_PARSER_CMD_WRITE, BIT(16));
	for (i = 0; i < VDEC_HEVC_PARSER_CMD_LEN; i++)
		zapis(h, HEVC_PARSER_CMD_WRITE, vdec_hevc_parser_cmd[i]);
	zapis(h, HEVC_PARSER_CMD_SKIP_0, PARSER_CMD_SKIP_CFG_0);
	zapis(h, HEVC_PARSER_CMD_SKIP_1, PARSER_CMD_SKIP_CFG_1);
	zapis(h, HEVC_PARSER_CMD_SKIP_2, PARSER_CMD_SKIP_CFG_2);
	zapis(h, HEVC_PARSER_IF_CONTROL, BIT(5) | BIT(2) | BIT(0));
	zapis(h, HEVCD_IPP_TOP_CNTL, BIT(0));
	zapis(h, HEVCD_IPP_TOP_CNTL, BIT(1));
	/* referencie čítať ako dve roviny NV12 (bez komprimácie) */
	zapis(h, HEVCD_MPP_DECOMP_CTL1, BIT(31));

	zapis(h, SKL_CAKANIE, 1);
	/* prerušenie poštovej schránky (jadro meson-vdec počúva na schránke 1; 0 pre istotu tiež) */
	zapis(h, HEVC_ASSIST_MBOX1_CLR_REG, 1);
	zapis(h, HEVC_ASSIST_MBOX1_MASK, 1);
	zapis(h, SKL_MBOX0_CLR, 1);
	zapis(h, SKL_MBOX0_MASK, 1);
	zapis(h, HEVC_PSCALE_CTRL, 0);
	zapis(h, SKL_LADENIE, 0);
	/* mikrokód sám preskočí všetko pred prvým VPS/SPS/PPS a prvým I-rezom */
	zapis(h, SKL_HLADANIE_NAL, 0x8);
	zapis(h, SKL_STOP_POZICIA, 0);
	zapis(h, SKL_TYP_NAL, 0);	/* režim: jeden tok */

	sess->priv = h;
	return 0;
}

/* ---------- snímky, referencie, výstup ---------- */

static struct skl_snimka *snimka_podla_poc(struct skl_hevc *h, int poc)
{
	struct skl_snimka *s = NULL;
	int i;

	if (poc == SKL_POC_ZIADNY)
		return NULL;
	for (i = 0; i < SKL_SNIMOK; i++) {
		struct skl_snimka *x = &h->snimky[i];

		if (!x->vbuf || x->poc != poc || x == h->aktualna)
			continue;
		if (!s || x->poradie > s->poradie)
			s = x;
	}
	return s;
}

/* snímka je voľná, keď ju už nikto nepotrebuje: nie je referencia ani nečaká na výstup */
static void skus_uvolnit(struct skl_hevc *h, struct skl_snimka *s)
{
	if (!s->vbuf || s->referencna || s->na_vystup || s->dekoduje_sa)
		return;
	if (!s->vydana) {
		/* nikdy nevydaná (napr. preskočená) – späť do fronty programu */
		v4l2_m2m_buf_queue(h->sess->m2m_ctx, s->vbuf);
	}
	s->vbuf = NULL;
	s->poc = SKL_POC_ZIADNY;
}

static void vydaj(struct skl_hevc *h, struct skl_snimka *s)
{
	s->na_vystup = false;
	s->vydana = true;
	h->vydanych++;
	amvdec_dst_buf_done_offset(h->sess, s->vbuf, s->posun, V4L2_FIELD_NONE, false);
	skus_uvolnit(h, s);
}

/* vydá snímku s najmenším POC, keď ich čaká viac, ako dovoľuje preusporiadanie (alebo všetky pri vyprázdnení) */
static void vydavaj(struct skl_hevc *h, bool vsetky)
{
	for (;;) {
		struct skl_snimka *najmensia = NULL;
		u32 cakajucich = 0;
		int i;

		for (i = 0; i < SKL_SNIMOK; i++) {
			struct skl_snimka *s = &h->snimky[i];

			if (!s->vbuf || !s->na_vystup)
				continue;
			cakajucich++;
			if (!najmensia || s->poc < najmensia->poc ||
			    (s->poc == najmensia->poc && s->poradie < najmensia->poradie))
				najmensia = s;
		}
		if (!najmensia || (!vsetky && cakajucich <= h->preusporiadanie))
			return;
		vydaj(h, najmensia);
	}
}

/* rozpracovaná snímka je hotová – čaká na výstup */
static void dokonci_aktualnu(struct skl_hevc *h)
{
	struct skl_snimka *s = h->aktualna;

	if (!s)
		return;
	if (h->dekod_od) {
		u64 ns = ktime_to_ns(ktime_sub(ktime_get(), h->dekod_od));

		h->dekod_ns_spolu += ns;
		h->dekodovanych++;
		if (h->meranie_platne) {
			h->dekod_ns_platne += ns;
			h->dekodovanych_platne++;
		}
		/* do riadenia taktu len časy bez čakania na tok (pri prehrávaní v reálnom čase sa čaká) */
		if (h->meranie_platne) {
			h->dekod_ns += ns;
			h->dekod_snimok++;
		}
		h->dekod_od = 0;
	}
	s->dekoduje_sa = false;
	s->na_vystup = true;
	h->aktualna = NULL;
}

/* označí za nereferenčné snímky, ktoré nie sú v sade referencií novej snímky (RPS) */
static void uplatni_rps(struct skl_hevc *h, int poc)
{
	const struct skl_rpm *p = &h->rpm;
	int i, j;

	for (i = 0; i < SKL_SNIMOK; i++) {
		struct skl_snimka *s = &h->snimky[i];
		bool ostava = false;

		if (!s->vbuf || !s->referencna)
			continue;
		for (j = 0; j < 16 && !(p->rps[j] & 0x8000); j++) {
			int delta = p->rps[j] & 0x1fff;
			int r = (p->rps[j] & BIT(13)) ? poc - (0x2000 - delta) : poc + delta;

			if (r == s->poc) {
				ostava = true;
				break;
			}
		}
		if (!ostava) {
			s->referencna = false;
			skus_uvolnit(h, s);
		}
	}
}

/* zoznamy referencií L0 a L1 aktuálneho rezu (8.3.4) z RPS a prípadných zmien zoznamov */
static void zoznamy_referencii(struct skl_hevc *h)
{
	const struct skl_rpm *p = &h->rpm;
	struct skl_snimka *s = h->aktualna;
	int pred[16], po[16], npred = 0, npo = 0, spolu, i;

	for (i = 0; i < SKL_REF; i++)
		s->l0[i] = s->l1[i] = 0;
	for (i = 0; i < 16 && !(p->rps[i] & 0x8000); i++) {
		int delta = p->rps[i] & 0x1fff;

		if (!(p->rps[i] & BIT(14)))
			continue;	/* len pre budúce snímky */
		if (p->rps[i] & BIT(13))
			pred[npred++] = s->poc - (0x2000 - delta);
		else
			po[npo++] = s->poc + delta;
	}
	spolu = npred + npo;
	if (!spolu)
		return;
	for (i = 0; i < h->ref_l0; i++) {
		int c = (p->zmena_zoznamov & 1) ? p->zoznam_zmien[i] : i % spolu;

		c = min(c, spolu - 1);
		s->l0[i] = c >= npred ? po[c - npred] : pred[c];
	}
	if (h->typ_rezu != REZ_B)
		return;
	for (i = 0; i < h->ref_l1; i++) {
		int c;

		if (p->zmena_zoznamov & 2)
			c = p->zoznam_zmien[(p->zmena_zoznamov & 1) ? h->ref_l0 + i : i];
		else
			c = i % spolu;
		c = min(c, spolu - 1);
		s->l1[i] = c >= npo ? pred[c - npo] : po[c];
	}
}

/* POC aktuálnej snímky (8.3.1) */
static void vypocitaj_poc(struct skl_hevc *h, bool bez_rasl)
{
	const struct skl_rpm *p = &h->rpm;
	int typ = p->typ_nal;
	int max_lsb = 1 << (p->log2_max_poc_lsb_minus4 + 4);
	int lsb = p->poc_lsb;
	int pred_lsb, pred_msb, msb;

	if (typ == NAL_IDR_W_RADL || typ == NAL_IDR_N_LP) {
		h->poc = 0;
	} else if (bez_rasl && typ >= NAL_BLA_W_LP && typ <= NAL_CRA) {
		/* IRAP s NoRaslOutputFlag: MSB = 0 */
		h->poc = lsb;
	} else {
		pred_lsb = h->poc_predch_tid0 & (max_lsb - 1);
		pred_msb = h->poc_predch_tid0 - pred_lsb;
		if (lsb < pred_lsb && pred_lsb - lsb >= max_lsb / 2)
			msb = pred_msb + max_lsb;
		else if (lsb > pred_lsb && lsb - pred_lsb > max_lsb / 2)
			msb = pred_msb - max_lsb;
		else
			msb = pred_msb;
		h->poc = msb + lsb;
	}
	/* prevTid0Pic: TemporalId 0 a nie RASL, RADL ani podvrstvová nereferenčná (párne typy do 14) */
	if (p->docasna_vrstva <= 1 && !(typ >= NAL_RADL_N && typ <= NAL_RASL_R) &&
	    !(typ <= 14 && !(typ & 1)))
		h->poc_predch_tid0 = h->poc;
}

/* tabuľka adries snímok pre kompenzáciu pohybu: snímka i → Y na kanvase 2i, Cb/Cr na 2i + 1 */
static void nastav_tabulku_snimok(struct skl_hevc *h)
{
	struct vb2_queue *q = v4l2_m2m_get_dst_vq(h->sess->m2m_ctx);
	u32 n = min_t(u32, vb2_get_num_buffers(q), SKL_SNIMOK);
	dma_addr_t y = 0, uv = 0;
	u32 i;

	zapis(h, HEVCD_MPP_ANC2AXI_TBL_CONF_ADDR, BIT(1) | BIT(2));
	for (i = 0; i < SKL_SNIMOK; i++) {
		struct vb2_buffer *vb = i < n ? vb2_get_buffer(q, i) : NULL;

		if (vb) {
			y = vb2_dma_contig_plane_dma_addr(vb, 0);
			uv = vb2_dma_contig_plane_dma_addr(vb, 1);
		}
		/* nepoužité miesta ukazujú na posledný buffer (nikdy nie na nulu) */
		zapis(h, HEVCD_MPP_ANC2AXI_TBL_DATA, y >> 5);
		zapis(h, HEVCD_MPP_ANC2AXI_TBL_DATA, uv >> 5);
	}
	zapis(h, HEVCD_MPP_ANC2AXI_TBL_CONF_ADDR, 1);
	zapis(h, HEVCD_MPP_ANC_CANVAS_ACCCONFIG_ADDR, 1);
	for (i = 0; i < 32; i++)
		zapis(h, HEVCD_MPP_ANC_CANVAS_DATA_ADDR, 0);
	h->zoznam_nastaveny = true;
}

static void uvolni_mv(struct skl_hevc *h)
{
	struct device *dev = jadro(h)->dev;
	int i;

	for (i = 0; i < SKL_SNIMOK; i++) {
		struct skl_snimka *s = &h->snimky[i];

		if (s->mv_virt)
			dma_free_coherent(dev, s->mv_velkost, s->mv_virt, s->mv);
		s->mv_virt = NULL;
	}
}

/* buffer pohybových vektorov snímky (pre časovú predikciu ďalších snímok) */
static int priprav_mv(struct skl_hevc *h, struct skl_snimka *s)
{
	u32 velkost = (h->sirka * h->vyska > 1920 * 1088) ? MV_4K : MV_FULL_HD;

	if (s->mv_virt && s->mv_velkost == velkost)
		return 0;
	if (s->mv_virt)
		dma_free_coherent(jadro(h)->dev, s->mv_velkost, s->mv_virt, s->mv);
	s->mv_virt = dma_alloc_coherent(jadro(h)->dev, velkost, &s->mv, GFP_KERNEL);
	if (!s->mv_virt)
		return -ENOMEM;
	s->mv_velkost = velkost;
	return 0;
}

static dma_addr_t mv_adresa(struct skl_snimka *s)
{
	return ALIGN(s->mv, SZ_64K);
}

/* nová snímka: voľný buffer z fronty programu, ktorý nie je referenciou */
static struct skl_snimka *nova_snimka(struct skl_hevc *h)
{
	struct v4l2_m2m_buffer *b;
	struct skl_snimka *s = NULL;

	v4l2_m2m_for_each_dst_buf(h->sess->m2m_ctx, b) {
		u32 i = b->vb.vb2_buf.index;

		if (i >= SKL_SNIMOK || h->snimky[i].vbuf)
			continue;
		s = &h->snimky[i];
		s->vbuf = &b->vb;
		break;
	}
	if (!s)
		return NULL;
	v4l2_m2m_dst_buf_remove_by_buf(h->sess->m2m_ctx, s->vbuf);
	if (priprav_mv(h, s)) {
		v4l2_m2m_buf_queue(h->sess->m2m_ctx, s->vbuf);
		s->vbuf = NULL;
		return NULL;
	}
	s->poc = h->poc;
	s->poradie = h->poradie++;
	s->referencna = true;
	s->na_vystup = false;
	s->vydana = false;
	s->dekoduje_sa = true;
	s->sirka = h->sirka;
	s->vyska = h->vyska;
	s->posun = citaj(h, HEVC_SHIFT_BYTE_COUNT);
	return s;
}

/* ---------- nastavenie blokov jadra HEVC pre rez ---------- */

static void nastav_sao_pamat(struct skl_hevc *h)
{
	u32 jednotka = (h->lcu == 16 ? 9 : h->lcu == 32 ? 14 : 24) << 4;
	u32 vb = (jednotka + (2 << 4)) * h->lcu_y;

	zapis(h, SKL_SAO_JEDNOTKA, jednotka);
	zapis(h, SKL_SAO_NAD, h->pp + O_SAO_NAD);
	zapis(h, SKL_SAO_VB_VELKOST, vb);
	zapis(h, SKL_SAO_VB, h->pp + O_SAO_VB);
}

static void nastav_referencie(struct skl_hevc *h)
{
	struct skl_snimka *s = h->aktualna;
	int zoznam, i;

	for (zoznam = 0; zoznam < 2; zoznam++) {
		int pocet = zoznam ? h->ref_l1 : h->ref_l0;
		int *poc = zoznam ? s->l1 : s->l0;

		if (h->typ_rezu == REZ_I || (zoznam && h->typ_rezu != REZ_B))
			continue;
		zapis(h, HEVCD_MPP_ANC_CANVAS_ACCCONFIG_ADDR, ((zoznam ? 16 : 0) << 8) | 1);
		for (i = 0; i < pocet; i++) {
			struct skl_snimka *r = snimka_podla_poc(h, poc[i]);
			u32 idx;

			if (!r || r->sirka != h->sirka || r->vyska != h->vyska) {
				/* chýbajúca referencia – radšej vlastná snímka ako nezmysel z pamäte */
				h->chyb_ref++;
				r = s;
			}
			idx = r->vbuf->vb2_buf.index;
			zapis(h, HEVCD_MPP_ANC_CANVAS_DATA_ADDR,
			      ((2 * idx + 1) << 16) | ((2 * idx + 1) << 8) | (2 * idx));
		}
	}
}

/* vyrovnávacia pamäť kompenzácie pohybu – prvé dve referencie */
static void nastav_mcrcc(struct skl_hevc *h)
{
	u32 a, b;

	zapis(h, HEVCD_MCRCC_CTL1, 0x2);
	if (h->typ_rezu == REZ_I) {
		zapis(h, HEVCD_MCRCC_CTL1, 0x0);
		return;
	}
	if (h->typ_rezu == REZ_B) {
		zapis(h, HEVCD_MPP_ANC_CANVAS_ACCCONFIG_ADDR, 0);
		a = citaj(h, HEVCD_MPP_ANC_CANVAS_DATA_ADDR) & 0xffff;
		zapis(h, HEVCD_MCRCC_CTL2, a | (a << 16));
		zapis(h, HEVCD_MPP_ANC_CANVAS_ACCCONFIG_ADDR, (16 << 8) | BIT(1));
		b = citaj(h, HEVCD_MPP_ANC_CANVAS_DATA_ADDR) & 0xffff;
		if (b == a && h->ref_l1 > 1)
			b = citaj(h, HEVCD_MPP_ANC_CANVAS_DATA_ADDR) & 0xffff;
		zapis(h, HEVCD_MCRCC_CTL3, b | (b << 16));
	} else {
		zapis(h, HEVCD_MPP_ANC_CANVAS_ACCCONFIG_ADDR, BIT(1));
		a = citaj(h, HEVCD_MPP_ANC_CANVAS_DATA_ADDR) & 0xffff;
		zapis(h, HEVCD_MCRCC_CTL2, a | (a << 16));
		if (h->ref_l0 > 1)
			a = citaj(h, HEVCD_MPP_ANC_CANVAS_DATA_ADDR) & 0xffff;
		zapis(h, HEVCD_MCRCC_CTL3, a | (a << 16));
	}
	zapis(h, HEVCD_MCRCC_CTL1, 0xff0);
}

/* predikcia pohybu: kam zapisovať vektory tejto snímky, odkiaľ čítať vektory spolupoloženej snímky */
static void nastav_mpred(struct skl_hevc *h)
{
	const struct skl_rpm *p = &h->rpm;
	struct skl_snimka *s = h->aktualna;
	struct skl_snimka *c = h->col ? h->col : s;
	u32 jednotka = h->lcu_log2 == 6 ? 0x200 : h->lcu_log2 == 5 ? 0x80 : 0x20;
	dma_addr_t zapis_od = mv_adresa(s);
	dma_addr_t citaj_od = mv_adresa(c);
	bool ldc = false;
	int col_poc = SKL_POC_ZIADNY;
	int i;
	u32 v;

	if (h->typ_rezu != REZ_I) {
		ldc = true;
		for (i = 0; i < h->ref_l0; i++)
			if (s->l0[i] > s->poc)
				ldc = false;
		if (h->typ_rezu == REZ_B)
			for (i = 0; i < h->ref_l1; i++)
				if (s->l1[i] > s->poc)
					ldc = false;
		if (h->typ_rezu == REZ_B && !p->col_z_l0)
			col_poc = p->col_ref < h->ref_l1 ? s->l1[p->col_ref] : SKL_POC_ZIADNY;
		else
			col_poc = p->col_ref < h->ref_l0 ? s->l0[p->col_ref] : SKL_POC_ZIADNY;
	}

	zapis(h, HEVC_MPRED_MV_WR_START_ADDR, zapis_od);
	zapis(h, HEVC_MPRED_MV_RD_START_ADDR, citaj_od);
	/* bez dlaždíc je dlaždica celá šírka snímky – riadok vektorov nadväzuje bez skoku */
	zapis(h, HEVC_MPRED_MV_WR_ROW_JUMP, 0);
	zapis(h, HEVC_MPRED_MV_RD_ROW_JUMP, 0);

	v = (h->typ_rezu & 3) | (h->nova_snimka << 2) | (h->nova_snimka << 3) |
	    (h->dalsi_segment << 4) | ((p->tmvp & 1) << 5) | (ldc << 6) |
	    ((p->col_z_l0 & 1) << 7) | BIT(9) | BIT(10) |
	    ((h->typ_rezu != REZ_I) << 11) | BIT(13) |
	    (h->lcu_log2 << 16) | (3 << 20) | ((p->log2_paralelny_merge & 0xf) << 24);
	zapis(h, HEVC_MPRED_CTRL0, v);
	zapis(h, HEVC_MPRED_CTRL1, (5 - p->pat_minus_merge) | (2 << 4) | (3 << 8) | (5 << 12) | (36 << 16));
	zapis(h, HEVC_MPRED_PIC_SIZE, h->sirka | (h->vyska << 16));
	zapis(h, HEVC_MPRED_PIC_SIZE_LCU, (h->lcu_x - 1) | ((h->lcu_y - 1) << 16));
	zapis(h, HEVC_MPRED_TILE_START, 0);
	zapis(h, HEVC_MPRED_TILE_SIZE_LCU, h->lcu_x | (h->lcu_y << 16));
	zapis(h, HEVC_MPRED_REF_NUM, h->ref_l0 | (h->ref_l1 << 8));
	zapis(h, HEVC_MPRED_REF_EN_L0, (1u << h->ref_l0) - 1);
	zapis(h, HEVC_MPRED_REF_EN_L1, (1u << h->ref_l1) - 1);
	zapis(h, HEVC_MPRED_CUR_POC, s->poc);
	zapis(h, HEVC_MPRED_COL_POC, col_poc);
	/* POC referencií v pároch L0, L1 – poradie zápisu je pre obvod povinné */
	for (i = 0; i < SKL_REF; i++) {
		zapis(h, HEVC_MPRED_L0_REF00_POC + 4 * i, s->l0[i]);
		zapis(h, HEVC_MPRED_L1_REF00_POC + 4 * i, s->l1[i]);
	}
	if (h->nova_snimka) {
		zapis(h, HEVC_MPRED_ABV_START_ADDR, h->pp + O_MPRED_NAD);
		zapis(h, HEVC_MPRED_MV_WPTR, zapis_od + h->adresa_rezu * jednotka);
		zapis(h, HEVC_MPRED_MV_RPTR, citaj_od);
	} else if (!h->dalsi_segment) {
		zapis(h, HEVC_MPRED_MV_RPTR, citaj_od + h->adresa_rezu * jednotka);
	}
	zapis(h, HEVC_MPRED_MV_RD_END_ADDR, citaj_od + h->lcu_x * h->lcu_y * jednotka);
}

/* výstup snímky (NV12), odblokovanie a SAO */
static void nastav_vystup_a_filtre(struct skl_hevc *h)
{
	const struct skl_rpm *p = &h->rpm;
	struct vb2_buffer *vb = &h->aktualna->vbuf->vb2_buf;
	dma_addr_t y = vb2_dma_contig_plane_dma_addr(vb, 0);
	dma_addr_t uv = vb2_dma_contig_plane_dma_addr(vb, 1);
	u32 f = p->priznaky;
	u32 v, v2;
	bool bez_odblokovania;

	v = (citaj(h, HEVC_SAO_CTRL0) & ~0xf) | h->lcu_log2;
	zapis(h, HEVC_SAO_CTRL0, v);
	zapis(h, HEVC_SAO_PIC_SIZE, h->sirka | (h->vyska << 16));
	zapis(h, HEVC_SAO_PIC_SIZE_LCU, (h->lcu_x - 1) | ((h->lcu_y - 1) << 16));
	zapis(h, HEVC_SAO_Y_START_ADDR, y);
	zapis(h, HEVC_SAO_C_START_ADDR, uv);
	zapis(h, HEVC_SAO_Y_WPTR, y);
	zapis(h, HEVC_SAO_C_WPTR, uv);
	zapis(h, HEVC_SAO_Y_LENGTH, vb2_plane_size(vb, 0));
	zapis(h, HEVC_SAO_C_LENGTH, vb2_plane_size(vb, 1));

	if (h->nova_snimka) {
		/* len nekomprimovaný zápis (NV12); zreťazenie odblokovania od 1280 bodov */
		v = (0x57 << 8) & ~0x300;
		v |= BIT(9);
		if (h->sirka >= 1280)
			v |= BIT(4);
		zapis(h, HEVC_DBLK_CFGB, v);
		zapis(h, HEVC_DBLK_CFG2, h->sirka | (h->vyska << 16));
		v = (f & F_PCM) ? ((f & F_PCM_BEZ_FILTRA) ? BIT(3) : 0) : 0;
		v |= ((p->pps_cb_qp & 0x1f) << 4) | ((p->pps_cr_qp & 0x1f) << 9);
		v |= h->lcu == 64 ? 0 : h->lcu == 32 ? 1 : 2;
		zapis(h, HEVC_DBLK_CFG1, v);
	}

	/* lineárne uloženie, poradie bajtov pre NV12 */
	v = citaj(h, HEVC_SAO_CTRL1) & ~0x3ff0;
	v |= 0xff0;
	zapis(h, HEVC_SAO_CTRL1, v);
	v = citaj(h, HEVC_SAO_CTRL5) & ~(0xff << 16);
	zapis(h, HEVC_SAO_CTRL5, v);
	v = citaj(h, HEVCD_IPP_AXIIF_CONFIG) & ~0x30;
	v = (v & ~0xf) | 0xf;
	v &= ~BIT(12);
	zapis(h, HEVCD_IPP_AXIIF_CONFIG, v);

	/* odblokovanie: hodnoty z rezu, ak ich rez prepisuje, inak z PPS */
	v = 0;
	v2 = citaj(h, HEVC_SAO_CTRL0) & ~0x300;
	if ((f & F_PREPIS_POVOLENY) && (f & F_PREPIS)) {
		bez_odblokovania = f & F_REZ_BEZ_ODBLOKOVANIA;
		v |= bez_odblokovania << 2;
		if (!bez_odblokovania)
			v |= ((p->rez_beta & 0xf) << 3) | ((p->rez_tc & 0xf) << 7);
	} else {
		bez_odblokovania = f & F_PPS_BEZ_ODBLOKOVANIA;
		v |= bez_odblokovania << 2;
		if (!bez_odblokovania)
			v |= ((p->pps_beta & 0xf) << 3) | ((p->pps_tc & 0xf) << 7);
	}
	bez_odblokovania = f & F_REZ_BEZ_ODBLOKOVANIA;
	if ((f & F_PPS_FILTER_CEZ_REZY) &&
	    ((f & F_SAO_JAS) || (f & F_SAO_FARBA) || !bez_odblokovania)) {
		v |= !!(f & F_REZ_FILTER_CEZ_REZY) << 1;
		v2 |= !!(f & F_REZ_FILTER_CEZ_REZY) << 9;
	} else {
		v |= !!(f & F_PPS_FILTER_CEZ_REZY) << 1;
		v2 |= !!(f & F_PPS_FILTER_CEZ_REZY) << 9;
	}
	zapis(h, HEVC_DBLK_CFG9, v);
	zapis(h, HEVC_SAO_CTRL0, v2);
}

/* ---------- takt podľa potreby ---------- */

static void nastav_takt(struct skl_hevc *h, int index)
{
	struct amvdec_core *core = jadro(h);
	unsigned long hz = skl_takty[index] * 1000000UL + 666666;

	/* zmena len pri zastavených hodinách – jadro HEVC teraz čaká na odpoveď */
	clk_disable_unprepare(core->vdec_hevc_clk);
	clk_disable_unprepare(core->vdec_hevcf_clk);
	clk_set_rate(core->vdec_hevcf_clk, hz);
	clk_set_rate(core->vdec_hevc_clk, hz);
	if (clk_prepare_enable(core->vdec_hevcf_clk) || clk_prepare_enable(core->vdec_hevc_clk))
		dev_err(core->dev, "sklenik-hevc: hodiny sa nezapli\n");
	h->takt_index = index;
}

/* po okne snímok: vyťaženie = priemerný čas dekódovania / perióda snímok */
static void zhodnot_takt(struct skl_hevc *h)
{
	u64 priemer, vytazenie;
	int i = h->takt_index;

	if (h->dekod_snimok < TAKT_OKNO || !h->perioda_ns)
		return;
	priemer = div_u64(h->dekod_ns, h->dekod_snimok);
	vytazenie = div64_u64(priemer * 100, h->perioda_ns);
	h->vytazenie = vytazenie;
	h->dekod_ns = 0;
	h->dekod_snimok = 0;

	if (vytazenie > 90)
		h->takt_ziadany = 0;	/* náročná scéna – hneď naplno */
	else if (vytazenie > 75 && i > 0)
		h->takt_ziadany = i - 1;
	else if (i + 1 < ARRAY_SIZE(skl_takty) &&
		 div_u64(vytazenie * skl_takty[i], skl_takty[i + 1]) < 70)
		h->takt_ziadany = i + 1;
}

/* ---------- spracovanie hlavičky rezu ---------- */

/* vráti true, ak treba čakať (formát alebo voľná snímka); inak už odpovedal mikrokódu */
static bool spracuj_rez(struct skl_hevc *h)
{
	const struct skl_rpm *p = &h->rpm;
	struct amvdec_session *sess = h->sess;
	int typ = p->typ_nal;
	bool bez_rasl = false;

	h->nova_snimka = p->adresa_rezu == 0;
	h->dalsi_segment = p->zavisly_rez != 0;
	if (!h->dalsi_segment)
		h->adresa_rezu = p->adresa_rezu;
	h->typ_rezu = p->typ_rezu == 2 ? REZ_I : p->typ_rezu == 1 ? REZ_P : REZ_B;
	h->ref_l0 = min_t(u16, p->ref_l0, SKL_REF);
	h->ref_l1 = min_t(u16, p->ref_l1, SKL_REF);

	if (h->nova_snimka && !h->cakame_snimku) {
		dokonci_aktualnu(h);
		zhodnot_takt(h);

		/* IRAP bez predchádzajúcich RASL: IDR, BLA, prvý CRA */
		if (typ == NAL_IDR_W_RADL || typ == NAL_IDR_N_LP || (typ >= NAL_BLA_W_LP && typ < NAL_IDR_W_RADL) ||
		    (typ == NAL_CRA && h->poc_nahodny_pristup == INT_MAX))
			bez_rasl = true;
		vypocitaj_poc(h, bez_rasl);
		if (bez_rasl) {
			/* všetko čakajúce von, staré referencie preč */
			vydavaj(h, true);
			h->poc_nahodny_pristup = (typ == NAL_CRA || typ < NAL_IDR_W_RADL) ? h->poc : INT_MIN;
		} else if ((typ == 8 || typ == 9) && h->poc < h->poc_nahodny_pristup) {
			/* RASL k nedostupnej referencii – preskočiť */
			zapis(h, SKL_CAKANIE, citaj(h, SKL_CAKANIE) & ~0x2);
			odpovedz(h, ST_HOTOVO);
			return false;
		}
		uplatni_rps(h, h->poc);
		vydavaj(h, false);
	} else if (!h->nova_snimka && !h->aktualna) {
		/* pokračovanie snímky, ktorá sa nezačala – preskočiť */
		zapis(h, SKL_CAKANIE, citaj(h, SKL_CAKANIE) & ~0x2);
		odpovedz(h, ST_HOTOVO);
		return false;
	}

	if (h->nova_snimka) {
		if (!h->zoznam_nastaveny)
			nastav_tabulku_snimok(h);
		h->aktualna = nova_snimka(h);
		if (!h->aktualna) {
			/* žiadny voľný buffer: vydať čakajúcu snímku a skúsiť o chvíľu */
			vydavaj(h, false);
			if (!h->cakame_snimku) {
				struct skl_snimka *najstarsia = NULL;
				int i;

				for (i = 0; i < SKL_SNIMOK; i++) {
					struct skl_snimka *s = &h->snimky[i];

					if (s->vbuf && s->na_vystup &&
					    (!najstarsia || s->poc < najstarsia->poc))
						najstarsia = s;
				}
				if (najstarsia)
					vydaj(h, najstarsia);
			}
			h->cakame_snimku = true;
			schedule_delayed_work(&h->opakuj, msecs_to_jiffies(10));
			return true;
		}
		h->cakame_snimku = false;
		h->aktualna->typ_rezu = h->typ_rezu;
		nastav_sao_pamat(h);
		/* takt podľa potreby – medzi snímkami */
		if (h->takt_ziadany != h->takt_index) {
			dev_info(jadro(h)->dev, "sklenik-hevc: takt jadra HEVC %u → %u MHz (vyťaženie %u %%)\n",
				 skl_takty[h->takt_index], skl_takty[h->takt_ziadany], h->vytazenie);
			nastav_takt(h, h->takt_ziadany);
		}
	}

	zoznamy_referencii(h);
	h->col = NULL;
	if (h->typ_rezu != REZ_I) {
		int col_ref = p->col_ref;
		int poc = (h->typ_rezu == REZ_B && !p->col_z_l0) ?
			  (col_ref < h->ref_l1 ? h->aktualna->l1[col_ref] : SKL_POC_ZIADNY) :
			  (col_ref < h->ref_l0 ? h->aktualna->l0[col_ref] : SKL_POC_ZIADNY);

		h->col = snimka_podla_poc(h, poc);
	}
	nastav_referencie(h);
	nastav_mcrcc(h);
	nastav_mpred(h);
	nastav_vystup_a_filtre(h);

	sess->keyframe_found = 1;
	zapis(h, SKL_CAKANIE, citaj(h, SKL_CAKANIE) | 0x2);
	if (h->nova_snimka) {
		h->dekod_od = ktime_get();
		h->meranie_platne = atomic_read(&sess->esparser_queued_bufs) > (int)skl_cakajuce(sess);
	}
	odpovedz(h, ST_DATA_REZU);
	return false;
}

/* rozmery a počet snímok z hlavičky – zmena formátu, ak treba nové buffre */
static bool skontroluj_format(struct skl_hevc *h)
{
	const struct skl_rpm *p = &h->rpm;
	u32 sirka = p->sirka, vyska = p->vyska;
	u32 lcu = 1 << (p->log2_min_cb_minus3 + 3 + p->log2_rozdiel_cb);
	u32 hlbka = (p->bitova_hlbka & 0xf) + 8;
	u32 dpb = clamp_t(u32, p->max_dpb_minus1 + 1 + 3, 4, SKL_SNIMOK);

	if (!sirka || !vyska || lcu < 16 || lcu > 64 || sirka > 4096 || vyska > 2304) {
		dev_err(jadro(h)->dev, "sklenik-hevc: neplatné rozmery %ux%u (CTB %u)\n", sirka, vyska, lcu);
		amvdec_abort(h->sess);
		return true;
	}
	if (hlbka != 8) {
		dev_err(jadro(h)->dev, "sklenik-hevc: %u-bitové video zatiaľ nie (len 8-bitové)\n", hlbka);
		amvdec_abort(h->sess);
		return true;
	}
	if (p->dlazdice & 1)
		dev_warn_once(jadro(h)->dev, "sklenik-hevc: dlaždice (tiles) zatiaľ nie – obraz môže mať chyby\n");
	h->lcu = lcu;
	h->lcu_log2 = ilog2(lcu);
	h->lcu_x = DIV_ROUND_UP(sirka, lcu);
	h->lcu_y = DIV_ROUND_UP(vyska, lcu);
	h->preusporiadanie = p->preusporiadanie;
	if (p->vui_skala_hore | p->vui_skala_dole) {
		u64 tick = ((u32)p->vui_tick_hore << 16) | p->vui_tick_dole;
		u64 skala = ((u32)p->vui_skala_hore << 16) | p->vui_skala_dole;

		h->perioda_ns = skala ? div64_u64(tick * NSEC_PER_SEC, skala) : 0;
	}
	if (!h->perioda_ns)
		h->perioda_ns = NSEC_PER_SEC / 60;	/* neznáma frekvencia – opatrne 60 snímok/s */

	if (sirka == h->sirka && vyska == h->vyska && h->zoznam_nastaveny)
		return false;
	h->sirka = sirka;
	h->vyska = vyska;
	h->zoznam_nastaveny = false;
	h->cakame_format = true;
	mutex_unlock(&h->zamok);
	/* buď hneď zavolá resume (buffre sedia), alebo pošle programu udalosť o zmene zdroja */
	amvdec_src_change(h->sess, sirka, vyska, dpb);
	mutex_lock(&h->zamok);
	return true;
}

static void nacitaj_rpm(struct skl_hevc *h)
{
	const u16 *z = h->pp_virt + O_RPM;
	u16 *d = (u16 *)&h->rpm;
	int i, j;

	/* mikrokód zapisuje 64-bitové slová s opačným poradím 16-bitových častí */
	for (i = 0; i < 128; i += 4)
		for (j = 0; j < 4; j++)
			d[i + j] = z[i + 3 - j];
}

static void odpovedz_na_nal(struct skl_hevc *h)
{
	u32 typ = citaj(h, SKL_TYP_NAL) & 0xffff;
	u32 odpoved = ST_ZAHOD_NAL;

	if (typ == NAL_EOS) {
		/* koniec toku: posledná snímka je hotová, všetko von */
		dokonci_aktualnu(h);
		vydavaj(h, true);
		h->poc_nahodny_pristup = INT_MAX;
		odpovedz(h, ST_ZAHOD_NAL);
		return;
	}
	if (typ == NAL_VPS) {
		odpoved = ST_VPS;
		h->ma_vps = true;
	} else if (typ == NAL_SPS && h->ma_vps) {
		odpoved = ST_SPS;
		h->ma_sps = true;
	} else if (typ == NAL_PPS && h->ma_vps) {
		odpoved = ST_PPS;
		h->ma_pps = true;
	} else if (typ <= NAL_CRA && h->ma_sps && h->ma_pps) {
		if (typ >= NAL_BLA_W_LP)
			h->ma_start = true;
		if (h->ma_start)
			odpoved = ST_HLAVICKA_REZU;
	}
	if (h->ma_vps && h->ma_sps && h->ma_pps && h->ma_start)
		zapis(h, SKL_HLADANIE_NAL, 0x2);	/* ďalej mikrokód sám, bez kontrol */
	odpovedz(h, odpoved);
}

static irqreturn_t skl_isr(struct amvdec_session *sess)
{
	struct amvdec_core *core = sess->core;

	amvdec_write_dos(core, HEVC_ASSIST_MBOX1_CLR_REG, 1);
	amvdec_write_dos(core, SKL_MBOX0_CLR, 1);
	return IRQ_WAKE_THREAD;
}

static irqreturn_t skl_threaded_isr(struct amvdec_session *sess)
{
	struct skl_hevc *h = sess->priv;
	u32 stav;

	if (!h)
		return IRQ_HANDLED;
	mutex_lock(&h->zamok);
	stav = citaj(h, SKL_STAV);
	switch (stav) {
	case ST_NAL_NAJDENY:
		odpovedz_na_nal(h);
		break;
	case ST_REZ_PRECITANY:
		if (h->rpm_cakajuci)
			break;	/* tá istá hlavička ešte čaká na buffre */
		nacitaj_rpm(h);
		h->rpm_cakajuci = true;
		if (h->rpm.adresa_rezu == 0 && skontroluj_format(h))
			break;
		if (!spracuj_rez(h))
			h->rpm_cakajuci = false;
		break;
	case ST_SEI:
		odpovedz(h, ST_SEI_HOTOVE);
		break;
	case ST_PREDEKODOVANE:
		zapis(h, SKL_STAV, 0);
		break;
	case ST_PRILIS_VELKY:
		dev_err(jadro(h)->dev, "sklenik-hevc: snímka je pre dekodér príliš veľká\n");
		amvdec_abort(sess);
		break;
	default:
		dev_dbg(jadro(h)->dev, "sklenik-hevc: stav mikrokódu %#x\n", stav);
		break;
	}
	mutex_unlock(&h->zamok);
	return IRQ_HANDLED;
}

/* program pripravil buffre po zmene formátu */
static void skl_resume(struct amvdec_session *sess)
{
	struct skl_hevc *h = sess->priv;

	mutex_lock(&h->zamok);
	if (h->cakame_format && h->rpm_cakajuci) {
		h->cakame_format = false;
		skl_nastav_pracovnu_pamat(h);
		nastav_tabulku_snimok(h);
		if (!spracuj_rez(h))
			h->rpm_cakajuci = false;
	}
	mutex_unlock(&h->zamok);
}

static void skl_opakuj(struct work_struct *w)
{
	struct skl_hevc *h = container_of(to_delayed_work(w), struct skl_hevc, opakuj);

	mutex_lock(&h->zamok);
	if (h->cakame_snimku && h->rpm_cakajuci && !h->cakame_format) {
		if (!spracuj_rez(h))
			h->rpm_cakajuci = false;
	}
	mutex_unlock(&h->zamok);
}

static u32 skl_cakajuce(struct amvdec_session *sess)
{
	struct skl_hevc *h = sess->priv;
	u32 n = 0;
	int i;

	if (!h)
		return 0;
	for (i = 0; i < SKL_SNIMOK; i++)
		if (h->snimky[i].vbuf && !h->snimky[i].vydana)
			n++;
	return n;
}

static int skl_stop(struct amvdec_session *sess)
{
	struct skl_hevc *h = sess->priv;
	struct amvdec_core *core = sess->core;
	int i;

	cancel_delayed_work_sync(&h->opakuj);
	mutex_lock(&h->zamok);
	if (h->vydanych)
		dev_info(core->dev,
			 "sklenik-hevc: %u snímok %ux%u, dekódovanie %llu µs (%u snímok bez čakania na tok), so všetkým %llu µs, takt %u MHz, chýbajúcich referencií %u\n",
			 h->vydanych, h->sirka, h->vyska,
			 div_u64(h->dekod_ns_platne, max_t(u32, h->dekodovanych_platne, 1) * 1000),
			 h->dekodovanych_platne,
			 div_u64(h->dekod_ns_spolu, max_t(u32, h->dekodovanych, 1) * 1000),
			 skl_takty[h->takt_index], h->chyb_ref);
	/* buffre, ktoré drží dekodér a program ich ešte nedostal, idú späť s chybou */
	for (i = 0; i < SKL_SNIMOK; i++) {
		struct skl_snimka *s = &h->snimky[i];

		if (s->vbuf && !s->vydana)
			v4l2_m2m_buf_done(s->vbuf, VB2_BUF_STATE_ERROR);
		s->vbuf = NULL;
	}
	/* jadro meson-vdec po zastavení vypína hodiny – takt späť na pôvodný, aby sedeli počty */
	if (h->takt_index)
		nastav_takt(h, 0);
	mutex_unlock(&h->zamok);
	uvolni_mv(h);
	dma_free_coherent(core->dev, PP_SPOLU, h->pp_virt, h->pp);
	return 0;
}

/* NAL EOS (typ 36) so štartovacím kódom */
static const u8 skl_eos[] = { 0x00, 0x00, 0x00, 0x01, 0x48, 0x01 };

/*
 * Vyprázdnenie (koniec toku, zastavenie). Rozbor 25. 11. 2026 (X96: 720×576 5/8, „vyprázdnenie po 500 ms –
 * snímka nedokončená“, priemer 144 ms na snímku):
 *   - parser jadra HEVC pozná koniec rezu až podľa ďalšieho štartovacieho kódu; za poslednou snímkou žiadny
 *     nie je, takže jej posledný rez sa nedekóduje celý a mikrokód snímku nikdy neohlási – vyprázdnenie vždy
 *     čakalo celých 500 ms a vydalo snímku s nedokončenými spodnými riadkami,
 *   - tých 500 ms sa započítalo do času dekódovania poslednej snímky (pri 8 snímkach ≈ +60 ms na snímku).
 * Oprava: do toku sa pošle NAL EOS (s výplňou esparsera) – parser dokončí posledný rez a mikrokód ohlási EOS
 * (odpovedz_na_nal dokončí a vydá všetko). Ak EOS neohlási, snímka je hotová, keď parser prejde posledný CTB
 * (HEVC_PARSER_LCU_START) a uplynie chvíľa na zápis SAO. Zámok sa počas čakania nedrží (vlákno prerušenia musí
 * bežať). Čas snímky dokončenej až po vypršaní sa do priemeru nezapočíta.
 */
static void skl_drain(struct amvdec_session *sess)
{
	struct skl_hevc *h = sess->priv;
	bool hotovo = false, posledny_ctb = false;
	int i;

	if (!h)
		return;
	if (esparser_queue_eos(sess->core, skl_eos, sizeof(skl_eos)))
		dev_warn(jadro(h)->dev, "sklenik-hevc: NAL EOS sa nepodarilo poslať\n");
	for (i = 0; i < 250 && !hotovo; i++) {
		mutex_lock(&h->zamok);
		if (!atomic_read(&sess->esparser_queued_bufs)) {
			if (!h->aktualna) {
				hotovo = true;
			} else if (h->lcu_x && h->lcu_y &&
				   (citaj(h, HEVC_PARSER_LCU_START) & 0xffffff) >= h->lcu_x * h->lcu_y - 1) {
				/* parser prešiel posledný CTB – pri ďalšom kole (2–3 ms) je zapísané aj SAO */
				hotovo = posledny_ctb;
				posledny_ctb = true;
			}
		}
		mutex_unlock(&h->zamok);
		if (!hotovo)
			usleep_range(2000, 3000);
	}
	mutex_lock(&h->zamok);
	if (!hotovo) {
		dev_info(jadro(h)->dev, "sklenik-hevc: vyprázdnenie po 500 ms – snímka %s\n",
			 h->aktualna ? "nedokončená" : "dokončená");
		h->dekod_od = 0;	/* čas čakania nie je čas dekódovania */
	}
	dokonci_aktualnu(h);
	vydavaj(h, true);
	mutex_unlock(&h->zamok);
}

struct amvdec_codec_ops sklenik_hevc_ops = {
	.start = skl_start,
	.stop = skl_stop,
	.isr = skl_isr,
	.threaded_isr = skl_threaded_isr,
	.num_pending_bufs = skl_cakajuce,
	.resume = skl_resume,
	.drain = skl_drain,
};
