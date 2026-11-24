#!/bin/sh
# ahci-test.sh - the system's disk on a SATA controller instead of a
# virtio one: the kernel hands the AHCI controller on (registers in a
# memory BAR, an MSI interrupt), init starts `ahci` for it, and fs
# finds its files there as on any disk. Two machines, both booted at
# once:
#
#   pc      QEMU's default PC (BIOS), with an AHCI controller added: an
#           empty CD drive on its port 0, the disk on port 2
#   q35     QEMU's newer PC (UEFI firmware), whose chipset has the
#           AHCI controller built in, at 00:1f.2 - with the boot CD on
#           port 2, which the firmware has just read the loader from,
#           and the disk on port 3
#
# On each: the disk's size, `disk check` (256 sectors at once against
# one by one, and the requests the driver must refuse), files read and
# written, programs loaded from it. On pc also the watchdog: the driver
# goes silent (`disk stall`), init stops it and starts a new one, which
# takes the controller over from the old one and works. Then the files
# written are read off the disk image on the host.
#
#   usage: ahci-test.sh IMAGE DISK LOG
#
# DISK is a test disk (make-test-disk: welcome, long, docs/readme and
# the programs); OVMF, in the environment, the UEFI firmware.

set -u
IMAGE=$1
DISK=$2
LOG=$3
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}
OVMF=${OVMF:?the UEFI firmware}

fail() {
    echo "FAIL: $1 (logs: $LOG.pc, $LOG.q35)"
    exit 1
}

keys() {
    python3 -I "$HERE/key-names.py" "$1" || fail "no keys for: $1"
}

# Each step is marked in the log by a line of its own, "== NAME ==".
step() { # step NAME COMMAND...
    out="$(keys "echo == $1 ==") ret ~"
    shift
    for command in "$@"; do out="$out $(keys "$command") ret ~"; done
    echo "$out"
}

[ -f "$DISK" ] && [ -f "$DISK.long" ] || fail "no test disk at $DISK"
cp "$DISK" "$DISK.pc" && cp "$DISK" "$DISK.q35" || fail "cannot copy the disk"

pc="$(step info 'disk')"
pc="$pc $(step check 'disk check')"
pc="$pc $(step files 'cat welcome' 'wc long' 'cat docs/readme')"
pc="$pc $(step write 'write saved kept' 'cp long copy' 'wc copy')"
# Silent for 3 s: init gives a disk driver 2. The step waits until the
# new driver is serving (or ten seconds pass), then uses it directly -
# fs keeps blocks it has read, so a file could come from its cache.
pc="$pc $(step stall 'disk stall 3') see:ahci,ended,with,code,-1001 see:disk,on,port,2:,QEMU,HARDDISK"
pc="$pc $(step after 'disk read 0' 'disk check' 'cat saved')"
pc="$pc $(keys 'echo == end ==') ret ~"

q35="$(step info 'disk')"
q35="$q35 $(step check 'disk check')"
q35="$q35 $(step files 'cat welcome' 'write saved q35')"
q35="$q35 $(keys 'echo == end ==') ret ~"

echo "Booting x86_64 twice with the disk on an AHCI controller: a PC with one added, and a q35's own (logs: $LOG.pc, $LOG.q35)..."
SATA_PC="-device ahci,id=sata -drive if=none,id=cd,media=cdrom -device ide-cd,drive=cd,bus=sata.0"
SATA_PC="$SATA_PC -drive file=$DISK.pc,format=raw,if=none,id=d0 -device ide-hd,drive=d0,bus=sata.2"
SATA_Q35="-machine q35 -bios $OVMF -drive file=$DISK.q35,format=raw,if=none,id=d0 -device ide-hd,drive=d0,bus=ide.3"
KEYS="$pc" QEMU_ARGS="$SATA_PC" SMP=2 SLOTS=3 "$HERE/qemu-boot.sh" "$IMAGE" "$LOG.pc" 60 &
PC_BOOT=$!
KEYS="$q35" QEMU_ARGS="$SATA_Q35" SMP=2 SLOTS=3 "$HERE/qemu-boot.sh" "$IMAGE" "$LOG.q35" 90 &
Q35_BOOT=$!
wait $PC_BOOT
wait $Q35_BOOT

# The log without the serial line's \r, and each step's part of it.
tr -d '\r' < "$LOG.pc" > "$LOG.pc.text"
tr -d '\r' < "$LOG.q35" > "$LOG.q35.text"
part() { # part MACHINE NAME: the lines after "== NAME ==", to the next mark
    awk -v start="== $2 ==" '$0 == start { on = 1; next } /^== [a-z-]+ ==$/ { on = 0 } on' "$LOG.$1.text"
}
has() { # has MACHINE NAME PATTERN WHY
    part "$1" "$2" | grep -aEq "$3" || fail "$1: $4"
}
logged() { # logged MACHINE PATTERN WHY: anywhere in the log
    grep -aEq "$2" "$LOG.$1.text" || fail "$1: $3"
}

for machine in pc q35; do
    grep -aq '^== end ==$' "$LOG.$machine.text" || fail "$machine: the steps did not all run"
    grep -aEq 'KERNEL PANIC|CPU EXCEPTION|THE SYSTEM HAS STOPPED' "$LOG.$machine.text" && fail "$machine: the kernel stopped"
    grep -aq '\[blk\]' "$LOG.$machine.text" && fail "$machine: blk ran - there should be no virtio disk"
    logged $machine '^\[pci\] [0-9a-f:.]+ AHCI controller: registers 0x[0-9a-f]+\+0x[0-9a-f]+, 132 KiB DMA at 0x[0-9a-f]+, MSI - handed to init$' \
        "the kernel did not hand the AHCI controller to init, with its registers and an MSI interrupt"
    logged $machine '^\[init\] started: .* ahci ' "init did not start ahci"
    # Not at the start of a line: the shell may echo the first key typed
    # just before it, on the same line.
    logged $machine '\[fs\] ready: TaajFS on the disk' "fs did not find TaajFS on the SATA disk"
    has $machine info '^disk: 8192 sectors of 512 bytes \(4096 KiB\)$' "the disk's size, from its IDENTIFY answer, is wrong"
    has $machine check '^disk check: all 9 checks passed$' "disk check failed through ahci"
    has $machine files '^Hello from the host$' "a file on the SATA disk could not be read"
    grep -aE '\[ahci\] (read|write) of' "$LOG.$machine.text" && fail "$machine: the disk reported an error"
done

logged pc '^\[ahci\] AHCI 1\.0 controller at 00:0[0-9a-f]\.0, 6 ports; disk on port 2: QEMU HARDDISK, 8192 sectors \(4 MiB\), MSI interrupts$' \
    "ahci did not find the disk on port 2, past the CD drive on port 0"
logged q35 '^\[ahci\] AHCI 1\.0 controller at 00:1f\.2, 6 ports; disk on port 3: QEMU HARDDISK, 8192 sectors \(4 MiB\), MSI interrupts$' \
    "ahci did not find the disk on the q35's own controller, port 3, past the boot CD on port 2"

has pc files '^long: 300 lines, 1200 words, 4692 bytes$' "wc did not count the long file, read in many sectors at once"
has pc files '^Read me, from the host$' "a file in a directory could not be read"
has pc files '\[init\] started wc for the shell, from a file' "wc was not loaded from the SATA disk"
has pc write '^copy: 300 lines, 1200 words, 4692 bytes$' "the copy, written to the SATA disk, is not the file it was copied from"
has pc stall '^disk: the driver will answer nothing for 3 s$' "ahci did not take the stall request"
has pc stall '\[ahci\] asked to stall' "ahci did not stall"
has pc stall '\[init\] ahci has not answered for 2 s - stopping it' "init did not notice that ahci stopped answering"
has pc stall '\[init\] ahci ended with code -1001 - restarting it' "init did not restart ahci"
[ "$(grep -ac '^\[ahci\] AHCI 1\.0 controller' "$LOG.pc.text")" = 2 ] \
    || fail "pc: the restarted ahci did not take the controller over"
has pc after '^0000  4a 41 46 53 ' "the restarted driver did not read sector 0 (TaajFS's JAFS)"
has pc after '^disk check: all 9 checks passed$' "disk check failed through the restarted driver"
has pc after '^kept$' "a file written before the restart was not there after it"

# What the guests wrote, read off the disk images by the host.
[ "$("$TFSTOOL" get "$DISK.pc" saved 2>&1)" = "kept" ] || fail "pc: \"saved\" on the disk does not hold \"kept\""
"$TFSTOOL" get "$DISK.pc" copy > "$LOG.copy" 2>&1 && cmp -s "$LOG.copy" "$DISK.long" \
    || fail "pc: \"copy\" on the disk is not byte for byte \"long\""
[ "$("$TFSTOOL" get "$DISK.q35" saved 2>&1)" = "q35" ] || fail "q35: \"saved\" on the disk does not hold \"q35\""

echo "ahci drove the disk on a PC's added AHCI controller (port 2, past a CD drive) and on a q35's own (port 3, past the boot CD): IDENTIFY, disk check, files read and written, programs loaded, a watchdog restart that took the controller over; the host read back what was written"
