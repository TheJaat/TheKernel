#!/bin/sh
# aarch64-test.sh - builds the AArch64 kernel, boots it in QEMU on
# several machines, and checks what it says.
#
#   usage: aarch64-test.sh BUILD_DIR OBJCOPY
#
# OBJCOPY must be LLVM's (llvm-objcopy): the one that comes with a PC's
# compiler does not know AArch64 files.
#
# The boots:
#   plain      the virt machine as QEMU makes it: entered at level 1
#   el2        entered at level 2, as real boards mostly do
#   a53, max   other CPUs, with other sizes of physical address
#   smp4       four CPUs, of which the kernel so far uses one
#   dirty      random bytes in the memory the kernel must zero itself
#   fault      a kernel built to read unmapped memory: the exception
#              must be caught and named

set -eu

BUILD=$1
OBJCOPY=$2
HERE=$(cd "$(dirname "$0")/.." && pwd)
TARGET=aarch64-unknown-none-softfloat
OUT="$BUILD/aarch64"
mkdir -p "$OUT"

failed=0
fail() {
    echo "  FAILED: $1"
    failed=1
}

# need LOG PATTERN: the log must have a line matching the pattern.
need() {
    grep -aEq "$2" "$1" || fail "no line matching: $2"
}

# ---- build ------------------------------------------------------------
(cd "$HERE" && cargo build --release --quiet --target $TARGET)
"$OBJCOPY" -O binary "$HERE/target/$TARGET/release/kernel64" "$OUT/Image"
(cd "$HERE" && cargo build --release --quiet --target $TARGET --features fault-page --target-dir target/aarch64-fault-page)
"$OBJCOPY" -O binary "$HERE/target/aarch64-fault-page/$TARGET/release/kernel64" "$OUT/Image-fault-page"

# ---- the image header -------------------------------------------------
# What a loader looks at before it runs a single instruction. Numbers
# are little-endian; od prints them as such.
echo "aarch64: the image header"
field() { od -A n -t "$1" -j "$2" -N "$3" "$OUT/Image" | tr -d ' \n'; }
[ "$(field c 56 4)" = "ARMd" ] || fail "the magic at byte 56 is '$(field c 56 4)', not 'ARMd'"
[ "$(field u8 8 8)" = 524288 ] || fail "the load offset is $(field u8 8 8), not 524288 (0x80000)"
size=$(field u8 16 8)
file=$(wc -c < "$OUT/Image")
[ "$size" -ge "$file" ] || fail "the header says the image needs $size bytes, but the file alone is $file"
[ $((size % 4096)) -eq 0 ] || fail "the image size $size is not whole pages"
echo "  $file bytes in the file, $size in memory"

# ---- boots ------------------------------------------------------------
# boot NAME LEVEL: runs the image (with whatever MACHINE, CPU and
# QEMU_ARGS say) and checks every line an ordinary boot must print.
boot() {
    name=$1
    level=$2
    log="$OUT/$name.log"
    echo "aarch64: boot '$name'"
    if ! "$HERE/scripts/aarch64-boot.sh" "$OUT/Image" "$log"; then
        fail "the boot did not finish"
        return
    fi
    need "$log" "^Hello from TheTaaJ!"
    need "$log" "kernel_main at 0xffffffff8008"
    need "$log" "\[cpu\] exception vectors loaded"
    need "$log" "\[cpu\] breakpoint at 0xffffffff8008[0-9a-f]+ - continuing"
    need "$log" "\[cpu\] resumed after the breakpoint"
    need "$log" "at exception level 1; the loader entered at level $level"
    need "$log" "the kernel loaded at 0x4[0-9a-f]*80000 and running at 0xffffffff80080000"
    need "$log" "\[boot\] device tree at 0x4[0-9a-f]+: [0-9]+ bytes, version 17, boot CPU 0"
    need "$log" "\[kernel\] this is as far as the AArch64 port goes - halting"
    [ "$(grep -ac 'breakpoint at' "$log")" -eq 1 ] || fail "the breakpoint was taken more than once"
    if grep -aEq "PANIC|EXCEPTION|FAILED" "$log"; then
        fail "the log has a panic, an exception or a failed check"
    fi
    grep -a "^\[cpu\] .* bits of physical\|entered at" "$log" | sed 's/^/  /' | cut -c1-110
}

boot plain 1
MACHINE=,virtualization=on boot el2 2
CPU=cortex-a53 MEMORY=1G boot a53 1
CPU=max MEMORY=128M MACHINE=,gic-version=3 boot max 1
QEMU_ARGS="-smp 4" boot smp4 1

# QEMU's memory starts as zeroes, which is exactly what the kernel's
# first instructions are supposed to make of the memory after the image
# (.bss: its tables, its stack, its variables). So a kernel that forgot
# to would pass every boot above. This one puts random bytes there
# first, as real memory after a restart would have.
bss_start=$(( (file + 4095) / 4096 * 4096 ))
head -c $((size - bss_start)) /dev/urandom > "$OUT/junk.bin"
QEMU_ARGS="-device loader,file=$OUT/junk.bin,addr=$((0x40080000 + bss_start)),force-raw=on" boot dirty 1

# ---- the fault --------------------------------------------------------
echo "aarch64: boot 'fault' (a read of unmapped memory)"
log="$OUT/fault.log"
"$HERE/scripts/aarch64-boot.sh" "$OUT/Image-fault-page" "$log" || fail "the boot did not finish"
need "$log" "\[test\] forcing a page fault"
need "$log" "CPU EXCEPTION 0x25: data abort"
need "$log" "kernel read at 0xffffffff00000000: not mapped \(the level 1 table has no entry\)"
need "$log" "from the kernel \(vector 4\)"
need "$log" "    pc  0xffffffff8008"
if grep -aEq "FAILED|this is as far as" "$log"; then
    fail "the kernel carried on after the fault"
fi
grep -a "kernel read at" "$log" | cut -c1-110

if [ $failed -ne 0 ]; then
    echo "aarch64-test: FAILED (logs in $OUT)"
    exit 1
fi
echo "aarch64-test: all passed"
