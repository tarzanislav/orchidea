#!/usr/bin/env python3
"""Kontrola samomerania SKLENÍKA (R122) po skúške v emulátore QEMU – bez uchovania obrázka.

Priečinok MERANIA skopírovaný z karty emulátora: overí textovú správu (hlavička, poradové číslo, nosič,
oddiely, žiadne nedoplnené značky), súbor SPOLU.TXT a snímku PNG – podpis, CRC kusov, rozbalenie
(zlib zo štandardnej knižnice, nezávislé od kódera KLÍČA), filtre riadkov, rozmery a farby obrazovky.
Obrázok sa nikde neukladá; priečinok zmaže krok v CI.

Použitie: python3 nastroje/kontrola_merania.py <priečinok MERANIA> <šírka> <výška>
Len štandardná knižnica Pythonu.
"""

import struct
import sys
import zlib
from pathlib import Path

POZADIE = (0x0F, 0x1C, 0x15)
ZELENA = (0x6F, 0xD0, 0x8C)


def blizko(a, b, tol=24):
    return all(abs(x - y) <= tol for x, y in zip(a, b))


def rozbal_png(data: bytes):
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("chýba podpis PNG")
    i, idat, w, h = 8, b"", 0, 0
    while True:
        n = struct.unpack(">I", data[i:i + 4])[0]
        typ, obsah = data[i + 4:i + 8], data[i + 8:i + 8 + n]
        crc = struct.unpack(">I", data[i + 8 + n:i + 12 + n])[0]
        if zlib.crc32(typ + obsah) & 0xFFFFFFFF != crc:
            raise ValueError(f"zlý CRC kusu {typ!r}")
        if typ == b"IHDR":
            w, h = struct.unpack(">II", obsah[:8])
            if obsah[8:] != bytes([8, 2, 0, 0, 0]):
                raise ValueError("čakal sa obrázok RGB s 8 bitmi")
        elif typ == b"IDAT":
            idat += obsah
        elif typ == b"IEND":
            break
        i += 12 + n
    surove = zlib.decompress(idat)
    r = 3 * w
    if len(surove) != (r + 1) * h:
        raise ValueError("nesprávna dĺžka údajov")
    body, predosly = bytearray(), bytearray(r)
    for y in range(h):
        f = surove[y * (r + 1)]
        riadok = bytearray(surove[y * (r + 1) + 1:(y + 1) * (r + 1)])
        for x in range(r):
            if f == 1:
                riadok[x] = (riadok[x] + (riadok[x - 3] if x >= 3 else 0)) & 0xFF
            elif f == 2:
                riadok[x] = (riadok[x] + predosly[x]) & 0xFF
            elif f != 0:
                raise ValueError(f"nečakaný filter {f}")
        body += riadok
        predosly = riadok
    return w, h, body


def main() -> int:
    if len(sys.argv) != 4:
        print(__doc__)
        return 1
    priecinok, w_ocak, h_ocak = Path(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
    chyby = []
    texty = sorted(priecinok.glob("01-*.txt"))
    obrazky = sorted(priecinok.glob("01-*-00min.png"))
    spolu = priecinok / "SPOLU.TXT"
    if len(texty) != 1 or len(obrazky) != 1 or not spolu.exists():
        print(f"- ❌ čakal sa 01-….txt, 01-…-00min.png a SPOLU.TXT, sú tam: {sorted(p.name for p in priecinok.iterdir())}")
        return 1
    text = texty[0].read_text(encoding="utf-8")
    print(f"- súbory: {texty[0].name} ({len(text.encode())} B), {obrazky[0].name} ({obrazky[0].stat().st_size} B), SPOLU.TXT")
    for musi in [
        "SKLENÍK – samomeranie č. 01 · záznam 0. minúty",
        "zapísané na: SD karta (mmcblk0p1)",
        "Raspberry Pi 3",
        "── OBRAZOVKA",
        "Teplota",
        "Meranie",
        "── STRÁŽCA TEPLOTY",
        "rozpoznanie: ✓ Raspberry Pi",
        "── SNÍMAČE TEPLOTY A SYSTÉM",
        "── IDENTIFIKÁCIA HARDVÉRU",
        "mmcblk0:",
        "── VAROVANIA A CHYBY JADRA OD ŠTARTU",
        "-00min.png",
        "záznam trval:",
    ]:
        if musi not in text:
            chyby.append(f"v správe chýba „{musi}“")
    if "{PORADIE}" in text or "{NOSIC}" in text:
        chyby.append("v správe zostala nedoplnená značka")
    if "\r\n" not in text:
        chyby.append("správa nemá konce riadkov pre Windows")
    if spolu.read_text(encoding="utf-8") != text:
        chyby.append("SPOLU.TXT sa líši od správy jediného zariadenia")
    trval = [r.strip() for r in text.splitlines() if "záznam trval" in r]
    if trval:
        print(f"- {trval[0]}")
    try:
        w, h, body = rozbal_png(obrazky[0].read_bytes())
        n = w * h
        pozadie = sum(1 for i in range(0, 3 * n, 3) if blizko(body[i:i + 3], POZADIE))
        zelene = sum(1 for i in range(0, 3 * n, 3) if blizko(body[i:i + 3], ZELENA, 40))
        print(f"- snímka PNG {w}×{h}: pozadie {100 * pozadie / n:.1f} %, zelená {100 * zelene / n:.2f} % (rozbalená nezávisle cez zlib)")
        if (w, h) != (w_ocak, h_ocak):
            chyby.append(f"snímka má {w}×{h}, obrazovka {w_ocak}×{h_ocak}")
        if not 0.5 < pozadie / n < 0.99:
            chyby.append("snímka nevyzerá ako stavová obrazovka (podiel pozadia)")
        if zelene == 0:
            chyby.append("na snímke chýba zelený nadpis")
    except (ValueError, zlib.error, struct.error) as e:
        chyby.append(f"snímka PNG je chybná: {e}")
    for c in chyby:
        print(f"- ❌ {c}")
    if not chyby:
        print("- ✅ samomeranie: správa, SPOLU.TXT a snímka sú v poriadku")
    return 1 if chyby else 0


if __name__ == "__main__":
    sys.exit(main())
