# Skúšobné súbory formátov

Malé súbory vytvorené **cudzími kodérmi** – len ako dáta pre skúšky a semienka fuzzera, nie kód.
Zapísané v `zdroje.json`. Naše dekodéry ich musia prečítať rovnako ako referenčná knižnica
(odtlačky výsledkov sú v `formaty/tests/subory.rs`).

| Súbory | Vytvoril | Licencia nástroja |
|---|---|---|
| `bezstratovy_*.jxl` | libjxl 0.13 (cez imagecodecs), bezstratový režim, úsilie 1, 2, 7, 9; `bezstratovy_zaplaty` – „snímka obrazovky“ so záplatami | BSD-3-Clause |
| `stratovy_*.jxl` | libjxl 0.13 (cez imagecodecs), stratový režim VarDCT, vzdialenosť 1, 1,5, 2, 5, 12; `stratovy_zaplaty` – záplaty, úsilie 9; `stratovy_jpeg420` – bezstratovo prekódovaný JPEG 4:2:0 (JPEG z Pillow) | BSD-3-Clause |
| `stratovy_*.png` | výstup dekodéra libjxl pre `stratovy_*.jxl` (referencia, odchýlka najviac 1) | BSD-3-Clause |

## Regresné vstupy (`regresia/`)

| Súbor | Pôvod | Skúška |
|---|---|---|
| `jxl_65tb_semienko2.jxl` | pozmenený súbor z dlhého fuzzera JPEG XL (semienko 2, beh CI 45482947475, commit d1b0a5b) – čítačka chcela vyhradiť 65 TB | `fuzzer.rs` → `regresne_vstupy` |
