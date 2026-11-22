#!/bin/sh
# clang-test.sh - Clang on TheTaaJ: clang, cross-compiled from
# ports/llvm by tools/build-llvm.py, compiles C there and writes byte for
# byte what the host's clang of the same release (18.1.3) writes from the
# same files with the same options.
#
#   usage: clang-test.sh ARCH IMAGE DISK LOG BIN_DIR
#
#   ARCH     x86_64 or aarch64
#   IMAGE    the ISO (x86_64) or the kernel image (aarch64)
#   DISK     the disk to make and boot with
#   BIN_DIR  where the LLVM tools for ARCH were built (BIN_DIR/../lib
#            holds the headers clang brings with it)
#
# LLVM_TBLGEN in the environment names the host's LLVM directory, as for
# llvm-test.sh: the host's clang is taken from beside it, and must be
# 18.1.3 too.
#
# The disk is laid out as TheTaaJ's clang expects to find it:
#
#   /clang                     the program
#   /lib/clang/18/include      its headers (stddef.h, stdarg.h, the
#                              intrinsics), found from /clang's own
#                              file: TheTaaJ tells a program which it is
#   /include                   the C library's headers
#   /src/*.c                   TheTaaJ's eleven C programs
#   /cflags, /oflags           the options for this machine and the
#                              other, in response files (clang @cflags):
#                              they would not fit on a line the shell
#                              takes, 256 bytes
#   /bad.c                     a mistake
#
# The steps:
#
#   clang --version                     "TheTaaJ clang version 18.1.3"
#   clang -print-resource-dir           "/lib/clang/18"
#   clang @cflags -c src/*.c            all eleven programs, with the
#                                       options TheTaaJ's own build gives
#                                       them (the Makefile's TAAJ_CFLAGS
#                                       for this machine), in one run of
#                                       clang: eleven objects
#   clang @oflags -c src/cwords.c       and for the other machine
#   clang @cflags -S src/cwords.c       assembly language
#   clang @cflags -E -P src/cstdio.c    the C after the preprocessor:
#                                       every header, as it was read
#   clang -c bad.c                      a mistake: where, what, exit 1
#   clang -c nothere.c                  no such file: "clang: error: ...",
#                                       signed with the name it was
#                                       started by (argv[0])
#   echo done
#
# -fno-ident: an object says in .comment which compiler wrote it -
# "TheTaaJ clang version 18.1.3" or "Ubuntu clang version 18.1.3
# (1ubuntu1)" - and -fno-ident leaves that out, so that what is compared
# is what was compiled.

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
RESOURCE=$BIN_DIR/../lib/clang/18/include
WORK=$LOG.work
PROGRAMS="chello cmem cprint cfile cstdio cwords cask cprimes cnet cfloat clib"

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

# The options TheTaaJ's build compiles its C programs with (the
# Makefile's TAAJ_CFLAGS and TAAJ_A64_CFLAGS), the include directory
# where it is on TheTaaJ's disk, or on the host's.
flags() { # flags MACHINE INCLUDE
    case $1 in
        x86_64) echo "--target=x86_64-unknown-none-elf -std=c11 -O2 -Wall -Wextra -Werror -ffreestanding" \
                     "-fno-stack-protector -fno-pic -mno-red-zone -nostdlibinc -isystem $2 -fno-ident" ;;
        aarch64) echo "--target=aarch64-unknown-none-elf -std=c11 -O2 -Wall -Wextra -Werror -ffreestanding" \
                      "-fno-stack-protector -fno-pic -nostdlibinc -isystem $2 -fno-ident" ;;
    esac
}
OTHER=aarch64
[ "$ARCH" = aarch64 ] && OTHER=x86_64

rm -rf "$WORK" "$DISK"
mkdir -p "$WORK/src"
"$HOST_BIN/clang" --version 2>/dev/null | grep -q 'version 18\.1\.3' \
    || fail "the host's clang (in $HOST_BIN) is not 18.1.3, so its output is not what TheTaaJ's must match"
[ -d "$RESOURCE" ] || fail "no headers for clang in $RESOURCE (tools/build-llvm.py puts them there)"

# 49152 blocks, 192 MiB: clang alone is 82 MB (the PC's) or 72 MB (AArch64's).
"$TFSTOOL" mkfs "$DISK" 49152 > /dev/null || fail "cannot make the disk"
"$TFSTOOL" put "$DISK" clang "$BIN_DIR/clang" > /dev/null || fail "cannot put clang on the disk"
put_tree() { # put_tree FROM TO: every file under FROM, at TO on the disk
    for directory in $(cd "$1" && find . -type d | sort); do
        directory=${directory#./}
        [ "$directory" = . ] || "$TFSTOOL" mkdir "$DISK" "$2/$directory" > /dev/null 2>&1
    done
    for file in $(cd "$1" && find . -type f | sort); do
        file=${file#./}
        # TaajFS names are at most 27 bytes; the few of clang's headers
        # with longer names (CUDA's, HIP's, Hexagon's, and one of the
        # PC's AVX-512 headers) are not on the disk. Nothing here
        # includes them.
        if [ "$(basename "$file" | wc -c)" -gt 28 ]; then
            echo "$file" >> "$WORK/left-out"
            continue
        fi
        "$TFSTOOL" put "$DISK" "$2/$file" "$1/$file" > /dev/null || fail "cannot put $2/$file on the disk"
    done
}
for directory in lib lib/clang lib/clang/18 lib/clang/18/include include src; do
    "$TFSTOOL" mkdir "$DISK" $directory > /dev/null || fail "cannot make $directory on the disk"
done
put_tree "$RESOURCE" lib/clang/18/include
put_tree "$ROOT/libc/include" include
sources=""
for program in $PROGRAMS; do
    cp "$ROOT/libc/programs/$program.c" "$WORK/src/"
    "$TFSTOOL" put "$DISK" src/$program.c "$WORK/src/$program.c" > /dev/null || fail "cannot put $program.c on the disk"
    sources="$sources src/$program.c"
done
flags "$ARCH" /include > "$WORK/cflags"
flags "$OTHER" /include > "$WORK/oflags"
"$TFSTOOL" put "$DISK" cflags "$WORK/cflags" > /dev/null || fail "cannot put cflags on the disk"
"$TFSTOOL" put "$DISK" oflags "$WORK/oflags" > /dev/null || fail "cannot put oflags on the disk"
printf 'int f(void) {\n    return x;\n}\n' > "$WORK/bad.c"
"$TFSTOOL" put "$DISK" bad.c "$WORK/bad.c" > /dev/null || fail "cannot put bad.c on the disk"

# What the host's clang writes, from the same names in $WORK: an object
# holds the name of the file it was compiled from.
(
    cd "$WORK" || exit 1
    "$HOST_BIN/clang" $(flags "$ARCH" "$ROOT/libc/include") -c $sources || exit 1
    "$HOST_BIN/clang" $(flags "$OTHER" "$ROOT/libc/include") -c src/cwords.c -o other.o || exit 1
    "$HOST_BIN/clang" $(flags "$ARCH" "$ROOT/libc/include") -S src/cwords.c -o cwords.s || exit 1
    "$HOST_BIN/clang" $(flags "$ARCH" "$ROOT/libc/include") -E -P src/cstdio.c -o cstdio.i || exit 1
) || fail "the host's clang failed on TheTaaJ's C programs"

steps="$(keys 'with readonly clang --version') ret ~~"
steps="$steps $(keys 'with readonly clang -print-resource-dir') ret ~~"
steps="$steps $(keys "time with files clang @cflags -c$sources") ret ~~ ~~ ~~ ~~ ~~ ~~ ~~ ~~"
steps="$steps $(keys "time with files clang @oflags -c src/cwords.c -o other.o") ret ~~ ~~"
steps="$steps $(keys "with files clang @cflags -S src/cwords.c -o cwords.s") ret ~~ ~~"
steps="$steps $(keys "with files clang @cflags -E -P src/cstdio.c -o cstdio.i") ret ~~"
steps="$steps $(keys 'with files clang -c bad.c') ret ~~"
steps="$steps $(keys 'with files clang -c nothere.c') ret ~~"
steps="$steps $(keys 'echo done') ret ~"

echo "Booting $ARCH to compile TheTaaJ's C programs with clang there (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" SLOTS=2 QEMU_ARGS="-smp 2 -drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 60
else
    DISK="$DISK" SMP=2 KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 60
fi

said '\[shell\] line: "with readonly clang --version"' "the keys did not reach the shell"
said '^TheTaaJ clang version 18\.1\.3' "clang did not say its version"
said "^Target: $ARCH-unknown-taaj" "clang did not say it is for TheTaaJ on $ARCH"
# Found from the file it was started from: /clang, so /lib/clang/18.
said '^/lib/clang/18[[:space:]]*$' "clang did not find its headers from its own file (-print-resource-dir)"
[ "$(grep -ac '"clang" exited with code 0' "$LOG")" -eq 6 ] \
    || fail "clang did not finish all six of its jobs with exit code 0"
said "^bad\.c:2:12: error: use of undeclared identifier 'x'" "clang did not say what is wrong in bad.c, and where"
# argv[0] is the name clang was started by, as on the host: "clang:".
said "^clang: error: no such file or directory: 'nothere\.c'" "clang did not sign its complaint about nothere.c with its name"
[ "$(grep -ac '"clang" exited with code 1' "$LOG")" -eq 2 ] || fail "clang did not fail on bad.c and on nothere.c"
said '\[shell\] line: "echo done"' "the shell did not get its keys back"
grep -a '"clang" killed' "$LOG" && fail "clang was killed"
grep -a 'has not answered for' "$LOG" && fail "a service stopped answering during the test"

same=""
for output in $(for program in $PROGRAMS; do echo $program.o; done) other.o cwords.s cstdio.i; do
    "$TFSTOOL" get "$DISK" "$output" > "$WORK/got-$output" 2> /dev/null || fail "clang on TheTaaJ wrote no $output"
    cmp -s "$WORK/$output" "$WORK/got-$output" \
        || fail "$output, written by clang on TheTaaJ, is not what the host's clang writes"
    same="$same $output"
done
times=$(grep -aEo '^took [0-9.]+ s' "$LOG" | sed 's/took //' | tr '\n' ' ')
echo "clang on TheTaaJ ($ARCH) wrote, byte for byte as the host's:$same"
echo "the timed steps took (s): $times"
[ -s "$WORK/left-out" ] && echo "clang's headers not on the disk (names over 27 bytes): $(tr '\n' ' ' < "$WORK/left-out")"
exit 0
