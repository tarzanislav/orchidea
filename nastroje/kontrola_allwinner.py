#!/usr/bin/env python3
"""Kontrola karty SKLENÍKA pre Tanix TX68 (Allwinner H618, R118).

Krabička sa v emulátore spustiť nedá, preto táto kontrola overí všetko, čo sa overiť dá bez nej:
  - zavádzač U-Boot (SPL v tvare eGON + hlavná časť) je od 8 KiB, kde ho hľadá zavádzací kód čipu,
    hlavička eGON má správny podpis a kontrolný súčet a celý zavádzač sa zmestí pred oddiel (4 MiB),
  - tabuľka oddielov je platná a oddiel FAT32 od 4 MiB je označený ako zavádzací,
  - extlinux.conf ukazuje na súbory, ktoré na karte sú (popis hardvéru podľa fdtfile zavádzača),
  - jadro je 64-bitové (Image pre ARM64) a popis hardvéru je pre H618 s menom Tanix TX68, so všetkými
    zásuvkami USB a s Wi-Fi bez napájania.

Použitie: python3 nastroje/kontrola_allwinner.py <obraz karty> <u-boot-sunxi-with-spl.bin> <priečinok oddielu>
Len štandardná knižnica Pythonu.
"""

import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from kontrola_odroid import nacitaj_extlinux, uzol_vlastnost, vlastnost_korena_dtb  # noqa: E402

ODDIEL_OD = 8192  # sektor, kde začína oddiel (4 MiB)
ZAVADZAC_OD = 8192  # bajt (sektor 16)
FDTFILE = "allwinner/sun50i-h618-transpeed-8k618-t.dtb"


def egon(spl: bytes) -> str | None:
    """Overí hlavičku eGON (podpis „eGON.BT0“ a kontrolný súčet). Vráti chybu alebo None."""
    if spl[4:12] != b"eGON.BT0":
        return f"chýba podpis eGON.BT0 ({spl[4:12]!r})"
    sucet, dlzka = struct.unpack_from("<II", spl, 12)
    if dlzka % 4 or dlzka > len(spl) or dlzka < 32:
        return f"neplatná dĺžka SPL {dlzka}"
    data = bytearray(spl[:dlzka])
    data[12:16] = struct.pack("<I", 0x5F0A6C39)
    spolu = sum(struct.unpack_from(f"<{dlzka // 4}I", data, 0)) & 0xFFFFFFFF
    if spolu != sucet:
        return f"kontrolný súčet {spolu:#x} ≠ {sucet:#x}"
    return None


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
        ZAVADZAC_OD + len(zavadzac) <= ODDIEL_OD * 512,
        f"zavádzač má {len(zavadzac)} B – od 8 KiB sa zmestí pred oddiel (4 MiB)",
        f"zavádzač má {len(zavadzac)} B – nezmestí sa pred oddiel",
    )
    over(
        obraz[ZAVADZAC_OD : ZAVADZAC_OD + len(zavadzac)] == zavadzac,
        "zavádzač je na karte celý od 8 KiB (sektor 16)",
        "zavádzač na karte nesedí so súborom",
    )
    chyba = egon(zavadzac)
    over(chyba is None, "SPL má platnú hlavičku eGON.BT0 a kontrolný súčet", f"SPL: {chyba}")
    over(obraz[510:512] == b"\x55\xaa", "tabuľka oddielov MBR je platná", "chýba podpis tabuľky oddielov")
    zaznam = obraz[446:462]
    zaciatok = struct.unpack_from("<I", zaznam, 8)[0]
    over(
        zaznam[0] == 0x80 and zaznam[4] == 0x0C and zaciatok == ODDIEL_OD,
        "oddiel FAT32 od 4 MiB, označený ako zavádzací",
        f"oddiel nesedí (príznak {zaznam[0]:#x}, typ {zaznam[4]:#x}, začiatok {zaciatok})",
    )

    e = nacitaj_extlinux(boot / "extlinux" / "extlinux.conf")
    for kluc in ("LINUX", "INITRD"):
        subor = e.get(kluc, "")
        over(
            bool(subor) and (boot / subor.lstrip("/")).is_file(),
            f"extlinux.conf {kluc} {subor} – súbor na karte je",
            f"extlinux.conf {kluc} {subor or '(chýba)'} – súbor na karte nie je",
        )
    over("rdinit=/init" in e.get("APPEND", ""), "jadro spustí KLÍČ (rdinit=/init)", "chýba rdinit=/init")
    jadro = boot / e.get("LINUX", "x").lstrip("/")
    if jadro.is_file():
        h = jadro.read_bytes()[:64]
        over(h[56:60] == b"ARM\x64", "jadro je 64-bitové (Image pre ARM64)", "jadro nie je Image pre ARM64")
    dtb_cesta = boot / e.get("FDTDIR", "/dtb").lstrip("/") / FDTFILE
    if not dtb_cesta.is_file():
        over(False, "", f"popis hardvéru {FDTFILE} na karte nie je")
    else:
        dtb = dtb_cesta.read_bytes()
        model = (vlastnost_korena_dtb(dtb, "model") or b"").rstrip(b"\0").decode(errors="replace")
        kompat = (vlastnost_korena_dtb(dtb, "compatible") or b"").split(b"\0")
        over(
            "Tanix TX68" in model and b"allwinner,sun50i-h618" in kompat,
            f"popis hardvéru: {model} (allwinner,sun50i-h618)",
            f"popis hardvéru nie je pre TX68: {model} {kompat}",
        )
        for uzol in ("usb@5310000", "usb@5310400", "usb@5311000", "usb@5311400"):
            stav = uzol_vlastnost(dtb, uzol, "status")
            over(stav == b"okay\0", f"USB {uzol} zapnuté", f"USB {uzol} nie je zapnuté ({stav!r})")
        stav = uzol_vlastnost(dtb, "mmc@4021000", "status")
        over(stav == b"disabled\0", "Wi-Fi (mmc@4021000) bez napájania (R112)", f"Wi-Fi nie je vypnutá ({stav!r})")
    if chyby:
        print(f"❌ chýb: {len(chyby)}")
        return 1
    print("✅ karta je pripravená pre Tanix TX68 (zavádzač a ovládače krabičky sa overia na skutočnom zariadení)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
