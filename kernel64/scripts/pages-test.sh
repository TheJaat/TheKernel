#!/bin/sh
# pages-test.sh - memory for big programs, part 1 (journal entry 102):
# mprotect really changing what pages allow, madvise(MADV_DONTNEED) and
# munmap of part of a mapping giving memory back, a heap that gives
# freed memory back, and system calls racing a thread that takes their
# buffer away.
#
#   usage: pages-test.sh ARCH IMAGE DISK LOG PAGES
#
# PAGES is pages.c (next to this script) built for ARCH. One boot;
# TheTaaJ runs
#   with programs pages     the checks: "pages: N checks, all passed"

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
PAGES=$5
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
"$TFSTOOL" put "$DISK" pages "$PAGES" > /dev/null || fail "cannot put pages on the disk"

steps="$(keys 'echo == checks ==') ret ~ $(keys 'with programs pages') ret ~~"
steps="$steps $(keys 'echo == end ==') ret ~ $(keys 'echo done') ret ~"
echo "Booting $ARCH to check mprotect, madvise, munmap of part of a mapping, a heap that gives memory back and copies racing an unmap (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" QEMU_ARGS="-drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 60
else
    DISK="$DISK" KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 60
fi
WORK=$LOG.clean
tr -d '\r' < "$LOG" > "$WORK"

grep -aq '^== end ==$' "$WORK" || fail "the steps did not all run"
grep -a '^pages: failed' "$WORK" | head -20
grep -a '^pages: [0-9]* copies' "$WORK"
CHECKS=$(grep -c 'check(' "$HERE/pages.c")
CHECKS=$((CHECKS - 1))
grep -aq "^pages: $CHECKS checks, all passed$" "$WORK" || fail "the $CHECKS checks in pages.c did not all pass"
grep -aq 'not reached' "$WORK" && fail "$(grep -a 'not reached' "$WORK" | head -1)"
grep -a 'has not answered for' "$WORK" && fail "a service stopped answering during the test"
grep -aiE 'panic|page fault in kernel|CPU EXCEPTION' "$WORK" && fail "the kernel failed"
grep -a '"pages" killed' "$WORK" && fail "pages was killed: a fault it should have caught, or out of memory"
rm -f "$WORK"
echo "memory for big programs, part 1 ($ARCH): $CHECKS checks of mprotect, madvise, munmap of part of a mapping, a heap giving memory back and copies racing an unmap passed"
