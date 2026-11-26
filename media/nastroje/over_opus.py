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


STRIEDANIE = (f"aevalsrc='if(lt(mod(t,3),1.5),{_hlas(140, 20, 0)},{_hudba(196, 3, 0.5)})"
              f"|if(lt(mod(t,3),1.5),{_hlas(190, 20, 1)},{_hudba(247, 5, 0.75)})':s=48000:d=6")


def zvuk(kanalov, hudba=False, ticho=False, striedanie=False):
    if striedanie:  # reč ↔ hudba každých 1,5 s – kodér prepína SILK/hybrid ↔ CELT (prechody s nadbytočnosťou)
        vyraz = STRIEDANIE if kanalov == 2 else STRIEDANIE.split("|")[0] + ":s=48000:d=6"
    elif ticho:  # každá druhá sekunda digitálne ticho (rámce CELT s príznakom ticha)
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
    # SILK pri 48 kHz (vlastné prevzorkovanie – nenormatívne, preto miera podobnosti s prevzorkovaním libopus)
    ("silk_nb_48k", 1, ["-b:a", "12k", "-cutoff", "4000"], 48000, 15),
    ("silk_mb_48k", 1, ["-b:a", "16k", "-cutoff", "6000"], 48000, 15),
    ("silk_wb_48k_stereo", 2, ["-b:a", "24k", "-cutoff", "8000"], 48000, 15),
    # hybrid (SILK WB + CELT od pásma 17)
    ("hybrid_swb", 1, ["-b:a", "24k", "-cutoff", "12000"], 48000, 15),
    ("hybrid_fb", 1, ["-b:a", "32k", "-cutoff", "20000"], 48000, 15),
    ("hybrid_fb_10_stereo", 2, ["-b:a", "48k", "-cutoff", "20000", "-frame_duration", "10"], 48000, 15),
    # prechody: kodér libopus cez ctypes s vynúteným režimom (každých 0,5 s), nadbytočné rámce vkladá sám
    ("prechody_silk_hybrid_celt", 1, {"rezimy": [1000, 1001, 1002], "tok": 32000}, 48000, 15),
    ("prechody_celt_silk_stereo", 2, {"rezimy": [1002, 1000], "tok": 48000}, 48000, 15),
    ("prechody_nb_silk_celt", 1, {"rezimy": [1000, 1001, 1002, 1001], "tok": 24000, "pasma": [1101, 1105]}, 48000,
     15),
    ("prechody_hybrid_silk_nb", 1, {"rezimy": [1001, 1000], "tok": 24000, "pasma": [1105, 1101]}, 48000, 15),
    ("prechody_silk_wb_hybrid", 1, {"rezimy": [1000, 1001], "tok": 24000, "pasma": [1103, 1105]}, 48000, 15),
]

# stopy cez `DekoderZvuku` (`media_opus --stopa`): viac prúdov (RFC 7845, rodina 1), pre-skip, orezanie konca
# (bezmedzerovo – dĺžka ako dekódovanie FFmpeg), poradie Vorbis → spoločné poradie; (meno, kanálov, kodér, prah SNR)
STOPY = [
    ("stopa_5.1", 6, ["-b:a", "256k"], 100),
    ("stopa_7.1_hybrid", 8, ["-b:a", "160k", "-cutoff", "12000"], 15),
    ("stopa_stereo_hybrid", 2, ["-application", "voip", "-b:a", "32k", "-cutoff", "20000"], 15),
    ("stopa_mono_silk", 1, ["-application", "voip", "-b:a", "16k", "-cutoff", "8000"], 15),
]
# poradie Vorbis (výstup libopus) → spoločné poradie: index v spoločnom poradí pre kanál Vorbis
PORADIE = {1: [0], 2: [0, 1], 3: [0, 2, 1], 4: [0, 1, 2, 3], 5: [0, 2, 1, 3, 4], 6: [0, 2, 1, 4, 5, 3],
           7: [0, 2, 1, 5, 6, 4, 3], 8: [0, 2, 1, 6, 7, 4, 5, 3]}


def zvuk_kanalov(n):
    vyrazy = [_hudba(130 + 37 * k, 3 + 2 * k, 0.5 + 0.125 * k) for k in range(n)]
    return ["-f", "lavfi", "-i", f"aevalsrc='{'|'.join(vyrazy)}':s=48000:d=6"]


def referencia_viac(pakety_, hlavicka):
    lib = ctypes.CDLL("libopus.so.0")
    chyba = ctypes.c_int()
    lib.opus_multistream_decoder_create.restype = ctypes.c_void_p
    n = hlavicka["kanalov"]
    mapa = (ctypes.c_ubyte * n)(*hlavicka["mapovanie"])
    dek = ctypes.c_void_p(lib.opus_multistream_decoder_create(48000, n, hlavicka["prudov"], hlavicka["spojenych"],
                                                              mapa, ctypes.byref(chyba)))
    assert chyba.value == 0, chyba.value
    buf = (ctypes.c_float * (5760 * n))()
    rng = ctypes.c_uint32()
    vzorky, stavy = [], []
    for p in pakety_:
        m = lib.opus_multistream_decode_float(dek, p, len(p), buf, 5760, 0)
        assert m >= 0, m
        lib.opus_multistream_decoder_ctl(dek, OPUS_GET_FINAL_RANGE, ctypes.byref(rng))
        stavy.append(rng.value)
        vzorky.extend(buf[: m * n])
    lib.opus_multistream_decoder_destroy(dek)
    return vzorky, stavy


def over_stopy(program, adresar, vyber):
    chyb = 0
    print("\n| stopa | paketov | prúdy (spojené) | konečný stav | dĺžka ako FFmpeg | SNR | výsledok |")
    print("|---|---|---|---|---|---|---|")
    for meno, kanalov, kodovanie, prah in STOPY:
        if not vyber.search(meno):
            continue
        cesta = os.path.join(adresar, meno + ".mka")
        vstup = zvuk_kanalov(kanalov) if kanalov > 2 else zvuk(kanalov, hudba=meno.endswith("5.1"))
        subprocess.run(["ffmpeg", "-v", "error", "-y"] + vstup + ["-c:a", "libopus"] + kodovanie + [cesta], check=True)
        pp = pakety(cesta)
        surove = os.path.join(adresar, meno + ".f32")
        r = subprocess.run([program, cesta, surove, "--stopa"], capture_output=True, text=True)
        if r.returncode != 0:
            print(f"| {meno} | {len(pp)} | – | – | – | – | ❌ {r.stdout.strip()[-200:]} |")
            chyb += 1
            continue
        hl = dict(re.findall(r"(\w+)=(\S+)", re.search(r"HLAVICKA (.*)", r.stdout).group(1)))
        hlavicka = {k: int(hl[k]) for k in ("kanalov", "predskok", "prudov", "spojenych", "zisk")}
        hlavicka["mapovanie"] = [int(x) for x in hl["mapovanie"].split(",")]
        ref, ref_stavy = referencia_viac(pp, hlavicka)
        nase_stavy = [int(m, 16) for m in re.findall(r"rng=([0-9a-f]{8})", r.stdout)]
        zhoda = sum(a == b for a, b in zip(nase_stavy, ref_stavy))
        d = open(surove, "rb").read()
        nase = struct.unpack(f"<{len(d) // 4}f", d)
        # dĺžka po pre-skip a orezaní konca: FFmpeg (dekodér libopus) ako rozhodca bezmedzerovosti
        ff = subprocess.run(["ffmpeg", "-v", "error", "-c:a", "libopus", "-i", cesta, "-f", "f32le", "-"],
                            check=True, capture_output=True).stdout
        ff_ramcov = len(ff) // 4 // kanalov
        nase_ramcov = len(nase) // kanalov
        # referencia: zahodiť pre-skip, prerovnať kanály, zisk hlavičky
        zisk = 10 ** (hlavicka["zisk"] / 256 / 20)
        poradie = PORADIE[kanalov]
        p0 = hlavicka["predskok"]
        x = [0.0] * len(nase)
        for i in range(nase_ramcov):
            for c in range(kanalov):
                x[i * kanalov + poradie[c]] = ref[(p0 + i) * kanalov + c] * zisk
        sig = sum(v * v for v in x)
        e = sum((a - b) ** 2 for a, b in zip(x, nase))
        snr = 10 * math.log10(sig / e) if e > 0 else float("inf")
        # po kanáloch (spoločné poradie): prúdy CELT musia sedieť až na zaokrúhlenie (správne mapovanie a poradie),
        # prúdy s hybridom určuje nenormatívne prevzorkovanie SILK
        po_kanaloch = []
        for c in range(kanalov):
            sc = sum(x[i] ** 2 for i in range(c, len(x), kanalov))
            ec = sum((x[i] - nase[i]) ** 2 for i in range(c, len(x), kanalov))
            po_kanaloch.append(10 * math.log10(sc / ec) if ec > 0 else 300.0)
        kriterium = max(po_kanaloch) if prah >= 100 else snr
        ok = zhoda == len(pp) and nase_ramcov == ff_ramcov and kriterium >= prah
        chyb += not ok
        kanaly = ", ".join(f"{v:.0f}" for v in po_kanaloch)
        print(f"| {meno} | {len(pp)} | {hlavicka['prudov']} ({hlavicka['spojenych']}) | {zhoda}/{len(pp)} | "
              f"{nase_ramcov} / {ff_ramcov} | {snr:.1f} dB (kanály {kanaly}) | {'✅' if ok else '❌'} |")
    return chyb


# vynútenie režimu kodéra (riadiaci kód libopus OPUS_SET_FORCE_MODE; 1000 SILK, 1001 hybrid, 1002 CELT)
OPUS_SET_FORCE_MODE = 11002


def vynutene_pakety(kanalov, nastavenie, adresar, meno):
    """Pakety z kodéra libopus s vynúteným režimom; zapíše `.opuspak` (dĺžka u16 + paket) a vráti zoznam."""
    surovy = subprocess.run(["ffmpeg", "-v", "error"] + zvuk(kanalov, striedanie=True)
                            + ["-f", "f32le", "-ac", str(kanalov), "-ar", "48000", "-"],
                            check=True, capture_output=True).stdout
    pcm = struct.unpack(f"<{len(surovy) // 4}f", surovy)
    lib = ctypes.CDLL("libopus.so.0")
    chyba = ctypes.c_int()
    lib.opus_encoder_create.restype = ctypes.c_void_p
    kod = ctypes.c_void_p(lib.opus_encoder_create(48000, kanalov, 2049, ctypes.byref(chyba)))
    assert chyba.value == 0
    assert lib.opus_encoder_ctl(kod, 4002, ctypes.c_int(nastavenie["tok"])) == 0  # OPUS_SET_BITRATE
    n = 960
    vysl = []
    out = (ctypes.c_ubyte * 1500)()
    for i in range(len(pcm) // (n * kanalov)):
        rezimy = nastavenie["rezimy"]
        assert lib.opus_encoder_ctl(kod, OPUS_SET_FORCE_MODE, ctypes.c_int(rezimy[(i // 25) % len(rezimy)])) == 0
        if "pasma" in nastavenie:  # OPUS_SET_MAX_BANDWIDTH – striedanie šírky pásma (napr. NB SILK ↔ hybrid FB)
            pasma = nastavenie["pasma"]
            lib.opus_encoder_ctl(kod, 4004, ctypes.c_int(pasma[(i // 25) % len(pasma)]))
        ramec = (ctypes.c_float * (n * kanalov))(*pcm[i * n * kanalov:(i + 1) * n * kanalov])
        m = lib.opus_encode_float(kod, ramec, n, out, 1500)
        assert m > 0, m
        vysl.append(bytes(out[:m]))
    lib.opus_encoder_destroy(kod)
    with open(os.path.join(adresar, meno + ".opuspak"), "wb") as f:
        for p in vysl:
            f.write(struct.pack("<H", len(p)) + p)
    return vysl


def ramce_celt_snr(x, y, kanalov, pp):
    """Najmenší SNR rámca CELT-only (20 ms) – None bez numpy alebo bez rámcov CELT."""
    try:
        import numpy as np
    except ImportError:
        return None
    a = np.asarray(x, dtype=np.float64).reshape(-1, kanalov)
    b = np.asarray(y, dtype=np.float64).reshape(-1, kanalov)
    vysl = []
    for k, p in enumerate(pp):
        if (p[0] >> 3) < 16:
            continue
        s = slice(k * 960, (k + 1) * 960)
        e = np.sum((a[s] - b[s]) ** 2)
        r = np.sum(a[s] ** 2)
        if r > 0:
            vysl.append(10 * math.log10(r / e) if e > 0 else 300.0)
    return min(vysl) if vysl else None


def diagnostika(x, y, kanalov, pp, hrana):
    """Pri neúspechu: rámce (20 ms) s najväčším podielom chyby v nízkom pásme a ich konfigurácie TOC."""
    try:
        import numpy as np
    except ImportError:
        return
    def nizke(v):
        f = np.fft.rfft(np.asarray(v, dtype=np.float64).reshape(-1, kanalov).mean(axis=1))
        k = np.fft.rfftfreq((len(v) // kanalov), 1 / 48000)
        f[(k < 50) | (k >= hrana)] = 0
        return np.fft.irfft(f, len(v) // kanalov)
    a, b = nizke(x), nizke(y)
    e = (a - b) ** 2
    spolu = float(np.sum(e))
    rad = sorted(range(len(pp)), key=lambda k: -float(np.sum(e[k * 960:(k + 1) * 960])))
    for k in rad[:8]:
        ek = float(np.sum(e[k * 960:(k + 1) * 960]))
        sk = float(np.sum(a[k * 960:(k + 1) * 960] ** 2))
        pred = pp[k - 1][0] >> 3 if k else -1
        print(f"  diagnostika: rámec {k} konfigurácia {pred}->{pp[k][0] >> 3} (dĺžka {len(pp[k])} B), "
              f"podiel chyby {100 * ek / spolu:.1f} %, SNR rámca {10 * math.log10(sk / ek) if ek else 999:.1f} dB")


def pasmovy_snr(x, y, kanalov, od, do):
    """SNR v pásme od–do Hz (stredný signál pri sterei); None bez numpy."""
    try:
        import numpy as np
    except ImportError:
        return None
    a = np.asarray(x, dtype=np.float64).reshape(-1, kanalov).mean(axis=1)
    b = np.asarray(y, dtype=np.float64).reshape(-1, kanalov).mean(axis=1)
    fa, fb = np.fft.rfft(a), np.fft.rfft(b)
    f = np.fft.rfftfreq(len(a), 1 / 48000)
    m = (f >= od) & (f < do)
    e = np.sum(np.abs(fa[m] - fb[m]) ** 2)
    return 10 * math.log10(np.sum(np.abs(fa[m]) ** 2) / e) if e > 0 else float("inf")


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
        if isinstance(kodovanie, dict):
            cesta = os.path.join(adresar, meno + ".opuspak")
            pp = vynutene_pakety(kanalov, kodovanie, adresar, meno)
        else:
            cesta = os.path.join(adresar, meno + ".mka")
            aplikacia = [] if "-application" in kodovanie else ["-application", "voip"]
            subprocess.run(["ffmpeg", "-v", "error", "-y"] + zvuk(kanalov, meno.startswith("celt"), "ticho" in meno)
                           + ["-c:a", "libopus"] + aplikacia + kodovanie + [cesta], check=True)
            pp = pakety(cesta)
        konfig = sorted({p[0] >> 3 for p in pp if p})
        ref, ref_stavy = referencia(pp, frekvencia, kanalov)
        surove = os.path.join(adresar, meno + ".f32")
        prepinac = [] if frekvencia == 48000 else ["--vnutorna"]
        r = subprocess.run([program, cesta, surove] + prepinac, capture_output=True, text=True)
        nase_stavy = [int(m, 16) for m in re.findall(r"rng=([0-9a-f]{8})", r.stdout)]
        if r.returncode != 0:
            print(f"| {meno} | {len(pp)} | {konfig} | – | – | ❌ {r.stdout.strip()[-200:]} |")
            chyb += 1
            continue
        d = open(surove, "rb").read()
        nase = struct.unpack(f"<{len(d) // 4}f", d)
        zhoda = sum(a == b for a, b in zip(nase_stavy, ref_stavy))
        prva = next((i for i, (a, b) in enumerate(zip(nase_stavy, ref_stavy)) if a != b), None)
        # zarovnanie: najlepšie oneskorenie referencie musí byť normatívne; pri SILK pri 48 kHz sa nehľadá – fáza
        # prevzorkovača libopus pri hornej hrane pásma SILK posúva maximum korelácie, oneskorenie overí nízke pásmo
        silk48 = frekvencia == 48000 and not meno.startswith("celt")

        def skore(lag):
            if lag < 0:
                return sum(x * y for x, y in zip(ref[:200000], nase[-lag * kanalov:-lag * kanalov + 200000]))
            return sum(x * y for x, y in zip(ref[lag * kanalov:lag * kanalov + 200000], nase[:200000]))
        lag = 0 if silk48 else max(range(-30, 30), key=skore)
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
        ok = zhoda == len(pp) and len(nase) == len(ref) and snr >= prah and lag == ONESKORENIE[frekvencia]
        pasma = ""
        if silk48:
            # SILK pri 48 kHz: prevzorkovanie nie je normatívne (RFC 6716 §4.2.9) – libopus nemá lineárnu fázu pri
            # hornej hrane pásma SILK; preto sa meria zvlášť nízke pásmo (dekodér SILK a normatívne oneskorenie)
            # a pásmo CELT nad 9 kHz (hybrid), celé pásmo len kontrolne
            hrana = 1500 if "_nb" in meno else 3000
            nizke = pasmovy_snr(x, y, kanalov, 50, hrana)
            vysoke = pasmovy_snr(x, y, kanalov, 9000, 20000) if meno.startswith("hybrid") else None
            if nizke is not None:
                pasma = f", 50–{hrana} Hz {nizke:.1f} dB"
                ok = ok and nizke >= 30
            if vysoke is not None:
                pasma += f", 9–20 kHz {vysoke:.1f} dB"
                ok = ok and vysoke >= 40
            celt = ramce_celt_snr(x, y, kanalov, pp) if isinstance(kodovanie, dict) else None
            if celt is not None:
                # rámce CELT pri prechodoch (aj prvý po nadbytočnom rámci) – zhoda stavov CELT až na zaokrúhlenie
                pasma += f", rámce CELT najmenej {celt:.1f} dB"
                ok = ok and celt >= 100
        else:
            ok = ok and snr_stred >= 40
        chyb += not ok
        stav = f"{zhoda}/{len(pp)}" + ("" if prva is None else f" (prvý rozdiel v pakete {prva})")
        dlzka = "" if len(nase) == len(ref) else f", dĺžka {len(nase)} ≠ {len(ref)}"
        if "CELT " in r.stdout:
            pokrytie.append((meno, re.search(r"CELT (.*)", r.stdout).group(1)))
        if frekvencia == 48000 and "REZIMY " in r.stdout:
            pokrytie.append((meno, re.search(r"REZIMY (.*)", r.stdout).group(1)))
        if not ok and silk48:
            diagnostika(x, y, kanalov, pp, 1500 if "_nb" in meno else 3000)
            if isinstance(kodovanie, dict):  # pakety do logu (rozbor rozdielov kodéra na inom procesore)
                import base64
                print(f"  PAKETY_BASE64 {meno} " + base64.b64encode(open(cesta, "rb").read()).decode())
        stred = "" if kanalov == 1 else f", stred {snr_stred:.1f} dB"
        print(f"| {meno} | {len(pp)} | {konfig} | {stav} | {snr:.1f} dB{stred}{pasma} (oneskorenie {lag}) "
              f"| {'✅' if ok else '❌'}{dlzka} |")
    chyb += over_stopy(program, adresar, vyber)
    if pokrytie:
        print("\nPokrytie nástrojov CELT a režimov (počty rámcov):")
        for meno, riadok in pokrytie:
            print(f"- {meno}: {riadok}")
    sys.exit(1 if chyb else 0)


if __name__ == "__main__":
    main()
