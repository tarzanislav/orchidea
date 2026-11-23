// SPDX-License-Identifier: GPL-2.0-only OR Apache-2.0
/*
 * SKLENÍK – takt obvodu obrazu VPU podľa potreby (R166), Amlogic G12A/SM1 (S905X3).
 *
 * Hlavné jadro nastaví takt VPU raz pri štarte na 666 MHz a nechá ho bežať stále – aj pri obraze
 * Full HD @ 24 Hz, aj pri vypnutom obraze. Takt VPU v čipe má dve rovnocenné cesty (vpu_0 a vpu_1) a
 * prepínač medzi nimi, ktorý mení cestu bez zákmitu. Tento ovládač nastaví voľnú cestu na nový takt a
 * prepne na ňu – obraz beží ďalej. KLÍČ zapisuje takt podľa režimu (Full HD 333 MHz, 4K 666 MHz) a pri
 * vypnutom obraze takt „zaparkuje“ (≈ 51 MHz) – registre ostanú dostupné, nič sa nemusí znovu nastavovať.
 *
 * Rozhranie: /sys/bus/platform/devices/sklenik-vpu/takt_mhz (čítanie: skutočný takt, zápis: želaný).
 * Vlastný kód projektu ORCHIDEA/SKLENÍK.
 */
#include <linux/clk.h>
#include <linux/device.h>
#include <linux/kernel.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/of.h>
#include <linux/platform_device.h>

#define NAJMENEJ_MHZ	24
#define NAJVIAC_MHZ	667

struct sklenik_vpu {
	struct clk *vpu;	/* prepínač ciest */
	struct clk *cesta[2];	/* vpu_0, vpu_1 (brány) */
	struct clk *vyber[2];	/* vpu_0_sel, vpu_1_sel */
	struct clk *zdroj[3];	/* fclk_div3, fclk_div4, fclk_div5 (posledné dva nepovinné) */
	struct mutex zamok;
	unsigned int prepnuti;
};

/* najlepší zdroj a takt: najvyšší zdroj/deliteľ ≤ hz (deliteľ 1 až 128) */
static unsigned long sklenik_vpu_vyber(struct sklenik_vpu *v, unsigned long hz, int *zdroj)
{
	unsigned long najlepsi = 0;
	int i;

	for (i = 0; i < ARRAY_SIZE(v->zdroj); i++) {
		unsigned long r, d, t;

		if (!v->zdroj[i])
			continue;
		r = clk_get_rate(v->zdroj[i]);
		if (!r)
			continue;
		d = clamp_t(unsigned long, DIV_ROUND_UP(r, hz), 1, 128);
		t = r / d;
		if (t <= hz && t > najlepsi) {
			najlepsi = t;
			*zdroj = i;
		}
	}
	return najlepsi;
}

static int sklenik_vpu_nastav(struct device *dev, struct sklenik_vpu *v, unsigned long hz)
{
	struct clk *rodic = clk_get_parent(v->vpu);
	int aktivna = clk_is_match(rodic, v->cesta[1]) ? 1 : 0;
	int volna = 1 - aktivna;
	int zdroj = 0;
	unsigned long takt = sklenik_vpu_vyber(v, hz, &zdroj);
	int ret;

	if (!takt)
		return -ERANGE;
	/* voľná cesta: zdroj a deliteľ podľa želaného taktu */
	ret = clk_set_parent(v->vyber[volna], v->zdroj[zdroj]);
	if (ret)
		return ret;
	ret = clk_set_rate(v->cesta[volna], takt);
	if (ret)
		return ret;
	/* prepnutie bez zákmitu – jadro samo zapne novú cestu pred prepnutím a starú potom vypne */
	ret = clk_set_parent(v->vpu, v->cesta[volna]);
	if (ret)
		return ret;
	v->prepnuti++;
	dev_dbg(dev, "takt VPU %lu Hz (cesta %d, zdroj %d)\n", clk_get_rate(v->vpu), volna, zdroj);
	return 0;
}

static ssize_t takt_mhz_show(struct device *dev, struct device_attribute *attr, char *buf)
{
	struct sklenik_vpu *v = dev_get_drvdata(dev);

	/* celé MHz nadol: 666,7 → 666, 333,3 → 333 (tak ich zapisuje KLÍČ) */
	return sysfs_emit(buf, "%lu\n", clk_get_rate(v->vpu) / 1000000);
}

static ssize_t takt_mhz_store(struct device *dev, struct device_attribute *attr,
			      const char *buf, size_t count)
{
	struct sklenik_vpu *v = dev_get_drvdata(dev);
	unsigned int mhz;
	int ret;

	ret = kstrtouint(buf, 10, &mhz);
	if (ret)
		return ret;
	if (mhz < NAJMENEJ_MHZ || mhz > NAJVIAC_MHZ)
		return -ERANGE;
	mutex_lock(&v->zamok);
	/* celé MHz zahŕňajú aj zlomok: „666“ dovolí 666,7 MHz */
	ret = sklenik_vpu_nastav(dev, v, (unsigned long)mhz * 1000000UL + 999999UL);
	mutex_unlock(&v->zamok);
	return ret ? ret : count;
}
static DEVICE_ATTR_RW(takt_mhz);

static ssize_t prepnuti_show(struct device *dev, struct device_attribute *attr, char *buf)
{
	struct sklenik_vpu *v = dev_get_drvdata(dev);

	return sysfs_emit(buf, "%u\n", v->prepnuti);
}
static DEVICE_ATTR_RO(prepnuti);

static struct attribute *sklenik_vpu_attrs[] = {
	&dev_attr_takt_mhz.attr,
	&dev_attr_prepnuti.attr,
	NULL,
};
ATTRIBUTE_GROUPS(sklenik_vpu);

static int sklenik_vpu_probe(struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	struct sklenik_vpu *v;

	v = devm_kzalloc(dev, sizeof(*v), GFP_KERNEL);
	if (!v)
		return -ENOMEM;
	mutex_init(&v->zamok);
	v->vpu = devm_clk_get(dev, "vpu");
	v->cesta[0] = devm_clk_get(dev, "vpu_0");
	v->cesta[1] = devm_clk_get(dev, "vpu_1");
	v->vyber[0] = devm_clk_get(dev, "vpu_0_sel");
	v->vyber[1] = devm_clk_get(dev, "vpu_1_sel");
	v->zdroj[0] = devm_clk_get(dev, "fclk_div3");
	v->zdroj[1] = devm_clk_get_optional(dev, "fclk_div4");
	v->zdroj[2] = devm_clk_get_optional(dev, "fclk_div5");
	if (IS_ERR(v->zdroj[1]))
		v->zdroj[1] = NULL;
	if (IS_ERR(v->zdroj[2]))
		v->zdroj[2] = NULL;
	if (IS_ERR(v->vpu) || IS_ERR(v->cesta[0]) || IS_ERR(v->cesta[1]) ||
	    IS_ERR(v->vyber[0]) || IS_ERR(v->vyber[1]) || IS_ERR(v->zdroj[0]))
		return dev_err_probe(dev, -ENODEV, "chýbajú takty VPU v popise hardvéru\n");
	platform_set_drvdata(pdev, v);
	dev_info(dev, "takt VPU %lu MHz – riadi KLÍČ (R166, R175)\n", clk_get_rate(v->vpu) / 1000000);
	return 0;
}

static const struct of_device_id sklenik_vpu_zhoda[] = {
	{ .compatible = "sklenik,vpu-takt" },
	{ }
};

static struct platform_driver sklenik_vpu_ovladac = {
	.probe = sklenik_vpu_probe,
	.driver = {
		.name = "sklenik-vpu",
		.of_match_table = sklenik_vpu_zhoda,
		.dev_groups = sklenik_vpu_groups,
	},
};
module_platform_driver(sklenik_vpu_ovladac);

MODULE_DESCRIPTION("SKLENÍK – takt obvodu obrazu VPU podľa potreby");
MODULE_LICENSE("GPL");
