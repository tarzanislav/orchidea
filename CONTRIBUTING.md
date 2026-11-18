# Ako prispievať do ORCHIDEE

Ďakujeme, že chceš pomôcť. Každý príspevok musí spĺňať tri veci.

## 1. Mierová charta

Prispievaním sa hlásiš k [Mierovej charte](MIEROVA-CHARTA.md). Pri každej novej funkcii sa pýtame:

> **Mohla by byť táto funkcia hlavne nástrojom vojny, sledovania ľudí alebo klamania?**

Ak áno, funkciu nevyvíjame, alebo len s ochranami: beží lokálne, iba so súhlasom človeka a s pôvodom (C2PA).

## 2. Kódex pôvodného kódu

| Trieda | Čo | Pravidlo |
|---|---|---|
| A | vlastný pôvodný kód | základ všetkého |
| B | rozhrania operačného systému a ovládačov | dovolené |
| C | malý cudzí kód s licenciou MIT, Apache-2.0, BSD, zlib alebo ISC | len so súhlasom vlastníka, oddelený, nahraditeľný, zapísaný v `zdroje.json` |
| D | copyleft (GPL, LGPL, AGPL…) | nikdy v programe |
| E | proprietárny kód | nikdy |

Nekopírujeme cudzí kód. Cudzie projekty študujeme len ako inšpiráciu a riešenie píšeme nanovo.

## 3. Register zdrojov

Každá cudzia závislosť, model, dátová sada či nástroj zostavenia musí byť v súbore [zdroje.json](zdroje.json):
meno, trieda, licencia, účel a posúdenie podľa charty. **CI zastaví zmenu, ktorá pridá nezapísaný zdroj.**

## Podpis príspevku (DCO)

Každý commit podpíš riadkom, ktorým potvrdzuješ
[Developer Certificate of Origin 1.1](https://developercertificate.org/), teda že príspevok smieš odovzdať pod
licenciou projektu:

```
Signed-off-by: Meno Priezvisko <email>
```

Stačí `git commit -s`.

## Zmeny charty

Chartu možno meniť len tak, aby sa jej záväzky **posilnili alebo spresnili, nikdy oslabili**.
