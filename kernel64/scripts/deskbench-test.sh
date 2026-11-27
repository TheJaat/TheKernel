#!/bin/sh
# deskbench-test.sh - the desktop benchmark (K114): what the window
# server costs, measured where the screen is drawn.
#
#   usage: deskbench-test.sh IMAGE DISK LOG
#
# Two boots of the PC, with 1 CPU and with 4 (LOG.1 and LOG.4), each:
#
#   display bench        one composition of the whole screen, and the
#                        terminal dragged a pixel at a time: frames/s
#   forever spin &  x4   four busy programs, at the shell's priority
#   display keys 20      the next 20 keys timed, from their arrival at
#                        the display to the screen showing their echo
#   a ... t              twenty letters at the prompt
#   kill 1 .. kill 4     the busy programs ended
#
# The display prints each measurement as one line; this test checks
# that each was printed - not how fast: a busy host and an emulated CPU
# are slower, and a benchmark that fails on a slow machine is a flaky
# test (entry 105). The numbers go in the journal (entry 147), and are
# printed here.

set -u
IMAGE=$1
DISK=$2
LOG=$3
HERE=$(dirname "$0")

fail() {
    echo "FAIL: $1 (log: $2)"
    exit 1
}

keys() {
    echo "$1" | sed -e 's/ /_/g' -e 's/./& /g' -e 's/_/spc/g'
}

steps="$(keys 'display bench') ret see:frames/s"
for job in 1 2 3 4; do
    steps="$steps $(keys 'forever spin') spc shift-7 ret"
done
steps="$steps ~ $(keys 'display keys 20') ret see:timing,the,next,20,keys"
steps="$steps a b c d e f g h i j k l m n o p q r s t see:key,to,screen:,20,keys ret ~"
for job in 1 2 3 4; do
    steps="$steps $(keys "kill $job") ret ~"
done

for cpus in 1 4; do
    log="$LOG.$cpus"
    echo "Booting x86_64 with $cpus CPU(s) for the desktop benchmark (log: $log)..."
    DISK="$DISK" SMP=$cpus KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$log" 120

    grep -aEq '\[display\] one composition of the [0-9]+x[0-9]+ screen: median [0-9]+ us' "$log" \
        || fail "$cpus CPU(s): the display did not time a composition" "$log"
    grep -aEq '\[display\] dragging a window a pixel at a time: 200 frames in [0-9]+ ms, [0-9]+ frames/s' "$log" \
        || fail "$cpus CPU(s): the display did not time the drag" "$log"
    grep -aEq '\[display\] window 1 moved to \(19, 80\)' "$log" \
        || fail "$cpus CPU(s): the dragged terminal did not end where it was" "$log"
    grep -aEq '\[display\] key to screen: 20 keys, median [0-9]+ us, worst [0-9]+ us' "$log" \
        || fail "$cpus CPU(s): the display did not time 20 keys to the screen" "$log"
    [ "$(grep -ac 'stopped: killed by request' "$log")" -ge 4 ] \
        || fail "$cpus CPU(s): the four busy programs were not all killed" "$log"
    grep -a 'has not answered for' "$log" && fail "$cpus CPU(s): a service stopped answering" "$log"

    echo "  $cpus CPU(s):"
    grep -aoE '\[display\] (one composition|dragging|key to screen: 20).*' "$log" | sed 's/^\[display\] /    /'
done
exit 0
