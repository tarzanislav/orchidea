#!/bin/sh
# Skúšky a merania ORCHIDEE na skutočnom zariadení (Raspberry Pi, kancelárske PC s Linuxom).
# Spustenie: sh spusti.sh   – výsledok sa zapíše do súboru vysledky.txt vedľa tohto skriptu.
cd "$(dirname "$0")"
# balík z GitHubu (zip) nezachová príznak spustiteľnosti
chmod +x zariadenia meranie vektor dlazdice skusky/* 2>/dev/null
vystup=vysledky.txt
teplota() {
  t=$(cat /sys/class/thermal/thermal_zone0/temp 2>/dev/null) && echo "teplota: $((t / 1000)).$(((t % 1000) / 100)) °C"
  command -v vcgencmd >/dev/null 2>&1 && vcgencmd get_throttled
}
{
  echo "=== ORCHIDEA – výsledky na zariadení ($(date '+%Y-%m-%d %H:%M'))"
  uname -m
  grep -m1 -E "^(Model|model name)" /proc/cpuinfo || true
  grep -c ^processor /proc/cpuinfo | sed 's/^/jadier: /'
  grep MemTotal /proc/meminfo || true
  grep -h PRETTY_NAME /etc/os-release || true
  echo
  echo "=== Teplota v nečinnosti (60 s, každých 10 s)"
  for i in 1 2 3 4 5 6; do teplota; sleep 10; done
  echo
  echo "=== Zariadenia"
  ./zariadenia 2>&1
  zlyhali=0
  for s in skusky/*; do
    echo
    echo "=== Skúška $(basename "$s")"
    ORCHIDEA_GPU_STOPA=1 "$s" --test-threads=1 2>&1 || zlyhali=$((zlyhali + 1))
    teplota
  done
  echo
  echo "=== Meranie – Neurón"
  ./meranie 2>&1
  teplota
  echo
  echo "=== Meranie – vektorový vykonávač"
  ./vektor 2>&1
  teplota
  echo
  echo "=== Meranie – dlaždice a pyramída náhľadu (E1)"
  ./dlazdice 2>&1
  teplota
  echo
  echo "=== Chladnutie po záťaži (60 s, každých 10 s)"
  for i in 1 2 3 4 5 6; do sleep 10; teplota; done
  echo
  echo "=== Zlyhaných skúšok: $zlyhali"
} > "$vystup" 2>&1
tail -n 1 "$vystup"
echo "Hotovo – výsledky sú v súbore $(pwd)/$vystup"
