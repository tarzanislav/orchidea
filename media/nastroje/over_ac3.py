#!/usr/bin/env python3
"""Rozhodca AC-3 (L9): presnosť dekódovania proti nezávislému dekodéru `ac3` FFmpeg (plávajúca čiarka).

AC-3 (ATSC A/52:2018) výstup bit po bite neurčuje a dither pri bap = 0 je náhodný (§7.3.4: „any reasonably random
sequence“) – dva správne dekodéry sa líšia v šume ditheru. Preto:

1. **Presnosť bez ditheru:** nástroj `media_ac3 --uprav` v toku z kodéra FFmpeg vynuluje bity dithflag
   a prepočíta crc1 a crc2 (§7.10.1) – dĺžka ani poloha ostatných polí sa nemení, tok ostáva platný. Náš výstup
   a výstup rozhodcu musia mať rovnakú dĺžku a presnosť ako v ISO/IEC 11172-4 (meradlo rozhodnutia vlastníka
   z L8): RMS rozdielu < 2^-15/√12 a najväčší rozdiel ≤ 2^-14 pri plnom rozsahu ±1,0.
2. **Krátke bloky (blksw):** kodér FFmpeg prepnutie bloku nepoužíva – `--uprav` nastaví blksw v každom 4. bloku
   všetkým kanálom (koeficienty sa vyložia ako dve transformácie 256, §7.9.4.2) – rovnaká presnosť.
3. **Dither:** zložka ditheru (výstup pôvodného toku − výstup toku bez dithflag) má po kanáloch rovnakú
   efektívnu hodnotu ako u rozhodcu (0,8–1,25; náhodná zložka – efektívnu hodnotu určuje málo košov s najmenším
   exponentom) – overí mierku ±0,707 a dither po rozložení väzby (§7.3.4).
4. **Nezávislosť kanálov:** v toku 2/0 s krátkymi blokmi len v jednom kanáli sa výstup kanála, ktorého bity sa
   nezmenili, nesmie zmeniť (u nás bit po bite). Rozhodca túto vlastnosť pri prvom krátkom bloku toku 2/0 poruší
   (zmení výstup druhého kanála – aj dekodér `ac3_fixed`); taký blok sa porovná s výstupom rozhodcu z pôvodného
   toku a vypíše sa ako odchýlka rozhodcu.
5. **Kontajnery:** AC-3 v Matroske, MP4 a MPEG-TS cez `kontajner::otvor` a `DekoderZvuku` (`media_mpa
   --kontajner`) – dĺžka presne ako rozhodca a rovnaká presnosť.
6. **Skúšobné súbory FATE** (voliteľne, adresár so súbormi `*.ac3` – len údaje, skutočné kodéry: prepnutie
   bloku, delta rozdelenia bitov, dynrng …): rovnaká presnosť bez ditheru.

Použitie: `over_ac3.py <media_ac3> <media_mpa> <adresár> [adresár FATE]`
"""
import math
import os
import re
import subprocess
import sys

import numpy as np

PROGRAM, KONTAJNER, PRAC = sys.argv[1], sys.argv[2], sys.argv[3]
FATE = sys.argv[4] if len(sys.argv) > 4 else None
os.makedirs(PRAC, exist_ok=True)
HRANICA_RMS = 2 ** -15 / math.sqrt(12)
HRANICA_MAX = 2 ** -14

# kanály s rôznym obsahom: tóny, kĺzavý tón a údery šumu (prechody → zmeny stratégie exponentov)
KANALY = [
    "0.30*sin(2*PI*220*t)+0.10*sin(2*PI*3300*t)+0.30*(random(1)-0.5)*exp(-30*mod(t,0.4))",
    "0.25*sin(2*PI*330*t+1)+0.12*sin(2*PI*(1000+800*t)*t)+0.25*(random(2)-0.5)*exp(-20*mod(t+0.13,0.55))",
    "0.30*sin(2*PI*440*t)+0.20*(random(3)-0.5)*exp(-25*mod(t+0.2,0.3))",
    "0.20*sin(2*PI*80*t)+0.10*sin(2*PI*55*t)",
    "0.20*sin(2*PI*550*t)+0.15*(random(4)-0.5)",
    "0.18*sin(2*PI*660*t+2)+0.20*(random(5)-0.5)*exp(-15*mod(t+0.3,0.7))",
]
POCET = {"mono": 1, "stereo": 2, "3.0": 3, "3.0(back)": 3, "4.0": 4, "quad(side)": 4, "5.0(side)": 5,
         "5.1(side)": 6, "2.1": 3}
# (meno, frekvencia, rozloženie, tok, ďalšie argumenty kodéra)
VARIANTY = [
    ("mono_48_96", 48000, "mono", "96k", []),
    ("stereo_48_192", 48000, "stereo", "192k", []),
    ("stereo_441_128", 44100, "stereo", "128k", []),
    ("stereo_32_96", 32000, "stereo", "96k", []),
    ("stereo_48_64", 48000, "stereo", "64k", []),
    ("stereo_bez_vazby", 48000, "stereo", "192k", ["-channel_coupling", "0"]),
    ("stereo_bez_rematrixu", 48000, "stereo", "160k", ["-stereo_rematrixing", "0"]),
    ("s30_256", 48000, "3.0", "256k", []),
    ("s21_256", 48000, "3.0(back)", "256k", []),
    ("s31_320", 48000, "4.0", "320k", []),
    ("s22_320", 44100, "quad(side)", "320k", []),
    ("s32_384", 48000, "5.0(side)", "384k", []),
    ("s51_448", 48000, "5.1(side)", "448k", []),
    ("s51_192_32k", 32000, "5.1(side)", "192k", []),
    ("s51_640", 48000, "5.1(side)", "640k", []),
    ("s21lfe_192", 44100, "2.1", "192k", []),
    ("bsi_polia", 48000, "stereo", "192k", ["-dialnorm", "-17", "-dsur_mode", "on", "-mixing_level", "100",
                                            "-room_type", "small", "-copyright", "1", "-original", "0"]),
    ("priloha_d_xbsi", 48000, "5.1(side)", "384k", ["-dmix_mode", "ltrt", "-ltrt_cmixlev", "0.595",
                                                    "-loro_surmixlev", "0.5", "-dsurex_mode", "on",
                                                    "-ad_conv_type", "hdcd"]),
]
CHYBY = []


def spusti(*argumenty):
    r = subprocess.run(list(argumenty), capture_output=True, text=True)
    return r.returncode, r.stdout, r.stderr


def ffmpeg_dekoduj(cesta, von, dekoder=None, volby=()):
    arg = (["ffmpeg", "-v", "error", "-y"] + (["-c:a", dekoder] if dekoder else []) + list(volby)
           + ["-i", cesta, "-f", "f32le", von])
    subprocess.run(arg, check=True)
    k = int(subprocess.run(["ffprobe", "-v", "error", "-select_streams", "a:0", "-show_entries", "stream=channels",
                            "-of", "default=nk=1:nw=1", cesta], capture_output=True, text=True,
                           check=True).stdout.split()[0])
    return np.fromfile(von, dtype="<f4").astype(np.float64).reshape(-1, k)


def nas_dekoduj(cesta, von, kanalov, *volby):
    rc, out, err = spusti(PROGRAM, *volby, cesta, von)
    riadok = next((r for r in out.splitlines() if r.startswith("AC3 ")), "")
    stat = dict(re.findall(r"(\w+)=(\d+)", riadok))
    stat["_bloky"] = [r.split(" ", 2)[2] for r in out.splitlines() if r.startswith("BLOK ")]
    ok = rc == 0 and "chyb=0" in out
    d = np.fromfile(von, dtype="<f4").astype(np.float64) if os.path.exists(von) else np.zeros(0)
    return ok, d.reshape(-1, kanalov) if len(d) % kanalov == 0 else d.reshape(-1, 1), stat, err.strip()[:200]


def presnost(nas, ref):
    if nas.shape != ref.shape or nas.size == 0:
        return math.inf, math.inf
    d = nas - ref
    return float(np.sqrt(np.mean(d * d))), float(np.abs(d).max())


def uprav(vstup, vystup, *volby):
    rc, out, err = spusti(PROGRAM, "--uprav", vstup, vystup, *volby)
    if rc != 0:
        raise RuntimeError(f"--uprav {vstup}: {err.strip()[:200]}")
    return [tuple(map(int, r.split()[1:])) for r in out.splitlines() if r.startswith("BLKSW")]


def zakoduj(meno, f, rozlozenie, tok, arg):
    n = POCET[rozlozenie]
    vyraz = "|".join(KANALY[:n])
    cesta = os.path.join(PRAC, meno + ".ac3")
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i",
                    f"aevalsrc='{vyraz}':c={rozlozenie}:s={f}:d=4", "-c:a", "ac3", "-b:a", tok] + arg + [cesta],
                   check=True)
    return cesta, n


def stlpec(rms, mx):
    return f"{rms:.1e} / {mx:.1e}"


def varianty():
    print("| variant | rámcov | kanálov | bez ditheru (RMS / max) | krátke bloky (RMS / max) | dither náš / rozhodca"
          " | pokrytie | výsledok |")
    print("|---|---|---|---|---|---|---|---|")
    spolu = {}
    for meno, f, rozlozenie, tok, arg in VARIANTY:
        povodny, n = zakoduj(meno, f, rozlozenie, tok, arg)
        bez = povodny[:-4] + ".bez.ac3"
        kratke = povodny[:-4] + ".kratke.ac3"
        uprav(povodny, bez, "--dither", "0")
        uprav(povodny, kratke, "--dither", "0", "--blksw-kazdy", "4", "--blksw-spolu")
        vysl = {}
        for druh, cesta in (("povodny", povodny), ("bez", bez), ("kratke", kratke)):
            ref = ffmpeg_dekoduj(cesta, cesta + ".ff.f32")
            ok, nas, stat, chyba = nas_dekoduj(cesta, cesta + ".nas.f32", n)
            vysl[druh] = (ref, nas, ok, stat, chyba)
        rms_b, max_b = presnost(vysl["bez"][1], vysl["bez"][0])
        rms_k, max_k = presnost(vysl["kratke"][1], vysl["kratke"][0])
        # dither: efektívna hodnota zložky ditheru po kanáloch
        pomery = []
        dither_ok = True
        if vysl["povodny"][1].shape == vysl["bez"][1].shape == vysl["povodny"][0].shape:
            d_nas = vysl["povodny"][1] - vysl["bez"][1]
            d_ref = vysl["povodny"][0] - vysl["bez"][0]
            for c in range(n):
                r_ref = float(np.sqrt(np.mean(d_ref[:, c] ** 2)))
                r_nas = float(np.sqrt(np.mean(d_nas[:, c] ** 2)))
                if r_ref > 1e-6:
                    pomery.append(r_nas / r_ref)
                    dither_ok &= 0.8 <= r_nas / r_ref <= 1.25
                else:
                    dither_ok &= r_nas <= 1e-6
        else:
            dither_ok = False
        stat = vysl["kratke"][3]
        for k, v in vysl["bez"][3].items():
            if not k.startswith("_"):
                spolu[k] = spolu.get(k, 0) + int(v)
        spolu["kratke"] = spolu.get("kratke", 0) + int(stat.get("kratke", 0))
        spolu["dither"] = spolu.get("dither", 0) + int(vysl["povodny"][3].get("dither", 0))
        ok = (all(v[2] for v in vysl.values()) and rms_b < HRANICA_RMS and max_b <= HRANICA_MAX
              and rms_k < HRANICA_RMS and max_k <= HRANICA_MAX and dither_ok)
        if not ok:
            CHYBY.append(f"{meno}: bez ditheru {stlpec(rms_b, max_b)}, krátke {stlpec(rms_k, max_k)}, dither "
                         f"{pomery}, {[v[4] for v in vysl.values() if v[4]]}")
        pokrytie = (f"väzba {stat.get('vazba', 0)}, rematrix {stat.get('rematrix', 0)}, D15/D25/D45 "
                    f"{stat.get('d15', 0)}/{stat.get('d25', 0)}/{stat.get('d45', 0)}, LFE {stat.get('lfe', 0)}, "
                    f"krátke {stat.get('kratke', 0)}")
        dith = ", ".join(f"{p:.3f}" for p in pomery) if pomery else "–"
        print(f"| {meno} | {stat.get('ramcov', 0)} | {n} | {stlpec(rms_b, max_b)} | {stlpec(rms_k, max_k)} | {dith} | "
              f"{pokrytie} | {'✅' if ok else '❌'} |")
    return spolu


def nezavislost():
    """Kanál s nezmenenými bitmi musí dať rovnaký výstup (tok 2/0, krátke bloky len v niektorých kanáloch)."""
    meno, f, rozlozenie, tok, arg = "nezavislost_2_0", 44100, "stereo", "128k", []
    povodny, n = zakoduj(meno, f, rozlozenie, tok, arg)
    bez = povodny[:-4] + ".bez.ac3"
    zvlast = povodny[:-4] + ".zvlast.ac3"
    uprav(povodny, bez, "--dither", "0")
    kratke = set(uprav(povodny, zvlast, "--dither", "0", "--blksw-kazdy", "5"))
    ref_bez = ffmpeg_dekoduj(bez, bez + ".ff.f32")
    ref_zv = ffmpeg_dekoduj(zvlast, zvlast + ".ff.f32")
    _, nas_bez, _, _ = nas_dekoduj(bez, bez + ".nas.f32", n)
    ok_zv, nas_zv, _, _ = nas_dekoduj(zvlast, zvlast + ".nas.f32", n)
    blokov = len(nas_zv) // 256
    nase_porusenia, odchylky_rozhodcu, ine = 0, [], []
    for g in range(blokov):
        for c in range(n):
            s = slice(g * 256, (g + 1) * 256)
            nezmeneny = (g, c) not in kratke and (g - 1, c) not in kratke
            if nezmeneny and not np.array_equal(nas_zv[s, c], nas_bez[s, c]):
                nase_porusenia += 1
            rms, mx = presnost(nas_zv[s, c], ref_zv[s, c])
            if mx > HRANICA_MAX:
                # rozhodca zmenil kanál s nezmenenými bitmi: porovná sa s jeho výstupom z pôvodného toku
                rms0, mx0 = presnost(nas_zv[s, c], ref_bez[s, c])
                if nezmeneny and mx0 <= HRANICA_MAX:
                    odchylky_rozhodcu.append((g, c))
                else:
                    ine.append((g, c, mx))
    ok = ok_zv and nase_porusenia == 0 and not ine and len(odchylky_rozhodcu) <= 2
    if not ok:
        CHYBY.append(f"nezávislosť kanálov: naše porušenia {nase_porusenia}, iné rozdiely {ine[:5]}")
    print(f"\n| nezávislosť kanálov (2/0, krátke bloky v {len(kratke)} blokoch jedného kanála) | výsledok |")
    print("|---|---|")
    print(f"| náš výstup kanála s nezmenenými bitmi | {'bit po bite rovnaký' if nase_porusenia == 0 else f'{nase_porusenia} porušení'} "
          f"| {'✅' if ok else '❌'} |")
    print(f"| rozhodca zmenil kanál s nezmenenými bitmi (blok, kanál) | {odchylky_rozhodcu or 'nie'} – náš výstup tam "
          f"sedí s rozhodcom z pôvodného toku | ✅ |")


def kontajnery():
    print("\n| kontajner (`kontajner::otvor` + `DekoderZvuku`) | vzoriek | vzoriek rozhodcu | RMS / max | výsledok |")
    print("|---|---|---|---|---|")
    for meno in ("stereo_48_192", "s51_448"):
        bez = os.path.join(PRAC, meno + ".bez.ac3")
        for kont in ("mkv", "mp4", "ts"):
            cesta = bez[:-4] + "." + kont
            subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", bez, "-c", "copy", cesta], check=True)
            ref = ffmpeg_dekoduj(cesta, cesta + ".ff.f32")
            rc, out, err = spusti(KONTAJNER, "--kontajner", cesta, cesta + ".nas.f32")
            nas = np.fromfile(cesta + ".nas.f32", dtype="<f4").astype(np.float64) if rc == 0 else np.zeros(0)
            nas = nas.reshape(-1, ref.shape[1]) if nas.size % ref.shape[1] == 0 else nas.reshape(-1, 1)
            rms, mx = presnost(nas, ref)
            ok = rc == 0 and "chyb=0" in out and rms < HRANICA_RMS and mx <= HRANICA_MAX
            if not ok:
                CHYBY.append(f"{meno}.{kont}: {len(nas)} / {len(ref)} vzoriek, {stlpec(rms, mx)} {err.strip()[:200]}")
            print(f"| {meno}.{kont} | {len(nas)} | {len(ref)} | {stlpec(rms, mx)} | {'✅' if ok else '❌'} |")


def fate():
    subory = sorted(s for s in os.listdir(FATE) if s.endswith(".ac3")) if FATE and os.path.isdir(FATE) else []
    if not subory:
        print("\n⏭ skúšobné súbory FATE AC-3 nie sú k dispozícii")
        return
    print("\n| súbor FATE (bez ditheru) | rámcov | kanálov | RMS / max | pokrytie | výsledok |")
    print("|---|---|---|---|---|---|")
    for s in subory:
        cesta = os.path.join(FATE, s)
        bez = os.path.join(PRAC, "fate_" + s[:-4] + ".bez.ac3")
        try:
            uprav(cesta, bez, "--dither", "0")
        except RuntimeError as e:
            CHYBY.append(f"FATE {s}: {e}")
            print(f"| {s} | – | – | – | {e} | ❌ |")
            continue
        ref = ffmpeg_dekoduj(bez, bez + ".ff.f32")
        ok, nas, stat, chyba = nas_dekoduj(bez, bez + ".nas.f32", ref.shape[1], "--bloky")
        if int(stat.get("ramcov", 0)) == 0:
            print(f"| {s} | 0 | {ref.shape[1]} | – | nie AC-3 (bsid > 8 – E-AC-3) | ⏭ |")
            continue
        poznamka = ""
        if len(ref) == len(nas) + 1536 and "neúplný" in chyba:
            # skrátený súbor: posledný rámec je neúplný – rozhodca z neho vydá 1536 vzoriek, my ho vynecháme
            ref = ref[:len(nas)]
            poznamka = " (posledný neúplný rámec vynechaný)"
        rms, mx = presnost(nas, ref)
        dobre = ok and rms < HRANICA_RMS and mx <= HRANICA_MAX
        if not dobre:
            CHYBY.append(f"FATE {s}: {stlpec(rms, mx)}, {len(nas)} / {len(ref)} {chyba}")
            diagnostika(s, bez, nas, ref, stat["_bloky"])
        pokrytie = ", ".join(f"{k} {stat.get(k, 0)}" for k in ("kratke", "vazba", "fazove", "rematrix", "delta",
                                                                 "dynrng", "preskok", "lfe"))
        print(f"| {s} | {stat.get('ramcov', 0)} | {ref.shape[1]} | {stlpec(rms, mx)}{poznamka} | {pokrytie} | "
              f"{'✅' if dobre else '❌'} |")


def diagnostika(meno, bez, nas, ref, bloky):
    """Pri nezhode: bez dynrng na oboch stranách a prvé bloky s rozdielom (príznaky bloku)."""
    ref0 = ffmpeg_dekoduj(bez, bez + ".ff0.f32", volby=["-drc_scale", "0"])
    _, nas0, _, _ = nas_dekoduj(bez, bez + ".nas0.f32", ref.shape[1], "--bez-dynrng")
    n = min(len(nas0), len(ref0))
    rms0, mx0 = presnost(nas0[:n], ref0[:n])
    CHYBY.append(f"  {meno} bez dynrng na oboch stranách: {stlpec(rms0, mx0)}")
    n = min(len(nas), len(ref))
    zle = []
    for g in range(n // 256):
        e = [float(np.abs(nas[g * 256:(g + 1) * 256, c] - ref[g * 256:(g + 1) * 256, c]).max())
             for c in range(ref.shape[1])]
        if max(e) > HRANICA_MAX:
            zle.append((g, e))
    for g, e in zle[:6]:
        CHYBY.append(f"  {meno} blok {g} (rámec {g // 6}): max po kanáloch {['%.1e' % x for x in e]}; "
                     f"{bloky[g] if g < len(bloky) else '?'}")
    CHYBY.append(f"  {meno}: blokov s rozdielom {len(zle)} z {n // 256}")


def main():
    spolu = varianty()
    print("\nPokrytie spolu: " + ", ".join(f"{k} {v}" for k, v in spolu.items() if k not in ("ramcov",)))
    nezavislost()
    kontajnery()
    fate()
    if CHYBY:
        print("\n**Nezhody:**\n")
        for c in CHYBY:
            print(f"- {c}")
        sys.exit(1)


main()
