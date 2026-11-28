#!/usr/bin/env python3
"""Skúšobné súbory pre samomeranie `media_meranie` (R6, Z14) – prúd D.

Použitie: meranie_subory.py <cieľový priečinok> [sekúnd]

FFmpeg tu len **zapisuje** (kodéry); dekóduje vlastný kód ORCHIDEE. Signál je syntetický a určený (rovnaké semienko
šumu, rovnaké výrazy) – každý beh CI dá ten istý zvuk; súbory z rôznych verzií FFmpeg sa môžu líšiť v bitoch, preto
odtlačok výstupu porovnáva CI vždy s binárkou z toho istého behu.

Obsah (dĺžka predvolene 30 s; ceny kodekov sú typické pre hudbu a film):
- hudba.flac  – FLAC 44,1 kHz, 16 b, stereo (úroveň 5)
- hudba.opus  – Opus 48 kHz stereo 128 kb/s (Ogg)
- hudba.mp3   – MP3 44,1 kHz stereo 192 kb/s CBR (LAME)
- hudba.mp2   – MP2 48 kHz stereo 256 kb/s (vysielanie DVB)
- film51.mkv  – AC-3 5.1 48 kHz 448 kb/s v Matroske
"""

import os
import subprocess
import sys

# „hudba“: akordy s pomalou obálkou, vibrato, krátke údery (prechodové javy – krátke bloky) a ružový šum
STEREO = (
    "0.22*sin(2*PI*220*t)*(0.6+0.4*sin(2*PI*0.5*t))+0.15*sin(2*PI*330*t+2*sin(2*PI*5*t))"
    "+0.10*sin(2*PI*1760*t)*lt(mod(t,0.5),0.04)+0.06*sin(2*PI*5000*t)*lt(mod(t+0.25,1),0.02)"
    "|0.22*sin(2*PI*277.2*t)*(0.6+0.4*sin(2*PI*0.37*t))+0.15*sin(2*PI*415.3*t+2*sin(2*PI*4*t))"
    "+0.10*sin(2*PI*1318.5*t)*lt(mod(t+0.1,0.5),0.04)+0.06*sin(2*PI*7000*t)*lt(mod(t+0.6,1),0.02)"
)


def kanal(f, f2, faza):
    return (
        f"0.20*sin(2*PI*{f}*t)*(0.6+0.4*sin(2*PI*0.4*t+{faza}))+0.12*sin(2*PI*{f2}*t+1.5*sin(2*PI*3*t))"
        f"+0.08*sin(2*PI*{4 * f}*t)*lt(mod(t+{faza / 10},0.5),0.04)"
    )


# „film“ 5.1: FL FR FC LFE BL BR
FILM_51 = "|".join(
    [kanal(196, 294, 0.0), kanal(247, 370, 1.0), kanal(165, 330, 2.0), "0.4*sin(2*PI*45*t)*(0.5+0.5*sin(2*PI*0.25*t))",
     kanal(131, 262, 3.0), kanal(147, 220, 4.0)]
)


def ffmpeg(*argumenty):
    subprocess.run(["ffmpeg", "-v", "error", "-y", *argumenty], check=True)


def zdroj(vyraz, frekvencia, sekund, kanalov):
    rozlozenie = {2: "stereo", 6: "5.1"}[kanalov]
    return [
        "-f", "lavfi", "-i", f"aevalsrc='{vyraz}':s={frekvencia}:d={sekund}:c={rozlozenie}",
        "-f", "lavfi", "-i", f"anoisesrc=d={sekund}:c=pink:r={frekvencia}:a=0.04:s=7",
        "-filter_complex", f"[1:a]aformat=channel_layouts={rozlozenie}[s];[0:a][s]amix=inputs=2:normalize=0[a]",
        "-map", "[a]",
    ]


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    ciel = sys.argv[1]
    sekund = int(sys.argv[2]) if len(sys.argv) > 2 else 30
    os.makedirs(ciel, exist_ok=True)
    p = lambda m: os.path.join(ciel, m)  # noqa: E731
    ffmpeg(*zdroj(STEREO, 44100, sekund, 2), "-c:a", "flac", "-sample_fmt", "s16", "-compression_level", "5", p("hudba.flac"))
    ffmpeg(*zdroj(STEREO, 48000, sekund, 2), "-c:a", "libopus", "-b:a", "128k", p("hudba.opus"))
    ffmpeg(*zdroj(STEREO, 44100, sekund, 2), "-c:a", "libmp3lame", "-b:a", "192k", p("hudba.mp3"))
    ffmpeg(*zdroj(STEREO, 48000, sekund, 2), "-c:a", "mp2", "-b:a", "256k", p("hudba.mp2"))
    ffmpeg(*zdroj(FILM_51, 48000, sekund, 6), "-c:a", "ac3", "-b:a", "448k", p("film51.mkv"))
    for m in sorted(os.listdir(ciel)):
        print(f"{m} {os.path.getsize(p(m))} B")


if __name__ == "__main__":
    main()
