#!/bin/sh
# aarch64-boot.sh - boots the AArch64 kernel image headless in QEMU's
# "virt" machine and stops as soon as the kernel is done, instead of
# waiting out a fixed timeout.
#
#   usage: aarch64-boot.sh IMAGE LOG [SECONDS]
#
# The kernel never exits, so "done" is read from its serial output: the
# idle line (or the line that says nothing more can run), a panic, or a
# CPU exception. SECONDS (default 20) is only the limit for a kernel
# that hangs silently.
#
# Environment:
#   MACHINE    QEMU machine options after "virt" (for example
#              ",virtualization=on" to be entered at exception level 2)
#   CPU        the CPU model (default cortex-a72)
#   MEMORY     how much memory (default 512M)
#   QEMU_ARGS  any further QEMU options
#   KEYS       keys to type at the serial port once its driver is ready,
#              by the names QEMU's monitor uses ("shift-h i ret") - the
#              list the PC's boot test types, typed here as the bytes a
#              terminal sends (serial-type.py, which also says what "/"
#              and "~" in the list mean)

set -u

# Boots are timed, so no more of them may run at once than the host has
# CPUs for: wait for a slot first (boot-slots.sh, which then runs this
# script again). The kernel uses one CPU so far, whatever the machine has.
if [ -z "${TAAJ_BOOT_HELD:-}" ]; then
    exec "$(dirname "$0")/boot-slots.sh" 1 "$0" "$@"
fi

IMAGE=$1
LOG=$2
LIMIT=${3:-20}

FINISHED="\[kernel\] idle|\[kernel\] no programs can run|KERNEL PANIC|CPU EXCEPTION|KERNEL STACK OVERFLOW|THE SYSTEM HAS STOPPED"
KEYS=${KEYS:-}

rm -f "$LOG"
: > "$LOG"
# -kernel with a flat image that has the AArch64 header: QEMU loads it
# as a loader on a real board would, and passes the device tree in x0.
if [ -n "$KEYS" ]; then
    # The serial port both ways: out to the log, in from a pipe this
    # script keeps open (QEMU would see the end of input otherwise, as
    # soon as the first writer closed it).
    FIFO="$LOG.keys"
    rm -f "$FIFO"
    mkfifo "$FIFO"
    exec 9<> "$FIFO"
    qemu-system-aarch64 -M "virt${MACHINE:-}" -cpu "${CPU:-cortex-a72}" -m "${MEMORY:-512M}" \
        -display none -no-reboot -serial stdio -monitor none -kernel "$IMAGE" ${QEMU_ARGS:-} < "$FIFO" > "$LOG" 2> "$LOG.qemu" &
else
    qemu-system-aarch64 -M "virt${MACHINE:-}" -cpu "${CPU:-cortex-a72}" -m "${MEMORY:-512M}" \
        -display none -no-reboot -serial "file:$LOG" -kernel "$IMAGE" ${QEMU_ARGS:-} 2> "$LOG.qemu" &
fi
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

# The driver is a process that starts after the kernel goes idle: type
# only once it says it is ready.
if [ -n "$KEYS" ] && grep -aq "\[kernel\] idle" "$LOG"; then
    ticks=0
    until grep -aq "\[serial\] serial port ready" "$LOG" || [ $ticks -ge 50 ]; do
        sleep 0.1
        ticks=$((ticks + 1))
    done
    if [ $ticks -lt 50 ]; then
        "$(dirname "$0")/serial-type.py" "$LOG" "$FIFO" $KEYS
    else
        echo "aarch64-boot: the serial driver never said it was ready - nothing typed" >&2
    fi
fi

kill $qemu 2>/dev/null
wait $qemu 2>/dev/null
[ -n "$KEYS" ] && { exec 9<&-; rm -f "$FIFO"; }
# QEMU's own remarks: only "terminating on signal 15" is expected.
grep -v "terminating on signal" "$LOG.qemu" >&2

if ! grep -aEq "$FINISHED" "$LOG"; then
    echo "aarch64-boot: the kernel did not finish within $LIMIT seconds (log: $LOG)" >&2
    exit 1
fi
exit 0
