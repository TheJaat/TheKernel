#!/bin/sh
# nvme-test.sh - the system's disk on an NVMe controller: the kernel
# hands the controller on (its BAR 0 cut short of the MSI-X table that
# follows the doorbells, an MSI-X interrupt), init starts `nvme` for
# it, and fs finds its files there as on any disk. Two machines, both
# booted at once:
#
#   pc      QEMU's default PC (BIOS), with an NVMe controller added -
#           as QEMU makes it, moving up to 512 KiB a command: each
#           request of the protocol is one command, its pages in a PRP
#           list of up to 31 entries
#   q35     QEMU's newer PC (UEFI firmware), whose controller moves at
#           most 16 KiB a command (mdts=2): a request of 128 KiB is
#           sixteen commands, each with its own part of the list
#
# On each: the disk's size, `disk check` (256 sectors at once against
# one by one, and the requests the driver must refuse), files read and
# written, programs loaded from it. On pc also the watchdog: the driver
# goes silent (`disk stall`), init stops it and starts a new one, which
# turns the controller off and on again - forgetting the old driver's
# queues - and works. Then the files written are read off the disk
# image on the host.
#
#   usage: nvme-test.sh IMAGE DISK LOG
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
# Silent for 3 s: init gives a disk driver 2. The step waits until init
# restarts it (or ten seconds pass) - the new driver says what the old
# one said, so its own line cannot be waited for; it comes within the
# same moment - then uses the disk directly: fs keeps blocks it has
# read, so a file could come from its cache.
pc="$pc $(step stall 'disk stall 3') see:nvme,ended,with,code,-1001,-,restarting,it"
pc="$pc $(step after 'disk read 0' 'disk check' 'cat saved')"
pc="$pc $(keys 'echo == end ==') ret ~"

q35="$(step info 'disk')"
q35="$q35 $(step check 'disk check')"
q35="$q35 $(step files 'cat welcome' 'wc long' 'cp long copy' 'write saved q35')"
q35="$q35 $(keys 'echo == end ==') ret ~"

echo "Booting x86_64 twice with the disk on an NVMe controller: a PC with one added, and a q35 with one that moves 16 KiB a command (logs: $LOG.pc, $LOG.q35)..."
NVME_PC="-drive file=$DISK.pc,format=raw,if=none,id=n0 -device nvme,serial=taaj0001,drive=n0"
NVME_Q35="-machine q35 -bios $OVMF -drive file=$DISK.q35,format=raw,if=none,id=n0 -device nvme,serial=taaj0002,drive=n0,mdts=2"
KEYS="$pc" QEMU_ARGS="$NVME_PC" SMP=2 SLOTS=3 "$HERE/qemu-boot.sh" "$IMAGE" "$LOG.pc" 60 &
PC_BOOT=$!
KEYS="$q35" QEMU_ARGS="$NVME_Q35" SMP=2 SLOTS=3 "$HERE/qemu-boot.sh" "$IMAGE" "$LOG.q35" 90 &
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
    # 0x2000: the registers and the first doorbells, and not the MSI-X
    # table QEMU puts after them in the same BAR.
    logged $machine '^\[nvme\] [0-9a-f:.]+ [0-9a-f]{4}:[0-9a-f]{4}: registers 0x[0-9a-f]+\+0x2000 \(BAR 0\), 148 KiB DMA at 0x[0-9a-f]+, MSI-X$' \
        "nvme was not given the controller's registers short of the MSI-X table, and an MSI-X interrupt"
    logged $machine '^\[init\] started: .* nvme ' "init did not start nvme"
    # Not at the start of a line: the shell may echo the first key typed
    # just before it, on the same line.
    logged $machine '\[fs\] ready: TaajFS on the disk' "fs did not find TaajFS on the NVMe disk"
    has $machine info '^disk: 8192 sectors of 512 bytes \(4096 KiB\)$' "the disk's size, from its IDENTIFY answer, is wrong"
    has $machine check '^disk check: all 9 checks passed$' "disk check failed through nvme"
    has $machine files '^Hello from the host$' "a file on the NVMe disk could not be read"
    has $machine files '^long: 300 lines, 1200 words, 4692 bytes$' "wc did not count the long file, read in many sectors at once"
    has $machine files '\[init\] started wc for the shell, from a file' "wc was not loaded from the NVMe disk"
    grep -aE '\[nvme\] (read|write) of' "$LOG.$machine.text" && fail "$machine: the disk reported an error"
done

logged pc '^\[nvme\] NVMe 1\.4 controller at 00:0[0-9a-f]\.0: QEMU NVMe Ctrl; namespace 1, 8192 sectors \(4 MiB\), 128 KiB a command, MSI-X interrupts$' \
    "nvme did not find namespace 1, moving a whole request in one command"
logged q35 '^\[nvme\] NVMe 1\.4 controller at 00:0[0-9a-f]\.0: QEMU NVMe Ctrl; namespace 1, 8192 sectors \(4 MiB\), 16 KiB a command, MSI-X interrupts$' \
    "nvme did not take the q35 controller's limit of 16 KiB a command"

has pc files '^Read me, from the host$' "a file in a directory could not be read"
has pc write '^copy: 300 lines, 1200 words, 4692 bytes$' "the copy, written to the NVMe disk, is not the file it was copied from"
has pc stall '^disk: the driver will answer nothing for 3 s$' "nvme did not take the stall request"
has pc stall '\[nvme\] asked to stall' "nvme did not stall"
has pc stall '\[init\] nvme has not answered for 2 s - stopping it' "init did not notice that nvme stopped answering"
has pc stall '\[init\] nvme ended with code -1001 - restarting it' "init did not restart nvme"
[ "$(grep -ac '^\[nvme\] NVMe 1\.4 controller' "$LOG.pc.text")" = 2 ] \
    || fail "pc: the restarted nvme did not take the controller over"
has pc after '^0000  4a 41 46 53 ' "the restarted driver did not read sector 0 (TaajFS's JAFS)"
has pc after '^disk check: all 9 checks passed$' "disk check failed through the restarted driver"
has pc after '^kept$' "a file written before the restart was not there after it"

# What the guests wrote, read off the disk images by the host.
[ "$("$TFSTOOL" get "$DISK.pc" saved 2>&1)" = "kept" ] || fail "pc: \"saved\" on the disk does not hold \"kept\""
"$TFSTOOL" get "$DISK.pc" copy > "$LOG.copy" 2>&1 && cmp -s "$LOG.copy" "$DISK.long" \
    || fail "pc: \"copy\" on the disk is not byte for byte \"long\""
[ "$("$TFSTOOL" get "$DISK.q35" saved 2>&1)" = "q35" ] || fail "q35: \"saved\" on the disk does not hold \"q35\""
"$TFSTOOL" get "$DISK.q35" copy > "$LOG.copy" 2>&1 && cmp -s "$LOG.copy" "$DISK.long" \
    || fail "q35: \"copy\" on the disk, written 16 KiB a command, is not byte for byte \"long\""

echo "nvme drove the disk on a PC's added NVMe controller (a request a command) and on a q35's (16 KiB a command): IDENTIFY, disk check, files read and written, programs loaded, a watchdog restart that took the controller over; the host read back what was written"
