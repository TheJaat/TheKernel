#!/bin/sh
# aarch64-boot.sh - boots the AArch64 kernel image headless in QEMU's
# "virt" machine and stops as soon as the kernel is done, instead of
# waiting out a fixed timeout.
#
#   usage: aarch64-boot.sh IMAGE LOG [SECONDS]
#
# The kernel never exits, so "done" is read from its serial output: its
# last line, a panic, or a CPU exception. SECONDS (default 20) is only
# the limit for a kernel that hangs silently.
#
# Environment:
#   MACHINE    QEMU machine options after "virt" (for example
#              ",virtualization=on" to be entered at exception level 2)
#   CPU        the CPU model (default cortex-a72)
#   MEMORY     how much memory (default 512M)
#   QEMU_ARGS  any further QEMU options

set -u

IMAGE=$1
LOG=$2
LIMIT=${3:-20}

FINISHED="\[kernel\] this is as far as|KERNEL PANIC|CPU EXCEPTION|KERNEL STACK OVERFLOW"

rm -f "$LOG"
: > "$LOG"
# -kernel with a flat image that has the AArch64 header: QEMU loads it
# as a loader on a real board would, and passes the device tree in x0.
qemu-system-aarch64 -M "virt${MACHINE:-}" -cpu "${CPU:-cortex-a72}" -m "${MEMORY:-512M}" \
    -display none -no-reboot -serial "file:$LOG" -kernel "$IMAGE" ${QEMU_ARGS:-} 2> "$LOG.qemu" &
qemu=$!

# Poll ten times a second: the boot is over when one of the patterns
# appears, QEMU has gone, or the limit is reached.
ticks=0
while [ $ticks -lt $((LIMIT * 10)) ]; do
    if grep -aEq "$FINISHED" "$LOG" 2>/dev/null; then
        # The line that ended the boot may be followed by more (a
        # register dump): give it a moment to arrive.
        sleep 0.3
        break
    fi
    kill -0 $qemu 2>/dev/null || break
    sleep 0.1
    ticks=$((ticks + 1))
done

kill $qemu 2>/dev/null
wait $qemu 2>/dev/null
# QEMU's own remarks: only "terminating on signal 15" is expected.
grep -v "terminating on signal" "$LOG.qemu" >&2

if ! grep -aEq "$FINISHED" "$LOG"; then
    echo "aarch64-boot: the kernel did not finish within $LIMIT seconds (log: $LOG)" >&2
    exit 1
fi
exit 0
