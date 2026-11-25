#!/bin/sh
# ground-test.sh - std's ground, part 1 (journal entry 099): random
# numbers from the kernel, real thread-local storage, signal stacks and
# SIGCHLD with waitid.
#
#   usage: ground-test.sh ARCH IMAGE DISK LOG GROUND
#
# GROUND is ground.c (next to this script) built for ARCH. Two boots:
#
#   1. QEMU's usual CPU, which has no random instruction: the kernel
#      seeds its generator from timings alone. TheTaaJ runs
#        ground random          16 random bytes, in hex
#        with programs ground   the checks: "ground: N checks, all passed"
#   2. -cpu max, which has one (RDSEED/RDRAND on x86, RNDR on AArch64):
#      the kernel must say it used it. TheTaaJ runs ground random again,
#      and its bytes must differ from the first boot's.

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
GROUND=$5
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

WORK=$(cd "$(dirname "$LOG")" && pwd)/$(basename "$LOG").work
rm -rf "$WORK"
mkdir -p "$WORK"

rm -f "$DISK"
"$TFSTOOL" mkfs "$DISK" 1024 > /dev/null && truncate -s 16M "$DISK" || fail "cannot make the disk"
"$TFSTOOL" put "$DISK" ground "$GROUND" > /dev/null || fail "cannot put ground on the disk"

boot() { # boot LOG STEPS CPU
    if [ "$ARCH" = aarch64 ]; then
        CPU="$3" KEYS="$2" QEMU_ARGS="-drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
            "$HERE/aarch64-boot.sh" "$IMAGE" "$1" 60
    else
        cpu=
        [ "$3" = max ] && cpu="-cpu max"
        DISK="$DISK" KEYS="$2" QEMU_ARGS="$cpu" "$HERE/qemu-boot.sh" "$IMAGE" "$1" 60
    fi
}

steps="$(keys 'echo == random ==') ret ~ $(keys 'ground random') ret ~"
steps="$steps $(keys 'echo == checks ==') ret ~ $(keys 'with programs ground') ret ~~"
steps="$steps $(keys 'echo == end ==') ret ~ $(keys 'echo done') ret ~"
echo "Booting $ARCH to check random numbers, thread-local storage, signal stacks and SIGCHLD (log: $LOG)..."
CPU1=
[ "$ARCH" = aarch64 ] && CPU1=cortex-a72
boot "$LOG" "$steps" "$CPU1"
tr -d '\r' < "$LOG" > "$WORK/log"

part() { # part LOG NAME: the lines after "== NAME ==", up to the next mark
    awk -v start="== $2 ==" '$0 == start { on = 1; next } /^== [a-z-]+ ==$/ { on = 0 } on' "$1"
}
has() { # has NAME PATTERN WHY
    part "$WORK/log" "$1" | grep -aEq "$2" || fail "$3"
}
healthy() { # healthy LOG
    grep -aq 'not reached' "$1" && fail "$(grep -a 'not reached' "$1" | head -1)"
    grep -a 'has not answered for' "$1" && fail "a service stopped answering during the test"
    grep -aiE 'panic|page fault in kernel|CPU EXCEPTION' "$1" && fail "the kernel failed"
    return 0
}

grep -aq '^== end ==$' "$WORK/log" || fail "the steps did not all run"
grep -aEq '^\[random\] seeded from [0-9]+ timings \(no random instruction on this CPU\)' "$WORK/log" \
    || fail "the kernel did not say it seeded its generator from timings alone"
FIRST=$(part "$WORK/log" random | grep -aEo '^ground: random [0-9a-f]{32}$' | head -1 | cut -d' ' -f3)
[ -n "$FIRST" ] || fail "ground random printed no bytes"
part "$WORK/log" checks | grep -a '^ground: failed' | head -20
# Every check(...) in ground.c but the function itself.
CHECKS=$(($(grep -c 'check(' "$HERE/ground.c") - 1))
has checks "^ground: $CHECKS checks, all passed$" "the $CHECKS checks in ground.c did not all pass"
# The checks' own process must end well: exactly one ground is killed
# (the child with no signal stack), and the shell says nothing of one.
KILLED=$(part "$WORK/log" checks | grep -ac '"ground" killed')
[ "$KILLED" -eq 1 ] || fail "$KILLED ground processes were killed, not just the child with no signal stack"
part "$WORK/log" checks | grep -aq 'ground was killed by the kernel' && fail "ground itself was killed after its checks"
has checks '^ground: caught the stack overflow on the signal stack$' "the child's stack overflow was not caught on its signal stack"
healthy "$WORK/log"

steps="$(keys 'echo == random ==') ret ~ $(keys 'ground random') ret ~"
steps="$steps $(keys 'echo == end ==') ret ~ $(keys 'echo done') ret ~"
echo "Booting $ARCH again, on a CPU with a random instruction (log: $LOG.max)..."
boot "$LOG.max" "$steps" max
tr -d '\r' < "$LOG.max" > "$WORK/max"
grep -aq '^== end ==$' "$WORK/max" || fail "the second boot's steps did not all run"
SEEDED=$(grep -aE '^\[random\] seeded from 8 numbers from (RDSEED|RDRAND|RNDR) and [0-9]+ timings' "$WORK/max" | head -1)
[ -n "$SEEDED" ] || fail "on -cpu max the kernel did not use the CPU's random instruction"
SECOND=$(part "$WORK/max" random | grep -aEo '^ground: random [0-9a-f]{32}$' | head -1 | cut -d' ' -f3)
[ -n "$SECOND" ] || fail "ground random printed no bytes on the second boot"
[ "$FIRST" != "$SECOND" ] || fail "two boots gave the same random bytes: $FIRST"
healthy "$WORK/max"

grep -aE '^\[random\] ' "$WORK/log" | head -1
echo "$SEEDED"
echo "random bytes: $FIRST, then $SECOND"
echo "std's ground ($ARCH): the kernel's random numbers seeded with and without a random instruction, different on two boots; $CHECKS checks of getrandom, thread-local storage, signal stacks and SIGCHLD passed; a stack overflow caught on the signal stack"
