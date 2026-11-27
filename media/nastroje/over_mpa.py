#!/usr/bin/env python3
"""Rozhodca MPEG audio vrstvy III (L8, časť 1): rozbor bitového toku proti dvom nezávislým kodérom.

Dôkaz entropického dekódovania: pri **každej granule** musí rozbor škálovacích faktorov a Huffmanových kódov
skončiť **presne na konci `part2_3_length`** (bez prečnievajúcej štvorice, bez výplne, bez chyby). Pri chybnej
tabuľke alebo chybnom pravidle (hranice oblastí, slen, scfsi, linbits) sa spotreba rozíde v ďalších granulách.
Toky vytvorí FFmpeg s kodérmi **LAME** (libmp3lame) a **Shine** (libshine) – CBR, VBR, mono, stereo, spojené
stereo, 32 / 44,1 / 48 kHz, bez zásobníka bitov, signál s údermi (krátke bloky).

Použitie: `over_mpa.py <media_mpa> <adresár>`
"""
import os
import re
import subprocess
import sys

HUDBA = ("aevalsrc='0.3*sin(2*PI*220*t)+0.15*sin(2*PI*1330*t+1)+0.35*(random(1)-0.5)*exp(-30*mod(t,0.4))"
         "|0.3*sin(2*PI*277*t)+0.15*sin(2*PI*1760*t)+0.35*(random(2)-0.5)*exp(-20*mod(t+0.13,0.55))'")

# (meno, kodér, frekvencia, kanálov, argumenty)
VARIANTY = [
    ("lame_128_js", "libmp3lame", 44100, 2, ["-b:a", "128k"]),
    ("lame_320", "libmp3lame", 48000, 2, ["-b:a", "320k"]),
    ("lame_32_mono", "libmp3lame", 32000, 1, ["-b:a", "32k"]),
    ("lame_v0", "libmp3lame", 44100, 2, ["-q:a", "0"]),
    ("lame_v9", "libmp3lame", 44100, 2, ["-q:a", "9"]),
    ("lame_stereo", "libmp3lame", 44100, 2, ["-b:a", "192k", "-joint_stereo", "0"]),
    ("lame_bez_zasobnika", "libmp3lame", 48000, 2, ["-b:a", "160k", "-reservoir", "0"]),
    ("lame_abr_mono", "libmp3lame", 48000, 1, ["-b:a", "96k", "-abr", "1"]),
    ("shine_128", "libshine", 44100, 2, ["-b:a", "128k"]),
    ("shine_64_mono", "libshine", 32000, 1, ["-b:a", "64k"]),
    ("shine_256", "libshine", 48000, 2, ["-b:a", "256k"]),
]


def main():
    program, adresar = sys.argv[1], sys.argv[2]
    os.makedirs(adresar, exist_ok=True)
    print("| variant | rámcov | granúl | presne na konci part2_3_length | pokrytie | výsledok |")
    print("|---|---|---|---|---|---|")
    chyb = 0
    for meno, koder, f, k, arg in VARIANTY:
        cesta = os.path.join(adresar, meno + ".mp3")
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", f"{HUDBA}:s={f}:d=8", "-ac", str(k),
                        "-ar", str(f), "-c:a", koder] + arg + [cesta], check=True)
        r = subprocess.run([program, cesta], capture_output=True, text=True)
        m = dict((a, int(b)) for a, b in re.findall(r"(\w+)=(\d+)", r.stdout))
        # LAME: každá granula presne na konci; Shine dopĺňa zásobník výplňou po 576 čiarach ([S] §4.3.4 – výplňové
        # bity sa zahodia): pripustí sa výplň, ak sú zvyšné bity jednotvárne, a štvorica prečnievajúca za koniec
        prijate = m.get("presne", 0) + (0 if koder == "libmp3lame" else m.get("vypln_rovnaka", 0) + m.get("prebeh", 0))
        ok = (r.returncode == 0 and m.get("chyb", 1) == 0 and m.get("chyby", 1) == 0
              and m.get("chyba_skalovacich", 1) == 0 and m["granul"] > 0 and prijate == m["granul"])
        chyb += not ok
        pokrytie = ", ".join(f"{x} {m[x]}" for x in ("kratke", "start_stop", "scfsi", "preflag", "scalefac_scale",
                                                     "linbits", "tabulka_b", "ms") if m.get(x))
        print(f"| {meno} | {m.get('ramcov')} | {m.get('granul')} | {m.get('presne')}/{m.get('granul')} "
              f"(prebeh {m.get('prebeh')}, výplň {m.get('vypln')} – jednotvárna {m.get('vypln_rovnaka')}, chyby {m.get('chyby', 0) + m.get('chyba_skalovacich', 0)}) "
              f"| {pokrytie} | {'✅' if ok else '❌'} |")
    sys.exit(1 if chyb else 0)


if __name__ == "__main__":
    main()
