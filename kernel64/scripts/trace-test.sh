#!/bin/sh
# trace-test.sh - tracing (TheTaaJ K139): the kernel's trace points on
# every CPU, recorded by `trace` for a span and shown.
#
#   usage: trace-test.sh ISO DISK LOG
#
# One PC with two CPUs and a test disk: `trace -s 2` in the background,
# and meanwhile `ls` - a program started, files read through fs, its
# pages filled. Every kind of event must be there - system calls, sends
# and receives, switches, faults, interrupts - on both CPUs, none lost,
# and the last events shown with their time, CPU, process and value.

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
echo "Booting x86_64 for tracing (log: $LOG)..."
DISK=$DISK SMP=2 KEYS="$(keys 'trace -s 2 -e 12 &') ret ~ $(keys 'ls') ret ~ see:the,last $(keys 'trace -x') ret ~" \
    "$HERE/qemu-boot.sh" "$ISO" "$LOG" 60 > /dev/null 2> "$LOG.err"

has '^\[kernel\] tracing: 2 rings of [0-9]+ events, handed to init$' "the kernel did not hand tracing to init"
has '^trace: [12]\.[0-9]{2} s on 2 CPUs: [0-9]+ events \(0 lost\)$' "trace did not record two seconds on both CPUs, none lost"
# Every kind, more than none.
KINDS=$(text | grep -a '^  by kind:') || fail "trace did not count the events by kind"
for k in syscall send receive switch fault interrupt sample; do
    echo "$KINDS" | grep -Eq "  $k [1-9][0-9]*( |$)" || fail "no $k events: $KINDS"
done
has '^  by CPU:  cpu0 [1-9][0-9]*  cpu1 [1-9][0-9]*$' "trace did not see events on both CPUs"
has '^ +[0-9]+ kernel +[0-9]+$' "trace did not count the kernel's own events"
has '^  the last 12 events \(ms from the start, CPU, process, what, value\):$' "trace did not show the last events"
[ "$(text | grep -acE '^ +[0-9]+\.[0-9]{3}  cpu[01] +[0-9]+ [^ ]+ +(syscall|send|receive|switch|fault|interrupt|sample) +0x[0-9a-f]+$')" -eq 12 ] \
    || fail "the last events did not each have their time, CPU, process, kind and value"
has '^usage: trace \[-s SECONDS\] \[-e EVENTS\]$' "trace -x did not say how to use it"
text | grep -aEq 'KERNEL PANIC|CPU EXCEPTION|THE SYSTEM HAS STOPPED|^trace: (this program was not|the handle is not)' && fail "something failed"
echo "tracing: every kind of event, on both CPUs, none lost"
