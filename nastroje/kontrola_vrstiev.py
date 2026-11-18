#!/usr/bin/env python3
"""Strážca vrstiev a neutrality ORCHIDEE.

Overí, že:
  1. každý balík kódu má v Cargo.toml údaj [package.metadata.orchidea] vrstva = 1..4,
  2. závislosti idú len nadol (balík smie závisieť len od balíka s rovnakou alebo nižšou vrstvou)
     a jadro (vrstva 1) nezávisí od ničoho,
  3. žiadny balík nemá cudziu závislosť (všetky závislosti sú vlastné balíky cez `path`),
  4. zdrojový kód vrstvy 1 (jadro) neobsahuje pojmy viazané na odbor, povolanie či postup
     (test neutrality, zoznam v nastroje/neutralita.txt).

Vrstvy: 1 jadro (gramatika) · 2 schopnosti · 3 ateliéry · 4 profily.
Používa len štandardnú knižnicu Pythonu (3.11+). Návratový kód 0 = v poriadku, 1 = chyba.
"""

import re
import sys
import tomllib
import unicodedata
from pathlib import Path

KOREN = Path(__file__).resolve().parent.parent
DRUHY_ZAVISLOSTI = ("dependencies", "dev-dependencies", "build-dependencies")


def bez_diakritiky(text: str) -> str:
    return "".join(z for z in unicodedata.normalize("NFD", text) if unicodedata.category(z) != "Mn")


def nacitaj_balicky():
    pracovny = tomllib.loads((KOREN / "Cargo.toml").read_text(encoding="utf-8"))
    clenovia = pracovny.get("workspace", {}).get("members", [])
    spolocne = pracovny.get("workspace", {}).get("dependencies", {})
    balicky = {}
    for clen in clenovia:
        cesta = KOREN / clen / "Cargo.toml"
        data = tomllib.loads(cesta.read_text(encoding="utf-8"))
        meno = data["package"]["name"]
        balicky[meno] = {"priecinok": KOREN / clen, "data": data}
    return balicky, spolocne


def kontrola(balicky, spolocne):
    chyby = []
    vrstvy = {}
    for meno, b in balicky.items():
        vrstva = b["data"].get("package", {}).get("metadata", {}).get("orchidea", {}).get("vrstva")
        if vrstva not in (1, 2, 3, 4):
            chyby.append(f"{meno}: chýba alebo je neplatný údaj [package.metadata.orchidea] vrstva (1–4)")
        vrstvy[meno] = vrstva

    for meno, b in balicky.items():
        for druh in DRUHY_ZAVISLOSTI:
            tabulky = [b["data"].get(druh, {})]
            for ciel in b["data"].get("target", {}).values():
                tabulky.append(ciel.get(druh, {}))
            for tabulka in tabulky:
                for dep, spec in tabulka.items():
                    if isinstance(spec, dict) and spec.get("workspace"):
                        spec = spolocne.get(dep, {})
                    if not (isinstance(spec, dict) and "path" in spec):
                        chyby.append(f"{meno}: závislosť '{dep}' nie je vlastný balík (cudzie balíky sú zakázané)")
                        continue
                    if dep not in vrstvy:
                        chyby.append(f"{meno}: závislosť '{dep}' nie je členom pracovného priestoru")
                        continue
                    if vrstvy[meno] == 1 and druh != "dev-dependencies":
                        chyby.append(f"{meno}: jadro (vrstva 1) nesmie závisieť od iného balíka ('{dep}')")
                    elif (vrstvy[dep] or 0) > (vrstvy[meno] or 0):
                        chyby.append(
                            f"{meno} (vrstva {vrstvy[meno]}) závisí od '{dep}' (vrstva {vrstvy[dep]}) – "
                            "závislosti smú ísť len nadol"
                        )
    return chyby, vrstvy


def kontrola_neutrality(balicky, vrstvy):
    zoznam = (KOREN / "nastroje" / "neutralita.txt").read_text(encoding="utf-8").splitlines()
    pojmy = [bez_diakritiky(r.strip().lower()) for r in zoznam if r.strip() and not r.startswith("#")]
    # pojem musí začínať slovo (pred ním nie je písmeno) – „zlučiteľná“ teda neobsahuje „učiteľ“
    vzor = re.compile("(?<![a-z])(?:" + "|".join(re.escape(p) for p in pojmy) + ")") if pojmy else None
    chyby = []
    if vzor is None:
        return chyby
    for meno, b in balicky.items():
        if vrstvy.get(meno) != 1:
            continue
        for subor in sorted(b["priecinok"].rglob("*.rs")):
            for cislo, riadok in enumerate(subor.read_text(encoding="utf-8").splitlines(), 1):
                najdene = vzor.search(bez_diakritiky(riadok.lower()))
                if najdene:
                    rel = subor.relative_to(KOREN)
                    chyby.append(f"{rel}:{cislo}: pojem viazaný na odbor alebo postup ('{najdene.group(0)}')")
    return chyby


def main():
    balicky, spolocne = nacitaj_balicky()
    chyby, vrstvy = kontrola(balicky, spolocne)
    chyby += kontrola_neutrality(balicky, vrstvy)
    if chyby:
        for c in chyby:
            print(f"CHYBA: {c}")
        return 1
    prehlad = ", ".join(f"{m} (vrstva {v})" for m, v in sorted(vrstvy.items(), key=lambda x: (x[1], x[0])))
    print(f"V poriadku: vrstvy a neutralita – {prehlad}.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
