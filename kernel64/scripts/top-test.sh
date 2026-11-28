#!/bin/sh
# top-test.sh - `top` (TheTaaJ K141): the processes with their CPU,
# memory, threads and handles, refreshed.
#
#   usage: top-test.sh ISO DISK LOG
#
# One PC with two CPUs and a test disk: `forever spin &` keeps a CPU
# busy in the background, then `top -n 2` shows two frames. The busy
# program must be at the top, near 100% of a CPU; init and the shell
# must show the memory charged to them, their threads and handles.

set -u
ISO=$1
DISK=$2
LOG=$3
HERE=$(cd "$(dirname "$0")" && pwd)

fail() {
    echo "FAIL: $1 (log: $LOG)"
    exit 1
}
text() { tr -d '\r\f' < "$LOG"; }  # (a frame begins with a form feed: a clear screen)
has() { text | grep -aEq "$1" || fail "$2"; }
keys() { python3 -I "$HERE/key-names.py" "$1" || fail "no keys for: $1"; }

[ -f "$DISK" ] || fail "no test disk at $DISK"
echo "Booting x86_64 for top (log: $LOG)..."
DISK=$DISK SMP=2 KEYS="$(keys 'forever spin &') ret ~ $(keys 'top -n 2') ret ~~ $(keys 'top -x') ret ~" \
    "$HERE/qemu-boot.sh" "$ISO" "$LOG" 60 > /dev/null 2> "$LOG.err"

ROW='^ +[0-9]+  '
has '^top: [0-9]+ processes, [0-9]+ threads, [0-9]+ channels; memory [0-9]+ MiB used of [0-9]+; 2 CPUs$' "top did not show the system's line"
has '^  PID  NAME +CPU% +CPU TIME +MEMORY +THREADS +HANDLES$' "top did not show its columns"
[ "$(text | grep -ac '^  PID  NAME')" -eq 2 ] || fail "top -n 2 did not show two frames"
# The busy program first in each frame, near a whole CPU (over 80%).
text | grep -aA1 '^  PID  NAME' | grep -aE "${ROW}forever +(8[0-9]|9[0-9]|1[0-9][0-9])\.[0-9]%" | [ "$(wc -l)" -eq 2 ] \
    || fail "forever spin was not at the top of both frames, near 100% of a CPU"
has "${ROW}init +[0-9]+\.[0-9]% +[0-9]+\.[0-9]{3} s +[1-9][0-9]* KiB +[1-9][0-9]* +[1-9][0-9]*$" "init's row did not show its memory, threads and handles"
has "${ROW}shell +[0-9]+\.[0-9]% +[0-9]+\.[0-9]{3} s +[1-9][0-9]* KiB +[1-9][0-9]* +[1-9][0-9]*$" "the shell's row did not show its memory, threads and handles"
has '^usage: top \[-n TIMES\] \[-d SECONDS\]$' "top -x did not say how to use it"
text | grep -aEq 'KERNEL PANIC|CPU EXCEPTION|THE SYSTEM HAS STOPPED|^top: this program was not given' && fail "something failed"
echo "top: the busy program first, near 100%; memory, threads and handles for each"
