#!/usr/bin/env python3
"""Rozhodca MPEG audio vrstvy III (L8): rozbor bitového toku a presnosť dekódovania.

1. **Spotreba bitov** (dôkaz entropického dekódovania): pri každej granule musí rozbor škálovacích faktorov
   a Huffmanových kódov skončiť presne na konci `part2_3_length`. LAME presne; Shine dopĺňa výplň po 576 čiarach
   ([S] §4.3.4 – výplňové bity sa zahodia), pripustí sa jednotvárna výplň a štvorica prečnievajúca za koniec.
2. **Presnosť podľa ISO/IEC 11172-4** proti nezávislému rozhodcovi (dekodér `mp3float` FFmpeg v plávajúcej
   čiarke): RMS rozdielu < 2^-15/√12 a najväčší rozdiel ≤ 2^-14 pri plnom rozsahu ±1,0; posun 0, rovnaká dĺžka.
   Toky bez hlavičky Xing/Info (`-write_xing 0`), aby rozhodca nič neorezal – porovnáva sa celé dekódovanie.
3. **Skúšobné toky ISO** (voliteľne, adresár s pármi `<meno>.bit` + `<meno>.pcm`): referenčný výstup 16 bitov;
   náš výstup zaokrúhlený na 16 bitov sa smie líšiť najviac o 1 LSB (≤ 2^-15 < 2^-14) a RMS rozdielu pred
   zaokrúhlením je v hraniciach zaokrúhlenia referencie. Formát PCM (LE/BE) sa zistí z menšej chyby.

Použitie: `over_mpa.py <media_mpa> <adresár> [adresár so skúšobnými tokmi]`
"""
import math
import os
import re
import subprocess
import sys

import numpy as np

HUDBA = ("aevalsrc='0.3*sin(2*PI*220*t)+0.15*sin(2*PI*1330*t+1)+0.35*(random(1)-0.5)*exp(-30*mod(t,0.4))"
         "|0.3*sin(2*PI*277*t)+0.15*sin(2*PI*1760*t)+0.35*(random(2)-0.5)*exp(-20*mod(t+0.13,0.55))'")
# silno korelované kanály – kodér volí M/S v každom rámci
MS = ["-af", "pan=stereo|c0=0.55*c0+0.45*c1|c1=0.45*c0+0.55*c1"]
HRANICA_RMS = 2 ** -15 / math.sqrt(12)
HRANICA_MAX = 2 ** -14

# (meno, kodér, frekvencia, kanálov, argumenty)
VARIANTY = [
    ("lame_128_js", "libmp3lame", 44100, 2, ["-b:a", "128k"]),
    ("lame_320", "libmp3lame", 48000, 2, ["-b:a", "320k"]),
    ("lame_32_mono", "libmp3lame", 32000, 1, ["-b:a", "32k"]),
    ("lame_v0", "libmp3lame", 44100, 2, ["-q:a", "0"]),
    ("lame_v9", "libmp3lame", 44100, 2, ["-q:a", "9"]),
    ("lame_stereo", "libmp3lame", 44100, 2, ["-b:a", "192k", "-joint_stereo", "0"]),
    ("lame_ms", "libmp3lame", 44100, 2, ["-b:a", "96k"] + MS),
    ("lame_bez_zasobnika", "libmp3lame", 48000, 2, ["-b:a", "160k", "-reservoir", "0"]),
    ("lame_abr_mono", "libmp3lame", 48000, 1, ["-b:a", "96k", "-abr", "1"]),
    ("shine_128", "libshine", 44100, 2, ["-b:a", "128k"]),
    ("shine_64_mono", "libshine", 32000, 1, ["-b:a", "64k"]),
    ("shine_256", "libshine", 48000, 2, ["-b:a", "256k"]),
]


def dekoduj(program, cesta, von):
    r = subprocess.run([program, cesta, von], capture_output=True, text=True)
    m = dict((a, int(b)) for a, b in re.findall(r"(\w+)=(\d+)", r.stdout))
    return r.returncode, m


def presnost(nas, ref):
    """(RMS, najväčší rozdiel, úroveň) dvoch f64 polí rovnakej dĺžky."""
    d = nas - ref
    return float(np.sqrt(np.mean(d * d))), float(np.abs(d).max()), float(np.sqrt(np.mean(ref * ref)))


def varianty(program, adresar):
    print("| variant | granúl | presne na konci part2_3_length | RMS rozdielu | najväčší rozdiel | pokrytie | výsledok |")
    print("|---|---|---|---|---|---|---|")
    chyb = 0
    for meno, koder, f, k, arg in VARIANTY:
        cesta = os.path.join(adresar, meno + ".mp3")
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", f"{HUDBA}:s={f}:d=8", "-ac", str(k),
                        "-ar", str(f), "-c:a", koder] + arg + ["-write_xing", "0", cesta], check=True)
        ref_cesta, nas_cesta = cesta + ".ff.f32", cesta + ".nas.f32"
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-c:a", "mp3float", "-i", cesta, "-f", "f32le", ref_cesta],
                       check=True)
        kod, m = dekoduj(program, cesta, nas_cesta)
        prijate = m.get("presne", 0) + (0 if koder == "libmp3lame" else m.get("vypln_rovnaka", 0) + m.get("prebeh", 0))
        spotreba = (kod == 0 and m.get("chyb", 1) == 0 and m.get("chyby", 1) == 0
                    and m.get("chyba_skalovacich", 1) == 0 and m.get("granul", 0) > 0 and prijate == m["granul"])
        nas = np.fromfile(nas_cesta, dtype="<f4").astype(np.float64)
        ref = np.fromfile(ref_cesta, dtype="<f4").astype(np.float64)
        rovnaka_dlzka = len(nas) == len(ref) and len(ref) > 0
        rms, mx, uroven = presnost(nas, ref) if rovnaka_dlzka else (math.inf, math.inf, 0.0)
        ok = spotreba and rovnaka_dlzka and rms < HRANICA_RMS and mx <= HRANICA_MAX and uroven > 0.05
        chyb += not ok
        pokrytie = ", ".join(f"{x} {m[x]}" for x in ("kratke", "start_stop", "scfsi", "preflag", "scalefac_scale",
                                                     "linbits", "tabulka_b", "ms") if m.get(x))
        print(f"| {meno} | {m.get('granul')} | {prijate}/{m.get('granul')} (prebeh {m.get('prebeh')}, výplň "
              f"{m.get('vypln')}) | {rms:.2e} | {mx:.2e} | {pokrytie} | {'✅' if ok else '❌'} |")
        if not rovnaka_dlzka:
            print(f"  dĺžka: náš {len(nas)}, rozhodca {len(ref)}")
    print(f"\nHranice ISO/IEC 11172-4 (plná presnosť): RMS < {HRANICA_RMS:.2e}, najväčší ≤ {HRANICA_MAX:.2e}.")
    return chyb


def skusobne_toky(program, adresar, prac):
    mena = sorted(m[:-4] for m in os.listdir(adresar) if m.endswith(".bit")
                  and os.path.exists(os.path.join(adresar, m[:-4] + ".pcm")))
    print("\n| skúšobný tok ISO | kanálov / kHz | vzoriek | formát ref. | RMS rozdielu | najväčší (LSB po zaokrúhlení) "
          "| intenzita | výsledok |")
    print("|---|---|---|---|---|---|---|---|")
    chyb = 0
    for meno in mena:
        nas_cesta = os.path.join(prac, meno + ".nas.f32")
        kod, m = dekoduj(program, os.path.join(adresar, meno + ".bit"), nas_cesta)
        nas = np.fromfile(nas_cesta, dtype="<f4").astype(np.float64)
        surove = open(os.path.join(adresar, meno + ".pcm"), "rb").read()
        najlepsi = None
        for fmt in ("<i2", ">i2"):
            ref = np.frombuffer(surove[: len(surove) // 2 * 2], dtype=fmt).astype(np.float64) / 32768.0
            n = min(len(ref), len(nas))
            if n == 0:
                continue
            rms, mx, _ = presnost(nas[:n], ref[:n])
            if najlepsi is None or rms < najlepsi[0]:
                najlepsi = (rms, fmt, ref, n)
        if najlepsi is None:
            print(f"| {meno} | – | 0 | – | – | – | – | ❌ |")
            chyb += 1
            continue
        rms, fmt, ref, n = najlepsi
        lsb = int(np.abs(np.clip(np.round(nas[:n] * 32768.0), -32768, 32767) - np.round(ref[:n] * 32768.0)).max())
        # RMS rozdielu proti 16-bitovej referencii = zaokrúhlenie referencie (≈ 2^-15/√12) + naša chyba
        ok = (kod == 0 and m.get("chyb", 1) == 0 and len(nas) == len(ref) and lsb <= 1
              and rms < HRANICA_RMS * 1.1)
        chyb += not ok
        print(f"| {meno} | {m.get('kanalov')} / {m.get('frekvencia', 0) / 1000:g} | {len(nas)} (ref. {len(ref)}) | "
              f"{'LE' if fmt == '<i2' else 'BE'} | {rms:.2e} | {lsb} | {m.get('intenzitne_pasma', 0)} | "
              f"{'✅' if ok else '❌'} |")
    if not mena:
        print("| (žiadne toky) | | | | | | | ❌ |")
        chyb += 1
    return chyb


def main():
    program, adresar = sys.argv[1], sys.argv[2]
    os.makedirs(adresar, exist_ok=True)
    chyb = varianty(program, adresar)
    if len(sys.argv) > 3:
        chyb += skusobne_toky(program, sys.argv[3], adresar)
    sys.exit(1 if chyb else 0)


if __name__ == "__main__":
    main()
