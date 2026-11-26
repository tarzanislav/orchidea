#!/usr/bin/env python3
"""Nezávislé overenie Pohľadu (CI): zmenšenie proti Pillow, hromadný prevod, brána E1 – 1000 fotiek bez
zamrznutia, s obmedzeným počtom vlákien a ohraničenou pamäťou – náhľady priečinka (natočenie, zoznam) a
porovnanie obrázkov (PSNR, SSIM proti scikit-image).

Použitie: over_pohlad.py <program pohlad> <pracovný priečinok> [počet fotiek brány, predvolene 1000]
Rozhodca: Pillow (prevzorkovanie Lanczos v plávajúcom režime „F“ v lineárnom svetle, čítanie výsledkov).
"""
import os
import subprocess
import sys
import time

import numpy as np
from PIL import Image

PROGRAM, PRAC = sys.argv[1], sys.argv[2]
FOTIEK = int(sys.argv[3]) if len(sys.argv) > 3 else 1000
os.makedirs(PRAC, exist_ok=True)
zlyhania = []


def vysledok(meno, ok, poznamka):
    print(f"{'OK      ' if ok else 'ZLYHALO '} {meno}: {poznamka}")
    if not ok:
        zlyhania.append(meno)


def do_lin(v):
    v = v / 255.0
    return np.where(v <= 0.04045, v / 12.92, ((v + 0.055) / 1.055) ** 2.4)


def z_lin(l):
    l = np.clip(l, 0, 1)
    return np.where(l <= 0.0031308, 12.92 * l, 1.055 * l ** (1 / 2.4) - 0.055) * 255.0


def obraz(rng, h, w, c, foto=False):
    """Hladký obraz so šumom; `foto` = podobný fotke (pomalšie zmeny, menej šumu)."""
    y, x = np.mgrid[0:h, 0:w]
    s = 6.0 if foto else 1.0
    z = np.stack([(np.sin(x / (s * (5 + 3 * k)) + y / (s * (9 + k))) + 1) / 2 for k in range(c)], -1)
    sum_ = 0.05 if foto else 0.2
    return np.clip((z * (1 - sum_) + rng.random((h, w, c)) * sum_) * 255, 0, 255).astype(np.uint8)


def citaj_surove(cesta):
    with open(cesta, "rb") as f:
        w, h, c, typ = f.readline().decode().split()
        return np.frombuffer(f.read(), dtype=np.uint8).reshape(int(h), int(w), int(c))


def zmensenie(rng):
    for (h, w, c, nw, nh) in [(120, 160, 3, 40, 30), (97, 131, 4, 33, 17), (64, 64, 1, 50, 50), (50, 70, 3, 140, 100)]:
        p = obraz(rng, h, w, c)
        if c == 4:
            p[..., 3] = (rng.random((h, w)) * 255).astype(np.uint8)
        zdroj = os.path.join(PRAC, f"zm_{w}x{h}_{c}.png")
        Image.fromarray(p if c > 1 else p[..., 0]).save(zdroj)
        ciel = zdroj + ".surove"
        subprocess.run([PROGRAM, "zmensi", zdroj, ciel, str(nw), str(nh)], check=True)
        nase = citaj_surove(ciel).astype(float)
        # rozhodca: Pillow Lanczos na kanáloch v lineárnom svetle, farba vynásobená alfou
        lin = do_lin(p.astype(float))
        alfa = p[..., 3:4] / 255.0 if c == 4 else None
        if alfa is not None:
            lin[..., :3] *= alfa
            lin[..., 3:4] = alfa
        kanaly = [np.asarray(Image.fromarray(lin[..., k].astype(np.float32), "F").resize((nw, nh), Image.LANCZOS))
                  for k in range(c)]
        ref = np.stack(kanaly, -1).astype(float)
        if alfa is not None:
            a = ref[..., 3:4]
            ref[..., :3] = np.where(a > 1e-8, ref[..., :3] / np.maximum(a, 1e-8), 0)
            ref = np.concatenate([np.round(z_lin(ref[..., :3])), np.round(np.clip(a, 0, 1) * 255)], -1)
            # pri takmer nulovej alfe je farba neurčitá – porovná sa len tam, kde je bod viditeľný
            maska = ref[..., 3:4] >= 8
            rozdiel = np.abs(nase - ref) * np.concatenate([maska.repeat(3, -1), np.ones_like(maska)], -1)
        else:
            ref = np.round(z_lin(ref))
            rozdiel = np.abs(nase - ref)
        vysledok(f"zmenšenie {w}×{h}×{c} → {nw}×{nh}", rozdiel.max() <= 1,
                 f"najväčší rozdiel {rozdiel.max():.0f}/255 voči Pillow (Lanczos, lineárne svetlo)")


def davka_bez_straty(rng):
    vstup = os.path.join(PRAC, "bez_straty")
    os.makedirs(vstup, exist_ok=True)
    polia = {}
    for i in range(5):
        p = obraz(rng, 30 + i, 40 + 2 * i, 3)
        polia[f"s{i}"] = p
        Image.fromarray(p).save(os.path.join(vstup, f"s{i}.png"))
    for fmt, prip in (("tif", ".tif"), ("png", ".png")):
        von = os.path.join(PRAC, f"bez_straty_{fmt}")
        r = subprocess.run([PROGRAM, "davka", vstup, von, "--format", fmt, "--meno", "{meno}_{cislo:02}"],
                           capture_output=True, text=True)
        ok = r.returncode == 0
        for i in range(5):
            cesta = os.path.join(von, f"s{i}_{i + 1:02}{prip}")
            ok = ok and os.path.exists(cesta) and np.array_equal(np.asarray(Image.open(cesta)), polia[f"s{i}"])
        vysledok(f"dávka PNG → {fmt}", ok, "obsah bit po bite a mená podľa šablóny" if ok else r.stderr[-200:])


def brana(rng):
    vstup = os.path.join(PRAC, "brana_vstup")
    von = os.path.join(PRAC, "brana_vystup")
    os.makedirs(vstup, exist_ok=True)
    zakladne = [obraz(rng, 480, 640, 3) for _ in range(8)]
    exif = Image.Exif()
    exif[0x010F] = "ORCHIDEA skuska"
    for i in range(FOTIEK):
        Image.fromarray(np.roll(zakladne[i % 8], i, axis=1)).save(os.path.join(vstup, f"foto_{i:04}.jpg"),
                                                                    quality=85, exif=exif)
    vlakna = 2
    zaciatok = time.monotonic()
    proc = subprocess.Popen([PROGRAM, "davka", vstup, von, "--format", "jpg", "--max", "320", "--vlakna", str(vlakna),
                             "--meno", "{cislo:04}_{meno}"], stdout=subprocess.PIPE, text=True)
    najviac_vlakien, najviac_pamate = 0, 0
    casy = []
    import threading

    def sleduj():
        nonlocal najviac_vlakien, najviac_pamate
        while proc.poll() is None:
            try:
                with open(f"/proc/{proc.pid}/status") as f:
                    for r in f:
                        if r.startswith("Threads:"):
                            najviac_vlakien = max(najviac_vlakien, int(r.split()[1]))
                        if r.startswith("VmRSS:"):
                            najviac_pamate = max(najviac_pamate, int(r.split()[1]))
            except OSError:
                pass
            time.sleep(0.05)

    t = threading.Thread(target=sleduj)
    t.start()
    koniec = ""
    for riadok in proc.stdout:
        if riadok.startswith("SUBOR"):
            casy.append(int(riadok.split()[2]))
        elif riadok.startswith("KONIEC"):
            koniec = riadok.strip()
    proc.wait()
    t.join()
    trvanie = time.monotonic() - zaciatok
    medzery = np.diff([0] + casy) if casy else np.array([0])
    vystupy = sorted(os.listdir(von)) if os.path.isdir(von) else []
    vysledok("brána E1: všetky fotky", proc.returncode == 0 and len(vystupy) == FOTIEK and len(casy) == FOTIEK,
             f"{len(vystupy)}/{FOTIEK} výstupov za {trvanie:.1f} s ({koniec})")
    vysledok("brána E1: bez zamrznutia", medzery.max() < 2000,
             f"najdlhšia medzera medzi hláseniami {medzery.max()} ms (priemer {medzery.mean():.0f} ms)")
    # hlavné vlákno + pracovné vlákno + najviac `vlakna` pomocných pri súbežnej práci
    vysledok("brána E1: obmedzené vlákna", 0 < najviac_vlakien <= vlakna + 2,
             f"najviac {najviac_vlakien} vlákien procesu (limit {vlakna} + hlavné + pracovné)")
    vysledok("brána E1: ohraničená pamäť", 0 < najviac_pamate < 200 * 1024, f"najviac {najviac_pamate / 1024:.0f} MB")
    if vystupy:
        prvy = Image.open(os.path.join(von, vystupy[0]))
        ok = prvy.size == (320, 240) and prvy.getexif().get(0x010F) == "ORCHIDEA skuska" and vystupy[0] == "0001_foto_0000.jpg"
        vysledok("brána E1: rozmer, Exif a meno", ok, f"{vystupy[0]} {prvy.size} Make={prvy.getexif().get(0x010F)!r}")


def rozmer_do(w, h, m):
    if max(w, h) <= m:
        return w, h
    k = m / max(w, h)
    return max(1, round(w * k)), max(1, round(h * k))


def referencny_nahlad(cesta, strana):
    from PIL import ImageOps
    im = ImageOps.exif_transpose(Image.open(cesta))
    if im.mode == "I;16":
        p = (np.asarray(im).astype(float) / 257.0)[..., None]
    else:
        im = im.convert("RGBA" if "A" in im.mode else ("L" if im.mode in ("L", "I;16") else "RGB"))
        p = np.asarray(im).astype(float)
        if p.ndim == 2:
            p = p[..., None]
    h, w, c = p.shape
    nw, nh = rozmer_do(w, h, strana)
    lin = do_lin(p)
    if c == 4:
        a = p[..., 3:4] / 255.0
        lin[..., :3] *= a
        lin[..., 3:4] = a
    kan = [np.asarray(Image.fromarray(lin[..., k].astype(np.float32), "F").resize((nw, nh), Image.LANCZOS)) for k in range(c)]
    r = np.stack(kan, -1).astype(float)
    if c == 4:
        a = r[..., 3:4]
        r[..., :3] = np.where(a > 1e-8, r[..., :3] / np.maximum(a, 1e-8), 0)
        return np.concatenate([np.round(z_lin(r[..., :3])), np.round(np.clip(a, 0, 1) * 255)], -1)
    return np.round(z_lin(r))


def nahlady(rng):
    pr = os.path.join(PRAC, "nahlady")
    von = os.path.join(PRAC, "nahlady_von")
    os.makedirs(pr, exist_ok=True)
    # JPEG: obraz podobný fotke – zmenšenie v DCT priemeruje 8 × 8 v kódovaných hodnotách, pri ostrých pruhoch
    # by sa líšilo od lineárneho svetla viac (vlastnosť rýchleho náhľadu, nie chyba)
    zaklad = obraz(rng, 300, 400, 3, foto=True)
    for n in range(1, 9):
        e = Image.Exif()
        e[0x0112] = n
        Image.fromarray(zaklad).save(os.path.join(pr, f"natocenie_{n}.jpg"), quality=95, exif=e)
    rgba = obraz(rng, 90, 150, 4)
    Image.fromarray(rgba).save(os.path.join(pr, "alfa.png"))
    Image.fromarray(obraz(rng, 200, 120, 1)[..., 0]).save(os.path.join(pr, "siva.png"))
    Image.fromarray((obraz(rng, 100, 140, 1)[..., 0].astype(np.uint16) * 257)).save(os.path.join(pr, "hlboka16.tif"))
    with open(os.path.join(pr, "poznamky.txt"), "w") as f:
        f.write("nie je obrázok")
    with open(os.path.join(pr, "falosny.jpg"), "w") as f:
        f.write("ani toto nie je obrázok")
    strana = 64
    r = subprocess.run([PROGRAM, "nahlady", pr, str(strana), "--vystup", von], capture_output=True, text=True)
    riadky = [x for x in r.stdout.splitlines() if x.startswith("NAHLAD")]
    mena = [x.split()[5] for x in riadky if "chyba" not in x]
    ocakavane = sorted([f"natocenie_{n}.jpg" for n in range(1, 9)] + ["alfa.png", "siva.png", "hlboka16.tif"])
    vysledok("náhľady: zoznam podľa podpisu", sorted(mena) == ocakavane and r.returncode == 0,
             f"{len(mena)} obrázkov, nepatrí tam .txt ani falošný .jpg" if sorted(mena) == ocakavane else f"{mena} {r.stderr[-200:]}")
    for meno in ocakavane:
        cesta_nahladu = os.path.join(von, meno + ".png")
        if not os.path.exists(cesta_nahladu):
            vysledok(f"náhľad {meno}", False, "chýba")
            continue
        nas = np.asarray(Image.open(cesta_nahladu)).astype(float)
        if nas.ndim == 2:
            nas = nas[..., None]
        ref = referencny_nahlad(os.path.join(pr, meno), strana)
        if nas.shape != ref.shape:
            vysledok(f"náhľad {meno}", False, f"tvar {nas.shape} ≠ {ref.shape}")
            continue
        d = np.abs(nas - ref)
        # JPEG sa dekóduje zmenšený v oblasti DCT – malý rozdiel voči plnému dekódovaniu je očakávaný
        jpeg = meno.endswith(".jpg")
        ok = d.mean() <= (1.0 if jpeg else 0.5) and d.max() <= (12 if jpeg else 1)
        vysledok(f"náhľad {meno}", ok, f"{nas.shape[1]}×{nas.shape[0]}, priemer {d.mean():.2f}, najviac {d.max():.0f}/255 "
                                       f"voči Pillow (natočenie podľa Exif, Lanczos)")


def nahlady_brana():
    pr = os.path.join(PRAC, "brana_vstup")
    if not os.path.isdir(pr):
        return
    zaciatok = time.monotonic()
    r = subprocess.run([PROGRAM, "nahlady", pr, "160", "--vlakna", "2"], capture_output=True, text=True)
    casy = [int(x.split()[2]) for x in r.stdout.splitlines() if x.startswith("NAHLAD")]
    medzery = np.diff([0] + casy) if casy else np.array([0])
    vysledok("náhľady priečinka s 1000 fotkami", r.returncode == 0 and len(casy) == FOTIEK and "chyba" not in r.stdout,
             f"{len(casy)} náhľadov 160 px za {time.monotonic() - zaciatok:.1f} s, najdlhšia medzera {medzery.max()} ms")



def porovnanie(rng):
    """PSNR a SSIM proti scikit-image (structural_similarity s Gaussovým oknom σ = 1,5 ako Wang 2004)."""
    try:
        from skimage.metrics import peak_signal_noise_ratio, structural_similarity
    except ImportError:
        vysledok("porovnanie", False, "rozhodca scikit-image chýba")
        return
    pripady = []
    a = obraz(rng, 120, 160, 3, foto=True)
    sum_ = np.clip(a.astype(int) + rng.integers(-12, 13, a.shape), 0, 255).astype(np.uint8)
    pripady.append(("šum ±12", a, sum_))
    jpeg_cesta = os.path.join(PRAC, "por_jpeg.jpg")
    Image.fromarray(a).save(jpeg_cesta, quality=40)
    pripady.append(("JPEG kvalita 40", a, np.asarray(Image.open(jpeg_cesta))))
    s = obraz(rng, 64, 80, 1)[..., 0]
    pripady.append(("sivý posun o 1 bod", s, np.roll(s, 1, axis=1)))
    hl = (obraz(rng, 50, 70, 3).astype(np.uint16) * 257)
    pripady.append(("16 bitov", hl, np.clip(hl.astype(int) + 300, 0, 65535).astype(np.uint16)))
    for meno, x, y in pripady:
        ca, cb = os.path.join(PRAC, "por_a.png"), os.path.join(PRAC, "por_b.png")
        if x.dtype == np.uint16:
            ca, cb = ca[:-4] + ".tif", cb[:-4] + ".tif"
            # 16 bitov: Pillow zapíše len jednokanálový 16-bitový TIFF – porovná sa prvý kanál
            for cesta, pole in ((ca, x), (cb, y)):
                Image.fromarray(pole[..., 0]).save(cesta)
            x, y = x[..., 0], y[..., 0]
        else:
            Image.fromarray(x).save(ca)
            Image.fromarray(y).save(cb)
        rozsah = 65535 if x.dtype == np.uint16 else 255
        r = subprocess.run([PROGRAM, "porovnaj", ca, cb], capture_output=True, text=True)
        try:
            c = r.stdout.split()
            psnr, ssim = float(c[5]), float(c[7])
        except (IndexError, ValueError):
            vysledok(f"porovnanie {meno}", False, r.stdout + r.stderr)
            continue
        ich_psnr = peak_signal_noise_ratio(x, y, data_range=rozsah)
        ich_ssim = structural_similarity(x, y, data_range=rozsah, gaussian_weights=True, sigma=1.5,
                                         use_sample_covariance=False, channel_axis=-1 if x.ndim == 3 else None)
        ok = abs(psnr - ich_psnr) < 1e-6 and abs(ssim - ich_ssim) < 1e-6
        vysledok(f"porovnanie {meno}", ok, f"PSNR {psnr:.4f} (ich {ich_psnr:.4f}) dB, SSIM {ssim:.6f} (ich {ich_ssim:.6f})")



def okno(rng):
    """Jadro okna bez okna (príkaz `snimka`): zobrazenie 1:1 a zmenšené, delené porovnanie, rozdiel, pokoj."""
    pr = os.path.join(PRAC, "okno")
    os.makedirs(pr, exist_ok=True)
    a = obraz(rng, 300, 400, 3, foto=True)
    b = np.clip(a.astype(int) + rng.integers(-20, 21, a.shape), 0, 255).astype(np.uint8)
    velky = obraz(rng, 1200, 1600, 3, foto=True)
    Image.fromarray(a).save(os.path.join(pr, "a.png"))
    Image.fromarray(b).save(os.path.join(pr, "b.png"))
    Image.fromarray(velky).save(os.path.join(pr, "c_velky.png"))

    def snimka(subor, *povely):
        von = os.path.join(PRAC, "okno_snimka.png")
        r = subprocess.run([PROGRAM, "snimka", os.path.join(pr, subor), "800", "600", von, "pas", "udaje", *povely],
                           capture_output=True, text=True)
        if r.returncode != 0:
            return None, r.stdout + r.stderr
        return np.asarray(Image.open(von).convert("RGB")).astype(int), r.stdout

    obr, txt = snimka("a.png")
    ok = obr is not None and np.array_equal(obr[150:450, 200:600], a) and "POKOJ ok" in txt and "VYKRESLENI 1 " in txt
    vysledok("okno: obraz 1:1 v strede", ok, "bit po bite, jedno vykreslenie, v pokoji sa nič nepočíta" if ok else txt[-200:])
    obr, txt = snimka("c_velky.png")
    if obr is not None:
        # zmenšenie na polovicu = priemer 2 × 2 v lineárnom svetle (úroveň pyramídy)
        lin = do_lin(velky.astype(float))
        ref = np.round(z_lin(lin.reshape(600, 2, 800, 2, 3).mean(axis=(1, 3))))
        d = np.abs(obr - ref)
        vysledok("okno: zmenšené na 50 %", d.max() <= 1, f"najviac {d.max():.0f}/255 voči priemeru v lineárnom svetle")
    obr, txt = snimka("a.png", "oznac", "dalsi", "rezim", "delic:400")
    if obr is not None:
        vlavo, vpravo = obr[150:450, 200:400], obr[150:450, 401:600]
        ok = np.array_equal(vlavo, a[:, :200]) and np.array_equal(vpravo, b[:, 201:])
        vysledok("okno: delené porovnanie A / B", ok, "vľavo A, vpravo B bit po bite" if ok else txt[-200:])
    obr, txt = snimka("a.png", "oznac", "dalsi", "rezim", "rezim")
    if obr is not None:
        ref = np.minimum(np.abs(a.astype(int) - b.astype(int)) * 8, 255)
        ok = np.array_equal(obr[150:450, 200:600], ref)
        vysledok("okno: rozdiel |A - B| × 8", ok, "bit po bite" if ok else "nezhoda")
    # prezentácia: celá plocha, čierne pozadie, bez pásu a údajov; časovač len počas prezentácie
    obr, txt = snimka("a.png", "udaje", "pas", "prezentacia")
    if obr is not None:
        ok = (np.array_equal(obr[150:450, 200:600], a) and obr[:150].max() == 0 and obr[450:].max() == 0
              and "CASOVAC 5000 ms" in txt and "POKOJ ok" in txt)
        vysledok("prezentácia: celá plocha, čierne okolie", ok, "obraz 1:1 v strede, okolie čierne, časovač 5 s" if ok else txt[-200:])
    obr, txt = snimka("a.png", "prezentacia", "tik")
    vysledok("prezentácia: tik = ďalší obraz", obr is not None and "b.png" in txt and np.array_equal(obr[150:450, 200:600], b),
             "po tiku obraz B (načítaný vopred)")
    _, txt = snimka("a.png", "prezentacia", "pauza")
    _, txt2 = snimka("a.png")
    vysledok("prezentácia: bez nej žiadny časovač", "CASOVAC ziadny" in txt and "CASOVAC ziadny" in txt2,
             "pozastavená aj bežné prezeranie bez časovača")



def png_s_gamou(cesta, pole, gama):
    """PNG s blokom gAMA (Pillow ho nezapíše) – vloží sa za IHDR."""
    import struct
    import zlib
    Image.fromarray(pole).save(cesta)
    d = open(cesta, "rb").read()
    telo = b"gAMA" + struct.pack(">I", round(gama * 100000))
    blok = struct.pack(">I", 4) + telo + struct.pack(">I", zlib.crc32(telo) & 0xFFFFFFFF)
    with open(cesta, "wb") as f:
        f.write(d[:33] + blok + d[33:])


def priestor_farby(rng):
    """Náhľad zobrazí súbor v sRGB podľa jeho farebného priestoru (tu PNG s gAMA 1/1,8)."""
    pr = os.path.join(PRAC, "priestor")
    os.makedirs(pr, exist_ok=True)
    p = obraz(rng, 40, 50, 3)
    png_s_gamou(os.path.join(pr, "gama18.png"), p, 1 / 1.8)
    von = os.path.join(PRAC, "priestor_von")
    r = subprocess.run([PROGRAM, "nahlady", pr, "64", "--vystup", von], capture_output=True, text=True)
    try:
        nas = np.asarray(Image.open(os.path.join(von, "gama18.png.png"))).astype(float)
    except OSError:
        vysledok("náhľad v sRGB podľa gAMA", False, r.stdout + r.stderr)
        return
    ref = np.round(z_lin((p / 255.0) ** 1.8))
    d = np.abs(nas - ref)
    vysledok("náhľad v sRGB podľa gAMA 1/1,8", d.max() <= 1, f"najviac {d.max():.0f}/255 (gama 1,8 → sRGB)")



def upravy(rng):
    """Úpravy v okne (jadro bez okna): bezstratové otočenie JPEG proti jpegtran, Exif, jas/kontrast/gama,
    orezanie na zobrazený výrez, zmenšenie, späť; uloženie vedľa pôvodného súboru."""
    import shutil

    jt = os.environ.get("ORCHIDEA_JPEGTRAN") or shutil.which("jpegtran")
    pr = os.path.join(PRAC, "upravy")
    if os.path.isdir(pr):
        shutil.rmtree(pr)
    os.makedirs(pr)
    a = obraz(rng, 300, 401, 3, foto=True)

    def snimka(subor, *povely):
        von = os.path.join(PRAC, "upravy_snimka.png")
        r = subprocess.run([PROGRAM, "snimka", os.path.join(pr, subor), "800", "600", von, "pas", "udaje", *povely],
                           capture_output=True, text=True)
        return r

    def ulozene(meno):
        c = os.path.join(pr, meno)
        return np.asarray(Image.open(c)).astype(int) if os.path.exists(c) else None

    # JPEG: otočiť, otočiť, späť, uložiť = otočenie o 90° bezstratovo
    Image.fromarray(a).save(os.path.join(pr, "j.jpg"), quality=90)
    snimka("j.jpg", "otoc", "otoc", "spat", "ulozit")
    nase = ulozene("j_upravene.jpg")
    if jt and nase is not None:
        ich = os.path.join(PRAC, "upravy_ich.jpg")
        subprocess.run([jt, "-rotate", "90", "-trim", "-outfile", ich, os.path.join(pr, "j.jpg")], check=True)
        i = np.asarray(Image.open(ich)).astype(int)
        ok = nase.shape == i.shape and np.array_equal(nase, i)
        vysledok("úpravy: JPEG otočiť ×2, späť, uložiť", ok, "bezstratovo, bit po bite ako jpegtran -rotate 90 -trim")
    else:
        vysledok("úpravy: JPEG otočiť ×2, späť, uložiť", False, "chýba výsledok alebo jpegtran")
    # JPEG s Exif 6: len uložiť = postaviť podľa Exif, natočenie 1
    e = Image.Exif()
    e[0x0112] = 6
    Image.fromarray(a).save(os.path.join(pr, "e.jpg"), quality=90, exif=e)
    snimka("e.jpg", "ulozit")
    nase = ulozene("e_upravene.jpg")
    if jt and nase is not None:
        ich = os.path.join(PRAC, "upravy_ich.jpg")
        subprocess.run([jt, "-rotate", "90", "-trim", "-outfile", ich, os.path.join(pr, "e.jpg")], check=True)
        n1 = Image.open(os.path.join(pr, "e_upravene.jpg")).getexif().get(0x0112)
        ok = np.array_equal(nase, np.asarray(Image.open(ich)).astype(int)) and n1 == 1
        vysledok("úpravy: postaviť podľa Exif 6", ok, f"bezstratovo ako jpegtran, natočenie {n1}")
    # PNG: jas, kontrast, gama (každý krok zaokrúhlený na 8 b)
    Image.fromarray(a).save(os.path.join(pr, "p.png"))
    snimka("p.png", "jas", "kontrast", "gama", "ulozit")
    nase = ulozene("p_upravene.png")
    v = a.astype(float) / 255
    for jas, kon, gam in ((0.05, 1.0, 1.0), (0.0, 1.1, 1.0), (0.0, 1.0, 1.1)):
        v = np.round(np.clip(((v ** (1 / gam)) - 0.5) * kon + 0.5 + jas, 0, 1) * 255) / 255
    ref = np.round(v * 255)
    d = np.abs(nase - ref).max() if nase is not None else 999
    vysledok("úpravy: jas, kontrast, gama", d <= 1, f"najviac {d:.0f}/255 voči vzorcu")
    # PNG 1600 × 1200: 1:1 a orezať na zobrazený výrez 800 × 600 v strede
    velky = obraz(rng, 1200, 1600, 3, foto=True)
    Image.fromarray(velky).save(os.path.join(pr, "v.png"))
    snimka("v.png", "1:1", "orezat", "ulozit")
    nase = ulozene("v_upravene.png")
    ok = nase is not None and nase.shape == (600, 800, 3) and np.array_equal(nase, velky[300:900, 400:1200])
    vysledok("úpravy: orezať na zobrazený výrez", ok, "800 × 600 zo stredu bit po bite" if ok else str(None if nase is None else nase.shape))
    # zmenšiť na polovicu (Lanczos v lineárnom svetle)
    snimka("p.png", "zmensit", "ulozit")
    nase = ulozene("p_upravene (2).png")
    if nase is not None:
        lin = do_lin(a.astype(float))
        ref = np.stack([np.asarray(Image.fromarray(lin[..., k].astype(np.float32), "F").resize((200, 150), Image.LANCZOS))
                        for k in range(3)], -1)
        d = np.abs(nase - np.round(z_lin(ref))).max()
        vysledok("úpravy: zmenšiť na polovicu", nase.shape == (150, 200, 3) and d <= 1, f"{nase.shape[1]}×{nase.shape[0]}, najviac {d:.0f}/255 voči Pillow")
    else:
        vysledok("úpravy: zmenšiť na polovicu", False, "chýba výsledok")



def herbar(rng):
    """Herbár: index priečinkov bez importu, metadáta proti ExifTool, dotazy, obnova len zmenených, rýchlosť."""
    import json
    import shutil

    et = os.environ.get("ORCHIDEA_EXIFTOOL", "").split() or ([shutil.which("exiftool")] if shutil.which("exiftool") else [])
    if not et:
        vysledok("Herbár", False, "rozhodca ExifTool chýba")
        return
    koren = os.path.join(PRAC, "herbar")
    if os.path.isdir(koren):
        shutil.rmtree(koren)
    os.makedirs(os.path.join(koren, "2023", "vianoce"))
    os.makedirs(os.path.join(koren, "2024"))
    katalog = os.path.join(PRAC, "herbar.txt")
    if os.path.exists(katalog):
        os.remove(katalog)
    vzory = [
        ("2024", ["-Make=Canon", "-Model=Canon EOS R6", "-DateTimeOriginal=2024:05:10 10:00:00", "-ISO=200",
                  "-IPTC:Keywords=orchidea", "-IPTC:Keywords=kvet", "-XMP-dc:Subject=skleník", "-XMP-xmp:Rating=5"]),
        ("2024", ["-Make=NIKON CORPORATION", "-Model=NIKON Z 6", "-DateTimeOriginal=2024:08:01 18:30:00", "-ISO=3200",
                  "-XMP-dc:Subject=západ slnka", "-XMP-dc:Title=Večer pri Dunaji"]),
        (os.path.join("2023", "vianoce"), ["-Make=Apple", "-Model=iPhone 15", "-DateTimeOriginal=2023:12:24 19:00:00",
                  "-IPTC:Keywords=Vianoce", "-IPTC:Caption-Abstract=Štedrý večer", "-GPSLatitude=48.15",
                  "-GPSLatitudeRef=N", "-GPSLongitude=17.11", "-GPSLongitudeRef=E", "-codedcharacterset=utf8"]),
    ]
    pole = obraz(rng, 120, 160, 3, foto=True)
    subory = []
    for i, (priecinok, tagy) in enumerate(vzory):
        c = os.path.join(koren, priecinok, f"vzor_{i}.jpg")
        Image.fromarray(pole).save(c, quality=80)
        subprocess.run(et + ["-q", "-overwrite_original"] + tagy + [c], check=True)
        subory.append(c)
    # kópie (rovnaké metadáta) – spolu 300 fotiek, plus PNG bez metadát a súbor, ktorý nie je obrázok
    for k in range(297):
        zdroj = subory[k % 3]
        shutil.copy(zdroj, os.path.join(os.path.dirname(zdroj), f"kopia_{k:03}.jpg"))
    Image.fromarray(pole).save(os.path.join(koren, "2024", "bez_metadat.png"))
    with open(os.path.join(koren, "poznamky.txt"), "w") as f:
        f.write("nie je obrázok")
    vsetky = sorted(os.path.join(d, f) for d, _, fs in os.walk(koren) for f in fs if not f.endswith(".txt"))

    def herbar_cmd(*arg):
        return subprocess.run([PROGRAM, "herbar", katalog, *arg], capture_output=True, text=True)

    r = herbar_cmd("pridaj", koren)
    vysledok("Herbár: index priečinka s podpriečinkami", "precitanych=301" in r.stdout,
             r.stdout.strip() + (f" ({len(vsetky)} obrázkov, .txt sa nepridal)" if "precitanych=301" in r.stdout else r.stderr[-200:]))
    # metadáta proti ExifTool
    ich = {d["SourceFile"]: d for d in json.loads(subprocess.run(et + ["-j", "-n", "-r", koren], capture_output=True, text=True).stdout)}

    def hladaj(dotaz):
        r = herbar_cmd("hladaj", dotaz)
        return sorted(x.split(" | ")[0][7:] for x in r.stdout.splitlines() if x.startswith("ZAZNAM ")), r.stdout

    def ocakavane(f):
        return sorted(c for c in vsetky if f(ich.get(c, {})))

    def zoznam(v):
        return v if isinstance(v, list) else ([v] if v is not None else [])

    dotazy = [
        ("kvet", lambda d: "kvet" in zoznam(d.get("Keywords"))),
        ("fotoaparat:nikon", lambda d: "NIKON" in str(d.get("Model", ""))),
        ("datum:2024", lambda d: str(d.get("DateTimeOriginal", "")).startswith("2024")),
        ("datum:2023-12..2024-05", lambda d: "2023:12" <= str(d.get("DateTimeOriginal", ""))[:7] <= "2024:05"),
        ("iso>=800", lambda d: (d.get("ISO") or 0) >= 800),
        ("zapad", lambda d: "západ slnka" in zoznam(d.get("Subject"))),
        ("stedry", lambda d: "Štedrý" in str(d.get("Caption-Abstract", ""))),
        ("gps:ano", lambda d: "GPSLatitude" in d),
        ("hodnotenie>=5", lambda d: (d.get("Rating") or 0) >= 5),
        ("-kvet format:jpeg", lambda d: "kvet" not in zoznam(d.get("Keywords")) and d.get("FileType") == "JPEG"),
        ("vianoce priecinok:2023", lambda d: "Vianoce" in zoznam(d.get("Keywords"))),
    ]
    for dotaz, f in dotazy:
        nase, txt = hladaj(dotaz)
        o = ocakavane(f)
        vysledok(f"Herbár: dotaz „{dotaz}“", nase == o, f"{len(nase)} nájdených = podľa ExifTool" if nase == o else f"{len(nase)} ≠ {len(o)}")
    # obnova: nič nové → nič sa nečíta; zmena jedného, zmazanie jedného
    r = herbar_cmd("obnov")
    vysledok("Herbár: obnova bez zmien", "precitanych=0 preskocenych=301 zmazanych=0" in r.stdout, r.stdout.strip())
    zmeneny = os.path.join(koren, "2024", "kopia_000.jpg")
    subprocess.run(et + ["-q", "-overwrite_original", "-IPTC:Keywords+=nové", zmeneny], check=True)
    os.remove(os.path.join(koren, "2024", "kopia_003.jpg"))
    r = herbar_cmd("obnov")
    vysledok("Herbár: obnova len zmeneného", "precitanych=1 preskocenych=299 zmazanych=1" in r.stdout, r.stdout.strip())
    nase, _ = hladaj("kluc:nove")
    vysledok("Herbár: nové kľúčové slovo nájdené", nase == [zmeneny], f"{nase}")
    # rýchlosť: katalóg so 100 000 záznamami
    with open(katalog, encoding="utf-8") as f:
        riadky = f.read().splitlines()
    zaznamy = [x for x in riadky if x.startswith("Z\t")]
    with open(os.path.join(PRAC, "herbar_velky.txt"), "w", encoding="utf-8") as f:
        f.write(riadky[0] + "\n")
        for i in range(100_000):
            z = zaznamy[i % len(zaznamy)].split("\t")
            z[1] = z[1].replace(".jpg", f"_{i}.jpg")
            f.write("\t".join(z) + "\n")
    r = subprocess.run([PROGRAM, "herbar", os.path.join(PRAC, "herbar_velky.txt"), "hladaj", "kvet", "iso>=100", "-vianoce"],
                       capture_output=True, text=True)
    riadok = [x for x in r.stdout.splitlines() if x.startswith("NAJDENYCH")]
    us = int(riadok[0].split()[-2]) if riadok else 10**9
    vysledok("Herbár: dotaz nad 100 000 záznamami", us < 200_000, riadok[0] if riadok else r.stderr[-200:])


rng = np.random.default_rng(2026)
zmensenie(rng)
davka_bez_straty(rng)
porovnanie(rng)
okno(rng)
upravy(rng)
brana(rng)
nahlady(rng)
priestor_farby(rng)
nahlady_brana()
herbar(rng)
print(f"\n{'Všetko v poriadku' if not zlyhania else 'Zlyhania: ' + ', '.join(zlyhania)}")
sys.exit(1 if zlyhania else 0)
