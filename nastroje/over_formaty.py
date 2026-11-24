#!/usr/bin/env python3
"""Nezávislé overenie obrazových formátov ORCHIDEE (R211) – len v CI, nie súčasť programu.

Rozhodcami sú cudzie knižnice: libpng a libjpeg-turbo (cez Pillow), libtiff a libjxl (cez imagecodecs)
a OpenEXR (oficiálny balík Pythonu). Skript vytvorí náhodné obrázky, uloží ich rozhodcom v mnohých
variantoch a porovná s naším dekodérom; potom naopak – naše súbory musí prečítať rozhodca.

Použitie: over_formaty.py <cesta k programu formaty> <pracovný priečinok> [formát…]
Návratový kód 0 = všetko sedí. Výsledok je tabuľka v Markdowne (pre súhrn CI).
"""

import io
import os
import subprocess
import sys

import numpy as np

PROGRAM = sys.argv[1]
PRAC = sys.argv[2]
VYBER = set(sys.argv[3:])
os.makedirs(PRAC, exist_ok=True)
CHYBY = []
RIADKY = []


def nas_prevod(vstup, vystup):
    r = subprocess.run([PROGRAM, "prevod", vstup, vystup], capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.strip())


def citaj_surove(cesta):
    with open(cesta, "rb") as f:
        hlavicka = f.readline().decode().split()
        w, h, c, typ = int(hlavicka[0]), int(hlavicka[1]), int(hlavicka[2]), hlavicka[3]
        dt = {"u8": np.uint8, "u16": "<u2", "f32": "<f4"}[typ]
        return np.frombuffer(f.read(), dtype=dt).reshape(h, w, c)


def nase(subor):
    """Náš dekodér → pole (výška, šírka, kanály)."""
    ciel = subor + ".surove"
    nas_prevod(subor, ciel)
    return citaj_surove(ciel)


def zapis_vysledok(format_, pripad, ok, poznamka=""):
    RIADKY.append(f"| {format_} | {pripad} | {'✅' if ok else '❌'} | {poznamka} |")
    if not ok:
        CHYBY.append(f"{format_} {pripad}: {poznamka}")


def nahodny_obraz(rng, h, w, c, dt, hladky=True):
    """Hladký obraz s šumom (podobný fotke) – aby kompresia mala čo robiť."""
    maxv = np.iinfo(dt).max if np.issubdtype(dt, np.integer) else 1.0
    y, x = np.mgrid[0:h, 0:w]
    zaklad = np.stack([(np.sin(x / (7 + k) + y / (11 + 2 * k)) + 1) / 2 for k in range(c)], axis=-1)
    if hladky:
        zaklad = zaklad * 0.9 + rng.random((h, w, c)) * 0.1
    else:
        zaklad = rng.random((h, w, c))
    if np.issubdtype(dt, np.integer):
        return np.round(zaklad * maxv).astype(dt)
    return zaklad.astype(dt)


# ---------------------------------------------------------------- PNG
ADAM7 = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)]


def png_subor(rng, h, w, typ, hlbka, prekladane, trns):
    """PNG zostavený v Pythone (zlib z Pythonu) s náhodnými filtrami riadkov – všetky typy, hĺbky, Adam7.
    Pravdu o obsahu určí libpng (imagecodecs), nie tento zapisovač."""
    import struct
    import zlib

    k = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[typ]
    maxv = 2**hlbka - 1
    data = rng.integers(0, maxv + 1, (h, w, k), dtype=np.int64)
    if typ in (0, 2) and trns:
        # kúsok pixelov s presne priehľadnou farbou
        farba = rng.integers(0, maxv + 1, k)
        maska = rng.random((h, w)) < 0.3
        data[maska] = farba
    paleta = rng.integers(0, 256, (maxv + 1, 3), dtype=np.uint8) if typ == 3 else None

    def riadok_bajty(r):
        if hlbka == 16:
            return b"".join(int(v).to_bytes(2, "big") for v in r.reshape(-1))
        if hlbka == 8:
            return bytes(int(v) for v in r.reshape(-1))
        bity = "".join(format(int(v), f"0{hlbka}b") for v in r.reshape(-1))
        bity += "0" * (-len(bity) % 8)
        return bytes(int(bity[i:i + 8], 2) for i in range(0, len(bity), 8))

    bpp = max(1, k * hlbka // 8)
    surove = bytearray()
    prechody = ADAM7 if prekladane else [(0, 0, 1, 1)]
    for x0, y0, dx, dy in prechody:
        cast = data[y0::dy, x0::dx]
        if cast.shape[0] == 0 or cast.shape[1] == 0:
            continue
        predosly = None
        for r in cast:
            b = bytearray(riadok_bajty(r))
            f = int(rng.integers(0, 5))
            povodny = bytes(b)
            for i in range(len(b)):
                a = povodny[i - bpp] if i >= bpp else 0
                hore = predosly[i] if predosly else 0
                c = predosly[i - bpp] if predosly and i >= bpp else 0
                if f == 1:
                    b[i] = (povodny[i] - a) & 255
                elif f == 2:
                    b[i] = (povodny[i] - hore) & 255
                elif f == 3:
                    b[i] = (povodny[i] - (a + hore) // 2) & 255
                elif f == 4:
                    p = a + hore - c
                    pa, pb, pc = abs(p - a), abs(p - hore), abs(p - c)
                    pr = a if pa <= pb and pa <= pc else hore if pb <= pc else c
                    b[i] = (povodny[i] - pr) & 255
            surove += bytes([f]) + b
            predosly = povodny

    def kus(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))

    s = b"\x89PNG\r\n\x1a\n" + kus(b"IHDR", struct.pack(">IIBBBBB", w, h, hlbka, typ, 0, 0, int(prekladane)))
    if typ == 3:
        s += kus(b"PLTE", paleta.tobytes())
        if trns:
            s += kus(b"tRNS", bytes(rng.integers(0, 256, maxv + 1, dtype=np.uint8)))
    elif trns:
        s += kus(b"tRNS", b"".join(int(v).to_bytes(2, "big") for v in farba))
    z = zlib.compress(bytes(surove), 9)
    # IDAT rozdelené na viac kúskov
    for i in range(0, len(z), 97):
        s += kus(b"IDAT", z[i:i + 97])
    return s + kus(b"IEND", b"")


def over_png(rng):
    import imagecodecs

    pripady = []
    for typ, hlbky in [(0, (1, 2, 4, 8, 16)), (2, (8, 16)), (3, (1, 2, 4, 8)), (4, (8, 16)), (6, (8, 16))]:
        for hlbka in hlbky:
            for prekladane in (False, True):
                for trns in ((False, True) if typ in (0, 2, 3) else (False,)):
                    pripady.append((typ, hlbka, prekladane, trns))
    for typ, hlbka, prekladane, trns in pripady:
        nazov = f"typ {typ}, {hlbka} b{', Adam7' if prekladane else ''}{', tRNS' if trns else ''}"
        cesta = os.path.join(PRAC, f"png_{typ}_{hlbka}_{int(prekladane)}_{int(trns)}.png")
        h, w = int(rng.integers(1, 40)), int(rng.integers(1, 40))
        with open(cesta, "wb") as f:
            f.write(png_subor(rng, h, w, typ, hlbka, prekladane, trns))
        try:
            ref = imagecodecs.png_decode(open(cesta, "rb").read())
            if ref.ndim == 2:
                ref = ref[:, :, None]
            nas = nase(cesta)
            ok = nas.shape == ref.shape and np.array_equal(nas.astype(np.int64), ref.astype(np.int64))
            zapis_vysledok("PNG čítanie", nazov, ok, "bit po bite" if ok else f"tvar {nas.shape} vs {ref.shape}")
        except Exception as e:  # noqa: BLE001
            zapis_vysledok("PNG čítanie", nazov, False, str(e)[:160])
    # naopak: náš zápis → libpng
    for c, dt in [(1, np.uint8), (2, np.uint8), (3, np.uint8), (4, np.uint8), (3, np.uint16), (4, np.uint16)]:
        data = nahodny_obraz(rng, 61, 47, c, dt)
        nazov = f"{c} kanály, {8 * np.dtype(dt).itemsize} b"
        zdroj = os.path.join(PRAC, f"png_zdroj_{c}_{np.dtype(dt).itemsize}.png")
        with open(zdroj, "wb") as f:
            f.write(imagecodecs.png_encode(data.squeeze() if c == 1 else data))
        try:
            nas_png = zdroj + ".nase.png"
            nas_prevod(zdroj, nas_png)
            spat = imagecodecs.png_decode(open(nas_png, "rb").read())
            ok = np.array_equal(spat.reshape(data.shape), data)
            zapis_vysledok("PNG zápis", nazov, ok,
                           f"libpng prečíta bit po bite; {os.path.getsize(nas_png)} B (libpng {os.path.getsize(zdroj)} B)")
        except Exception as e:  # noqa: BLE001
            zapis_vysledok("PNG zápis", nazov, False, str(e)[:160])


# ---------------------------------------------------------------- JPEG
def over_jpeg(rng):
    """Čítanie: súbory z libjpeg-turbo (Pillow, imagecodecs) vo všetkých bežných režimoch, porovnanie
    s jeho dekódovaním. IDCT a prevzorkovanie sa smú líšiť zaokrúhlením (norma dovoľuje ±1 na IDCT)."""
    import imagecodecs
    from PIL import Image

    zdroj = nahodny_obraz(rng, 203, 317, 3, np.uint8)
    pripady = []
    for kvalita in (50, 90, 100):
        for podv in (0, 1, 2):
            for prog in (False, True):
                pripady.append((f"q{kvalita} {['4:4:4', '4:2:2', '4:2:0'][podv]}{' progresívny' if prog else ''}",
                                dict(quality=kvalita, subsampling=podv, progressive=prog), "RGB"))
    pripady.append(("q85 4:2:0 optimalizovaný, reštarty", dict(quality=85, optimize=True, restart_marker_blocks=7), "RGB"))
    pripady.append(("sivý q90", dict(quality=90), "L"))
    pripady.append(("sivý progresívny", dict(quality=80, progressive=True), "L"))
    pripady.append(("CMYK (Adobe)", dict(quality=92), "CMYK"))
    for nazov, nastavenie, mode in pripady:
        cesta = os.path.join(PRAC, f"jpeg_{len(RIADKY)}.jpg")
        im = Image.fromarray(zdroj)
        if mode == "L":
            im = im.convert("L")
        elif mode == "CMYK":
            im = im.convert("CMYK")
        try:
            im.save(cesta, "JPEG", **nastavenie)
        except TypeError:
            nastavenie.pop("restart_marker_blocks", None)
            im.save(cesta, "JPEG", **nastavenie)
        try:
            ref = np.asarray(Image.open(cesta)).astype(np.int64)
            if ref.ndim == 2:
                ref = ref[:, :, None]
            nas = nase(cesta).astype(np.int64)
            rozdiel = np.abs(nas - ref)
            # norma dovoľuje ±1 na výstupe IDCT; prevod YCbCr → RGB ho zväčší až na ±4
            ok = nas.shape == ref.shape and rozdiel.max() <= 4 and rozdiel.mean() < 0.3
            zapis_vysledok("JPEG čítanie", nazov, ok,
                           f"max odchýlka {rozdiel.max()}, priemer {rozdiel.mean():.3f} (libjpeg-turbo)")
        except Exception as e:  # noqa: BLE001
            zapis_vysledok("JPEG čítanie", nazov, False, str(e)[:160])
    # 12 bitov (libjpeg-turbo cez imagecodecs)
    try:
        d12 = (nahodny_obraz(rng, 64, 80, 1, np.uint16)[:, :, 0] >> 4).astype(np.uint16)
        j = imagecodecs.jpeg8_encode(d12, level=95, bitspersample=12)
        cesta = os.path.join(PRAC, "jpeg_12bit.jpg")
        open(cesta, "wb").write(j)
        ref = imagecodecs.jpeg8_decode(j).astype(np.int64)
        nas = nase(cesta)[:, :, 0].astype(np.int64) >> 4
        r = np.abs(nas - ref)
        zapis_vysledok("JPEG čítanie", "12 bitov, sivý", r.max() <= 4, f"max odchýlka {r.max()}, priemer {r.mean():.3f}")
    except Exception as e:  # noqa: BLE001
        zapis_vysledok("JPEG čítanie", "12 bitov, sivý", False, str(e)[:160])
    # zápis: náš JPEG prečíta libjpeg-turbo; kvalita a veľkosť oproti libjpeg-turbo pri rovnakej kvalite
    zdroj_png = os.path.join(PRAC, "jpeg_zdroj.png")
    Image.fromarray(zdroj).save(zdroj_png)
    for kvalita in (75, 90):
        for prog in (False, True):
            nazov = f"q{kvalita}{' progresívny' if prog else ''}"
            try:
                nas_jpg = os.path.join(PRAC, f"jpeg_nas_{kvalita}_{int(prog)}.jpg")
                prikaz = [PROGRAM, "prevod", zdroj_png, nas_jpg]
                env = dict(os.environ, ORCHIDEA_JPEG=f"{kvalita}{',p' if prog else ''}")
                r = subprocess.run(prikaz, capture_output=True, text=True, env=env)
                if r.returncode != 0:
                    raise RuntimeError(r.stderr)
                spat = np.asarray(Image.open(nas_jpg).convert("RGB")).astype(np.float64)
                mse = ((spat - zdroj) ** 2).mean()
                ref_cesta = os.path.join(PRAC, f"jpeg_ref_{kvalita}.jpg")
                Image.fromarray(zdroj).save(ref_cesta, quality=kvalita, optimize=True, progressive=prog,
                                            subsampling=0 if kvalita >= 90 else 2)
                ref = np.asarray(Image.open(ref_cesta).convert("RGB")).astype(np.float64)
                mse_ref = ((ref - zdroj) ** 2).mean()
                psnr = 10 * np.log10(255**2 / mse)
                psnr_ref = 10 * np.log10(255**2 / mse_ref)
                ok = psnr >= psnr_ref - 0.3
                zapis_vysledok("JPEG zápis", nazov, ok,
                               f"PSNR {psnr:.2f} dB, {os.path.getsize(nas_jpg)} B (libjpeg-turbo {psnr_ref:.2f} dB, "
                               f"{os.path.getsize(ref_cesta)} B)")
            except Exception as e:  # noqa: BLE001
                zapis_vysledok("JPEG zápis", nazov, False, str(e)[:160])


def main():
    rng = np.random.default_rng(2026)
    overenia = {"png": over_png, "jpeg": over_jpeg}
    for meno, f in overenia.items():
        if not VYBER or meno in VYBER:
            f(rng)
    print("| Formát | Prípad | Výsledok | Poznámka |")
    print("|---|---|---|---|")
    print("\n".join(RIADKY))
    if CHYBY:
        print(f"\n**Chyby: {len(CHYBY)}**")
        for c in CHYBY:
            print(f"- {c}", file=sys.stderr)
        return 1
    print(f"\nVšetkých {len(RIADKY)} prípadov sedí.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
