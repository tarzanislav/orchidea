#!/usr/bin/env python3
"""Skúšobné vektory RFC 8251 (Opus, etapa L7) proti `media_opus`.

Sada: https://opus-codec.org/static/testvectors/opus_testvectors-rfc8251.tar.gz (RFC 8251 §11). Súbor
`testvectorNN.bit` obsahuje pre každý paket dĺžku (32 b, veľký endián), **konečný stav rozsahového kodéra**
(32 b) a bajty paketu; `testvectorNN.dec` je výstup referenčného dekodéra (48 kHz, stereo, 16 b).

Dôkaz:
1. **konečný stav rozsahového dekodéra = stav kodéra** pri každom pakete (bitovo správne dekódovanie, RFC 6716
   §4.1 a nástroj `opus_demo`),
2. dĺžka výstupu ako `.dec` a odstup (SNR) od `.dec` – výstup nie je bitovo predpísaný (RFC 6716 §6 – miera
   kvality nástroja `opus_compare`, ktorý tu nie je); vypíše sa, prah len hrubá kontrola.

Použitie: `over_vektory.py <media_opus> <adresár>`; sadu stiahne, ak v adresári nie je.
"""
import math
import os
import struct
import subprocess
import sys
import tarfile

ZDROJ = "https://opus-codec.org/static/testvectors/opus_testvectors-rfc8251.tar.gz"


def main():
    program, adresar = sys.argv[1], sys.argv[2]
    os.makedirs(adresar, exist_ok=True)
    archiv = os.path.join(adresar, "vektory.tar.gz")
    if not os.path.exists(archiv):
        subprocess.run(["curl", "-fsSL", "--retry", "5", "-o", archiv, ZDROJ], check=True)
    with tarfile.open(archiv) as t:
        t.extractall(adresar, filter="data")
    vektory = sorted(
        os.path.join(k, s) for k, _, subory in os.walk(adresar) for s in subory if s.endswith(".bit"))
    print("| vektor | paketov | konečný stav | dĺžka ako .dec | SNR voči .dec | výsledok |")
    print("|---|---|---|---|---|---|")
    chyb = 0
    for bit in vektory:
        meno = os.path.basename(bit)[:-4]
        d = open(bit, "rb").read()
        pakety, stavy, i = [], [], 0
        while i + 8 <= len(d):
            n, rng = struct.unpack(">II", d[i:i + 8])
            pakety.append(d[i + 8:i + 8 + n])
            stavy.append(rng)
            i += 8 + n
        pak = os.path.join(adresar, meno + ".opuspak")
        with open(pak, "wb") as f:
            for p in pakety:
                f.write(struct.pack("<H", len(p)) + p)
        surove = os.path.join(adresar, meno + ".f32")
        r = subprocess.run([program, pak, surove, "--kanaly=2"], capture_output=True, text=True)
        if r.returncode != 0:
            print(f"| {meno} | {len(pakety)} | – | – | – | ❌ {r.stdout.strip()[-200:]} |")
            chyb += 1
            continue
        nase_stavy = [int(x.split("rng=")[1][:8], 16) for x in r.stdout.splitlines() if x.startswith("PAKET")]
        platne = [(a, b) for a, b, p in zip(nase_stavy, stavy, pakety) if p]
        zhoda = sum(a == b for a, b in platne)
        prva = next((k for k, (a, b, p) in enumerate(zip(nase_stavy, stavy, pakety)) if p and a != b), None)
        nase = open(surove, "rb").read()
        nase = struct.unpack(f"<{len(nase) // 4}f", nase)
        ref_cesta = bit[:-4] + ".dec"
        ref = open(ref_cesta, "rb").read()
        ref = [v / 32768.0 for v in struct.unpack(f"<{len(ref) // 2}h", ref)]
        m = min(len(ref), len(nase))
        sig = sum(v * v for v in ref[:m])
        e = sum((a - min(max(b, -1.0), 32767 / 32768)) ** 2 for a, b in zip(ref[:m], nase[:m]))
        snr = 10 * math.log10(sig / e) if e > 0 else float("inf")
        ok = zhoda == len(platne) and len(nase) == len(ref) and snr >= 10
        chyb += not ok
        stav = f"{zhoda}/{len(platne)}" + ("" if prva is None else f" (prvý rozdiel v pakete {prva})")
        dlzka = "áno" if len(nase) == len(ref) else f"{len(nase) // 2} ≠ {len(ref) // 2}"
        print(f"| {meno} | {len(pakety)} | {stav} | {dlzka} | {snr:.1f} dB | {'✅' if ok else '❌'} |")
    if not vektory:
        print("žiadne vektory")
        chyb += 1
    sys.exit(1 if chyb else 0)


if __name__ == "__main__":
    main()
