#!/bin/sh
# basics-test.sh - the core basics (journal entry 097): the date from
# the real-time clock, a clock finer than a tick, signals from the
# kernel, and writes that reach the disk (flushes).
#
#   usage: basics-test.sh ARCH IMAGE DISK LOG BASICS
#
# BASICS is basics.c (next to this script) built for ARCH. TheTaaJ runs:
#
#   basics date              the date: it must be the host's, to a minute
#   with programs basics     the checks ("basics: N checks, all passed"):
#                            the clock, handlers run by raise and by
#                            alarms - while computing, while reading,
#                            in pause, sigsuspend and nanosleep - blocked
#                            signals, SIGSEGV caught, children ended by
#                            signals or catching them, abort, a file
#                            written, fsync'd and read back
#   basics catch, Ctrl+C     a program waiting for a line catches SIGINT
#   with files basics speed  4 MiB in 64 commits, each with two flushes
#   uptime                   the shell's: still running fine

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
BASICS=$5
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
"$TFSTOOL" put "$DISK" basics "$BASICS" > /dev/null || fail "cannot put basics on the disk"

steps="$(keys 'echo == date ==') ret ~ $(keys 'basics date') ret ~"
steps="$steps $(keys 'echo == checks ==') ret ~ $(keys 'with programs basics') ret ~~"
steps="$steps $(keys 'echo == catch ==') ret ~ $(keys 'basics catch') ret see:interrupt) ctrl-c ~"
steps="$steps $(keys 'echo == speed ==') ret ~ $(keys 'with files basics speed') ret ~~"
steps="$steps $(keys 'echo == end ==') ret ~ $(keys 'echo done') ret ~"

# The host's time before the boot and after it: TheTaaJ's date, read
# during the boot, must lie between them. Not "within 300 s of the
# start": boot-slots.sh can make a boot wait minutes for a CPU before
# QEMU starts (6.7 min on 2026-11-27, two boots run beside a race hunt),
# and that wait is not a wrong clock (K193's entry, bug 11).
HOST_DATE=$(date -u +%s)
echo "Booting $ARCH to check the date, the clock, signals and flushes (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" QEMU_ARGS="-drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 60
else
    DISK="$DISK" KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 60
fi
HOST_END=$(date -u +%s)

tr -d '\r' < "$LOG" > "$WORK/log"
part() { # part NAME: the lines after "== NAME ==", up to the next mark
    awk -v start="== $1 ==" '$0 == start { on = 1; next } /^== [a-z-]+ ==$/ { on = 0 } on' "$WORK/log"
}
has() { # has NAME PATTERN WHY
    part "$1" | grep -aEq "$2" || fail "$3"
}

grep -aq '^== end ==$' "$WORK/log" || fail "the steps did not all run"
grep -aEq '^\[clock\] .* says [0-9]{4}-[0-9]{2}-[0-9]{2} [0-9:]{8} UTC; time is counted from here at [0-9]+ Hz' "$WORK/log" \
    || fail "the kernel did not read the real-time clock"
# The date TheTaaJ says against the host's: QEMU's clock is the host's,
# so it lies between the host's time before the boot and after it, give
# or take the RTC's whole seconds (5 s either side).
SAID=$(part date | grep -aEo '[0-9]{2} [A-Z][a-z]+ [0-9]{4}, [0-9:]{8} UTC' | head -1)
[ -n "$SAID" ] || fail "basics date said no date"
SAID_SECONDS=$(date -u -d "$(echo "$SAID" | sed 's/,//; s/ UTC//')" +%s) || fail "cannot read the date: $SAID"
APART=$((SAID_SECONDS - HOST_DATE))
[ "$SAID_SECONDS" -ge $((HOST_DATE - 5)) ] && [ "$SAID_SECONDS" -le $((HOST_END + 5)) ] \
    || fail "TheTaaJ's date ($SAID) is not within the test's time: $APART s after its start, which was $((HOST_END - HOST_DATE)) s before its end"

part checks | grep -a '^basics: failed' | head -20
# Every check(...) in basics.c but the function itself.
CHECKS=$(($(grep -c 'check(' "$HERE/basics.c") - 1))
has checks "^basics: $CHECKS checks, all passed$" "the $CHECKS checks in basics.c did not all pass"
has checks '"basics" exited with code 0' "basics did not exit with code 0"
has catch '^basics: caught SIGINT while waiting for a line' "Ctrl+C did not reach the handler of a program waiting for a line"
has catch '"basics" exited with code 0' "basics catch did not exit as its handler chose"
grep -aq '^\[blk\] virtio disk .*, a write cache (flushed on commit)' "$WORK/log" || fail "the disk's write cache was not taken: no flushes"
has speed '^basics: 4 MiB in 64 writes took [0-9]+ ms' "basics speed did not write its 4 MiB"
grep -aq 'not reached' "$WORK/log" && fail "$(grep -a 'not reached' "$WORK/log" | head -1)"
grep -a 'has not answered for' "$WORK/log" && fail "a service stopped answering during the test"
grep -aiE 'panic|page fault in kernel|CPU EXCEPTION' "$WORK/log" && fail "the kernel failed"

grep -aE '^\[clock\] ' "$WORK/log" | head -1
part date | grep -a 'UTC'
part speed | grep -a '^basics: 4 MiB'
echo "the core basics ($ARCH): the date read at boot is the host's ($APART s apart); $CHECKS checks of the clock, signals and fsync passed; Ctrl+C reached a waiting program's handler; 4 MiB written with flushes"
