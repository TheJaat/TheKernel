#!/bin/sh
# stick-test.sh - booting from a USB stick, and the loader reading its
# disk through the BIOS.
#
#   usage: stick-test.sh ISO STICK DISK LOG
#
# STICK is a stick image (make's hybrid64: the ISO, the EFI system
# partition, and a TaajFS partition holding the test disk's files);
# DISK the test disk itself. Three machines, booted at once:
#
#   bios    SeaBIOS, no CD, no disk: only the stick, on an xHCI
#           controller. The BIOS runs the stick's MBR, which loads
#           stage1; stage1 and stage2 read the ISO 9660 file system
#           through int 13h in 512-byte sectors; the kernel's usb finds
#           the stick, and fs TaajFS in its third partition. Files are
#           read, written and copied, programs loaded from it.
#   uefi    the same stick under UEFI (OVMF): the firmware finds the EFI
#           system partition (type 0xEF) and runs the loader in it.
#   sata    q35 with SeaBIOS, booted from the ISO in its CD drive - which
#           on a q35 is on the chipset's SATA (AHCI) controller, which
#           the old loader's IDE driver could not read: int 13h,
#           2048-byte sectors.
#
# The files written are then read off the stick images on the host.
# OVMF, in the environment: the UEFI firmware.

set -u
ISO=$1
STICK=$2
DISK=$3
LOG=$4
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}
OVMF=${OVMF:?the UEFI firmware}

fail() {
    echo "FAIL: $1"
    exit 1
}
keys() {
    python3 -I "$HERE/key-names.py" "$1" || fail "no keys for: $1"
}
step() { # step NAME COMMAND...
    out="$(keys "echo == $1 ==") ret ~"
    shift
    for command in "$@"; do out="$out $(keys "$command") ret ~"; done
    echo "$out"
}
[ -f "$STICK" ] && [ -f "$DISK.long" ] || fail "no stick image at $STICK, or no test disk at $DISK"
for copy in bios uefi; do
    cp "$STICK" "$STICK.$copy" || fail "cannot copy the stick"
done
cp "$DISK" "$DISK.sata" || fail "cannot copy the disk"

ON_STICK="$(step info 'disk')"
ON_STICK="$ON_STICK $(step files 'cat welcome' 'wc long' 'cat docs/readme')"
ON_STICK="$ON_STICK $(step write 'write saved kept' 'cp long copy' 'wc copy')"
ON_STICK="$ON_STICK $(keys 'echo == end ==') ret ~"
SATA="$(step files 'cat welcome')"
SATA="$SATA $(keys 'echo == end ==') ret ~"

stick_args() { # stick_args IMAGE
    echo "-device qemu-xhci,id=xhci -drive if=none,id=stick,format=raw,file=$1 -device usb-storage,bus=xhci.0,drive=stick,bootindex=0"
}
echo "Booting x86_64 from a USB stick (BIOS, UEFI) and from a SATA CD (logs: $LOG.*)..."
# qemu-boot.sh always gives the machine a CD drive; for the sticks it
# holds a blank "CD" that nothing can boot from, so whatever boots, boots
# from the stick.
head -c 65536 /dev/zero > "$LOG.blank" || fail "cannot make the blank CD"
BIOS_ARGS="$(stick_args "$STICK.bios")"
UEFI_ARGS="-bios $OVMF $(stick_args "$STICK.uefi")"
DISK= SMP=2 SLOTS=3 KEYS="$ON_STICK" QEMU_ARGS="$BIOS_ARGS" "$HERE/qemu-boot.sh" "$LOG.blank" "$LOG.bios" 90 > /dev/null 2> "$LOG.bios.err" &
DISK= SMP=2 SLOTS=3 KEYS="$ON_STICK" QEMU_ARGS="$UEFI_ARGS" "$HERE/qemu-boot.sh" "$LOG.blank" "$LOG.uefi" 90 > /dev/null 2> "$LOG.uefi.err" &
DISK="$DISK.sata" SMP=2 KEYS="$SATA" QEMU_ARGS="-machine q35" "$HERE/qemu-boot.sh" "$ISO" "$LOG.sata" 90 > /dev/null 2> "$LOG.sata.err" &
wait

for machine in bios uefi sata; do
    tr -d '\r' < "$LOG.$machine" > "$LOG.$machine.text"
done
part() { # part MACHINE NAME: the lines after "== NAME ==", to the next mark
    awk -v start="== $2 ==" '$0 == start { on = 1; next } /^== [a-z-]+ ==$/ { on = 0 } on' "$LOG.$1.text"
}
has() { # has MACHINE NAME PATTERN WHY
    part "$1" "$2" | grep -aEq "$3" || fail "$LOG.$1: $4"
}
logged() { # logged MACHINE PATTERN WHY
    grep -aEq "$2" "$LOG.$1.text" || fail "$LOG.$1: $3"
}
for machine in bios uefi sata; do
    grep -aq '^== end ==$' "$LOG.$machine.text" || fail "$LOG.$machine: the steps did not all run"
    grep -aEq 'KERNEL PANIC|CPU EXCEPTION|THE SYSTEM HAS STOPPED' "$LOG.$machine.text" && fail "$LOG.$machine: the kernel stopped"
done
logged bios '^  loader +TheTaaJBoot \(BIOS\)$' "the BIOS loader did not boot the kernel from the stick"
logged uefi '^  loader +TheTaaJBoot \(UEFI\)$' "the UEFI loader did not boot the kernel from the stick"
logged sata '^  loader +TheTaaJBoot \(BIOS\)$' "the BIOS loader did not boot the kernel from the SATA CD"
for machine in bios uefi; do
    grep -aEq '\[(blk|ahci|nvme)\]' "$LOG.$machine.text" && fail "$LOG.$machine: another disk driver ran"
    logged $machine '^\[usb\] port [0-9]+: disk "QEMU QEMU HARDDISK" \(46f4:0001, [a-zA-Z ]+\), [0-9]+ sectors \([0-9]+ MiB\) ready$' "usb did not start the stick"
    logged $machine '^\[fs\] TaajFS partition: sectors [0-9]+ to [0-9]+$' "fs did not find the TaajFS partition"
    logged $machine '\[fs\] ready: TaajFS on the disk' "fs did not mount TaajFS from the stick"
    has $machine files '^Hello from the host$' "a file in the stick's TaajFS partition could not be read"
    has $machine files '^long: 300 lines, 1200 words, 4692 bytes$' "wc did not count the long file"
    has $machine files '\[init\] started wc for the shell, from a file' "wc was not loaded from the stick"
    has $machine write '^copy: 300 lines, 1200 words, 4692 bytes$' "the copy, written to the stick, is not the file it was copied from"
    [ "$("$TFSTOOL" get "$STICK.$machine" saved 2>&1)" = "kept" ] || fail "$machine: the stick's \"saved\" does not hold \"kept\""
    "$TFSTOOL" get "$STICK.$machine" copy > "$LOG.copy" 2>&1 && cmp -s "$LOG.copy" "$DISK.long" || fail "$machine: the stick's \"copy\" is not \"long\""
done
has sata files '^Hello from the host$' "the machine booted from the SATA CD could not read its disk"
echo "Booting from a USB stick worked: the MBR, stage1 and stage2 through int 13h (BIOS), the EFI system partition (UEFI), TaajFS in the stick's partition; and the loader read a SATA CD"
