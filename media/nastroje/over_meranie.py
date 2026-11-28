#!/usr/bin/env python3
"""Rozhodca samomerania `media_meranie` (R6, R307) – prúd D.

Použitie: over_meranie.py <priečinok súborov> <media_meranie> [<meno>=<spúšťač a binárka> ...]

1. Vytvorí skúšobné súbory (`meranie_subory.py`), ak priečinok neexistuje.
2. Pôvodná binárka (tento procesor) zmeria každý súbor naplno a v tempe 5 s: `stav=ok`, `podtecenia=0`,
   budenia ≈ 1000 / dávka (± 1) a **dĺžka výstupu presne ako dekódovanie FFmpeg** (nezávislý rozhodca).
3. Ďalšie binárky (napr. `aarch64="qemu-aarch64 -cpu cortex-a53 cesta"`) dekódujú ten istý súbor bez tempa –
   **odtlačok BLAKE3 výstupu musí byť rovnaký na každej platforme** (R307: vlastná matematika, bez FMA).
Výstup: tabuľka v Markdown pre súhrn CI; návratový kód 1 pri akejkoľvek nezhode.
"""

import os
import shlex
import subprocess
import sys


def riadky(vystup):
    """{kľúčové slovo: {pole: hodnota}} z výstupu media_meranie."""
    r = {}
    for l in vystup.splitlines():
        casti = l.split(" ")
        if not casti or not casti[0].isupper():
            continue
        if casti[0] == "CHYBA":
            r.setdefault("CHYBA", []).append(l[6:])
            continue
        polia = {}
        posledne = None
        for c in casti[1:]:
            if "=" in c:
                k, v = c.split("=", 1)
                polia[k] = v
                posledne = k
            elif posledne:  # hodnota s medzerou (napr. kodek=MPEG audio vrstva 2)
                polia[posledne] += " " + c
        r[casti[0]] = polia
    return r


def spusti(prikaz, subor, *volby):
    p = subprocess.run([*prikaz, *volby, subor], capture_output=True, text=True, timeout=1800)
    return p.returncode, riadky(p.stdout), p.stdout + p.stderr


def vzoriek_ffmpeg(subor):
    kanalov = int(
        subprocess.run(
            ["ffprobe", "-v", "error", "-select_streams", "a:0", "-show_entries", "stream=channels", "-of", "csv=p=0", subor],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
    )
    pcm = subprocess.run(["ffmpeg", "-v", "error", "-i", subor, "-map", "0:a:0", "-f", "s16le", "-"], capture_output=True, check=True)
    return len(pcm.stdout) // (2 * kanalov)


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    priecinok, binarka = sys.argv[1], sys.argv[2]
    ine = []
    for a in sys.argv[3:]:
        meno, prikaz = a.split("=", 1)
        ine.append((meno, shlex.split(prikaz)))
    if not os.path.isdir(priecinok):
        tu = os.path.dirname(os.path.abspath(__file__))
        subprocess.run([sys.executable, os.path.join(tu, "meranie_subory.py"), priecinok], check=True)

    davka = 250
    chyby = 0
    print("| súbor | kodek | × reálny čas | % jadra naplno | % jadra v tempe | budenia/s | dĺžka = FFmpeg | odtlačok " + " ".join(f"= {m}" for m, _ in ine) + " |")
    print("|---|---|---|---|---|---|---|---|")
    for m in sorted(os.listdir(priecinok)):
        s = os.path.join(priecinok, m)
        kod, r, text = spusti([binarka], s, "--tempo", "5", "--davka", str(davka))
        ok = kod == 0 and r.get("VYSLEDOK", {}).get("stav") == "ok"
        hl, ry, od, te = r.get("MEDIA_MERANIE", {}), r.get("RYCHLOST", {}), r.get("ODTLACOK", {}), r.get("TEMPO", {})
        ocakavane = vzoriek_ffmpeg(s)
        dlzka_ok = od.get("vzoriek") == str(ocakavane)
        budenia = float(te.get("budenia_s", "nan")) if te.get("budenia_s", "-") != "-" else float("nan")
        tempo_ok = te.get("podtecenia") == "0" and abs(budenia - 1000 / davka) <= 1.0
        zhody = []
        for meno, prikaz in ine:
            k2, r2, t2 = spusti(prikaz, s, "--tempo", "0", "--opakovani", "1")
            zhoda = k2 == 0 and r2.get("ODTLACOK", {}).get("blake3") == od.get("blake3") and r2.get("ODTLACOK", {}).get("vzoriek") == od.get("vzoriek")
            if not zhoda:
                print(f"<!-- {meno} {m}:\n{t2}\n-->")
            zhody.append("✅" if zhoda else "❌")
            ok &= zhoda
        ok &= dlzka_ok and tempo_ok
        if not ok:
            chyby += 1
            print(f"<!-- {m}:\n{text}\n-->")
        print(
            f"| {m} | {hl.get('kodek', '-')} {hl.get('kanalov', '-')} k | {ry.get('x_rt', '-')} | {ry.get('jadro_pct', '-')} | "
            f"{te.get('jadro_pct', '-')} | {te.get('budenia_s', '-')} | {'✅' if dlzka_ok else '❌'} {od.get('vzoriek', '-')} | "
            f"{(od.get('blake3') or '-')[:16]}… {' '.join(zhody)} |"
        )
    print()
    print("Čísla × reálny čas a % jadra sú z virtuálneho stroja CI (nie z dosky) – platí až samomeranie na krabičkách.")
    print("✅ všetko sedí" if chyby == 0 else f"❌ nezhoda v {chyby} súboroch")
    sys.exit(1 if chyby else 0)


if __name__ == "__main__":
    main()
