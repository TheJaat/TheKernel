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
#   el2v3      the same with the newer interrupt controller, which the
#              kernel must allow itself to use on the way down
#   a53, max   other CPUs, with other sizes of physical address
#   smp4       four CPUs, of which the kernel so far uses one
#   dirty      random bytes in the memory the kernel must zero itself
#   unnamed    a device tree that does not say which serial port to use
#   clockless  a device tree that does not give the serial port's clock
#   nogic      a device tree whose interrupt controller is unknown: the
#              kernel must say it has no interrupts, and not wait for one
#   notimer    a device tree that does not give the timer's interrupt
#   reserved   device trees that keep some of the memory back: at the
#              very start and the top of 512 MiB, and at the top of 6 GiB
#   fault-*    kernels built to read through a null pointer, to read
#              unmapped memory, to write to a constant, and to execute
#              data: each exception must be caught and named
#   fault-stack, fault-thread-stack
#              kernels that recurse until a stack runs out - the boot
#              stack, and a thread's: each must be reported, from a
#              stack kept for the purpose

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
for fault in null page rodata nx stack thread-stack; do
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
#
# The memory manager must then account for all of that memory: all but
# a megabyte or two free (the kernel, the tree and the page tables take
# the rest), and the highest free frame the last one there is. KEPT is
# how many MiB at the top a test's tree reserves.
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
    if [ "$gic" = none ]; then
        need "$log" "\[tree\] interrupts  no controller that this kernel knows"
        need "$log" "\[irq\] the device tree describes no interrupt controller this kernel knows: no interrupts, no threads"
        if grep -aEq "\[selftest\] (timer|threads)" "$log"; then fail "a timer or thread test ran with no interrupt controller"; fi
    else
        need "$log" "\[tree\] interrupts  GIC version $gic: shared part at 0x8000000, per-CPU part at 0x80[1a]0000"
        # Interrupts: the controller set up, the timer at its rate (ten
        # ticks are 100 ms; an emulator on a busy machine may stretch
        # them, never shorten them much), registers kept, masking kept.
        need "$log" "\[irq\] GIC version $gic: 288 interrupt lines, all off but the timer's"
        need "$log" "\[timer\] the CPU's own: counts at [0-9]+ Hz, interrupt 27, 100 ticks a second"
        need "$log" "\[selftest\] timer: 10 ticks in (9[0-9]|[12][0-9][0-9]) ms \(expected 100\)"
        need "$log" "\[selftest\] context switch: there and back with a known value in each of 11 registers, none changed"
        need "$log" "\[selftest\] interrupts: 5 taken with a known value in each of 28 registers, none changed"
        need "$log" "\[selftest\] interrupts masked for 35 ms: no tick arrived; unmasked: [3-9] counted at once"
        # Threads, on the same scheduler as the PC: taking turns, being
        # interrupted, sleeping, and giving everything back.
        need "$log" "\[selftest\] threads: two yielding threads ran as \"[ab]+\""
        need "$log" "\[selftest\] threads: a thread that never yields was preempted"
        need "$log" "\[selftest\] threads: sleep\(5\) took [567] ticks; 200 threads spawned and reaped, nothing leaked"
    fi
    need "$log" "\[tree\] timer       ${TIMER:-interrupts 29 \(secure\), 30 \(physical\), 27 \(virtual\), 26 \(hypervisor\)}"
    if grep -aq "unexpected interrupt" "$log"; then fail "an interrupt arrived that nothing asked for"; fi
    need "$log" "\[tree\] firmware    called with $call"
    [ "$(grep -ac '^\[tree\] memory' "$log")" -eq 1 ] || fail "more than one memory range"
    if [ -z "${CLOCK:-}" ] && grep -aq "as a guess" "$log"; then
        fail "the serial port was guessed though the tree describes it"
    fi
    free=$((mib - ${KEPT:-0}))
    top=$(printf '%x' $((0x40000000 + free * 1048576 - 4096)))
    need "$log" "\[memory\] [0-9]+ frames free of $((mib * 256 + 262144)) \(($((free - 3))|$((free - 2))|$((free - 1))) MiB\), bitmap [0-9]+ KiB at 0x4"
    need "$log" "\[memory\] own page tables at 0x4[0-9a-f]+: direct map 0xffff800000000000, NX on"
    need "$log" "\[memory\] heap at 0xffffc00000000000, 1024 KiB to start"
    need "$log" "\[tree\] read again through the direct map: the same [0-9]+ nodes, in memory kept out of the allocator"
    need "$log" "\[selftest\] frames: allocate, free, reuse; the kernel's own are not free"
    need "$log" "\[selftest\] top of memory: the highest free frame, at 0x$top, written and read back"
    need "$log" "\[selftest\] paging: map, alias through the direct map, unmap"
    need "$log" "\[selftest\] protections: code RX, rodata R, data RW\+NX, null and stack guard unmapped"
    need "$log" "\[selftest\] heap: Box, Vec, String; grew from 1024 KiB to [0-9]+ KiB"
    need "$log" "\[selftest\] memory: all passed"
    need "$log" "\[paging\] the kernel's own tables, read by the kernel and by the CPU: 8 lookups, all the same"
    need "$log" "\[paging\] the CPU refuses a write to code and to constants, and allows one to variables and the heap"
    need "$log" "\[paging\] the low half is off: address 0 and 0x4[0-9a-f]+, where this code really is, translate to nothing"
    need "$log" "\[paging\] one address mapped to the page at 0x4[0-9a-f]+, then to the one at 0x4[0-9a-f]+: the CPU reads the new page there"
    need "$log" "\[kernel\] this is as far as the AArch64 port goes - halting"
    [ "$(grep -ac 'breakpoint at' "$log")" -eq 1 ] || fail "the breakpoint was taken more than once"
    if grep -aEq "PANIC|EXCEPTION|FAILED" "$log"; then
        fail "the log has a panic, an exception or a failed check"
    fi
    grep -a "entered at\|^\[tree\] memory\|^\[tree\] cpus\|^\[tree\] interrupts\|frames free\|top of memory\|selftest\] timer" "$log" | sed 's/^/  /' | cut -c1-110
}

boot plain 1 512 1 2 hvc
MACHINE=,virtualization=on boot el2 2 512 1 2 smc
MACHINE=,virtualization=on,gic-version=3 boot el2v3 2 512 1 3 smc
# 6 GiB: the end of memory is above 4 GiB, a number that needs both of
# the two cells the tree gives it. And five of the six are out of the
# kernel's reach while it builds its page tables: this is the boot that
# has memory handed over in the second go.
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
# The interrupt controller is one the kernel has never heard of (its
# "compatible" is gone): no interrupts, said plainly, and no waiting.
"$HERE/scripts/dtb-without.py" "$OUT/virt.dtb" "$OUT/nogic.dtb" intc@8000000 compatible
QEMU_ARGS="-dtb $OUT/nogic.dtb" boot nogic 1 512 1 none hvc
# The timer's interrupts are not given: the usual number is used.
"$HERE/scripts/dtb-without.py" "$OUT/virt.dtb" "$OUT/notimer.dtb" timer interrupts
TIMER="not described" QEMU_ARGS="-dtb $OUT/notimer.dtb" boot notimer 1 512 1 2 hvc
# The loader keeps memory for itself: the top 16 MiB, and the first
# 64 KiB - which is exactly where the kernel would otherwise put its
# frame bitmap. The kernel must say so, have that much less, find its
# highest free frame below the 16 MiB, and put the bitmap elsewhere.
"$HERE/scripts/dtb-reserve.py" "$OUT/virt.dtb" "$OUT/reserved-top.dtb" 0x5f000000 0x1000000
"$HERE/scripts/dtb-reserve.py" "$OUT/reserved-top.dtb" "$OUT/reserved.dtb" 0x40000000 0x10000
KEPT=16 QEMU_ARGS="-dtb $OUT/reserved.dtb" boot reserved 1 512 1 2 hvc
need "$OUT/reserved.log" "\[tree\] reserved    0x40000000 - 0x40010000"
need "$OUT/reserved.log" "\[tree\] reserved    0x5f000000 - 0x60000000"
need "$OUT/reserved.log" "bitmap 48 KiB at 0x40010000"
# The same at the top of 6 GiB: memory the kernel is only given in the
# second go, where what is reserved has to be taken out a second time.
"$HERE/scripts/dtb-reserve.py" "$OUT/virt.dtb" "$OUT/reserved-high.dtb" 0x1bf000000 0x1000000
KEPT=16 CPU=cortex-a53 MEMORY=6G QEMU_ARGS="-dtb $OUT/reserved-high.dtb" boot reserved-high 1 6144 1 2 hvc
need "$OUT/reserved-high.log" "\[tree\] reserved    0x1bf000000 - 0x1c0000000"

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

fault null "reading through a null pointer" "0x25: data abort" \
    "kernel read at 0x0: not mapped \(the level 0 table has no entry\)"
fault page "forcing a page fault" "0x25: data abort" \
    "kernel read at 0xffffffff00000000: not mapped \(the level 1 table has no entry\)"
# The two below break the rules of the kernel's own image, as its own
# page tables state them: a constant is written to, a variable is run.
fault rodata "writing to read-only data" "0x25: data abort" \
    "kernel write at 0xffffffff800[0-9a-f]+: protection violation \(at level 3\)"
fault nx "executing data" "0x21: instruction abort" \
    "kernel instruction fetch at 0xffffffff800[0-9a-f]+: protection violation \(at level 3\)"

# overflow NAME ANNOUNCE WHOSE WHERE: a kernel that recurses until a
# stack runs out. The report must name the stack, and the address must
# be in the page below it.
overflow() {
    name=$1
    log="$OUT/fault-$name.log"
    echo "aarch64: boot 'fault-$name'"
    "$HERE/scripts/aarch64-boot.sh" "$OUT/Image-fault-$name" "$log" || fail "the boot did not finish"
    need "$log" "\[test\] $2"
    need "$log" "KERNEL STACK OVERFLOW: $3 ran out"
    need "$log" "kernel write at $4: the page kept unmapped below the stack"
    need "$log" "    pc  0xffffffff"
    [ "$(grep -ac 'STACK OVERFLOW\|CPU EXCEPTION\|KERNEL PANIC' "$log")" -eq 1 ] || fail "more than one report"
    if grep -aEq "FAILED|this is as far as" "$log"; then
        fail "the kernel carried on after the overflow"
    fi
    grep -a "STACK OVERFLOW\|kernel write at" "$log" | cut -c1-110
}

overflow stack "overflowing the boot stack" "the boot stack" "0xffffffff80[0-9a-f]+0[0-9a-f]{3}"
overflow thread-stack "overflowing a thread's stack" "a thread's stack" "0xffffd800[0-9a-f]+0[0-9a-f]{3}"

if [ $failed -ne 0 ]; then
    echo "aarch64-test: FAILED (logs in $OUT)"
    exit 1
fi
echo "aarch64-test: all passed"
