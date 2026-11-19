#!/bin/sh
# Zbalí skúšky a merania ORCHIDEE pre skutočné Raspberry Pi.
# Použitie (v CI): sh nastroje/rpi_balik.sh <cieľový priečinok> <priečinok s príkladmi> < binarky.txt
set -eu
ciel="$1"; priklady="$2"
mkdir -p "$ciel/skusky"
while read -r b; do cp "$b" "$ciel/skusky/"; done
for e in meranie vektor zariadenia; do cp "$priklady/$e" "$ciel/"; done
cp nastroje/rpi_spusti.sh "$ciel/spusti.sh"
chmod +x "$ciel"/* "$ciel"/skusky/*
