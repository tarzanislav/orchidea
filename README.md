# ORCHIDEA – symbol krásy

> **Slobodný a mierový projekt.** Licencia [Apache-2.0](LICENSE) · [Mierová charta](MIEROVA-CHARTA.md) ·
> [Peace Charter](PEACE-CHARTER.md)

ORCHIDEA je integrovaná suita na profesionálnu prácu s obrazom, fotografiou, kresbou, vektorom, sadzbou, videom,
zvukom, hudbou, pohybom a diskami. Spája ich jedna gramatika a jedno jadro, nie lepidlo medzi programami.

- **Potenciál, nie predpis.** Program nikomu nepredpisuje cieľ ani postup. Profily (vzdelávací, vedecký, dizajnový,
  reklamný a ďalšie) sú len voliteľné dáta.
- **Profesionálna presnosť.** Platí zmluva kvality: žiadne tiché zníženie kvality.
- **Vlastný pôvodný kód.** Cudzí kód len výnimočne, malý, so slobodnou permisívnou licenciou, oddelený a
  nahraditeľný. Pravidlá sú v [CONTRIBUTING.md](CONTRIBUTING.md) a v registri [zdroje.json](zdroje.json).
- **Na každom zariadení.** Windows, Linux, Android, macOS, iOS a web. Prácu možno dokončiť na inom zariadení.
- **Bez kont, bez telemetrie, bez kreditov.** Umelá inteligencia MYKORÍZA beží lokálne a zadarmo.
- **LÝRA** je samostatný prehrávač hudby, videa a animácií vrátane interaktívnych.

## Stav

**E0, časť 1 je hotová:** jadro (gramatika 12 prvotných pojmov a 7 pravidiel skladania) a motor Neurón
(tenzor ako signál, presné a rýchle jadrá, 8-bitové a trojhodnotové váhy, učenie). Všetko vlastným kódom bez
cudzích knižníc. Skúšky prechádzajú na Linuxe (x86-64, ARM64), Windows, macOS, Androide a vo WebAssembly
s bitovo rovnakými výsledkami. Ďalej: GPU vrstva a zhoda CPU/GPU.

| Balík | Vrstva | Obsah |
|---|---|---|
| `jadro` | 1 – gramatika | Signál, Štruktúra, Vlastnosť, Operácia, Parameter, Graf, Odkaz, Výber, Zmena, Výstup, Pohľad, Nástroj; denník; deterministická matematika; BLAKE3 |
| `neuron` | 2 – schopnosť | výpočtový motor MYKORÍZY |

```
cargo test --workspace                                   # všetky skúšky
cargo run --release -p orchidea-neuron --example meranie  # meranie výkonu
```

## Kópie repozitára

- hlavné miesto: GitHub `tarzanislav/orchidea`
- zrkadlo: Codeberg `tarzanislav/orchidea` (automatická kópia pri každej zmene)

## English

ORCHIDEA is a free and peaceful integrated suite for professional work with images, photos, painting, vector,
layout, video, audio, music, motion and discs, built on its own original code. Licensed under Apache-2.0 and
guided by the [Peace Charter](PEACE-CHARTER.md).
