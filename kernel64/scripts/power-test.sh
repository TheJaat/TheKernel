#!/bin/sh
# power-test.sh - `poweroff` and `reboot`: init stops the services, in
# order, and `power` switches the machine off or resets it - through
# ACPI's ports on a PC, through PSCI on AArch64.
#
#   usage: power-test.sh x86 ISO DISK LOG
#          power-test.sh aarch64 IMAGE LOG
#
# x86: five machines, booted at once.
#   pc-off      QEMU's PC (BIOS) with a virtio disk: a file is written,
#               then `poweroff`. QEMU must end by itself (QEMU's own
#               "terminating on signal" line is what the boot script's
#               kill leaves; a machine that switched off has none), and
#               the file must be on the disk, read by the host.
#   pc-reboot   `reboot`, with QEMU told to reset rather than quit on a
#               reset (-action reboot=reset): the kernel must start a
#               second time and get as far as the shell.
#   pc-none     no ACPI tables at all (acpi=off): the kernel hands no
#               power control on, `poweroff` says it cannot, and the
#               system goes on as before.
#   q35-off, q35-reboot   the q35 (UEFI firmware), whose FADT names a
#               reset register of its own.
# aarch64: two machines, `poweroff` and `reboot`, through PSCI.
#
# DISK is a test disk (make-test-disk); OVMF, in the environment, the
# UEFI firmware.

set -u
MODE=$1
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}

fail() {
    echo "FAIL: $1"
    exit 1
}

keys() {
    python3 -I "$HERE/key-names.py" "$1" || fail "no keys for: $1"
}

# text LOG: the log without the serial line's \r.
text() { tr -d '\r' < "$1"; }
has() { # has LOG PATTERN WHY
    text "$1" | grep -aEq "$2" || fail "$1: $3"
}
count() { # count LOG PATTERN: how many lines match
    text "$1" | grep -acE "$2"
}
# The services stopped in order: the shell first, the file service
# after it, the disk driver after that (if there is one), the registry
# last - and then power's line.
stopped_in_order() { # stopped_in_order LOG DRIVER
    order=$(text "$1" | grep -aE '^\[init\] stopped [a-z]+' | sed 's/^\[init\] stopped \([a-z]*\).*/\1/' | tr '\n' ' ')
    case "$order" in
        "shell "*"fs ${2:+$2 }"*"registry ") ;;
        *) fail "$1: the services were not stopped shell first, then fs${2:+, then $2}, the registry last: $order" ;;
    esac
    text "$1" | grep -aE '^\[init\] stopped .*(did not)' && fail "$1: a service was not stopped cleanly"
    return 0
}

if [ "$MODE" = x86 ]; then
    ISO=$2
    DISK=$3
    LOG=$4
    OVMF=${OVMF:?the UEFI firmware}
    [ -f "$DISK" ] || fail "no test disk at $DISK"
    cp "$DISK" "$DISK.off" || fail "cannot copy the disk"
    echo "Booting x86_64 five times to switch it off and reset it: a PC and a q35, and a PC without ACPI (logs: $LOG.*)..."
    boot() { # boot NAME LIMIT KEYS QEMU_ARGS [DISK]
        DISK=${5:-} KEYS="$3" QEMU_ARGS="$4" SMP=2 SLOTS=2 "$HERE/qemu-boot.sh" "$ISO" "$LOG.$1" "$2" 2> "$LOG.$1.err" > /dev/null
    }
    Q35="-machine q35 -bios $OVMF"
    boot pc-off 60 "$(keys 'write saved before-off') ret ~ $(keys 'poweroff') ret" "" "$DISK.off" &
    # "/": wait for the second boot's keyboard driver - the boot script
    # would otherwise end QEMU before the machine has started again.
    boot pc-reboot 60 "$(keys 'reboot') ret /" "-action reboot=reset" &
    boot pc-none 40 "$(keys 'poweroff') ret ~ $(keys 'echo still here') ret ~" "-machine pc,acpi=off" &
    # The q35's AHCI controller has no disk: its driver ends at once, five
    # times, and init gives up on it (checked below). The shell is ready
    # before that - typed at once, under load, `poweroff` came first, the
    # services were stopped, and the give-up line never came: "ahci did
    # not give up" (the second night, entry NNN). So: that line first.
    GAVE_UP="see:ahci,ended,(code,5),5,times,in,30,s,-,giving,up,on,it"
    boot q35-off 90 "$GAVE_UP $(keys 'poweroff') ret" "$Q35" &
    boot q35-reboot 120 "$GAVE_UP $(keys 'reboot') ret /" "$Q35 -action reboot=reset" &
    wait

    # The kernel's part: what it found, and handed on.
    for machine in pc-off pc-reboot; do
        has "$LOG.$machine" '^\[acpi\] power: PM1a control 0x604, reset 0xcf9 <- 0x6 \(the PC.s; the FADT names none\), DSDT [0-9]+ bytes at 0x[0-9a-f]+ - handed to init$' \
            "the kernel did not hand on the PC's PM1a control, the DSDT and 0xCF9"
    done
    for machine in q35-off q35-reboot; do
        has "$LOG.$machine" '^\[acpi\] power: PM1a control 0x604, reset 0xcf9 <- 0xf, DSDT [0-9]+ bytes at 0x[0-9a-f]+ - handed to init$' \
            "the kernel did not hand on the q35's PM1a control, the DSDT and the FADT's own reset register"
    done
    for machine in pc-off pc-reboot q35-off q35-reboot; do
        has "$LOG.$machine" '^\[init\] started: .* power; supervising$' "init did not start power"
        has "$LOG.$machine" '^\[power\] ACPI: off is sleep type 0/0 \(\\_S5\); reset is 0x[6f] into port 0xcf9; ready$' \
            "power did not read \\_S5 from the DSDT"
        text "$LOG.$machine" | grep -aEq 'KERNEL PANIC|CPU EXCEPTION|THE SYSTEM HAS STOPPED|\[init\] could not' && fail "$LOG.$machine: something failed"
    done

    # The q35's own AHCI controller has no disk, only the CD: its driver
    # gives up, and fs must not wait for it for ever - init closes the
    # disk's channels, and fs goes on with files in memory.
    for machine in q35-off q35-reboot; do
        has "$LOG.$machine" '^\[init\] ahci ended \(code 5\) 5 times in 30 s - giving up on it$' "ahci did not give up on a controller with no disk"
        text "$LOG.$machine" | grep -aA3 'giving up on it' | grep -aq '^\[fs\] no disk (no disk (or its driver stopped)): files in memory only$' \
            || fail "$LOG.$machine: fs did not go on without the disk driver that gave up"
    done

    # Off: the services stopped, then S5, and QEMU ended by itself.
    for machine in pc-off q35-off; do
        has "$LOG.$machine" '^\[init\] switching off: stopping the services$' "init did not stop the services"
        stopped_in_order "$LOG.$machine" "$([ $machine = pc-off ] && echo blk)"
        [ "$(text "$LOG.$machine" | grep -av '^$' | tail -1)" = "[power] switching off: sleep type 0/0 into PM1 control (ACPI S5)" ] \
            || fail "$LOG.$machine: the last line is not power's switching off"
        grep -q "terminating on signal" "$LOG.$machine.err" && fail "$LOG.$machine: QEMU was still running - the machine did not switch off"
    done
    [ "$("$TFSTOOL" get "$DISK.off" saved 2>&1)" = "before-off" ] || fail "pc-off: the file written before poweroff is not on the disk"

    # Reset: the services stopped, the reset written, and a second boot.
    for machine in pc-reboot q35-reboot; do
        has "$LOG.$machine" '^\[init\] resetting: stopping the services$' "init did not stop the services"
        has "$LOG.$machine" '^\[power\] resetting: 0x[6f] into port 0xcf9$' "power did not reset"
        [ "$(count "$LOG.$machine" '^Hello from TheTaaJ!')" = 2 ] || fail "$LOG.$machine: the kernel did not start a second time"
        [ "$(count "$LOG.$machine" '^\[shell\] ready')" = 2 ] || fail "$LOG.$machine: the second boot did not get as far as the shell"
    done

    # No ACPI: no power control, refused, and nothing stopped.
    has "$LOG.pc-none" '^\[acpi\] power: .* - no power control$' "the kernel did not say there is no power control"
    has "$LOG.pc-none" '^error: this machine cannot be switched off or reset from here$' "poweroff was not refused"
    has "$LOG.pc-none" '^still here$' "the shell did not go on after the refusal"
    text "$LOG.pc-none" | grep -aq '^\[init\] stopped' && fail "pc-none: services were stopped although nothing could switch off"
    echo "poweroff and reboot worked on a PC and a q35: the services stopped in order (shell, fs, disk driver, registry), S5 from \\_S5 switched the machine off with the file written before it on the disk, the reset register reset it, and a PC without ACPI refused"
else
    IMAGE=$2
    LOG=$3
    echo "Booting AArch64 twice to switch it off and reset it, through PSCI (logs: $LOG.*)..."
    # "/" waits for a new serial driver (ten seconds at most): the boot
    # script ends QEMU as soon as the keys are typed, which would be
    # before the machine has switched off - or started again.
    KEYS="$(keys 'poweroff') ret /" SLOTS=2 QEMU_ARGS="-smp 2" "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG.off" 40 > /dev/null 2>&1 &
    KEYS="$(keys 'reboot') ret /" SLOTS=2 QEMU_ARGS="-smp 2 -action reboot=reset" "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG.reboot" 60 > /dev/null 2>&1 &
    wait
    for machine in off reboot; do
        has "$LOG.$machine" '^\[psci\] power: SYSTEM_OFF and SYSTEM_RESET - handed to init$' "the kernel did not hand on a power handle"
        has "$LOG.$machine" '^\[power\] PSCI: SYSTEM_OFF and SYSTEM_RESET; ready$' "power did not start"
        stopped_in_order "$LOG.$machine" ""
        text "$LOG.$machine" | grep -aEq 'KERNEL PANIC|CPU EXCEPTION|\[init\] could not|firmware refused' && fail "$LOG.$machine: something failed"
    done
    [ "$(text "$LOG.off" | grep -av '^$' | tail -1)" = "[psci] SYSTEM_OFF" ] || fail "$LOG.off: the last line is not the kernel's SYSTEM_OFF"
    grep -q "terminating on signal" "$LOG.off.qemu" && fail "$LOG.off: QEMU was still running - the machine did not switch off"
    has "$LOG.reboot" '^\[psci\] SYSTEM_RESET$' "the kernel did not make the SYSTEM_RESET call"
    [ "$(count "$LOG.reboot" '^\[kernel\] idle')" = 2 ] || fail "$LOG.reboot: the kernel did not start a second time"
    echo "poweroff and reboot worked on AArch64: the services stopped in order, then PSCI's SYSTEM_OFF switched the machine off and SYSTEM_RESET started it again"
fi
