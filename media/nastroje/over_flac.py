#!/usr/bin/env python3
"""Nezávislé overenie dekodéra FLAC ORCHIDEE (RFC 9639, etapa L2) – len v CI, nie súčasť programu.

Dôkaz je bitová zhoda: MD5 dekódovaného zvuku sa musí rovnať MD5 v STREAMINFO, ktoré tam zapísal cudzí kodér.
1. sada IETF CELLAR flac-test-files (CC0): `subset` – všetky zhoda; `uncommon` – 05–09 zhoda, 10 a 11 (bez MD5)
   bajt po bajte ako dekodér FFmpeg, 01–04 (meniace sa parametre, žiadny rozhodca ich nedekóduje) bez chyby;
   `faulty` – nikdy pád ani zacyklenie, súbory s platnými rámcami zhoda,
2. súbory z referenčného kodéra `flac` (úrovne 0–8, rád LPC 32, bloky 192–16384, 8/16/24/32 bitov, 1–8 kanálov),
3. súbory z kodéra FFmpeg (spôsoby kanálov, rády oddielov) aj v kontajneri Matroska.

Použitie: over_flac.py <media_flac> <sada flac-test-files> <pracovný priečinok>
"""

import os
import subprocess
import sys

PROGRAM, SADA, PRAC = sys.argv[1], sys.argv[2], sys.argv[3]
os.makedirs(PRAC, exist_ok=True)
CHYBY, RIADKY = [], []


def nas(cesta, surove=None):
    argumenty = [PROGRAM] + (["--surove", surove] if surove else []) + [cesta]
    try:
        r = subprocess.run(argumenty, capture_output=True, text=True, timeout=120)
    except subprocess.TimeoutExpired:
        return {"stav": "zacyklenie"}, -9
    pole = {}
    for slovo in r.stdout.split():
        if "=" in slovo:
            k, v = slovo.split("=", 1)
            pole[k] = v
    if r.returncode not in (0, 1) or "panicked" in r.stderr:
        pole["stav"] = "pad"
    return pole, r.returncode


def zapis(skupina, meno, ok, text):
    RIADKY.append(f"| {skupina} | {meno} | {'✅' if ok else '❌'} {text} |")
    if not ok:
        CHYBY.append(f"{skupina}/{meno}: {text}")


def popis(p):
    return f"{p.get('stav')} · rámcov {p.get('ramcov', '-')} · {p.get('x_realny_cas', '-')}× reálny čas"


def ffmpeg_surove(cesta, bitov):
    vystup = os.path.join(PRAC, "ffmpeg.raw")
    fmt = "s16le" if int(bitov) <= 16 else "s32le"
    if int(bitov) <= 8:
        fmt = "s16le"
    r = subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", cesta, "-f", fmt, vystup], capture_output=True)
    return vystup if r.returncode == 0 else None


def rovnake_s_ffmpeg(cesta):
    """Náš surový výstup (⌈bitov/8⌉ B na vzorku) proti FFmpeg (s16le) – len pre 16-bitové súbory."""
    moje = os.path.join(PRAC, "nas.raw")
    p, _ = nas(cesta, moje)
    if p.get("bitov") != "16":
        return p, None
    ref = ffmpeg_surove(cesta, 16)
    if not ref:
        return p, None
    return p, open(moje, "rb").read() == open(ref, "rb").read()


def sada():
    for f in sorted(os.listdir(os.path.join(SADA, "subset"))):
        if f.endswith(".flac"):
            p, _ = nas(os.path.join(SADA, "subset", f))
            zapis("IETF subset", f, p.get("stav") == "zhoda", popis(p))
    for f in sorted(os.listdir(os.path.join(SADA, "uncommon"))):
        if not f.endswith(".flac"):
            continue
        cesta = os.path.join(SADA, "uncommon", f)
        cislo = int(f[:2])
        if cislo in (10, 11):
            p, rovnake = rovnake_s_ffmpeg(cesta)
            zapis("IETF uncommon", f, rovnake is True, popis(p) + " · bajt po bajte ako FFmpeg" * bool(rovnake))
        elif cislo <= 4:
            p, kod = nas(cesta)
            zapis("IETF uncommon", f, kod == 0 and p.get("stav") in ("bez_md5", "zhoda"), popis(p) + " (bez rozhodcu)")
        else:
            p, _ = nas(cesta)
            zapis("IETF uncommon", f, p.get("stav") == "zhoda", popis(p))
    zhoda_ocak = {1, 3, 4, 5, 7, 9, 10}
    for f in sorted(os.listdir(os.path.join(SADA, "faulty"))):
        if not f.endswith(".flac"):
            continue
        p, kod = nas(os.path.join(SADA, "faulty", f))
        ok = p.get("stav") not in ("pad", "zacyklenie") and (int(f[:2]) not in zhoda_ocak or p.get("stav") == "zhoda")
        zapis("IETF faulty", f, ok, popis(p))


def vstup(meno, kanalov, frekvencia, fmt, sekund=4):
    """WAV z FFmpeg: tóny a šum v každom kanáli (iný signál na kanál – stereo dekorelácia sa naozaj použije)."""
    cesta = os.path.join(PRAC, meno + ".wav")
    vyraz = "|".join(f"0.4*sin({220 * (c + 1)}*2*PI*t)+0.05*random({c})" for c in range(kanalov))
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i",
                    f"aevalsrc={vyraz}:s={frekvencia}:d={sekund}", "-c:a", fmt, cesta], check=True)
    return cesta


def kodery():
    wav16 = vstup("v16", 2, 44100, "pcm_s16le")
    wav24 = vstup("v24", 2, 96000, "pcm_s24le")
    wav8 = vstup("v8", 1, 22050, "pcm_u8")
    wav32 = vstup("v32", 2, 48000, "pcm_s32le")
    wav6 = vstup("v6", 6, 48000, "pcm_s24le")
    wav8k = vstup("v8k", 8, 48000, "pcm_s16le")
    varianty = [(f"-{u}", wav16, [f"-{u}"]) for u in range(9)] + [
        ("24 b 96 kHz -8", wav24, ["-8"]),
        ("8 b mono", wav8, ["-5"]),
        ("32 b", wav32, ["-5"]),
        ("5.1 24 b", wav6, ["-8"]),
        ("7.1", wav8k, ["-5"]),
        ("LPC 32, --lax", wav16, ["--lax", "-l", "32", "-r", "15"]),
        ("blok 192", wav16, ["--lax", "-b", "192"]),
        ("blok 16384", wav16, ["--lax", "-b", "16384"]),
        ("presné Rice, -e -p", wav24, ["-8", "-e", "-p"]),
    ]
    for meno, wav, volby in varianty:
        vystup = os.path.join(PRAC, "flac_" + meno.replace(" ", "_").replace(",", "") + ".flac")
        r = subprocess.run(["flac", "-s", "-f", "-o", vystup] + volby + [wav], capture_output=True, text=True)
        if r.returncode != 0:
            zapis("kodér flac", meno, False, "kodér zlyhal: " + r.stderr.strip()[-120:])
            continue
        p, _ = nas(vystup)
        zapis("kodér flac", meno, p.get("stav") == "zhoda", popis(p))
    ff = [
        ("úroveň 12", wav16, ["-compression_level", "12"], "flac"),
        ("nezávislé kanály", wav16, ["-ch_mode", "indep"], "flac"),
        ("ľavý/bočný", wav16, ["-ch_mode", "left_side"], "flac"),
        ("bočný/pravý", wav16, ["-ch_mode", "right_side"], "flac"),
        ("stredný/bočný", wav16, ["-ch_mode", "mid_side"], "flac"),
        ("pevný predpovedač", wav16, ["-lpc_type", "fixed"], "flac"),
        ("Cholesky", wav24, ["-lpc_type", "cholesky", "-lpc_passes", "3"], "flac"),
        ("oddiely 0–8", wav16, ["-min_partition_order", "0", "-max_partition_order", "8"], "flac"),
        ("v Matroske", wav24, [], "mkv"),
        ("v Matroske 5.1", wav6, [], "mkv"),
    ]
    for meno, wav, volby, pripona in ff:
        vystup = os.path.join(PRAC, "ffmpeg_" + meno.replace(" ", "_").replace("/", "_") + "." + pripona)
        r = subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", wav, "-c:a", "flac"] + volby + [vystup],
                           capture_output=True, text=True)
        if r.returncode != 0:
            zapis("kodér FFmpeg", meno, False, "kodér zlyhal: " + r.stderr.strip()[-120:])
            continue
        p, _ = nas(vystup)
        zapis("kodér FFmpeg", meno, p.get("stav") == "zhoda", popis(p))


sada()
kodery()
print("| Skupina | Súbor | Výsledok (MD5 v STREAMINFO = MD5 nášho dekódovania) |")
print("|---|---|---|")
print("\n".join(RIADKY))
if CHYBY:
    print("\n**Chyby:**\n")
    for c in CHYBY:
        print(f"- {c}")
    sys.exit(1)
