#!/bin/sh
# aarch64-uefi-test.sh - boots the AArch64 kernel the way a UEFI
# machine would: firmware, which runs TheTaaJBoot's AArch64 loader from
# a FAT partition, which loads the kernel from the same partition.
#
#   usage: aarch64-uefi-test.sh ESP DISK BUILD_DIR
#
# ESP is the partition's image (the loader as EFI/BOOT/BOOTAA64.EFI,
# the kernel as kernel/Image); DISK a disk with the system's programs
# on it, as the typed test has. Logs go to BUILD_DIR.
#
# The boots:
#   uefi      four CPUs, the disk, and a few lines typed: the loader
#             must say what it did, the kernel must stay out of the
#             memory the firmware keeps, start the other CPUs, and the
#             system must run a program from the disk
#   uefi-el2  firmware running at exception level 2, and the newer
#             interrupt controller: the loader turns the MMU off at
#             that level, the kernel comes down from it
#   uefi-acpi firmware left to describe the machine with ACPI tables:
#             it has no device tree to give, and the loader must say so
#             and start nothing
set -u

ESP=$1
DISK=$2
BUILD=$3
HERE=$(dirname "$0")

fail() {
    echo "FAIL: $1"
    exit 1
}

# What every boot through the loader must show. $1 log, $2 the level
# the firmware runs at, $3 CPUs, $4 how the firmware is called.
check() {
    log=$1
    grep -aq "TheTaaJBoot (UEFI, AArch64)" "$log" || fail "$log: the loader did not start"
    grep -aq "firmware at exception level $2 - exiting boot services" "$log" \
        || fail "$log: the loader did not find the firmware at exception level $2"
    grep -aq "Hello from TheTaaJ" "$log" || fail "$log: the kernel did not start"
    # (The firmware runs its loaders with the MMU on; turning it off is
    # the loader's last act.)
    grep -aq "the loader entered at level $2, with the MMU off" "$log" || fail "$log: the kernel was not entered at level $2 with the MMU off"
    # The ranges the loader wrote into the tree are the ones the kernel
    # reads out of it: as many as the loader said, and at least one -
    # firmware that keeps nothing for itself is not UEFI.
    said=$(sed -n 's/.* with \([0-9]*\) ranges the firmware keeps.*/\1/p' "$log" | head -1)
    read=$(grep -ac "^\[tree\] reserved " "$log")
    [ -n "$said" ] && [ "$said" -ge 1 ] || fail "$log: the loader named no memory the firmware keeps"
    [ "$read" -eq "$said" ] || fail "$log: the loader wrote $said reserved ranges, the kernel read $read"
    grep -aq "\[selftest\] memory: all passed" "$log" || fail "$log: memory self-test"
    grep -aq "\[smp\] $3 CPUs awake: .*asked by $4" "$log" || fail "$log: the other CPUs were not started"
    grep -aq "\[smp\] $3 CPUs run threads" "$log" || fail "$log: the other CPUs do not run threads"
    grep -aq "\[kernel\] idle" "$log" || fail "$log: did not reach idle"
    ! grep -aqE "KERNEL PANIC|CPU EXCEPTION|FAILED" "$log" || fail "$log: a panic, an exception or a failed check"
}

echo "Booting the AArch64 kernel through UEFI firmware and its loader (log: $BUILD/uefi.log)..."
cp "$DISK" "$BUILD/uefi.disk"
UEFI=$ESP SLOTS=4 KEYS="e c h o spc b o o t e d spc b y spc u e f i ret c h e l l o spc o n e ret" \
    QEMU_ARGS="-smp 4 -drive file=$BUILD/uefi.disk,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
    "$HERE/aarch64-boot.sh" - "$BUILD/uefi.log" 60 || fail "the boot through UEFI did not finish"
check "$BUILD/uefi.log" 1 4 hvc
grep -aq "^booted by uefi" "$BUILD/uefi.log" || fail "uefi: the shell did not answer"
grep -aq "\[init\] started chello for the shell, from a file" "$BUILD/uefi.log" || fail "uefi: no program was run from the disk"
grep -aq "^Hello from C" "$BUILD/uefi.log" || fail "uefi: the program from the disk did not run"

echo "Again with the firmware at exception level 2 (log: $BUILD/uefi-el2.log)..."
UEFI=$ESP SLOTS=4 MACHINE=",virtualization=on,gic-version=3" QEMU_ARGS="-smp 4" \
    "$HERE/aarch64-boot.sh" - "$BUILD/uefi-el2.log" 60 || fail "the boot through UEFI at level 2 did not finish"
check "$BUILD/uefi-el2.log" 2 4 smc
grep -aq "\[irq\] GIC version 3" "$BUILD/uefi-el2.log" || fail "uefi-el2: not the newer interrupt controller"

echo "And with firmware that has no device tree to give (log: $BUILD/uefi-acpi.log)..."
UEFI=$ESP ACPI=on "$HERE/aarch64-boot.sh" - "$BUILD/uefi-acpi.log" 60 || fail "the boot with ACPI did not finish"
grep -aq "TheTaaJBoot: the firmware has no device tree to give" "$BUILD/uefi-acpi.log" \
    || fail "uefi-acpi: the loader did not say that there is no device tree"
! grep -aq "Hello from TheTaaJ" "$BUILD/uefi-acpi.log" || fail "uefi-acpi: the kernel was started without a device tree"

echo "aarch64-uefi-test passed"
