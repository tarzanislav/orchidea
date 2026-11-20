#!/usr/bin/env python3
"""Kontrola univerzálnej karty SKLENÍKA (R86) – „firmvér na skúšku“.

Emulátor nevie prečítať config.txt tak ako firmvér Raspberry Pi, preto táto kontrola prejde config.txt
za každý model rovnako ako firmvér (časti [pi…], neskoršie prepíše skoršie) a overí, že:
  - jadro a systém v pamäti (initramfs) pre model sú na karte,
  - jadro je pre správny procesor (32-bit zImage / 64-bit Image, na RPi 5 stránky 16 kB),
  - KLÍČ v systéme je pre správny procesor (ARMv6 pre RPi 1 a Zero, ARMv7 pre RPi 2, AArch64 inak)
    a na RPi 5 má úseky zarovnané aspoň na 16 kB,
  - na karte je popis hardvéru (.dtb) modelu.

Použitie: python3 nastroje/kontrola_karty.py <priečinok so súbormi zavádzacieho oddielu>
Len štandardná knižnica Pythonu.
"""

import gzip
import io
import struct
import sys
from pathlib import Path

# model: (filtre config.txt, ktoré pre model platia; očakávaný procesor; popisy hardvéru)
MODELY = {
    "RPi 1 A/B": ({"pi1"}, "armv6", ["bcm2708-rpi-b.dtb"]),
    "RPi 1 A+/B+": ({"pi1"}, "armv6", ["bcm2708-rpi-b-plus.dtb"]),
    "RPi Zero": ({"pi0"}, "armv6", ["bcm2708-rpi-zero.dtb"]),
    "RPi Zero W": ({"pi0", "pi0w"}, "armv6", ["bcm2708-rpi-zero-w.dtb"]),
    "RPi 2 B": ({"pi2"}, "armv7", ["bcm2709-rpi-2-b.dtb"]),
    "RPi 2 B v1.2": ({"pi2"}, "armv7", ["bcm2710-rpi-2-b.dtb"]),
    "RPi 3 B": ({"pi3"}, "aarch64", ["bcm2710-rpi-3-b.dtb"]),
    "RPi 3 B+": ({"pi3", "pi3+"}, "aarch64", ["bcm2710-rpi-3-b-plus.dtb"]),
    "RPi Zero 2 W": ({"pi0", "pi0w", "pi02"}, "aarch64", ["bcm2710-rpi-zero-2-w.dtb"]),
    "RPi 4 B": ({"pi4"}, "aarch64", ["bcm2711-rpi-4-b.dtb"]),
    "RPi 400": ({"pi4", "pi400"}, "aarch64", ["bcm2711-rpi-400.dtb"]),
    "RPi 5": ({"pi5"}, "aarch64-16k", ["bcm2712-rpi-5-b.dtb", "bcm2712d0-rpi-5-b.dtb"]),
    "RPi 500": ({"pi5", "pi500"}, "aarch64-16k", []),
}

# filtre modelov (ostatné druhy filtrov SKLENÍK nepoužíva – zistí sa ako chyba)
FILTRE_MODELOV = {"pi1", "pi2", "pi3", "pi3+", "pi4", "pi400", "pi5", "pi500", "pi0", "pi0w", "pi02",
                  "cm4", "cm5"}


def nastavenie(config: str, filtre: set) -> dict:
    """Nastavenie, ktoré z config.txt uvidí model so zadanými filtrami."""
    platne = True
    vysledok = {}
    for cislo, riadok in enumerate(config.splitlines(), 1):
        riadok = riadok.split("#", 1)[0].strip()
        if not riadok:
            continue
        if riadok.startswith("["):
            filter_ = riadok.strip("[]").strip().lower()
            if filter_ == "all":
                platne = True
            elif filter_ == "none":
                platne = False
            elif filter_ in FILTRE_MODELOV:
                platne = filter_ in filtre
            else:
                raise ValueError(f"config.txt:{cislo}: neznámy filter [{filter_}]")
            continue
        if not platne:
            continue
        if riadok.startswith("initramfs "):
            casti = riadok.split()
            vysledok["initramfs"] = casti[1]
        elif riadok.startswith("dtoverlay="):
            vysledok.setdefault("dtoverlay", []).append(riadok.split("=", 1)[1].split(",")[0].strip())
        elif "=" in riadok:
            k, v = riadok.split("=", 1)
            vysledok[k.strip()] = v.strip()
    return vysledok


def druh_jadra(data: bytes) -> str:
    """32-bit zImage alebo 64-bit Image (aj komprimovaný gzip) a veľkosť stránky."""
    if len(data) > 0x28 and struct.unpack_from("<I", data, 0x24)[0] == 0x016F2818:
        return "arm32"
    if data[:2] == b"\x1f\x8b":
        data = gzip.GzipFile(fileobj=io.BytesIO(data)).read(64)
    if len(data) >= 64 and data[56:60] == b"ARM\x64":
        stranka = (struct.unpack_from("<Q", data, 24)[0] >> 1) & 3
        return {1: "arm64-4k", 2: "arm64-16k", 3: "arm64-64k"}.get(stranka, "arm64")
    return "neznámy"


def subory_cpio(data: bytes) -> dict:
    """Súbory v archíve cpio newc."""
    subory = {}
    i = 0
    while i + 110 <= len(data):
        if data[i:i + 6] != b"070701":
            raise ValueError("initramfs nie je cpio newc")
        pole = [int(data[i + 6 + 8 * k:i + 14 + 8 * k], 16) for k in range(13)]
        velkost, dlzka_mena = pole[6], pole[11]
        meno = data[i + 110:i + 110 + dlzka_mena - 1].decode()
        j = i + 110 + dlzka_mena
        j += -j % 4
        if meno == "TRAILER!!!":
            break
        subory[meno] = data[j:j + velkost]
        j += velkost
        i = j + (-j % 4)
    return subory


def uleb(data: bytes, i: int):
    v = posun = 0
    while True:
        b = data[i]
        i += 1
        v |= (b & 0x7F) << posun
        posun += 7
        if b < 0x80:
            return v, i


def arm_architektura(elf: bytes) -> int:
    """Tag_CPU_arch z úseku .ARM.attributes (6 = ARMv6, 7 = ARMv6KZ, 9 = ARMv6K, 10 = ARMv7)."""
    e_shoff, = struct.unpack_from("<I", elf, 0x20)
    e_shentsize, e_shnum, e_shstrndx = struct.unpack_from("<HHH", elf, 0x2E)
    useky = [struct.unpack_from("<IIIIII", elf, e_shoff + k * e_shentsize) for k in range(e_shnum)]
    mena = useky[e_shstrndx]
    for meno_i, typ, _, _, posun, velkost in useky:
        koniec = elf.index(b"\0", mena[4] + meno_i)
        if elf[mena[4] + meno_i:koniec] != b".ARM.attributes":
            continue
        a = elf[posun:posun + velkost]
        if a[:1] != b"A":
            break
        i = 1
        while i < len(a):
            dlzka, = struct.unpack_from("<I", a, i)
            koniec_pod = i + dlzka
            j = a.index(b"\0", i + 4) + 1
            if a[i + 4:j - 1] == b"aeabi":
                while j < koniec_pod:
                    znacka = a[j]
                    dlzka_casti, = struct.unpack_from("<I", a, j + 1)
                    k, koniec_casti = j + 5, j + dlzka_casti
                    while znacka == 1 and k < koniec_casti:
                        t, k = uleb(a, k)
                        if t in (4, 5, 67) or (t > 32 and t % 2 == 1):
                            k = a.index(b"\0", k) + 1
                        elif t == 32:
                            _, k = uleb(a, k)
                            k = a.index(b"\0", k) + 1
                        else:
                            v, k = uleb(a, k)
                            if t == 6:
                                return v
                    j = koniec_casti
            i = koniec_pod
    return -1


def druh_programu(elf: bytes) -> str:
    if elf[:4] != b"\x7fELF":
        return "nie je ELF"
    trieda, = struct.unpack_from("B", elf, 4)
    stroj, = struct.unpack_from("<H", elf, 18)
    if trieda == 2 and stroj == 183:
        e_phoff, = struct.unpack_from("<Q", elf, 0x20)
        e_phentsize, e_phnum = struct.unpack_from("<HH", elf, 0x36)
        zarovnanie = min(
            struct.unpack_from("<Q", elf, e_phoff + k * e_phentsize + 48)[0]
            for k in range(e_phnum)
            if struct.unpack_from("<I", elf, e_phoff + k * e_phentsize)[0] == 1
        )
        return "aarch64" if zarovnanie >= 16384 else f"aarch64 (zarovnanie len {zarovnanie} B)"
    if trieda == 1 and stroj == 40:
        arch = arm_architektura(elf)
        if arch in (6, 7, 9):
            return "armv6"
        if arch == 10:
            return "armv7"
        return f"arm (Tag_CPU_arch {arch})"
    return f"iný procesor ({trieda}, {stroj})"


def zodpoveda(program: str, ocakavany: str) -> bool:
    if ocakavany == "aarch64":
        return program.startswith("aarch64")
    if ocakavany == "aarch64-16k":
        # jadro RPi 5 má stránky 16 kB – úseky programu musia byť zarovnané aspoň na 16 kB
        return program == "aarch64"
    return program == ocakavany


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 1
    koren = Path(sys.argv[1])
    config = (koren / "config.txt").read_text()
    chyby = 0
    pamat = {}
    print("| Model | Jadro | Systém v pamäti | Výsledok |")
    print("|---|---|---|---|")
    for model, (filtre, procesor, dtb) in MODELY.items():
        n = nastavenie(config, filtre)
        jadro, initramfs = n.get("kernel"), n.get("initramfs")
        problemy = []
        if not jadro or not (koren / jadro).is_file():
            problemy.append(f"chýba jadro {jadro}")
        if not initramfs or not (koren / initramfs).is_file():
            problemy.append(f"chýba systém {initramfs}")
        if not problemy:
            if jadro not in pamat:
                pamat[jadro] = druh_jadra((koren / jadro).read_bytes())
            if initramfs not in pamat:
                subory = subory_cpio(gzip.decompress((koren / initramfs).read_bytes()))
                pamat[initramfs] = druh_programu(subory.get("init", b""))
            dj, dp = pamat[jadro], pamat[initramfs]
            bit64 = n.get("arm_64bit", "1" if procesor.startswith("aarch64") and filtre & {"pi4", "pi5"} else "0")
            if procesor.startswith("aarch64"):
                if not dj.startswith("arm64"):
                    problemy.append(f"jadro je {dj}, treba 64-bit")
                if procesor == "aarch64-16k" and dj != "arm64-16k":
                    problemy.append(f"jadro pre RPi 5 má {dj} (čaká sa 16 kB)")
                if bit64 != "1" and not filtre & {"pi5"}:
                    problemy.append("chýba arm_64bit=1")
            else:
                if dj != "arm32":
                    problemy.append(f"jadro je {dj}, treba 32-bit")
                if bit64 == "1":
                    problemy.append("arm_64bit=1 pri 32-bitovom jadre")
            if not zodpoveda(dp, procesor):
                problemy.append(f"KLÍČ je {dp}, treba {procesor}")
            for d in dtb:
                if not (koren / d).is_file():
                    problemy.append(f"chýba {d}")
            for o in n.get("dtoverlay", []):
                if not (koren / "overlays" / f"{o}.dtbo").is_file():
                    problemy.append(f"chýba prekrytie overlays/{o}.dtbo")
        else:
            dj = dp = "–"
        vysledok = "✅" if not problemy else "❌ " + "; ".join(problemy)
        chyby += bool(problemy)
        print(f"| {model} | {jadro} ({dj}) | {initramfs} ({dp}) | {vysledok} |")
    print()
    print("✅ každý model dostane svoje jadro a svoj systém" if not chyby else f"❌ chybných modelov: {chyby}")
    return 1 if chyby else 0


if __name__ == "__main__":
    sys.exit(main())
