#!/usr/bin/env python3
"""Kontrola stavovej obrazovky SKLENÍKA v emulátore QEMU – bez ukladania obrázka.

Cez monitor QEMU urobí snímku obrazovky do dočasného súboru, zmeria ju (rozmery, podiel pozadia,
textu a zeleného nadpisu) a súbor hneď zmaže. Obrázok sa nikde neuchováva.

Použitie: python3 nastroje/kontrola_obrazovky.py <socket monitora> <šírka> <výška>
Len štandardná knižnica Pythonu.
"""

import os
import socket
import sys
import tempfile
import time

POZADIE = (0x0F, 0x1C, 0x15)
ZELENA = (0x6F, 0xD0, 0x8C)


def blizko(a, b, tol=24):
    return all(abs(x - y) <= tol for x, y in zip(a, b))


def snimka(cesta_socketu: str, ciel: str) -> None:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect(cesta_socketu)
    s.settimeout(5)
    time.sleep(0.3)
    s.recv(4096)
    s.sendall(f"screendump {ciel}\n".encode())
    for _ in range(50):
        time.sleep(0.2)
        if os.path.exists(ciel) and os.path.getsize(ciel) > 0:
            break
    time.sleep(0.5)
    s.close()


def citaj_ppm(cesta: str):
    data = open(cesta, "rb").read()
    casti, i = [], 0
    while len(casti) < 4:
        while data[i:i + 1].isspace():
            i += 1
        j = i
        while not data[j:j + 1].isspace():
            j += 1
        casti.append(data[i:j])
        i = j
    if casti[0] != b"P6":
        raise ValueError("snímka nie je PPM P6")
    w, h = int(casti[1]), int(casti[2])
    return w, h, data[i + 1:i + 1 + w * h * 3]


def main() -> int:
    if len(sys.argv) != 4:
        print(__doc__)
        return 1
    sock, ocak_w, ocak_h = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    fd, cesta = tempfile.mkstemp(suffix=".ppm")
    os.close(fd)
    os.unlink(cesta)
    try:
        snimka(sock, cesta)
        w, h, body = citaj_ppm(cesta)
    finally:
        if os.path.exists(cesta):
            os.unlink(cesta)
    n = w * h
    pozadie = text = zelena = zelena_hore = 0
    for k in range(n):
        b = (body[3 * k], body[3 * k + 1], body[3 * k + 2])
        if blizko(b, POZADIE, 6):
            pozadie += 1
            continue
        text += 1
        if blizko(b, ZELENA):
            zelena += 1
            if k // w < h // 8:
                zelena_hore += 1
    chyby = []
    if (w, h) != (ocak_w, ocak_h):
        chyby.append(f"rozmer {w}×{h}, čakal sa {ocak_w}×{ocak_h}")
    if not 0.6 <= pozadie / n <= 0.995:
        chyby.append(f"pozadie tvorí {100 * pozadie / n:.1f} % (čaká sa 60–99,5 %)")
    if text / n < 0.005:
        chyby.append("na obrazovke takmer nie je text")
    if zelena_hore < 20:
        chyby.append("chýba zelený nadpis ORCHIDEA OS hore")
    print(f"obrazovka {w}×{h}: pozadie {100 * pozadie / n:.1f} %, text {100 * text / n:.1f} %, "
          f"zelená {100 * zelena / n:.2f} % (snímka zmazaná)")
    for c in chyby:
        print("❌", c)
    return 1 if chyby else 0


if __name__ == "__main__":
    sys.exit(main())
