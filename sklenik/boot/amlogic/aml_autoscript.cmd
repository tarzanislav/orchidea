# SKLENIK - X96 Max Plus (Amlogic S905X3): spusti sa RAZ pri stlaceni tlacidla v konektore AV (spachtla)
# Povodny zavadzac krabicky si zapamata nove poradie startu: najprv karta, potom USB, nakoniec Android.
# Android zostava nedotknuty - bez karty a bez USB kluca krabicka nastartuje Android ako predtym.
setenv sklenik_start 'if mmcinfo; then if fatload mmc 0 0x09000000 s905_autoscript; then autoscr 0x09000000; fi; fi; if usb start; then for d in 0 1 2 3; do if fatload usb ${d} 0x09000000 s905_autoscript; then autoscr 0x09000000; fi; done; fi'
setenv bootcmd 'run sklenik_start; run storeboot'
setenv upgrade_step 2
saveenv
sleep 1
reboot
