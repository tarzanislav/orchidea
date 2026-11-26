#!/usr/bin/env python3
"""Rozhodca dekodéra Opus (etapa L7): libopus (systémová knižnica cez ctypes) proti `media_opus`.

Pre každý variant FFmpeg (kodér libopus) vyrobí súbor Matroska, z neho sa vezmú pakety (ffprobe -show_data)
a dekódujú referenčným libopus pri tej istej frekvencii ako náš výstup (vnútorná frekvencia SILK, neskôr 48 kHz).
Porovnáva sa:

1. **konečný stav rozsahového dekodéra po každom pakete** (`OPUS_GET_FINAL_RANGE`) – zhoda dokazuje bitovo
   správne entropické dekódovanie celého paketu (každý symbol, každá vetva rozboru),
2. **vzorky** – odstup signálu od rozdielu (SNR) celého výstupu po zarovnaní o normatívne oneskorenie
   prevzorkovania; rekonštrukcia SILK podľa RFC 6716 §4.2.7.9 nie je bitovo predpísaná (my f32, libopus pevná
   čiarka), požaduje sa SNR ≥ prah variantu (40 dB; pre porovnanie: natívny dekodér FFmpeg v plávajúcej čiarke
   dáva voči libopus 17–28 dB pri 48 kHz). CELT (48 kHz, plávajúca čiarka ako libopus): prah 120 dB – zhoda až
   na zaokrúhlenie f32; vypíše sa aj pokrytie nástrojov CELT (prechodové rámce, postfilter, intenzita, …).

Použitie: `over_opus.py <media_opus> <pracovný adresár>`; návratový kód 0 = všetko sedí.
"""

import ctypes
import math
import os
import re
import struct
import subprocess
import sys

OPUS_GET_FINAL_RANGE = 4031

def _hlas(f0, vibrato, sum_seed, fazy=(0.5, 1.0)):
    t0 = f"({f0}+{vibrato}*sin(2*PI*1.5*t))"
    return (f"(0.25*sin(2*PI*{t0}*t)+0.12*sin(4*PI*{t0}*t+{fazy[0]})+0.06*sin(6*PI*{t0}*t+{fazy[1]}))"
            f"*(0.5+0.5*sin(2*PI*2.3*t))+0.1*(random({sum_seed})-0.5)")


# „hlas“: harmonické s vibratom, obálka slabík a šum (prirodzenejší ako čisté harmonické – pri dokonale periodickom
# signáli sa každá rekonštrukcia v plávajúcej čiarke vzďaľuje od pevnej čiarky libopus pri zisku LTP ≈ 1, aj natívny
# dekodér FFmpeg; zistené pri L7)
HLAS = f"aevalsrc='{_hlas(140, 20, 0)}|{_hlas(220, 30, 1, (1.2, 0.3))}':s=48000:d=6"


def _hudba(f0, seed, rytmus):
    # akord s vibratom, šumové údery (prechodové rámce CELT) a tichý šum v pozadí
    t0 = f"({f0}*(1+0.003*sin(2*PI*5*t)))"
    return (f"(0.18*sin(2*PI*{t0}*t)+0.12*sin(2*PI*1.26*{t0}*t+0.4)+0.1*sin(2*PI*1.5*{t0}*t+1.1)"
            f"+0.05*sin(2*PI*4.02*{t0}*t))*(0.6+0.4*sin(2*PI*0.37*t))"
            f"+0.35*(random({seed})-0.5)*exp(-25*mod(t,{rytmus}))+0.02*(random({seed + 7})-0.5)")


HUDBA = f"aevalsrc='{_hudba(196, 3, 0.5)}|{_hudba(247, 5, 0.75)}':s=48000:d=6"


def zvuk(kanalov, hudba=False, ticho=False):
    if ticho:  # každá druhá sekunda digitálne ticho (rámce CELT s príznakom ticha)
        vyraz = f"aevalsrc='({_hudba(196, 3, 0.5)})*gt(mod(t,2),1)':s=48000:d=6"
    elif hudba:
        vyraz = HUDBA if kanalov == 2 else f"aevalsrc='{_hudba(196, 3, 0.5)}':s=48000:d=6"
    else:
        vyraz = HLAS if kanalov == 2 else f"aevalsrc='{_hlas(140, 20, 0)}':s=48000:d=6"
    return ["-f", "lavfi", "-i", vyraz]


# normatívne oneskorenie prevzorkovania SILK (RFC 6716 §4.2.9, tabuľka 54) – libopus ho drží aj pri výstupe na
# vnútornej frekvencii: NB 0,538 ms ≈ 4, MB 0,692 ms ≈ 9 (8,3), WB 0,706 ms ≈ 12 (11,3) vzoriek
ONESKORENIE = {8000: 4, 12000: 9, 16000: 12, 48000: 0}


# (meno, kanálov, argumenty kodéra, frekvencia porovnania, najmenší SNR v dB); CELT: „-application lowdelay“
CELT = ["-application", "lowdelay"]
VARIANTY = [
    ("silk_nb_20", 1, ["-b:a", "8k", "-cutoff", "4000"], 8000, 40),
    ("silk_mb_20", 1, ["-b:a", "12k", "-cutoff", "6000"], 12000, 40),
    ("silk_wb_20", 1, ["-b:a", "20k", "-cutoff", "8000"], 16000, 40),
    ("silk_wb_10", 1, ["-b:a", "20k", "-cutoff", "8000", "-frame_duration", "10"], 16000, 40),
    ("silk_wb_40", 1, ["-b:a", "16k", "-cutoff", "8000", "-frame_duration", "40"], 16000, 40),
    ("silk_wb_60", 1, ["-b:a", "16k", "-cutoff", "8000", "-frame_duration", "60"], 16000, 40),
    ("silk_nb_60", 1, ["-b:a", "6k", "-cutoff", "4000", "-frame_duration", "60"], 8000, 40),
    ("silk_wb_stereo", 2, ["-b:a", "28k", "-cutoff", "8000"], 16000, 25),
    ("silk_mb_stereo_40", 2, ["-b:a", "20k", "-cutoff", "6000", "-frame_duration", "40"], 12000, 25),
    ("silk_wb_lbrr", 1, ["-b:a", "24k", "-cutoff", "8000", "-packet_loss", "20", "-fec", "1"], 16000, 40),
    ("silk_wb_stereo_lbrr", 2, ["-b:a", "32k", "-cutoff", "8000", "-packet_loss", "15", "-fec", "1",
                                "-frame_duration", "60"], 16000, 25),
    ("celt_fb_20", 1, CELT + ["-b:a", "64k"], 48000, 120),
    ("celt_fb_20_stereo", 2, CELT + ["-b:a", "128k"], 48000, 120),
    ("celt_fb_10_stereo", 2, CELT + ["-b:a", "96k", "-frame_duration", "10"], 48000, 120),
    ("celt_fb_5", 1, CELT + ["-b:a", "64k", "-frame_duration", "5"], 48000, 120),
    ("celt_fb_2.5", 1, CELT + ["-b:a", "64k", "-frame_duration", "2.5"], 48000, 120),
    ("celt_nb_20", 1, CELT + ["-b:a", "16k", "-cutoff", "4000"], 48000, 120),
    ("celt_wb_20", 1, CELT + ["-b:a", "24k", "-cutoff", "8000"], 48000, 120),
    ("celt_swb_20_stereo", 2, CELT + ["-b:a", "40k", "-cutoff", "12000"], 48000, 120),
    ("celt_nizky_stereo", 2, CELT + ["-b:a", "24k"], 48000, 120),
    ("celt_vysoky_stereo", 2, CELT + ["-b:a", "320k", "-frame_duration", "10"], 48000, 120),
    ("celt_cbr_stereo", 2, CELT + ["-b:a", "48k", "-vbr", "off", "-frame_duration", "5"], 48000, 120),
    ("celt_256k", 1, CELT + ["-b:a", "256k", "-frame_duration", "2.5"], 48000, 120),
    ("celt_ticho", 1, CELT + ["-b:a", "32k", "-frame_duration", "10"], 48000, 120),
]


def pakety(cesta):
    r = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "a:0", "-show_packets", "-show_data",
                        "-of", "json", cesta], check=True, capture_output=True, text=True).stdout
    import json
    vysl = []
    for p in json.loads(r)["packets"]:
        # riadok: „00000000: 4882 e2b7 … 1f  text“ – skupiny po 4 hex, posledná môže mať 2
        hexa = "".join("".join(re.findall(r"\b[0-9a-f]{2,4}\b", riadok[10:50]))
                       for riadok in p["data"].split("\n") if re.match(r"^[0-9a-f]{8}: ", riadok))
        b = bytes.fromhex(hexa)
        assert len(b) == int(p["size"]), (len(b), p["size"])
        vysl.append(b)
    return vysl


def referencia(pakety_, frekvencia, kanalov):
    lib = ctypes.CDLL("libopus.so.0")
    chyba = ctypes.c_int()
    lib.opus_decoder_create.restype = ctypes.c_void_p
    dek = lib.opus_decoder_create(frekvencia, kanalov, ctypes.byref(chyba))
    assert chyba.value == 0
    max_n = frekvencia * 120 // 1000
    buf = (ctypes.c_float * (max_n * kanalov))()
    rng = ctypes.c_uint32()
    vzorky, stavy = [], []
    for p in pakety_:
        n = lib.opus_decode_float(ctypes.c_void_p(dek), p, len(p), buf, max_n, 0)
        assert n >= 0, n
        lib.opus_decoder_ctl(ctypes.c_void_p(dek), OPUS_GET_FINAL_RANGE, ctypes.byref(rng))
        stavy.append(rng.value)
        vzorky.extend(buf[: n * kanalov])
    lib.opus_decoder_destroy(ctypes.c_void_p(dek))
    return vzorky, stavy


def main():
    program, adresar = sys.argv[1], sys.argv[2]
    os.makedirs(adresar, exist_ok=True)
    print("| variant | paketov | TOC | konečný stav rozsahového dekodéra | SNR | výsledok |")
    print("|---|---|---|---|---|---|")
    chyb = 0
    pokrytie = []
    vyber = re.compile(os.environ.get("OPUS_VARIANT", "."))  # na ladenie: len varianty podľa regulárneho výrazu
    for meno, kanalov, kodovanie, frekvencia, prah in VARIANTY:
        if not vyber.search(meno):
            continue
        cesta = os.path.join(adresar, meno + ".mka")
        aplikacia = [] if "-application" in kodovanie else ["-application", "voip"]
        subprocess.run(["ffmpeg", "-v", "error", "-y"] + zvuk(kanalov, meno.startswith("celt"), "ticho" in meno) + ["-c:a", "libopus"]
                       + aplikacia + kodovanie + [cesta], check=True)
        pp = pakety(cesta)
        konfig = sorted({p[0] >> 3 for p in pp if p})
        ref, ref_stavy = referencia(pp, frekvencia, kanalov)
        surove = os.path.join(adresar, meno + ".f32")
        r = subprocess.run([program, cesta, surove], capture_output=True, text=True)
        nase_stavy = [int(m, 16) for m in re.findall(r"rng=([0-9a-f]{8})", r.stdout)]
        if r.returncode != 0:
            print(f"| {meno} | {len(pp)} | {konfig} | – | – | ❌ {r.stdout.strip()[-200:]} |")
            chyb += 1
            continue
        d = open(surove, "rb").read()
        nase = struct.unpack(f"<{len(d) // 4}f", d)
        zhoda = sum(a == b for a, b in zip(nase_stavy, ref_stavy))
        prva = next((i for i, (a, b) in enumerate(zip(nase_stavy, ref_stavy)) if a != b), None)
        # zarovnanie: najlepšie oneskorenie referencie musí byť normatívne
        def skore(lag):
            if lag < 0:
                return sum(x * y for x, y in zip(ref[:200000], nase[-lag * kanalov:-lag * kanalov + 200000]))
            return sum(x * y for x, y in zip(ref[lag * kanalov:lag * kanalov + 200000], nase[:200000]))
        lag = max(range(-30, 30), key=skore)
        if lag >= 0:
            x, y = ref[lag * kanalov:], nase[: len(ref) - lag * kanalov]
        else:
            x, y = ref[: len(ref) + lag * kanalov], nase[-lag * kanalov:]
        sig = sum(v * v for v in x)
        sum_r = sum((a - b) ** 2 for a, b in zip(x, y))
        snr = 10 * math.log10(sig / sum_r) if sum_r > 0 else float("inf")
        # stereo: stredný signál (L + R) / 2 musí sedieť ako mono (≥ 40 dB); ľavý a pravý kanál menej – bočný kanál
        # má pri tomto signáli filtre LPC so súčtom koeficientov 4095/4096 (jednosmerný zisk 4 096), ktoré zosilnia
        # rozdiel zaokrúhlenia pevnej čiarky libopus a našej plávajúcej čiarky (RFC 6716 §4.2.7.9 bitovú zhodu
        # nevyžaduje; f64 namiesto f32 výsledok nezmení – určuje ho libopus)
        snr_stred = snr
        if kanalov == 2:
            xs = [(x[i] + x[i + 1]) / 2 for i in range(0, len(x) - 1, 2)]
            ys = [(y[i] + y[i + 1]) / 2 for i in range(0, len(y) - 1, 2)]
            er = sum((a - b) ** 2 for a, b in zip(xs, ys))
            snr_stred = 10 * math.log10(sum(v * v for v in xs) / er) if er > 0 else float("inf")
        ok = (zhoda == len(pp) and len(nase) == len(ref) and snr >= prah and snr_stred >= 40
              and lag == ONESKORENIE[frekvencia])
        chyb += not ok
        stav = f"{zhoda}/{len(pp)}" + ("" if prva is None else f" (prvý rozdiel v pakete {prva})")
        dlzka = "" if len(nase) == len(ref) else f", dĺžka {len(nase)} ≠ {len(ref)}"
        if "CELT " in r.stdout:
            pokrytie.append((meno, re.search(r"CELT (.*)", r.stdout).group(1)))
        print(f"| {meno} | {len(pp)} | {konfig} | {stav} | {snr:.1f} dB{"" if kanalov == 1 else f", stred {snr_stred:.1f} dB"} (oneskorenie {lag}) | {'✅' if ok else '❌'}{dlzka} |")
    if pokrytie:
        print("\nPokrytie nástrojov CELT (počty rámcov):")
        for meno, riadok in pokrytie:
            print(f"- {meno}: {riadok}")
    sys.exit(1 if chyb else 0)


if __name__ == "__main__":
    main()
