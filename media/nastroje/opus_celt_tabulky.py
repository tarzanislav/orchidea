#!/usr/bin/env python3
"""Generátor číselných tabuliek CELT (`media/src/opus/celt_tabulky.rs`) – etapa L7, rozhodnutie vlastníka A (#15).

Z **prílohy A RFC 6716** (zdrojový kód zakódovaný v texte RFC) sa berú **iba číselné inicializátory polí** –
skript nečíta nič iné ako zoznam čísel medzi `{` a `}` vybraných polí. Tabuľky, ktoré má aj text RFC (tabuľka 55
– šírky pásiem, tabuľka 57 – statické rozdelenie bitov), sa skontrolujú proti textu.

Príprava prílohy A (podľa RFC 6716, príloha A.1):
    grep '^\\ \\ \\ ###' rfc6716.txt | sed -e 's/...###//' | base64 --decode > opus-rfc6716.tar.gz
    sha1sum opus-rfc6716.tar.gz   # 86a927223e73d2476646a1b933fcd3fffb6ecc8c (hodnota z RFC)
    tar xzf opus-rfc6716.tar.gz

Použitie: `opus_celt_tabulky.py <rfc6716.txt> <opus-rfc6716/celt> > media/src/opus/celt_tabulky.rs`
"""

import os
import re
import sys

VZOR = re.compile(
    r"(?:static\s+)?const\s+([A-Za-z_0-9 ]+?)\s+([A-Za-z_][A-Za-z_0-9]*)\s*((?:\[[^\]]*\])+)\s*=\s*\{", re.S)


def bez_komentarov(t):
    return re.sub(r"//[^\n]*", " ", re.sub(r"/\*.*?\*/", " ", t, flags=re.S))


def telo_pola(cesta, meno, poradie=0):
    """Text inicializátora poľa `meno` (pri viacerých definíciách – pevná a plávajúca čiarka – `poradie`)."""
    t = bez_komentarov(open(cesta, encoding="latin-1").read())
    najdene = []
    for m in VZOR.finditer(t):
        if m.group(2) != meno:
            continue
        hlbka, i = 1, m.end()
        while hlbka:
            hlbka += {"{": 1, "}": -1}.get(t[i], 0)
            i += 1
        najdene.append(t[m.end(): i - 1])
    return najdene[poradie]


def cisla(cesta, meno, poradie=0):
    telo = telo_pola(cesta, meno, poradie)
    telo = re.sub(r"QCONST16\(\s*([-0-9.]+)f?\s*,\s*15\)", r"\1", telo)
    vysl = []
    for x in re.split(r"[,{}\s]+", telo):
        if not x:
            continue
        if "/" in x:  # zlomok typu 29440/32768.
            c, m = x.split("/")
            vysl.append(int(c))
            assert float(m) == 32768.0
        elif x.startswith("0x"):
            vysl.append(int(x, 16))
        else:
            vysl.append(float(x.rstrip("f")) if "." in x else int(x))
    return vysl


def tabulka_57(rfc):
    """Tabuľka 57 z textu RFC 6716: riadky = pásma (bez čísla pásma), stĺpce = kvalita q (1/32 bitu na koeficient)."""
    riadky = open(rfc, encoding="latin-1").read().split("\n")
    koniec = next(i for i, r in enumerate(riadky) if "Table 57: CELT Static Allocation Table" in r)
    vysl = []
    for r in riadky[koniec - 120: koniec]:
        if re.match(r"^\s*\|(?:\s*\d+\s*\|){11}\s*$", r):  # 11 stĺpcov q = 0..10, riadky v poradí pásiem
            vysl.append([int(x) for x in r.split("|") if x.strip()])
    vysl = vysl[-21:]  # tabuľka prechádza cez zlom strany
    assert len(vysl) == 21 and all(r[0] == 0 and r[10] >= 100 for r in vysl), vysl
    return vysl


def tabulka_55(rfc):
    """Tabuľka 55: počet koeficientov MDCT pásma pri 2,5 ms (prvý stĺpec)."""
    riadky = open(rfc, encoding="latin-1").read().split("\n")
    koniec = next(i for i, r in enumerate(riadky) if "Table 55: MDCT Bins per Channel" in r)
    vysl = []
    for r in riadky[koniec - 120: koniec]:
        m = re.match(r"^\s*\|\s*(\d+)\s*\|\s*(\d+)\s*\|", r)
        if m:
            vysl.append((int(m.group(1)), int(m.group(2))))
    vysl = vysl[-21:]  # tabuľka môže prechádzať cez zlom strany
    assert [b for b, _ in vysl] == list(range(21)), vysl
    vysl = [n for _, n in vysl]
    return vysl


def rust_pole(meno, typ, hodnoty, prameň, na_riadok=16):
    out = [f"/// {prameň}", f"pub(crate) const {meno}: [{typ}; {len(hodnoty)}] = ["]
    for i in range(0, len(hodnoty), na_riadok):
        out.append("    " + " ".join(f"{v}," for v in hodnoty[i: i + na_riadok]))
    out.append("];")
    return "\n".join(out)


def main():
    rfc, celt = sys.argv[1], sys.argv[2]
    p = lambda s: os.path.join(celt, s)  # noqa: E731
    vystup = [
        "//! Číselné tabuľky CELT (RFC 6716) – **vygenerované** skriptom `media/nastroje/opus_celt_tabulky.py`, neupravovať.",
        "//!",
        "//! Rozhodnutie vlastníka A (#15, 26. 11. 2026): z prílohy A RFC 6716 sa berú iba číselné tabuľky ako údaje;",
        "//! pri každej je prameň (súbor prílohy A a meno poľa, prípadne číslo tabuľky v texte RFC 6716).",
        "",
    ]
    eband = cisla(p("modes.c"), "eband5ms")
    sirky = [b - a for a, b in zip(eband, eband[1:])]
    assert sirky == tabulka_55(rfc), "eband5ms nesedí s tabuľkou 55"
    vystup.append(rust_pole("HRANICE_PASIEM", "u8", eband,
                            "Hranice pásiem v koeficientoch MDCT pri 2,5 ms – príloha A `celt/modes.c: eband5ms` "
                            "(= RFC 6716, tabuľka 55)."))
    alok = cisla(p("modes.c"), "band_allocation")
    t57 = tabulka_57(rfc)
    assert all(alok[q * 21 + b] == t57[b][q] for q in range(11) for b in range(21)), "band_allocation ≠ tabuľka 57"
    vystup.append(rust_pole("ROZDELENIE", "u8", alok,
                            "Statické rozdelenie bitov [q·21 + pásmo] v 1/32 bitu na koeficient – príloha A "
                            "`celt/modes.c: band_allocation` (= RFC 6716, tabuľka 57, transponovaná).", 21))
    for meno, subor, pole, typ in [
        ("LOG_N", "static_modes_float.h", "logN400", "i16"),
        ("CACHE_INDEX", "static_modes_float.h", "cache_index50", "i16"),
        ("CACHE_BITY", "static_modes_float.h", "cache_bits50", "u8"),
        ("CACHE_MAXIMA", "static_modes_float.h", "cache_caps50", "u8"),
        ("E_PROB_MODEL", "quant_bands.c", "e_prob_model", "u8"),
        ("MALA_ENERGIA_ICDF", "quant_bands.c", "small_energy_icdf", "u8"),
        ("TF_VYBER", "celt.c", "tf_select_table", "i8"),
        ("TRIM_ICDF", "celt.c", "trim_icdf", "u8"),
        ("SPREAD_ICDF", "celt.c", "spread_icdf", "u8"),
        ("TAPSET_ICDF", "celt.c", "tapset_icdf", "u8"),
        ("LOG2_ZLOMOK", "rate.c", "LOG2_FRAC_TABLE", "u8"),
        ("PORADIE_Y", "bands.c", "ordery_table", "u8"),
        ("PREKLADANIE_BITOV", "bands.c", "bit_interleave_table", "u8"),
        ("ROZKLADANIE_BITOV", "bands.c", "bit_deinterleave_table", "u8"),
        ("EXP2_8", "bands.c", "exp2_table8", "u16"),
        ("FAKTOR_ROZPRESTRETIA", "vq.c", "SPREAD_FACTOR", "u8"),
    ]:
        vystup.append(rust_pole(meno, typ, cisla(p(subor), pole), f"Príloha A `celt/{subor}: {pole}`."))
    # plávajúca čiarka: druhá definícia v quant_bands.c (prvá je pre pevnú čiarku)
    stredy = cisla(p("quant_bands.c"), "eMeans", 1)
    vystup.append(rust_pole("STREDNA_ENERGIA", "f32", [f"{v:.6}" for v in stredy],
                            "Stredná energia pásma (log2) – príloha A `celt/quant_bands.c: eMeans` (plávajúca "
                            "čiarka).", 8))
    for meno, pole in [("PREDIKCIA_CAS", "pred_coef"), ("PREDIKCIA_PASMO", "beta_coef")]:
        vystup.append(rust_pole(meno, "u16", cisla(p("quant_bands.c"), pole, 1),
                                f"Koeficient predikcie hrubej energie ×32768 podľa LM – príloha A "
                                f"`celt/quant_bands.c: {pole}`."))
    zisky = cisla(p("celt.c"), "gains")
    vystup.append(rust_pole("ZISKY_POSTFILTRA", "f32", [f"{v}" for v in zisky],
                            "Odbočky postfiltra [tapset·3 + k] – príloha A `celt/celt.c: gains` (= text RFC 6716 "
                            "§4.3.7.1).", 3))
    print("\n\n".join(vystup))


if __name__ == "__main__":
    main()
