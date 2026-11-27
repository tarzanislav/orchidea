#!/usr/bin/env python3
"""Kontrola karty SKLENÍKA pre dosky Amlogic S905X3: Odroid C4 (R88) a X96 Max Plus (R117).

Ani jedna doska sa v emulátore spustiť nedá, preto táto kontrola overí všetko, čo sa overiť dá bez dosky:
  - zavádzač (U-Boot podpísaný nástrojom Amlogic) je na karte presne tam, kde ho hľadá procesor S905X3:
    prvých 440 bajtov 1. sektora a od 2. sektora ďalej, a celý sa zmestí pred oddiel (4 MiB),
  - tabuľka oddielov zostala nedotknutá (Raspberry Pi aj Windows kartu prečítajú ako predtým),
  - oddiel je označený ako zavádzací (U-Boot hľadá extlinux/extlinux.conf na takom oddiele),
  - extlinux.conf ukazuje na súbory, ktoré na karte sú (popis hardvéru podľa fdtfile každého zavádzača),
  - jadro je 64-bitové (Image pre ARM64) a popisy hardvéru sú naozaj pre Odroid C4 a krabičku X96,
  - X96 Max Plus: skripty pôvodného zavádzača krabičky sú platné obrazy U-Boot (typ skript, kontrolné
    súčty sedia) a spúšťajú náš u-boot.ext; ten nikdy nezapisuje nastavenie a hľadá správny popis hardvéru.

Použitie: python3 nastroje/kontrola_odroid.py <obraz karty> <u-boot.bin.sd.bin> <priečinok zavádzacieho oddielu>
Len štandardná knižnica Pythonu.
"""

import struct
import sys
import zlib
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
        elif v_zazname and kluc in {"LINUX", "KERNEL", "INITRD", "FDT", "FDTDIR", "APPEND"}:
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


def skript_ubootu(data: bytes) -> tuple[str | None, str]:
    """Overí obraz U-Boot typu skript (mkimage -T script). Vráti (chyba alebo None, text skriptu)."""
    if len(data) < 72:
        return "príliš krátky", ""
    magic, hcrc, _, velkost, _, _, dcrc, _, _, typ, komp = struct.unpack_from(">IIIIIIIBBBB", data, 0)
    if magic != 0x27051956:
        return f"nie je obraz U-Boot (magic {magic:#x})", ""
    hlavicka = bytearray(data[:64])
    hlavicka[4:8] = b"\0\0\0\0"
    if zlib.crc32(hlavicka) != hcrc:
        return "kontrolný súčet hlavičky nesedí", ""
    telo = data[64 : 64 + velkost]
    if len(telo) != velkost or zlib.crc32(telo) != dcrc:
        return "kontrolný súčet obsahu nesedí", ""
    if typ != 6 or komp != 0:
        return f"typ {typ} / kompresia {komp} – čaká sa skript bez kompresie", ""
    # obsah skriptu: dĺžky častí (32 b) ukončené nulou, potom časti
    dlzky, i = [], 0
    while True:
        (d,) = struct.unpack_from(">I", telo, i)
        i += 4
        if d == 0:
            break
        dlzky.append(d)
    return None, telo[i : i + dlzky[0]].decode("ascii", errors="replace") if dlzky else ""


def uzol_vlastnost(dtb: bytes, meno_uzla: str, vlastnost: str) -> bytes | None:
    """Hodnota vlastnosti uzla s daným menom (prvý výskyt)."""
    _, _, struct_od, retazce_od = struct.unpack_from(">IIII", dtb, 0)
    i, cesta = struct_od, []
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
            hodnota = dtb[i + 8 : i + 8 + dlzka]
            i = (i + 8 + dlzka + 3) & ~3
            m = dtb[retazce_od + meno_od : dtb.index(b"\0", retazce_od + meno_od)].decode()
            if cesta and cesta[-1] == meno_uzla and m == vlastnost:
                return hodnota
        elif token == 9:
            break
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
    for kluc in ("LINUX", "INITRD"):
        subor = e.get(kluc, "")
        over(
            bool(subor) and (boot / subor.lstrip("/")).is_file(),
            f"extlinux.conf {kluc} {subor} – súbor na karte je",
            f"extlinux.conf {kluc} {subor or '(chýba)'} – súbor na karte nie je",
        )
    fdtdir = e.get("FDTDIR", "")
    over(
        bool(fdtdir) and "FDT" not in e,
        f"extlinux.conf FDTDIR {fdtdir} – popis hardvéru si vyberie zavádzač podľa svojej dosky",
        "extlinux.conf nemá FDTDIR (alebo má aj pevné FDT)",
    )
    over("rdinit=/init" in e.get("APPEND", ""), "jadro spustí KLÍČ (rdinit=/init)", "chýba rdinit=/init")

    jadro_cesta = boot / e.get("LINUX", "x").lstrip("/")
    if jadro_cesta.is_file():
        hlavicka = jadro_cesta.read_bytes()[:64]
        over(hlavicka[56:60] == b"ARM\x64", "jadro je 64-bitové (Image pre ARM64)", "jadro nie je Image pre ARM64")
        priznaky = struct.unpack_from("<Q", hlavicka, 24)[0]
        over((priznaky >> 1) & 3 == 1, "jadro má stránky 4 kB", "jadro nemá stránky 4 kB")

    def popis(subor: str, kompat_dosky: bytes, model_ma: str, bez_uhs: bool, bez_wifi: bool) -> None:
        cesta = boot / fdtdir.lstrip("/") / subor
        if not cesta.is_file():
            over(False, "", f"{fdtdir}/{subor} na karte nie je")
            return
        dtb = cesta.read_bytes()
        model = (vlastnost_korena_dtb(dtb, "model") or b"").rstrip(b"\0").decode(errors="replace")
        kompat = (vlastnost_korena_dtb(dtb, "compatible") or b"").split(b"\0")
        over(
            kompat_dosky in kompat and b"amlogic,sm1" in kompat and model_ma in model,
            f"{subor}: {model} ({kompat_dosky.decode()}, amlogic,sm1)",
            f"{subor} nie je správny popis: {model} {kompat}",
        )
        if bez_uhs:
            sd = vlastnosti_uzla(dtb, "mmc@ffe05000")
            uhs = sorted(v for v in sd if v.startswith("sd-uhs"))
            over(
                bool(sd) and not uhs,
                f"{subor}: SD karta bez rýchlych režimov UHS – reštart zo SD funguje (R96)",
                f"{subor}: SD karta má režimy {uhs or 'uzol chýba'} – reštart zo SD by zamrzol",
            )
            hz = uzol_vlastnost(dtb, "mmc@ffe05000", "max-frequency")
            hz = struct.unpack(">I", hz)[0] if hz and len(hz) == 4 else None
            over(
                hz is not None and hz <= 25_000_000,
                f"{subor}: SD karta najviac {hz // 1_000_000 if hz else '?'} MHz – bez chýb CRC (R238)",
                f"{subor}: SD karta až {hz} Hz – pri 50 MHz chyby CRC (R238)",
            )
            nevym = uzol_vlastnost(dtb, "mmc@ffe05000", "non-removable") is not None
            cd = uzol_vlastnost(dtb, "mmc@ffe05000", "cd-gpios") is not None
            over(
                nevym and not cd,
                f"{subor}: SD karta nevymeniteľná – jadro ju neskúša každú sekundu (R240)",
                f"{subor}: SD karta {'s cd-gpios' if cd else 'vymeniteľná'} – jadro ju skúša každú sekundu (R240)",
            )
        if bez_wifi:
            stav = uzol_vlastnost(dtb, "mmc@ffe03000", "status")
            over(
                stav == b"disabled\0",
                f"{subor}: Wi-Fi bez napájania (R112)",
                f"{subor}: Wi-Fi nie je vypnutá ({stav!r})",
            )

    # Odroid C4: fdtfile podľa dosky = amlogic/meson-sm1-odroid-c4.dtb
    popis("amlogic/meson-sm1-odroid-c4.dtb", b"hardkernel,odroid-c4", "ODROID-C4", True, False)
    # X96 Max Plus: fdtfile nášho u-boot.ext = amlogic/meson-sm1-x96-air-gbit.dtb (+ verzia 100 Mb/s)
    popis("amlogic/meson-sm1-x96-air-gbit.dtb", b"amediatech,x96-air-gbit", "X96 Max Plus", False, True)
    popis("amlogic/meson-sm1-x96-air.dtb", b"amediatech,x96-air", "X96 Max Plus (Ethernet 100", False, True)

    # X96 Max Plus – reťaz štartu: pôvodný zavádzač → aml_autoscript (raz) → s905_autoscript → u-boot.ext
    for meno, musi, nesmie in (
        ("aml_autoscript", ["saveenv", "storeboot", "s905_autoscript", "fatload mmc 0", "fatload usb", "upgrade_step 2"], []),
        ("s905_autoscript", ["u-boot.ext", "go 0x01000000", "fatload mmc 0", "fatload usb"], ["saveenv", "store ", "mmc write"]),
    ):
        cesta = boot / meno
        if not cesta.is_file():
            over(False, "", f"{meno} na karte nie je")
            continue
        chyba, text = skript_ubootu(cesta.read_bytes())
        chyba = chyba or next((f"chýba „{m}“" for m in musi if m not in text), None)
        chyba = chyba or next((f"obsahuje zakázané „{n}“" for n in nesmie if n in text), None)
        over(chyba is None, f"{meno}: platný skript U-Boot ({len(text)} znakov)", f"{meno}: {chyba}")
    ext = boot / "u-boot.ext"
    if ext.is_file():
        u = ext.read_bytes()
        over(
            b"SKLENIK x96-max-plus" in u and b"amlogic/meson-sm1-x96-air-gbit.dtb" in u,
            f"u-boot.ext ({len(u)} B): náš U-Boot pre X96, hľadá popis meson-sm1-x96-air-gbit.dtb",
            "u-boot.ext nie je náš U-Boot pre X96",
        )
        over(len(u) < 2 << 20, "u-boot.ext má menej ako 2 MB (nahrá sa na 0x01000000, skripty sú na 0x09000000)", "u-boot.ext je príliš veľký")
    else:
        over(False, "", "u-boot.ext na karte nie je")

    if chyby:
        print(f"❌ chýb: {len(chyby)}")
        return 1
    print("✅ karta je pripravená pre Odroid C4 a X96 Max Plus (zavádzače a ovládače dosiek sa overia na skutočných zariadeniach)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
