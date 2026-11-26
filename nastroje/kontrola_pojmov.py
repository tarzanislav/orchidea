#!/usr/bin/env python3
"""Kontrola pojmov (ORCHIDEA/terminologia.md, časť 7): tvorivý názov smie byť v textoch pre ľudí len spolu
s tradičným pojmom alebo menom produktu – napríklad „SKLENÍK“ len vedľa „ORCHIDEA OS“, „Pohľad“ len vedľa
„prehliadač a editor fotografií“, „Herbár“ len vedľa „katalóg fotografií“ (v ľubovoľnom páde).

Prechádza:
- reťazce v zdrojovom kóde Rust (mimo komentárov a modulov so skúškami),
- README.md a návody v balíkoch (NAVOD.txt),
- popisy balíkov (description v Cargo.toml),
- texty v pracovných postupoch CI, ktoré vidí človek (name:, echo, printf, Write-Output).

Výnimka pre jeden riadok: značka „pojmy: výnimka“ v riadku (napríklad v komentári za reťazcom).
Použitie: python3 nastroje/kontrola_pojmov.py   (kód 1 pri nájdenom probléme)
"""
import pathlib
import re
import sys

KOREN = pathlib.Path(__file__).resolve().parent.parent

# (tvorivý názov – regulárny výraz, čo musí byť v tom istom texte)
DVOJICE = [
    (re.compile(r"SKLENÍK\w*"), "ORCHIDEA OS"),
    (re.compile(r"SADIČ\w*"), "Zapisovač"),
    (re.compile(r"\bPÔD[AUYE]\b"), "rofil hardvéru"),
    (re.compile(r"SÚPIS\w*"), "periféri"),
    (re.compile(r"STRÁŽC\w*"), "REGULÁTOR"),
    # 26. 11. 2026, vlastník: tradičný odborný pojem, tvorivý názov v zátvorke
    (re.compile(r"\bPohľad\w*"), re.compile(r"prehliada\w* a editor\w* fotografi", re.I)),
    (re.compile(r"\bHerbár\w*"), re.compile(r"katal[oó]g\w* fotografi", re.I)),
]
RETAZEC = re.compile(r'"(?:[^"\\]|\\.)*"')
VYNIMKA = "pojmy: výnimka"


def priecinky_rust() -> list[pathlib.Path]:
    """Všetky balíky pracovného priestoru podľa Cargo.toml (aj tie, ktoré pridajú iné relácie)."""
    import tomllib

    with open(KOREN / "Cargo.toml", "rb") as f:
        clenovia = tomllib.load(f).get("workspace", {}).get("members", [])
    return [KOREN / c for c in clenovia]


# Popisy balíkov ORCHIDEA OS ešte používajú staré názvy – prevedie ich relácia ORCHIDEA OS (issue v repozitári);
# dovtedy sa v popisoch balíkov kontrolujú len ostatné dvojice.
ODLOZENE_V_POPISOCH = ("SKLENÍK", "SADIČ", "PÔD", "SÚPIS", "STRÁŽC")


def problemy(text: str, odlozene: tuple[str, ...] = ()) -> list[str]:
    chyby = []
    for vzor, musi in DVOJICE:
        if any(vzor.pattern.lstrip("\\b").startswith(o) for o in odlozene):
            continue
        for m in vzor.finditer(text):
            if isinstance(musi, str):
                ok, meno = musi.lower() in text.lower(), musi
            else:
                ok, meno = musi.search(text) is not None, musi.pattern
            if not ok:
                chyby.append(f"„{m.group(0)}“ bez „{meno}“")
    return chyby


def texty_rust(cesta: pathlib.Path):
    obsah = cesta.read_text(encoding="utf-8")
    # moduly so skúškami sú na konci súboru – skúšobné dáta nie sú texty pre ľudí
    obsah = re.split(r"^\s*#\[cfg\(test\)\]", obsah, maxsplit=1, flags=re.M)[0]
    for c, riadok in enumerate(obsah.splitlines(), 1):
        if riadok.lstrip().startswith("//") or VYNIMKA in riadok:
            continue
        for r in RETAZEC.findall(riadok):
            yield c, r


def texty_ci(cesta: pathlib.Path):
    for c, riadok in enumerate(cesta.read_text(encoding="utf-8").splitlines(), 1):
        s = riadok.strip()
        if s.startswith("#") or VYNIMKA in riadok:
            continue
        if re.match(r"-?\s*name:", s) or re.search(r"\b(echo|printf|Write-Output)\b", s):
            yield c, s


def main() -> int:
    najdene = []
    for p in priecinky_rust():
        for subor in sorted(p.rglob("*.rs")):
            if "target" in subor.parts:
                continue
            for c, t in texty_rust(subor):
                for ch in problemy(t):
                    najdene.append(f"{subor.relative_to(KOREN)}:{c}: {ch} – {t[:90]}")
    texty = [KOREN / "README.md"] + sorted(p / "NAVOD.txt" for p in priecinky_rust() if (p / "NAVOD.txt").exists())
    for subor in texty:
        for c, riadok in enumerate(subor.read_text(encoding="utf-8").splitlines(), 1):
            if VYNIMKA not in riadok:
                for ch in problemy(riadok):
                    najdene.append(f"{subor.relative_to(KOREN)}:{c}: {ch}")
    for p in priecinky_rust():
        if (p / "Cargo.toml").exists():
            for c, riadok in enumerate((p / "Cargo.toml").read_text(encoding="utf-8").splitlines(), 1):
                if riadok.startswith("description") and VYNIMKA not in riadok:
                    for ch in problemy(riadok, ODLOZENE_V_POPISOCH):
                        najdene.append(f"{(p / 'Cargo.toml').relative_to(KOREN)}:{c}: {ch}")
    for subor in sorted((KOREN / ".github" / "workflows").glob("*.yml")):
        for c, t in texty_ci(subor):
            for ch in problemy(t):
                najdene.append(f"{subor.relative_to(KOREN)}:{c}: {ch} – {t[:90]}")
    if najdene:
        print("Pojmy (ORCHIDEA/terminologia.md): tvorivý názov bez tradičného pojmu:")
        for n in najdene:
            print("  " + n)
        return 1
    print("V poriadku: pojmy podľa ORCHIDEA/terminologia.md.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
