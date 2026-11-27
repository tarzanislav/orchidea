#!/usr/bin/env python3
"""Generátor tabuliek MPEG audio vrstiev I a II (`media/src/mpa/tabulky_l2.rs`) – etapa L8.

Zdroj (relácia A 27. 11. 2026, #20; voľne dostupná oficiálna norma – vlastník normu ISO nekupuje):
**ETSI TS 103 466 V1.2.1 (2019-09)** – Digital Audio Broadcasting (DAB); DAB audio coding (MPEG Layer II),
PDF so SHA-256 b4450aef6dd4e9b5e9d8776dd602621e8fdde091caa86dc47b777146dfe932be. Text strán 17–23 a 27 sa získa
ručným postupom CI `media-zdroje.yml` (`pdftotext -layout`; repozitár je verejný – PDF ani text normy sa
neukladá, v zdrojovom kóde sú len číselné tabuľky s číslom tabuľky).

Tabuľky: 1 (mierky ScF – v norme ISO 11172-3 tabuľka 3-B.1, platí pre vrstvy I a II), 4 a 5 (rozdelenie bitov
pre 48 kHz = 3-B.2a a 3-B.2c), 6 (24 kHz – MPEG-2 LSF, 13818-3), 7 (koeficienty kvantovania A, B), 8 (triedy
kvantovania = 3-B.4 bez C a D), 15 (hranica intenzity podľa mode_extension).

Kontroly z textu normy: mierky = 2^(1 − i/3); každý riadok rozdelenia má 2^nbal − 1 krokov; súčty nbal 88
(tabuľka 4, sblimit 27), 26 (tabuľka 5, sblimit 8), 75 (tabuľka 6, sblimit 30) – uvedené v texte §5.2.6;
A = kroky/2^n a B = A − 1 (tabuľka 7); bity kódového slova (tabuľka 8) = ⌈log2(kroky³)⌉ pri zoskupení, inak
log2(kroky + 1).

Použitie: `mpa_tabulky_l2.py <strany_17-23.txt> <strany_27-28.txt> > media/src/mpa/tabulky_l2.rs`
"""

import math
import re
import sys
from fractions import Fraction

KROKY = [3, 5, 7, 9, 15, 31, 63, 127, 255, 511, 1023, 2047, 4095, 8191, 16383, 32767, 65535]


def cisla(riadok):
    """Celé čísla riadku; oddeľovač tisícov je medzera („1 023“) – spojí sa, ak vznikne platný počet krokov."""
    t = riadok.split()
    vysl = []
    for x in t:
        if (re.fullmatch(r"\d{3}", x) and vysl and isinstance(vysl[-1], int) and vysl[-1] < 100
                and vysl[-1] * 1000 + int(x) in KROKY):
            vysl[-1] = vysl[-1] * 1000 + int(x)
        elif x.isdigit():
            vysl.append(int(x))
        else:
            vysl.append(x)
    return vysl


def telo(riadky, nadpis, koniec):
    """Riadky od nadpisu tabuľky po riadok, ktorý začína `koniec`."""
    i = next(k for k, r in enumerate(riadky) if r.strip().startswith(nadpis))
    vysl = []
    for r in riadky[i + 1:]:
        if r.strip().startswith(koniec):
            break
        vysl.append(r)
    return vysl


def rozdelenie(riadky, nadpis, sblimit, sucet):
    tab = []
    for r in telo(riadky, nadpis, "ETSI"):
        c = cisla(r)
        if len(c) >= 3 and c[0] == len(tab) and isinstance(c[1], int) and c[2] == "-":
            sb, nbal, kroky = c[0], c[1], c[3:]
            assert all(k in KROKY for k in kroky), f"{nadpis}: sb {sb} – neznámy počet krokov {kroky}"
            assert len(kroky) == (2 ** nbal - 1 if nbal else 0), f"{nadpis}: sb {sb} má {len(kroky)} krokov, nbal {nbal}"
            tab.append((nbal, kroky))
    assert len(tab) == 32, f"{nadpis}: {len(tab)} riadkov"
    assert all(n == 0 for n, _ in tab[sblimit:]) and all(n > 0 for n, _ in tab[:sblimit]), f"{nadpis}: sblimit"
    assert sum(n for n, _ in tab) == sucet, f"{nadpis}: súčet nbal {sum(n for n, _ in tab)} ≠ {sucet}"
    return tab


def main():
    s = open(sys.argv[1], encoding="utf-8").read().splitlines()
    s2 = open(sys.argv[2], encoding="utf-8").read().splitlines()
    von = [
        "//! Tabuľky MPEG audio vrstiev I a II – **vygenerované** skriptom `media/nastroje/mpa_tabulky_l2.py`,",
        "//! neupravovať. Zdroj: ETSI TS 103 466 V1.2.1 (2019-09), čísla tabuliek pri každom poli.",
        "",
    ]
    # tabuľka 1 – mierky: dvojice stĺpcov (index, hodnota s desatinnou čiarkou)
    mierky = {}
    for r in telo(s, "Table 1: Scale Factors", "5.2.3"):
        for i, v in re.findall(r"(\d+)\s+(\d,\d{14})", r):
            mierky[int(i)] = v.replace(",", ".")
    assert sorted(mierky) == list(range(63)), f"tabuľka 1: {len(mierky)} mierok"
    for i in range(63):
        assert abs(float(mierky[i]) - 2 ** (1 - i / 3)) < 1e-13, f"tabuľka 1: mierka {i}"
    von.append("/// ETSI TS 103 466 tabuľka 1 (= 11172-3 tabuľka 3-B.1) – mierky ScF vrstiev I a II; skontrolované 2^(1 − i/3).")
    von.append(f"pub(crate) static MIERKY: [f64; 63] = [{', '.join(mierky[i] for i in range(63))}];")
    # tabuľky 4, 5, 6 – rozdelenie bitov
    for meno, nadpis, sblimit, sucet, popis in [
        ("ROZDELENIE_A", "Table 4: Bit allocation", 27, 88, "tabuľka 4 (48 kHz, vyššie toky = 11172-3 tabuľka 3-B.2a)"),
        ("ROZDELENIE_C", "Table 5: Bit allocation", 8, 26, "tabuľka 5 (48 kHz, nízke toky = 11172-3 tabuľka 3-B.2c)"),
        ("ROZDELENIE_LSF", "Table 6: Bit allocation", 30, 75, "tabuľka 6 (24 kHz, MPEG-2 LSF – 13818-3)"),
    ]:
        tab = rozdelenie(s, nadpis, sblimit, sucet)
        riadky = ", ".join(f"({n}, &[{', '.join(map(str, k))}])" for n, k in tab[:sblimit])
        von.append(f"/// ETSI TS 103 466 {popis}: (nbal, počty krokov podľa indexu 1..) pre sb < sblimit = {sblimit}.")
        von.append(f"pub(crate) static {meno}: [(u8, &[u16]); {sblimit}] = [{riadky}];")
    # tabuľka 7 – A, B a tabuľka 8 – triedy
    ab = {}
    for r in telo(s, "Table 7: Quantization coefficients", "The quantization"):
        c = re.fullmatch(r"\s*(\d[\d ]*?)\s+(\d,\d{9})\s+(-\d,\d{9})\s*", r)
        if c:
            ab[int(c.group(1).replace(" ", ""))] = (float(c.group(2).replace(",", ".")), float(c.group(3).replace(",", ".")))
    assert sorted(ab) == KROKY, f"tabuľka 7: {sorted(ab)}"
    triedy = []
    for r in telo(s, "Table 8: Classes of quantization", "ETSI"):
        c = re.fullmatch(r"\s*(\d[\d ]*?)\s+(yes|no)\s+(\d)\s+(\d+)\s*", r)
        if c:
            triedy.append((int(c.group(1).replace(" ", "")), c.group(2) == "yes", int(c.group(3)), int(c.group(4))))
    assert [t[0] for t in triedy] == KROKY, "tabuľka 8: poradie krokov"
    for kroky, zoskupenie, vzoriek, bitov in triedy:
        n = math.ceil(math.log2(kroky)) if zoskupenie else bitov
        a, b = ab[kroky]
        # A = kroky / 2^n, B = A − 1 presne (tabuľka 7 ich uvádza zaokrúhlené na 9 desatinných miest)
        assert abs(a - kroky / 2 ** n) <= 5.1e-10 and abs(b - (kroky / 2 ** n - 1)) <= 5.1e-10, f"tabuľka 7: {kroky}"
        if zoskupenie:
            assert vzoriek == 3 and bitov == math.ceil(math.log2(kroky ** 3)), f"tabuľka 8: {kroky}"
        else:
            assert vzoriek == 1 and 2 ** bitov == kroky + 1, f"tabuľka 8: {kroky}"
    von.append("/// ETSI TS 103 466 tabuľky 7 a 8 (= 11172-3 tabuľka 3-B.4 bez C, D): (počet krokov, zoskupenie,")
    von.append("/// bitov kódového slova, bitov vzorky n). Skontrolované: A = kroky/2^n, B = A − 1, bity podľa zoskupenia.")
    prvky = ", ".join(f"({k}, {str(z).lower()}, {b}, {math.ceil(math.log2(k)) if z else b})" for k, z, _, b in triedy)
    von.append(f"pub(crate) static TRIEDY: [(u16, bool, u8, u8); {len(triedy)}] = [{prvky}];")
    # tabuľka 15 – hranica intenzity
    hranice = [int(m.group(2)) for r in telo(s2, "Table 15:", "ETSI")
               if (m := re.search(r'"(\d\d)"\s+\d+ to 31\s+bound == (\d+)', r))]
    assert hranice == [4, 8, 12, 16], f"tabuľka 15: {hranice}"
    von.append("/// ETSI TS 103 466 tabuľka 15 (= 11172-3 2.4.2.3 mode_extension, aj text CD) – hranica intenzity.")
    von.append(f"pub(crate) static HRANICA_INTENZITY: [u8; 4] = [{', '.join(map(str, hranice))}];")
    print("\n".join(von))


if __name__ == "__main__":
    main()
