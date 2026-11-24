// SPDX-License-Identifier: GPL-2.0-only OR Apache-2.0
/*
 * SKLENÍK – spoločný časovač budenia jadier (R186), vlastný ovládač pre Amlogic S905X3 (X96 Max Plus).
 *
 * Prečo: vypnuté jadro (hlboký spánok R167) má zastavený vlastný časovač. Budiť ho musí časovač, ktorý beží
 * mimo jadier („broadcast“). Popis hardvéru S905X3 v hlavnom jadre taký nemá, preto Linux na arm64 použije
 * náhradu – softvérový časovač na jednom z jadier, a toto jadro potom hlboko spať nesmie. V pokoji beží len
 * cpu0 (ostatné sú zaparkované), takže hlboký spánok nenastal nikdy (nočný test 24. 11.: 0 vstupov).
 *
 * Čip má v doméne periférií (EE, mimo zhluku jadier) časovače ISA A–I. Používame **Timer F** (výrobca ho v
 * Androide používa na to isté, hlavné jadro ho nepoužíva):
 *   riadenie 0xffd0f190 (ISA_TIMER_MUX1, spoločné pre F–I): bit 16 zapnutý F, bit 12 opakovaný F,
 *                       bity 1–0 krok F (0 = 1 µs, 1 = 10 µs, 2 = 100 µs, 3 = 1 ms),
 *   hodnota  0xffd0f194 (ISA_TIMERF): bity 15–0 počet krokov, zápis spustí odpočet; na konci prerušenie
 *                       GIC SPI 60 (hrana).
 * Krok 10 µs: najdlhšie čakanie 655 ms (pri 1 µs by bolo len 65 ms – zbytočné budenie 15× za sekundu),
 * presnosť 10 µs stačí (výstup z hlbokého spánku trvá ~1,5 ms).
 *
 * Bezpečnosť – pri štarte sa časovača nedotkne (len prečíta riadenie). Zapne ho až KLÍČ zápisom 1 do
 * /sys/module/sklenik_casovac/parameters/zapnute, a to tesne pred hlbokým spánkom jadier, keď už beží strážny
 * časovač a na karte je poistka R167. Pred zaregistrovaním ovládač časovač vyskúša: prerušenie po 2 ms a po
 * 20 ms musí prísť načas (±3 %). Ak riadenie hlási, že Timer F už niekto používa, alebo skúška nevyjde,
 * nezapne sa. Zaregistrovaný časovač ostane do reštartu (jadro výmenu späť nepodporuje).
 */

#define pr_fmt(fmt) "sklenik-casovac: " fmt

#include <linux/bitfield.h>
#include <linux/clockchips.h>
#include <linux/completion.h>
#include <linux/cpumask.h>
#include <linux/init.h>
#include <linux/interrupt.h>
#include <linux/io.h>
#include <linux/jiffies.h>
#include <linux/ktime.h>
#include <linux/moduleparam.h>
#include <linux/mutex.h>
#include <linux/of.h>
#include <linux/platform_device.h>
#include <linux/spinlock.h>

#define SKL_RIADENIE		0x00
#define SKL_HODNOTA		0x04
#define SKL_F_ZAPNUTY		BIT(16)
#define SKL_F_OPAKOVANY		BIT(12)
#define SKL_F_KROK		GENMASK(1, 0)
#define SKL_KROK_10US		1
#define SKL_TAKT_HZ		100000
#define SKL_MIN_KROKOV		2
#define SKL_MAX_KROKOV		0xfffe

static void __iomem *skl_reg;
static int skl_irq = -ENXIO;
static u32 skl_riadenie_pri_starte;
/* riadenie je spoločné pre časovače F–I – meníme len bity F, pod zámkom (volá sa aj s vypnutými prerušeniami) */
static DEFINE_RAW_SPINLOCK(skl_reg_zamok);
static DEFINE_MUTEX(skl_zamok);
static atomic_t skl_skuska = ATOMIC_INIT(0);
static DECLARE_COMPLETION(skl_hotovo);
static ktime_t skl_prislo;
static atomic64_t skl_preruseni = ATOMIC64_INIT(0);
static bool skl_bezi;
static char skl_stav[200] = "jadro nenašlo uzol /soc/bus@ffd00000/sklenik-casovac@f190";

static void skl_zastav(void)
{
	unsigned long f;

	raw_spin_lock_irqsave(&skl_reg_zamok, f);
	writel(readl(skl_reg + SKL_RIADENIE) & ~SKL_F_ZAPNUTY, skl_reg + SKL_RIADENIE);
	raw_spin_unlock_irqrestore(&skl_reg_zamok, f);
}

/* jednorazový odpočet `krokov` × 10 µs */
static void skl_spusti(u32 krokov)
{
	unsigned long f;
	u32 r;

	raw_spin_lock_irqsave(&skl_reg_zamok, f);
	r = readl(skl_reg + SKL_RIADENIE) & ~(SKL_F_ZAPNUTY | SKL_F_OPAKOVANY | SKL_F_KROK);
	r |= FIELD_PREP(SKL_F_KROK, SKL_KROK_10US);
	writel(r, skl_reg + SKL_RIADENIE);
	writel(krokov, skl_reg + SKL_HODNOTA);
	writel(r | SKL_F_ZAPNUTY, skl_reg + SKL_RIADENIE);
	raw_spin_unlock_irqrestore(&skl_reg_zamok, f);
}

static int skl_dalsia(unsigned long krokov, struct clock_event_device *ce)
{
	skl_spusti(krokov);
	return 0;
}

static int skl_stoj(struct clock_event_device *ce)
{
	skl_zastav();
	return 0;
}

static struct clock_event_device skl_ce = {
	.name			= "sklenik-casovac-f",
	/* DYNIRQ: prerušenie ide priamo jadru, ktoré sa má zobudiť (nie cez cpu0 a medzijadrové prerušenie) */
	.features		= CLOCK_EVT_FEAT_ONESHOT | CLOCK_EVT_FEAT_DYNIRQ,
	.rating			= 300,
	.set_next_event		= skl_dalsia,
	.set_state_shutdown	= skl_stoj,
	.set_state_oneshot	= skl_stoj,
	.set_state_oneshot_stopped = skl_stoj,
	.tick_resume		= skl_stoj,
};

static irqreturn_t skl_prerusenie(int irq, void *id)
{
	atomic64_inc(&skl_preruseni);
	/*
	 * Odpočet je jednorazový – zastaví sa aj tu. Ak ho medzitým iné jadro znova nastavilo, obsluha jadra Linux
	 * (tick_handle_oneshot_broadcast) prejde všetky spiace jadrá a časovač nastaví znova na najbližšie.
	 */
	skl_zastav();
	if (atomic_read(&skl_skuska)) {
		skl_prislo = ktime_get();
		complete(&skl_hotovo);
		return IRQ_HANDLED;
	}
	if (READ_ONCE(skl_bezi) && skl_ce.event_handler)
		skl_ce.event_handler(&skl_ce);
	return IRQ_HANDLED;
}

/* jedna skúška: odpočet `krokov` – vráti nameraný čas v µs alebo -1, ak prerušenie neprišlo do 200 ms */
static s64 skl_skus(u32 krokov)
{
	ktime_t start;
	long zostalo;

	reinit_completion(&skl_hotovo);
	atomic_set(&skl_skuska, 1);
	start = ktime_get();
	skl_spusti(krokov);
	zostalo = wait_for_completion_timeout(&skl_hotovo, msecs_to_jiffies(200));
	atomic_set(&skl_skuska, 0);
	skl_zastav();
	return zostalo ? ktime_us_delta(skl_prislo, start) : -1;
}

static bool skl_v_tolerancii(s64 us, s64 ciel)
{
	return us >= ciel - ciel * 3 / 100 - 50 && us <= ciel + ciel * 3 / 100 + 100;
}

static int skl_zapni(void)
{
	s64 kratka, dlha;

	if (!skl_reg)
		return -ENODEV;
	if (skl_riadenie_pri_starte & SKL_F_ZAPNUTY) {
		scnprintf(skl_stav, sizeof(skl_stav),
			  "nezapnutý – Timer F už niekto používa (riadenie pri štarte 0x%08x)",
			  skl_riadenie_pri_starte);
		return -EBUSY;
	}
	kratka = skl_skus(200);
	dlha = kratka >= 0 ? skl_skus(2000) : -1;
	if (kratka < 0 || dlha < 0) {
		scnprintf(skl_stav, sizeof(skl_stav),
			  "nezapnutý – Timer F neprerušil do 200 ms (prerušenie SPI 60 nedošlo)");
		return -EIO;
	}
	if (!skl_v_tolerancii(kratka, 2000) || !skl_v_tolerancii(dlha, 20000)) {
		scnprintf(skl_stav, sizeof(skl_stav),
			  "nezapnutý – skúška nevyšla: 2 ms trvali %lld µs, 20 ms trvalo %lld µs", kratka, dlha);
		return -EIO;
	}

	skl_ce.irq = skl_irq;
	skl_ce.cpumask = cpu_possible_mask;
	WRITE_ONCE(skl_bezi, true);
	clockevents_config_and_register(&skl_ce, SKL_TAKT_HZ, SKL_MIN_KROKOV, SKL_MAX_KROKOV);
	if (clockevent_state_detached(&skl_ce))
		scnprintf(skl_stav, sizeof(skl_stav),
			  "zaregistrovaný, ale jadro ho za spoločný časovač nevybralo (skúšky %lld a %lld µs)",
			  kratka, dlha);
	else
		scnprintf(skl_stav, sizeof(skl_stav),
			  "beží – spoločný časovač budenia (Timer F, krok 10 µs, skúšky 2 ms → %lld µs, 20 ms → %lld µs)",
			  kratka, dlha);
	pr_info("%s\n", skl_stav);
	return 0;
}

static int skl_nastav(const char *hodnota, const struct kernel_param *kp)
{
	bool chce;
	int ret = kstrtobool(hodnota, &chce);

	if (ret)
		return ret;
	mutex_lock(&skl_zamok);
	if (skl_bezi)
		ret = chce ? 0 : -EBUSY;	/* zaregistrovaný ostáva do reštartu */
	else if (chce)
		ret = skl_zapni();
	mutex_unlock(&skl_zamok);
	return ret;
}

static int skl_citaj(char *buf, const struct kernel_param *kp)
{
	return scnprintf(buf, PAGE_SIZE, "%d\n", READ_ONCE(skl_bezi) ? 1 : 0);
}

static const struct kernel_param_ops skl_ops = {
	.set = skl_nastav,
	.get = skl_citaj,
};

static int skl_citaj_stav(char *buf, const struct kernel_param *kp)
{
	int n;

	mutex_lock(&skl_zamok);
	n = scnprintf(buf, PAGE_SIZE, "%s · prerušení %lld\n", skl_stav, atomic64_read(&skl_preruseni));
	mutex_unlock(&skl_zamok);
	return n;
}

static const struct kernel_param_ops skl_stav_ops = {
	.get = skl_citaj_stav,
};

module_param_cb(zapnute, &skl_ops, NULL, 0644);
MODULE_PARM_DESC(zapnute, "spoločný časovač budenia (1 = zapnutý, do reštartu)");
module_param_cb(stav, &skl_stav_ops, NULL, 0444);
MODULE_PARM_DESC(stav, "výsledok skúšky časovača a počet prerušení");

static int skl_probe(struct platform_device *pdev)
{
	void __iomem *reg;
	int irq, ret;

	reg = devm_platform_ioremap_resource(pdev, 0);
	if (IS_ERR(reg))
		return PTR_ERR(reg);
	irq = platform_get_irq(pdev, 0);
	if (irq < 0)
		return irq;
	/* časovač ešte nebeží, prerušenie nepríde, kým ho KLÍČ nezapne */
	ret = devm_request_irq(&pdev->dev, irq, skl_prerusenie, IRQF_TIMER, "sklenik-casovac", &skl_ce);
	if (ret)
		return ret;

	mutex_lock(&skl_zamok);
	skl_reg = reg;
	skl_irq = irq;
	skl_riadenie_pri_starte = readl(reg + SKL_RIADENIE);
	scnprintf(skl_stav, sizeof(skl_stav), "pripravený – zapína ho KLÍČ (riadenie pri štarte 0x%08x)",
		  skl_riadenie_pri_starte);
	mutex_unlock(&skl_zamok);
	dev_info(&pdev->dev, "Timer F pripravený (riadenie 0x%08x), čaká na KLÍČ\n", skl_riadenie_pri_starte);
	return 0;
}

static const struct of_device_id skl_zhoda[] = {
	{ .compatible = "sklenik,casovac-budenia" },
	{ }
};

static struct platform_driver skl_ovladac = {
	.probe = skl_probe,
	.driver = {
		.name = "sklenik-casovac",
		.of_match_table = skl_zhoda,
		.suppress_bind_attrs = true,
	},
};
builtin_platform_driver(skl_ovladac);
