#!/bin/sh
# tcc-test.sh - a C compiler running on TheTaaJ: TCC compiles C
# programs there, which then run there - a few on their own, then the C
# library and all of TheTaaJ's C programs by a build script, then a
# program typed into the editor, and TCC building itself. Afterwards, on the host, each file it
# wrote is compared, byte for byte, with what the same TCC built to run
# on the host (the cross compiler) writes from the same sources.
#
#   usage: tcc-test.sh ARCH IMAGE DISK LOG TCC_DIR LIBC_A
#
#   ARCH     x86_64 or aarch64
#   IMAGE    the ISO (x86_64) or the kernel image (aarch64)
#   DISK     the disk to make and boot with
#   TCC_DIR  what tools/build-tcc.sh built for ARCH
#   LIBC_A   the C library for ARCH
#
# and EDIT in the environment: the editor, built for ARCH.
#
# The disk has what tcc needs, where it looks for it (tools/put-tcc.sh
# puts it there); the C library's and programs' sources in /src, with
# the script that builds them (tools/put-sources.sh); and hello.c
# (printf, a double, a struct, a loop), chello.c and clib.c from
# libc/programs, bad.c (a mistake), and welcome and docs/, which clib
# reads; and TCC's own sources in /src/tcc. The steps:
#
#   tcc hello.c -o hello       compile and link in one go
#   hello                      "hello from tcc: 6 * 7 = 42, 1/8 = 0.125"
#   tcc chello.c -o chello2    chello, the first C program, again
#   chello2 one two            its usual lines
#   tcc -c clib.c              compile only: clib.o
#   tcc clib.o -o clib2        and link it: an object file read back,
#                              libc.a and libtcc1.a searched
#   with files clib2           every check of the C library passes, in
#                              code TCC wrote: setjmp, long doubles, ...
#   tcc bad.c                  the error, where it is, exit code 1
#   source broken              a script whose second command fails: it
#                              stops there, and says so
#   time source /src/build     the C library's C half compiled, put
#                              together with its Rust half into a new
#                              libc.a, and the 11 C programs compiled
#                              and linked with that (libc/build.taaj)
#   cd /, then with files src/libc/programs/clib
#                              clib again, now with a C library TCC
#                              compiled too
#   src/libc/programs/cprimes 100, src/libc/programs/chello built here
#   time source /src/tcc/build TCC builds itself (ports/tcc-ARCH.taaj):
#                              /tcc compiles tcc.c into tcc2, tcc2
#                              compiles it into tcc3, tcc3 compiles
#                              TCC's library, libtcc1.a
#   cd /, then with files src/tcc/tcc3 hello.c -o hello3, and hello3
#                              the third generation compiles and links
#                              a program, which runs
#   echo done
#
# and then, booted again with the same disk:
#
#   edit hi.c                  a program typed in, saved, quit
#   tcc hi.c -o hi             compiled
#   hi > hi.out                run: hi.out, read on the host, is "0.125"
#   echo done
#
# On the host, tcc2 and tcc3 must both be the file the cross compiler
# makes of tcc.c - the compiler compiled by itself is the same as the
# compiler compiled by the one before it - and libtcc1.a, its objects
# and hello3 must be the ones TheTaaJ was given and built before.
#
# The second boot types into the window system's terminal, as a person
# would, so the editor gets the keys too: on AArch64 that machine has a
# screen and a virtio keyboard (as in gui-test.sh), not the serial line.
# What a program prints then goes to the terminal window and not to the
# log - hence hi.out.

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
TCC_DIR=$(cd "$5" && pwd)
LIBC_A=$6
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}
EDIT=${EDIT:?the editor program, built for $ARCH}

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

# ---- the disk ----

# Absolute: the build script on the host changes directory.
WORK=$(cd "$(dirname "$LOG")" && pwd)/$(basename "$LOG").work
rm -rf "$WORK"
mkdir -p "$WORK"
cat > "$WORK/hello.c" <<'EOF'
#include <stdio.h>

struct pair {
    int a, b;
};

int main(void)
{
    struct pair p = {6, 7};
    double eighth = 1.0;
    for (int i = 0; i < 3; i++)
        eighth /= 2;
    printf("hello from tcc: %d * %d = %d, 1/8 = %.3f\n", p.a, p.b, p.a * p.b, eighth);
    return 0;
}
EOF
printf 'int main(void)\n{\n    return nothing;\n}\n' > "$WORK/bad.c"
# A script that fails part way: source must stop at line 3.
printf '# a script with a mistake in it\necho first line\ntcc nofile.c\necho never printed\n' > "$WORK/broken"
cp "$ROOT/libc/programs/chello.c" "$ROOT/libc/programs/clib.c" "$WORK/"
printf 'Hello from the host\n' > "$WORK/welcome"
printf 'Read me, from the host\n' > "$WORK/readme"

put() {
    "$TFSTOOL" put "$DISK" "$1" "$2" > /dev/null || fail "cannot put $1 on the disk"
}

rm -f "$DISK"
"$TFSTOOL" mkfs "$DISK" 4096 > /dev/null && truncate -s 16M "$DISK" || fail "cannot make the disk"
TFSTOOL="$TFSTOOL" "$ROOT/tools/put-tcc.sh" "$DISK" "$TCC_DIR" "$LIBC_A" || fail "cannot put tcc on the disk"
TFSTOOL="$TFSTOOL" "$ROOT/tools/put-sources.sh" "$DISK" "$ARCH" "$TCC_DIR" || fail "cannot put the sources on the disk"
for file in hello.c chello.c clib.c bad.c welcome broken; do put "$file" "$WORK/$file"; done
put edit "$EDIT"
"$TFSTOOL" mkdir "$DISK" docs > /dev/null || fail "cannot make docs on the disk"
put docs/readme "$WORK/readme"

# ---- on TheTaaJ ----

# "~~" after each tcc: wait (up to ninety seconds) for it to be done
# before typing on.
steps="$(keys 'tcc hello.c -o hello') ret ~~ $(keys 'hello') ret ~"
steps="$steps $(keys 'tcc chello.c -o chello2') ret ~~ $(keys 'chello2 one two') ret ~"
steps="$steps $(keys 'tcc -c clib.c') ret ~~ $(keys 'tcc clib.o -o clib2') ret ~~"
steps="$steps $(keys 'with files clib2') ret ~ $(keys 'tcc bad.c') ret ~~ $(keys 'source broken') ret ~~"
# The build: one line for the shell, 23 commands, 70 files compiled.
# Each "~~" waits up to ninety seconds, and returns as soon as the
# command is over, so a spare one costs nothing: with libm the build
# took 640 s on the PC inside make test64, past six of them.
steps="$steps $(keys 'time source /src/build') ret ~~ ~~ ~~ ~~ ~~ ~~ ~~ ~~ ~~ ~~ ~~ ~~"
steps="$steps $(keys 'cd /') ret ~ $(keys 'with files src/libc/programs/clib') ret ~"
steps="$steps $(keys 'src/libc/programs/cprimes 100') ret ~ $(keys 'src/libc/programs/chello built here') ret ~"
# TCC builds itself: two compilations of tcc.c and TCC's library.
steps="$steps $(keys 'time source /src/tcc/build') ret ~~ ~~ ~~ ~~ ~~ ~~ ~~ ~~"
steps="$steps $(keys 'cd /') ret ~ $(keys 'with files src/tcc/tcc3 hello.c -o hello3') ret ~~ $(keys 'hello3') ret ~"
steps="$steps $(keys 'echo done') ret ~"

# The editor: Ctrl+J, not Enter, for the new line - the boot scripts
# count every Enter as a line for the shell. It is window 2, the first
# opened.
edit="$(keys 'edit hi.c') ret see:window,2,open $(keys '#include <stdio.h>') ctrl-j"
edit="$edit $(keys 'int main(void) { printf("%.3f\n", 1.0 / 8); return 0; }') ctrl-s see:saved,hi.c"
edit="$edit ctrl-q see:window,2,closed $(keys 'tcc hi.c -o hi') ret ~~ $(keys 'hi > hi.out') ret ~"
edit="$edit $(keys 'echo done') ret ~"

echo "Booting $ARCH to compile C programs with tcc, there (log: $LOG)..."
# Three slots for two CPUs: the third is for QEMU's disk thread, which
# reading these big programs keeps busy (qemu-boot.sh, SLOTS).
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" SLOTS=3 QEMU_ARGS="-smp 2 -drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 40
else
    DISK="$DISK" SMP=2 SLOTS=3 KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 40
fi
EDIT_LOG=$LOG.edit
echo "Booting $ARCH again, to type a program into the editor and compile it (log: $EDIT_LOG)..."
if [ "$ARCH" = aarch64 ]; then
    SCREEN=1 KEYS="$edit" SLOTS=3 QEMU_ARGS="-smp 2 -drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0 \
-device ramfb -device virtio-keyboard-device -device virtio-tablet-device" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$EDIT_LOG" 40
else
    DISK="$DISK" SMP=2 SLOTS=3 KEYS="$edit" "$HERE/qemu-boot.sh" "$IMAGE" "$EDIT_LOG" 40
fi

said '\[shell\] line: "tcc hello.c -o hello"' "the keys did not reach the shell"
said '^hello from tcc: 6 \* 7 = 42, 1/8 = 0\.125' "the program tcc compiled on TheTaaJ did not print its line"
said '^Hello from C' "chello, compiled by tcc on TheTaaJ, did not say hello"
said '^argv\[2\] two' "chello, compiled by tcc on TheTaaJ, did not get its arguments"
said '^library: [0-9]+ checks, all passed' "clib, compiled by tcc on TheTaaJ, did not pass its checks"
grep -a "clib: FAILED" "$LOG" && fail "clib, compiled by tcc on TheTaaJ, failed checks"
said '^bad\.c:3: error: .nothing. undeclared' "tcc did not say what is wrong in bad.c, and where"
said '^tcc exited with code 1' "tcc did not fail on bad.c"
said '^first line' "source broken did not run its first command"
said '^source: broken: stopped at line 3' "source did not stop at the command that failed"
grep -aq '^never printed' "$LOG" && fail "source went on after a command failed"
[ "$(grep -ac '^tcc exited with code' "$LOG")" -eq 2 ] || fail "tcc failed where it should not have"
said '\[shell\] /src/build line [0-9]+: "tcc -c __math_oflow\.c' "the build script did not start"
said '\[shell\] /src/build line [0-9]+: "tcc -c format\.c' "the build script did not reach the C library's own files"
said '^built: the C library and 11 programs' "the build script did not get to its end"
grep -a "source: /src/build: stopped" "$LOG" && fail "the build script stopped part way"
[ "$(grep -ac '^library: [0-9]* checks, all passed' "$LOG")" -eq 2 ] \
    || fail "clib, with the C library built on TheTaaJ, did not pass its checks"
[ "$(grep -ac '^library: atexit ran its handlers, newest first' "$LOG")" -eq 2 ] \
    || fail "exit did not run clib's atexit handlers, newest first"
said '^cprimes: 25 primes below 100' "cprimes, built on TheTaaJ, did not count"
said '^argv\[2\] here' "chello, built on TheTaaJ, did not get its arguments"
said '\[shell\] /src/tcc/build line [0-9]+: "tcc tcc\.c -o tcc2"' "the script in which tcc builds itself did not start"
said '\[shell\] /src/tcc/build line [0-9]+: "with files \.\./tcc3 -ar rcs libtcc1\.a' "tcc3 did not get to building TCC's library"
said '^built: tcc by tcc, tcc by itself, and its library' "the script in which tcc builds itself did not get to its end"
grep -a "source: /src/tcc/build: stopped" "$LOG" && fail "tcc did not build itself: the script stopped part way"
[ "$(grep -ac '^hello from tcc: 6 \* 7 = 42, 1/8 = 0\.125' "$LOG")" -eq 2 ] \
    || fail "hello3, compiled by the tcc that tcc compiled on TheTaaJ, did not print its line"
said '\[shell\] line: "echo done"' "the shell did not get its keys back"
grep -a 'has not answered for' "$LOG" && fail "a service stopped answering during the test"
grep -aEq '\[edit\] saved hi\.c: 74 bytes, 2 lines' "$EDIT_LOG" || fail "the program typed in the editor was not saved (log: $EDIT_LOG)"
grep -aEq '\[process\] [0-9]+ "hi" exited with code 0' "$EDIT_LOG" || fail "the program typed in the editor did not run (log: $EDIT_LOG)"
grep -aEq '\[shell\] line: "echo done"' "$EDIT_LOG" || fail "the shell did not get its keys back after the editor (log: $EDIT_LOG)"
grep -a 'has not answered for' "$EDIT_LOG" && fail "a service stopped answering during the editor's boot"

# ---- on the host: the same compiler, built to run here ----

# The same command lines, in a directory of the same sources, so that
# the names written into the files (each object's file symbol) are the
# same too.
(cd "$WORK" && "$TCC_DIR/cross/taaj-tcc" hello.c -o hello \
    && "$TCC_DIR/cross/taaj-tcc" chello.c -o chello2 \
    && "$TCC_DIR/cross/taaj-tcc" -c clib.c && "$TCC_DIR/cross/taaj-tcc" clib.o -o clib2) \
    || fail "the cross compiler on the host failed"
# The build script, made to run here: the cross compiler for tcc,
# copies of the sources for /src, the Rust half where build-tcc.sh
# left it.
mkdir -p "$WORK/src/libc/c" "$WORK/src/libc/programs"
for f in "$ROOT"/libc/c/*.c "$ROOT"/libc/c/*.h; do
    case $f in *_test.c) ;; *) cp "$f" "$WORK/src/libc/c/" ;; esac
done
mkdir -p "$WORK/src/libc/c/libm"
cp "$ROOT"/libc/c/libm/*.c "$ROOT"/libc/c/libm/*.h "$WORK/src/libc/c/libm/"
cp "$ROOT"/libc/programs/*.c "$WORK/src/libc/programs/"
sed -e "s|^tcc |$TCC_DIR/cross/taaj-tcc |" -e "s|/src/|$WORK/src/|g" -e "s|/lib/libc-rust.o|$TCC_DIR/libc-rust.o|" \
    "$ROOT/libc/build.taaj" > "$WORK/build.sh"
(sh -e "$WORK/build.sh" > "$WORK/build.out" 2>&1) || fail "the build script failed on the host (see $WORK/build.out)"
# TCC's sources as put-sources.sh puts them (more of them: the others
# are not read), compiled here once: tcc2 and tcc3 on the disk must
# both be this.
mkdir -p "$WORK/src/tcc"
cp "$ROOT"/ports/tcc/*.c "$ROOT"/ports/tcc/*.h "$ROOT"/ports/tcc/*.def "$WORK/src/tcc/"
case $ARCH in aarch64) TARGET=TCC_TARGET_ARM64 ;; *) TARGET=TCC_TARGET_X86_64 ;; esac
{ cat "$TCC_DIR/native/config.h"; echo "#define $TARGET 1"; } > "$WORK/src/tcc/config.h"
(cd "$WORK/src/tcc" && "$TCC_DIR/cross/taaj-tcc" tcc.c -o tcc2) || fail "the cross compiler on the host failed on tcc.c"
cp "$WORK/src/tcc/tcc2" "$WORK/src/tcc/tcc3"
mkdir -p "$WORK/src/tcc/lib"
cp "$TCC_DIR/lib/tcc/libtcc1.a" "$TCC_DIR"/helpers/*.o "$WORK/src/tcc/lib/"
cp "$WORK/hello" "$WORK/hello3"
# What was typed into the editor, and that, compiled here.
printf '#include <stdio.h>\nint main(void) { printf("%%.3f\\n", 1.0 / 8); return 0; }' > "$WORK/hi.c.typed"
"$TFSTOOL" get "$DISK" hi.c > "$WORK/hi.c" || fail "hi.c cannot be read off the disk"
cmp -s "$WORK/hi.c" "$WORK/hi.c.typed" || fail "hi.c on the disk is not what was typed (see $WORK/hi.c)"
(cd "$WORK" && "$TCC_DIR/cross/taaj-tcc" hi.c -o hi) || fail "the cross compiler on the host failed on hi.c"
"$TFSTOOL" get "$DISK" hi.out > "$WORK/hi.out" || fail "hi.out cannot be read off the disk"
[ "$(cat "$WORK/hi.out")" = 0.125 ] || fail "the program typed in the editor printed $(cat "$WORK/hi.out"), not 0.125"

FILES="hello chello2 clib.o clib2 hi src/libc/libc.a"
LIBC_OBJECTS=0
for f in format stdio fcntl ctype stdlib inet float setjmp time abort fp128 math scan locale atexit pthread emutls signal mmap users libm; do
    FILES="$FILES src/libc/c/$f.o"
    LIBC_OBJECTS=$((LIBC_OBJECTS + 1))
done
# libm.o, and the 39 objects of libc/c/libm it was joined from.
for f in "$ROOT"/libc/c/libm/*.c; do FILES="$FILES src/libc/c/libm/$(basename "${f%.c}").o"; done
for p in chello cmem cprint cfile cstdio cwords cask cprimes cnet cfloat clib; do FILES="$FILES src/libc/programs/$p"; done
FILES="$FILES src/tcc/tcc2 src/tcc/tcc3 src/tcc/lib/libtcc1.a hello3"
for o in "$TCC_DIR"/helpers/*.o; do FILES="$FILES src/tcc/lib/$(basename "$o")"; done
count=0
for file in $FILES; do
    "$TFSTOOL" get "$DISK" "$file" > "$WORK/got" || fail "$file cannot be read off the disk"
    cmp -s "$WORK/$file" "$WORK/got" \
        || fail "$file, written by tcc on TheTaaJ, is not what the same tcc on the host writes (see $WORK)"
    count=$((count + 1))
done
echo "tcc on TheTaaJ wrote all $count files byte for byte as tcc on the host does: hello, chello2, clib.o, clib2, hi, and the C library (libc.a, its $LIBC_OBJECTS objects, and the 39 that libm.o joins) and 11 programs built by source /src/build, and tcc2, tcc3 (the same file: TCC compiled by itself), libtcc1.a and its objects, and hello3 built by source /src/tcc/build"
exit 0
