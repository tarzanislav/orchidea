#!/usr/bin/env python3
"""Vytvorí initramfs SKLENÍKA (formát cpio „newc“, komprimovaný gzip) – celý systém v pamäti.

Použitie: python3 nastroje/initramfs.py <cesta k programu KLÍČ> <výstup.cpio.gz> [cesta/v/systeme=subor ...]
Používa len štandardnú knižnicu Pythonu; bez programu cpio.
"""

import gzip
import sys
from pathlib import Path


def zaznam(meno: str, rezim: int, data: bytes = b"", ino: int = 1, zariadenie: tuple = (0, 0)) -> bytes:
    meno_b = meno.encode() + b"\0"
    hlavicka = "070701" + "".join(
        f"{x:08x}"
        for x in (ino, rezim, 0, 0, 1, 0, len(data), 0, 0, zariadenie[0], zariadenie[1], len(meno_b), 0)
    )
    out = hlavicka.encode() + meno_b
    out += b"\0" * (-len(out) % 4)
    out += data
    out += b"\0" * (-len(out) % 4)
    return out


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__)
        return 1
    klic, vystup = Path(sys.argv[1]), Path(sys.argv[2])
    obsah = bytearray()
    ino = 1
    for d in ("dev", "proc", "sys", "run", "tmp", "usr", "usr/bin"):
        obsah += zaznam(d, 0o040755, ino=ino)
        ino += 1
    # konzola musí existovať skôr, než jadro spustí KLÍČ (inak by nemal kam písať)
    obsah += zaznam("dev/console", 0o020600, ino=ino, zariadenie=(5, 1))
    ino += 1
    # ak konzola nie je k dispozícii (napr. bez sériovej linky), štandardné vstupy a výstupy KLÍČA
    # dostanú /dev/null – program beží ďalej a hlási sa do denníka jadra
    obsah += zaznam("dev/null", 0o020666, ino=ino, zariadenie=(1, 3))
    ino += 1
    obsah += zaznam("init", 0o100755, klic.read_bytes(), ino=ino)
    ino += 1
    for par in sys.argv[3:]:
        ciel, zdroj = par.split("=", 1)
        obsah += zaznam(ciel.lstrip("/"), 0o100755, Path(zdroj).read_bytes(), ino=ino)
        ino += 1
    obsah += zaznam("TRAILER!!!", 0)
    vystup.write_bytes(gzip.compress(bytes(obsah), compresslevel=9, mtime=0))
    print(f"{vystup}: {len(obsah)} B → {vystup.stat().st_size} B")
    return 0


if __name__ == "__main__":
    sys.exit(main())
