#!/usr/bin/env python3
"""Nezávislá kontrola zväzku UDF (podľa ECMA-167 a OSTA UDF 2.01), napísaná oddelene od SADIČA.

Overí rozpoznávaciu postupnosť, všetky tri kotvy, hlavnú aj záložnú postupnosť popisov, záznam
celistvosti, bitmapu voľného miesta, popis množiny súborov a koreňový priečinok – pri každom popise
kontrolný súčet značky, CRC a polohu.

    python3 over_udf.py disk.img [veľkosť_bloku]
"""
import struct
import sys


def crc_itu(d):
    c = 0
    for b in d:
        c ^= b << 8
        for _ in range(8):
            c = ((c << 1) ^ 0x1021) & 0xFFFF if c & 0x8000 else (c << 1) & 0xFFFF
    return c


class Chyba(Exception):
    pass


def nacitaj(f, blok, bs, n=1):
    f.seek(blok * bs)
    return f.read(n * bs)


def znacka(d, ocakavane_id, poloha, kde):
    tid, ver, sucet, _, _, crc, dlzka, loc = struct.unpack("<HHBBHHHI", d[:16])
    if tid != ocakavane_id:
        raise Chyba(f"{kde}: značka {tid}, čakal som {ocakavane_id}")
    if ver not in (2, 3):
        raise Chyba(f"{kde}: verzia popisu {ver}")
    if sum(d[:4]) + sum(d[5:16]) & 0xFF != sucet:
        raise Chyba(f"{kde}: kontrolný súčet značky")
    if 16 + dlzka > len(d) or crc_itu(d[16:16 + dlzka]) != crc:
        raise Chyba(f"{kde}: CRC popisu")
    if loc != poloha:
        raise Chyba(f"{kde}: poloha v značke {loc}, je na {poloha}")
    return dlzka


def dstring(d):
    n = d[-1]
    if n == 0:
        return ""
    if d[0] == 8:
        return d[1:n].decode("latin-1")
    if d[0] == 16:
        return d[1:n].decode("utf-16-be")
    raise Chyba(f"d-string s kompresiou {d[0]}")


def over(cesta, bs):
    with open(cesta, "rb") as f:
        f.seek(0, 2)
        blokov = f.tell() // bs
        # rozpoznávacia postupnosť od 32 KB, každý záznam na celom sektore (aspoň 2 KB)
        krok = max(bs, 2048)
        vrs = []
        for i in range(3):
            f.seek(32768 + i * krok)
            z = f.read(7)
            vrs.append(z[1:6].decode("ascii", "replace"))
            if z[0] != 0 or z[6] != 1:
                raise Chyba(f"VRS {i}: typ {z[0]}, verzia {z[6]}")
        if vrs != ["BEA01", "NSR03", "TEA01"] and vrs != ["BEA01", "NSR02", "TEA01"]:
            raise Chyba(f"VRS: {vrs}")
        # kotvy
        kotvy = []
        for p in (256, blokov - 257, blokov - 1):
            d = nacitaj(f, p, bs)
            znacka(d, 2, p, f"kotva {p}")
            kotvy.append(struct.unpack("<IIII", d[16:32]))
        if len(set(kotvy)) != 1:
            raise Chyba("kotvy sa líšia")
        hl_dlzka, hl_zac, zal_dlzka, zal_zac = kotvy[0]
        # postupnosti popisov
        postupnosti = []
        for zac, dlzka, nazov in ((hl_zac, hl_dlzka, "hlavná"), (zal_zac, zal_dlzka, "záložná")):
            popisy = {}
            for i in range(dlzka // bs):
                d = nacitaj(f, zac + i, bs)
                tid = struct.unpack("<H", d[:2])[0]
                if tid == 0:
                    continue
                znacka(d, tid, zac + i, f"{nazov} postupnosť, blok {zac + i}")
                if tid == 8:
                    break
                popisy[tid] = d[16:]
            else:
                raise Chyba(f"{nazov} postupnosť bez ukončenia")
            for t in (1, 4, 5, 6):
                if t not in popisy:
                    raise Chyba(f"{nazov} postupnosť: chýba popis {t}")
            postupnosti.append(popisy)
        h = postupnosti[0]
        if {k: v[4:] for k, v in h.items()} != {k: v[4:] for k, v in postupnosti[1].items()}:
            raise Chyba("hlavná a záložná postupnosť sa líšia")
        pvd, lvd, pd = h[1], h[6], h[5]
        menovka = dstring(pvd[24 - 16:56 - 16])
        # logický zväzok
        if struct.unpack("<I", lvd[212 - 16:216 - 16])[0] != bs:
            raise Chyba("veľkosť logického bloku sa nezhoduje")
        domena = lvd[217 - 16:240 - 16].rstrip(b"\0")
        if domena != b"*OSTA UDF Compliant":
            raise Chyba(f"doména {domena}")
        rev = struct.unpack("<H", lvd[240 - 16:242 - 16])[0]
        fsd_dl, fsd_blok, fsd_odd = struct.unpack("<IIH", lvd[248 - 16:258 - 16])
        lvid_dl, lvid_zac = struct.unpack("<II", lvd[432 - 16:440 - 16])
        mapa = lvd[440 - 16:446 - 16]
        if mapa[0] != 1 or mapa[1] != 6:
            raise Chyba("mapa oddielu nie je typu 1")
        # oddiel
        if pd[24 - 16 + 1:24 - 16 + 7] != b"+NSR03" and pd[24 - 16 + 1:24 - 16 + 7] != b"+NSR02":
            raise Chyba("obsah oddielu nie je NSR")
        bit_dl, bit_poz = struct.unpack("<II", pd[64 - 16:72 - 16])
        odd_zac, odd_dl = struct.unpack("<II", pd[188 - 16:196 - 16])
        if odd_zac + odd_dl > blokov:
            raise Chyba("oddiel presahuje disk")
        # celistvosť
        d = nacitaj(f, lvid_zac, bs)
        znacka(d, 9, lvid_zac, "záznam celistvosti")
        typ = struct.unpack("<I", d[28:32])[0]
        if typ != 1:
            raise Chyba("zväzok nie je zatvorený")
        n_odd, dl_impl = struct.unpack("<II", d[72:80])
        volne, velkost = struct.unpack("<II", d[80:88])
        subory, priecinky = struct.unpack("<II", d[88 + 32:88 + 40])
        if velkost != odd_dl:
            raise Chyba("veľkosť oddielu v zázname celistvosti")
        # bitmapa voľného miesta
        d = nacitaj(f, odd_zac + bit_poz, bs, -(-bit_dl // bs))
        znacka(d, 264, bit_poz, "bitmapa")
        bitov, bajtov = struct.unpack("<II", d[16:24])
        if bitov != odd_dl or bajtov != -(-bitov // 8):
            raise Chyba("rozmer bitmapy")
        bitmapa = d[24:24 + bajtov]
        nastavene = sum(bin(b).count("1") for b in bitmapa)
        if nastavene != volne:
            raise Chyba(f"bitmapa: voľných {nastavene}, záznam celistvosti hovorí {volne}")

        def obsadeny(blok):
            return not (bitmapa[blok // 8] >> (blok % 8)) & 1

        # popis množiny súborov
        if not obsadeny(fsd_blok):
            raise Chyba("popis množiny súborov leží vo voľnom mieste")
        d = nacitaj(f, odd_zac + fsd_blok, bs)
        znacka(d, 256, fsd_blok, "popis množiny súborov")
        koren_dl, koren = struct.unpack("<II", d[400:408])
        if d[417:436].rstrip(b"\0") != b"*OSTA UDF Compliant":
            raise Chyba("doména v popise množiny súborov")
        # koreňový priečinok
        if not obsadeny(koren):
            raise Chyba("koreň leží vo voľnom mieste")
        d = nacitaj(f, odd_zac + koren, bs)
        tid = struct.unpack("<H", d[:2])[0]
        znacka(d, tid, koren, "koreňový priečinok")
        if tid not in (261, 266):
            raise Chyba(f"koreň nie je záznam súboru ({tid})")
        if d[27] != 4:
            raise Chyba("koreň nie je priečinok")
        druh_ad = struct.unpack("<H", d[34:36])[0] & 7
        if tid == 266:
            dl_info, = struct.unpack("<Q", d[56:64])
            dl_ea, dl_ad = struct.unpack("<II", d[208:216])
            zac_ad = 216 + dl_ea
        else:
            dl_info, = struct.unpack("<Q", d[56:64])
            dl_ea, dl_ad = struct.unpack("<II", d[168:176])
            zac_ad = 176 + dl_ea
        if druh_ad != 3:
            raise Chyba("očakávam obsah priečinka priamo v ICB")
        obsah = d[zac_ad:zac_ad + dl_ad]
        if len(obsah) != dl_info:
            raise Chyba("dĺžka obsahu priečinka")
        p = 0
        polozky = 0
        while p < len(obsah):
            fid = obsah[p:]
            znacka(fid, 257, koren, f"FID {polozky}")
            dl_mena = fid[19]
            icb_dl, icb_blok = struct.unpack("<II", fid[20:28])
            dl_impl = struct.unpack("<H", fid[36:38])[0]
            if polozky == 0 and (fid[18] & 0x0A != 0x0A or icb_blok != koren):
                raise Chyba("prvá položka koreňa nie je odkaz na rodiča (seba)")
            p += (38 + dl_impl + dl_mena + 3) // 4 * 4
            polozky += 1
        lv = dstring(lvd[84 - 16:212 - 16])
        print(f"UDF {rev >> 8}.{rev & 0xFF:02}: „{menovka}“ (logický zväzok „{lv}“), blok {bs} B, "
              f"{blokov} blokov, oddiel {odd_zac}+{odd_dl}, voľných {volne}, {priecinky} priečinok, "
              f"{subory} súborov – všetky popisy, CRC a kotvy v poriadku")


if __name__ == "__main__":
    try:
        over(sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 512)
    except Chyba as e:
        print(f"❌ UDF: {e}")
        sys.exit(1)
