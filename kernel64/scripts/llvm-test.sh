#!/bin/sh
# llvm-test.sh - LLVM on TheTaaJ: llvm-tblgen, llc and llvm-mc,
# cross-compiled from ports/llvm by tools/build-llvm.py, run there and
# write byte for byte what the host's tools of the same release (23.1.5)
# write: llvm-tblgen what LLVM's own build writes first, llc and
# llvm-mc machine code for both of TheTaaJ's machines.
#
#   usage: llvm-test.sh ARCH IMAGE DISK LOG BIN_DIR
#
#   ARCH     x86_64 or aarch64
#   IMAGE    the ISO (x86_64) or the kernel image (aarch64)
#   DISK     the disk to make and boot with
#   BIN_DIR  where the LLVM tools for ARCH were built
#
# LLVM_TBLGEN in the environment is the host's llvm-tblgen: it says
# which .td files each job reads (its -d list), and writes the files
# TheTaaJ's are compared with. The host's clang, llc and llvm-mc are
# taken from the same directory, and must be 23.1.5 too.
#
# The steps:
#
#   llvm-tblgen --version          "TheTaaJ LLVM version 23.1.5"
#   llvm-tblgen -gen-vt            the value types (GenVT.inc): the one
#                                  job CodeGen's types need
#   llvm-tblgen -gen-intrinsic-enums, -gen-intrinsic-impl
#                                  every target's intrinsics, from 25
#                                  .td files (1.9 MB): IntrinsicEnums.inc
#                                  and IntrinsicImpl.inc (3.2 MB)
#   llvm-tblgen bad.td             a mistake: the file, the line, what
#                                  is wrong, and exit code 1
#   llvm-mc --version
#   llc -O2 x86_64.ll -o x86_64.s  libc/programs/cwords.c, which the host's
#                                  clang turned into LLVM's own language
#                                  for each machine, compiled into
#                                  assembly language for the PC
#   llc -O2 -filetype=obj aarch64.ll -o aarch64.o
#                                  and straight into an object for AArch64
#   llvm-mc ... x86_64.s -o x86_64.o
#                                  the assembly llc just wrote, assembled
#   llvm-mc ... aarch64.s -o mc-aarch64.o
#                                  the host llc's AArch64 assembly too
#   llc bad.ll, llvm-mc bad.s      a mistake each: where, what, exit 1
#   echo done
#
# Both machines compile for both: llc and llvm-mc have the PC's and
# AArch64's code generators in them, wherever they run.

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
BIN_DIR=$5
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}
TBLGEN=${LLVM_TBLGEN:-llvm-tblgen}
HOST_BIN=$(dirname "$(command -v "$TBLGEN")")
LLVM=$ROOT/ports/llvm/llvm
WORK=$LOG.work

fail() {
    echo "FAIL: $1 (log: $LOG)"
    exit 1
}

said() {
    grep -aEq "$1" "$LOG" || fail "$2"
}

keys() {
    python3 -I "$HERE/key-names.py" "$1" || fail "no keys for: $1"
}

# The jobs: name, output, input, options. TheTaaJ runs each from the
# root, with the sources in /src/llvm; the host runs the same in $WORK.
JOBS="vt.inc include/llvm/CodeGen/ValueTypes.td -gen-vt
enums.inc include/llvm/IR/Intrinsics.td -gen-intrinsic-enums
impl.inc include/llvm/IR/Intrinsics.td -gen-intrinsic-impl"

rm -rf "$WORK" "$DISK"
mkdir -p "$WORK"
for tool in clang llc llvm-mc; do
    "$HOST_BIN/$tool" --version 2>/dev/null | grep -q 'version 23\.1\.5' \
        || fail "the host's $tool (in $HOST_BIN) is not LLVM 23.1.5, so its output is not what TheTaaJ's must match"
done
# 32768 blocks, 128 MiB: llc alone is 29 MB (AArch64) or 34 MB (the PC).
"$TFSTOOL" mkfs "$DISK" 32768 > /dev/null || fail "cannot make the disk"
for tool in llvm-tblgen llc llvm-mc; do
    "$TFSTOOL" put "$DISK" $tool "$BIN_DIR/$tool" > /dev/null || fail "cannot put $tool on the disk"
done

# The code: cwords.c in LLVM's language for each machine, from the
# host's clang, as clang -c on TheTaaJ writes it too (clang-test.sh).
# What the host's llc and llvm-mc make of it is what TheTaaJ's must.
for machine in x86_64 aarch64; do
    "$HOST_BIN/clang" --target=$machine-unknown-none-elf -O2 -ffreestanding -nostdlibinc -isystem "$ROOT/libc/include" \
        -S -emit-llvm "$ROOT/libc/programs/cwords.c" -o "$WORK/$machine.ll" || fail "the host's clang failed on cwords.c"
    "$TFSTOOL" put "$DISK" $machine.ll "$WORK/$machine.ll" > /dev/null || fail "cannot put $machine.ll on the disk"
done
"$HOST_BIN/llc" -O2 "$WORK/x86_64.ll" -o "$WORK/x86_64.s" || fail "the host's llc failed"
"$HOST_BIN/llc" -O2 -filetype=obj "$WORK/aarch64.ll" -o "$WORK/aarch64.o" || fail "the host's llc failed"
"$HOST_BIN/llc" -O2 "$WORK/aarch64.ll" -o "$WORK/aarch64.s" || fail "the host's llc failed"
"$HOST_BIN/llvm-mc" -triple=x86_64-unknown-none-elf -filetype=obj "$WORK/x86_64.s" -o "$WORK/x86_64.o" \
    || fail "the host's llvm-mc failed"
"$HOST_BIN/llvm-mc" -triple=aarch64-unknown-none-elf -filetype=obj "$WORK/aarch64.s" -o "$WORK/mc-aarch64.o" \
    || fail "the host's llvm-mc failed"
"$TFSTOOL" put "$DISK" aarch64.s "$WORK/aarch64.s" > /dev/null || fail "cannot put aarch64.s on the disk"
printf 'define i32 @f() {\n  ret i64 0\n}\n' > "$WORK/bad.ll"
printf '\tmovq %%rax, %%rbx\n\tnotaninstruction %%rax\n' > "$WORK/bad.s"
"$TFSTOOL" put "$DISK" bad.ll "$WORK/bad.ll" > /dev/null || fail "cannot put bad.ll on the disk"
"$TFSTOOL" put "$DISK" bad.s "$WORK/bad.s" > /dev/null || fail "cannot put bad.s on the disk"

# What the host writes, and the .td files it read for it: those, and
# the input itself, go on the disk under the same names.
echo "$JOBS" | while read -r output input option; do
    "$TBLGEN" "$option" -I "$LLVM/include" "$LLVM/$input" -o "$WORK/$output" -d "$WORK/$output.d" \
        || fail "the host's llvm-tblgen failed on $input"
    { echo "$LLVM/$input"; tr ' ' '\n' < "$WORK/$output.d" | grep '\.td$'; } >> "$WORK/sources"
done
[ -s "$WORK/sources" ] || fail "the host's llvm-tblgen said nothing"
sort -u "$WORK/sources" | sed "s|^$LLVM/||" > "$WORK/sources.list"
for file in $(cat "$WORK/sources.list"); do
    path=src/llvm
    "$TFSTOOL" mkdir "$DISK" src > /dev/null 2>&1
    "$TFSTOOL" mkdir "$DISK" src/llvm > /dev/null 2>&1
    for part in $(dirname "$file" | tr '/' ' '); do
        path=$path/$part
        "$TFSTOOL" mkdir "$DISK" "$path" > /dev/null 2>&1
    done
    "$TFSTOOL" put "$DISK" "src/llvm/$file" "$LLVM/$file" > /dev/null || fail "cannot put $file on the disk"
done
printf 'class Thing;\ndef something : Nothing;\n' > "$WORK/bad.td"
"$TFSTOOL" put "$DISK" bad.td "$WORK/bad.td" > /dev/null || fail "cannot put bad.td on the disk"
count=$(wc -l < "$WORK/sources.list")

steps="$(keys 'with readonly llvm-tblgen --version') ret ~~"
echo "$JOBS" | while read -r output input option; do
    echo "$(keys "time with files llvm-tblgen $option -I src/llvm/include src/llvm/$input -o $output") ret ~~"
done > "$WORK/steps"
steps="$steps $(cat "$WORK/steps") $(keys 'with files llvm-tblgen bad.td') ret ~"
steps="$steps $(keys 'with readonly llvm-mc --version') ret ~~"
steps="$steps $(keys 'time with files llc -O2 x86_64.ll -o x86_64.s') ret ~~"
steps="$steps $(keys 'time with files llc -O2 -filetype=obj aarch64.ll -o aarch64.o') ret ~~"
steps="$steps $(keys 'time with files llvm-mc -triple=x86_64-unknown-none-elf -filetype=obj x86_64.s -o x86_64.o') ret ~~"
steps="$steps $(keys 'time with files llvm-mc -triple=aarch64-unknown-none-elf -filetype=obj aarch64.s -o mc-aarch64.o') ret ~~"
steps="$steps $(keys 'with files llc bad.ll -o bad.s2') ret ~~"
steps="$steps $(keys 'with files llvm-mc -triple=x86_64-unknown-none-elf -filetype=obj bad.s -o bad.o') ret ~~"
steps="$steps $(keys 'echo done') ret ~"

echo "Booting $ARCH to run llvm-tblgen on $count .td files, and llc and llvm-mc on cwords.c (log: $LOG)..."
# Three slots for two CPUs: the third is for QEMU's disk thread, which
# reading these big programs keeps busy (qemu-boot.sh, SLOTS).
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" SLOTS=3 QEMU_ARGS="-smp 2 -drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 60
else
    DISK="$DISK" SMP=2 SLOTS=3 KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 60
fi

said '\[shell\] line: "with readonly llvm-tblgen --version"' "the keys did not reach the shell"
said '^TheTaaJ LLVM version 23\.1\.5' "llvm-tblgen did not say its version"
[ "$(grep -ac '"llvm-tblgen" exited with code 0' "$LOG")" -eq 4 ] \
    || fail "llvm-tblgen did not finish all four of its jobs with exit code 0"
said "^bad\.td:2:[0-9]+: error: Couldn't find class 'Nothing'" "llvm-tblgen did not say what is wrong in bad.td, and where"
said '"llvm-tblgen" exited with code 1' "llvm-tblgen did not fail on bad.td"
[ "$(grep -ac '^TheTaaJ LLVM version 23\.1\.5' "$LOG")" -ge 2 ] || fail "llvm-mc did not say its version"
[ "$(grep -ac '"llc" exited with code 0' "$LOG")" -eq 2 ] || fail "llc did not compile both files with exit code 0"
[ "$(grep -ac '"llvm-mc" exited with code 0' "$LOG")" -eq 3 ] \
    || fail "llvm-mc did not say its version and assemble both files with exit code 0"
# As the host's llc says it: "llc: error: llc: bad.ll:...", signed with
# the name it was started by (argv[0], which TheTaaJ tells a program).
said "^llc: error: llc: bad\.ll:2:7: error: value doesn't match function result type 'i32'" \
    "llc did not say what is wrong in bad.ll, and where, signed with its name"
said '"llc" exited with code 1' "llc did not fail on bad.ll"
said "^bad\.s:2:2: error: invalid instruction mnemonic 'notaninstruction'" \
    "llvm-mc did not say what is wrong in bad.s, and where"
said '"llvm-mc" exited with code 1' "llvm-mc did not fail on bad.s"
said '\[shell\] line: "echo done"' "the shell did not get its keys back"
grep -aE '"(llvm-tblgen|llc|llvm-mc)" killed' "$LOG" && fail "an LLVM tool was killed"
grep -a 'has not answered for' "$LOG" && fail "a service stopped answering during the test"

same=""
for output in $(echo "$JOBS" | cut -d' ' -f1); do
    "$TFSTOOL" get "$DISK" "$output" > "$WORK/got-$output" 2> /dev/null || fail "llvm-tblgen on TheTaaJ wrote no $output"
    cmp -s "$WORK/$output" "$WORK/got-$output" \
        || fail "$output, written by llvm-tblgen on TheTaaJ, is not what the host's llvm-tblgen writes"
    same="$same $output ($(wc -c < "$WORK/$output") bytes)"
done
code=""
for output in x86_64.s aarch64.o x86_64.o mc-aarch64.o; do
    "$TFSTOOL" get "$DISK" "$output" > "$WORK/got-$output" 2> /dev/null || fail "TheTaaJ's llc or llvm-mc wrote no $output"
    cmp -s "$WORK/$output" "$WORK/got-$output" \
        || fail "$output, written on TheTaaJ, is not what the host's llc and llvm-mc write"
    code="$code $output ($(wc -c < "$WORK/$output") bytes)"
done
times=$(grep -aEo '^took [0-9.]+ s' "$LOG" | sed 's/took //' | tr '\n' ' ')
echo "llvm-tblgen on TheTaaJ ($ARCH) read $count .td files and wrote, byte for byte as the host's:$same"
echo "llc and llvm-mc on TheTaaJ ($ARCH) wrote, byte for byte as the host's:$code"
echo "the timed steps took (s): $times"
exit 0
