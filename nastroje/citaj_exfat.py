#!/usr/bin/env python3
"""Nezávislá kontrola zväzku exFAT od SADIČA: nájde súbor podľa cesty a vypíše jeho SHA-256.

Píše sa priamo podľa špecifikácie exFAT (zavádzací sektor, FAT, sady záznamov priečinka, príznak
NoFatChain), bez kódu SADIČA – ak SADIČ zapíše záznamy alebo dáta zle, súčet nebude sedieť.

    python3 citaj_exfat.py oddiel.img sources/install.wim
"""

import hashlib
import struct
import sys


def main(cesta_obrazu: str, cesta: str) -> int:
    f = open(cesta_obrazu, "rb")
    b = f.read(512)
    if b[3:11] != b"EXFAT   ":
        print("nie je exFAT")
        return 1
    fat_od, _fat_dlzka, halda, klastre, koren = struct.unpack_from("<IIIII", b, 80)
    sektor = 1 << b[108]
    klaster = sektor << b[109]

    def fat(c: int) -> int:
        f.seek(fat_od * sektor + c * 4)
        return struct.unpack("<I", f.read(4))[0]

    def klastre_retaze(c: int, n: int | None, bez_fat: bool) -> list[int]:
        if bez_fat:
            return list(range(c, c + n))
        v = []
        while 2 <= c < 0xFFFFFFF7:
            v.append(c)
            c = fat(c)
            if len(v) > klastre:
                raise RuntimeError("slučka vo FAT")
        return v

    def citaj(c: int, dlzka: int, bez_fat: bool):
        zostava = dlzka
        for k in klastre_retaze(c, -(-dlzka // klaster), bez_fat):
            f.seek((halda + (k - 2) * (klaster // sektor)) * sektor)
            kus = f.read(min(klaster, zostava))
            zostava -= len(kus)
            yield kus
            if zostava <= 0:
                break

    def zaznamy(c: int, dlzka: int | None, bez_fat: bool):
        data = b"".join(citaj(c, dlzka, bez_fat)) if dlzka else b"".join(
            citaj(c, len(klastre_retaze(c, None, False)) * klaster, False)
        )
        i = 0
        while i + 32 <= len(data):
            t = data[i]
            if t == 0:
                break
            if t == 0x85:
                pocet = data[i + 1]
                sada = data[i:i + 32 * (pocet + 1)]
                sucet = 0
                for j, x in enumerate(sada):
                    if j not in (2, 3):
                        sucet = (((sucet >> 1) | (sucet << 15)) & 0xFFFF) + x & 0xFFFF
                if sucet != struct.unpack_from("<H", sada, 2)[0]:
                    raise RuntimeError("zlý kontrolný súčet sady záznamov")
                prud = sada[32:64]
                assert prud[0] == 0xC0, "chýba rozšírenie prúdu"
                dlzka_mena = prud[3]
                meno = b"".join(sada[64 + 32 * k + 2:64 + 32 * k + 32] for k in range(pocet - 1))
                meno = meno[:2 * dlzka_mena].decode("utf-16-le")
                priecinok = struct.unpack_from("<H", sada, 4)[0] & 0x10 != 0
                prvy = struct.unpack_from("<I", prud, 20)[0]
                velkost = struct.unpack_from("<Q", prud, 24)[0]
                yield meno, priecinok, prvy, velkost, prud[1] & 2 != 0
                i += 32 * (pocet + 1)
            else:
                i += 32

    c, dlzka, bez_fat = koren, None, False
    casti = [x for x in cesta.split("/") if x]
    for i, cast in enumerate(casti):
        najdene = None
        for meno, priecinok, prvy, velkost, nf in zaznamy(c, dlzka, bez_fat):
            if meno.lower() == cast.lower():
                najdene = (priecinok, prvy, velkost, nf)
                break
        if najdene is None:
            print(f"nenájdené: {cast}")
            return 1
        priecinok, c, dlzka, bez_fat = najdene
        if i + 1 < len(casti) and not priecinok:
            print(f"{cast} nie je priečinok")
            return 1
    h = hashlib.sha256()
    for kus in citaj(c, dlzka, bez_fat):
        h.update(kus)
    print(f"{h.hexdigest()}  {dlzka}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1], sys.argv[2]))
