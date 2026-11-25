#!/usr/bin/env python3
"""Nezávislé overenie balíka farba (CI): prenosové funkcie a matice proti colour-science, prevody ICC proti
Little CMS (Pillow ImageCms), výstup ACES 1.x (RRT + ODT SDR) proti OpenColorIO 2.

Použitie: over_farbu.py <cesta k programu rozhodca> [priečinky s profilmi ICC …] (prehľadávajú sa aj podpriečinky)
Rozhodcovia sa používajú len tu, nikdy ako predloha kódu.
"""
import glob
import io
import os
import subprocess
import sys

import numpy as np

np.seterr(all="ignore")

PROGRAM = sys.argv[1]
PROFILY = sys.argv[2:]
zlyhania = []
skusok = 0


def spusti(argumenty, vstup):
    v = subprocess.run([PROGRAM] + argumenty, input=vstup, capture_output=True)
    if v.returncode != 0:
        # zlyhanie nášho programu je výsledok skúšky (ZLYHALO), nie pád overovania
        raise RuntimeError(f"rozhodca {' '.join(argumenty[:3])}: {v.stderr.decode(errors='replace').strip()[-300:]}")
    return v.stdout


def cisla(argumenty, hodnoty):
    text = " ".join(repr(float(v)) for v in np.ravel(hodnoty)).encode()
    return np.array(spusti(argumenty, text).split(), dtype=np.float64)


def porovnaj(meno, nase, ich, tolerancia, relativne=False):
    global skusok
    skusok += 1
    nase, ich = np.asarray(nase, dtype=np.float64), np.asarray(ich, dtype=np.float64)
    rozdiel = np.abs(nase - ich)
    if relativne:
        rozdiel = rozdiel / np.maximum(np.abs(ich), 1e-12)
    najviac = float(np.max(rozdiel))
    stav = "OK" if najviac <= tolerancia else "ZLYHALO"
    print(f"{stav:8} {meno}: najväčší rozdiel {najviac:.3g} (tolerancia {tolerancia:g})")
    if najviac > tolerancia:
        zlyhania.append(meno)


def prenosy():
    import colour

    x = np.linspace(0.0, 1.0, 1001)
    lin = np.concatenate([np.geomspace(1e-6, 1.0, 600), np.linspace(0.0, 1.0, 401)])
    pary = [
        ("srgb", lambda v: colour.cctf_decoding(v, "sRGB"), lambda l: colour.cctf_encoding(l, "sRGB")),
        ("bt709", lambda v: colour.models.oetf_inverse_BT709(v), lambda l: colour.models.oetf_BT709(l)),
        ("pq", lambda v: colour.models.eotf_ST2084(v) / 10000.0, lambda l: colour.models.eotf_inverse_ST2084(l * 10000.0)),
        ("hlg", lambda v: colour.models.oetf_inverse_BT2100_HLG(v), lambda l: colour.models.oetf_BT2100_HLG(l)),
        ("prophoto", lambda v: colour.models.cctf_decoding_ProPhotoRGB(v), lambda l: colour.models.cctf_encoding_ProPhotoRGB(l)),
        ("acescc", lambda v: colour.models.log_decoding_ACEScc(v), lambda l: colour.models.log_encoding_ACEScc(l)),
        ("acescct", lambda v: colour.models.log_decoding_ACEScct(v), lambda l: colour.models.log_encoding_ACEScct(l)),
    ]
    for meno, do_lin, z_lin in pary:
        vstup = x if meno not in ("acescc", "acescct") else np.linspace(-0.35, 1.4, 1001)
        porovnaj(f"prenos {meno} → lineárne", cisla(["prenos", meno, "do"], vstup), do_lin(vstup), 1e-8, True)
        l = lin[lin > 0] if meno in ("acescc",) else lin
        porovnaj(f"prenos {meno} ← lineárne", cisla(["prenos", meno, "z"], l), z_lin(l), 1e-9)


def matice():
    import colour

    mena = {"srgb": "sRGB", "bt2020": "ITU-R BT.2020", "dcip3": "DCI-P3", "p3": "Display P3",
            "adobe": "Adobe RGB (1998)", "prophoto": "ProPhoto RGB", "ap0": "ACES2065-1", "ap1": "ACEScg"}
    pary = [("srgb", "bt2020"), ("srgb", "prophoto"), ("p3", "srgb"), ("dcip3", "srgb"), ("ap0", "ap1"),
            ("ap0", "srgb"), ("adobe", "ap1"), ("prophoto", "bt2020")]
    for z, c in pary:
        nase = cisla(["matica", z, c], []).reshape(3, 3)
        zc, cc = colour.RGB_COLOURSPACES[mena[z]], colour.RGB_COLOURSPACES[mena[c]]
        # matice z chromatickostí (colour inak používa zaokrúhlené tabuľky noriem)
        zc, cc = zc.copy(), cc.copy()
        zc.use_derived_transformation_matrices(True)
        cc.use_derived_transformation_matrices(True)
        ich = colour.matrix_RGB_to_RGB(zc, cc, chromatic_adaptation_transform="Bradford")
        porovnaj(f"matica {z} → {c} (Bradford)", nase, ich, 1e-9)


def aces():
    import PyOpenColorIO as ocio

    rng = np.random.default_rng(2026)
    hodnoty = np.concatenate([
        np.geomspace(1e-4, 200.0, 300)[:, None].repeat(3, 1),
        np.exp(rng.uniform(np.log(1e-4), np.log(60.0), (3000, 3))),
        rng.uniform(0.0, 1.0, (1000, 3)) * rng.choice([0.05, 1.0, 8.0], (1000, 1)),
    ]).astype(np.float32)
    nase = cisla(["aces"], hodnoty.astype(np.float64)).reshape(-1, 3)
    cfg = ocio.Config.CreateRaw()
    proc = cfg.getProcessor(ocio.BuiltinTransform("ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO_1.0"))
    cpu = proc.getDefaultCPUProcessor()
    ich = hodnoty.copy()
    cpu.applyRGB(ich)
    # OpenColorIO počíta v 32-bitových plávajúcich číslach – tolerancia relatívna k jasu výstupu
    porovnaj("ACES 1.x RRT + ODT SDR (XYZ D65)", nase, ich, 2e-4, False)


def icc():
    from PIL import Image, ImageCms

    if not PROFILY:
        print("bez priečinka s profilmi ICC – preskočené")
        return
    BEZ_OPTIMALIZACIE = 0x0100  # cmsFLAGS_NOOPTIMIZE: rozhodca počíta celý reťazec po bodoch
    mriezka = np.stack(np.meshgrid(*[np.linspace(0, 255, 18).round()] * 3, indexing="ij"), -1).reshape(-1, 3)
    rgb = mriezka.astype(np.uint8)
    srgb = ImageCms.createProfile("sRGB")

    def lcms(z, c, rz, rc, zamer, vstup, k_von, flags=0):
        t = ImageCms.buildTransform(z, c, rz, rc, renderingIntent=zamer, flags=flags)
        obr = Image.frombytes(rz, (len(vstup), 1), vstup.tobytes())
        # priamo jadro Little CMS (Pillow inak pripája výstupný profil a pri niektorých profiloch zlyhá)
        o = Image.new(rc, obr.size)
        vez = (lambda i: i.getim()) if hasattr(obr, "getim") else (lambda i: i.im)
        t.transform.apply(vez(obr), vez(o))
        return np.frombuffer(o.tobytes(), dtype=np.uint8).reshape(-1, k_von).astype(int)

    def nase(z, c, zamer, k, vstup, k_von, presne=False):
        a = ["icc", z, c, str(zamer), str(k)] + (["presne"] if presne else [])
        return np.frombuffer(spusti(a, vstup.tobytes()), dtype=np.uint8).reshape(-1, k_von).astype(int)

    cesty = sorted({c for p in PROFILY for c in glob.glob(os.path.join(p, "**", "*.ic[cm]"), recursive=True)})
    for cesta in cesty:
        try:
            try:
                profil = ImageCms.getOpenProfile(cesta)
            except Exception:
                continue
            priestor = profil.profile.xcolor_space.strip()
            if priestor == "RGB":
                rezim, vstup, k = "RGB", rgb, 3
            elif priestor == "GRAY":
                rezim, vstup, k = "L", rgb[:, :1].copy(), 1
            elif priestor == "CMYK":
                g = np.stack(np.meshgrid(*[np.linspace(0, 255, 9).round()] * 4, indexing="ij"), -1).reshape(-1, 4)
                rezim, vstup, k = "CMYK", g.astype(np.uint8), 4
            else:
                continue
            meno = os.path.basename(cesta)
            with open(cesta, "rb") as f:
                tabulkovy = b"A2B0" in f.read(132 + 12 * 200)
            # percepčný zámer: Little CMS pri profile v4 (jeho sRGB) zapína kompenzáciu čierneho bodu (ISO 18619),
            # ktorú farba zatiaľ nemá – porovnáva sa len pri profiloch bez tabuliek
            zamery = (1,) if tabulkovy else (0, 1)
            for zamer in zamery:
                n = nase(cesta, "srgb", zamer, k, vstup, 3)
                i = lcms(profil, srgb, rezim, "RGB", zamer, vstup, 3)
                j = lcms(profil, srgb, rezim, "RGB", zamer, vstup, 3, BEZ_OPTIMALIZACIE)
                i = np.where(np.abs(n - i).max(1, keepdims=True) <= np.abs(n - j).max(1, keepdims=True), i, j)
                porovnaj(f"ICC {meno} → sRGB, zámer {zamer}", n, i, 2)
                porovnaj(f"ICC {meno} → sRGB, zámer {zamer}, presne", nase(cesta, "srgb", zamer, k, vstup, 3, True),
                         lcms(profil, srgb, rezim, "RGB", zamer, vstup, 3, BEZ_OPTIMALIZACIE), 2)
            if priestor not in ("RGB", "CMYK"):
                continue
            kc = 3 if priestor == "RGB" else 4
            # profil len so smerom do PCS (napr. CMYK „micro“ bez B2A) – cieľom byť nemôže ani pre rozhodcu
            try:
                ImageCms.buildTransform(srgb, profil, "RGB", rezim, renderingIntent=1)
            except Exception:
                print(f"PRESKOČENÉ sRGB → {meno}: profil nemá spätný smer (ani rozhodca ho nevie použiť ako cieľ)")
                continue
            for presne in (False, True):
                n = nase("srgb", cesta, 1, 3, rgb, kc, presne)
                i = lcms(srgb, profil, "RGB", rezim, 1, rgb, kc, BEZ_OPTIMALIZACIE if presne else 0)
                if not presne:
                    # pri málo bodoch počíta farba presne aj bez voľby (mriežka by bola zbytočná) – uzná sa zhoda
                    # s optimalizovaným aj presným výsledkom rozhodcu
                    j = lcms(srgb, profil, "RGB", rezim, 1, rgb, kc, BEZ_OPTIMALIZACIE)
                    i = np.where(np.abs(n - i).max(1, keepdims=True) <= np.abs(n - j).max(1, keepdims=True), i, j)
                # krivky s plochými úsekmi (Cineon: čierna a biela orezané) nemajú jednoznačnú inverziu – bod sa
                # uzná aj vtedy, keď oba výsledky dajú späť v sRGB tú istú farbu (to, čo z hodnôt naozaj vznikne)
                if priestor == "RGB":
                    spat_n = lcms(profil, srgb, "RGB", "RGB", 1, n.astype(np.uint8), 3)
                    spat_i = lcms(profil, srgb, "RGB", "RGB", 1, i.astype(np.uint8), 3)
                    rozdiel = np.minimum(np.abs(n - i).max(1), np.abs(spat_n - spat_i).max(1))
                    n, i = rozdiel, np.zeros_like(rozdiel)
                porovnaj(f"ICC sRGB → {meno}, zámer 1{', presne' if presne else ''}", n, i, 2)
        except RuntimeError as e:
            print(f"ZLYHALO   {os.path.basename(cesta)}: {e}")
            zlyhania.append(os.path.basename(cesta))


for cast in (prenosy, matice, aces, icc):
    try:
        cast()
    except RuntimeError as e:
        print(f"ZLYHALO   {cast.__name__}: {e}")
        zlyhania.append(cast.__name__)
    except ImportError as e:
        print(f"rozhodca chýba ({e}) – časť {cast.__name__} preskočená")
        if os.environ.get("CI"):
            zlyhania.append(cast.__name__)

print(f"\n{skusok - len(zlyhania)}/{skusok} skúšok v poriadku")
sys.exit(1 if zlyhania else 0)
