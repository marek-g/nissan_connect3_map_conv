#!/bin/sh
# navdiag - one-shot navigation diagnostics collector for the LCN2KAI Bosch head unit.
#
# How it runs: this file is placed on the USB as `logger` (see rootshellfs.sh). The
# dir-traversal mount (label "../../usr/bin/") makes the USB appear at /usr/bin/, so
# when the system calls `logger` this script executes (as root). It collects map/nav
# state + logs onto the USB (a ./navdiag/ dir under /usr/bin/ = the stick), then
# reboots - the reboot is your "done, pull the USB" signal.
#
# Output: /usr/bin/navdiag/*  -> after reboot, on the USB stick as ./navdiag/
#
# !!! IMPORTANT FOR A CRASH/REBOOT INVESTIGATION !!!
# This unit boots with `panic=1 panic_on_oops=1` on console=ttyS0,115200n8. That means a
# native fault (SIGSEGV/oops) in procmapengine/DAPIAPP reboots the box INSTANTLY and dmesg
# (RAM only) is LOST before this script can ever run. If the map engine is the thing that
# reboots, the USB logger may only capture the PRE-crash boot. The ONE reliable instrument
# for the actual fault address is the SERIAL CONSOLE (ttyS0 @ 115200 8N1): log it while it
# reboots and you get "pgfault ...", "pc/lr", "comm=<engine>" and the Call trace directly.
# Keep using this logger for the boot-time state; get a UART capture for the fault itself.
#
# This script is busybox-safe: NO head/tail/awk/sed-of-fancy, NO basename (use ${x##*/}).

OUT=/usr/bin/navdiag

# Root fs and the USB (/usr/bin) are mounted read-only; make them writable.
mount -o remount,rw / 2>/dev/null
mount -o remount,rw /usr/bin/ 2>/dev/null

# Re-entrancy guard: logger may be invoked more than once; only collect once.
[ -e "$OUT/.done" ] && exit 0
mkdir -p "$OUT" 2>/dev/null
echo "collected $(date) pid=$$ ppid=$PPID" > "$OUT/.done" 2>/dev/null

# helper: find a pid by matching $1 against /proc/<pid>/comm FIRST (15-char, but non-empty),
# then fall back to /proc/<pid>/cmdline. On this unit the nav processes leave cmdline EMPTY and
# expose only comm ("DAPIAPP.OUT", "procmapengine.o", "procbaselx_out."), so comm MUST be tried.
findpid() {
    for p in /proc/[0-9]*; do
        c=$(cat "$p/comm" 2>/dev/null)
        [ -n "$c" ] || continue
        case "$c" in *"$1"*) echo "${p#/proc/}"; return 0 ;; esac
    done
    for p in /proc/[0-9]*; do
        if grep -qa "$1" "$p/cmdline" 2>/dev/null; then echo "${p#/proc/}"; return 0; fi
    done
    return 1
}

# The nav map is NOT in /data on this unit. It lives on the SD card (vfat, RO) mounted at
# /dev/media/<label>/ (e.g. /dev/media/9016-4EF8/CRYPTNAV). Discover the CRYPTNAV root so every
# section below can address it. Search the real mount points, not the wrong old /data path.
CR=""
for d in /dev/media/* /media/* /mnt/* /run/media/*; do
    [ -d "$d/CRYPTNAV" ] && { CR="$d/CRYPTNAV"; break; }
done
[ -n "$CR" ] || CR=$(find /dev/media /media /mnt /run/media -maxdepth 6 -type d -name CRYPTNAV 2>/dev/null)
[ -n "$CR" ] || CR=""

# ---------------------------------------------------------------------------
# Small, always-fit files first (so even on a tiny USB the key facts survive)
# ---------------------------------------------------------------------------
{
    echo "=== CRYPTNAV root = [$CR] ==="
    echo "=== uname ==="; uname -a
    echo; echo "=== kernel cmdline (look for panic_on_oops / console) ==="; cat /proc/cmdline 2>/dev/null
    echo; echo "=== versions ==="
    cat /lcn2kai_version.txt 2>/dev/null; echo
    cat /rfs_version.txt 2>/dev/null; echo
    cat /cc_label.txt 2>/dev/null; echo
    echo "=== uptime ==="; cat /proc/uptime 2>/dev/null
    echo "=== date ==="; date
    echo "=== df (USB is /usr/bin) ==="; df 2>/dev/null
} > "$OUT/00_manifest.txt" 2>&1

# full process list (whatever format this busybox gives)
{ echo "--- ps ---"; ps 2>/dev/null; echo "--- ps w ---"; ps w 2>/dev/null; } > "$OUT/01_ps.txt" 2>&1

# all mounts: shows where the SD card / map data actually lives
{ echo "=== /proc/mounts ==="; cat /proc/mounts 2>/dev/null
  echo "=== mount ==="; mount 2>/dev/null; } > "$OUT/02_mounts.txt" 2>&1

# kernel ring buffer (dmesg only - do NOT read /dev/kmsg, it blocks)
{ dmesg 2>/dev/null; } > "$OUT/03_dmesg.txt" 2>&1

# FAULT DISTILLATION from dmesg: with panic_on_oops a fault is an oops - grab the tell-tale
# lines into one small file. Empty here just means the crash wasn't this boot (see header note).
{
    echo "=== dmesg fault/exception grep (this boot only) ==="
    grep -inE 'segfault|internal error|pgfault|unhandled|unable to handle|alignment|watchdog|reset|panic|oops|pc ?:|lr ?:|sp ?:|psr ?:|call ?trace|backtrace|comm:"?[A-Za-z]|SIGSEGV|SIGABRT' "$OUT/03_dmesg.txt" 2>/dev/null
    echo "=== (end) ==="
} > "$OUT/33_dmesg_faults.txt" 2>&1

# Storage inventory: the SD CRYPTNAV region dir AND the internal bosch partitions, so we can
# see exactly which N6E2 files are present + sizes across every candidate map root.
{
    echo "=== CRYPTNAV root = [$CR] ==="
    if [ -z "$CR" ]; then
        echo "!! CRYPTNAV NOT discovered - listing every media root instead:"
        for base in /dev/media /media /mnt /run/media; do echo "-- $base --"; ls -la "$base" 2>/dev/null; done
    fi
    echo "=== $CR/DATA/DATA/MAP (region tiles) ==="; ls -la "$CR/DATA/DATA/MAP/" 2>/dev/null
    echo "=== $CR/DATA/CONNECT/MAP (index: IDX_CNT.TBL / RPITABLE.RPI / *.IDX) ==="
    ls -la "$CR/DATA/CONNECT/MAP/" 2>/dev/null
    echo "=== internal: /var/opt/bosch/region_static ==="; ls -la /var/opt/bosch/region_static/ 2>/dev/null
    echo "=== internal: /var/opt/bosch/dynamic ==="; ls -la /var/opt/bosch/dynamic/ 2>/dev/null
    echo "=== any CRYPTNAV roots (in case of multiple SDs) ==="
    find /dev/media /media /mnt /run/media -maxdepth 6 -type d -name CRYPTNAV 2>/dev/null
} > "$OUT/04_mapfiles.txt" 2>&1

# DISTILLED KNEST/LinkedTable evidence - the whole-world-blank decision happens at
# boot when DAPIAPP re-knits the region list against IDX_CNT.TBL. Pull every line
# mentioning the index/knit/region machinery out of ALL logs into one small file so
# the smoking gun is trivial to find. Broad net on purpose (exact wording may vary).
{
    echo "=== grep: knit / linked-table / idx / region / dataserver across /var/log ==="
    grep -aiHE 'knit|linked ?table|linkled|IDX_CNT|RPITABLE|region list|RegProf|N6E2|oIdxFileIdList|u16SetFlagsInLinkledTbl|InitLinkedTable|Storelinked|bIsLinkedTableInitialized|u16CorrectIdxFileIDList|Data Server init|start of dataserver|Inconsistency|map file|assert|abort|invalid|fault|exception|crash' \
        /var/log/*.log 2>/dev/null
    echo "=== (end grep; empty == no trace_syslog output -> enable map-engine trace, see 31/34) ==="
    echo "=== note: 'Cannot find map file.' == klogd missing System.map, NOT the nav map ==="
} > "$OUT/06_knit_grep.txt" 2>&1

# the persistent index itself, as it sits on the SD card. compare against stock to
# learn whether it matches my swapped region (it is RO, so a re-knit CANNOT rewrite it).
cp "$CR"/DATA/CONNECT/MAP/IDX_CNT.TBL  "$OUT/07_IDX_CNT.TBL"  2>/dev/null
cp "$CR"/DATA/CONNECT/MAP/RPITABLE.RPI "$OUT/08_RPITABLE.RPI" 2>/dev/null

# prove the swapped files actually landed on the SD card (md5+size), both dirs.
{
    echo "CRYPTNAV=[$CR]"
    echo "=== md5 + size of N6E2* (DATA/DATA/MAP) ==="
    for f in "$CR"/DATA/DATA/MAP/N6E2*; do [ -e "$f" ] && { command -v md5sum >/dev/null 2>&1 && md5sum "$f"; ls -l "$f"; }; done 2>/dev/null
    echo "=== md5 + size of N6E2* (DATA/CONNECT/MAP) ==="
    for f in "$CR"/DATA/CONNECT/MAP/N6E2*; do [ -e "$f" ] && { command -v md5sum >/dev/null 2>&1 && md5sum "$f"; ls -l "$f"; }; done 2>/dev/null
} > "$OUT/09_oncar_files.txt" 2>&1

# which nav + trace/syslog processes are alive (matched on comm, since cmdline is empty here).
{
    echo "=== nav + trace procs (from /proc/*/comm) ==="
    for p in /proc/[0-9]*; do
        c=$(cat "$p/comm" 2>/dev/null) || continue
        case "$c" in
            *mapengine*|*DAPIAPP*|*PROCNAV*|*procnav*|*baselx*|*trace*|*syslog*)
                echo "pid=${p#/proc/} comm=$c exe=$(readlink "$p/exe" 2>/dev/null)";;
        esac
    done
} > "$OUT/05_navprocs.txt" 2>&1

# trace + syslog configuration (for reference)
cp /etc/syslog.conf          "$OUT/30_syslog.conf"      2>/dev/null
cp /etc/pfcfg/PFCFG_trace.cfg "$OUT/31_PFCFG_trace.cfg" 2>/dev/null
{ echo "=== /tmp/trace/cf03 (live FIFOs) ==="; ls -la /tmp/trace/cf03/ 2>/dev/null
  echo "=== /var/log ==="; ls -la /var/log/ 2>/dev/null
  echo "=== trace-data dir ==="; ls -la /var/opt/bosch/dynamic/trace-data/ 2>/dev/null; } \
    > "$OUT/32_trace_status.txt" 2>&1

# snapshot the live trace FIFOs WITHOUT blocking (busybox `timeout` present on this unit).
# These are where the map-engine trace flows; /var/log/trace_*.log being empty means the levels
# are low, but if a writer pushes anything this catches a burst. Non-fatal if empty.
{
    echo "=== live trace FIFO burst (timeout 1s each, non-blocking) ==="
    for fifo in /tmp/trace/cf03/*; do
        [ -p "$fifo" ] || continue
        echo "--- $fifo ---"
        timeout 1 cat "$fifo" 2>/dev/null
    done
    echo "=== (end) ==="
} > "$OUT/34_trace_fifo_burst.txt" 2>&1

# per-process detail for the map engine + data app: which libs mapped, which region
# files open, thread state, and which map dir it uses. Key evidence for a render/load crash.
for name in procmapengine DAPIAPP PROCNAV; do
    pid=$(findpid "$name") || { echo "findpid $name: NOT FOUND" >> "$OUT/20_${name}_status.txt"; continue; }
    { echo "=== $name pid=$pid status ===";  cat "/proc/$pid/status" 2>/dev/null; } \
        > "$OUT/20_${name}_status.txt" 2>&1
    { echo "=== $name pid=$pid cwd/exe ==="; readlink "/proc/$pid/cwd" 2>/dev/null; readlink "/proc/$pid/exe" 2>/dev/null;
      echo "=== $name pid=$pid open files (fd -> path) ==="; ls -l "/proc/$pid/fd/" 2>/dev/null; } \
        > "$OUT/21_${name}_fd.txt" 2>&1
    { echo "=== $name pid=$pid maps ==="; cat "/proc/$pid/maps" 2>/dev/null; } \
        > "$OUT/22_${name}_maps.txt" 2>&1
done

# DAPIAPP is a block-read engine (opens a tile, reads, closes) so a single fd
# snapshot usually catches nothing. Poll for ~15s while you stare at the map:
# if it EVER opens an N6E2 .MAP/.IDX/.TCI we'll see it here; if it stays empty the engine
# is not even trying to read that region (wrong region / gave up early).
{
    echo "=== DAPIAPP fd poll 15 x 1s (want any .MAP/.IDX/.TCI / CRYPTNAV open) ==="
    i=0
    while [ $i -lt 15 ]; do
        pid=$(findpid DAPIAPP) || { echo "iter $i: DAPIAPP not found"; i=$((i+1)); sleep 1; continue; }
        echo "--- iter $i pid=$pid $(date) ---"
        ls -l "/proc/$pid/fd/" 2>/dev/null | grep -iE '\.MAP|\.IDX|\.TCI|CRYPTNAV|/dev/media'
        i=$((i+1)); sleep 1
    done
    echo "=== (end poll) ==="
} > "$OUT/23_DAPIAPP_fd_poll.txt" 2>&1

# ---------------------------------------------------------------------------
# Big logs LAST (best effort - may not fit on a small USB image)
# ---------------------------------------------------------------------------
cp /var/log/trace_err.log    "$OUT/10_trace_err.log"    2>/dev/null
cp /var/log/trace_notice.log "$OUT/11_trace_notice.log" 2>/dev/null
for f in /var/log/*; do
    [ -f "$f" ] || continue
    b=${f##*/}; [ -n "$b" ] || continue            # busybox-safe basename
    case "$b" in
        trace_err.log|trace_notice.log|lastlog|wtmp) ;;
        *) cp "$f" "$OUT/12_$b" 2>/dev/null ;;
    esac
done

# final inventory of what actually landed on the stick
ls -la "$OUT" > "$OUT/99_collected.txt" 2>&1

sync; sync
reboot
