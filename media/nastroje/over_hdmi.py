#!/usr/bin/env python3
"""Rozhodca prevodníka zvuku pre HDMI (R5b, etapa L5).

Pre každý variant vyrobí FFmpeg súbor, `media_hdmi` z neho urobí bajty pre ovládač HDMI a ich MD5 sa porovná
s nezávisle získaným očakávaním:

- **priechod AC-3 / E-AC-3**: `ffmpeg -c copy -f spdif` (IEC 61937) – bit po bite; pri formáte podrámcov
  IEC 60958 (32-bitové slová) sa 16-bitové slová FFmpeg posunú o 16 bitov nahor,
- **bitovo presné PCM**: FFmpeg dekóduje do `s32le`/`s16le` v poradí WAVE a tento skript preusporiada kanály
  do slotov CTA-861 podľa vlastnej tabuľky (nie podľa výstupu programu): 5.1 → FL FR LFE FC RL RR, 7.1 →
  FL FR LFE FC SL SR BL BR, 3.0 → FL FR (ticho) FC, mono → FL = FR,
- **zúženie 24 → 16 bitov**: očakávanie z dekódovania FFmpeg a pravidla „polovicou nahor, orezanie“ v Pythone.

Použitie: `over_hdmi.py <media_hdmi> <pracovný adresár>` – na stdout tabuľka Markdown; návratový kód 0 = všetko
sedí.
"""

import hashlib
import os
import struct
import subprocess
import sys


def tonov(n, frekvencia=48000, sekund=2):
    vyrazy = "|".join(f"0.{3 + k % 6}*sin({300 + 110 * k}*2*PI*t)" for k in range(n))
    return ["-f", "lavfi", "-i", f"aevalsrc={vyrazy}:s={frekvencia}:d={sekund}"]


# (meno, prípona, argumenty FFmpeg, argumenty media_hdmi, druh očakávania, sloty CTA alebo None)
VARIANTY = [
    ("ac3_51_mkv", "mkv", tonov(6) + ["-c:a", "ac3", "-ac", "6"], ["--format", "s16"], "spdif16", None),
    ("ac3_20_44k_ts", "ts", tonov(2, 44100) + ["-c:a", "ac3"], ["--format", "s16"], "spdif16", None),
    ("ac3_51_iec958", "mkv", tonov(6) + ["-c:a", "ac3", "-b:a", "640k"], ["--format", "iec958"], "spdif32", None),
    ("eac3_20_mkv", "mkv", tonov(2) + ["-c:a", "eac3"], ["--format", "s16"], "spdif16", None),
    ("eac3_51_mp4", "mp4", tonov(6) + ["-c:a", "eac3"], ["--format", "s32"], "spdif16", None),
    ("flac_51_24b", "flac", tonov(6) + ["-c:a", "flac", "-sample_fmt", "s32", "-bits_per_raw_sample", "24"],
     ["--format", "s32"], "s32", [0, 1, 3, 2, 4, 5]),
    ("flac_71_24b_mkv", "mkv", tonov(8) + ["-c:a", "flac", "-sample_fmt", "s32", "-bits_per_raw_sample", "24"],
     ["--format", "s32"], "s32", [0, 1, 3, 2, 6, 7, 4, 5]),
    ("flac_30_16b", "flac", tonov(3) + ["-c:a", "flac", "-sample_fmt", "s16"], ["--format", "s16"], "s16", [0, 1, None, 2]),
    ("flac_mono_16b", "flac", tonov(1) + ["-c:a", "flac", "-sample_fmt", "s16"], ["--format", "s16"], "s16", [0, 0]),
    ("flac_20_16b_iec958", "flac", tonov(2, 44100) + ["-c:a", "flac", "-sample_fmt", "s16"], ["--format", "iec958"], "s32", [0, 1]),
    ("pcm_20_24b_mkv", "mkv", tonov(2, 96000) + ["-c:a", "pcm_s24le"], ["--format", "s32"], "s32", [0, 1]),
    ("flac_20_24b_na_16", "flac", tonov(2) + ["-c:a", "flac", "-sample_fmt", "s32", "-bits_per_raw_sample", "24"],
     ["--format", "s16", "--bitov", "16"], "zuzenie16", [0, 1]),
]


def ffmpeg_bajty(cesta, format_):
    return subprocess.run(["ffmpeg", "-v", "error", "-i", cesta, "-map", "0:a:0", "-f", format_,
                           "-c:a", "pcm_" + format_, "-"], check=True, capture_output=True).stdout


def ocakavanie(cesta, druh, sloty, kanalov):
    if druh in ("spdif16", "spdif32"):
        d = subprocess.run(["ffmpeg", "-v", "error", "-i", cesta, "-map", "0:a:0", "-c", "copy", "-f", "spdif", "-"],
                           check=True, capture_output=True).stdout
        if druh == "spdif32":
            slova = struct.unpack(f"<{len(d) // 2}H", d)
            d = struct.pack(f"<{len(slova)}I", *(w << 16 for w in slova))
        return d
    velkost, fmt = (2, "h") if druh == "s16" else (4, "i")
    d = ffmpeg_bajty(cesta, "s16le" if druh == "s16" else "s32le")
    v = struct.unpack(f"<{len(d) // velkost}{fmt}", d)
    ramcov = len(v) // kanalov
    vystup = []
    for i in range(ramcov):
        r = v[i * kanalov:(i + 1) * kanalov]
        for s in sloty:
            x = 0 if s is None else r[s]
            if druh == "zuzenie16":
                # 24 bitov v hornej časti s32 → 16 bitov: ⌊v / 2^8 + ½⌋, orezanie
                x = max(-32768, min(32767, ((x >> 8) + 128) >> 8))
            vystup.append(x)
    if druh == "zuzenie16":
        return struct.pack(f"<{len(vystup)}h", *vystup)
    return struct.pack(f"<{len(vystup)}{fmt}", *vystup)


def kanalov_suboru(cesta):
    r = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "a:0", "-show_entries", "stream=channels",
                        "-of", "csv=p=0", cesta], check=True, capture_output=True, text=True).stdout
    return int(r.split()[0].strip(","))


def main():
    program, adresar = sys.argv[1], sys.argv[2]
    os.makedirs(adresar, exist_ok=True)
    print("| variant | cesta | nastavenie | bajtov | výsledok |")
    print("|---|---|---|---|---|")
    chyb = 0
    for meno, pripona, ff, argumenty, druh, sloty in VARIANTY:
        cesta = os.path.join(adresar, f"{meno}.{pripona}")
        subprocess.run(["ffmpeg", "-v", "error", "-y"] + ff + [cesta], check=True)
        r = subprocess.run([program] + argumenty + [cesta], capture_output=True, text=True)
        riadky = {x.split(" ", 1)[0]: dict(p.split("=", 1) for p in x.split(" ")[1:] if "=" in p)
                  for x in r.stdout.splitlines() if x}
        if r.returncode != 0 or "VYSLEDOK" not in riadky:
            print(f"| {meno} | – | – | – | ❌ {r.stdout.strip()} {r.stderr.strip()[:200]} |")
            chyb += 1
            continue
        n, v = riadky["NASTAVENIE"], riadky["VYSLEDOK"]
        ocak = ocakavanie(cesta, druh, sloty, kanalov_suboru(cesta))
        md5 = hashlib.md5(ocak).hexdigest()
        ok = md5 == v["md5"] and int(v["bajtov"]) == len(ocak)
        chyb += not ok
        nast = f"{n['format']} {n['frekvencia']} Hz {n['kanalov']} k CA {n['ca']} stav {n['stav']}"
        vysledok = "✅ bit po bite" if ok else f"❌ md5 {v['md5']} ≠ {md5}, {v['bajtov']} ≠ {len(ocak)} B"
        print(f"| {meno} | {n['cesta']} | {nast} | {v['bajtov']} | {vysledok} |")
    sys.exit(1 if chyb else 0)


if __name__ == "__main__":
    main()
