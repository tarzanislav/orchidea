#!/usr/bin/env python3
"""Rozhodca jadra prehrávania LÝRY (R5d, `media::prehravanie`) na skutočných súboroch – prúd D.

Použitie: over_prehravanie.py <media_prehravanie> <pracovný priečinok>

FFmpeg zapíše súbory (MKV, MP4, WebM, MPEG-TS; H.264 a HEVC so snímkami B, MPEG-2, VP9; AC-3, FLAC, MP3, MP2,
Opus, zvuk bez videa, video bez zvuku) a je nezávislým rozhodcom (ffprobe – pakety videa, FFmpeg – dekódovanie
zvuku a priechod IEC 61937). `media_prehravanie` prehrá súbor jadrom so simulovaným systémom. Podmienky:
1. pakety videa: počet a časy (relatívne k prvému, na µs) ako ffprobe,
2. každá snímka zobrazená práve raz, v poradí zobrazenia, nič vynechané; odchýlka nástupu ≤ P/2 + P/8 + 2 ms,
3. zvuk: priechod – bajty bit po bite ako `ffmpeg -f spdif`; bitovo presné PCM/FLAC – bajty ako `ffmpeg -f s16le`;
   stratové kodeky – počet rámcov ako dekódovanie FFmpeg,
4. presný posun: prvá zobrazená snímka = prvá s časom ≥ cieľ, prvá vzorka zvuku presne v cieli (± 1 vzorka).
"""

import hashlib
import os
import subprocess
import sys

PROGRAM = sys.argv[1] if len(sys.argv) > 1 else None
PRAC = sys.argv[2] if len(sys.argv) > 2 else None
CHYBY = []

OBRAZ = lambda f, d: ["-f", "lavfi", "-i", f"testsrc2=size=320x240:rate={f}:duration={d}"]  # noqa: E731
ZVUK = lambda fr, d, k=2: ["-f", "lavfi", "-i", f"sine=frequency=440:sample_rate={fr}:duration={d}", "-ac", str(k)]  # noqa: E731

# meno, prípona, argumenty FFmpeg, voľby media_prehravanie, druh kontroly zvuku, posun (s)
VARIANTY = [
    ("mkv_h264b_ac3", "mkv", OBRAZ(25, 10) + ZVUK(48000, 10) + ["-c:v", "libx264", "-bf", "2", "-g", "25", "-c:a", "ac3"],
     [], "dlzka", 5.3),
    ("mkv_h264b_ac3_priechod", "mkv", OBRAZ(25, 10) + ZVUK(48000, 10) + ["-c:v", "libx264", "-bf", "2", "-g", "25",
     "-c:a", "ac3"], ["--priechod"], "spdif", None),
    ("mkv_hevc_flac_23976", "mkv", OBRAZ("24000/1001", 10) + ZVUK(44100, 10) + ["-c:v", "libx265", "-x265-params",
     "log-level=none:keyint=24:bframes=3", "-c:a", "flac", "-sample_fmt", "s16"], [], "s16", 4.0),
    ("mp4_h264b_mp3_30", "mp4", OBRAZ(30, 10) + ZVUK(44100, 10) + ["-c:v", "libx264", "-bf", "3", "-g", "30",
     "-c:a", "libmp3lame"], [], "dlzka", 6.5),
    ("ts_h264_ac3_51", "ts", OBRAZ(25, 10) + ZVUK(48000, 10, 6) + ["-c:v", "libx264", "-bf", "2", "-g", "25",
     "-c:a", "ac3", "-b:a", "448k"], [], "dlzka", 3.1),
    ("ts_h264_ac3_51_priechod", "ts", OBRAZ(25, 10) + ZVUK(48000, 10, 6) + ["-c:v", "libx264", "-bf", "2", "-g",
     "25", "-c:a", "ac3", "-b:a", "448k"], ["--priechod"], "spdif", None),
    ("ts_mpeg2_mp2", "ts", OBRAZ(25, 10) + ZVUK(48000, 10) + ["-c:v", "mpeg2video", "-g", "12", "-c:a", "mp2"],
     [], "dlzka", None),
    ("webm_vp9_opus_30", "webm", OBRAZ(30, 10) + ZVUK(48000, 10) + ["-c:v", "libvpx-vp9", "-deadline", "realtime",
     "-g", "30", "-c:a", "libopus"], [], "dlzka", 5.0),
    ("mkv_h264_bez_zvuku", "mkv", OBRAZ(25, 6) + ["-c:v", "libx264", "-g", "25"], [], None, 2.0),
    ("flac_hudba", "flac", ZVUK(44100, 6) + ["-sample_fmt", "s16"], [], "s16", None),
    ("mkv_h264_ac3_50hz", "mkv", OBRAZ(25, 6) + ZVUK(48000, 6) + ["-c:v", "libx264", "-bf", "2", "-g", "25",
     "-c:a", "ac3"], ["--hz", "50"], "dlzka", None),
]


def spusti(argumenty):
    p = subprocess.run([PROGRAM, *argumenty], capture_output=True, text=True, timeout=900)
    riadky = {"PV": []}
    for l in p.stdout.splitlines():
        c = l.split(" ")
        polia = dict(x.split("=", 1) for x in c[1:] if "=" in x)
        if c[0] == "PV":
            riadky["PV"].append(int(polia["pts"]))
        else:
            riadky[c[0]] = polia
    return p.returncode, riadky, p.stderr.strip()[-300:]


def ffprobe_video_us(cesta):
    r = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time",
                        "-of", "csv=p=0", cesta], capture_output=True, text=True, check=True)
    t = [round(float(x) * 1e6) for x in (y.strip(",") for y in r.stdout.split()) if x and x != "N/A"]
    return t


def ffmpeg_bajty(cesta, *argumenty):
    r = subprocess.run(["ffmpeg", "-v", "error", "-i", cesta, "-map", "0:a:0", *argumenty, "-"], capture_output=True,
                       check=True)
    return r.stdout


def kanaly_ffmpeg(cesta):
    r = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "a:0", "-show_entries", "stream=channels",
                        "-of", "csv=p=0", cesta], capture_output=True, text=True, check=True)
    return int(r.stdout.split()[0].strip(","))


def relativne(us):
    if not us:
        return []
    z = min(us)
    return sorted(x - z for x in us)


def over(meno, pripona, argumenty, volby, zvuk, posun):
    cesta = os.path.join(PRAC, f"{meno}.{pripona}")
    r = subprocess.run(["ffmpeg", "-v", "error", "-y", *argumenty, cesta], capture_output=True, text=True)
    if r.returncode != 0:
        print(f"| {meno} | ⏭ zapisovač FFmpeg nevie: {r.stderr.strip()[-120:]} |||||")
        return
    kod, v, chyba = spusti([*volby, cesta])
    nezhody = []
    if kod != 0 or v.get("VYSLEDOK", {}).get("stav") != "ok":
        nezhody.append(f"stav {kod} {chyba}")
    p = v.get("PREHRAVANIE", {})
    s = v.get("SNIMKY", {})
    hz = float(volby[volby.index("--hz") + 1]) if "--hz" in volby else 60.0
    hranica_us = (1e6 / hz) * (0.5 + 0.125) + 2000
    # 1. pakety videa
    if p.get("video", "-") != "-":
        ocak = relativne(ffprobe_video_us(cesta))
        nase = relativne([round(x / 1000) for x in v["PV"]])
        if len(ocak) != len(nase):
            nezhody.append(f"paketov videa {len(nase)}, ffprobe {len(ocak)}")
        elif any(abs(a - b) > 1 for a, b in zip(ocak, nase)):
            nezhody.append("časy paketov videa sa líšia od ffprobe")
        # 2. snímky
        if int(s.get("zobrazenych", -1)) != len(nase) or s.get("vynechanych") != "0" or s.get("poradie") != "ok":
            nezhody.append(f"snímky {s}")
        if int(s.get("odchylka_max_us", 10**9)) > hranica_us:
            nezhody.append(f"odchýlka {s.get('odchylka_max_us')} µs > {hranica_us:.0f}")
    # 3. zvuk
    z = v.get("ZVUK", {})
    zvuk_stav = "–"
    if zvuk == "spdif":
        ref = ffmpeg_bajty(cesta, "-c", "copy", "-f", "spdif")
        ok = z.get("md5") == hashlib.md5(ref).hexdigest()
        zvuk_stav = f"priechod {len(ref)} B {'bit po bite' if ok else 'NEZHODA'}"
        if not ok:
            nezhody.append(f"priechod {z.get('bajtov')} B, ffmpeg {len(ref)} B")
    elif zvuk == "s16":
        ref = ffmpeg_bajty(cesta, "-f", "s16le")
        ok = z.get("md5") == hashlib.md5(ref).hexdigest()
        zvuk_stav = f"PCM {len(ref)} B {'bit po bite' if ok else 'NEZHODA'}"
        if not ok:
            nezhody.append(f"PCM {z.get('bajtov')} B, ffmpeg {len(ref)} B")
    elif zvuk == "dlzka":
        k = kanaly_ffmpeg(cesta)
        ref = len(ffmpeg_bajty(cesta, "-f", "s16le")) // (2 * k)
        ok = z.get("ramcov") == str(ref)
        zvuk_stav = f"{p.get('cesta')} {ref} rámcov {'ako FFmpeg' if ok else 'NEZHODA'}"
        if not ok:
            nezhody.append(f"zvuk {z.get('ramcov')} rámcov, FFmpeg {ref}")
    # 4. presný posun
    posun_stav = "–"
    if posun is not None:
        k2, v2, ch2 = spusti([*volby, "--posun", str(posun), cesta])
        q = v2.get("POSUN", {})
        ciel = int(posun * 1e9)
        ocak_snimka = min((t for t in v["PV"] if t >= ciel), default=None) if p.get("video", "-") != "-" else None
        snimka = None if q.get("prva_snimka_pts", "-") == "-" else int(q["prva_snimka_pts"])
        zv = None if q.get("prvy_zvuk_pts", "-") == "-" else int(q["prvy_zvuk_pts"])
        f = int(p.get("frekvencia", "48000")) if p.get("frekvencia", "-") != "-" else 48000
        ok = k2 == 0 and snimka == ocak_snimka and (zvuk is None or (zv is not None and abs(zv - ciel) <= 1e9 / f))
        posun_stav = f"snímka {snimka / 1e9 if snimka else '-'} s, zvuk {zv / 1e9 if zv else '-'} s {'✅' if ok else '❌'}"
        if not ok:
            nezhody.append(f"posun na {posun} s: snímka {snimka} (čakaná {ocak_snimka}), zvuk {zv} {ch2}")
    stav = "✅" if not nezhody else "❌ " + "; ".join(nezhody[:4])
    if nezhody:
        CHYBY.append(f"{meno}: " + "; ".join(nezhody[:4]))
    print(f"| {meno} | {p.get('video', '-')} + {p.get('zvuk', '-')} | {s.get('zobrazenych', '-')}/{len(v['PV'])} "
          f"| {s.get('odchylka_max_us', '-')} µs | {zvuk_stav} | {posun_stav} | {stav} |")


def main():
    if not PROGRAM or not PRAC:
        print(__doc__)
        sys.exit(2)
    os.makedirs(PRAC, exist_ok=True)
    print("| súbor | video + zvuk | snímky zobrazené / pakety | odchýlka max | zvuk | presný posun | výsledok |")
    print("|---|---|---|---|---|---|---|")
    for varianta in VARIANTY:
        over(*varianta)
    print()
    print("Simulovaný systém: dekodér vracia snímky v poradí zobrazenia (snímky B), zvuk +120 ppm, displej −60 ppm.")
    if CHYBY:
        print("\n**Nezhody:**\n")
        for c in CHYBY:
            print(f"- {c}")
        sys.exit(1)
    print("✅ všetko sedí")


if __name__ == "__main__":
    main()
