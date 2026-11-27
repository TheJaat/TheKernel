#!/bin/sh
# fuzz-test.sh - the system-call fuzzer (K104, journal entry 123): the
# kernel takes random calls from programs with no handles, and is still
# there afterwards.
#
#   usage: fuzz-test.sh ARCH IMAGE DISK LOG FUZZ [CHILDREN CALLS SEED]
#
# FUZZ is the fuzz program (user/src/bin/fuzz.rs) built for ARCH. The
# machine runs `with programs fuzz CHILDREN CALLS SEED` (100 children of
# 3000 calls each, seed 1, unless told) and then a few ordinary
# commands. It must say how every child ended, never panic, and answer
# the commands after - a kernel that hung or lost something would not.

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
FUZZ=$5
CHILDREN=${6:-100}
CALLS=${7:-3000}
SEED=${8:-1}
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
"$TFSTOOL" put "$DISK" fuzz "$FUZZ" > /dev/null || fail "cannot put fuzz on the disk"

# The fuzzer's line runs for up to two seconds a child: "~~" a few times.
steps="$(keys "with programs fuzz $CHILDREN $CALLS $SEED ${TRACE:-}") ret ~~ ~~ ~~ ~~"
steps="$steps $(keys 'ps') ret ~ $(keys 'echo still here') ret ~"

echo "Booting $ARCH for the system-call fuzzer: $CHILDREN programs, $CALLS calls each, seed $SEED (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" QEMU_ARGS="-drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 120
else
    DISK="$DISK" KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 120
fi

said() { grep -aEq "$1" "$LOG" || fail "$2"; }
! grep -aq "KERNEL PANIC\|CPU EXCEPTION" "$LOG" || { sed -n '/KERNEL PANIC\|CPU EXCEPTION/,$p' "$LOG" | head -30; fail "the kernel stopped"; }
said "^fuzz: $CHILDREN children, $CALLS calls each, seeds $SEED to " "fuzz did not start"
said "^fuzz: [0-9]+ made all their calls, [0-9]+ were killed for breaking a rule, [0-9]+ were stopped after 2 s, [0-9]+ ended otherwise, 0 did not start" \
    "fuzz did not say how every child ended (or one did not start)"
said "^fuzz: done - $CHILDREN programs with no handles, [0-9]+ calls from those that finished, the kernel still here" "fuzz did not finish"
said '^\[shell\] ran "echo still here"' "the shell did not answer after the fuzzer"
grep -aE "^fuzz: [0-9]+ made all" "$LOG" | head -1
