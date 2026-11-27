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
5. **Skúšobné toky ISO** (voliteľne, adresár s pármi `<meno>.bit` + `<meno>.pcm`): referenčný výstup 16 bitov;
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


def porovnaj_s_rozhodcom(nas, ref, k):
    """(RMS, najväčší, posun v rámcoch po k kanáloch): posun len o rámce, ktoré rozhodca na začiatku vynechal."""
    if len(nas) == len(ref):
        rms, mx, _ = presnost(nas, ref)
        return rms, mx, 0
    rozdiel = len(nas) - len(ref)
    for ramec in (1152, 384):
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


def main():
    program, adresar = sys.argv[1], sys.argv[2]
    os.makedirs(adresar, exist_ok=True)
    chyb = varianty(program, adresar)
    chyb += bezmedzerovo(program, adresar)
    chyb += vrstvy12(program, adresar)
    if len(sys.argv) > 3:
        chyb += skusobne_toky(program, sys.argv[3], adresar)
    sys.exit(1 if chyb else 0)


if __name__ == "__main__":
    main()
