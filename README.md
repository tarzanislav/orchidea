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
- **Dve základné zásady na každom systéme** (Windows, Linux, Android, macOS, iOS, web, ORCHIDEA OS): **chladný systém** –
  každá práca má známu cenu v energii a teple a robí sa tou najchladnejšou cestou bez straty kvality; **pôvodnosť** –
  vlastný, silný a rýchly kód a vlastné riešenia, žiadne kopírovanie cudzích.
- **Pojmy:** najprv tradičný pojem z IT a vedy, tvorivý názov v zátvorke – napr. správca systému (KLÍČ),
  profil hardvéru (PÔDA), tepelný regulátor (strážca); mená produktov ORCHIDEA, ORCHIDEA OS, ORCHIDEA Zapisovač, LÝRA.

## Stav

**Etapa E0 je hotová** – jadro, motor Neurón a GPU vrstva, všetko vlastným kódom bez cudzích knižníc:

| Balík | Vrstva | Obsah |
|---|---|---|
| `jadro` | 1 – gramatika | Signál, Štruktúra, Vlastnosť, Operácia, Parameter, Graf, Odkaz, Výber, Zmena, Výstup, Pohľad, Nástroj; denník; deterministická matematika; BLAKE3; vyhodnotenie len potrebnej oblasti |
| `gpu` | 2 – schopnosť | jazyk kernelov, prekladače do SPIR-V, HLSL, MSL a WGSL; zariadenia Vulkan, Direct3D 12, Metal; vektorový vykonávač na procesore (SIMD + všetky jadrá) a referenčný interpret |
| `neuron` | 2 – schopnosť | výpočtový motor MYKORÍZY: tenzor ako signál, presné a rýchle jadrá, 8-bitové a trojhodnotové váhy, učenie |
| `zariadenie` | 2 – schopnosť | profil hardvéru (PÔDA): čip, jadrá, inštrukcie, pamäť, napájanie – spoločný pre plánovač ORCHIDEE aj správcu systému ORCHIDEA OS |
| `teplo` | 2 – schopnosť | tepelný model: odhad teploty okolia a vlastného ohrevu, prostredie, tepelný rozpočet – základ zákona Z14 (cena práce v teple) |
| `dlazdice` | 2 – schopnosť | **E1:** obraz na zdieľaných dlaždiciach s kopírovaním pri zápise a odtlačkami (R70), pyramída náhľadu s obnovou len zmenenej časti (R71); bitovo rovnaké pri každej veľkosti dlaždice |
| `hevc` | 2 – schopnosť | rozbor syntaxe HEVC (H.265): jednotky NAL, VPS, SPS, PPS, matice kvantovania, sady referencií, VUI, rozšírenia rozsahu, odtlačky SEI – základ prehrávania HEVC priamo na Raspberry Pi (R214); overené skúšobnými tokmi normy a tokmi x265 |
| `media` | 2 – schopnosť | kodeky a kontajnery (Médiá) – základ prehrávača médií LÝRA: presná časová základňa, stopy a pakety, rozpoznanie Matroska/MP4/MPEG-TS, rozhranie R5 pre ORCHIDEA OS (pakety videa pre hardvérový dekodér, zvuk pre HDMI); ďalej čítačky kontajnerov a dekodéry zvuku PCM, FLAC, Opus, MP3, AC-3 |

Skúšky prechádzajú na Linuxe (x86-64, ARM64), Windows, macOS, Androide a vo WebAssembly s bitovo rovnakými
výsledkami; násobenie matíc na GPU (Vulkan, Direct3D 12, Metal) je bitovo rovnaké ako na procesore.
ORCHIDEA je pre **všetky zariadenia od Raspberry Pi 1** – CI overuje aj profil slabého zariadenia (4 jadrá, 1 GB,
bez GPU), 32-bitový ARMv7 a ARMv6 (Raspberry Pi 1 a Zero). Model efektívnosti **FOTOSYNTÉZA**: rýchlosť bez straty kvality odstránením práce,
ktorá výsledok nemení. **Prebieha E1 – prehliadač obrázkov (Pohľad):** dlaždice a pyramída náhľadu hotové,
ďalej obrazové formáty s fuzzerom.

```
cargo test --workspace                                   # všetky skúšky
cargo run --release -p orchidea-neuron --example meranie  # meranie výkonu
cargo run -p orchidea-gpu --example zariadenia            # dostupné výpočtové zariadenia
cargo run --release -p orchidea-gpu --example vektor      # interpret vs. vektorový vykonávač
cargo run --release -p orchidea-dlazdice --example dlazdice # dlaždice, história a pyramída náhľadu (4K)
```

## ORCHIDEA OS – vlastný systém pre Raspberry Pi, Odroid C4 a TV krabičky (SKLENÍK)

Heslo: **Chladný systém.** Oficiálne jadro Raspberry Pi s vlastným nastavením a nad ním všetko vlastné.
**Jedna univerzálna karta** štartuje na RPi 1, 2, 3, 4, 5, Zero, Zero W a Zero 2 W – firmvér každého modelu si
nájde svoje jadro a svoj systém; klávesnica, myš, USB a Ethernet fungujú hneď po štarte.

| Balík | Obsah |
|---|---|
| `sklenik/klic` | správca systému (KLÍČ) – prvý proces systému; grafická stavová obrazovka (sk/cs/en), meno zariadenia zo Zapisovača, inventár periférií (SÚPIS) a zapnutie siete; tepelný regulátor (strážca) drží Raspberry Pi chladné aj bez chladiča (cieľ 60 °C, strop 70 °C na RPi 3), ventilátor zapne až vtedy, keď treba |
| `sklenik/sadic` | **ORCHIDEA Zapisovač** (SADIČ) pre Windows 11: **dvojklik otvorí okno** (obraz → karta → nastavenie → Zapísať), obraz vezme aj priamo zo stiahnutého ZIP; zapíše a overí kartu, pripraví meno, Wi-Fi a jazyk, zálohuje a obnovuje; ponúkne len SD karty a USB disky, nikdy disk so systémom |
| `sklenik/karta` | spoločný kód karty: oddiel FAT32 a nastavenie `SKLENIK.TXT` (meno, Wi-Fi, jazyk) – Zapisovač ho zapíše, správca systému prečíta |
| `sklenik/korene` | zabezpečené spojenie zariadení (KORENE) – sieť ORCHIDEE; zatiaľ vlastná kryptografia X25519 a ChaCha20-Poly1305 overená testovacími vektormi RFC 7748 a RFC 8439 |
| `pismo` | vlastné ťahové písmo (STONKA) s diakritikou (slovenčina, čeština), ľubovoľná veľkosť, vyhladené kreslenie |
| `sklenik/jadro.config`, `sklenik/boot` | nastavenie jadra a zavádzania podľa modelu (systém beží z pamäte, karta sa takmer nepoužíva) |

Obraz `sklenik-rpi.img` a `sadic.exe` zostavuje CI. Pri každej zmene sa overí výber jadra a systému za každý
model a karta sa naštartuje v emulovanom RPi Zero, RPi 2 a RPi 3 (jadro pre RPi 5 na procesore Cortex-A76).
ORCHIDEA OS a ORCHIDEA zdieľajú balíky `zariadenie` a `teplo` (R204) – jeden profil hardvéru a jeden tepelný model.

Najjednoduchšie: stiahnuť `sklenik-rpi` a `sadic-windows-x64`, spustiť `sadic.exe` dvojklikom a stlačiť Zapísať.
V termináli:

```
sadic zoznam                                        # disky, na ktoré sa smie zapisovať
sadic zapis sklenik-rpi.zip 3 meno=pi1 wifi=Domov wifi_heslo=… jazyk=sk
```

## Kópie repozitára

- hlavné miesto: GitHub `tarzanislav/orchidea`
- zrkadlo: Codeberg `tarzanislav/orchidea` (automatická kópia pri každej zmene)

## English

ORCHIDEA is a free and peaceful integrated suite for professional work with images, photos, painting, vector,
layout, video, audio, music, motion and discs, built on its own original code. Licensed under Apache-2.0 and
guided by the [Peace Charter](PEACE-CHARTER.md).
