#!/usr/bin/env python3
"""Rozhodca MPEG audio vrstvy III (L8): rozbor bitového toku a presnosť dekódovania.

1. **Spotreba bitov** (dôkaz entropického dekódovania): pri každej granule musí rozbor škálovacích faktorov
   a Huffmanových kódov skončiť presne na konci `part2_3_length`. LAME presne; Shine dopĺňa výplň po 576 čiarach
   ([S] §4.3.4 – výplňové bity sa zahodia), pripustí sa jednotvárna výplň a štvorica prečnievajúca za koniec.
2. **Presnosť podľa ISO/IEC 11172-4** proti nezávislému rozhodcovi (dekodér `mp3float` FFmpeg v plávajúcej
   čiarke): RMS rozdielu < 2^-15/√12 a najväčší rozdiel ≤ 2^-14 pri plnom rozsahu ±1,0; posun 0, rovnaká dĺžka.
   Toky bez hlavičky Xing/Info (`-write_xing 0`), aby rozhodca nič neorezal – porovnáva sa celé dekódovanie.
3. **Bezmedzerovo** (čítačka `.mp3` s Xing/Info a značkou LAME, MP3 v MP4, Matroske a MPEG-TS cez
   `kontajner::otvor` a `DekoderZvuku`): dĺžka výstupu presne ako bezmedzerové dekódovanie rozhodcu (pri .mp3
   presne dĺžka originálu) a rovnaká presnosť.
4. **Vrstva II** (MP2 – kodéry `mp2` FFmpeg a twolame; 48/44,1/32 kHz pri nízkych aj vysokých tokoch, mono,
   stereo, spojené stereo, dva kanály, MPEG-2 LSF 24/22,05/16 kHz) a **vrstva I** (toky vytvorené priamo podľa
   syntaxe CD 11172-3 2.4.1.5 s náhodným rozdelením bitov, mierkami a kódmi – kodér vrstvy I nie je) – plná
   presnosť 11172-4 proti FFmpeg. Demuxer FFmpeg niekedy zahodí prvé rámce ako „junk“ – pripustí sa posun o presne
   toľko celých rámcov, o koľko je rozhodca kratší, a porovná sa po nábehu banky filtrov (1 rámec).
5. **Vrstva III MPEG-2 LSF a 2.5** (ID = 0, 1 granula): LAME pri 22,05/24/16/11,025/12/8 kHz (CBR, VBR, mono,
   M/S) a toky vytvorené priamo podľa syntaxe [S] (`tok_lsf3`) – všetky rozsahy scalefac_compress a preflag,
   krátke, zmiešané a štart/stop bloky, intenzitné stereo LSF s M/S aj bez neho (LAME ich nepoužíva) – plná
   presnosť 11172-4 proti FFmpeg a spotreba bitov presne na konci part2_3_length.
6. **Skúšobné toky ISO** (voliteľne, adresár s pármi `<meno>.bit` + `<meno>.pcm`): referenčný výstup 16 bitov;
   náš výstup zaokrúhlený na 16 bitov sa smie líšiť najviac o 1 LSB (≤ 2^-15 < 2^-14) a RMS rozdielu pred
   zaokrúhlením je v hraniciach zaokrúhlenia referencie. Formát PCM (LE/BE) sa zistí z menšej chyby, posun musí
   byť 0 (alebo o rámce bez dostatku zásobníka bitov na začiatku, ktoré referenčný dekodér nevydá); referencia
   smie byť na konci kratšia najviac o 2 rámce; náš výstup sa pred porovnaním oreže na 16-bitový rozsah.

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
    # MPEG-2 LSF (22,05/24/16 kHz) a MPEG-2.5 (11,025/12/8 kHz) – 1 granula v rámci
    ("lame_lsf_22k", "libmp3lame", 22050, 2, ["-b:a", "64k"]),
    ("lame_lsf_24k_160", "libmp3lame", 24000, 2, ["-b:a", "160k"]),
    ("lame_lsf_16k_mono", "libmp3lame", 16000, 1, ["-b:a", "32k"]),
    ("lame_lsf_22k_v4", "libmp3lame", 22050, 2, ["-q:a", "4"]),
    ("lame_lsf_16k_ms", "libmp3lame", 16000, 2, ["-b:a", "48k"] + MS),
    ("lame_25_11k_mono", "libmp3lame", 11025, 1, ["-b:a", "16k"]),
    ("lame_25_12k", "libmp3lame", 12000, 2, ["-b:a", "32k"]),
    ("lame_25_8k_mono", "libmp3lame", 8000, 1, ["-b:a", "8k"]),
    ("lame_25_8k_v6", "libmp3lame", 8000, 2, ["-q:a", "6"]),
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


# (meno, frekvencia, kanálov, argumenty) – súbory s hlavičkou Info/Xing a značkou LAME (predvolené FFmpeg)
BEZMEDZEROVO = [
    ("cbr_128", 44100, 2, ["-b:a", "128k"]),
    ("vbr_v2", 44100, 2, ["-q:a", "2"]),
    ("mono_32k", 32000, 1, ["-b:a", "32k"]),
    ("cbr_48k", 48000, 2, ["-b:a", "192k"]),
    ("lsf_vbr_22k", 22050, 2, ["-q:a", "5"]),
    ("lsf25_8k_mono", 8000, 1, ["-b:a", "16k"]),
]


def bezmedzerovo(program, adresar):
    print("\n| bezmedzerovo | kontajner | vzoriek (originál) | vzoriek rozhodcu | RMS rozdielu | najväčší | výsledok |")
    print("|---|---|---|---|---|---|---|")
    chyb = 0
    for meno, f, k, arg in BEZMEDZEROVO:
        mp3 = os.path.join(adresar, "bm_" + meno + ".mp3")
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", f"{HUDBA}:s={f}:d=8", "-ac", str(k),
                        "-ar", str(f), "-c:a", "libmp3lame"] + arg + [mp3], check=True)
        subory = [(mp3, "MP3")]
        if meno == "cbr_128":
            for kont in ("mkv", "mp4", "ts"):
                cesta = mp3[:-3] + kont
                subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", mp3, "-c", "copy", cesta], check=True)
                subory.append((cesta, kont.upper()))
        for cesta, kont in subory:
            ref_cesta, nas_cesta = cesta + ".ff.f32", cesta + ".nas.f32"
            subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", cesta, "-f", "f32le", ref_cesta], check=True)
            r = subprocess.run([program, "--kontajner", cesta, nas_cesta], capture_output=True, text=True)
            nas = np.fromfile(nas_cesta, dtype="<f4").astype(np.float64)
            ref = np.fromfile(ref_cesta, dtype="<f4").astype(np.float64)
            rovnaka = len(nas) == len(ref) and len(ref) > 0
            rms, mx, _ = presnost(nas, ref) if rovnaka else (math.inf, math.inf, 0.0)
            # súbor .mp3 so značkou LAME: presne dĺžka originálu (8 s)
            original = len(nas) // k == 8 * f if kont == "MP3" else True
            ok = r.returncode == 0 and "chyb=0" in r.stdout and rovnaka and original and rms < HRANICA_RMS \
                and mx <= HRANICA_MAX
            chyb += not ok
            print(f"| {meno} | {kont} | {len(nas) // k}{' (' + str(8 * f) + ')' if kont == 'MP3' else ''} | "
                  f"{len(ref) // k} | {rms:.2e} | {mx:.2e} | {'✅' if ok else '❌'} |")
    return chyb


# (meno, kodér, frekvencia, kanálov, argumenty)
VRSTVA2 = [
    ("mp2_48k_192", "mp2", 48000, 2, ["-b:a", "192k"]),
    ("mp2_48k_mono_48", "mp2", 48000, 1, ["-b:a", "48k"]),
    ("mp2_48k_64", "mp2", 48000, 2, ["-b:a", "64k"]),
    ("mp2_44k_160", "mp2", 44100, 2, ["-b:a", "160k"]),
    ("mp2_44k_192", "mp2", 44100, 2, ["-b:a", "192k"]),
    ("mp2_44k_256", "mp2", 44100, 2, ["-b:a", "256k"]),
    ("mp2_44k_96", "mp2", 44100, 2, ["-b:a", "96k"]),
    ("mp2_32k_192", "mp2", 32000, 2, ["-b:a", "192k"]),
    ("mp2_32k_mono_48", "mp2", 32000, 1, ["-b:a", "48k"]),
    ("mp2_32k_64", "mp2", 32000, 2, ["-b:a", "64k"]),
    ("mp2_lsf_24k", "mp2", 24000, 2, ["-b:a", "96k"]),
    ("mp2_lsf_22k_mono", "mp2", 22050, 1, ["-b:a", "32k"]),
    ("mp2_lsf_16k", "mp2", 16000, 2, ["-b:a", "64k"]),
    ("twolame_js_48k", "libtwolame", 48000, 2, ["-b:a", "128k", "-mode", "joint_stereo"]),
    ("twolame_js_44k", "libtwolame", 44100, 2, ["-b:a", "96k", "-mode", "joint_stereo"]),
    ("twolame_js_32k", "libtwolame", 32000, 2, ["-b:a", "64k", "-mode", "joint_stereo"]),
    ("twolame_dual_48k", "libtwolame", 48000, 2, ["-b:a", "192k", "-mode", "dual_channel"]),
    ("twolame_224_44k", "libtwolame", 44100, 2, ["-b:a", "224k"]),
]


class Zapisovac:
    def __init__(self):
        self.bity = []

    def pis(self, hodnota, n):
        self.bity.extend((hodnota >> (n - 1 - i)) & 1 for i in range(n))

    def bajty(self, dlzka):
        b = self.bity + [0] * (dlzka * 8 - len(self.bity))
        assert len(b) == dlzka * 8, "rámec pretiekol"
        return bytes(int("".join(map(str, b[i:i + 8])), 2) for i in range(0, len(b), 8))


def tok_vrstvy1(cesta, rezim, ramcov=200, semienko=1):
    """Rámce vrstvy I podľa syntaxe CD 11172-3 2.4.1.5: MPEG-1, 48 kHz, 448 kb/s (1344 B), bez CRC; náhodné
    rozdelenie bitov (0–8), mierky 18–62 (≤ 2^-5 – súčet 22 pásiem ostane v rozsahu ±1) a kódy vzoriek (bez kódu
    zo samých jednotiek). `rezim` 0 stereo,
    1 spojené (rozšírenie podľa čísla rámca – všetky hranice 4, 8, 12, 16), 3 mono."""
    g = np.random.default_rng(semienko)
    nch = 1 if rezim == 3 else 2
    with open(cesta, "wb") as f:
        for r in range(ramcov):
            ext = r % 4
            hranica = 4 * (ext + 1) if rezim == 1 else 32
            z = Zapisovac()
            z.pis(0xFFF, 12); z.pis(1, 1); z.pis(3, 2); z.pis(1, 1)  # sync, MPEG-1, vrstva I, bez CRC
            z.pis(14, 4); z.pis(1, 2); z.pis(0, 1); z.pis(0, 1)      # 448 kb/s, 48 kHz, bez výplne
            z.pis(rezim, 2); z.pis(ext, 2); z.pis(0, 4)
            # pásma do 22 (vyššie ticho), menej bitov – rámec sa zmestí
            alok = np.zeros((2, 32), dtype=int)
            for sb in range(22):
                for ch in range(nch if sb < hranica else 1):
                    alok[ch, sb] = g.integers(0, 9) if g.random() < 0.8 else 0
                if sb >= hranica:
                    alok[1, sb] = alok[0, sb]
            for sb in range(32):
                for ch in range(nch if sb < hranica else 1):
                    z.pis(int(alok[ch, sb]), 4)
            for sb in range(32):
                for ch in range(nch):
                    if alok[ch, sb]:
                        z.pis(int(g.integers(18, 63)), 6)
            for _ in range(12):
                for sb in range(32):
                    for ch in range(nch if sb < hranica else 1):
                        nb = int(alok[ch, sb]) + 1 if alok[ch, sb] else 0
                        if nb:
                            z.pis(int(g.integers(0, 2 ** nb - 1)), nb)
            f.write(z.bajty(1344))


def _tabulky_pasiem():
    """Začiatky pásiem (PASMA_DLHE, PASMA_KRATKE) z vygenerovaného media/src/mpa/tabulky.rs."""
    t = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "src", "mpa", "tabulky.rs"),
             encoding="utf-8").read()

    def pole(meno):
        m = re.search(meno + r": \[\[u\d+; \d+\]; 9\] = \[(.*?)\];", t, re.S)
        return [list(map(int, re.findall(r"\d+", r))) for r in re.findall(r"\[([^\[\]]*)\]", m.group(1))]

    return pole("PASMA_DLHE"), pole("PASMA_KRATKE")


def lsf_slen(sc, intenzita):
    """(slen1–4, nr_of_sfb [dlhé, krátke, zmiešané][1–4], preflag) – [S] §4.2.4, scalefac_compress pri ID = 0;
    `intenzita` = pravý kanál pri mode_extension 01/11 (intensity_scale = sc mod 2, int_scalefac_compress = sc/2)."""
    if not intenzita:
        if sc < 400:
            return [(sc >> 4) // 5, (sc >> 4) % 5, (sc % 16) >> 2, sc % 4], [[6, 5, 5, 5], [9, 9, 9, 9], [6, 9, 9, 9]], 0
        if sc < 500:
            x = sc - 400
            return [(x >> 2) // 5, (x >> 2) % 5, x % 4, 0], [[6, 5, 7, 3], [9, 9, 12, 6], [6, 9, 12, 6]], 0
        x = sc - 500
        return [x // 3, x % 3, 0, 0], [[11, 10, 0, 0], [18, 18, 0, 0], [15, 18, 0, 0]], 1
    i = sc >> 1
    if i < 180:
        return [i // 36, (i % 36) // 6, (i % 36) % 6, 0], [[7, 7, 7, 0], [12, 12, 12, 0], [6, 15, 12, 0]], 0
    if i < 244:
        x = i - 180
        return [(x % 64) >> 4, (x % 16) >> 2, x % 4, 0], [[6, 6, 6, 3], [12, 9, 9, 6], [6, 12, 9, 6]], 0
    x = i - 244
    return [x // 3, x % 3, 0, 0], [[8, 8, 5, 0], [15, 12, 9, 0], [6, 18, 9, 0]], 0


# Huffmanova tabuľka 1 ([S] tabuľka 27): (|x|, |y|) → (kód, dĺžka)
HUFF1 = {(0, 0): (1, 1), (0, 1): (1, 3), (1, 0): (1, 2), (1, 1): (0, 3)}


def tok_lsf3(cesta, f, druh, nch=2, ramcov=150, semienko=1):
    """Rámce vrstvy III MPEG-2 LSF / 2.5 podľa syntaxe [S] (ID = 0): 160 kb/s, bez CRC a zásobníka bitov,
    1 granula. Typy blokov v povolených postupnostiach (dlhé → štart → krátke/zmiešané → stop → dlhé; zmiešaný len
    vedľa zmiešaného, štartu a stopu), všetky tri rozsahy scalefac_compress (aj preflag), subblock_gain,
    scalefac_scale; big_values s Huffmanovou tabuľkou 0 alebo 1 v každej oblasti, count1 tabuľkou B.
    `druh`: "skalovacie" (L/R), "intenzita" (mode_extension 01), "intenzita_ms" (11) – pravý kanál nenulový len
    v dolnej časti spektra, všetky tri rozsahy int_scalefac_compress, oba intensity_scale.

    Vynechané sú len prípady, kde sa FFmpeg odchyľuje od jednoznačného textu [S] (rozhodca ich nemôže overiť):
    nepovolené postupnosti okien (FFmpeg pri krátkom bloku predpokladá nulové okraje predošlého okna), zmiešaný
    blok pri 8 kHz (FFmpeg berie dlhé len podpásma 0–1), rôzne tabuľky oblastí 0 a 1 zmiešaného bloku (FFmpeg
    končí oblasť 0 s dlhou časťou) a najväčšie polohy intenzity 2^slen − 1 a polohy ≥ 16 (FFmpeg: neplatné ≥ 16)."""
    verzia, sf_idx, index_f = {24000: (2, 1, 4), 16000: (2, 2, 5), 12000: (0, 1, 7), 8000: (0, 2, 8)}[f]
    dlhe, kratke = (t[index_f] for t in _tabulky_pasiem())
    dlzka = 72 * 160000 // f
    ext = {"skalovacie": 0, "intenzita": 1, "intenzita_ms": 3}[druh]
    rezim = 3 if nch == 1 else (1 if ext else 0)
    g = np.random.default_rng(semienko)
    nasledujuce = {(0, False): [(0, False), (1, False)], (3, False): [(0, False), (1, False)],
                   (1, False): [(2, False), (2, True), (3, False)], (2, False): [(2, False), (3, False)],
                   (2, True): [(2, True), (3, False)]}
    blok = (0, False)
    with open(cesta, "wb") as subor:
        for _ in range(ramcov):
            moznosti = [m for m in nasledujuce[blok] if not (m[1] and f == 8000)]
            blok = moznosti[int(g.integers(0, len(moznosti)))]
            typ, zmiesany = blok
            prepinanie = typ != 0
            kanaly = []
            for ch in range(nch):
                intenz = ext & 1 and ch == 1
                z = Zapisovac()
                if intenz:
                    i = int(g.choice([g.integers(0, 180), g.integers(180, 244), g.integers(244, 256)]))
                    sc = 2 * i + int(g.integers(0, 2))
                else:
                    sc = int(g.choice([g.integers(0, 400), g.integers(400, 500), g.integers(500, 512)]))
                slen, nr, _ = lsf_slen(sc, intenz)
                t = 0 if typ != 2 else (2 if zmiesany else 1)
                for cast in range(4):
                    for _ in range(nr[t][cast]):
                        n = slen[cast]
                        horna = min(2 ** n - 2, 15) if intenz and n else 2 ** n - 1
                        z.pis(int(g.integers(0, horna + 1)), n)
                # oblasti big_values v poradí bitového toku
                if not prepinanie:
                    r0, r1 = int(g.integers(0, 16)), int(g.integers(0, 8))
                    hranice = [dlhe[min(r0 + 1, 22)], dlhe[min(r0 + r1 + 2, 22)], 576]
                    tabulky = [int(x) for x in g.integers(0, 2, 3)]
                else:
                    r0 = r1 = 0
                    hranice = [3 * kratke[3] if typ == 2 and not zmiesany else dlhe[8], 576]
                    tabulky = [int(x) for x in g.integers(0, 2, 2)]
                    if zmiesany:
                        tabulky[1] = tabulky[0]
                if intenz:
                    tabulky = [1] * len(tabulky)
                    bv = int(g.integers(0, 24)) if g.random() < 0.7 else 0
                    stvoric = int(g.integers(0, 30)) if g.random() < 0.7 else 0
                else:
                    bv = int(g.integers(0, min(288, hranice[0] // 2 + 30) + 1))
                    stvoric = int(g.integers(0, (576 - 2 * bv) // 4 + 1))
                hodnoty = [int(v) for v in g.choice([-1, 0, 0, 1], 2 * bv + 4 * stvoric)]
                if intenz and hodnoty:
                    hodnoty[-1] = 1  # koniec nenulovej časti pravého kanála = začiatok intenzity
                for i in range(bv):
                    oblast = next(k for k, h in enumerate(hranice) if 2 * i < h)
                    if tabulky[oblast] == 0:  # tabuľka 0: dvojica núl bez bitov
                        continue
                    x, y = hodnoty[2 * i], hodnoty[2 * i + 1]
                    kod, n = HUFF1[(abs(x), abs(y))]
                    z.pis(kod, n)
                    for v in (x, y):
                        if v:
                            z.pis(int(v < 0), 1)
                for i in range(stvoric):
                    q = hodnoty[2 * bv + 4 * i:2 * bv + 4 * i + 4]
                    z.pis(15 - (8 * abs(q[0]) + 4 * abs(q[1]) + 2 * abs(q[2]) + abs(q[3])), 4)
                    for v in q:
                        if v:
                            z.pis(int(v < 0), 1)
                kanaly.append(dict(bity=z.bity, bv=bv, gain=int(g.integers(160, 186)), sc=sc, tabulky=tabulky,
                                   r=(r0, r1), sbg=[int(x) for x in g.integers(0, 8, 3)],
                                   scale=int(g.integers(0, 2))))
            z = Zapisovac()
            z.pis(0x7FF, 11); z.pis(verzia, 2); z.pis(1, 2); z.pis(1, 1)  # sync, ID (10 MPEG-2, 00 2.5), III
            z.pis(14, 4); z.pis(sf_idx, 2); z.pis(0, 1); z.pis(0, 1)      # 160 kb/s, frekvencia
            z.pis(rezim, 2); z.pis(ext, 2); z.pis(0, 4)
            z.pis(0, 8); z.pis(0, 1 if nch == 1 else 2)                     # main_data_begin, private_bits
            for k in kanaly:
                assert len(k["bity"]) < 4096
                z.pis(len(k["bity"]), 12); z.pis(k["bv"], 9); z.pis(k["gain"], 8); z.pis(k["sc"], 9)
                z.pis(int(prepinanie), 1)
                if prepinanie:
                    z.pis(typ, 2); z.pis(int(zmiesany), 1)
                    for tb in k["tabulky"]:
                        z.pis(tb, 5)
                    for x in k["sbg"]:
                        z.pis(x, 3)
                else:
                    for tb in k["tabulky"]:
                        z.pis(tb, 5)
                    z.pis(k["r"][0], 4); z.pis(k["r"][1], 3)
                z.pis(k["scale"], 1); z.pis(1, 1)  # scalefac_scale, count1table_select = tabuľka B
            for k in kanaly:
                z.bity.extend(k["bity"])
            subor.write(z.bajty(dlzka))


def porovnaj_s_rozhodcom(nas, ref, k):
    """(RMS, najväčší, posun v rámcoch po k kanáloch): posun len o rámce, ktoré rozhodca na začiatku vynechal."""
    if len(nas) == len(ref):
        rms, mx, _ = presnost(nas, ref)
        return rms, mx, 0
    rozdiel = len(nas) - len(ref)
    for ramec in (1152, 576, 384):
        if rozdiel > 0 and rozdiel % (ramec * k) == 0 and rozdiel // (ramec * k) <= 2:
            a = nas[rozdiel:]
            rms, mx, _ = presnost(a[ramec * k:], ref[ramec * k:])
            return rms, mx, rozdiel // (ramec * k)
    return math.inf, math.inf, None


def vrstvy12(program, adresar):
    print("\n| vrstva I/II | kanálov / kHz | vzoriek | rámcov vynechal rozhodca | RMS rozdielu | najväčší | výsledok |")
    print("|---|---|---|---|---|---|---|")
    chyb = 0
    subory = []
    for meno, koder, f, k, arg in VRSTVA2:
        cesta = os.path.join(adresar, meno + ".mp2")
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", f"{HUDBA}:s={f}:d=6", "-ac", str(k),
                        "-ar", str(f), "-c:a", koder] + arg + [cesta], check=True)
        subory.append((meno, cesta, k, f, "mp2float"))
    # MP2 ako vo vysielaní DVB (MPEG-TS) a v Matroske – cez kontajner::otvor a DekoderZvuku
    for kont in ("ts", "mkv"):
        cesta = os.path.join(adresar, f"mp2_48k_192.{kont}")
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", os.path.join(adresar, "mp2_48k_192.mp2"), "-c", "copy",
                        cesta], check=True)
        subory.append((f"mp2_v_{kont}", cesta, 2, 48000, "mp2float"))
    for meno, rezim in (("l1_stereo", 0), ("l1_spojene", 1), ("l1_mono", 3)):
        cesta = os.path.join(adresar, meno + ".mp1")
        tok_vrstvy1(cesta, rezim, semienko=rezim + 7)
        subory.append((meno, cesta, 1 if rezim == 3 else 2, 48000, "mp1float"))
    for meno, cesta, k, f, dek in subory:
        ref_cesta, nas_cesta = cesta + ".ff.f32", cesta + ".nas.f32"
        # vrstva I: formát sa nerozpozná z prípony – demuxer MPEG audio výslovne
        vstup = ["-f", "mp3"] if dek == "mp1float" else []
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-c:a", dek] + vstup + ["-i", cesta, "-f", "f32le", ref_cesta],
                       check=True, stderr=subprocess.DEVNULL)
        if meno.startswith("mp2_v_"):
            r = subprocess.run([program, "--kontajner", cesta, nas_cesta], capture_output=True, text=True)
            kod, m = r.returncode, dict((a, int(b)) for a, b in re.findall(r"(\w+)=(\d+)", r.stdout))
        else:
            kod, m = dekoduj(program, cesta, nas_cesta)
        nas = np.fromfile(nas_cesta, dtype="<f4").astype(np.float64)
        ref = np.fromfile(ref_cesta, dtype="<f4").astype(np.float64)
        rms, mx, vynechal = porovnaj_s_rozhodcom(nas, ref, k)
        ok = kod == 0 and m.get("chyb", 1) == 0 and vynechal is not None and rms < HRANICA_RMS and mx <= HRANICA_MAX
        chyb += not ok
        print(f"| {meno} | {k} / {f / 1000:g} | {len(nas) // k} | {vynechal} | {rms:.2e} | {mx:.2e} | "
              f"{'✅' if ok else '❌'} |")
    return chyb


def skusobne_toky(program, adresar, prac):
    """Referenčné PCM 16 bitov; referenčný dekodér nevydá posledné 1–2 rámce (nevidí za nimi ďalšiu hlavičku),
    preto sa porovná dĺžka referencie a zarovnanie sa overí hľadaním posunu (musí vyjsť 0)."""
    mena = sorted(m[:-4] for m in os.listdir(adresar) if m.endswith(".bit")
                  and os.path.exists(os.path.join(adresar, m[:-4] + ".pcm")))
    print("\n| skúšobný tok ISO | kanálov / kHz | vzoriek (ref.) | formát ref. | posun | RMS rozdielu "
          "| najväčší (LSB po zaokrúhlení) | intenzita | výsledok |")
    print("|---|---|---|---|---|---|---|---|---|")
    chyb = 0
    for meno in mena:
        nas_cesta = os.path.join(prac, meno + ".nas.f32")
        kod, m = dekoduj(program, os.path.join(adresar, meno + ".bit"), nas_cesta)
        # referencia je 16-bitová, teda orezaná na plný rozsah – náš výstup sa oreže rovnako
        nas = np.clip(np.fromfile(nas_cesta, dtype="<f4").astype(np.float64), -1.0, 32767 / 32768)
        surove = open(os.path.join(adresar, meno + ".pcm"), "rb").read()
        k = max(m.get("kanalov", 1), 1)
        # rámce na začiatku bez dostatku zásobníka bitov referenčný dekodér nevydá (my ticho)
        pripustny_posun = m.get("nedostatok_zasobnika", 0) * 1152 * k
        najlepsi = None
        for fmt in ("<i2", ">i2"):
            ref = np.frombuffer(surove[: len(surove) // 2 * 2], dtype=fmt).astype(np.float64) / 32768.0
            # posun sa hľadá na úvodnom úseku (≈ 4 s), štatistika potom na celej spoločnej dĺžke
            for posun in range(-2 * 1152 * k, 2 * 1152 * k + 1, k):
                a, b = nas[max(0, posun):], ref[max(0, -posun):]
                n = min(len(a), len(b), 200_000)
                if n < 1152:
                    continue
                rms, _, _ = presnost(a[:n], b[:n])
                if najlepsi is None or rms < najlepsi[0]:
                    najlepsi = (rms, fmt, posun, ref)
        if najlepsi is not None:
            _, fmt, posun, ref = najlepsi
            a, b = nas[max(0, posun):], ref[max(0, -posun):]
            n = min(len(a), len(b))
            najlepsi = (presnost(a[:n], b[:n])[0], fmt, posun, a[:n], b[:n], len(ref))
        if najlepsi is None:
            print(f"| {meno} | – | {len(nas)} | – | – | – | – | – | ❌ |")
            chyb += 1
            continue
        rms, fmt, posun, a, b, n_ref = najlepsi
        lsb = int(np.abs(np.clip(np.round(a * 32768.0), -32768, 32767) - np.round(b * 32768.0)).max())
        kratsia = len(nas) - n_ref
        # RMS proti 16-bitovej referencii = zaokrúhlenie referencie (≈ 2^-15/√12) + naša chyba
        kratsia -= posun
        ok = (kod == 0 and m.get("chyb", 1) == 0 and posun in (0, pripustny_posun) and 0 <= kratsia <= 2 * 1152 * k
              and lsb <= 1 and rms < HRANICA_RMS * 1.1)
        chyb += not ok
        print(f"| {meno} | {k} / {m.get('frekvencia', 0) / 1000:g} | {len(nas)} ({n_ref}) | "
              f"{'LE' if fmt == '<i2' else 'BE'} | {posun} | {rms:.2e} | {lsb} | {m.get('intenzitne_pasma', 0)} | "
              f"{'✅' if ok else '❌'} |")
    if not mena:
        print("| (žiadne toky) | | | | | | | | ❌ |")
        chyb += 1
    return chyb


# (meno, frekvencia, druh, kanálov)
LSF_TOKY = [
    ("lsf_skalovacie_24k", 24000, "skalovacie", 2),
    ("lsf_skalovacie_16k_mono", 16000, "skalovacie", 1),
    ("lsf25_skalovacie_8k", 8000, "skalovacie", 2),
    ("lsf_intenzita_16k", 16000, "intenzita", 2),
    ("lsf25_intenzita_12k", 12000, "intenzita", 2),
    ("lsf_intenzita_ms_24k", 24000, "intenzita_ms", 2),
    ("lsf25_intenzita_ms_8k", 8000, "intenzita_ms", 2),
]


def lsf(program, adresar):
    print("\n| vrstva III LSF / 2.5 (syntax [S]) | granúl | presne na konci part2_3_length | rámcov vynechal "
          "rozhodca | RMS rozdielu | najväčší | pokrytie | výsledok |")
    print("|---|---|---|---|---|---|---|---|")
    chyb = 0
    for i, (meno, f, druh, k) in enumerate(LSF_TOKY):
        cesta = os.path.join(adresar, meno + ".mp3")
        tok_lsf3(cesta, f, druh, k, semienko=40 + i)
        ref_cesta, nas_cesta = cesta + ".ff.f32", cesta + ".nas.f32"
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-c:a", "mp3float", "-f", "mp3", "-i", cesta, "-f", "f32le",
                        ref_cesta], check=True, stderr=subprocess.DEVNULL)
        kod, m = dekoduj(program, cesta, nas_cesta)
        nas = np.fromfile(nas_cesta, dtype="<f4").astype(np.float64)
        ref = np.fromfile(ref_cesta, dtype="<f4").astype(np.float64)
        rms, mx, vynechal = porovnaj_s_rozhodcom(nas, ref, k)
        spotreba = m.get("granul", 0) > 0 and m.get("presne") == m.get("granul") and m.get("chyba_skalovacich") == 0
        ok = kod == 0 and m.get("chyb", 1) == 0 and spotreba and vynechal is not None and rms < HRANICA_RMS \
            and mx <= HRANICA_MAX
        chyb += not ok
        pokrytie = ", ".join(f"{x} {m[x]}" for x in ("kratke", "zmiesane", "start_stop", "preflag", "ms",
                                                     "intenzitne_pasma") if m.get(x))
        print(f"| {meno} | {m.get('granul')} | {m.get('presne')}/{m.get('granul')} | {vynechal} | {rms:.2e} | "
              f"{mx:.2e} | {pokrytie} | {'✅' if ok else '❌'} |")
    return chyb


def main():
    program, adresar = sys.argv[1], sys.argv[2]
    os.makedirs(adresar, exist_ok=True)
    chyb = varianty(program, adresar)
    chyb += bezmedzerovo(program, adresar)
    chyb += vrstvy12(program, adresar)
    chyb += lsf(program, adresar)
    if len(sys.argv) > 3:
        chyb += skusobne_toky(program, sys.argv[3], adresar)
    sys.exit(1 if chyb else 0)


if __name__ == "__main__":
    main()
