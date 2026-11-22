#!/bin/sh
# tcc-test.sh - a C compiler running on TheTaaJ: TCC compiles three C
# programs there, which then run there; and afterwards, on the host,
# each program it wrote is compared, byte for byte, with what the same
# TCC built to run on the host (the cross compiler) writes from the
# same source.
#
#   usage: tcc-test.sh ARCH IMAGE DISK LOG TCC_DIR LIBC_A
#
#   ARCH     x86_64 or aarch64
#   IMAGE    the ISO (x86_64) or the kernel image (aarch64)
#   DISK     the disk to make and boot with
#   TCC_DIR  what tools/build-tcc.sh built for ARCH
#   LIBC_A   the C library for ARCH
#
# The disk has what tcc needs, where it looks for it (tools/put-tcc.sh
# puts it there), and the sources: hello.c (printf, a double, a
# struct, a loop), chello.c and clib.c from libc/programs, bad.c (a
# mistake); and welcome and docs/, which clib reads. The steps:
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
#   echo done

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

fail() {
    echo "FAIL: $1 (log: $LOG)"
    exit 1
}

said() {
    grep -aEq "$1" "$LOG" || fail "$2"
}

keys() {
    echo "$1" | sed -e 's/ /_/g' -e 's/./& /g' -e 's/_/spc/g' -e 's/-/minus/g' -e 's/\./dot/g'
}

# ---- the disk ----

WORK=$LOG.work
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
cp "$ROOT/libc/programs/chello.c" "$ROOT/libc/programs/clib.c" "$WORK/"
printf 'Hello from the host\n' > "$WORK/welcome"
printf 'Read me, from the host\n' > "$WORK/readme"

put() {
    "$TFSTOOL" put "$DISK" "$1" "$2" > /dev/null || fail "cannot put $1 on the disk"
}

rm -f "$DISK"
"$TFSTOOL" mkfs "$DISK" 1024 > /dev/null && truncate -s 4M "$DISK" || fail "cannot make the disk"
TFSTOOL="$TFSTOOL" "$ROOT/tools/put-tcc.sh" "$DISK" "$TCC_DIR" "$LIBC_A" || fail "cannot put tcc on the disk"
for file in hello.c chello.c clib.c bad.c welcome; do put "$file" "$WORK/$file"; done
"$TFSTOOL" mkdir "$DISK" docs > /dev/null || fail "cannot make docs on the disk"
put docs/readme "$WORK/readme"

# ---- on TheTaaJ ----

# "~~" after each tcc: wait (up to ninety seconds) for it to be done
# before typing on.
steps="$(keys 'tcc hello.c -o hello') ret ~~ $(keys 'hello') ret ~"
steps="$steps $(keys 'tcc chello.c -o chello2') ret ~~ $(keys 'chello2 one two') ret ~"
steps="$steps $(keys 'tcc -c clib.c') ret ~~ $(keys 'tcc clib.o -o clib2') ret ~~"
steps="$steps $(keys 'with files clib2') ret ~ $(keys 'tcc bad.c') ret ~~"
steps="$steps $(keys 'echo done') ret ~"

echo "Booting $ARCH to compile C programs with tcc, there (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" SLOTS=2 QEMU_ARGS="-smp 2 -drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 40
else
    DISK="$DISK" SMP=2 KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 40
fi

said '\[shell\] line: "tcc hello.c -o hello"' "the keys did not reach the shell"
said '^hello from tcc: 6 \* 7 = 42, 1/8 = 0\.125' "the program tcc compiled on TheTaaJ did not print its line"
said '^Hello from C' "chello, compiled by tcc on TheTaaJ, did not say hello"
said '^argv\[2\] two' "chello, compiled by tcc on TheTaaJ, did not get its arguments"
said '^library: [0-9]+ checks, all passed' "clib, compiled by tcc on TheTaaJ, did not pass its checks"
grep -a "clib: FAILED" "$LOG" && fail "clib, compiled by tcc on TheTaaJ, failed checks"
said '^bad\.c:3: error: .nothing. undeclared' "tcc did not say what is wrong in bad.c, and where"
said '^tcc exited with code 1' "tcc did not fail on bad.c"
[ "$(grep -ac '^tcc exited with code' "$LOG")" -eq 1 ] || fail "tcc failed where it should not have"
said '\[shell\] line: "echo done"' "the shell did not get its keys back"
grep -a 'has not answered for' "$LOG" && fail "a service stopped answering during the test"

# ---- on the host: the same compiler, built to run here ----

# The same command lines, in a directory of the same sources, so that
# the names written into the files (each object's file symbol) are the
# same too.
(cd "$WORK" && "$TCC_DIR/cross/taaj-tcc" hello.c -o hello \
    && "$TCC_DIR/cross/taaj-tcc" chello.c -o chello2 \
    && "$TCC_DIR/cross/taaj-tcc" -c clib.c && "$TCC_DIR/cross/taaj-tcc" clib.o -o clib2) \
    || fail "the cross compiler on the host failed"
for file in hello chello2 clib.o clib2; do
    "$TFSTOOL" get "$DISK" "$file" > "$WORK/$file.got" || fail "$file cannot be read off the disk"
    cmp -s "$WORK/$file" "$WORK/$file.got" \
        || fail "$file, written by tcc on TheTaaJ, is not what the same tcc on the host writes (see $WORK)"
done
echo "tcc on TheTaaJ wrote hello, chello2, clib.o and clib2 byte for byte as tcc on the host does"
exit 0
