#!/usr/bin/env bash
# Skúška rozšíreného SADIČA na Linuxe nezávislými nástrojmi (fsck.vfat, fsck.exfat, e2fsck, ntfs-3g,
# fsck.hfsplus, fsck.f2fs, 7-Zip a udfinfo pre UDF, sgdisk, mtools, xorriso, genisoimage): formátovanie, rozbor ISO, napálenie bit po bite
# aj rozbalením súborov, veľký súbor nad 4 GB na druhý oddiel NTFS či exFAT a skutočné ISO distribúcie
# s kontrolným súčtom.
#
#   bash nastroje/skuska_sadica.sh target/release/sadic /mnt/sadic
set -euo pipefail
export LC_ALL=C.UTF-8  # mtools prevádza dlhé mená podľa jazyka prostredia
S=$(realpath "$1")
T=$2
NASTROJE=$(dirname "$(realpath "$0")")
mkdir -p "$T"
cd "$T"
chyba() { echo "❌ $*"; exit 1; }

# vyreže oddiel N (1, 2 …) z obrazu disku do súboru oddiel.img
oddiel() {
  local obraz=$1 n=$2
  python3 - "$obraz" "$n" <<'EOF'
import re, subprocess, sys
d = subprocess.run(["sfdisk", "-d", sys.argv[1]], capture_output=True, text=True).stdout
m = list(re.finditer(r"start=\s*(\d+), size=\s*(\d+)", d))[int(sys.argv[2]) - 1]
st, sz = int(m.group(1)), int(m.group(2))
nuly = bytes(1 << 22)
with open(sys.argv[1], "rb") as f, open("oddiel.img", "wb") as o:
    f.seek(st * 512)
    zostava = sz * 512
    while zostava:
        k = f.read(min(zostava, 1 << 22))
        if k == nuly[: len(k)]:
            o.seek(len(k), 1)  # riedky súbor – nuly nezaberajú miesto
        else:
            o.write(k)
        zostava -= len(k)
    o.truncate(sz * 512)
EOF
}

echo "== 1. formátovanie (NTFS, FAT32, exFAT, ext4/3/2, HFS+, F2FS, UDF; MBR, GPT; malé aj veľké disky)"
for v in "300 ntfs mbr menovka=Škola_NTFS" "2000 ntfs gpt menovka=Data" \
         "300 ext4 mbr menovka=Linux_ext4" "40000 ext4 gpt" "3000 ext3 mbr" "1000 ext2 gpt menovka=stary" \
         "300 hfs+ mbr menovka=Škola_Mac" "40000 hfs+ gpt" "3000 hfs+ gpt menovka=Fotky" \
         "300 udf menovka=Škola_UDF" "40000 udf" "2000 udf gpt menovka=Spolocny" \
         "300 f2fs mbr menovka=Linux_F2FS" "40000 f2fs gpt" "120 f2fs gpt" \
         "300 fat32 mbr menovka=Škola" "300 exfat gpt menovka=Fotky_2026" "300 exfat mbr klaster=4K" \
         "8000 auto mbr menovka=OSEM" "33000 auto gpt menovka=VELKA" "64000 fat32 mbr"; do
  set -- $v; mb=$1; shift
  "$S" formatuj-obraz f.img "$mb" "$@"
  sfdisk -d f.img | grep -q "start=" || chyba "chýba tabuľka oddielov ($v)"
  if [[ " $* " == *" gpt "* && " $* " != *" udf "* ]]; then sgdisk -v f.img | grep -q "No problems found" || chyba "GPT ($v)"; fi
  oddiel f.img 1
  podpis=$(dd if=oddiel.img bs=1 skip=3 count=5 status=none)
  if [[ "$(dd if=oddiel.img bs=1 skip=32769 count=5 status=none)" == "BEA01" ]]; then
    sfdisk -d f.img | grep -qE "start= +0," || chyba "UDF: pomocný oddiel MBR nezačína na sektore 0"
    python3 -I "$NASTROJE/over_udf.py" oddiel.img 512 || chyba "UDF ($v)"
    7z l -tudf oddiel.img | grep -q "^Type = Udf" || chyba "7-Zip neotvoril UDF ($v)"
    udfinfo oddiel.img | grep -E "^(label|blocksize|udfrev|integrity)=" | tr '\n' ' '; echo
  elif [[ "$(dd if=oddiel.img bs=1 skip=1024 count=4 status=none | od -An -tx1 | tr -d ' ')" == "1020f5f2" ]]; then
    fsck.f2fs --dry-run oddiel.img > /tmp/f2fs.log 2>&1 || { cat /tmp/f2fs.log; chyba "fsck.f2fs ($v)"; }
    if grep "^\[FSCK\]" /tmp/f2fs.log | grep -v "Ok\.\.\|fixing SIT types\|Max image"; then cat /tmp/f2fs.log; chyba "fsck.f2fs ($v)"; fi
    grep -q "Checked valid nat_bits" /tmp/f2fs.log || chyba "F2FS: bity NAT ($v)"
    grep -E "^\[FSCK\] Max image" /tmp/f2fs.log
  elif [[ "$(dd if=oddiel.img bs=1 skip=1024 count=2 status=none)" == "H+" ]]; then
    fsck.hfsplus -fn oddiel.img | tee /tmp/hfs.log | grep "volume name"
    grep -q "appears to be OK" /tmp/hfs.log || { cat /tmp/hfs.log; chyba "fsck.hfsplus ($v)"; }
  elif [[ "$(dd if=oddiel.img bs=1 skip=1080 count=2 status=none | od -An -tx1 | tr -d ' ')" == "53ef" ]]; then
    e2fsck -fn oddiel.img || chyba "e2fsck ($v)"
    dumpe2fs -h oddiel.img 2>/dev/null | grep -E "volume name|features"
  elif [[ "$podpis" == "EXFAT" ]]; then
    fsck.exfat -n oddiel.img | tail -1
  elif [[ "$podpis" == "NTFS " ]]; then
    ntfsfix -n oddiel.img | grep -q "processed successfully" || chyba "NTFS ($v)"
    ntfsls -a -s oddiel.img | grep -q '\$UpCase' || chyba "NTFS metasúbory ($v)"
    ntfsinfo -m oddiel.img | grep -E "Volume Name|Volume Version"
  else
    fsck.vfat -n oddiel.img | tail -1
  fi
  blkid oddiel.img
  rm -f f.img oddiel.img
done
for sys4k in fat32 hfs+ udf; do
"$S" formatuj-obraz f4k.img 300 "$sys4k" gpt sektor=4096
if [[ "$sys4k" == "udf" ]]; then
  python3 -I "$NASTROJE/over_udf.py" f4k.img 4096 || chyba "UDF pri 4K sektoroch"
  rm -f f4k.img
  continue
fi
python3 - <<'EOF'
import struct, zlib
d = open("f4k.img", "rb").read()
h = d[4096:4096 + 92]
assert h[:8] == b"EFI PART", "GPT pri 4K sektoroch"
z = bytearray(h); z[16:20] = b"\0" * 4
assert zlib.crc32(bytes(z)) == struct.unpack("<I", h[16:20])[0], "CRC hlavičky"
e = d[2 * 4096:2 * 4096 + 128 * 128]
f, l = struct.unpack("<QQ", e[32:48])
open("oddiel.img", "wb").write(d[f * 4096:(l + 1) * 4096])
assert d[-4096:-4088] == b"EFI PART", "záloha GPT"
EOF
if [[ "$sys4k" == "hfs+" ]]; then
  fsck.hfsplus -fn oddiel.img | grep -q "appears to be OK" || chyba "HFS+ pri 4K sektoroch"
else
  fsck.vfat -n oddiel.img | tail -1
fi
rm -f f4k.img oddiel.img
done
echo "✅ formátovanie"

echo "== 2. hybridné ISO (BIOS + UEFI) – rozbor a napálenie bit po bite"
rm -rf strom && mkdir -p strom/EFI/BOOT strom/casper strom/.disk strom/isolinux
echo 'Ubuntu 24.04.1 LTS "Noble Numbat" - Release amd64 (20240827)' > strom/.disk/info
head -c 3000000 /dev/urandom > strom/casper/vmlinuz
echo efi > strom/EFI/BOOT/BOOTX64.EFI
cp /usr/lib/ISOLINUX/isolinux.bin strom/isolinux/
cp /usr/lib/syslinux/modules/bios/ldlinux.c32 strom/isolinux/
mkfs.vfat -C efi.img 1440 >/dev/null
mmd -i efi.img ::EFI ::EFI/BOOT && mcopy -i efi.img strom/EFI/BOOT/BOOTX64.EFI ::EFI/BOOT/
cp efi.img strom/efi.img
xorriso -as mkisofs -V "Ubuntu 24.04.1 LTS amd64" -J -R -isohybrid-mbr /usr/lib/ISOLINUX/isohdpfx.bin \
  -c isolinux/boot.cat -b isolinux/isolinux.bin -no-emul-boot -boot-load-size 4 -boot-info-table \
  -eltorito-alt-boot -e efi.img -no-emul-boot -isohybrid-gpt-basdat -o hybrid.iso strom 2>/dev/null
vystup=$("$S" skumaj hybrid.iso sucet="$(sha256sum hybrid.iso | cut -d' ' -f1)")
echo "$vystup"
grep -q "hybridné ISO" <<<"$vystup" || chyba "hybridné ISO nerozpoznané"
grep -q "štart: BIOS + UEFI" <<<"$vystup" || chyba "BIOS + UEFI"
grep -q "Noble Numbat" <<<"$vystup" || chyba ".disk/info"
grep -q "súčet SHA-256 sedí" <<<"$vystup" || chyba "SHA-256"
if "$S" skumaj hybrid.iso sucet=0000000000000000000000000000000000000000000000000000000000000000 >/dev/null 2>&1; then
  chyba "zlý súčet nebol odhalený"
fi
"$S" napal-obraz hybrid.iso h.img 64
cmp -n "$(stat -c %s hybrid.iso)" hybrid.iso h.img || chyba "napálené hybridné ISO nesedí"
rm -f h.img
echo "✅ hybridné ISO"

echo "== 3. ISO Windows (UDF) – rozbalenie súborov na FAT32"
rm -rf win && mkdir -p win/sources win/efi/boot win/boot/fonts win/support
head -c 300000 /dev/urandom > win/sources/install.wim
head -c 100000 /dev/urandom > win/sources/boot.wim
echo "[EditionID]" > win/sources/ei.cfg
# rôzne druhy mien na NTFS (8.3 malými aj veľkými, dlhé, s medzerou, s diakritikou, podpriečinok)
mkdir -p win/sources/sk-sk
echo a > win/sources/README.TXT
echo b > win/sources/Setup.Exe
echo c > "win/sources/Inštalácia – dlhý názov súboru.txt"
echo d > "win/sources/ab cd.txt"
echo e > win/sources/sk-sk/setup.exe.mui
echo f > win/sources/sk-sk/LICENSE.RTF
echo bm > win/bootmgr
head -c 5000 /dev/urandom > win/bootmgr.efi
head -c 2048 /dev/urandom > win/efi/boot/bootx64.efi
echo "Pozdrav z SADIČA" > "win/boot/čítaj ma prosím.txt"
for i in $(seq 1 60); do echo "$i" > "win/support/súbor s dlhým menom $i.txt"; done
echo x > win/setup.exe
genisoimage -quiet -input-charset utf-8 -udf -J -joliet-long -V CCCOMA_X64FRE_SK-SK_DV9 \
  -e efi/boot/bootx64.efi -no-emul-boot -o win.iso win
vystup=$("$S" skumaj win.iso)
echo "$vystup"
grep -q "inštalačné ISO Windows" <<<"$vystup" || chyba "ISO Windows nerozpoznané"
grep -q "UDF" <<<"$vystup" || chyba "UDF"
"$S" napal-obraz win.iso w.img 200
oddiel w.img 1
fsck.vfat -n oddiel.img | tail -1
(cd win && find . -type f) | while read -r f; do
  mcopy -n -i oddiel.img "::${f#./}" kopia.bin || chyba "chýba ${f#./}"
  cmp -s "win/$f" kopia.bin || chyba "obsah ${f#./} nesedí"
done
rm -f w.img oddiel.img kopia.bin
echo "✅ ISO Windows rozbalené na FAT32 a každý súbor sedí"
# to isté ISO s nízkou hranicou: sources (okrem boot.wim) idú na druhý oddiel NTFS
SADIC_SKUSKA=1 SADIC_SKUSKA_HRANICA=50000 "$S" napal-obraz win.iso w2.img 96
oddiel w2.img 2
ntfsfix -n oddiel.img | grep -q "processed successfully" || chyba "NTFS druhého oddielu"
(cd win && find sources -type f ! -name boot.wim) | while read -r f; do
  ntfscat oddiel.img "/$f" > kopia.bin || chyba "na NTFS chýba $f"
  cmp -s "win/$f" kopia.bin || chyba "obsah $f na NTFS nesedí"
done
oddiel w2.img 1
mcopy -n -i oddiel.img ::sources/boot.wim kopia.bin && cmp -s win/sources/boot.wim kopia.bin || chyba "boot.wim na FAT32"
rm -f w2.img oddiel.img kopia.bin
echo "✅ FAT32 + NTFS: každý súbor sedí"

echo "== 3b. NTFS s veľkým priečinkom (B-strom indexu, 3 050 súborov)"
rm -rf mnoho && mkdir -p mnoho/sources/sk-sk mnoho/efi/boot
echo e > mnoho/efi/boot/bootx64.efi
echo bm > mnoho/bootmgr
head -c 100000 /dev/urandom > mnoho/sources/boot.wim
python3 -c '
import os, random
random.seed(7)
for i in range(3000):
    n = "mnoho/sources/Knižnica_%05d_%s.dll" % (i, "x" * random.randint(0, 60))
    open(n, "wb").write(os.urandom(random.randint(0, 20000)))
for i in range(50):
    open("mnoho/sources/sk-sk/súbor%d.mui" % i, "wb").write(os.urandom(random.randint(1, 3000)))
'
genisoimage -quiet -input-charset utf-8 -udf -J -joliet-long -V CCCOMA_X64FRE -o mnoho.iso mnoho
SADIC_SKUSKA=1 SADIC_SKUSKA_HRANICA=50000 "$S" napal-obraz mnoho.iso m.img 400
oddiel m.img 2
ntfsfix -n oddiel.img | grep -q "processed successfully" || chyba "NTFS s veľkým priečinkom"
pocet=$(ntfsls oddiel.img -p /sources | wc -l)
[[ "$pocet" -ge 3001 ]] || chyba "v indexe chýbajú súbory ($pocet)"
(cd mnoho && find sources -type f ! -name boot.wim) | while read -r f; do
  ntfscat oddiel.img "/$f" | cmp -s - "mnoho/$f" || chyba "obsah $f nesedí"
done
rm -rf m.img oddiel.img mnoho mnoho.iso
echo "✅ NTFS: 3 050 súborov nájdených cez index a každý sedí"

echo "== 4. súbor nad 4 GB – FAT32 (štart) + NTFS alebo exFAT (sources)"
rm -rf velke && mkdir -p velke/sources velke/efi/boot
head -c 4404019200 /dev/urandom > velke/sources/install.wim
head -c 100000 /dev/urandom > velke/sources/boot.wim
echo bm > velke/bootmgr
echo e > velke/efi/boot/bootx64.efi
genisoimage -quiet -input-charset utf-8 -udf -allow-limited-size -J -joliet-long -V CCCOMA_X64FRE \
  -o velke.iso velke
sucet_wim=$(sha256sum velke/sources/install.wim | cut -d' ' -f1)
rm -f velke/sources/install.wim
"$S" skumaj velke.iso | tee vystup.txt
grep -q "súbor nad 4 GB: sources/install.wim" vystup.txt || chyba "súbor nad 4 GB"
"$S" napal-obraz velke.iso v.img 6000 gpt
sgdisk -v v.img | grep -q "No problems found" || chyba "GPT dvoch oddielov"
oddiel v.img 1
fsck.vfat -n oddiel.img | tail -1
mcopy -n -i oddiel.img ::sources/boot.wim kopia.bin && cmp -s velke/sources/boot.wim kopia.bin || chyba "boot.wim na FAT32"
oddiel v.img 2
ntfsfix -n oddiel.img | grep -q "processed successfully" || chyba "NTFS s install.wim"
[[ "$(ntfscat oddiel.img /sources/install.wim | sha256sum | cut -d' ' -f1)" == "$sucet_wim" ]] || chyba "install.wim na NTFS nesedí"
rm -f v.img oddiel.img
echo "✅ veľký install.wim na NTFS sedí bajt po bajte"
"$S" napal-obraz velke.iso v.img 6000 gpt exfat
rm -f velke.iso
oddiel v.img 2
fsck.exfat -n oddiel.img | tail -1
python3 "$NASTROJE/citaj_exfat.py" oddiel.img sources/install.wim | tee vystup.txt
grep -q "^$sucet_wim " vystup.txt || chyba "install.wim na exFAT nesedí"
rm -f v.img oddiel.img kopia.bin vystup.txt
echo "✅ veľký install.wim aj na exFAT sedí bajt po bajte"

echo "== 5. skutočné ISO distribúcie (Alpine Linux) s kontrolným súčtom"
URL=https://dl-cdn.alpinelinux.org/alpine/latest-stable/releases/x86_64
MENO=$(curl -fsSL "$URL/latest-releases.yaml" | grep -o 'alpine-standard-[0-9.]*-x86_64.iso' | head -1)
curl -fsSL -o alpine.iso "$URL/$MENO"
SUCET=$(curl -fsSL "$URL/$MENO.sha256" | cut -d' ' -f1)
vystup=$("$S" skumaj alpine.iso sucet="$SUCET")
echo "$vystup"
grep -q "hybridné ISO" <<<"$vystup" || chyba "Alpine nie je rozpoznané ako hybridné"
grep -q "UEFI" <<<"$vystup" || chyba "Alpine UEFI"
grep -q "súčet SHA-256 sedí" <<<"$vystup" || chyba "Alpine SHA-256"
rm -f alpine.iso
echo "✅ $MENO"
echo "✅ SADIČ: formátovanie, rozbor, napálenie a overenie – všetko sedí"
