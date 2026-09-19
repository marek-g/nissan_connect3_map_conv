#!/bin/sh
# routeprobe - on-device file-open capture for the RNW load-path question (TODO #16 POL blocker).
#
# WHY: no .tci shard on the stock card references POL's road clusters and Krakow's own
# N6E2102.TCI ref pool is zero-filled, yet routing works in Krakow. We must learn WHICH
# files the running system actually opens when it computes a Polish route: per-shard
# DATA/DATA/MAP/*.tci, the per-region NAV_ROOT.DAT, the NAV%05u.DAT files, the
# NAV00001.DAT ("databases//00001.wrk") container, or something in DATA/CONNECT.
#
# HOW: this file is placed on the USB exploit stick as `logger` (see
# rootshellfs_navdiag.sh, mode "route"). The dir-traversal mount makes it run as root at
# boot. It then polls every /proc/<pid>/fd every 2 s for ~7 minutes and appends every NEW
# data-file fd seen (diff against the seen-set) to one event log, with the owning pid+comm.
# At the end it also dumps filtered /proc/<pid>/maps (mmaps survive fd closes) and reboots
# - the reboot is your "done, pull the USB" signal.
#
# OPERATING PROTOCOL (do this in the 7 minutes after the home screen appears):
#   1. let the nav app start fully with the STOCK card (Poland region);
#   2. set destination somewhere inside Krakow (city/street search is fine - that is LID,
#      we want the ROUTING part); start route calculation;
#   3. wait for the route to be drawn, then drive/scroll the map a bit around Krakow;
#   4. wait for the automatic reboot; pull the USB; the capture is in ./routeprobe/ .
# Capture with the SAME procedure on a German destination afterwards if you want a control
# (boot again, boot the logger, route Berlin->Munich, reboot, keep both ./routeprobe dirs).
#
# Busybox-safe: no awk/sed magic, no basename, no sort, no inotify. grep -vxF -f is used
# for the seen-set diff.

OUT=/usr/bin/routeprobe

mount -o remount,rw / 2>/dev/null
mount -o remount,rw /usr/bin/ 2>/dev/null

[ -e "$OUT/.done" ] && exit 0
mkdir -p "$OUT" 2>/dev/null
echo "routeprobe start $(date) pid=$$ ppid=$PPID" > "$OUT/.done" 2>/dev/null

EVENTS="$OUT/10_fd_events.txt"
SEEN="$OUT/99_seen.txt"
CYCLES=200
SLEEP_S=2

comm_of() { cat "/proc/$1/comm" 2>/dev/null; }

# fd lines for one pid, annotated: "<pid> <comm> <fdline>"
fd_lines() {
    p=$1
    [ -d "/proc/$p/fd" ] || return 0
    c=$(comm_of "$p")
    ls -l "/proc/$p/fd" 2>/dev/null | grep -aiE 'CRYPTNAV|DATA|\.tci|NAV|AEX|\.DAT|\.WRK|wrk' | while read -r l; do
        echo "$p $c $l"
    done
}

# all-pids fd snapshot -> stdout
all_fd() {
    for p in /proc/[0-9]*; do
        fd_lines "${p#/proc/}"
    done
}

# ---------------------------------------------------------------------------
# identity context (small files, do first)
# ---------------------------------------------------------------------------
{
    echo "=== routeprobe ==="; date; cat /proc/uptime 2>/dev/null
    echo; echo "=== ps w ==="; ps w 2>/dev/null; ps 2>/dev/null
    echo; echo "=== procs (pid comm cmdline) ==="
    for p in /proc/[0-9]*; do
        pid=${p#/proc/}
        c=$(comm_of "$pid")
        [ -n "$c" ] || continue
        echo "$pid $c $(cat "$p/cmdline" 2>/dev/null | tr -d '\0' )"
    done
} > "$OUT/00_procs.txt" 2>&1

{ echo "=== /proc/mounts ==="; cat /proc/mounts 2>/dev/null
  echo "=== mount ==="; mount 2>/dev/null; } > "$OUT/01_mounts.txt" 2>&1

CR=""
for d in /dev/media/* /media/* /mnt/* /run/media/*; do
    [ -d "$d/CRYPTNAV" ] && { CR="$d/CRYPTNAV"; break; }
done
echo "CRYPTNAV=[$CR]" > "$OUT/02_cryptnav.txt"
ls -la "$CR/DATA/DATA/RNW/CCP/" >> "$OUT/02_cryptnav.txt" 2>/dev/null
ls -la "$CR/DATA/DATA/RNW/CCP/POL/" >> "$OUT/02_cryptnav.txt" 2>/dev/null

# ---------------------------------------------------------------------------
# baseline snapshot + seed the seen-set (what is open BEFORE we do anything)
# ---------------------------------------------------------------------------
all_fd > "$OUT/03_fd_baseline.txt" 2>/dev/null
cp "$OUT/03_fd_baseline.txt" "$SEEN" 2>/dev/null
: > "$EVENTS" 2>/dev/null
echo "# events = fds that APPEARED after baseline, one line per new (pid,fd,path)" >> "$EVENTS"

# ---------------------------------------------------------------------------
# polling loop: ~200 x 2 s = ~7 min of user driving/routing
# ---------------------------------------------------------------------------
n=0
while [ "$n" -lt "$CYCLES" ]; do
    n=$((n + 1))
    cur="$OUT/98_cyc.tmp"
    all_fd > "$cur" 2>/dev/null
    new=$(grep -vxF -f "$SEEN" "$cur" 2>/dev/null)
    if [ -n "$new" ]; then
        echo "# CYC $n $(date)" >> "$EVENTS"
        echo "$new" >> "$EVENTS"
        cat "$cur" >> "$SEEN"
    fi
    rm -f "$cur" 2>/dev/null
    sleep "$SLEEP_S"
done

# ---------------------------------------------------------------------------
# final full snapshot + maps (mmaps of data files survive fd close)
# ---------------------------------------------------------------------------
all_fd > "$OUT/04_fd_final.txt" 2>/dev/null
{
    for p in /proc/[0-9]*; do
        pid=${p#/proc/}
        [ -r "$p/maps" ] || continue
        m=$(grep -aiE 'CRYPTNAV|DATA|\.tci|NAV|AEX|\.DAT|wrk' "$p/maps" 2>/dev/null)
        [ -n "$m" ] || continue
        echo "### pid $pid $(comm_of "$pid")"
        echo "$m"
    done
} > "$OUT/05_maps.txt" 2>&1

# which nav processes existed, so the event pids can be resolved offline
{
    echo "=== dmesg nav/rnw/tci grep (this boot) ==="
    dmesg 2>/dev/null | grep -inE 'rnw|tci|NAV_|cluster|routing|route|procnav|dapi'
} > "$OUT/06_dmesg.txt" 2>&1

echo "routeprobe done $(date)" >> "$OUT/.done"
rm -f "$SEEN" 2>/dev/null
sync; sync
reboot
