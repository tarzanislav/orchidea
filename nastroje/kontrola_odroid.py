#!/usr/bin/env python3
"""Kontrola karty SKLENÍKA pre Odroid C4 (R88).

Odroid C4 v emulátore spustiť nejde, preto táto kontrola overí všetko, čo sa overiť dá bez dosky:
  - zavádzač (U-Boot podpísaný nástrojom Amlogic) je na karte presne tam, kde ho hľadá procesor S905X3:
    prvých 440 bajtov 1. sektora a od 2. sektora ďalej, a celý sa zmestí pred oddiel (4 MiB),
  - tabuľka oddielov zostala nedotknutá (Raspberry Pi aj Windows kartu prečítajú ako predtým),
  - oddiel je označený ako zavádzací (U-Boot hľadá extlinux/extlinux.conf na takom oddiele),
  - extlinux.conf ukazuje na súbory, ktoré na karte sú,
  - jadro je 64-bitové (Image pre ARM64) a popis hardvéru je naozaj pre Odroid C4.

Použitie: python3 nastroje/kontrola_odroid.py <obraz karty> <u-boot.bin.sd.bin> <priečinok zavádzacieho oddielu>
Len štandardná knižnica Pythonu.
"""

import struct
import sys
from pathlib import Path

ODDIEL_OD = 8192  # sektor, kde začína oddiel (4 MiB)


def nacitaj_extlinux(cesta: Path) -> dict:
    """Vráti kľúče LINUX, INITRD, FDT, APPEND prvého záznamu LABEL."""
    hodnoty = {}
    v_zazname = False
    for riadok in cesta.read_text(encoding="ascii").splitlines():
        riadok = riadok.strip()
        if not riadok or riadok.startswith("#"):
            continue
        kluc, _, hodnota = riadok.partition(" ")
        kluc = kluc.upper()
        if kluc == "LABEL":
            if v_zazname:
                break
            v_zazname = True
        elif v_zazname and kluc in {"LINUX", "KERNEL", "INITRD", "FDT", "APPEND"}:
            hodnoty[kluc] = hodnota.strip()
    return hodnoty


def vlastnost_korena_dtb(dtb: bytes, meno: str) -> bytes | None:
    """Prečíta vlastnosť koreňového uzla z popisu hardvéru (formát FDT)."""
    magic, _, struct_od, retazce_od = struct.unpack_from(">IIII", dtb, 0)
    if magic != 0xD00DFEED:
        return None
    i = struct_od
    hlbka = 0
    while i < len(dtb):
        (token,) = struct.unpack_from(">I", dtb, i)
        i += 4
        if token == 1:  # začiatok uzla
            koniec = dtb.index(b"\0", i)
            i = (koniec + 4) & ~3
            hlbka += 1
        elif token == 2:  # koniec uzla
            hlbka -= 1
        elif token == 3:  # vlastnosť
            dlzka, meno_od = struct.unpack_from(">II", dtb, i)
            i += 8
            hodnota = dtb[i : i + dlzka]
            i = (i + dlzka + 3) & ~3
            m = dtb[retazce_od + meno_od : dtb.index(b"\0", retazce_od + meno_od)].decode()
            if hlbka == 1 and m == meno:
                return hodnota
        elif token == 4:  # nič
            continue
        elif token == 9:  # koniec
            break
    return None


def vlastnosti_uzla(dtb: bytes, meno_uzla: str) -> set:
    """Mená vlastností uzla s daným menom (napr. „mmc@ffe05000“) v popise hardvéru (FDT)."""
    _, _, struct_od, retazce_od = struct.unpack_from(">IIII", dtb, 0)
    i, cesta, vysledok = struct_od, [], set()
    while i < len(dtb):
        (token,) = struct.unpack_from(">I", dtb, i)
        i += 4
        if token == 1:
            koniec = dtb.index(b"\0", i)
            cesta.append(dtb[i:koniec].decode())
            i = (koniec + 4) & ~3
        elif token == 2:
            cesta.pop()
        elif token == 3:
            dlzka, meno_od = struct.unpack_from(">II", dtb, i)
            i = (i + 8 + dlzka + 3) & ~3
            if cesta and cesta[-1] == meno_uzla:
                vysledok.add(dtb[retazce_od + meno_od : dtb.index(b"\0", retazce_od + meno_od)].decode())
        elif token == 9:
            break
    return vysledok


def main() -> int:
    obraz = Path(sys.argv[1]).read_bytes()[: (ODDIEL_OD + 1) * 512]
    zavadzac = Path(sys.argv[2]).read_bytes()
    boot = Path(sys.argv[3])
    chyby = []

    def over(podmienka: bool, dobre: str, zle: str) -> None:
        print(f"- ✅ {dobre}" if podmienka else f"- ❌ {zle}")
        if not podmienka:
            chyby.append(zle)

    over(
        len(zavadzac) <= ODDIEL_OD * 512,
        f"zavádzač má {len(zavadzac)} B – zmestí sa pred oddiel (4 MiB)",
        f"zavádzač má {len(zavadzac)} B – nezmestí sa pred oddiel",
    )
    over(
        obraz[:440] == zavadzac[:440] and obraz[512 : len(zavadzac)] == zavadzac[512:],
        "zavádzač je na karte celý a na správnom mieste (1. sektor do 440 B, od 2. sektora)",
        "zavádzač na karte nesedí so súborom",
    )
    over(obraz[510:512] == b"\x55\xaa", "tabuľka oddielov MBR je platná", "chýba podpis tabuľky oddielov")
    zaznam = obraz[446:462]
    zaciatok = struct.unpack_from("<I", zaznam, 8)[0]
    over(
        zaznam[0] == 0x80 and zaznam[4] == 0x0C and zaciatok == ODDIEL_OD,
        "oddiel FAT32 od 4 MiB, označený ako zavádzací",
        f"oddiel nesedí (príznak {zaznam[0]:#x}, typ {zaznam[4]:#x}, začiatok {zaciatok})",
    )
    over(
        all(b == 0 for b in obraz[462:510]),
        "ostatné záznamy tabuľky oddielov sú prázdne",
        "zavádzač prepísal tabuľku oddielov",
    )

    e = nacitaj_extlinux(boot / "extlinux" / "extlinux.conf")
    for kluc in ("LINUX", "INITRD", "FDT"):
        subor = e.get(kluc, "")
        over(
            bool(subor) and (boot / subor.lstrip("/")).is_file(),
            f"extlinux.conf {kluc} {subor} – súbor na karte je",
            f"extlinux.conf {kluc} {subor or '(chýba)'} – súbor na karte nie je",
        )
    over("rdinit=/init" in e.get("APPEND", ""), "jadro spustí KLÍČ (rdinit=/init)", "chýba rdinit=/init")

    jadro_cesta = boot / e.get("LINUX", "x").lstrip("/")
    if jadro_cesta.is_file():
        hlavicka = jadro_cesta.read_bytes()[:64]
        over(hlavicka[56:60] == b"ARM\x64", "jadro je 64-bitové (Image pre ARM64)", "jadro nie je Image pre ARM64")
        priznaky = struct.unpack_from("<Q", hlavicka, 24)[0]
        over((priznaky >> 1) & 3 == 1, "jadro má stránky 4 kB", "jadro nemá stránky 4 kB")
    dtb_cesta = boot / e.get("FDT", "x").lstrip("/")
    if dtb_cesta.is_file():
        dtb = dtb_cesta.read_bytes()
        model = (vlastnost_korena_dtb(dtb, "model") or b"").rstrip(b"\0").decode(errors="replace")
        kompat = (vlastnost_korena_dtb(dtb, "compatible") or b"").split(b"\0")
        over(
            b"hardkernel,odroid-c4" in kompat and b"amlogic,sm1" in kompat,
            f"popis hardvéru: {model} (hardkernel,odroid-c4, amlogic,sm1)",
            f"popis hardvéru nie je pre Odroid C4: {model} {kompat}",
        )
        sd = vlastnosti_uzla(dtb, "mmc@ffe05000")
        uhs = sorted(v for v in sd if v.startswith("sd-uhs"))
        over(
            bool(sd) and not uhs,
            "SD karta bez rýchlych režimov UHS – reštart zo SD funguje (R96)",
            f"SD karta má režimy {uhs or 'uzol chýba'} – reštart zo SD by zamrzol",
        )
    if chyby:
        print(f"❌ chýb: {len(chyby)}")
        return 1
    print("✅ karta je pripravená pre Odroid C4 (zavádzač a ovládače dosky sa overia na skutočnom Odroide)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
