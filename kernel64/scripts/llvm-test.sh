#!/bin/sh
# llvm-test.sh - LLVM on TheTaaJ: llvm-tblgen, cross-compiled from
# ports/llvm by tools/build-llvm.py, runs there and writes what LLVM's
# own build writes first, byte for byte as the host's llvm-tblgen of the
# same release does.
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
# TheTaaJ's are compared with.
#
# The steps:
#
#   llvm-tblgen --version          "TheTaaJ LLVM version 18.1.3"
#   llvm-tblgen -gen-vt            the value types (GenVT.inc): the one
#                                  job CodeGen's types need
#   llvm-tblgen -gen-intrinsic-enums, -gen-intrinsic-impl
#                                  every target's intrinsics, from 25
#                                  .td files (1.9 MB): IntrinsicEnums.inc
#                                  and IntrinsicImpl.inc (3.2 MB)
#   llvm-tblgen bad.td             a mistake: the file, the line, what
#                                  is wrong, and exit code 1
#   echo done

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
"$TFSTOOL" mkfs "$DISK" 8192 > /dev/null && truncate -s 32M "$DISK" || fail "cannot make the disk"
"$TFSTOOL" put "$DISK" llvm-tblgen "$BIN_DIR/llvm-tblgen" > /dev/null || fail "cannot put llvm-tblgen on the disk"

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
steps="$steps $(cat "$WORK/steps") $(keys 'with files llvm-tblgen bad.td') ret ~ $(keys 'echo done') ret ~"

echo "Booting $ARCH to run llvm-tblgen on $count .td files (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" SLOTS=2 QEMU_ARGS="-smp 2 -drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 60
else
    DISK="$DISK" SMP=2 KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 60
fi

said '\[shell\] line: "with readonly llvm-tblgen --version"' "the keys did not reach the shell"
said '^TheTaaJ LLVM version 18\.1\.3' "llvm-tblgen did not say its version"
[ "$(grep -ac '"llvm-tblgen" exited with code 0' "$LOG")" -eq 4 ] \
    || fail "llvm-tblgen did not finish all four of its jobs with exit code 0"
said "^bad\.td:2:[0-9]+: error: Couldn't find class 'Nothing'" "llvm-tblgen did not say what is wrong in bad.td, and where"
said '"llvm-tblgen" exited with code 1' "llvm-tblgen did not fail on bad.td"
said '\[shell\] line: "echo done"' "the shell did not get its keys back"
grep -a '"llvm-tblgen" killed' "$LOG" && fail "llvm-tblgen was killed"
grep -a 'has not answered for' "$LOG" && fail "a service stopped answering during the test"

same=""
for output in $(echo "$JOBS" | cut -d' ' -f1); do
    "$TFSTOOL" get "$DISK" "$output" > "$WORK/got-$output" 2> /dev/null || fail "llvm-tblgen on TheTaaJ wrote no $output"
    cmp -s "$WORK/$output" "$WORK/got-$output" \
        || fail "$output, written by llvm-tblgen on TheTaaJ, is not what the host's llvm-tblgen writes"
    same="$same $output ($(wc -c < "$WORK/$output") bytes)"
done
times=$(grep -aEo '^took [0-9.]+ s' "$LOG" | sed 's/took //' | tr '\n' ' ')
echo "llvm-tblgen on TheTaaJ ($ARCH) read $count .td files and wrote, byte for byte as the host's:$same; took $times"
exit 0
