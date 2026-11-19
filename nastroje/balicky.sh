#!/bin/sh
# Inštalácia balíkov nástrojov v CI s opakovaním (zrkadlá Ubuntu občas vypadnú).
# Použitie: sh nastroje/balicky.sh balík1 balík2 ...
set -u
[ $# -eq 0 ] && exit 0
s=""; [ "$(id -u)" -ne 0 ] && s="sudo"
for pokus in 1 2 3 4 5; do
  if $s apt-get -o Acquire::Retries=5 update -qq && $s apt-get -o Acquire::Retries=5 install -y -qq --fix-missing "$@"; then
    exit 0
  fi
  echo "balíky: pokus $pokus zlyhal, čakám…"
  sleep $((pokus * 20))
done
exit 1
