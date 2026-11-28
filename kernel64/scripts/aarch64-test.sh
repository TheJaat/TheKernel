#!/bin/sh
# aarch64-test.sh - builds the AArch64 kernel, boots it in QEMU on
# several machines, and checks what it says.
#
#   usage: aarch64-test.sh BUILD_DIR OBJCOPY
#
# These boots are about the kernel and the machine under it; each ends
# when the kernel has started init and gone idle. What the programs
# then do, with a disk and someone typing, is another test
# (`make aarch64-keys-test`).
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
#   smp4       four CPUs: the others started through the firmware (PSCI),
#              counting together and running threads
#   smp4-v3, smp4-el2, smp4-el2v3
#              the same with the newer interrupt controller (a part of
#              it per CPU), entered at level 2 (the firmware is then
#              called with smc, and every CPU comes down by itself),
#              and both
#   smp8       eight
#   smp20      twenty, of which the kernel runs sixteen and says so
#   dirty      random bytes in the memory the kernel must zero itself
#   unnamed    a device tree that does not say which serial port to use
#   clockless  a device tree that does not give the serial port's clock
#   nogic      a device tree whose interrupt controller is unknown: the
#              kernel must say it has no interrupts, and not wait for one
#   notimer    a device tree that does not give the timer's interrupt
#   disk       a virtio disk and a virtio source of random numbers: the
#              kernel must find both among the thirty-two places the
#              tree describes, and say what each is. Then the disk is
#              driven, by a program: the block driver, with a client
#              that tests it - and afterwards the image file must hold
#              exactly what was written, and nothing else changed
#   disk-v3    the same with the other interrupt controller, entered at
#              exception level 2
#   disk-ro    a disk that may not be written: every write refused, the
#              image untouched
#   disk-level a device tree that says the disk's interrupt line is read
#              by level - the kind that interrupts for ever unless it
#              is switched off until its driver has answered the device
#   modern     the same disk, speaking the newer virtio interface: found,
#              and left alone - no driver speaks that yet
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
# A check that did not hold. The boots run at once, each in a shell of
# its own, so a variable would not reach the end of this script: a
# failure is also a file, named after the boot.
fail() {
    echo "  FAILED: $1"
    failed=1
    if [ -n "${FAILFILE:-}" ]; then : > "$FAILFILE"; fi
}

# job NAME COMMAND...: runs one boot and its checks in the background,
# keeping what it prints for the end, where the reports are shown in
# the order the jobs were started. How many really run at once is not
# decided here: every boot waits for a slot (boot-slots.sh).
names=
job() {
    job_name=$1
    shift
    names="$names $job_name"
    rm -f "$OUT/$job_name.failed"
    ( FAILFILE="$OUT/$job_name.failed"; "$@" ) > "$OUT/$job_name.report" 2>&1 &
}

# need LOG PATTERN: the log must have a line matching the pattern.
need() {
    grep -aEq "$2" "$1" || fail "no line matching: $2"
}

# ---- build ------------------------------------------------------------
(cd "$HERE" && cargo build --release --quiet --target $TARGET)
python3 "$HERE/scripts/fill-symbols.py" "$HERE/target/$TARGET/release/kernel64" > /dev/null
# Put in place in one step: another test may be booting the image that
# is there now (the same one, unless the sources changed in between).
"$OBJCOPY" -O binary "$HERE/target/$TARGET/release/kernel64" "$OUT/Image.$$" && mv -f "$OUT/Image.$$" "$OUT/Image"
for fault in null page rodata nx stack thread-stack; do
    (cd "$HERE" && cargo build --release --quiet --target $TARGET --features fault-$fault --target-dir target/aarch64-fault-$fault)
    python3 "$HERE/scripts/fill-symbols.py" "$HERE/target/aarch64-fault-$fault/$TARGET/release/kernel64" > /dev/null
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
# a few megabytes free (the kernel with the programs built into it, the
# tree and the page tables take the rest), and the highest free frame
# the last one there is. KEPT is how many MiB at the top a test's tree
# reserves.
boot() {
    name=$1
    level=$2
    mib=$3
    cpus=$4
    gic=$5
    call=$6
    log="$OUT/$name.log"
    echo "aarch64: boot '$name'"
    # One boot slot for each CPU the machine has, as far as the kernel
    # will use them: they all run at once.
    if ! SLOTS=$((cpus > 16 ? 16 : cpus)) "$HERE/scripts/aarch64-boot.sh" "$OUT/Image" "$log"; then
        fail "the boot did not finish"
        return
    fi
    need "$log" "^Hello from TheTaaJ!"
    need "$log" "kernel_main at 0xffffffff80[0-9a-f]{6}"
    need "$log" "\[cpu\] exception vectors loaded"
    need "$log" "\[cpu\] breakpoint at 0xffffffff80[0-9a-f]{6} - continuing"
    need "$log" "\[cpu\] resumed after the breakpoint"
    need "$log" "at exception level 1; the loader entered at level $level, with the MMU off"
    need "$log" "the kernel loaded at 0x4[0-9a-f]*80000 and running at 0xffffffff80080000"
    need "$log" "\[tree\] device tree at 0x4[0-9a-f]+: [0-9]+ bytes, version 17, [0-9]+ nodes"
    need "$log" "\[tree\] machine     \"linux,dummy-virt\""
    need "$log" "\[tree\] memory      0x40000000 - 0x[0-9a-f]+ \($mib MiB\)"
    need "$log" "\[tree\] cpus        $cpus, with ids \[0[, 0-9a-f]*\]"
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
        need "$log" "\[selftest\] timer: 10 interrupts in (9[0-9]|[12][0-9][0-9]) ms \(expected 100\)"
        need "$log" "\[selftest\] late interrupts: 35 ms without any, then [0-9]+ came; the tick count moved on by [3-9]"
        need "$log" "\[selftest\] context switch: there and back with a known value in each of 11 registers, none changed"
        need "$log" "\[selftest\] interrupts: 5 taken with a known value in each of 28 registers, none changed"
        need "$log" "\[selftest\] interrupts masked for 35 ms: no tick arrived; unmasked: [3-9] counted at once"
        # The other CPUs: each started by the firmware and saying so
        # itself, all of them counting at once without losing a step
        # under a lock, and then all running threads.
        used=$((cpus > 16 ? 16 : cpus))
        if [ "$used" -gt 1 ]; then
            n=1
            while [ "$n" -lt "$used" ]; do
                need "$log" "\[smp\] CPU $n awake: named 0x[0-9a-f]+, stack below 0xffff[0-9a-f]+"
                n=$((n + 1))
            done
            need "$log" "\[smp\] $used CPUs awake: CPU 1[, 0-9]* started by the firmware \(PSCI [0-9]+\.[0-9]+, asked by $call\)"
            need "$log" "\[smp\] counting together: $used CPUs reached $((used * 100000)) of $((used * 100000)) with a lock"
            need "$log" "\[smp\] $used CPUs run threads: each has its own timer and idle thread, and all take from one queue"
            need "$log" "\[selftest\] smp: threads ran on all $used CPUs at once; 40000 hand-offs between two threads, none lost; nothing leaked"
            need "$log" "\[selftest\] wake-ups: a thread woken by a busy CPU ran on an idle one [0-9]+ us later \(median of 40; slowest [0-9]+ us; [0-9]+ interrupts between CPUs\)"
            if [ "$cpus" -gt 16 ]; then
                need "$log" "\[smp\] $((cpus - 16)) more CPU\(s\) than the 16 this kernel uses: left asleep"
            fi
        else
            need "$log" "\[smp\] 1 CPU: no others to start"
            need "$log" "\[selftest\] smp: 1 CPU; 40000 hand-offs between two threads, none lost; nothing leaked"
        fi
        # Threads, on the same scheduler as the PC: taking turns, being
        # interrupted, sleeping, and giving everything back.
        need "$log" "\[selftest\] threads: two yielding threads ran as \"[ab]+\""
        need "$log" "\[selftest\] threads: a thread that never yields was preempted"
        need "$log" "\[selftest\] threads: sleep\(5\) took [567] ticks; 200 threads spawned and reaped, nothing leaked"
        # Programs, at exception level 0: the ones that keep to the
        # rules end with their own codes, and each of the others is
        # ended for the rule it broke - reading the kernel's memory,
        # masking interrupts, writing to memory it was given to read.
        need "$log" "\[hello\] hello from user mode! I am process 1, running at exception level 0"
        need "$log" "\[hello\] the kernel refused all 5 bad pointers"
        need "$log" "\[process\]   user read at 0xffffffff80100000: protection violation \(at level 3\)"
        need "$log" "\[process\] [0-9]+ \"peek\" killed: data abort in a program"
        need "$log" "\[process\] [0-9]+ \"cli\" killed: trapped system register access"
        need "$log" "\[process\]   user write at 0x100000000000: protection violation \(at level 3\)"
        need "$log" "\[regs\] at the start: 29 registers zero, the stack pointer a multiple of 16"
        need "$log" "\[regs\] 26 registers and the stack pointer, through system calls and timer interrupts for 5 ticks: none changed"
        need "$log" "\[selftest\] user: hello exited 42, spin, heapcheck and regs exited 0, peek, cli and memro were killed; 14 processes, nothing leaked"
        need "$log" "\[selftest\] ipc: echo service and client - rights, transfer, backpressure, crash isolation; 0 channels left"
        need "$log" "\[selftest\] files: fs and fscheck, twice - every check passed, 0 channels and no frames left"
        need "$log" "\[selftest\] kill: spinning, yielding, sleeping, waiting and not yet started - each stopped within 3 ticks; nothing leaked"
        # A disk, if the machine has one: DISK_TEST says what is to
        # happen to the image DISK_IMAGE (6143 sectors of a pattern).
        case "${DISK_TEST:-none}" in
        none)
            need "$log" "\[selftest\] disk: no block device a driver exists for - not run"
            if grep -aq "\[blk\]" "$log"; then fail "a disk driver ran on a machine with no disk for it"; fi
            ;;
        written | untouched)
            if [ "$DISK_TEST" = written ]; then
                did="257 written at 1400 and the last one"; mode=""; checks=348
            else
                did="read-only - every write refused"; mode=", read-only"; checks=340
            fi
            need "$log" "\[virtio\] 0xa003e00 block device: registers in the page at 0xa003000, 192 KiB DMA at 0x4[0-9a-f]+000, interrupt 79, off until its driver waits - ready for a driver"
            need "$log" "\[blk\] virtio disk at 0xa003e00: 6143 sectors \(3071 KiB\)$mode(, a write cache \(flushed on commit\))?, queue of 1024, interrupts"
            need "$log" "\[blkcheck\] $checks checks, all passed: 6143 sectors, $did"
            # Twice: the second driver takes over the device the first left.
            [ "$(grep -ac "^\[blkcheck\] $checks checks, all passed" "$log")" -eq 2 ] || fail "the disk test did not run exactly twice"
            [ "$(grep -ac '"blk" exited with code 0' "$log")" -eq 2 ] || fail "the driver did not end cleanly twice"
            need "$log" "\[selftest\] disk: blk and blkcheck, twice on one device - every check passed; [0-9]{2,} interrupts passed to the driver; nothing leaked"
            "$HERE/scripts/disk-pattern.py" check "$DISK_IMAGE" 6143 "$DISK_TEST" | sed 's/^/  /'
            "$HERE/scripts/disk-pattern.py" check "$DISK_IMAGE" 6143 "$DISK_TEST" > /dev/null 2>&1 || fail "the disk image is not what the test should have left"
            ;;
        unrun)
            # A disk no driver is started for: nothing may touch it.
            if grep -aq "\[blk\]" "$log"; then fail "a disk driver ran on a machine with no disk for it"; fi
            "$HERE/scripts/disk-pattern.py" check "$DISK_IMAGE" 6143 untouched | sed 's/^/  /'
            "$HERE/scripts/disk-pattern.py" check "$DISK_IMAGE" 6143 untouched > /dev/null 2>&1 || fail "the disk image changed"
            ;;
        esac
    fi
    need "$log" "\[tree\] timer       ${TIMER:-interrupts 29 \(secure\), 30 \(physical\), 27 \(virtual\), 26 \(hypervisor\)}"
    if grep -aq "unexpected interrupt" "$log"; then fail "an interrupt arrived that nothing asked for"; fi
    need "$log" "\[tree\] firmware    called with $call"
    [ "$(grep -ac '^\[tree\] memory' "$log")" -eq 1 ] || fail "more than one memory range"
    if [ -z "${CLOCK:-}" ] && grep -aq "as a guess" "$log"; then
        fail "the serial port was guessed though the tree describes it"
    fi
    free=$((mib - ${KEPT:-0}))
    # The highest free frame: below the crash record's 64 KiB, which is
    # at the top of what is free (K188).
    top=$(printf '%x' $((0x40000000 + free * 1048576 - 65536 - 4096)))
    need "$log" "\[memory\] [0-9]+ frames free of $((mib * 256 + 262144)) \(($((free - 4))|$((free - 3))|$((free - 2))|$((free - 1))) MiB\), bitmap [0-9]+ KiB at 0x4"
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
    # The places for virtio devices: all thirty-two looked at, and as
    # many found occupied as this machine was given devices.
    devices=${DEVICES:-0}
    need "$log" "\[virtio\] 32 places at 0xa000000 - 0xa004000: $devices with a device, $((32 - devices)) empty"
    [ "$(grep -acE '^\[virtio\] 0x[0-9a-f]+: ' "$log")" -eq "$devices" ] || fail "not exactly $devices virtio devices reported"
    # The end of the kernel's own part: init started, with the serial
    # port for a terminal - or, with no interrupts, nothing can run.
    if [ "$gic" = none ]; then
        need "$log" "\[kernel\] no programs can run on this machine - halting"
    else
        need "$log" "\[kernel\] serial port at 0x9000000, interrupt 33 \(level\): handed to init, for a driver"
        need "$log" "\[kernel\] boot done; running: "
        need "$log" "\[kernel\] idle - the services take it from here"
    fi
    # What only this boot must also say: ALSO, a pattern on each line.
    if [ -n "${ALSO:-}" ]; then
        printf '%s\n' "$ALSO" | while IFS= read -r pattern; do
            need "$log" "$pattern"
        done
    fi
    [ "$(grep -ac 'breakpoint at' "$log")" -eq 1 ] || fail "the breakpoint was taken more than once"
    if grep -aEq "PANIC|EXCEPTION|FAILED" "$log"; then
        fail "the log has a panic, an exception or a failed check"
    fi
    grep -a "entered at\|^\[tree\] memory\|^\[tree\] cpus\|^\[tree\] interrupts\|frames free\|top of memory\|selftest\] timer" "$log" | sed 's/^/  /' | cut -c1-110
}

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
    # Where it happened, in words: the backtrace names kernel_main (the
    # fault tests run there), from the kernel's own symbols (journal
    # entry 122).
    need "$log" "^    (#[0-3]|lr ) +0x[0-9a-f]+  (kernel64::)?kernel_main\\+0x"
    if grep -aEq "FAILED|\[kernel\] idle|\[kernel\] no programs can run" "$log"; then
        fail "the kernel carried on after the fault"
    fi
    grep -aE "$what" "$log" | cut -c1-110
}

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
    if grep -aEq "FAILED|\[kernel\] idle|\[kernel\] no programs can run" "$log"; then
        fail "the kernel carried on after the overflow"
    fi
    grep -a "STACK OVERFLOW\|kernel write at" "$log" | cut -c1-110
}

# ---- the ordinary boots ----------------------------------------------
job plain boot plain 1 512 1 2 hvc
MACHINE=,virtualization=on job el2 boot el2 2 512 1 2 smc
MACHINE=,virtualization=on,gic-version=3 job el2v3 boot el2v3 2 512 1 3 smc
# 6 GiB: the end of memory is above 4 GiB, a number that needs both of
# the two cells the tree gives it. And five of the six are out of the
# kernel's reach while it builds its page tables: this is the boot that
# has memory handed over in the second go.
CPU=cortex-a53 MEMORY=6G job a53 boot a53 1 6144 1 2 hvc
CPU=max MEMORY=128M MACHINE=,gic-version=3 job max boot max 1 128 1 3 hvc
# More CPUs than one. Four, on each version of the interrupt controller
# (CPUs interrupt each other in a different way on each) and entered
# at each level (the firmware is asked by a different instruction,
# and every CPU has to come down from level 2 itself). Eight: all the
# older controller can have. Twenty: four more than the kernel uses.
QEMU_ARGS="-smp 4" job smp4 boot smp4 1 512 4 2 hvc
MACHINE=,gic-version=3 QEMU_ARGS="-smp 4" job smp4-v3 boot smp4-v3 1 512 4 3 hvc
MACHINE=,virtualization=on QEMU_ARGS="-smp 4" job smp4-el2 boot smp4-el2 2 512 4 2 smc
MACHINE=,virtualization=on,gic-version=3 QEMU_ARGS="-smp 4" job smp4-el2v3 boot smp4-el2v3 2 512 4 3 smc
QEMU_ARGS="-smp 8" job smp8 boot smp8 1 512 8 2 hvc
MACHINE=,gic-version=3 QEMU_ARGS="-smp 20" job smp20 boot smp20 1 512 20 3 hvc

# QEMU's memory starts as zeroes, which is exactly what the kernel's
# first instructions are supposed to make of the memory after the image
# (.bss: its tables, its stack, its variables). So a kernel that forgot
# to would pass every boot above. This one puts random bytes there
# first, as real memory after a restart would have.
bss_start=$(( (file + 4095) / 4096 * 4096 ))
head -c $((size - bss_start)) /dev/urandom > "$OUT/junk.bin"
QEMU_ARGS="-device loader,file=$OUT/junk.bin,addr=$((0x40080000 + bss_start)),force-raw=on" job dirty boot dirty 1 512 1 2 hvc

# Trees with something missing. QEMU writes out the tree it would make
# (dumpdtb), a property is taken out of the copy, and QEMU is given the
# copy to pass on (-dtb).
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512M -display none -machine dumpdtb="$OUT/virt.dtb" 2>/dev/null
# Nobody says which serial port is the console: the first one is used.
"$HERE/scripts/dtb-without.py" "$OUT/virt.dtb" "$OUT/unnamed.dtb" chosen stdout-path
SERIAL="the first one found" QEMU_ARGS="-dtb $OUT/unnamed.dtb" job unnamed boot unnamed 1 512 1 2 hvc
# The serial port's clock is not given: the kernel must say it guessed.
"$HERE/scripts/dtb-without.py" "$OUT/virt.dtb" "$OUT/clockless.dtb" pl011@9000000 clocks
CLOCK="is not given" QEMU_ARGS="-dtb $OUT/clockless.dtb" ALSO="\[tree\] no serial port with a clock described: using the one QEMU's virt machine has, as a guess" job clockless boot clockless 1 512 1 2 hvc
# The interrupt controller is one the kernel has never heard of (its
# "compatible" is gone): no interrupts, said plainly, and no waiting.
"$HERE/scripts/dtb-without.py" "$OUT/virt.dtb" "$OUT/nogic.dtb" intc@8000000 compatible
QEMU_ARGS="-dtb $OUT/nogic.dtb" job nogic boot nogic 1 512 1 none hvc
# The timer's interrupts are not given: the usual number is used.
"$HERE/scripts/dtb-without.py" "$OUT/virt.dtb" "$OUT/notimer.dtb" timer interrupts
TIMER="not described" QEMU_ARGS="-dtb $OUT/notimer.dtb" job notimer boot notimer 1 512 1 2 hvc
# The loader keeps memory for itself: the top 16 MiB, and the first
# 64 KiB - which is exactly where the kernel would otherwise put its
# frame bitmap. The kernel must say so, have that much less, find its
# highest free frame below the 16 MiB, and put the bitmap elsewhere.
"$HERE/scripts/dtb-reserve.py" "$OUT/virt.dtb" "$OUT/reserved-top.dtb" 0x5f000000 0x1000000
"$HERE/scripts/dtb-reserve.py" "$OUT/reserved-top.dtb" "$OUT/reserved.dtb" 0x40000000 0x10000
KEPT=16 QEMU_ARGS="-dtb $OUT/reserved.dtb" ALSO="\[tree\] reserved    0x40000000 - 0x40010000
\[tree\] reserved    0x5f000000 - 0x60000000
bitmap 48 KiB at 0x40010000" job reserved boot reserved 1 512 1 2 hvc
# The same at the top of 6 GiB: memory the kernel is only given in the
# second go, where what is reserved has to be taken out a second time.
"$HERE/scripts/dtb-reserve.py" "$OUT/virt.dtb" "$OUT/reserved-high.dtb" 0x1bf000000 0x1000000
KEPT=16 CPU=cortex-a53 MEMORY=6G QEMU_ARGS="-dtb $OUT/reserved-high.dtb" ALSO="\[tree\] reserved    0x1bf000000 - 0x1c0000000" job reserved-high boot reserved-high 1 6144 1 2 hvc

# Devices. A disk of 3 MiB less one sector - a size no default would
# give - filled with a pattern (disk-pattern.py). QEMU fills the places
# from the last one down, so the disk, named first, is in the last.
# Each boot has an image of its own: QEMU locks the file, the boots run
# at once, and each is looked at afterwards for what its boot did.
disk() {
    "$HERE/scripts/disk-pattern.py" make "$OUT/$1.img" 6143
    echo "-drive file=$OUT/$1.img,if=none,format=raw,id=d0${2:-} -device virtio-blk-device,drive=d0"
}
FOUND="\[virtio\] 0xa003e00: block device \(legacy interface\), 6143 sectors \(3071 KiB\), interrupt 79"
DEVICES=2 DISK_TEST=written DISK_IMAGE="$OUT/disk.img" QEMU_ARGS="$(disk disk) -device virtio-rng-device" ALSO="$FOUND \(edge\)
\[virtio\] 0xa003c00: source of random numbers \(legacy interface\), interrupt 78 \(edge\)" job disk boot disk 1 512 1 2 hvc
# The other interrupt controller: a shared interrupt has to be told
# which CPU to go to in a different way.
DEVICES=1 DISK_TEST=written DISK_IMAGE="$OUT/disk-v3.img" MACHINE=,virtualization=on,gic-version=3 QEMU_ARGS="$(disk disk-v3)" ALSO="$FOUND \(edge\)" job disk-v3 boot disk-v3 2 512 1 3 smc
# A disk that may not be written.
DEVICES=1 DISK_TEST=untouched DISK_IMAGE="$OUT/disk-ro.img" QEMU_ARGS="$(disk disk-ro ,readonly=on)" ALSO="$FOUND \(edge\)" job disk-ro boot disk-ro 1 512 1 2 hvc
# The tree says "level" of the disk's interrupt (the third cell: 4,
# where QEMU writes 1). The device does hold its line up until it is
# answered, so the controller, believing the tree, would interrupt
# again the moment the handler returned - unless the line is off until
# the driver, a program, has answered.
"$HERE/scripts/dtb-set.py" "$OUT/virt.dtb" "$OUT/level.dtb" virtio_mmio@a003e00 interrupts 0 0x2f 4
DEVICES=1 DISK_TEST=written DISK_IMAGE="$OUT/disk-level.img" QEMU_ARGS="-dtb $OUT/level.dtb $(disk disk-level)" ALSO="$FOUND \(level\)" job disk-level boot disk-level 1 512 1 2 hvc
# The same disk on a machine told to offer the newer interface: found,
# said to have no driver, and not touched.
DEVICES=1 DISK_TEST=unrun DISK_IMAGE="$OUT/modern.img" QEMU_ARGS="-global virtio-mmio.force-legacy=false $(disk modern)" ALSO="\[virtio\] 0xa003e00: block device \(modern interface\), 6143 sectors \(3071 KiB\), interrupt 79 \(edge\)
\[virtio\] 0xa003e00 block device: the modern interface - no driver speaks it yet
\[selftest\] disk: no block device a driver exists for - not run" job modern boot modern 1 512 1 2 hvc

# ---- the faults -------------------------------------------------------
job fault-null fault null "reading through a null pointer" "0x25: data abort" \
    "kernel read at 0x0: not mapped \(the level 0 table has no entry\)"
job fault-page fault page "forcing a page fault" "0x25: data abort" \
    "kernel read at 0xffffffff00000000: not mapped \(the level 1 table has no entry\)"
# The two below break the rules of the kernel's own image, as its own
# page tables state them: a constant is written to, a variable is run.
job fault-rodata fault rodata "writing to read-only data" "0x25: data abort" \
    "kernel write at 0xffffffff80[0-9a-f]{6}: protection violation \(at level 3\)"
job fault-nx fault nx "executing data" "0x21: instruction abort" \
    "kernel instruction fetch at 0xffffffff80[0-9a-f]{6}: protection violation \(at level 3\)"

job fault-stack overflow stack "overflowing the boot stack" "the boot stack" "0xffffffff80[0-9a-f]+0[0-9a-f]{3}"
job fault-thread-stack overflow thread-stack "overflowing a thread's stack" "a thread's stack" "0xffffd800[0-9a-f]+0[0-9a-f]{3}"

# ---- the reports ------------------------------------------------------
wait
for name in $names; do
    cat "$OUT/$name.report"
    if [ -e "$OUT/$name.failed" ]; then failed=1; fi
done

if [ $failed -ne 0 ]; then
    echo "aarch64-test: FAILED (logs in $OUT)"
    exit 1
fi
echo "aarch64-test: all passed"
