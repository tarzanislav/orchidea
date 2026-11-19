#!/bin/sh
# Spustí všetky zostavené skúšky v emulátore Androidu (zoznam je v binarky.txt).
# Zlyhá, ak niektorá skúška zlyhá alebo ak sa nespustili všetky – nič neprejde potichu.
set -e
adb wait-for-device </dev/null
ocakavane=0
uspesne=0
for subor in $(cat binarky.txt); do
  ocakavane=$((ocakavane + 1))
  meno=$(basename "$subor")
  adb push "$subor" "/data/local/tmp/$meno" </dev/null >/dev/null
  adb shell chmod 755 "/data/local/tmp/$meno" </dev/null
  echo "== $meno"
  adb shell "cd /data/local/tmp && ORCHIDEA_GPU_STOPA=1 ./$meno --test-threads=1 --nocapture; echo KONIEC=\$?" </dev/null > vystup.txt 2>&1 || true
  grep -E "test result|panicked|FAILED" vystup.txt || true
  grep -B2 -A30 "Segmentation\|Aborted" vystup.txt | grep -v "^INFO" | head -60 || true
  if grep -q "KONIEC=0" vystup.txt; then
    uspesne=$((uspesne + 1))
  else
    cat vystup.txt
  fi
done
echo "Spustených a úspešných: $uspesne z $ocakavane"
[ "$ocakavane" -gt 0 ] && [ "$uspesne" -eq "$ocakavane" ]
