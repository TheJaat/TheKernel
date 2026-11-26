#!/bin/sh
# times-test.sh - TaajFS version 6 (journal entry 101): file times, a
# volume and a file over 256 MiB, a rename that replaces, and programs
# found in /bin.
#
#   usage: times-test.sh ARCH IMAGE DISK LOG TIMES LS
#
# TIMES is times.c (next to this script) built for ARCH, LS the ls
# program built for it. The disk is a
# 320 MiB volume (81920 blocks: over the 65536 an older TheTaaJ could
# mount) with TIMES at /bin/times and LS at /bin/ls only, and "big", 300 MiB, put by the
# host's tool. One boot; TheTaaJ runs
#   with programs times      the checks: "times: N checks, all passed"
#   ls -l                    the files, with when each changed
# and afterwards the host's tool reads back what TheTaaJ wrote past
# 256 MiB, and writes a file of its own with the host's time.

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
TIMES=$5
LS=$6
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}

fail() {
    echo "FAIL: $1 (log: $LOG)"
    exit 1
}

keys() {
    python3 -I "$HERE/key-names.py" "$1" || fail "no keys for: $1"
}

MIB=1048576
BIG=$DISK.big
rm -f "$DISK" "$BIG"
"$TFSTOOL" mkfs "$DISK" 81920 > /dev/null || fail "cannot make the disk"
"$TFSTOOL" mkdir "$DISK" bin > /dev/null || fail "cannot make /bin"
"$TFSTOOL" put "$DISK" bin/times "$TIMES" > /dev/null || fail "cannot put times in /bin"
"$TFSTOOL" put "$DISK" bin/ls "$LS" > /dev/null || fail "cannot put ls in /bin"
# Block n of "big": n as four bytes, low first, then n's low byte.
python3 -I - "$BIG" <<'EOF' || fail "cannot make the 300 MiB file"
import struct, sys
with open(sys.argv[1], "wb") as out:
    for n in range(300 * 256):
        out.write(struct.pack("<I", n) + bytes([n & 0xFF]) * 4092)
EOF
"$TFSTOOL" put "$DISK" big "$BIG" > /dev/null || fail "cannot put the 300 MiB file on the disk"
rm -f "$BIG"

steps="$(keys 'echo == checks ==') ret ~ $(keys 'with programs times') ret ~~"
steps="$steps $(keys 'echo == list ==') ret ~ $(keys 'ls -l') ret ~"
steps="$steps $(keys 'echo == end ==') ret ~ $(keys 'echo done') ret ~"
echo "Booting $ARCH to check file times, a 320 MiB volume with a 300 MiB file, a replacing rename and /bin (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" QEMU_ARGS="-drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 90
else
    DISK="$DISK" KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 90
fi
WORK=$LOG.clean
tr -d '\r' < "$LOG" > "$WORK"

grep -aq '^== end ==$' "$WORK" || fail "the steps did not all run"
grep -a '^times: ' "$WORK" | grep -av 'all passed' | head -20
CHECKS=$(($(grep -c 'check(' "$HERE/times.c") - 1))
grep -aq "^times: $CHECKS checks, all passed$" "$WORK" || fail "the $CHECKS checks in times.c did not all pass"
grep -aqE 'unknown command: (times|ls)' "$WORK" && fail "the shell did not find a program in /bin"
# ls -l: big, 301 MiB, with a date and time this century.
grep -aqE "^ *315621376  20[0-9][0-9]-[01][0-9]-[0-3][0-9] [0-2][0-9]:[0-5][0-9]:[0-5][0-9]  big$" "$WORK" \
    || fail "ls -l did not show big's size and time"
grep -a 'has not answered for' "$WORK" && fail "a service stopped answering during the test"
grep -aiE 'panic|page fault in kernel|CPU EXCEPTION' "$WORK" && fail "the kernel failed"
rm -f "$WORK"

# What TheTaaJ wrote, read by the host: past 256 MiB, and the MiB added.
got=$("$TFSTOOL" get "$DISK" big 2> /dev/null | dd bs=1 skip=$((290 * MIB + 100)) count=18 2> /dev/null)
[ "$got" = "written by TheTaaJ" ] || fail "the host does not read what TheTaaJ wrote past 256 MiB (got '$got')"
got=$("$TFSTOOL" get "$DISK" big 2> /dev/null | tail -c 4)
[ "$got" = "mmmm" ] || fail "the host does not read the MiB TheTaaJ added"
# The disk keeps times now (version 6), and the host's tool writes them.
DUMP=$("$TFSTOOL" dump "$DISK") || fail "the disk does not dump clean after the boot"
echo "$DUMP" | grep -q 'format version 6' || fail "the disk was not raised to version 6"
echo late > "$DISK.late"
"$TFSTOOL" put "$DISK" late "$DISK.late" > /dev/null || fail "the host's tool cannot write to the version 6 disk"
rm -f "$DISK.late"
newest=$("$TFSTOOL" dump "$DISK" | sed -n 's/.* time \([0-9]*\)$/\1/p' | sort -n | tail -1)
late=$("$TFSTOOL" dump "$DISK" | sed -n 's/^ *late .* time \([0-9]*\)$/\1/p')
[ -n "$late" ] && [ "$late" = "$newest" ] || fail "the file the host wrote last is not the newest ($late, newest $newest)"
[ "$late" -gt 1700000000000000000 ] || fail "the host's file has no time of day ($late)"
rm -f "$DISK"
echo "TaajFS version 6 ($ARCH): $CHECKS checks of file times, a 320 MiB volume with a 301 MiB file, a replacing rename and /bin passed; ls -l showed the time; the host read TheTaaJ's writes and wrote the newest file"
