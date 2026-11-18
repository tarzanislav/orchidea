#!/usr/bin/env python3
"""Kontrola charty a registra zdrojov ORCHIDEE.

Overí, že:
  1. charta je v repozitári a súbor NOTICE na ňu odkazuje,
  2. zdroje.json má platný tvar a každý zdroj spĺňa kódex pôvodného kódu,
  3. každý cudzí balík v Cargo.lock je zapísaný v registri.

Používa len štandardnú knižnicu Pythonu. Návratový kód 0 = v poriadku, 1 = chyba.
"""

import json
import sys
from pathlib import Path

KOREN = Path(__file__).resolve().parent.parent
POVINNE_POLIA = ("meno", "trieda", "licencia", "ucel", "posudenie_charty")
TRIEDY = {"B", "C"}  # A je vlastný kód (do registra nepatrí), D a E sú zakázané


def chyby_charty():
    chyby = []
    for subor in ("MIEROVA-CHARTA.md", "PEACE-CHARTER.md", "LICENSE", "NOTICE", "ZNAMKY.md"):
        if not (KOREN / subor).is_file():
            chyby.append(f"chýba súbor {subor}")
    notice = KOREN / "NOTICE"
    if notice.is_file() and "MIEROVA-CHARTA.md" not in notice.read_text(encoding="utf-8"):
        chyby.append("NOTICE neodkazuje na MIEROVA-CHARTA.md")
    return chyby


def nacitaj_register():
    data = json.loads((KOREN / "zdroje.json").read_text(encoding="utf-8"))
    return data, {z.get("meno") for z in data.get("zdroje", [])}


def chyby_registra(data):
    chyby = []
    povolene = set(data.get("povolene_licencie_triedy_C", []))
    for i, zdroj in enumerate(data.get("zdroje", [])):
        meno = zdroj.get("meno", f"#{i}")
        for pole in POVINNE_POLIA:
            if not zdroj.get(pole):
                chyby.append(f"{meno}: chýba pole '{pole}'")
        trieda = zdroj.get("trieda")
        if trieda not in TRIEDY:
            chyby.append(f"{meno}: trieda '{trieda}' nie je dovolená (len B alebo C)")
        if trieda == "C":
            if zdroj.get("licencia") not in povolene:
                chyby.append(f"{meno}: licencia '{zdroj.get('licencia')}' nie je povolená pre triedu C")
            if not zdroj.get("suhlas_vlastnika"):
                chyby.append(f"{meno}: trieda C vyžaduje 'suhlas_vlastnika'")
    return chyby


def cudzie_balicky_cargo():
    """Vráti mená balíkov z Cargo.lock, ktoré majú vonkajší zdroj (nie sú z tohto repozitára)."""
    zamok = KOREN / "Cargo.lock"
    if not zamok.is_file():
        return set()
    mena, meno = set(), None
    for riadok in zamok.read_text(encoding="utf-8").splitlines():
        riadok = riadok.strip()
        if riadok == "[[package]]":
            meno = None
        elif riadok.startswith("name = "):
            meno = riadok.split("=", 1)[1].strip().strip('"')
        elif riadok.startswith("source = ") and meno:
            mena.add(meno)
    return mena


def main():
    chyby = chyby_charty()
    try:
        data, zapisane = nacitaj_register()
    except (OSError, json.JSONDecodeError) as e:
        print(f"CHYBA: zdroje.json sa nedá prečítať: {e}")
        return 1
    chyby += chyby_registra(data)
    for balik in sorted(cudzie_balicky_cargo() - zapisane):
        chyby.append(f"Cargo.lock: balík '{balik}' nie je v zdroje.json")
    if chyby:
        for c in chyby:
            print(f"CHYBA: {c}")
        return 1
    print(f"V poriadku: charta je na mieste, register má {len(zapisane)} zdrojov.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
