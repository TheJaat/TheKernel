#!/bin/sh
# qemu-boot.sh - boots an ISO headless and stops as soon as the kernel is
# done, instead of waiting out a fixed timeout.
#
#   usage: qemu-boot.sh ISO LOG [SECONDS]
#
# The kernel never exits, so "done" is read from its serial output: the
# idle line, a panic, or a CPU exception. SECONDS (default 20) is only
# the limit for a kernel that hangs silently.
#
# Environment:
#   TAAJ_ACCEL kvm or tcg (default tcg): the Makefile sets it, kvm when
#              /dev/kvm can be opened
#   QEMU_ARGS  extra QEMU options (for example the firmware)
#   KEYS       QEMU key names to type once the keyboard driver is ready,
#              through QEMU's monitor (`sendkey`) - e.g. "shift-h i ret".
#              A "/" waits until a new keyboard driver is ready (after a
#              crash and restart) before typing on. A "~" waits until
#              the shell has run every line typed so far - for ten
#              seconds at most; "~~" waits up to ninety, for a line that
#              is meant to take a while. The script then waits
#              for the shell to echo a line. A token starting with "!"
#              is a monitor command, its commas standing for spaces:
#              "!mouse_move,100,50" moves the PS/2 mouse, and
#              "!mouse_button,1" holds its left button down (0 lets go).
#              "see:TEXT" waits (ten seconds at most) until TEXT is in
#              the log, its commas standing for spaces - for something
#              a program says when it is ready for the next key.
#   DISK       a raw disk image, attached as a virtio block device
#   SMP        how many CPUs the machine has (default 4)
#   SLOTS      how many host CPUs the boot keeps busy (default: SMP). A
#              boot that reads a lot from its disk asks for one more:
#              QEMU does the disk's work in a thread of its own, and a
#              disk driver kept waiting is stopped by init's watchdog

set -u

# Boots are timed, so no more of them may run at once than the host has
# CPUs for: wait for a slot first (boot-slots.sh, which then runs this
# script again). One slot for each CPU of the machine, unless SLOTS says.
if [ -z "${TAAJ_BOOT_HELD:-}" ]; then
    exec "$(dirname "$0")/boot-slots.sh" "${SLOTS:-${SMP:-4}}" "$0" "$@"
fi

ISO=$1
LOG=$2
LIMIT=${3:-20}
KEYS=${KEYS:-}
DISK=${DISK:-}
DISK_ARGS=
if [ -n "$DISK" ]; then
    DISK_ARGS="-drive file=$DISK,format=raw,if=none,id=disk0 -device virtio-blk-pci,drive=disk0"
fi

FINISHED="\[kernel\] idle|KERNEL PANIC|CPU EXCEPTION|THE SYSTEM HAS STOPPED"
# FINISHED_ONLY: wait for this instead (a boot that goes idle and then
# does something more).
FINISHED=${FINISHED_ONLY:-$FINISHED}
MONITOR="$LOG.monitor"

start_qemu() {
    rm -f "$LOG" "$LOG.ppm" "$LOG.hang.ppm" "$LOG.qemu" "$MONITOR"
    : > "$LOG"
    # TAAJ_ACCEL: kvm or tcg, worked out by TheTaaJ's Makefile (KVM when
    # /dev/kvm can be opened); emulated when this script runs on its own.
    qemu-system-x86_64 -accel "${TAAJ_ACCEL:-tcg}" -m 512M -cdrom "$ISO" -boot d \
        -smp "${SMP:-4}" -display none -no-reboot -serial "file:$LOG" \
        -monitor "unix:$MONITOR,server,nowait" \
        -d cpu_reset -D "$LOG.qemu" $DISK_ARGS ${QEMU_ARGS:-} &
    qemu=$!
}

# True if QEMU has gone and the kernel never ran: not a byte on the
# serial line, and the boot CPU reset only the two times every start
# has (power-on, and the firmware's own reset). With several CPUs QEMU
# now and then ends like this while the firmware is starting them -
# before the loader, let alone the kernel. That is not a result about
# the kernel, so such a start is done again. A kernel that resets the
# machine before its first word resets the boot CPU a third time, and
# is NOT retried.
died_in_firmware() {
    kill -0 $qemu 2>/dev/null && return 1
    [ -s "$LOG" ] && return 1
    [ "$(grep -c "CPU Reset (CPU 0)" "$LOG.qemu" 2>/dev/null)" -le 2 ]
}

start_qemu

# Polls (ten times a second) until PATTERN appears in the log or the
# time runs out; true if it appeared.
wait_for() {
    pattern=$1
    ticks=$2
    while [ "$ticks" -gt 0 ]; do
        grep -qE "$pattern" "$LOG" && return 0
        kill -0 $qemu 2>/dev/null || return 1   # QEMU quit (a triple fault resets; -no-reboot quits)
        sleep 0.1
        ticks=$((ticks - 1))
    done
    return 1
}

wait_for "$FINISHED" $((LIMIT * 10))
finished=$?
attempt=1
while [ $finished -ne 0 ] && [ $attempt -lt 3 ] && died_in_firmware; do
    echo "qemu-boot: QEMU ended in the firmware, before the kernel ran - starting it again" >&2
    wait $qemu 2>/dev/null
    attempt=$((attempt + 1))
    start_qemu
    wait_for "$FINISHED" $((LIMIT * 10))
    finished=$?
done

if [ $finished -eq 0 ]; then
    # The driver is a process that starts after the kernel goes idle, and
    # it flushes the controller as it sets it up: type only once it is
    # ready, or the first keys are lost.
    if [ -n "$KEYS" ] && grep -q "\[kernel\] idle" "$LOG" && wait_for "\[kbd\] PS/2 keyboard ready" 50; then
        # One key at a time, with a pause: a real typist, not a burst.
        ready="\[kbd\] PS/2 keyboard ready"
        entered=0
        for key in $KEYS; do
            if [ "$key" = "/" ]; then
                # Wait for one more "ready" line than there is now.
                seen=$(grep -c "$ready" "$LOG")
                ticks=100
                while [ "$(grep -c "$ready" "$LOG")" -le "$seen" ] && [ $ticks -gt 0 ]; do
                    sleep 0.1
                    ticks=$((ticks - 1))
                done
                continue
            fi
            if [ "$key" = "~" ] || [ "$key" = "~~" ]; then
                # "~~" is for a line that is meant to take a while.
                ticks=100
                [ "$key" = "~~" ] && ticks=900
                while [ "$(grep -c '\[shell\] ran' "$LOG")" -lt "$entered" ] && [ $ticks -gt 0 ]; do
                    sleep 0.1
                    ticks=$((ticks - 1))
                done
                continue
            fi
            case $key in
                see:*)
                    want=$(echo "${key#see:}" | tr ',' ' ')
                    ticks=100
                    until grep -aqF -- "$want" "$LOG" || [ $ticks -le 0 ]; do
                        sleep 0.1
                        ticks=$((ticks - 1))
                    done
                    [ $ticks -gt 0 ] || echo "qemu-boot: never saw \"$want\"" >&2
                    continue
                    ;;
                !*)
                    # Not a key: a command for the monitor. A pause after
                    # it, so the mouse's packet is read before the next.
                    echo "${key#!}" | tr ',' ' ' | nc -U -N "$MONITOR" > /dev/null
                    sleep 0.2
                    continue
                    ;;
            esac
            [ "$key" = "ret" ] && entered=$((entered + 1))
            echo "sendkey $key" | nc -U -N "$MONITOR" > /dev/null
            sleep 0.05
        done
        # Each Enter runs a command; wait until the shell has run them all.
        commands=$(echo "$KEYS" | tr ' ' '\n' | grep -c '^ret$')
        ticks=100
        while [ "$(grep -c '\[shell\] ran' "$LOG")" -lt "$commands" ] && [ $ticks -gt 0 ]; do
            sleep 0.1
            ticks=$((ticks - 1))
        done
        [ $ticks -gt 0 ] || echo "qemu-boot: the shell did not run all $commands commands" >&2
    fi
    sleep 0.3   # let the rest of a fault report reach the log
    # A picture of the screen, for check-boot.sh to look at.
    echo "screendump $LOG.ppm" | nc -U -N "$MONITOR" > /dev/null
    sleep 0.3
else
    if kill -0 $qemu 2>/dev/null; then
        echo "qemu-boot: no end of output after ${LIMIT}s - killed" >&2
        # What was on the screen: the BIOS loader reports only there.
        echo "screendump $LOG.hang.ppm" | nc -U -N "$MONITOR" > /dev/null
        sleep 0.3
    else
        # -no-reboot: a triple fault resets the CPU, and QEMU quits.
        # $LOG.qemu holds the CPU state at each reset (-d cpu_reset).
        echo "qemu-boot: QEMU quit before the kernel finished - a triple fault? (CPU state in $LOG.qemu)" >&2
    fi
fi

kill $qemu 2>/dev/null
wait $qemu 2>/dev/null
rm -f "$MONITOR"
exit 0
