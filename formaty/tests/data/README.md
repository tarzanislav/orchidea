# Skúšobné súbory formátov

Malé súbory vytvorené **cudzími kodérmi** – len ako dáta pre skúšky a semienka fuzzera, nie kód.
Zapísané v `zdroje.json`. Naše dekodéry ich musia prečítať rovnako ako referenčná knižnica
(odtlačky výsledkov sú v `formaty/tests/subory.rs`).

| Súbory | Vytvoril | Licencia nástroja |
|---|---|---|
| `bezstratovy_*.jxl` | libjxl 0.13 (cez imagecodecs), bezstratový režim, úsilie 1, 2, 7, 9 | BSD-3-Clause |
