#!/usr/bin/env python3
"""Nezávislé overenie obrazových formátov ORCHIDEE (R211) – len v CI, nie súčasť programu.

Rozhodcami sú cudzie knižnice: libpng, libjpeg-turbo, libtiff, libjxl a OpenEXR (cez imagecodecs), Pillow,
ExifTool (metadáta);
skúšobné súbory TIFF zapisuje tifffile. Skript vytvorí náhodné obrázky, uloží ich rozhodcom v mnohých
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


# ---------------------------------------------------------------- TIFF
def over_tiff(rng):
    """Čítanie: súbory z tifffile (kompresia cez imagecodecs/zlib) vo všetkých bežných variantoch; pravdu
    určí libtiff (imagecodecs.tiff_decode). Zápis: náš TIFF musí prečítať libtiff bit po bite."""
    import imagecodecs
    import tifffile

    pripady = []
    for dt, c in [(np.uint8, 1), (np.uint8, 3), (np.uint8, 4), (np.uint16, 1), (np.uint16, 3), (np.float32, 3),
                  (np.float16, 1), (np.float64, 1), (np.int16, 1)]:
        for komp in (None, "lzw", "zlib", "packbits"):
            if komp == "packbits" and np.dtype(dt).itemsize > 2:
                continue
            for pred in ((None, "horizontal") if np.issubdtype(dt, np.integer) else (None, "floatingpoint")):
                if komp is None and pred:
                    continue
                pripady.append((dt, c, komp, pred, False, False, "<"))
    # dlaždice, roviny, big-endian, BigTIFF
    pripady += [(np.uint8, 3, "zlib", None, True, False, "<"), (np.uint16, 3, "lzw", "horizontal", True, True, ">"),
                (np.uint8, 3, None, None, False, True, ">"), (np.float32, 1, "zlib", "floatingpoint", True, False, ">"),
                (np.uint16, 4, "zlib", "horizontal", False, False, "<")]
    for i, (dt, c, komp, pred, tiled, planar, endian) in enumerate(pripady):
        nazov = (f"{np.dtype(dt).name} ×{c} {komp or 'bez kompresie'}{' +' + pred if pred else ''}"
                 f"{' dlaždice' if tiled else ''}{' roviny' if planar else ''}{' BE' if endian == '>' else ''}")
        cesta = os.path.join(PRAC, f"tiff_{i}.tif")
        h, w = int(rng.integers(5, 90)), int(rng.integers(5, 90))
        if np.issubdtype(dt, np.floating):
            data = (rng.random((h, w, c)) * 4 - 1).astype(dt)
        else:
            data = rng.integers(np.iinfo(dt).min, np.iinfo(dt).max, (h, w, c), dtype=np.int64, endpoint=True).astype(dt)
        try:
            kwargs = dict(compression=komp, predictor=pred, byteorder=endian, bigtiff=(i % 4 == 3))
            if tiled:
                kwargs["tile"] = (16, 32)
            if planar and c > 1:
                kwargs["planarconfig"] = "separate"
                arr = np.moveaxis(data, 2, 0)
            else:
                arr = data.squeeze(2) if c == 1 else data
            if c == 4:
                kwargs["photometric"] = "rgb"
                kwargs["extrasamples"] = ["unassalpha"]
            elif c == 3:
                kwargs["photometric"] = "rgb"
            else:
                kwargs["photometric"] = "minisblack"
            tifffile.imwrite(cesta, arr, **kwargs)
            ref = imagecodecs.tiff_decode(open(cesta, "rb").read())
            if planar and c > 1 and ref.shape[0] == c:
                ref = np.moveaxis(ref, 0, 2)
            ref = ref.reshape(h, w, c)
            nas = nase(cesta)
            if np.issubdtype(dt, np.floating):
                ok = np.array_equal(nas.astype(np.float32), ref.astype(np.float32), equal_nan=True)
            elif dt == np.int16:
                ok = np.allclose(nas, ref.astype(np.float32) / 32767, atol=1e-6)
            else:
                ok = np.array_equal(nas.astype(np.int64), ref.astype(np.int64))
            zapis_vysledok("TIFF čítanie", nazov, ok, "zhoda s libtiff" if ok else f"tvar {nas.shape} vs {ref.shape}")
        except Exception as e:  # noqa: BLE001
            zapis_vysledok("TIFF čítanie", nazov, False, str(e)[:160])
    # paleta a CMYK (zvláštne fotometrie)
    try:
        idx = rng.integers(0, 256, (33, 47), dtype=np.uint8)
        mapa = rng.integers(0, 65536, (3, 256), dtype=np.uint16)
        cesta = os.path.join(PRAC, "tiff_paleta.tif")
        tifffile.imwrite(cesta, idx, photometric="palette", colormap=mapa, compression="lzw")
        nas = nase(cesta)
        ref = (mapa[:, idx] >> 8).transpose(1, 2, 0)
        zapis_vysledok("TIFF čítanie", "paleta 8 b", np.array_equal(nas, ref), "farby z ColorMap")
        cmyk = rng.integers(0, 256, (21, 30, 4), dtype=np.uint8)
        cesta = os.path.join(PRAC, "tiff_cmyk.tif")
        tifffile.imwrite(cesta, cmyk, photometric="separated", compression="zlib")
        zapis_vysledok("TIFF čítanie", "CMYK 8 b", np.array_equal(nase(cesta), cmyk), "")
        bity = rng.random((19, 37)) < 0.5
        cesta = os.path.join(PRAC, "tiff_1bit.tif")
        tifffile.imwrite(cesta, bity, compression="packbits")
        # tifffile píše „biela je nula“ (MinIsWhite): bit 1 = čierna
        zapis_vysledok("TIFF čítanie", "1 bit, biela je nula", np.array_equal(nase(cesta)[:, :, 0], np.where(bity, 0, 255)),
                       "fotometria 0 obrátená na jas")
        foto = nahodny_obraz(rng, 70, 90, 3, np.uint8)
        cesta = os.path.join(PRAC, "tiff_jpeg.tif")
        tifffile.imwrite(cesta, foto, photometric="rgb", compression="jpeg", compressionargs={"level": 90}, tile=(32, 48))
        ref = imagecodecs.tiff_decode(open(cesta, "rb").read()).astype(np.int64)
        r = np.abs(nase(cesta).astype(np.int64) - ref)
        zapis_vysledok("TIFF čítanie", "JPEG v dlaždiciach (YCbCr)", r.max() <= 4 and r.mean() < 0.3,
                       f"max odchýlka {r.max()}, priemer {r.mean():.3f} (libtiff + libjpeg)")
    except Exception as e:  # noqa: BLE001
        zapis_vysledok("TIFF čítanie", "paleta / CMYK / 1 bit / JPEG", False, str(e)[:160])
    # zápis
    for dt, c in [(np.uint8, 3), (np.uint16, 4), (np.float32, 1), (np.uint8, 1)]:
        nazov = f"{np.dtype(dt).name} ×{c}"
        try:
            data = (rng.random((40, 57, c)) * (1 if dt == np.float32 else np.iinfo(dt).max)).astype(dt)
            zdroj = os.path.join(PRAC, f"tiff_zdroj_{nazov.replace(' ', '')}.tif")
            tifffile.imwrite(zdroj, data.squeeze(2) if c == 1 else data,
                             photometric="minisblack" if c == 1 else "rgb",
                             extrasamples=["unassalpha"] if c == 4 else None)
            nas_tif = zdroj + ".nase.tif"
            nas_prevod(zdroj, nas_tif)
            spat = imagecodecs.tiff_decode(open(nas_tif, "rb").read()).reshape(data.shape)
            ok = np.array_equal(spat, data)
            zapis_vysledok("TIFF zápis", nazov, ok, f"libtiff prečíta bit po bite; {os.path.getsize(nas_tif)} B")
        except Exception as e:  # noqa: BLE001
            zapis_vysledok("TIFF zápis", nazov, False, str(e)[:160])


# ---------------------------------------------------------------- OpenEXR
def over_exr(rng):
    """Čítanie: súbory z knižnice OpenEXR (oficiálny balík Pythonu) so všetkými kompresiami, ktoré vieme,
    riadky aj dlaždice; porovnanie bit po bite s jej čítaním. Zápis: náš súbor prečíta OpenEXR."""
    import OpenEXR

    kompresie = [("NONE", OpenEXR.NO_COMPRESSION), ("RLE", OpenEXR.RLE_COMPRESSION), ("ZIPS", OpenEXR.ZIPS_COMPRESSION),
                 ("ZIP", OpenEXR.ZIP_COMPRESSION), ("PIZ", OpenEXR.PIZ_COMPRESSION), ("PXR24", OpenEXR.PXR24_COMPRESSION),
                 ("B44", OpenEXR.B44_COMPRESSION), ("B44A", OpenEXR.B44A_COMPRESSION)]
    for meno, k in kompresie:
        for typ in ("half", "float", "zmes", "uint"):
            for dlazdice in (False, True):
                h, w = int(rng.integers(3, 80)), int(rng.integers(3, 80))
                obr = (np.exp(rng.normal(0, 2, (h, w, 4))) * (rng.random((h, w, 4)) < 0.95)).astype(np.float32)
                if meno in ("B44", "B44A"):
                    obr[: h // 2, : w // 2] = 0.25  # ploché bloky (B44A ich ukladá 3 bajtmi)
                ch = {}
                for i, c in enumerate("RGBA"):
                    kanal = np.ascontiguousarray(obr[:, :, i])
                    if typ == "half" or (typ == "zmes" and c != "B"):
                        ch[c] = kanal.astype(np.float16)
                    elif typ == "uint":
                        ch[c] = (kanal * 1000).astype(np.uint32)
                    else:
                        ch[c] = kanal
                hdr = {"compression": k, "type": OpenEXR.tiledimage if dlazdice else OpenEXR.scanlineimage}
                if dlazdice:
                    td = OpenEXR.TileDescription()
                    td.xSize, td.ySize = 16, 8
                    hdr["tiles"] = td
                nazov = f"{meno} {typ}{' dlaždice' if dlazdice else ''}"
                cesta = os.path.join(PRAC, f"exr_{meno}_{typ}_{int(dlazdice)}.exr")
                try:
                    with OpenEXR.File(hdr, ch) as f:
                        f.write(cesta)
                    ref_ch = OpenEXR.File(cesta, separate_channels=True).channels()
                    ref = np.stack([ref_ch[c].pixels.astype(np.float32) for c in "RGBA"], axis=-1)
                    nas = nase(cesta)
                    ok = nas.shape == ref.shape and np.array_equal(nas, ref, equal_nan=True)
                    pozn = "bit po bite" if ok else (f"max rozdiel {np.nanmax(np.abs(nas - ref)):.4g}" if nas.shape == ref.shape else f"tvar {nas.shape}")
                    zapis_vysledok("OpenEXR čítanie", nazov, ok, pozn)
                except Exception as e:  # noqa: BLE001
                    zapis_vysledok("OpenEXR čítanie", nazov, False, str(e)[:160])
    # podvzorkovaný kanál a posunuté dataWindow
    try:
        h, w = 24, 32
        y = rng.random((h, w)).astype(np.float16)
        hdr = {"compression": OpenEXR.ZIP_COMPRESSION, "type": OpenEXR.scanlineimage,
               "dataWindow": (np.array([-7, 5], dtype=np.int32), np.array([w - 8, h + 4], dtype=np.int32)),
               "displayWindow": (np.array([0, 0], dtype=np.int32), np.array([63, 63], dtype=np.int32))}
        cesta = os.path.join(PRAC, "exr_okno.exr")
        with OpenEXR.File(hdr, {"Y": y}) as f:
            f.write(cesta)
        nas = nase(cesta)[:, :, 0]
        zapis_vysledok("OpenEXR čítanie", "sivý Y, posunuté dataWindow", np.array_equal(nas, y.astype(np.float32)), "")
    except Exception as e:  # noqa: BLE001
        zapis_vysledok("OpenEXR čítanie", "sivý Y, posunuté dataWindow", False, str(e)[:160])
    # zápis
    for meno in ("zip", "zips", "rle", "none"):
        for polovicna in (True, False):
            nazov = f"{meno.upper()} {'half' if polovicna else 'float'}"
            try:
                obr = np.exp(rng.normal(0, 1.5, (33, 45, 4))).astype(np.float32)
                if polovicna:
                    obr = obr.astype(np.float16).astype(np.float32)
                zdroj = os.path.join(PRAC, f"exr_zdroj_{meno}_{int(polovicna)}.exr")
                with OpenEXR.File({"compression": OpenEXR.NO_COMPRESSION, "type": OpenEXR.scanlineimage},
                                  {c: np.ascontiguousarray(obr[:, :, i]) for i, c in enumerate("RGBA")}) as f:
                    f.write(zdroj)
                nas_exr = zdroj + ".nase.exr"
                env = dict(os.environ, ORCHIDEA_EXR=f"{meno},{'half' if polovicna else 'float'}")
                r = subprocess.run([PROGRAM, "prevod", zdroj, nas_exr], capture_output=True, text=True, env=env)
                if r.returncode != 0:
                    raise RuntimeError(r.stderr)
                ref_ch = OpenEXR.File(nas_exr, separate_channels=True).channels()
                spat = np.stack([ref_ch[c].pixels.astype(np.float32) for c in "RGBA"], axis=-1)
                ok = np.array_equal(spat, obr)
                zapis_vysledok("OpenEXR zápis", nazov, ok, f"OpenEXR prečíta bit po bite; {os.path.getsize(nas_exr)} B")
            except Exception as e:  # noqa: BLE001
                zapis_vysledok("OpenEXR zápis", nazov, False, str(e)[:160])


# ---------------------------------------------------------------- JPEG XL
def over_jxl(rng):
    """Čítanie súborov z libjxl: bezstratové (Modular) bit po bite, stratové (VarDCT) v tolerancii
    konformity normy. Zápis JPEG XL zatiaľ nie je."""
    import imagecodecs

    y, x = np.mgrid[0:203, 0:317]
    hladky = ((np.sin(x / 9) + np.cos(y / 7)) * 60 + 128).astype(np.uint8)
    rgb = np.stack([hladky, hladky[::-1], (hladky // 2 + 40).astype(np.uint8)], -1)
    pripady = [("sivý 8 b", hladky, {}), ("RGB 8 b", rgb, {}),
               ("RGBA 8 b", np.dstack([rgb, (hladky > 100).astype(np.uint8) * 255]), {}),
               ("RGB 16 b", rng.integers(0, 65536, (60, 80, 3)).astype(np.uint16), {}),
               ("RGB šum", rng.integers(0, 256, (90, 110, 3), dtype=np.uint8), {}),
               ("paleta", np.array([[255, 0, 0], [0, 255, 0], [0, 0, 255], [9, 9, 9]], np.uint8)[rng.integers(0, 4, (120, 150))], {}),
               ("float32", rng.random((40, 50, 3)).astype(np.float32), {}),
               ("veľký 700×600 (skupiny)", np.stack([hladky[:1, :1].repeat(600, 0).repeat(700, 1)] * 3, -1) + rng.integers(0, 4, (600, 700, 3)).astype(np.uint8), {})]
    for e in (1, 2, 3, 5, 7, 9):
        pripady.append((f"RGB úsilie {e}", rgb, {"effort": e}))
    for nazov, data, kw in pripady:
        cesta = os.path.join(PRAC, f"jxl_{len(RIADKY)}.jxl")
        try:
            d = imagecodecs.jpegxl_encode(data, lossless=True, **kw)
            open(cesta, "wb").write(d)
            ref = imagecodecs.jpegxl_decode(d)
            ref = ref.reshape(data.shape if data.ndim == 3 else data.shape + (1,))
            nas = nase(cesta)
            ok = nas.shape == ref.shape and np.array_equal(nas, ref)
            zapis_vysledok("JPEG XL bezstratovo", nazov, ok, f"bit po bite s libjxl ({len(d)} B)" if ok else f"tvar {nas.shape}")
        except Exception as e:  # noqa: BLE001
            zapis_vysledok("JPEG XL bezstratovo", nazov, False, str(e)[:160])

    # stratové (VarDCT): odchýlka od libjxl najviac 1/255 rozsahu (zaokrúhlenie), priemer pod 0,35/255
    def obraz(h, w):
        yy, xx = np.mgrid[0:h, 0:w]
        a = np.stack([(np.sin(xx / 9) + np.cos(yy / 7)) * 50 + 128, np.sin((xx + yy) / 15) * 80 + 120,
                      ((xx // 16 + yy // 16) % 2) * 120 + 60], -1) + rng.normal(0, 6, (h, w, 3))
        a[h // 3:h // 2, w // 4:w // 2] = [250, 30, 40]
        return np.clip(a, 0, 255).astype(np.uint8)
    stredny = obraz(203, 317)
    stratove = [("vzdialenosť 1", stredny, {"distance": 1.0}), ("vzdialenosť 0,5", stredny, {"distance": 0.5}),
                ("vzdialenosť 2 (EPF 2 kroky)", stredny, {"distance": 2.0}),
                ("vzdialenosť 5 (EPF 3 kroky)", stredny, {"distance": 5.0}),
                ("vzdialenosť 12 (zväčšenie 2×)", stredny, {"distance": 12.0}),
                ("vzdialenosť 25", obraz(301, 433), {"distance": 25.0}),
                ("sivý", stredny[..., 1].copy(), {"distance": 1.0}),
                ("RGBA", np.dstack([stredny, (stredny[..., 0] > 100).astype(np.uint8) * 255]), {"distance": 1.5}),
                ("16 b", stredny.astype(np.uint16) * 257, {"distance": 1.0}),
                ("nepárne rozmery 37×53", obraz(37, 53), {"distance": 1.5}),
                ("1×1", obraz(1, 1), {"distance": 1.0}),
                ("viac skupín 600×700", obraz(600, 700), {"distance": 1.0}),
                ("viac LF skupín 2100×260", obraz(260, 2100), {"distance": 2.0})]
    for e in (3, 7, 9):
        stratove.append((f"úsilie {e}", stredny, {"distance": 1.0, "effort": e}))
    for nazov, data, kw in stratove:
        cesta = os.path.join(PRAC, f"jxl_{len(RIADKY)}.jxl")
        try:
            d = imagecodecs.jpegxl_encode(data, **kw)
            open(cesta, "wb").write(d)
            ref = imagecodecs.jpegxl_decode(d)
            ref = ref.reshape(data.shape if data.ndim == 3 else data.shape + (1,))
            nas = nase(cesta)
            mx = 65535.0 if data.dtype == np.uint16 else 255.0
            dd = np.abs(nas.astype(np.float64) - ref.astype(np.float64)) / mx * 255
            ok = nas.shape == ref.shape and dd.max() <= 1.0 and (dd.size < 3000 or dd.mean() < 0.35)
            zapis_vysledok("JPEG XL stratovo", nazov, ok,
                           f"max {dd.max():.2f}/255, priemer {dd.mean():.3f}/255 ({len(d)} B)")
        except Exception as e:  # noqa: BLE001
            zapis_vysledok("JPEG XL stratovo", nazov, False, str(e)[:160])

    # prekódovaný JPEG (bezstratové prekódovanie JPEG → JPEG XL, VarDCT v YCbCr s podvzorkovaním farby)
    import io
    from PIL import Image
    for (h, w) in [(200, 300), (37, 53), (257, 129), (9, 17), (600, 2100)]:
        for ss, meno_ss in [(0, "4:4:4"), (1, "4:2:2"), (2, "4:2:0")]:
            for sivy in (False, True):
                nazov = f"{w}×{h} {meno_ss}{' sivý' if sivy else ''}"
                cesta = os.path.join(PRAC, f"jxl_{len(RIADKY)}.jxl")
                try:
                    o = obraz(h, w)
                    b = io.BytesIO()
                    Image.fromarray(o[..., 1] if sivy else o).save(b, "JPEG", quality=85, subsampling=ss)
                    d = imagecodecs.jpegxl_encode_jpeg(b.getvalue())
                    open(cesta, "wb").write(d)
                    ref = imagecodecs.jpegxl_decode(d)
                    nas = nase(cesta)
                    ref = ref.reshape(nas.shape)
                    dd = np.abs(nas.astype(np.float64) - ref.astype(np.float64))
                    zapis_vysledok("JPEG XL z JPEG", nazov, dd.max() <= 1.0, f"max {dd.max():.0f}, priemer {dd.mean():.3f} ({len(d)} B)")
                except Exception as e:  # noqa: BLE001
                    zapis_vysledok("JPEG XL z JPEG", nazov, False, str(e)[:160])

    # súbory konformity libjxl (ak je k dispozícii ich priečinok): podporované prípady voči libjxl
    konf = os.environ.get("ORCHIDEA_JXL_KONFORMITA")
    if konf:
        import json
        for meno in ["alpha_nonpremultiplied", "alpha_triangles", "animation_icos4d", "animation_newtons_cradle",
                     "bench_oriented_brg", "bicycles", "bike", "blendmodes", "cafe", "cmyk_layers", "delta_palette",
                     "grayscale_jpeg", "grayscale_public_university",
                     "lossless_pfm", "lz77_flower", "mul_no_extra_channels", "opsin_inverse", "patches_lossless",
                     "spot", "sunset_logo", "upsampling"]:
            cesta = os.path.join(konf, "testcases", meno, "input.jxl")
            try:
                d = open(cesta, "rb").read()
                # natočenie sa neaplikuje – náš obraz ho nesie v metadátach
                ref = imagecodecs.jpegxl_decode(d, keeporientation=True)
                if ref.ndim == 4:
                    ref = ref[0]
                nas = nase(cesta)
                ref = ref.reshape(nas.shape)
                bps = json.load(open(os.path.join(konf, "testcases", meno, "test.json")))["bits_per_sample"][0]
                ref = ref.astype(np.float64)
                if nas.dtype == np.uint16 and bps < 16:
                    ref = np.round(ref * 65535 / ((1 << bps) - 1))
                mx = {np.dtype("uint8"): 255.0, np.dtype("uint16"): 65535.0}.get(nas.dtype, 1.0)
                dd = np.abs(nas.astype(np.float64) - ref) / mx * 255
                ok = dd.max() <= 1.0
                zapis_vysledok("JPEG XL konformita", meno, ok, f"max {dd.max():.2f}/255 voči libjxl")
            except Exception as e:  # noqa: BLE001
                zapis_vysledok("JPEG XL konformita", meno, False, str(e)[:160])
        # XYB s vloženým profilom ICC: obraz sa prevedie do profilu (balík farba); rozhodcom je referenčný obraz
        # sady (ref.png): v jeho profile priamo, v sRGB po prevode nášho výsledku Little CMS (8 b dvakrát → ±3)
        import io
        from PIL import Image, ImageCms
        for meno in ["grayscale", "patches", "progressive"]:
            try:
                vystup = os.path.join(PRAC, f"konf_{meno}.png")
                nas_prevod(os.path.join(konf, "testcases", meno, "input.jxl"), vystup)
                o, ref = Image.open(vystup), Image.open(os.path.join(konf, "testcases", meno, "ref.png"))
                if o.mode == "RGBA":
                    o = o.convert("RGB")
                if ref.info.get("icc_profile"):
                    a, tol, ako = np.asarray(o), 1.0, "v profile ICC"
                else:
                    pr = ImageCms.ImageCmsProfile(io.BytesIO(o.info["icc_profile"]))
                    t = ImageCms.buildTransform(pr, ImageCms.createProfile("sRGB"), o.mode, "RGB")
                    a, tol, ako = np.asarray(ImageCms.applyTransform(o, t)), 3.0, "po prevode do sRGB (Little CMS)"
                r = np.asarray(ref.convert("RGB" if np.asarray(a).ndim == 3 else ref.mode))
                d = np.abs(a.astype(float).reshape(r.shape) - r.astype(float))
                zapis_vysledok("JPEG XL konformita", f"{meno} (XYB → ICC)", d.max() <= tol,
                               f"max {d.max():.0f}/255, priemer {d.mean():.3f} voči ref.png {ako}")
            except Exception as e:  # noqa: BLE001
                zapis_vysledok("JPEG XL konformita", f"{meno} (XYB → ICC)", False, str(e)[:160])


def over_metadata(rng):
    """Exif, IPTC a XMP: zápis rozhodcom ExifTool do JPEG, TIFF a PNG, náš prevod do všetkých troch formátov
    a späť; ExifTool musí vo výsledku nájsť tie isté značky s tými istými hodnotami. Navyše naše čítanie
    vybraných položiek proti hodnotám ExifToolu."""
    import json
    import shutil

    et = os.environ.get("ORCHIDEA_EXIFTOOL", "").split() or ([shutil.which("exiftool")] if shutil.which("exiftool") else [])
    if not et:
        zapis_vysledok("Metadáta", "ExifTool", False, "rozhodca ExifTool chýba")
        return
    from PIL import Image

    skupiny = ("IFD0", "ExifIFD", "GPS", "InteropIFD", "XMP-dc", "XMP-xmp", "IPTC")
    # štruktúrne značky TIFF (rozlíšenie, poloha YCbCr) patria k uloženiu obrazu, nie k popisu
    popisne = {"Make", "Model", "Artist", "Copyright", "ImageDescription", "ModifyDate", "HostComputer", "Orientation"}
    struktura = {"IFD0:XResolution", "IFD0:YResolution", "IFD0:ResolutionUnit", "IFD0:YCbCrPositioning", "IFD0:Software"}

    def znacky(f):
        r = subprocess.run(et + ["-j", "-G1", "-n", f], capture_output=True, text=True)
        d = json.loads(r.stdout)[0]
        # z IFD0 len popisné značky (zvyšok IFD0 v TIFF opisuje uloženie obrazu)
        return {k: v for k, v in d.items() if k.split(":")[0] in skupiny and k not in struktura
                and (not k.startswith("IFD0:") or k[5:] in popisne)}

    zapis = ["-Make=Výrobca", "-Model=Model X", "-Artist=Autor Ž", "-Copyright=(c) 2026", "-ImageDescription=Popis",
             "-DateTimeOriginal=2026:11:25 12:00:00", "-ExposureTime=1/250", "-FNumber=2.8", "-ISO=200",
             "-FocalLength=35", "-LensModel=Objektív 35", "-GPSLatitude=48.1486", "-GPSLatitudeRef=N",
             "-GPSLongitude=17.1077", "-GPSLongitudeRef=E", "-GPSAltitude=140", "-XMP-dc:Title=Názov",
             "-XMP-dc:Subject=kvet", "-XMP-dc:Subject=orchidea", "-IPTC:Keywords=kvet", "-IPTC:Keywords=orchidea",
             "-IPTC:Caption-Abstract=Popis IPTC – ž", "-IPTC:By-line=Autor", "-IPTC:ObjectName=Názov",
             "-codedcharacterset=utf8"]
    pole = (rng.random((40, 60, 3)) * 255).astype(np.uint8)
    for zdroj, ulozenie in (("jpg", "JPEG"), ("tif", "TIFF"), ("png", "PNG")):
        cesta = os.path.join(PRAC, f"meta_zdroj.{zdroj}")
        Image.fromarray(pole).save(cesta, ulozenie)
        r = subprocess.run(et + ["-q", "-overwrite_original"] + zapis + [cesta], capture_output=True, text=True)
        povodne = znacky(cesta)
        if r.returncode != 0 or len(povodne) < 20:
            zapis_vysledok("Metadáta", f"zdroj {zdroj}", False, f"ExifTool nezapísal značky: {r.stderr[:120]}")
            continue
        # naše čítanie vybraných položiek proti ExifToolu
        nase_r = subprocess.run([PROGRAM, "metadata", cesta], capture_output=True, text=True).stdout.splitlines()
        nase_p = {}
        for riadok in nase_r:
            c = riadok.split("\t")
            if c[0] == "EXIF":
                nase_p[c[2]] = c[4]
            elif c[0] == "IPTC":
                nase_p.setdefault("IPTC:" + c[1], []).append(c[2])
        chyby = []
        for k_et, k_nase in (("IFD0:Make", "Make"), ("IFD0:Model", "Model"), ("ExifIFD:DateTimeOriginal", "DateTimeOriginal"),
                             ("ExifIFD:LensModel", "LensModel"), ("IFD0:Artist", "Artist")):
            if str(povodne.get(k_et)) != nase_p.get(k_nase):
                chyby.append(f"{k_nase}: {nase_p.get(k_nase)!r} ≠ {povodne.get(k_et)!r}")
        for k_et, k_nase in (("ExifIFD:ExposureTime", "ExposureTime"), ("ExifIFD:FNumber", "FNumber"),
                             ("ExifIFD:ISO", "ISO"), ("ExifIFD:FocalLength", "FocalLength"), ("GPS:GPSAltitude", "GPSAltitude")):
            try:
                if abs(float(nase_p[k_nase]) - float(povodne[k_et])) > 1e-6:
                    chyby.append(f"{k_nase}: {nase_p[k_nase]} ≠ {povodne[k_et]}")
            except (KeyError, ValueError):
                chyby.append(f"{k_nase} chýba")
        kl = povodne.get("IPTC:Keywords")
        if (kl if isinstance(kl, list) else [kl]) != nase_p.get("IPTC:Keywords"):
            chyby.append(f"IPTC Keywords {nase_p.get('IPTC:Keywords')} ≠ {kl}")
        if [povodne.get("IPTC:Caption-Abstract")] != nase_p.get("IPTC:Caption-Abstract"):
            chyby.append("IPTC Caption-Abstract")
        zapis_vysledok("Metadáta", f"čítanie {zdroj}", not chyby, "; ".join(chyby)[:200] or f"{len(nase_r)} položiek ako ExifTool")
        for ciel in ("jpg", "tif", "png"):
            vystup = os.path.join(PRAC, f"meta_{zdroj}_na.{ciel}")
            try:
                nas_prevod(cesta, vystup)
                nove = znacky(vystup)
                rozdiel = {k: (v, nove.get(k)) for k, v in povodne.items() if nove.get(k) != v}
                zapis_vysledok("Metadáta", f"{zdroj} → {ciel}", not rozdiel,
                               str(rozdiel)[:200] if rozdiel else f"{len(povodne)} značiek Exif/GPS/XMP/IPTC zachovaných")
            except Exception as e:  # noqa: BLE001
                zapis_vysledok("Metadáta", f"{zdroj} → {ciel}", False, str(e)[:160])


def main():
    rng = np.random.default_rng(2026)
    overenia = {"png": over_png, "jpeg": over_jpeg, "tiff": over_tiff, "exr": over_exr, "jxl": over_jxl,
                "metadata": over_metadata}
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
