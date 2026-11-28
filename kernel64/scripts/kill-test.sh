#!/bin/sh
# kill-test.sh - each disk driver killed in the middle of its I/O (K82,
# Era 15's measure): the system recovers, and nothing that was
# acknowledged is lost.
#
#   usage: kill-test.sh IMAGE DISK LOG NETDISK BIG
#
# DISK is a test disk (make-test-disk: the programs, killcheck among
# them). Four PCs at once, each with its system disk on another kind
# of controller, and so another driver: virtio (blk), SATA (ahci), NVMe
# (nvme), a USB stick (usb). On each, three times:
#
#   disk crash N                    the driver will die in the middle of
#                                   its Nth write from now: half of it
#                                   written, no answer (block_service.rs)
#   with files killcheck write S 12 12 files of 8 KiB, each written and
#                                   committed; every write the file
#                                   service answered is said ("acked"),
#                                   one that failed is tried again
#
# with N = 3, 17 and 40, so that the driver dies under different parts
# of a commit: a file's data, the tree above it, the superblock. Then
# `killcheck check` reads every file back in the running system, and
# after the machine is off the disk image is checked on the host: the
# volume healthy (tfstool dump), and every acked file exactly as
# written (tfstool get) - on the medium, not only in fs's cache.
#
# A fifth PC, at the same time, for the network card: its driver (nic,
# virtio) will die in the middle of sending its 200th frame from now
# (`niccheck crash 200`), and `httpd` serves this host two downloads of
# BIG (1 MiB, www/big.bin on NETDISK) through QEMU's own network. The
# first is under way when the driver dies: its frames meanwhile are
# lost, and TCP must send them again once init has started another
# driver - the download whole, byte for byte. The second, after, on
# the new driver.

set -u
IMAGE=$1
DISK=$2
LOG=$3
NETDISK=$4
BIG=$5
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}

fail() {
    echo "FAIL: $1 (logs: $LOG.*)"
    exit 1
}

keys() {
    python3 -I "$HERE/key-names.py" "$1" || fail "no keys for: $1"
}

[ -f "$DISK" ] || fail "no test disk at $DISK"
SETS="a:3 b:17 c:40"

steps=""
for set in $SETS; do
    steps="$steps $(keys "disk crash ${set#*:}") ret ~ $(keys "with files killcheck write ${set%:*} 12") ret ~~"
done
for set in $SETS; do
    steps="$steps $(keys "with files killcheck check ${set%:*} 12") ret ~~"
done
steps="$steps $(keys 'echo == end ==') ret ~"

# Each kind of disk: its QEMU options (the disk at $DISK.NAME).
options() {
    case $1 in
        blk) echo "" ;;
        # The CD on the SATA bus too: the PC's BIOS boots from it there.
        ahci) echo "-device ahci,id=sata -drive if=none,id=cd,media=cdrom -device ide-cd,drive=cd,bus=sata.0 -drive file=$DISK.ahci,format=raw,if=none,id=d0 -device ide-hd,drive=d0,bus=sata.2" ;;
        nvme) echo "-drive file=$DISK.nvme,format=raw,if=none,id=n0 -device nvme,serial=taaj0001,drive=n0" ;;
        # With no other disk, usb serves the stick as the system disk.
        usb) echo "-device qemu-xhci,id=xhci -drive if=none,id=stick,format=raw,file=$DISK.usb -device usb-storage,bus=xhci.0,drive=stick" ;;
    esac
}

DRIVERS="blk ahci nvme usb"
echo "Booting four PCs - virtio, SATA, NVMe and USB disks - and killing each disk driver mid-write, three times (logs: $LOG.*)..."
for driver in $DRIVERS; do
    cp "$DISK" "$DISK.$driver" || fail "cannot copy the disk"
    # The options first: in "DISK=... QEMU_ARGS=$(options)" the shell
    # sets DISK before it runs `options`, which reads $DISK.
    args=$(options $driver)
    disk=""
    [ "$driver" = blk ] && disk="$DISK.$driver"
    DISK="$disk" KEYS="$steps" QEMU_ARGS="$args" SMP=2 SLOTS=3 \
        "$HERE/qemu-boot.sh" "$IMAGE" "$LOG.$driver" 150 > "$LOG.$driver.out" 2>&1 &
done

# The network card's PC: a port of this host passed to its port 80.
port=$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
net_steps="$(keys 'niccheck crash 200') ret ~ $(keys 'httpd 80 /www 2') ret ~~ $(keys 'echo == end ==') ret ~"
DISK="$NETDISK" KEYS="$net_steps" QEMU_ARGS="-netdev user,id=n0,hostfwd=tcp:127.0.0.1:$port-:80 -device virtio-net-pci,netdev=n0" SMP=1 \
    "$HERE/qemu-boot.sh" "$IMAGE" "$LOG.net" 150 > "$LOG.net.out" 2>&1 &
# This host's side: once httpd serves, the two downloads.
(
    ticks=0
    until grep -aq '^httpd: serving' "$LOG.net" 2>/dev/null || [ $ticks -ge 1200 ]; do
        sleep 0.1
        ticks=$((ticks + 1))
    done
    for n in 1 2; do
        curl -s --max-time 90 -o "$LOG.net.got$n" -w '%{http_code} %{size_download}\n' "http://127.0.0.1:$port/big.bin" > "$LOG.net.curl$n" 2>&1
    done
) &
wait

# The network card: killed mid-send, restarted, and both downloads whole.
L="$LOG.net"
tr -d '\r' < "$L" > "$L.clean"
grep -aq '^== end ==$' "$L.clean" || fail "net: the steps did not all run"
grep -aq '\[nic\] asked to die in the middle of sending frame 200 from now' "$L.clean" || fail "net: nic was not asked to die mid-send"
grep -aq '\[nic\] dying mid-send, as asked' "$L.clean" || fail "net: nic did not die mid-send (fewer than 200 frames sent?)"
grep -aq '\[init\] nic ended with code -1000 - restarting it' "$L.clean" || fail "net: init did not restart nic after it was killed"
grep -aiE 'panic|page fault in kernel|CPU EXCEPTION' "$L.clean" && fail "net: the kernel failed"
for n in 1 2; do
    cmp -s "$L.got$n" "$BIG" || fail "net: download $n is not BIG byte for byte (curl said: $(cat "$L.curl$n"))"
done
echo "net: nic killed mid-send and restarted while a download was under way; it and the next arrived whole ($(wc -c < "$BIG" | tr -d ' ') bytes each)"

for driver in $DRIVERS; do
    L="$LOG.$driver"
    tr -d '\r' < "$L" > "$L.clean"
    has() { grep -aEq "$1" "$L.clean" || fail "$driver: $2"; }
    count() { grep -acE "$1" "$L.clean"; }
    has '^== end ==$' "the steps did not all run"
    [ "$(count "\\[$driver\\] asked to die in the middle of write")" = 3 ] || fail "$driver: was not asked three times to die mid-write"
    [ "$(count "\\[$driver\\] dying mid-write, as asked")" = 3 ] \
        || fail "$driver: did not die mid-write three times ($(count "\\[$driver\\] dying mid-write") times)"
    [ "$(count "\\[init\\] $driver ended with code -1000 - restarting it")" = 3 ] \
        || fail "$driver: init did not restart it three times after it was killed"
    for set in $SETS; do
        s=${set%:*}
        has "^killcheck: wrote 12 of 12 files of 8192 bytes" "set $s: killcheck did not write all 12 files once the driver was back"
        has "^killcheck: 12 files of $s, all as written$" "set $s: the files read back are not all as written"
    done
    grep -aiE 'panic|page fault in kernel|CPU EXCEPTION' "$L.clean" && fail "$driver: the kernel failed"
    grep -aE '^\[init\] .* giving up' "$L.clean" && fail "$driver: init gave a service up"
    # On the medium: the volume, then every acked file, byte for byte.
    "$TFSTOOL" dump "$DISK.$driver" > "$L.dump" 2>&1 || fail "$driver: the disk image is not a healthy volume (tfstool dump: $L.dump)"
    acked=$(grep -aoE '^killcheck: acked [a-z]+[0-9]+' "$L.clean" | sed 's/.* //')
    [ "$(echo "$acked" | wc -w)" -ge 36 ] || fail "$driver: fewer than 36 writes acked"
    for name in $acked; do
        "$TFSTOOL" get "$DISK.$driver" "$name" > "$L.file" 2> /dev/null || fail "$driver: $name was acked, but is not on the disk"
        python3 -I "$HERE/killcheck-file.py" "$name" "$L.file" || fail "$driver: $name was acked, but is not as written on the disk"
    done
    retried=$(count '^killcheck: [a-z]+[0-9]+: .* - trying again')
    echo "$driver: killed mid-write 3 times and restarted; $(echo "$acked" | wc -w) writes acked, $retried tried again; all on the disk as written"
done
rm -f "$LOG".*.file
echo "kill-test: every disk driver and the network card's recovered from being killed mid-I/O, and nothing acknowledged was lost"
