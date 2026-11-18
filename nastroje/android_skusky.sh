#!/bin/sh
# Spustí zostavené skúšky v emulátore Androidu. Zoznam spustiteľných súborov je v binarky.txt.
set -e
adb wait-for-device
chyba=0
while read -r subor; do
  [ -n "$subor" ] || continue
  meno=$(basename "$subor")
  adb push "$subor" "/data/local/tmp/$meno" >/dev/null
  adb shell chmod 755 "/data/local/tmp/$meno"
  echo "== $meno"
  if ! adb shell "cd /data/local/tmp && ./$meno; echo KONIEC=\$?" | tee vystup.txt | tail -n 3; then chyba=1; fi
  grep -q "KONIEC=0" vystup.txt || chyba=1
done < binarky.txt
exit $chyba
