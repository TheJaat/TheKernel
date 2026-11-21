#!/bin/bash
# boot-slots.sh - runs a command while holding some of this machine's
# "boot slots", so that emulated boots started from anywhere - another
# make, another checkout of the sources - queue instead of piling up.
#
#   usage: boot-slots.sh UNITS COMMAND [ARGS...]
#
# Why: the boot tests time things. A kernel under test counts ticks,
# expects a sleep of 5 ticks to take 5 to 7, expects a killed program
# to be gone within 3. An emulator that is not given the host CPUs it
# needs runs slow and unevenly, and those checks fail for a reason that
# has nothing to do with the kernel.
#
# So a boot says how many host CPUs it can keep busy - UNITS: one for
# each emulated CPU - and waits here until that many slots are free.
#
# Environment:
#   TAAJ_BOOT_SLOTS  how many slots the machine has (default: its CPUs
#                    less one, at least one). A boot that asks for more
#                    than there are gets them all.
#   TAAJ_BOOT_LOCKS  where the lock files are (default /tmp/taaj-boot-slots
#                    - one place for every checkout, which is the point)
#
# How: a slot is a file, held with flock(1) for as long as this script
# has it open; the kernel lets go of it when the script ends, however
# it ends. Taking several slots one by one could deadlock - two boots
# of four each holding two - so slots are only ever *taken* by whoever
# holds one more lock, the gate. Giving them back needs no gate, so
# the holder of the gate is never waiting for someone who waits for it.

set -u

units=$1
shift

cpus=$(nproc 2>/dev/null || echo 1)
default=$((cpus > 1 ? cpus - 1 : 1))
slots=${TAAJ_BOOT_SLOTS:-$default}
[ "$slots" -ge 1 ] 2>/dev/null || slots=1
[ "$units" -ge 1 ] 2>/dev/null || units=1
[ "$units" -gt "$slots" ] && units=$slots

locks=${TAAJ_BOOT_LOCKS:-/tmp/taaj-boot-slots}
mkdir -p "$locks"
chmod a+rwx "$locks" 2>/dev/null

# The gate: blocks until nobody else is in the middle of taking slots.
exec {gate}> "$locks/gate"
flock "$gate"

held=0
waited=0
while [ "$held" -lt "$units" ]; do
    for n in $(seq 1 "$slots"); do
        [ "$held" -lt "$units" ] || break
        # Already ours? (A slot we hold would refuse a second flock from
        # a new descriptor, so remember which we have.)
        case " ${mine:-} " in *" $n "*) continue ;; esac
        exec {slot}> "$locks/slot-$n"
        if flock -n "$slot"; then
            mine="${mine:-} $n"
            held=$((held + 1))
        else
            exec {slot}>&-
        fi
    done
    if [ "$held" -lt "$units" ]; then
        sleep 0.2
        waited=$((waited + 1))
    fi
done

# Done taking: let the next one in. The slots stay held until exit.
exec {gate}>&-

[ "$waited" -gt 0 ] && [ -n "${TAAJ_BOOT_VERBOSE:-}" ] && echo "boot-slots: waited $((waited / 5)) s for $units of $slots slots" >&2

# The command inherits the open slot files, and so keeps the slots for
# exactly as long as it (or anything it starts) lives.
TAAJ_BOOT_HELD=1 "$@"
