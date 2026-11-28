#!/bin/sh
# debug-test.sh - the debugger (TheTaaJ K142): `debug` starts a program,
# and so holds its handle with the DEBUG right; it stops it, reads its
# registers and memory, sets a breakpoint where it runs, catches it
# there, and lets it go on past it.
#
#   usage: debug-test.sh ISO DISK LOG
#
# One PC with two CPUs and a test disk; `debug forever spin`, then:
#   stop            stopped as asked, at the loop's address (A)
#   regs            its registers
#   x 400000 16     the first 16 bytes of its code
#   break pc        a breakpoint at A
#   cont            it reaches the breakpoint at once (it loops there)
#   wait            stopped at a breakpoint, at A; regs: pc A+1 (after int3)
#   cont            the original byte back, pc at A: it goes on
#   stop            stopped as asked again - still looping, not crashed
#   quit            it ends, killed by request

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
echo "Booting x86_64 for the debugger (log: $LOG)..."
DISK=$DISK SMP=2 KEYS="$(keys 'debug forever spin') ret see:debug:,started \
$(keys 'stop') ret see:stopped,as $(keys 'regs') ret see:flags $(keys 'x 400000 16') ret see:0x400000: \
$(keys 'break pc') ret see:a,breakpoint,at $(keys 'cont') ret see:going $(keys 'wait') ret see:at,a,breakpoint \
$(keys 'regs') ret see:thread,1: $(keys 'cont') ret see:going,on $(keys 'stop') ret see:thread,2 $(keys 'quit') ret ~" \
    "$HERE/qemu-boot.sh" "$ISO" "$LOG" 60 > /dev/null 2> "$LOG.err"

has '^debug: started forever; commands: ' "debug did not start forever"
A=$(text | sed -n 's/^debug: thread 0 of pid [0-9]* stopped as asked, at \(0x[0-9a-f]*\)$/\1/p' | head -1)
[ -n "$A" ] || fail "stop did not stop forever, as asked"
has "^debug: thread 0: pc $A  sp 0x[0-9a-f]+  flags 0x[0-9a-f]+$" "regs did not give the stopped thread's pc ($A), sp and flags"
has '^ rax 0x[0-9a-f]{16}   rbx 0x[0-9a-f]{16}' "regs did not give the general registers"
has '^0x400000:( [0-9a-f]{2}){16}$' "x did not read 16 bytes of its code"
has "^debug: a breakpoint at $A$" "break pc did not set a breakpoint at $A"
has "^debug: thread 1 of pid [0-9]+ stopped at a breakpoint, at $A$" "forever did not stop at the breakpoint at $A"
B=$(printf '0x%x' $((A + 1)))
has "^debug: thread 1: pc $B  " "at the breakpoint, its pc was not just after the int3 ($B)"
has '^debug: thread 2 of pid [0-9]+ stopped as asked, at 0x[0-9a-f]+$' "after the breakpoint, forever did not go on (a second stop)"
has '^\[process\] [0-9]+ "forever" stopped: killed by request$' "quit did not end forever"
text | grep -aEq '"forever" killed: |KERNEL PANIC|CPU EXCEPTION|THE SYSTEM HAS STOPPED|^debug: [A-Za-z]+$' && fail "something failed"
echo "the debugger: stop, registers, memory, a breakpoint caught and gone past"
