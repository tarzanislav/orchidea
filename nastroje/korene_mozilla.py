#!/usr/bin/env python3
"""Zoznam dôveryhodných koreňových autorít pre HTTPS z programu Mozilla CA (A12 krok 5).

Vstup: súbor `certdata.txt` z NSS (Mozilla, licencia MPL 2.0 – dáta, nie kód programu):
  https://raw.githubusercontent.com/mozilla-firefox/firefox/main/security/nss/lib/ckfw/builtins/certdata.txt
Výstup: `siet/data/korene-mozilla.txt` – pre každú autoritu dôveryhodnú pre servery (CKA_TRUST_SERVER_AUTH =
CKT_NSS_TRUSTED_DELEGATOR) riadok s menom, prípadne dátumom „nedôverovať po“ (CKA_NSS_SERVER_DISTRUST_AFTER) a
certifikát DER v base64. KLÍČ má zoznam v sebe (include_str!) – nový zoznam príde aktualizáciou po sieti (R264).

Vlastník 28. 11. 2026, 10:43: verejný zoznam Mozilla ako dáta – súhlas.

Spustenie: python3 -I nastroje/korene_mozilla.py certdata.txt
"""

import base64
import calendar
import hashlib
import pathlib
import sys

ZDROJ = "https://raw.githubusercontent.com/mozilla-firefox/firefox/main/security/nss/lib/ckfw/builtins/certdata.txt"
CIEL = pathlib.Path(__file__).resolve().parent.parent / "siet" / "data" / "korene-mozilla.txt"


def objekty(text):
    """Objekty certdata.txt ako slovníky atribút → hodnota (MULTILINE_OCTAL → bytes)."""
    obj = None
    riadky = iter(text.splitlines())
    for r in riadky:
        if not r or r.startswith("#"):
            continue
        casti = r.split(" ", 2)
        if casti[0] == "CKA_CLASS":
            if obj:
                yield obj
            obj = {}
        if obj is None or len(casti) < 2:
            continue
        meno, typ = casti[0], casti[1]
        if typ == "MULTILINE_OCTAL":
            b = bytearray()
            for r2 in riadky:
                if r2 == "END":
                    break
                for oct_ in r2.split("\\")[1:]:
                    b.append(int(oct_, 8))
            obj[meno] = bytes(b)
        elif typ == "UTF8":
            obj[meno] = casti[2].strip('"')
        else:
            obj[meno] = casti[2] if len(casti) > 2 else typ
    if obj:
        yield obj


def utc_na_unix(t):
    """UTCTime „YYMMDDHHMMSSZ“ → sekundy od 1. 1. 1970."""
    t = t.decode()
    rok = int(t[:2])
    rok += 1900 if rok >= 50 else 2000
    return calendar.timegm((rok, int(t[2:4]), int(t[4:6]), int(t[6:8]), int(t[8:10]), int(t[10:12])))


def main():
    data = pathlib.Path(sys.argv[1]).read_bytes()
    odtlacok = hashlib.sha256(data).hexdigest()
    obj = list(objekty(data.decode("utf-8")))
    certifikaty = {(o["CKA_ISSUER"], o["CKA_SERIAL_NUMBER"]): o for o in obj
                   if o.get("CKA_CLASS") == "CKO_CERTIFICATE" and "CKA_VALUE" in o}
    vystup = [
        "# Koreňové autority pre servery (HTTPS) z programu Mozilla CA – dáta, licencia MPL 2.0 (Mozilla Foundation).",
        f"# Zdroj: {ZDROJ}",
        f"# SHA-256 zdroja: {odtlacok}",
        "# Vytvoril nastroje/korene_mozilla.py. Riadok: „= meno“, voliteľne „po <sekundy>“ (CKA_NSS_SERVER_DISTRUST_AFTER –",
        "# certifikátom servera vydaným neskôr sa nedôveruje), potom certifikát DER v base64.",
    ]
    pocet = 0
    for o in obj:
        if o.get("CKA_CLASS") != "CKO_NSS_TRUST" or o.get("CKA_TRUST_SERVER_AUTH") != "CKT_NSS_TRUSTED_DELEGATOR":
            continue
        c = certifikaty.get((o["CKA_ISSUER"], o["CKA_SERIAL_NUMBER"]))
        if c is None:
            sys.exit(f"dôvera bez certifikátu: {o.get('CKA_LABEL')}")
        vystup.append(f"= {c['CKA_LABEL']}")
        po = c.get("CKA_NSS_SERVER_DISTRUST_AFTER")
        if isinstance(po, bytes):
            vystup.append(f"po {utc_na_unix(po)}")
        vystup.append(base64.b64encode(c["CKA_VALUE"]).decode())
        pocet += 1
    vystup.insert(5, f"# Autorít: {pocet}")
    CIEL.parent.mkdir(parents=True, exist_ok=True)
    CIEL.write_text("\n".join(vystup) + "\n", encoding="utf-8")
    print(f"{pocet} autorít, SHA-256 zdroja {odtlacok} → {CIEL}")


if __name__ == "__main__":
    main()
