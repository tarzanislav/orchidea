#!/usr/bin/env python3
"""Generátor tabuliek AC-3 (`media/src/ac3/tabulky.rs`) – etapa L9.

Zdroj (rozhodnutie vlastníka 27. 11. 2026, #19): text normy **ATSC A/52:2018** (Digital Audio Compression
Standard), nie kód iných dekodérov. Text sa číta nástrojom `pdftotext -layout`; pri každej tabuľke je v kóde číslo
tabuľky normy. Kontroly (každá tabuľka, ktorú text opisuje aj inak, sa s tým porovná):
- tabuľka 5.18 (veľkosť rámca) – počet riadkov 38, nominálny tok v kb/s = slová · 16 · fs / 1536 pri 48 a 32 kHz,
  pri 44,1 kHz nepárne kódy o 1 slovo viac;
- tabuľka 7.34 (5/8 rámca) = vzorec §7.10.1 `(framesize >> 1) + (framesize >> 3)` z tabuľky 5.18;
- tabuľka 7.12 (pásma) súvislá a v zhode s tabuľkou 7.13 (masktab);
- tabuľky 7.19–7.23 (kvantizéry) = (2·kód − (úrovne − 1)) / úrovne;
- tabuľka 7.24 (podpásma väzby) = 37 + 12·sb … 48 + 12·sb (§7.4.2);
- tabuľky 7.25–7.28 (pásma rematrixu) – hranice 13, 25, 37, 61 a koniec 252 / 36 + 12·cplbegf;
- tabuľka 7.33 (okno) – text ju uvádza na 5 desatinných miest. Okno sa vypočíta ako odvodené Kaiser–Besselovo
  (KBD, α = 5 – hypotéza z publikácie Fielder a kol., AES 1996, nie z textu normy) a **všetkých 256 hodnôt musí
  po zaokrúhlení na 5 desatinných miest presne sedieť s tabuľkou** (iné α nesedí); do kódu ide presná hodnota.

Použitie: `ac3_tabulky.py <A52-2018.pdf> > media/src/ac3/tabulky.rs`
PDF sa do repozitára nedáva (licencia ATSC); generátor číta len čísla tabuliek.
"""

import math
import re
import subprocess
import sys
from fractions import Fraction


def text(pdf):
    return subprocess.run(["pdftotext", "-layout", pdf, "-"], check=True, capture_output=True,
                          text=True).stdout.splitlines()


RIADKY = text(sys.argv[1])
ZAHLAVIE = re.compile(r"^ATSCA/52|^\s*\d+\s*$")


def tabulka(cislo):
    """Riadky tela tabuľky `cislo` (posledný výskyt nadpisu – prvý je v zozname tabuliek)."""
    nadpis = re.compile(r"^\s+Table " + re.escape(cislo) + r"\b")
    zaciatky = [i for i, r in enumerate(RIADKY) if nadpis.match(r)]
    assert zaciatky, f"tabuľka {cislo} nenájdená"
    i = zaciatky[-1] + 1
    telo = []
    while i < len(RIADKY):
        r = RIADKY[i]
        if re.match(r"^\s*Table \d+\.\d+ ", r) or re.match(r"^\d+\.\d+(\.\d+)*\s+\S", r):
            break
        if not ZAHLAVIE.match(r):
            telo.append(r)
        i += 1
    return telo


def cisla(r):
    return [int(x, 16) if x.startswith("0x") else int(x) for x in re.findall(r"0x[0-9a-fA-F]+|(?<![.\d])\d+(?![.\d])", r)]


# --- tabuľka 5.18: veľkosť rámca v 16-bitových slovách; stĺpce 32, 44,1, 48 kHz ---
ramce = []
for r in tabulka("5.18"):
    m = re.match(r"^‘([01]{6})’\s+\((\d+)\)\s+(\d+) kbps\s+(\d+)\s+(\d+)\s+(\d+)\s*$", r)
    if m:
        kod, polovica, tok, s32, s44, s48 = int(m[1], 2), *map(int, m.groups()[1:])
        assert kod == len(ramce) and polovica == kod // 2, r
        assert s48 * 16 * 48 // 1536 == tok and s32 * 16 * 32 // 1536 == tok, r
        assert s44 == int(tok * 1536 / 16 / 44.1) + (kod & 1), r
        ramce.append((s48, s44, s32))  # poradie fscod: 48, 44,1, 32 kHz
assert len(ramce) == 38, len(ramce)

# --- tabuľka 7.34: 5/8 rámca ---
patosmin = []
for r in tabulka("7.34"):
    m = re.match(r"^‘([01]{6})’\s+\((\d+)\)\s+(\d+) kbps\s+(\d+)\s+(\d+)\s+(\d+)\s*$", r)
    if m:
        kod, s32, s44, s48 = int(m[1], 2), int(m[4]), int(m[5]), int(m[6])
        assert kod == len(patosmin)
        patosmin.append((s48, s44, s32))
assert len(patosmin) == 38
for kod in range(38):
    for f in range(3):
        v = ramce[kod][f]
        assert patosmin[kod][f] == (v >> 1) + (v >> 3), (kod, f, patosmin[kod][f])


def adresova(cislo, n):
    """Tabuľka „Address – hodnota“ (7.6–7.11, 7.16): adresy 0 … n − 1 v jednom alebo dvoch stĺpcoch."""
    hodnoty = {}
    for r in tabulka(cislo):
        c = cisla(r)
        if len(c) in (2, 4):
            for a, v in zip(c[0::2], c[1::2]):
                hodnoty[a] = v
    assert sorted(hodnoty) == list(range(n)), (cislo, sorted(hodnoty))
    return [hodnoty[a] for a in range(n)]


slowdec = adresova("7.6", 4)
fastdec = adresova("7.7", 4)
slowgain = adresova("7.8", 4)
dbpbtab = adresova("7.9", 4)
floortab = adresova("7.10", 8)
fastgain = adresova("7.11", 8)
baptab = adresova("7.16", 64)
# §7.2.1: celočíselná aritmetika v doplnkovom kóde; 0xf800 je 16-bitové −2048 (jediná hodnota s najvyšším bitom)
assert floortab[7] == 0xF800 and all(v < 0x8000 for v in floortab[:7])
floortab[7] -= 0x10000
assert all(baptab[i] <= baptab[i + 1] for i in range(63)) and baptab[0] == 0 and baptab[63] == 15

# --- tabuľka 7.12: pásma ---
pasma = {}
for r in tabulka("7.12"):
    c = cisla(r)
    if len(c) == 6:
        for b, zac, sz in (c[0:3], c[3:6]):
            pasma[b] = (zac, sz)
assert sorted(pasma) == list(range(50))
bndtab = [pasma[b][0] for b in range(50)]
bndsz = [pasma[b][1] for b in range(50)]
for b in range(49):
    assert bndtab[b] + bndsz[b] == bndtab[b + 1], b
assert bndtab[49] + bndsz[49] == 253


def mriezka(cislo, n):
    """Tabuľka „A=…, B=0…9“ (7.13, 7.14, 7.33): hodnota indexu 10·A + B."""
    hodnoty = []
    for r in tabulka(cislo):
        m = re.match(r"^\s*A=(\d+)\s+(.*)$", r)
        if m:
            assert int(m[1]) * 10 == len(hodnoty), (cislo, r)
            hodnoty += m[2].split()
    assert len(hodnoty) == n, (cislo, len(hodnoty))
    return hodnoty


masktab = [int(v) for v in mriezka("7.13", 256)]
latab = [int(v, 16) for v in mriezka("7.14", 256)]
for b in range(50):
    for k in range(bndtab[b], bndtab[b] + bndsz[b]):
        assert masktab[k] == b, (k, b)
assert masktab[253:] == [0, 0, 0]
assert latab[0] == 0x40 and all(latab[i] >= latab[i + 1] for i in range(255))

# --- tabuľka 7.15: prah počutia, poradie fscod 48, 44,1, 32 kHz ---
hth = {}
for r in tabulka("7.15"):
    c = cisla(r)
    if len(c) == 8:
        for skup in (c[0:4], c[4:8]):
            hth[skup[0]] = skup[1:]
assert sorted(hth) == list(range(50))
hth = [[hth[b][f] for b in range(50)] for f in range(3)]

# --- tabuľky 7.17, 7.18: úrovne a bity mantís ---
urovne, bity = {}, {}
for r in tabulka("7.18"):
    m = re.match(r"^\s*(\d+)\s+([\d,]+)\s+(none|symmetric|asymmetric)\s+([\d.]+)(?:\s+\((\d+)/(\d+)\))?\s*$", r)
    if m:
        bap = int(m[1])
        urovne[bap] = int(m[2].replace(",", ""))
        bity[bap] = (int(m[5]), int(m[6])) if m[5] else (int(float(m[4])), 1)
assert sorted(urovne) == list(range(16))
for bap in range(6, 16):
    assert urovne[bap] == 2 ** bity[bap][0], bap  # asymetrické: doplnkový kód s qntztab bitmi
skupiny = {bap: bity[bap] for bap in range(1, 6)}  # (bitov skupiny, mantís v skupine)
assert skupiny == {1: (5, 3), 2: (7, 3), 3: (3, 1), 4: (7, 2), 5: (4, 1)}, skupiny

# --- tabuľky 7.19–7.23: symetrické kvantizéry ---
kvant = {}
for cislo, bap in (("7.19", 1), ("7.20", 2), ("7.21", 3), ("7.22", 4), ("7.23", 5)):
    hodnoty = []
    for r in tabulka(cislo):
        m = re.match(r"^\s*(\d+)\s+([–-]?)(\d+)\.?(?:/(\d+))?\s*$", r)
        if m:
            assert int(m[1]) == len(hodnoty)
            v = Fraction(int(m[3]), int(m[4]) if m[4] else 1)
            hodnoty.append(-v if m[2] else v)
    n = urovne[bap]
    assert hodnoty == [Fraction(2 * k - (n - 1), n) for k in range(n)], (cislo, hodnoty)
    kvant[bap] = hodnoty

# --- tabuľka 7.24: podpásma väzby ---
riadky_24 = [cisla(r)[:3] for r in tabulka("7.24") if re.match(r"^\d+\s+\d+\s+\d+\s", r)]
assert riadky_24 == [[sb, 37 + 12 * sb, 48 + 12 * sb] for sb in range(18)], riadky_24

# --- tabuľky 7.25–7.28: pásma rematrixu ---
for cislo, pocet, koniec in (("7.25", 4, "252"), ("7.26", 4, "A"), ("7.27", 3, "A"), ("7.28", 2, None)):
    hranice = []
    for r in tabulka(cislo):
        m = re.match(r"^(\d)\s+(\d+)\s+(\d+|A)\s", r)
        if m:
            hranice.append((int(m[2]), m[3]))
    ocak = [(13, "24"), (25, "36"), (37, "60"), (61, "252")][:pocet]
    if koniec == "A":
        ocak[-1] = (ocak[-1][0], "A")
    assert hranice == ocak, (cislo, hranice)
    if koniec == "A":
        assert any("A = 36 + cplbegf * 12" in r for r in tabulka(cislo)), cislo

# --- tabuľka 7.33: okno (5 desatinných miest) a KBD ---
okno_text = [float(v) for v in mriezka("7.33", 256)]


def bessel_i0(x):
    s, clen, k = 1.0, 1.0, 1
    while clen > 1e-20 * s:
        clen *= (x / (2 * k)) ** 2
        s += clen
        k += 1
    return s


def kbd(alfa, n=512):
    pol = n // 2
    kaiser = [bessel_i0(math.pi * alfa * math.sqrt(1 - (2 * j / pol - 1) ** 2)) for j in range(pol + 1)]
    celkom = sum(kaiser)
    vysl, sucet = [], 0.0
    for j in range(pol):
        sucet += kaiser[j]
        vysl.append(math.sqrt(sucet / celkom))
    return vysl


def sedi(okno):
    return all(abs(round(w, 5) - t) < 1e-9 for w, t in zip(okno, okno_text))


okno = kbd(5)
assert sedi(okno), "KBD α=5 nesedí s tabuľkou 7.33"
assert not sedi(kbd(4.9)) and not sedi(kbd(5.1)) and not sedi(kbd(4))
odchylka = max(abs(w - t) for w, t in zip(okno, okno_text))


def riadok(hodnoty, sirka=16, fmt=str):
    v = [fmt(x) for x in hodnoty]
    return "\n".join("    " + ", ".join(v[i:i + sirka]) + "," for i in range(0, len(v), sirka))


def f32(x):
    return repr(float(x)) if float(x) != int(x) else f"{int(x)}.0"


print(f"""//! Tabuľky AC-3 z textu **ATSC A/52:2018** – vygenerované skriptom `media/nastroje/ac3_tabulky.py` (neupravovať
//! ručne). Čísla tabuliek sú čísla normy; kontroly zhody medzi tabuľkami a vzorcami textu robí generátor.

/// Tabuľka 5.18 – veľkosť synchronizačného rámca v 16-bitových slovách, `[frmsizecod][fscod]` (48, 44,1, 32 kHz).
pub(super) const VELKOST_RAMCA: [[u16; 3]; 38] = [
{riadok([f"[{a}, {b}, {c}]" for a, b, c in ramce], 6)}
];

/// Tabuľka 7.6 – slowdec[].
pub(super) const SLOWDEC: [i32; 4] = [{", ".join(map(hex, slowdec))}];
/// Tabuľka 7.7 – fastdec[].
pub(super) const FASTDEC: [i32; 4] = [{", ".join(map(hex, fastdec))}];
/// Tabuľka 7.8 – slowgain[].
pub(super) const SLOWGAIN: [i32; 4] = [{", ".join(map(hex, slowgain))}];
/// Tabuľka 7.9 – dbpbtab[].
pub(super) const DBPBTAB: [i32; 4] = [{", ".join(map(hex, dbpbtab))}];
/// Tabuľka 7.10 – floortab[]; posledná hodnota 0xf800 je 16-bitové −2048 (doplnkový kód, §7.2.1).
pub(super) const FLOORTAB: [i32; 8] = [{", ".join(map(str, floortab))}];
/// Tabuľka 7.11 – fastgain[].
pub(super) const FASTGAIN: [i32; 8] = [{", ".join(map(hex, fastgain))}];

/// Tabuľka 7.12 – prvý koeficient pásma (bndtab[]).
pub(super) const BNDTAB: [u8; 50] = [
{riadok(bndtab, 25)}
];
/// Tabuľka 7.12 – šírka pásma (bndsz[]).
pub(super) const BNDSZ: [u8; 50] = [
{riadok(bndsz, 25)}
];
/// Tabuľka 7.13 – pásmo koeficientu (masktab[]).
pub(super) const MASKTAB: [u8; 256] = [
{riadok(masktab, 32)}
];
/// Tabuľka 7.14 – logaritmické sčítanie (latab[]).
pub(super) const LATAB: [i32; 256] = [
{riadok(latab, 32)}
];
/// Tabuľka 7.15 – prah počutia hth[fscod][pásmo] (48, 44,1, 32 kHz).
pub(super) const HTH: [[i32; 50]; 3] = [
{chr(10).join("    [" + chr(10) + riadok(h, 10, hex) + chr(10) + "    ]," for h in hth)}
];
/// Tabuľka 7.16 – baptab[].
pub(super) const BAPTAB: [u8; 64] = [
{riadok(baptab, 32)}
];

/// Tabuľka 7.18 – bity mantisy pri asymetrickej kvantizácii (qntztab[bap], bap 6–15); pri bap 1–5 skupiny.
pub(super) const BITY_MANTISY: [u8; 16] = [{", ".join(str(bity[b][0] if b >= 6 else 0) for b in range(16))}];

/// Tabuľka 7.19 – bap = 1 (3 úrovne).
pub(super) const KVANT_3: [f32; 3] = [{", ".join(f32(v) for v in kvant[1])}];
/// Tabuľka 7.20 – bap = 2 (5 úrovní).
pub(super) const KVANT_5: [f32; 5] = [{", ".join(f32(v) for v in kvant[2])}];
/// Tabuľka 7.21 – bap = 3 (7 úrovní).
pub(super) const KVANT_7: [f32; 7] = [{", ".join(f32(v) for v in kvant[3])}];
/// Tabuľka 7.22 – bap = 4 (11 úrovní).
pub(super) const KVANT_11: [f32; 11] = [{", ".join(f32(v) for v in kvant[4])}];
/// Tabuľka 7.23 – bap = 5 (15 úrovní).
pub(super) const KVANT_15: [f32; 15] = [{", ".join(f32(v) for v in kvant[5])}];

/// Tabuľka 7.33 – okno w[0 … 255] (prvá polovica okna 512). Text ho uvádza na 5 desatinných miest; tu je presné
/// odvodené Kaiser–Besselovo okno α = 5, ktoré sa s tabuľkou zhoduje na všetkých 256 miestach (najväčšia
/// odchýlka od textu {odchylka:.2e}).
pub(super) const OKNO: [f32; 256] = [
{riadok(okno, 8, repr)}
];""")
