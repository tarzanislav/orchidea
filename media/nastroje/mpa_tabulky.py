#!/usr/bin/env python3
"""Generátor tabuliek MPEG audio vrstvy III (`media/src/mpa/tabulky.rs`) – etapa L8.

Zdroje (rozhodnutie vlastníka 27. 11. 2026, #15): text normy, nie kód iných dekodérov.
- **[S]** M. Sieler, R. Sperschneider: „MPEG-Layer3 – Bitstream Syntax and Decoding“, vydanie 1.2 (1997) – tabuľky
  16, 21–60 (Huffmanove kódy, pásma škálovacích faktorov, pretab, koeficienty aliasu).
- **[CD]** návrh normy CD 11172-3 (ISO/IEC JTC1/SC29/WG11, 22. 11. 1991) – hlavný text (príloha B v ňom nie je);
  z neho sa porovnávajú tabuľky, ktoré sú v texte aj tam: veľkosti slen1/slen2 a skupiny scfsi_band.

Text sa číta nástrojom `pdftotext -raw`. PDF niekde zlieva susedné čísla („1510“ = y 15, hlen 10), preto sa riadok
číta bez medzier a hľadá sa jediný rozklad, ktorý súhlasí s očakávaným poradím indexov a dĺžkou kódu. Kontroly:
počet riadkov, dĺžka kódu = hlen, kód bez predpony, Kraftov súčet = 1, súvislé pásma.

Použitie: `mpa_tabulky.py <sieler.pdf> <cd11172-3.pdf> > media/src/mpa/tabulky.rs`
"""

import re
import subprocess
import sys
from fractions import Fraction


def text(pdf):
    return subprocess.run(["pdftotext", "-raw", pdf, "-"], check=True, capture_output=True, text=True).stdout


def telo_tabulky(riadky, cislo):
    """Riadky tabuľky `cislo` vrátane pokračovaní (po nadpis ďalšej tabuľky)."""
    vysl, dnu = [], False
    for r in riadky:
        m = re.fullmatch(r"(Continuation of )?Table (\d+)", r.strip())
        if m:
            dnu = int(m.group(2)) == cislo
            continue
        if dnu:
            vysl.append(r.strip())
    assert vysl, f"tabuľka {cislo} nenájdená"
    return vysl


def huffman(riadky, cislo, indexov, rozmer):
    """Huffmanova tabuľka: zoznam (hodnoty, kód) v poradí; `indexov` 2 (x, y) alebo 4 (v, w, x, y)."""
    ocakavane = []
    if indexov == 2:
        ocakavane = [(x, y) for x in range(rozmer) for y in range(rozmer)]
    else:
        ocakavane = [((i >> 3) & 1, (i >> 2) & 1, (i >> 1) & 1, i & 1) for i in range(16)]
    vysl = []
    for r in telo_tabulky(riadky, cislo):
        s = r.replace(" ", "")
        if not s or not s.isdigit() or len(vysl) == len(ocakavane):
            continue
        hodnoty = ocakavane[len(vysl)]
        predpona = "".join(str(v) for v in hodnoty)
        if not s.startswith(predpona):
            raise ValueError(f"tabuľka {cislo}: riadok {r!r} nesedí s {hodnoty}")
        zvysok = s[len(predpona):]
        kandidati = [(int(zvysok[:k]), zvysok[k:]) for k in (1, 2) if len(zvysok) > k
                     and int(zvysok[:k]) == len(zvysok) - k and set(zvysok[k:]) <= {"0", "1"}]
        if len(kandidati) != 1:
            raise ValueError(f"tabuľka {cislo}: nejednoznačný riadok {r!r}: {kandidati}")
        vysl.append((hodnoty, kandidati[0][1]))
    assert len(vysl) == len(ocakavane), f"tabuľka {cislo}: {len(vysl)} z {len(ocakavane)} riadkov"
    kody = [k for _, k in vysl]
    assert len(set(kody)) == len(kody), f"tabuľka {cislo}: opakovaný kód"
    for a in kody:
        for b in kody:
            assert a == b or not b.startswith(a), f"tabuľka {cislo}: {a} je predpona {b}"
    assert sum(Fraction(1, 2 ** len(k)) for k in kody) == 1, f"tabuľka {cislo}: Kraftov súčet ≠ 1"
    return vysl


def pasma(riadky, cislo, ciar):
    """Pásma škálovacích faktorov: začiatky pásiem + koniec (počet čiar)."""
    zac = []
    for r in telo_tabulky(riadky, cislo):
        m = re.fullmatch(r"(\d+) (\d+) (\d+) (\d+)", r)
        if m:
            i, w, a, b = map(int, m.groups())
            assert i == len(zac) and b - a + 1 == w and (not zac or a == zac[-1] + prev), f"tabuľka {cislo}: {r}"
            zac.append(a)
            prev = w
            koniec = b
    # posledné pásmo (21 pri dlhých, 12 pri krátkych) tabuľka neuvádza: od konca predošlého po koniec granuly
    zac += [koniec + 1, ciar]
    return zac


def main():
    s = text(sys.argv[1]).splitlines()
    cd = text(sys.argv[2])
    von = [
        "//! Tabuľky MPEG audio vrstvy III – **vygenerované** skriptom `media/nastroje/mpa_tabulky.py`, neupravovať.",
        "//!",
        "//! Zdroj [S]: Sieler, Sperschneider – „MPEG-Layer3 Bitstream Syntax and Decoding“ (1997), čísla tabuliek",
        "//! pri každom poli; slen a scfsi_band porovnané s textom návrhu normy CD 11172-3 (1991).",
        "",
    ]
    # Huffmanove tabuľky (tabuľky 24–41): (číslo v [S], tabuľky bitového toku, rozmer, linbits)
    tab = {26: [0], 27: [1], 28: [2], 29: [3], 30: [5], 31: [6], 32: [7], 33: [8], 34: [9], 35: [10], 36: [11],
           37: [12], 38: [13], 39: [15], 40: list(range(16, 24)), 41: list(range(24, 32))}
    rozmer = {26: 1, 27: 2, 28: 3, 29: 3, 30: 4, 31: 4, 32: 6, 33: 6, 34: 6, 35: 8, 36: 8, 37: 8, 38: 16, 39: 16,
              40: 16, 41: 16}
    linbits = {}
    for c in (40, 41):
        nadpis = next(r for i, r in enumerate(s) if s[i - 1].strip() == f"Table {c}")
        for t, lb in re.findall(r"(\d+) \(linbits=(\d+)\)", nadpis):
            linbits[int(t)] = int(lb)
    popis = ["(0, 0, 0)"] * 32  # (index kódov, rozmer, linbits) pre tabuľky 0–31; 4 a 14 sa nepoužívajú
    kody_vsetky = []
    for c, tabulky in tab.items():
        if c == 26:  # tabuľka 0: všetky hodnoty nulové, bez kódu
            continue
        h = huffman(s, c, 2, rozmer[c])
        start = len(kody_vsetky)
        kody_vsetky.append((c, h))
        for t in tabulky:
            popis[t] = f"({start + 1}, {rozmer[c]}, {linbits.get(t, 0)})"
    for c in (24, 25):
        kody_vsetky.append((c, huffman(s, c, 4, 2)))
    for i, (c, h) in enumerate(kody_vsetky):
        meno = {24: "A", 25: "B"}.get(c)
        nazov = f"HUFF_{meno}" if meno else f"HUFF_{i + 1}"
        prvky = ", ".join(f"({int(k, 2)}, {len(k)}, {int(''.join(map(str, v)), 2) if meno else v[0] * 16 + v[1]})"
                          for v, k in h)
        von.append(f"/// [S] tabuľka {c}: (kód, dĺžka, hodnota" + (" vwxy" if meno else " x·16 + y") + ").")
        von.append(f"pub(crate) static {nazov}: [(u32, u8, u8); {len(h)}] = [{prvky}];")
    zoznam = ", ".join(f"&HUFF_{i + 1}" for i in range(len(kody_vsetky) - 2))
    von.append("/// Kódy veľkých hodnôt podľa indexu `HUFF_TABULKY` (0 = žiadne – tabuľka 0, 4, 14).")
    von.append(f"pub(crate) static KODY: [&[(u32, u8, u8)]; {len(kody_vsetky) - 1}] = [&[], {zoznam}];")
    von.append("/// (index v `KODY`, rozmer, linbits) pre tabuľky table_select 0–31 – [S] tabuľky 26–41.")
    von.append(f"pub(crate) static HUFF_TABULKY: [(u8, u8, u8); 32] = [{', '.join(popis)}];")
    # pásma škálovacích faktorov: poradie frekvencií 44,1 / 48 / 32 / 22,05 / 24 / 16 / 11,025 / 12 / 8 kHz
    poradie = [(56, 57), (58, 59), (54, 55), (50, 51), (52, 53), (48, 49), (44, 45), (46, 47), (42, 43)]
    dlhe = [pasma(s, a, 576) for a, _ in poradie]
    kratke = [pasma(s, b, 192) for _, b in poradie]
    assert all(len(p) == 23 for p in dlhe) and all(len(p) == 14 for p in kratke)
    von.append("/// Začiatky pásiem škálovacích faktorov, dlhé bloky (+ 576) – [S] tabuľky 56, 58, 54, 50, 52, 48, 44, 46, 42")
    von.append("/// (frekvencie 44,1; 48; 32; 22,05; 24; 16; 11,025; 12; 8 kHz).")
    von.append("pub(crate) static PASMA_DLHE: [[u16; 23]; 9] = [" + ", ".join(f"[{', '.join(map(str, p))}]" for p in dlhe) + "];")
    von.append("/// Začiatky pásiem škálovacích faktorov, krátke bloky (+ 192) – [S] tabuľky 57, 59, 55, 51, 53, 49, 45, 47, 43.")
    von.append("pub(crate) static PASMA_KRATKE: [[u8; 14]; 9] = [" + ", ".join(f"[{', '.join(map(str, p))}]" for p in kratke) + "];")
    # pretab (tabuľka 23): sfb 0–9 sú v PDF zlepené so svojou hodnotou („00“)
    pretab = []
    for r in telo_tabulky(s, 23):
        t = r.replace(" ", "")
        if t.isdigit():
            sfb = str(len(pretab))
            assert t.startswith(sfb), r
            pretab.append(int(t[len(sfb):]))
    assert len(pretab) == 21
    von.append(f"/// [S] tabuľka 23 – pretab[sfb].\npub(crate) static PRETAB: [u8; 22] = [{', '.join(map(str, pretab + [0]))}];")
    # koeficienty aliasu (tabuľka 60)
    c = [float(m.group(1)) for r in telo_tabulky(s, 60) if (m := re.fullmatch(r"\d (-?[0-9.]+)", r))]
    assert len(c) == 8
    von.append(f"/// [S] tabuľka 60 – koeficienty c[i] redukcie aliasu.\npub(crate) static ALIAS_C: [f64; 8] = [{', '.join(map(str, c))}];")
    # okno syntézy D[i] – [S] príloha „I. Tables“, prvý zoznam bez nadpisu (text [S] §4.3.12.6 naň odkazuje ako na
    # „Table 22“, tá má však iný obsah); v norme 11172-3 je to tabuľka 3-B.3 „Coefficients Di of the synthesis window“
    surovy = "\n".join(s)
    okno = {}
    for m in re.finditer(r"D\[\s*(\d+)\]\s*=\s*(-?)\s*(\d+),(\d+)", surovy):
        i, hodnota = int(m.group(1)), f"{m.group(2)}{m.group(3)}.{m.group(4)}"
        assert okno.get(i, hodnota) == hodnota, f"D[{i}] dvakrát s inou hodnotou"
        okno[i] = hodnota
    assert sorted(okno) == list(range(512)), f"okno D: {len(okno)} z 512 koeficientov"
    d = [float(okno[i]) for i in range(512)]
    # kontroly: každá hodnota je násobok 2^-16 zaokrúhlený na 9 desatinných miest; D[512-i] = -D[i] okrem i = 64k,
    # kde D[512-i] = D[i]; vrchol v strede (D[256])
    assert all(abs(round(v * 65536) / 65536 - v) <= 5.1e-10 for v in d), "okno D: hodnota nie je násobok 2^-16"
    for i in range(1, 512):
        assert d[512 - i] == (d[i] if i % 64 == 0 else -d[i]), f"okno D: súmernosť pri {i}"
    assert max(range(512), key=lambda i: d[i]) == 256
    von.append("/// Okno syntézy D[i] – [S] príloha I (zoznam bez nadpisu; text ho volá „Table 22“) = 11172-3 tabuľka 3-B.3.")
    von.append("/// Hodnoty presne z textu; kontroly v generátore: násobky 2^-16, súmernosť D[512 − i] = ∓D[i], vrchol D[256].")
    von.append(f"pub(crate) static OKNO_D: [f64; 512] = [{', '.join(okno[i] for i in range(512))}];")
    # slen1, slen2 (tabuľka 16) – porovnanie s CD 11172-3 (2.4.3.4, scalefac_compress)
    slen = [tuple(map(int, m.groups()[1:])) for r in telo_tabulky(s, 16)
            if (m := re.fullmatch(r"(\d+) (\d) (\d)", r))][:16]
    assert len(slen) == 16
    cd_slen = re.search(r"scalefac_compress slen1 slen2\n(.*?)\n(?=[A-Za-z])", cd, re.S)
    assert cd_slen, "CD 11172-3: tabuľka slen nenájdená"
    cisla = [int(x) for x in re.findall(r"\d+", cd_slen.group(1))]
    cd_tab = [(cisla[i + 1], cisla[i + 2]) for i in range(0, 48, 3)]
    assert cd_tab == slen, f"slen: [S] {slen} ≠ [CD] {cd_tab}"
    von.append("/// [S] tabuľka 16 = CD 11172-3 2.4.3.4 (scalefac_compress → slen1, slen2) – zhodné.")
    von.append(f"pub(crate) static SLEN: [(u8, u8); 16] = [{', '.join(f'({a}, {b})' for a, b in slen)}];")
    # skupiny scfsi_band (tabuľka 15) – porovnanie s CD 11172-3 2.4.3.4
    skup = [list(map(int, re.findall(r"\d+", m.group(2)))) for r in telo_tabulky(s, 15)
            if (m := re.fullmatch(r"(\d) ((?:\d+, )+\d+)", r))]
    zac_s = [g[0] for g in skup] + [skup[-1][-1] + 1]
    cd_skup = re.search(r"scfsi_band scalefactor bands \(see 3-Annex B, Table 3-B.8\)\n0 0,1,2,3,4,5,\n1 6,7,8,9,10,\n"
                        r"2 11 \.\.\. 15\n3 16 \.\.\. 20", cd)
    assert zac_s == [0, 6, 11, 16, 21] and cd_skup, f"scfsi_band: [S] {skup} / [CD] {bool(cd_skup)}"
    von.append("/// [S] tabuľka 15 = CD 11172-3 2.4.3.4 – začiatky skupín scfsi_band (+ koniec) – zhodné.")
    von.append(f"pub(crate) static SCFSI_SKUPINY: [u8; 5] = [{', '.join(map(str, zac_s))}];")
    print("\n".join(von))


if __name__ == "__main__":
    main()
