#!/bin/sh
# Meranie vlastného dekodéra HEVC na skutočnom Raspberry Pi (R214, krok H5):
# bitová zhoda (MD5 každého obrazu), obrazy za sekundu pri 1, 2 a 4 vláknach a teplota dosky.
#
# Použitie: rozbaliť balík z CI („orchidea-hevc-rpi-64bit“ pre 64-bitový systém, „orchidea-hevc-rpi-32bit-armv7“
# pre 32-bitový) na dosku a spustiť  sh meraj_hevc.sh
# Výsledok: hevc_vysledky.txt (poslať späť). Trvá niekoľko minút.
#
# Chladný systém: pred každým meraním doska vychladne pod 60 °C (najviac 5 minút čakania) a pri 80 °C sa
# meranie preruší – doska sa nemá prehrievať kvôli číslu.
set -u
cd "$(dirname "$0")"
vysledok=hevc_vysledky.txt
chmod +x ./hevc_dekoduj ./hevc_meranie 2>/dev/null

teplota() {
  t=$(cat /sys/class/thermal/thermal_zone0/temp 2>/dev/null || echo 0)
  echo $((t / 1000))
}
takt() {
  f=$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq 2>/dev/null || echo 0)
  echo $((f / 1000))
}
obmedzenie() { vcgencmd get_throttled 2>/dev/null || echo "nezistené"; }
ochlad() {
  i=0
  while [ "$(teplota)" -gt 60 ] && [ $i -lt 30 ]; do sleep 10; i=$((i + 1)); done
}
# najvyššia teplota počas behu príkazu; pri 80 °C sa beh ukončí
s_teplotou() {
  rm -f .max
  echo "$(teplota)" > .max
  "$@" &
  p=$!
  while kill -0 "$p" 2>/dev/null; do
    t=$(teplota)
    [ "$t" -gt "$(cat .max)" ] && echo "$t" > .max
    if [ "$t" -ge 80 ]; then
      kill "$p" 2>/dev/null
      echo "PRERUŠENÉ pri $t °C (chladný systém)"
    fi
    sleep 2
  done
  wait "$p" 2>/dev/null
  echo "najvyššia teplota: $(cat .max) °C, takt na konci: $(takt) MHz, obmedzenie: $(obmedzenie)"
  rm -f .max
}

{
  echo "# HEVC – meranie vlastného dekodéra na doske ($(date '+%Y-%m-%d %H:%M'))"
  echo "doska: $(tr -d '\0' < /proc/device-tree/model 2>/dev/null || uname -m), jadier: $(nproc)"
  echo "na začiatku: $(teplota) °C, takt $(takt) MHz, obmedzenie: $(obmedzenie)"
  echo
  echo "## Bitová zhoda (MD5 každého obrazu, všetky jadrá)"
  ochlad
  s_teplotou env ORCHIDEA_HEVC_PRISNE=1 ./hevc_dekoduj toky
  echo
  for n in 1 2 4; do
    ochlad
    echo "## Rýchlosť – vlákien: $n (štart pri $(teplota) °C)"
    s_teplotou env ORCHIDEA_HEVC_RYCHLOST=1 ORCHIDEA_HEVC_VLAKNA="$n" ./hevc_dekoduj toky
    echo
  done
  # rozpis času po etapách (riadky CAS, rozhranie R4); 6 a 8 vlákien na 4 jadrách – vyplní čakanie na referencie?
  # (aj film1080_tazky: zrno a 8 Mb/s ako skutočný film)
  for f in film1080 film1080_tazky; do
    [ -f "toky/$f.hevc" ] || continue
    for n in 1 4 6 8; do
      ochlad
      echo "## Etapy – $f, vlákien: $n (štart pri $(teplota) °C)"
      s_teplotou ./hevc_meranie --rychlost --etapy --vlakna "$n" "toky/$f.hevc" | grep -v '^OBRAZ'
      echo
    done
  done
} 2>&1 | tee "$vysledok"
echo "Hotovo – výsledok je v súbore $vysledok"
