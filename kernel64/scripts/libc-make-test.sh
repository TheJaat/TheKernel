#!/bin/sh
# libc-make-test.sh - TheTaaJ's C library and its C programs built by
# make on TheTaaJ, with tcc: /src/libc/c/Makefile (libc/c/Makefile.taaj,
# put there by tools/put-sources.sh). Then built again, only what a
# change puts out of date; and the shell built there runs. What make
# wrote is compared byte for byte with what the host's cross tcc makes
# from the same sources (by libc/build.taaj, the script the Makefile
# does the same as).
#
#   usage: libc-make-test.sh ARCH IMAGE DISK LOG TCC_DIR LIBC_A MAKE
#
# MAKE is pdpmake built for ARCH; APPEND and SH, in the environment,
# the append and sh programs for ARCH. TheTaaJ runs, in /src/libc/c:
#
#   with programs make          19 + 39 files compiled, libm.o, libc.a,
#                               12 programs (sh one of them)
#   with programs make          nothing to be done
#   append ctype.c, make        ctype.c alone compiled again, libc.a,
#                               and the 12 programs linked again
#   ../programs/sh -c '...'     the shell built there: a loop, $((...))

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
TCC_DIR=$(cd "$5" && pwd)
LIBC_A=$6
MAKE=$7
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}
APPEND=${APPEND:?the append program, built for $ARCH}
SH=${SH:?the sh program, built for $ARCH}
PROGRAMS="chello cmem cprint cfile cstdio cwords cask cprimes cnet cfloat clib sh"

fail() {
    echo "FAIL: $1 (log: $LOG)"
    exit 1
}

keys() {
    python3 -I "$HERE/key-names.py" "$1" || fail "no keys for: $1"
}

WORK=$(cd "$(dirname "$LOG")" && pwd)/$(basename "$LOG").work
rm -rf "$WORK"
mkdir -p "$WORK"

rm -f "$DISK"
"$TFSTOOL" mkfs "$DISK" 4096 > /dev/null && truncate -s 16M "$DISK" || fail "cannot make the disk"
TFSTOOL="$TFSTOOL" "$ROOT/tools/put-tcc.sh" "$DISK" "$TCC_DIR" "$LIBC_A" || fail "cannot put tcc on the disk"
TFSTOOL="$TFSTOOL" "$ROOT/tools/put-sources.sh" "$DISK" "$ARCH" "$TCC_DIR" || fail "cannot put the sources on the disk"
for program in make append sh; do
    case $program in make) from=$MAKE ;; append) from=$APPEND ;; sh) from=$SH ;; esac
    "$TFSTOOL" put "$DISK" "$program" "$from" > /dev/null || fail "cannot put $program on the disk"
done

# Each step is marked in the log by a line of its own, "== NAME ==".
# "~~" waits up to ninety seconds, and returns as soon as the command is
# over: the first make has 76 commands.
long() { # long N: N "~~"
    n=0; while [ $n -lt "$1" ]; do printf ' ~~'; n=$((n + 1)); done
}
step() { # step NAME WAITS COMMAND...
    out="$(keys "echo == $1 ==") ret ~"
    waits=$2
    shift 2
    for command in "$@"; do out="$out $(keys "$command") ret$(long "$waits")"; done
    echo "$out"
}
steps="$(keys 'cd /src/libc/c') ret ~"
steps="$steps $(step first 40 'time with programs make')"
steps="$steps $(step again 2 'with programs make')"
steps="$steps $(step change 20 'with files append ctype.c /* changed */' 'time with programs make')"
steps="$steps $(step shell 2 "with programs ../programs/sh -c 'for i in 1 2; do echo \"built here: \$i \$((i * 21))\"; done'")"
steps="$steps $(keys 'echo == end ==') ret ~"

echo "Booting $ARCH to build the C library and its programs with make, there (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" SLOTS=3 QEMU_ARGS="-smp 2 -drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 40
else
    DISK="$DISK" SMP=2 SLOTS=3 KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 40
fi

tr -d '\r' < "$LOG" > "$WORK/log"
part() { # part NAME: the lines after "== NAME ==", up to the next mark
    awk -v start="== $1 ==" '$0 == start { on = 1; next } /^== [a-z-]+ ==$/ { on = 0 } on' "$WORK/log"
}
has() { # has NAME PATTERN WHY
    part "$1" | grep -aEq "$2" || fail "$3"
}
count() { # count NAME PATTERN: how many lines match
    part "$1" | grep -aEc "$2"
}

grep -aq '^== end ==$' "$WORK/log" || fail "the steps did not all run"
compiled=$(count first '^cd (\.|libm) && tcc -c [a-z0-9_]+\.c$')
[ "$compiled" = 58 ] || fail "the first make compiled $compiled files, not 58 (19 and libm's 39)"
has first '^tcc -ar rcs \.\./libc\.a /lib/libc-rust\.o format\.o ' "the first make did not put libc.a together"
linked=$(count first '^cd \.\./programs && tcc -L\.\. [a-z]+\.c -o [a-z]+$')
[ "$linked" = 12 ] || fail "the first make linked $linked programs, not 12"
has first '"make" exited with code 0' "the first make did not exit with code 0"
has again '^make: nothing to be done for all$' "a second make did not say there is nothing to do"
[ "$(count again ' && tcc |^tcc ')" = 0 ] || fail "a second make ran tcc"
[ "$(count change '^cd \. && tcc -c ctype\.c$')" = 1 ] || fail "after ctype.c changed, make did not compile it"
[ "$(count change '^cd (\.|libm) && tcc -c ')" = 1 ] || fail "after ctype.c changed, make compiled more than ctype.c"
has change '^tcc -ar rcs \.\./libc\.a ' "after ctype.c changed, make did not put libc.a together again"
[ "$(count change '^cd \.\./programs && tcc -L\.\. ')" = 12 ] || fail "after ctype.c changed, make did not link the 12 programs again"
[ "$(count change '^cd libm && tcc -r ')" = 0 ] || fail "after ctype.c changed, make joined libm.o again"
has shell '^built here: 1 21$' "the sh make built did not run its loop"
has shell '^built here: 2 42$' "the sh make built did not run its loop to the end"
grep -aE '"(make|tcc|sh)" killed' "$WORK/log" && fail "make, tcc or sh was killed"
grep -a 'has not answered for' "$WORK/log" && fail "a service stopped answering during the test"

# The same build on the host: build.taaj, with the cross compiler for
# tcc, and sh, which build.taaj does not build.
mkdir -p "$WORK/src/libc/c/libm" "$WORK/src/libc/programs"
for f in "$ROOT"/libc/c/*.c "$ROOT"/libc/c/*.h; do
    case $f in *_test.c) ;; *) cp "$f" "$WORK/src/libc/c/" ;; esac
done
cp "$ROOT"/libc/c/libm/*.c "$ROOT"/libc/c/libm/*.h "$WORK/src/libc/c/libm/"
cp "$ROOT"/libc/programs/*.c "$WORK/src/libc/programs/"
"$TFSTOOL" get "$DISK" src/libc/c/ctype.c > "$WORK/ctype.c" || fail "src/libc/c/ctype.c cannot be read off the disk"
grep -q '/\* changed \*/' "$WORK/ctype.c" || fail "the appended line is not in ctype.c"
# What was appended is a comment, which changes nothing tcc writes.
cp "$WORK/ctype.c" "$WORK/src/libc/c/ctype.c"
sed -e "s|^tcc |$TCC_DIR/cross/taaj-tcc |" -e "s|/src/|$WORK/src/|g" -e "s|/lib/libc-rust.o|$TCC_DIR/libc-rust.o|" \
    "$ROOT/libc/build.taaj" > "$WORK/build.sh"
(sh -e "$WORK/build.sh" > "$WORK/build.out" 2>&1) || fail "the build script failed on the host (see $WORK/build.out)"
(cd "$WORK/src/libc/programs" && "$TCC_DIR/cross/taaj-tcc" -L.. sh.c -o sh) || fail "the cross compiler on the host failed on sh.c"

FILES="src/libc/libc.a"
for f in "$ROOT"/libc/c/*.c; do
    case $f in *_test.c) ;; *) FILES="$FILES src/libc/c/$(basename "${f%.c}").o" ;; esac
done
FILES="$FILES src/libc/c/libm.o src/libc/c/libm/first.o src/libc/c/libm/second.o"
for f in "$ROOT"/libc/c/libm/*.c; do FILES="$FILES src/libc/c/libm/$(basename "${f%.c}").o"; done
for p in $PROGRAMS; do FILES="$FILES src/libc/programs/$p"; done
n=0
for file in $FILES; do
    "$TFSTOOL" get "$DISK" "$file" > "$WORK/got" || fail "$file cannot be read off the disk"
    cmp -s "$WORK/$file" "$WORK/got" || fail "$file, built by make on TheTaaJ, is not what tcc on the host writes (see $WORK)"
    n=$((n + 1))
done
took=$(part first | grep -a '^took' | head -1)

echo "make on TheTaaJ ($ARCH) built the C library and its 12 programs (sh one of them, which then ran), then after one change only ctype.o, libc.a and the links; all $n files byte for byte the host's tcc's ($took)"
