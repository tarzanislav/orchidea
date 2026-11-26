#!/usr/bin/env python3
"""Nezávislé overenie čítačiek kontajnerov ORCHIDEE (balík media) – len v CI, nie súčasť programu.

Rozhodcami sú cudzie nástroje: FFmpeg vytvorí skúšobné súbory rôznymi kodekmi a nastaveniami zapisovača,
MKVToolNix (mkvmerge, ak je) ich prebalí iným zapisovačom (lacing, iné rozloženie), ffprobe vypíše pakety
(čas, trvanie, kľúčovosť, veľkosť, MD5 obsahu). Náš program `media_pakety` musí dať bit po bite tie isté pakety.
Jediná výnimka je trvanie posledného paketu stopy: RFC 9559 ho udáva prvkom BlockDuration (skutočné trvanie
posledného bloku), ffprobe tam hlási celú dĺžku rámca – rozdiel sa vypíše, nie je chybou.

Použitie: over_media.py <cesta k media_pakety> <pracovný priečinok>
Návratový kód 0 = všetko sedí. Výsledok je tabuľka v Markdowne (pre súhrn CI).
"""

import json
import os
import shutil
import subprocess
import sys

PROGRAM = sys.argv[1]
PRAC = sys.argv[2]
os.makedirs(PRAC, exist_ok=True)
CHYBY = []
RIADKY = []

OBRAZ = ["-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=3"]
ZVUK = ["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=3", "-ac", "2"]
ZVUK44 = ["-f", "lavfi", "-i", "sine=frequency=1000:sample_rate=44100:duration=3"]

# meno, prípona, argumenty FFmpeg (bez vstupu a výstupu)
VARIANTY = [
    ("h264_flac", "mkv", OBRAZ + ZVUK + ["-c:v", "libx264", "-g", "12", "-bf", "2", "-c:a", "flac"]),
    ("vp9_opus", "webm", OBRAZ + ZVUK + ["-c:v", "libvpx-vp9", "-g", "10", "-deadline", "realtime", "-c:a", "libopus"]),
    ("hevc_ac3", "mkv", OBRAZ + ZVUK + ["-c:v", "libx265", "-x265-params", "log-level=none:keyint=10", "-c:a", "ac3"]),
    ("mpeg2_mp2", "mkv", OBRAZ + ZVUK + ["-c:v", "mpeg2video", "-c:a", "mp2"]),
    ("vp8_vorbis", "webm", OBRAZ + ZVUK + ["-c:v", "libvpx", "-deadline", "realtime", "-c:a", "libvorbis"]),
    ("pcm16_mp3", "mkv", ZVUK44 + ZVUK + ["-map", "0", "-map", "1", "-c:a:0", "pcm_s16le", "-c:a:1", "libmp3lame"]),
    ("pcm24_f32", "mkv", ZVUK + ZVUK44 + ["-map", "0", "-map", "1", "-c:a:0", "pcm_s24le", "-c:a:1", "pcm_f32le"]),
    ("zivy_webm", "webm", OBRAZ + ZVUK + ["-c:v", "libvpx-vp9", "-deadline", "realtime", "-c:a", "libopus", "-live", "1"]),
    ("male_zhluky", "mkv", OBRAZ + ZVUK + ["-c:v", "libx264", "-c:a", "flac", "-cluster_time_limit", "100", "-cluster_size_limit", "20000"]),
    # dlhší súbor (≈ 20 MB) – čítanie ide cez viac dávok po 4 MiB
    ("dlhy_h264_opus", "mkv", ["-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30:duration=40"] + ZVUK[:3]
     + ["sine=frequency=440:sample_rate=48000:duration=40", "-ac", "2", "-c:v", "libx264", "-preset", "ultrafast",
        "-b:v", "4M", "-c:a", "libopus"]),
    ("av1_opus", "mkv", OBRAZ + ZVUK + ["-c:v", "libaom-av1", "-cpu-used", "8", "-g", "10", "-c:a", "libopus"]),
]


def ffmpeg(vystup, argumenty):
    r = subprocess.run(["ffmpeg", "-v", "error", "-y"] + argumenty + [vystup], capture_output=True, text=True)
    return r.returncode == 0, r.stderr.strip()[-300:]


def rozhodca(cesta):
    r = subprocess.run(["ffprobe", "-v", "error", "-show_packets", "-show_data_hash", "MD5", "-of", "json", cesta],
                       capture_output=True, text=True, check=True)
    pakety = []
    for p in json.loads(r.stdout).get("packets", []):
        pakety.append({
            "stopa": p["stream_index"],
            "pts": p.get("pts"),
            "trvanie": p.get("duration"),
            "kluc": "K" in p.get("flags", ""),
            "velkost": int(p["size"]),
            "md5": p.get("data_hash", "").split(":")[-1],
        })
    return pakety


def nas(cesta):
    r = subprocess.run([PROGRAM, cesta], capture_output=True, text=True)
    pakety, stopy, citani = [], 0, None
    for riadok in r.stdout.splitlines():
        slova = riadok.split()
        if not slova:
            continue
        pole = dict(s.split("=", 1) for s in slova[1:] if "=" in s)
        if slova[0] == "STOPA":
            stopy += 1
        elif slova[0] == "PAKET":
            cislo = lambda k: None if pole[k] == "-" else int(pole[k])
            pakety.append({"stopa": int(pole["stopa"]), "pts": cislo("pts"), "trvanie": cislo("trvanie"),
                           "kluc": pole["kluc"] == "1", "velkost": int(pole["velkost"]), "md5": pole["md5"]})
        elif slova[0] == "VYSLEDOK":
            citani = pole.get("citani")
        elif slova[0] == "CHYBA":
            raise RuntimeError(riadok)
    if r.returncode != 0:
        raise RuntimeError(f"návratový kód {r.returncode}: {r.stderr.strip()}")
    return pakety, stopy, citani


def porovnaj(meno, cesta):
    try:
        ocak = rozhodca(cesta)
        moje, stopy, citani = nas(cesta)
    except Exception as e:  # noqa: BLE001 – každá chyba je výsledok skúšky
        CHYBY.append(f"{meno}: {e}")
        RIADKY.append(f"| {meno} | – | – | ❌ {e} |")
        return
    nezhody = []
    poznamky = []
    # posledný paket každej stopy: zapisovač dá poslednému bloku BlockDuration (skutočné trvanie, RFC 9559),
    # ffprobe udáva celú dĺžku rámca – pri ňom sa trvanie neporovná, len sa zapíše
    posledny = {}
    for i, b in enumerate(ocak):
        posledny[b["stopa"]] = i
    if len(ocak) != len(moje):
        nezhody.append(f"paketov {len(moje)}, rozhodca {len(ocak)}")
    for i, (a, b) in enumerate(zip(moje, ocak)):
        for k in ("stopa", "pts", "kluc", "velkost", "md5"):
            if a[k] != b[k]:
                nezhody.append(f"paket {i} {k}: náš {a[k]}, rozhodca {b[k]}")
        if a["trvanie"] is not None and b["trvanie"] is not None and a["trvanie"] != b["trvanie"]:
            if posledny.get(b["stopa"]) == i:
                poznamky.append(f"posledný paket stopy {b['stopa']}: BlockDuration {a['trvanie']}, rámec {b['trvanie']}")
            else:
                nezhody.append(f"paket {i} trvanie: náš {a['trvanie']}, rozhodca {b['trvanie']}")
        if len(nezhody) > 5:
            break
    stav = "✅ bit po bite" if not nezhody else "❌ " + "; ".join(nezhody[:5])
    if poznamky:
        stav += " – " + "; ".join(poznamky)
    if nezhody:
        CHYBY.append(f"{meno}: " + "; ".join(nezhody[:5]))
    RIADKY.append(f"| {meno} | {stopy} | {len(moje)} | {stav} (čítaní zo zdroja: {citani}) |")


def main():
    mkvmerge = shutil.which("mkvmerge")
    for meno, pripona, argumenty in VARIANTY:
        cesta = os.path.join(PRAC, f"{meno}.{pripona}")
        ok, chyba = ffmpeg(cesta, argumenty)
        if not ok:
            RIADKY.append(f"| {meno} | – | – | ⏭ zapisovač FFmpeg nevie ({chyba}) |")
            continue
        porovnaj(meno, cesta)
        if mkvmerge and pripona == "mkv":
            druhy = os.path.join(PRAC, f"{meno}_mkvmerge.mkv")
            r = subprocess.run([mkvmerge, "-q", "-o", druhy, cesta], capture_output=True, text=True)
            if r.returncode in (0, 1):
                porovnaj(f"{meno} (mkvmerge)", druhy)
    print("| Súbor | Stôp | Paketov | Zhoda s rozhodcom (ffprobe) |")
    print("|---|---|---|---|")
    print("\n".join(RIADKY))
    if CHYBY:
        print("\n**Nezhody:**\n")
        for c in CHYBY:
            print(f"- {c}")
        sys.exit(1)


main()
