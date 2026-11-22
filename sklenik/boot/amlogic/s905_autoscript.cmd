# SKLENIK - X96 Max Plus: povodny zavadzac krabicky spusti nas zavadzac u-boot.ext (U-Boot, vlastna zostava),
# ten najde extlinux/extlinux.conf a spusti jadro Linux so SKLENIKOM. Do pamate krabicky sa nic nezapisuje.
if fatload mmc 0 0x01000000 u-boot.ext; then go 0x01000000; fi
for d in 0 1 2 3; do if fatload usb ${d} 0x01000000 u-boot.ext; then go 0x01000000; fi; done
