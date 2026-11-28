#!/bin/sh
# profile-test.sh - the profiler (TheTaaJ K140): the timer's samples of
# where each CPU is, counted by `profile` - the kernel's functions by
# name, programs by name and address.
#
#   usage: profile-test.sh ISO DISK LOG
#
# One PC with two CPUs and a test disk: `forever spin &` keeps one CPU
# busy, then `profile -s 2`. The busy program must have about half the
# time (one CPU of two), by program and at its place; the kernel's places
# must be named by its symbols (the idle loop, the other CPU's half).

set -u
ISO=$1
DISK=$2
LOG=$3
HERE=$(cd "$(dirname "$0")" && pwd)

fail() {
    echo "FAIL: $1 (log: $LOG)"
    exit 1
}
text() { tr -d '\r' < "$LOG"; }
has() { text | grep -aEq "$1" || fail "$2"; }
keys() { python3 -I "$HERE/key-names.py" "$1" || fail "no keys for: $1"; }

[ -f "$DISK" ] || fail "no test disk at $DISK"
echo "Booting x86_64 for the profiler (log: $LOG)..."
DISK=$DISK SMP=2 KEYS="$(keys 'forever spin &') ret ~ $(keys 'profile -s 2 -n 6') ret ~~ $(keys 'profile -x') ret ~" \
    "$HERE/qemu-boot.sh" "$ISO" "$LOG" 60 > /dev/null 2> "$LOG.err"

has '^profile: [12]\.[0-9]{2} s on 2 CPUs: [1-9][0-9]+ samples \(0 lost\)$' "profile did not sample two seconds on both CPUs, with the kernel's symbols, none lost"
# About half the time: one CPU of two (40% to 60%).
HALF='([45][0-9]|60)\.[0-9]%'
has "^ +$HALF +[0-9]+ ms +[0-9]+ forever$" "forever did not have about half the time by program"
has "^ +$HALF +[0-9]+ ms  forever \([0-9]+\)  at 0x[0-9a-f]+$" "forever's place (its address) did not have about half the time"
has '^ +[0-9]+\.[0-9]% +[0-9]+ ms  kernel  kernel64::threads::idle_loop$' "the kernel's idle loop was not named by its symbols"
has '^usage: profile \[-s SECONDS\] \[-n PLACES\]$' "profile -x did not say how to use it"
text | grep -aEq 'KERNEL PANIC|CPU EXCEPTION|THE SYSTEM HAS STOPPED|^profile: (this program|the handle)' && fail "something failed"
echo "the profiler: the busy program about half the time, the kernel's functions by name"
