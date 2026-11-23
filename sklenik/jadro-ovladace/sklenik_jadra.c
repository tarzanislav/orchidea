// SPDX-License-Identifier: GPL-2.0-only OR Apache-2.0
/*
 * SKLENÍK – hlboký spánok jadier procesora (R167), vlastný ovládač cpuidle pre Amlogic S905X3.
 *
 * Popis hardvéru S905X3 v hlavnom jadre nemá žiadne úsporné stavy jadier: nečinné jadro len čaká (WFI) –
 * hodiny stoja, ale jadro je stále napájané a jeho únik prúdu hreje (s teplotou rastie). Firmvér krabičky
 * (BL31) vie jadro na čas nečinnosti vypnúť cez PSCI CPU_SUSPEND – Android ho tak používa.
 *
 * Ovládač je pri štarte VYPNUTÝ. Zapne ho až KLÍČ zápisom 1 do
 * /sys/module/sklenik_jadra/parameters/zapnute, keď beží hardvérový strážny časovač a na karte je poistka:
 * keby firmvér pri vypnutí jadra zamrzol, strážny časovač krabičku reštartuje, KLÍČ poistku nájde a hlboký
 * spánok už nezapne. Zápis 0 ho kedykoľvek vypne (tepelný pokus).
 *
 * Stavy:
 *   0 WFI      – jadro čaká, hodiny stoja (ako doteraz),
 *   1 vypnuté  – PSCI CPU_SUSPEND, pôvodný formát parametra: typ 1 (vypnutie), úroveň 0 (len jadro),
 *                identifikátor stavu 0. Miestny časovač jadra stojí – budenie preberie spoločný časovač
 *                jadra Linux (na arm64 hrtimer na jadre, ktoré nespí hlboko).
 *
 * Firmvér sa overí pred zapnutím: musí mať CPU_SUSPEND a pôvodný formát parametra. Pri rozšírenom formáte
 * (identifikátory stavov určuje výrobca) sa ovládač nezapne – nič sa nehádže naslepo. Ak firmvér stav
 * odmietne, jadro ostane vo WFI a počítadlo „rejected“ v /sys/devices/system/cpu/cpu*\/cpuidle/state1 to ukáže.
 *
 * Hlboký stav celého zhluku (úroveň 1) zámerne nie: zhluk sa môže vypnúť, len keď spia všetky jadrá, a jadro
 * so spoločným časovačom nikdy nespí hlboko – bez vlastného časovača budenia mimo zhluku by stav nikdy nenastal.
 */

#define pr_fmt(fmt) "sklenik-jadra: " fmt

#include <linux/cpu_pm.h>
#include <linux/cpuidle.h>
#include <linux/cpumask.h>
#include <linux/init.h>
#include <linux/kernel.h>
#include <linux/moduleparam.h>
#include <linux/mutex.h>
#include <linux/psci.h>
#include <linux/string.h>
#include <linux/sysfs.h>
#include <uapi/linux/psci.h>

#include <asm/cpuidle.h>

/* pôvodný formát PSCI 0.2: bit 16 = typ stavu (1 = vypnutie), bity 24–25 = úroveň (0 = jadro), 0–15 = id */
#define SKL_JADRO_VYPNUTE	(PSCI_POWER_STATE_TYPE_POWER_DOWN << PSCI_0_2_POWER_STATE_TYPE_SHIFT)
/* bit 30 je typ stavu len v rozšírenom formáte – v pôvodnom je to rezervovaný bit (neplatný parameter) */
#define SKL_BIT_ROZSIRENEHO	BIT(30)

static DEFINE_MUTEX(skl_zamok);
static bool skl_zapnute;
static char skl_stav[128] = "vypnutý – zapína ho KLÍČ so strážnym časovačom";

static __cpuidle int skl_vstup(struct cpuidle_device *dev, struct cpuidle_driver *drv, int idx)
{
	/* stav 0: WFI; stav 1: uloženie stavu jadra (cpu_pm), PSCI CPU_SUSPEND, obnova */
	return CPU_PM_CPU_IDLE_ENTER_PARAM_RCU(psci_cpu_suspend_enter, idx, SKL_JADRO_VYPNUTE);
}

/*
 * Oneskorenia sú zámerne opatrné (výrobca ich pre tento firmvér nezverejnil): jadro sa vypne, len keď
 * správca nečinnosti čaká pokoj aspoň 6 ms. Pri videu (snímka každých 17–42 ms) aj v pokoji to stačí;
 * počítadlá „above“ a „below“ v sysfs ukážu, či by sa oplatili kratšie.
 */
static struct cpuidle_driver skl_ovladac = {
	.name = "sklenik_jadra",
	.states = {
		[0] = {
			.enter			= skl_vstup,
			.exit_latency		= 1,
			.target_residency	= 1,
			.power_usage		= UINT_MAX,
			.name			= "WFI",
			.desc			= "ARM WFI",
		},
		[1] = {
			.enter			= skl_vstup,
			.flags			= CPUIDLE_FLAG_TIMER_STOP | CPUIDLE_FLAG_RCU_IDLE,
			.exit_latency		= 1500,
			.target_residency	= 6000,
			.name			= "jadro-vyp",
			.desc			= "jadro vypnute (PSCI)",
		},
	},
	.state_count = 2,
	.safe_state_index = 0,
};

static int skl_over_firmver(void)
{
	u32 verzia;

	if (!psci_ops.cpu_suspend) {
		strscpy(skl_stav, "nezapnutý – firmvér nemá PSCI CPU_SUSPEND", sizeof(skl_stav));
		return -EOPNOTSUPP;
	}
	if (psci_power_state_is_valid(SKL_BIT_ROZSIRENEHO)) {
		strscpy(skl_stav, "nezapnutý – firmvér má rozšírený formát PSCI (stavy určuje výrobca)",
			sizeof(skl_stav));
		return -EOPNOTSUPP;
	}
	if (!psci_power_state_is_valid(SKL_JADRO_VYPNUTE)) {
		strscpy(skl_stav, "nezapnutý – parameter stavu neplatný", sizeof(skl_stav));
		return -EINVAL;
	}
	verzia = psci_ops.get_version ? psci_ops.get_version() : 0;
	snprintf(skl_stav, sizeof(skl_stav), "zapnutý – PSCI %u.%u, pôvodný formát, stav 0x%08x",
		 PSCI_VERSION_MAJOR(verzia), PSCI_VERSION_MINOR(verzia), SKL_JADRO_VYPNUTE);
	return 0;
}

static int skl_nastav(const char *hodnota, const struct kernel_param *kp)
{
	bool chce;
	int ret = kstrtobool(hodnota, &chce);

	if (ret)
		return ret;
	mutex_lock(&skl_zamok);
	if (chce && !skl_zapnute) {
		ret = skl_over_firmver();
		if (!ret) {
			skl_ovladac.cpumask = (struct cpumask *)cpu_possible_mask;
			ret = cpuidle_register(&skl_ovladac, NULL);
			if (ret)
				snprintf(skl_stav, sizeof(skl_stav), "nezapnutý – registrácia zlyhala (%d)", ret);
		}
		if (!ret)
			skl_zapnute = true;
		pr_info("%s\n", skl_stav);
	} else if (!chce && skl_zapnute) {
		cpuidle_unregister(&skl_ovladac);
		skl_zapnute = false;
		strscpy(skl_stav, "vypnutý (KLÍČ)", sizeof(skl_stav));
		pr_info("%s\n", skl_stav);
	}
	mutex_unlock(&skl_zamok);
	return ret;
}

static int skl_citaj(char *buf, const struct kernel_param *kp)
{
	return sysfs_emit(buf, "%d\n", skl_zapnute);
}

static const struct kernel_param_ops skl_ops = {
	.set = skl_nastav,
	.get = skl_citaj,
};

module_param_cb(zapnute, &skl_ops, NULL, 0644);
MODULE_PARM_DESC(zapnute, "hlboký spánok jadier (1 = zapnutý)");
module_param_string(stav, skl_stav, sizeof(skl_stav), 0444);
MODULE_PARM_DESC(stav, "výsledok overenia firmvéru");
