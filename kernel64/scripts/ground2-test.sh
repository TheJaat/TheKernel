#!/bin/sh
# ground2-test.sh - std's ground, part 2 (journal entry 100): poll and
# select, dl_iterate_phdr and getauxval, dlfcn, pthread attributes, the
# exec family with PATH, descriptors through spawn and exec, argv[0],
# stat and realpath, and zlib.
#
#   usage: ground2-test.sh ARCH IMAGE DISK LOG GROUND2
#
# GROUND2 is ground2.c (next to this script) built for ARCH, linked with
# zlib. One boot; TheTaaJ runs
#   with programs ground2     the checks: "ground2: N checks, all passed"

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
GROUND2=$5
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
"$TFSTOOL" put "$DISK" ground2 "$GROUND2" > /dev/null || fail "cannot put ground2 on the disk"

steps="$(keys 'echo == checks ==') ret ~ $(keys 'with programs ground2') ret ~~"
steps="$steps $(keys 'echo == end ==') ret ~ $(keys 'echo done') ret ~"
echo "Booting $ARCH to check poll, dl_iterate_phdr, pthread attributes, exec and zlib (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" QEMU_ARGS="-drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 60
else
    DISK="$DISK" KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 60
fi
WORK=$LOG.clean
tr -d '\r' < "$LOG" > "$WORK"

grep -aq '^== end ==$' "$WORK" || fail "the steps did not all run"
grep -a '^ground2: failed' "$WORK" | head -20
# Every check(...) in ground2.c but the function itself and the two
# that only run when something is already wrong (check(0, ...)).
CHECKS=$(($(grep -c 'check(' "$HERE/ground2.c") - $(grep -c 'check(0,' "$HERE/ground2.c") - 1))
grep -aq "^ground2: $CHECKS checks, all passed$" "$WORK" || fail "the $CHECKS checks in ground2.c did not all pass"
grep -aq 'not reached' "$WORK" && fail "$(grep -a 'not reached' "$WORK" | head -1)"
grep -a 'has not answered for' "$WORK" && fail "a service stopped answering during the test"
grep -aiE 'panic|page fault in kernel|CPU EXCEPTION' "$WORK" && fail "the kernel failed"
# Only the child that raised SIGTERM is ended by a signal; nothing is
# killed by the kernel.
grep -a 'killed by the kernel' "$WORK" && fail "a ground2 was killed by the kernel"
rm -f "$WORK"
echo "std's ground, part 2 ($ARCH): $CHECKS checks of poll, select, dl_iterate_phdr, getauxval, dlfcn, pthread attributes, exec with PATH, descriptors through exec, argv[0], realpath and zlib passed"
