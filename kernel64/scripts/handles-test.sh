#!/bin/sh
# handles-test.sh - open files as handles (journal entry NNN, J60 and
# J61): a descriptor follows its file through a rename, outlives a
# removal and a replacing rename, and is handed to a child as the file.
#
#   usage: handles-test.sh ARCH IMAGE DISK LOG HANDLES
#
# HANDLES is handles.c (next to this script) built for ARCH. One boot;
# TheTaaJ runs
#   with programs handles     the checks: "handles: N checks, all passed"

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
HANDLES=$5
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

rm -f "$DISK"
"$TFSTOOL" mkfs "$DISK" 1024 > /dev/null && truncate -s 16M "$DISK" || fail "cannot make the disk"
"$TFSTOOL" put "$DISK" handles "$HANDLES" > /dev/null || fail "cannot put handles on the disk"

steps="$(keys 'echo == checks ==') ret ~ $(keys 'with programs handles') ret ~~"
steps="$steps $(keys 'echo == end ==') ret ~ $(keys 'echo done') ret ~"
echo "Booting $ARCH to check open files as handles (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" QEMU_ARGS="-drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 60
else
    DISK="$DISK" KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 60
fi
WORK=$LOG.clean
tr -d '\r' < "$LOG" > "$WORK"

grep -aq '^== end ==$' "$WORK" || fail "the steps did not all run"
grep -a '^handles: failed' "$WORK" | head -20
# Every check(...) in handles.c but the function itself.
CHECKS=$(($(grep -c 'check(' "$HERE/handles.c") - 1))
grep -aq "^handles: $CHECKS checks, all passed$" "$WORK" || fail "the $CHECKS checks in handles.c did not all pass"
grep -aq 'not reached' "$WORK" && fail "$(grep -a 'not reached' "$WORK" | head -1)"
grep -a 'has not answered for' "$WORK" && fail "a service stopped answering during the test"
grep -aiE 'panic|page fault in kernel|CPU EXCEPTION' "$WORK" && fail "the kernel failed"
grep -a 'killed by the kernel' "$WORK" && fail "a handles was killed by the kernel"
rm -f "$WORK"
echo "open files as handles ($ARCH): $CHECKS checks of descriptors through renames, removals and a child passed"
