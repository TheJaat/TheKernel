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
#   unnamed    a device tree that does not say which serial port to use
#   clockless  a device tree that does not give the serial port's clock
#   fault-*    kernels built to read unmapped memory, to write to a
#              read-only page, and to execute data: each exception must
#              be caught and named

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
for fault in page rodata nx; do
    (cd "$HERE" && cargo build --release --quiet --target $TARGET --features fault-$fault --target-dir target/aarch64-fault-$fault)
    "$OBJCOPY" -O binary "$HERE/target/aarch64-fault-$fault/$TARGET/release/kernel64" "$OUT/Image-fault-$fault"
done

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
# boot NAME LEVEL MEMORY CPUS GIC CALL: runs the image (with whatever
# MACHINE, CPU, MEMORY and QEMU_ARGS say) and checks every line an
# ordinary boot must print - including what the kernel read from the
# device tree, which must be what this machine was made with: MEMORY in
# MiB, how many CPUS, which version of interrupt controller, and the
# instruction that calls the firmware.
boot() {
    name=$1
    level=$2
    mib=$3
    cpus=$4
    gic=$5
    call=$6
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
    need "$log" "\[tree\] device tree at 0x4[0-9a-f]+: [0-9]+ bytes, version 17, [0-9]+ nodes"
    need "$log" "\[tree\] machine     \"linux,dummy-virt\""
    need "$log" "\[tree\] memory      0x40000000 - 0x[0-9a-f]+ \($mib MiB\)"
    need "$log" "\[tree\] cpus        $cpus, with ids \[0[, 0-9]*\]"
    need "$log" "\[tree\] serial      PL011 at 0x9000000, ${SERIAL:-named as the console}"
    need "$log" "\[tree\]             its clock ${CLOCK:-runs at 24000000 Hz}"
    need "$log" "\[tree\] interrupts  GIC version $gic: shared part at 0x8000000, per-CPU part at 0x80[1a]0000"
    need "$log" "\[tree\] timer       interrupts 29 \(secure\), 30 \(physical\), 27 \(virtual\), 26 \(hypervisor\)"
    need "$log" "\[tree\] firmware    called with $call"
    [ "$(grep -ac '^\[tree\] memory' "$log")" -eq 1 ] || fail "more than one memory range"
    if [ -z "${CLOCK:-}" ] && grep -aq "as a guess" "$log"; then
        fail "the serial port was guessed though the tree describes it"
    fi
    need "$log" "\[paging\] the tables the CPU is using, read by the kernel: 5 lookups, all as the CPU does them"
    need "$log" "\[paging\] a page at 0x4[0-9a-f]+ mapped twice: .* where the CPU refuses a write"
    need "$log" "\[paging\] a third address unmapped, then mapped to the page at 0x4[0-9a-f]+: the CPU reads the new page there"
    need "$log" "\[kernel\] this is as far as the AArch64 port goes - halting"
    [ "$(grep -ac 'breakpoint at' "$log")" -eq 1 ] || fail "the breakpoint was taken more than once"
    if grep -aEq "PANIC|EXCEPTION|FAILED" "$log"; then
        fail "the log has a panic, an exception or a failed check"
    fi
    grep -a "entered at\|^\[tree\] memory\|^\[tree\] cpus\|^\[tree\] interrupts" "$log" | sed 's/^/  /' | cut -c1-110
}

boot plain 1 512 1 2 hvc
MACHINE=,virtualization=on boot el2 2 512 1 2 smc
# 6 GiB: the end of memory is above 4 GiB, a number that needs both of
# the two cells the tree gives it.
CPU=cortex-a53 MEMORY=6G boot a53 1 6144 1 2 hvc
CPU=max MEMORY=128M MACHINE=,gic-version=3 boot max 1 128 1 3 hvc
QEMU_ARGS="-smp 4" boot smp4 1 512 4 2 hvc

# QEMU's memory starts as zeroes, which is exactly what the kernel's
# first instructions are supposed to make of the memory after the image
# (.bss: its tables, its stack, its variables). So a kernel that forgot
# to would pass every boot above. This one puts random bytes there
# first, as real memory after a restart would have.
bss_start=$(( (file + 4095) / 4096 * 4096 ))
head -c $((size - bss_start)) /dev/urandom > "$OUT/junk.bin"
QEMU_ARGS="-device loader,file=$OUT/junk.bin,addr=$((0x40080000 + bss_start)),force-raw=on" boot dirty 1 512 1 2 hvc

# Trees with something missing. QEMU writes out the tree it would make
# (dumpdtb), a property is taken out of the copy, and QEMU is given the
# copy to pass on (-dtb).
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -display none -machine dumpdtb="$OUT/virt.dtb" 2>/dev/null
# Nobody says which serial port is the console: the first one is used.
"$HERE/scripts/dtb-without.py" "$OUT/virt.dtb" "$OUT/unnamed.dtb" chosen stdout-path
SERIAL="the first one found" QEMU_ARGS="-dtb $OUT/unnamed.dtb" boot unnamed 1 512 1 2 hvc
# The serial port's clock is not given: the kernel must say it guessed.
"$HERE/scripts/dtb-without.py" "$OUT/virt.dtb" "$OUT/clockless.dtb" pl011@9000000 clocks
CLOCK="is not given" QEMU_ARGS="-dtb $OUT/clockless.dtb" boot clockless 1 512 1 2 hvc
need "$OUT/clockless.log" "\[tree\] no serial port with a clock described: using the one QEMU's virt machine has, as a guess"

# ---- the faults -------------------------------------------------------
# One build and one boot per fault. Each must be caught, named, and
# end the kernel there.
fault() {
    name=$1
    announce=$2
    class=$3
    what=$4
    log="$OUT/fault-$name.log"
    echo "aarch64: boot 'fault-$name'"
    "$HERE/scripts/aarch64-boot.sh" "$OUT/Image-fault-$name" "$log" || fail "the boot did not finish"
    need "$log" "\[test\] $announce"
    need "$log" "CPU EXCEPTION $class"
    need "$log" "$what"
    need "$log" "from the kernel \(vector 4\)"
    need "$log" "    pc  0xffffffff"
    if grep -aEq "FAILED|this is as far as" "$log"; then
        fail "the kernel carried on after the fault"
    fi
    grep -aE "$what" "$log" | cut -c1-110
}

fault page "forcing a page fault" "0x25: data abort" \
    "kernel read at 0xffffffff00000000: not mapped \(the level 1 table has no entry\)"
# The two below break a rule on a page the kernel mapped with its own
# page-table code: they check that what that code writes means to the
# CPU what was asked for.
fault rodata "writing to read-only data" "0x25: data abort" \
    "kernel write at 0xffffffffffe01000: protection violation \(at level 3\)"
fault nx "executing data" "0x21: instruction abort" \
    "kernel instruction fetch at 0xffffffffffe00000: protection violation \(at level 3\)"

if [ $failed -ne 0 ]; then
    echo "aarch64-test: FAILED (logs in $OUT)"
    exit 1
fi
echo "aarch64-test: all passed"
