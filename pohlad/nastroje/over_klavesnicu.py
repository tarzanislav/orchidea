#!/usr/bin/env python3
"""Porovná rozbor rozložení klávesnice XKB (okno pre Linux) s libxkbcommon ako nezávislým rozhodcom.

Rozloženia skompiluje `xkbcli compile-keymap` (balík libxkbcommon-tools); pre každý kláves, skupinu
a kombináciu modifikátorov porovná klávesový symbol (xkb_state_key_get_syms) a znak (xkb_state_key_get_utf32).
Použitie: over_klavesnicu.py <prikazy> [rozloženie …]
"""
import ctypes, os, subprocess, sys, tempfile

def main():
    prikazy = sys.argv[1]
    rozlozenia = sys.argv[2:] or ["sk", "sk(qwerty)", "cz", "us", "de", "fr", "pl", "hu", "gb", "ru", "ua", "sk,us", "us,sk", "cz,sk", "de(nodeadkeys)", "us(intl)", "us(dvorak)"]
    x = ctypes.CDLL("libxkbcommon.so.0")
    x.xkb_context_new.restype = ctypes.c_void_p
    x.xkb_keymap_new_from_string.restype = ctypes.c_void_p
    x.xkb_keymap_new_from_string.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int, ctypes.c_int]
    x.xkb_state_new.restype = ctypes.c_void_p
    x.xkb_state_new.argtypes = [ctypes.c_void_p]
    x.xkb_state_update_mask.argtypes = [ctypes.c_void_p] + [ctypes.c_uint32] * 6
    x.xkb_state_key_get_utf32.restype = ctypes.c_uint32
    x.xkb_state_key_get_utf32.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
    x.xkb_state_key_get_syms.restype = ctypes.c_int
    x.xkb_state_key_get_syms.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.POINTER(ctypes.POINTER(ctypes.c_uint32))]
    x.xkb_state_unref.argtypes = [ctypes.c_void_p]
    ctx = x.xkb_context_new(0)
    spolu = zle_spolu = 0
    for r in rozlozenia:
        if "(" in r:
            lay, var = r.split("(")[0], r.split("(")[1].rstrip(")")
            arg = ["--layout", lay, "--variant", var]
        else:
            arg = ["--layout", r]
        text = subprocess.run(["xkbcli", "compile-keymap"] + arg, capture_output=True, text=True, check=True).stdout
        km = x.xkb_keymap_new_from_string(ctx, text.encode(), 1, 0)
        with tempfile.NamedTemporaryFile("w", suffix=".xkb", delete=False) as f:
            f.write(text)
        nase = subprocess.run([prikazy, "klavesnica", f.name], capture_output=True, text=True, check=True).stdout
        os.unlink(f.name)
        zle = []
        n = 0
        for riadok in nase.splitlines():
            _, kod, st, zam, sk, sym, zn = riadok.split()
            kod, st, zam, sk, sym = int(kod), int(st), int(zam), int(sk), int(sym, 16)
            s = x.xkb_state_new(km)
            x.xkb_state_update_mask(s, st, 0, zam, 0, 0, sk)
            p = ctypes.POINTER(ctypes.c_uint32)()
            k = x.xkb_state_key_get_syms(s, kod + 8, ctypes.byref(p))
            ich_sym = p[0] if k > 0 else 0
            u = x.xkb_state_key_get_utf32(s, kod + 8)
            x.xkb_state_unref(s)
            nas_z = None if zn == "-" else int(zn, 16)
            ich_z = u or None
            # mŕtve klávesy: libxkbcommon dáva znak 0 (skladanie robí až compose) – my tiež nič
            # Ctrl: libxkbcommon dá riadiaci znak aj pre nepísmená (napr. Ctrl+1 = '1'); my len pre písmená a @[\]^_
            if st & 4 and nas_z is None and ich_z is not None and not (0x40 <= ich_z < 0x60 or 0x60 <= ich_z < 0x7f):
                ich_z = None
            if st & 4 and ich_z is not None and ich_z >= 0x20 and nas_z is None:
                ich_z = None
            # Shift+Tab (ISO_Left_Tab) píše tabulátor ako vo Windows (prepína panel); libxkbcommon nič
            if sym == 0xfe20 and nas_z == 9 and ich_z is None:
                ich_z = 9
            # libxkbcommon 1.6: Caps Lock pri µ a ß dá neplatný symbol (0x39c, 0x1e9e bez 0x01000000) – bez znaku
            if (st | zam) & 2 and sym in (0xb5, 0xdf) and ich_z is None:
                ich_z = nas_z
            n += 1
            if sym != ich_sym or nas_z != ich_z:
                zle.append((kod, st, zam, sk, hex(sym), hex(ich_sym), nas_z, ich_z))
        spolu += n
        zle_spolu += len(zle)
        print(f"{r:16} kombinácií {n:6}  nezhôd {len(zle)}")
        for z in zle[:8]:
            print("   kód %d mod %x zámok %x skupina %d: náš symbol %s, libxkbcommon %s; znak %r / %r" % z)
    print(f"SPOLU kombinácií {spolu}, nezhôd {zle_spolu}")
    sys.exit(1 if zle_spolu else 0)

main()
