#!/usr/bin/env python3
"""Nezávislé overenie Pohľadu (CI): zmenšenie proti Pillow, hromadný prevod a brána E1 – 1000 fotiek bez
zamrznutia, s obmedzeným počtom vlákien a ohraničenou pamäťou.

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


def obraz(rng, h, w, c):
    y, x = np.mgrid[0:h, 0:w]
    z = np.stack([(np.sin(x / (5 + 3 * k) + y / (9 + k)) + 1) / 2 for k in range(c)], -1)
    return np.clip((z * 0.8 + rng.random((h, w, c)) * 0.2) * 255, 0, 255).astype(np.uint8)


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


rng = np.random.default_rng(2026)
zmensenie(rng)
davka_bez_straty(rng)
brana(rng)
print(f"\n{'Všetko v poriadku' if not zlyhania else 'Zlyhania: ' + ', '.join(zlyhania)}")
sys.exit(1 if zlyhania else 0)
