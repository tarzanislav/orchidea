#!/usr/bin/env python3
"""Vygeneruje pohlad/src/klavesnica/tabulky.rs z normových dát:

- mená a kódy klávesových symbolov X11 a ich znaky Unicode z registra keysymdef.h (a mien výrobcov XF86keysym.h, Sunkeysym.h, DECkeysym.h, HPkeysym.h)
  (balík xorgproto / x11proto-dev, licencia MIT/X11; kódy definuje „X Window System Protocol“, príloha A),
- spojenia mŕtvych klávesov s písmenami z rozkladu znakov v databáze Unicode (UCD, modul unicodedata).

Použitie: tabulky_klavesnice.py <priečinok s keysymdef.h> <výstup.rs>
Voliteľne overí výsledok proti libxkbcommon (nezávislý rozhodca), ak je nainštalovaná.
"""
import re, sys, unicodedata, os

def citaj(subor, predpona_mena):
    mena, znaky = {}, {}
    for r in open(subor, encoding="utf-8", errors="replace"):
        # XF86keysym.h: _EVDEVK(kód) = 0x10081000 + kód jadra Linuxu
        m = re.match(r"#define\s+(\w*?)XK_(\w+)\s+(_EVDEVK\()?0x([0-9a-fA-F]+)\)?\s*(/\*.*)?", r)
        if not m or m.group(1) != predpona_mena:
            continue
        meno = predpona_mena + m.group(2)
        v = int(m.group(4), 16) + (0x10081000 if m.group(3) else 0)
        mena.setdefault(meno, v)
        u = re.search(r"U\+([0-9A-Fa-f]{4,6})", m.group(5) or "")
        if u and v not in znaky:
            znaky[v] = int(u.group(1), 16)
    return mena, znaky

def main():
    zdroj, vystup = sys.argv[1], sys.argv[2]
    mena, znaky = citaj(os.path.join(zdroj, "keysymdef.h"), "")
    mena["NoSymbol"] = 0
    for subor, predpona in [("XF86keysym.h", "XF86"), ("Sunkeysym.h", "Sun"), ("DECkeysym.h", "D"), ("HPkeysym.h", "hp"), ("HPkeysym.h", "osf")]:
        m2, _ = citaj(os.path.join(zdroj, subor), predpona)
        for k, v in m2.items():
            mena.setdefault(k, v)
    # znaky mimo priameho kódovania (Latin-1 a 0x01000000 + Unicode) a mimo riadiacich klávesov (rieši ich kód)
    znaky = {k: u for k, u in znaky.items() if not (0x20 <= k <= 0x7e or 0xa0 <= k <= 0xff or k >= 0x01000000 or 0xff00 <= k <= 0xffff)}
    mrtve = {0xfe50: 0x300, 0xfe51: 0x301, 0xfe52: 0x302, 0xfe53: 0x303, 0xfe54: 0x304, 0xfe55: 0x306, 0xfe56: 0x307,
             0xfe57: 0x308, 0xfe58: 0x30a, 0xfe59: 0x30b, 0xfe5a: 0x30c, 0xfe5b: 0x327, 0xfe5c: 0x328}
    samostatne = {0xfe50: 0x60, 0xfe51: 0xb4, 0xfe52: 0x5e, 0xfe53: 0x7e, 0xfe54: 0xaf, 0xfe55: 0x2d8, 0xfe56: 0x2d9,
                  0xfe57: 0xa8, 0xfe58: 0x2da, 0xfe59: 0x2dd, 0xfe5a: 0x2c7, 0xfe5b: 0xb8, 0xfe5c: 0x2db}
    spoj = []
    for cp in list(range(0xc0, 0x250)) + list(range(0x1e00, 0x1f00)):
        d = unicodedata.decomposition(chr(cp)).split()
        if len(d) == 2 and not d[0].startswith("<"):
            b, m = int(d[0], 16), int(d[1], 16)
            if b < 0x80 and chr(b).isalpha():
                for k, mm in mrtve.items():
                    if mm == m:
                        spoj.append((k, b, cp))
    with open(vystup, "w") as f:
        f.write("// Vygenerované nástrojom pohlad/nastroje/tabulky_klavesnice.py z registra klávesových symbolov X11\n")
        f.write("// (keysymdef.h, XF86keysym.h – xorgproto, licencia MIT/X11) a z databázy Unicode (UCD).\n\n")
        f.write("/// Meno klávesového symbolu → kód (zoradené podľa mena).\n")
        f.write("pub(super) const MENA: &[(&str, u32)] = &[\n")
        for n, v in sorted(mena.items(), key=lambda t: t[0].encode()):
            f.write(f"    (\"{n}\", 0x{v:x}),\n")
        f.write("];\n\n/// Kód klávesového symbolu → znak (mimo Latin-1 a priameho kódovania Unicode).\n")
        f.write("pub(super) const ZNAKY: &[(u32, char)] = &[\n")
        for k, u in sorted(znaky.items()):
            f.write(f"    (0x{k:x}, '\\u{{{u:x}}}'),\n")
        f.write("];\n\n/// Mŕtvy kláves + písmeno → písmeno so znamienkom.\npub(super) const SPOJENIA: &[(u32, char, char)] = &[\n")
        for k, b, c in sorted(spoj):
            f.write(f"    (0x{k:x}, '{chr(b)}', '\\u{{{c:x}}}'),\n")
        f.write("];\n\n/// Mŕtvy kláves samotný (po medzere alebo dvakrát) → znamienko.\npub(super) const SAMOSTATNE: &[(u32, char)] = &[\n")
        for k, c in sorted(samostatne.items()):
            f.write(f"    (0x{k:x}, '\\u{{{c:x}}}'),\n")
        f.write("];\n")
    print(f"{len(mena)} mien, {len(znaky)} znakov, {len(spoj)} spojení")
    try:
        import ctypes
        x = ctypes.CDLL("libxkbcommon.so.0")
    except OSError:
        return
    x.xkb_keysym_from_name.restype = ctypes.c_uint32
    x.xkb_keysym_to_utf32.restype = ctypes.c_uint32
    zle = [n for n, v in mena.items() if x.xkb_keysym_from_name(n.encode(), 0) != v]
    zle2 = [hex(k) for k, u in znaky.items() if x.xkb_keysym_to_utf32(k) != u]
    print("rozhodca libxkbcommon – nezhody mien:", zle[:20], len(zle), " znakov:", zle2[:20], len(zle2))

main()
