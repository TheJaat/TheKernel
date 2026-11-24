#!/bin/sh
# make-test.sh - make on TheTaaJ: pdpmake builds a program of two C
# files and a header with tcc, there, and builds again only what a
# change makes out of date - which it tells by the files' change stamps
# (TaajFS version 4). The program it makes is compared byte for byte
# with what the host's cross tcc makes from the same sources.
#
#   usage: make-test.sh ARCH IMAGE DISK LOG TCC_DIR LIBC_A MAKE
#
# MAKE is pdpmake built for ARCH (make pdpmake64, pdpmake-aarch64);
# APPEND, in the environment, the append program for ARCH, which
# changes the sources between the steps; SH the sh program, which runs
# each recipe ("set -e;" and the command, as POSIX says).
# TheTaaJ runs, in /proj:
#
#   with programs make          tcc -c main.c, tcc -c util.c, the link
#   ./prog                      "prog: 42"
#   with programs make          make: 'prog' is up to date
#   append util.c, make         util.c alone compiled again, and the link
#   append util.h, make         both compiled again (each needs util.h)
#   append main.c, make -n      "tcc -c main.c" said, not run; then make
#                               runs it, and the link
#   append main.c, make -t      "touch main.o", "touch prog": nothing
#                               compiled; then make: up to date
#   with programs make fails    tcc's error; make: failed to build
#                               'bad.o' exit 1 (tcc's), and make exits
#                               with 2
#   with programs make shell    a list, run by sh: "one", then "two"
#   append main.c, with files make
#                               without the right to start programs:
#                               make cannot run tcc

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
cat > "$WORK/Makefile" <<'EOF'
# A program of two files and a header, built with tcc.
CC = tcc

prog: main.o util.o
	$(CC) main.o util.o -o prog

main.o: main.c util.h
	$(CC) -c main.c

util.o: util.c util.h
	$(CC) -c util.c

fails: bad.o

bad.o: bad.c
	$(CC) -c bad.c

shell:
	echo one && echo two
EOF
printf '#include <stdio.h>\n#include "util.h"\n\nint main(void)\n{\n    printf("prog: %%d\\n", twice(21));\n    return 0;\n}\n' > "$WORK/main.c"
printf '#include "util.h"\n\nint twice(int x)\n{\n    return 2 * x;\n}\n' > "$WORK/util.c"
printf 'int twice(int x);\n' > "$WORK/util.h"
printf 'int f(void)\n{\n    return nothing;\n}\n' > "$WORK/bad.c"

put() {
    "$TFSTOOL" put "$DISK" "$1" "$2" > /dev/null || fail "cannot put $1 on the disk"
}

rm -f "$DISK"
"$TFSTOOL" mkfs "$DISK" 4096 > /dev/null && truncate -s 16M "$DISK" || fail "cannot make the disk"
TFSTOOL="$TFSTOOL" "$ROOT/tools/put-tcc.sh" "$DISK" "$TCC_DIR" "$LIBC_A" || fail "cannot put tcc on the disk"
put make "$MAKE"
put append "$APPEND"
put sh "$SH"
"$TFSTOOL" mkdir "$DISK" proj > /dev/null || fail "cannot make proj on the disk"
for file in Makefile main.c util.c util.h bad.c; do put "proj/$file" "$WORK/$file"; done

# Each step is marked in the log by a line of its own, "== NAME ==".
step() { # step NAME COMMAND...
    out="$(keys "echo == $1 ==") ret ~"
    shift
    for command in "$@"; do out="$out $(keys "$command") ret ~~"; done
    echo "$out"
}
steps="$(keys 'cd proj') ret ~"
steps="$steps $(step first 'with programs make' './prog')"
steps="$steps $(step again 'with programs make')"
steps="$steps $(step source 'with files append util.c int unused;' 'with programs make')"
steps="$steps $(step header 'with files append util.h int other(void);' 'with programs make')"
steps="$steps $(step dry 'with files append main.c /* dry */' 'with programs make -n' 'echo == dry-then ==' 'with programs make')"
steps="$steps $(step touch 'with files append main.c /* touch */' 'with programs make -t' 'echo == touch-then ==' 'with programs make')"
steps="$steps $(step fails 'with programs make fails')"
steps="$steps $(step shell 'with programs make shell')"
steps="$steps $(step refused 'with files append main.c /* refused */' 'with files make')"
steps="$steps $(step end './prog')"
steps="$steps $(keys 'echo done') ret ~"

echo "Booting $ARCH to build a program with make and tcc, there (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" SLOTS=3 QEMU_ARGS="-smp 2 -drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 40
else
    DISK="$DISK" SMP=2 SLOTS=3 KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 40
fi

# The log without the serial line's \r, and each step's part of it.
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
compiles() { # compiles NAME: which of main.c and util.c were compiled, and whether linked
    printf '%s %s %s' "$(count "$1" '^tcc -c main\.c$')" "$(count "$1" '^tcc -c util\.c$')" "$(count "$1" '^tcc main\.o util\.o -o prog$')"
}

grep -aq '^== end ==$' "$WORK/log" || fail "the steps did not all run"
[ "$(compiles first)" = "1 1 1" ] || fail "the first make did not compile both files and link them: $(compiles first)"
has first '^prog: 42$' "the program make built did not run"
has again "^make: 'prog' is up to date$" "a second make did not say prog is up to date"
[ "$(compiles again)" = "0 0 0" ] || fail "a second make compiled something: $(compiles again)"
[ "$(compiles source)" = "0 1 1" ] || fail "after util.c changed, make did not compile util.c alone and link: $(compiles source)"
[ "$(compiles header)" = "1 1 1" ] || fail "after util.h changed, make did not compile both: $(compiles header)"
# Only the compile: nothing is run, so prog is still newer than main.o
# (pdpmake on the host says the same).
[ "$(compiles dry)" = "1 0 0" ] || fail "make -n did not say it would compile main.c: $(compiles dry)"
part dry | grep -aq '\[init\] started tcc' && fail "make -n started tcc"
[ "$(compiles dry-then)" = "1 0 1" ] || fail "after make -n, make did not do what it had said: $(compiles dry-then)"
has touch '^touch main\.o$' "make -t did not touch main.o"
has touch '^touch prog$' "make -t did not touch prog"
part touch | grep -aq '\[init\] started tcc' && fail "make -t started tcc"
has touch-then "^make: 'prog' is up to date$" "after make -t, prog was not up to date"
has fails "^bad\.c:3: error: .nothing. undeclared" "tcc did not say what is wrong in bad.c"
has fails "^make: \(Makefile:16\): failed to build 'bad\.o' exit 1$" "make did not say it failed to build bad.o, and tcc's exit code"
has fails '^make exited with code 2$' "make did not exit with code 2 when a command failed"
has shell '^one$' "sh did not run the recipe's first command"
has shell '^two$' "sh did not run the recipe's command after &&"
has shell '"make" exited with code 0' "make did not exit with code 0 after a recipe sh ran"
has refused "^make: \(Makefile:8\): couldn't execute 'tcc -c main\.c'$" "make, not given the right to start programs, started tcc"
part refused | grep -aq '\[init\] started tcc' && fail "make, not given the right to start programs, started tcc"
has end '^prog: 42$' "prog, built and rebuilt by make, did not run at the end"
grep -aE '"(make|tcc|sh)" killed' "$WORK/log" && fail "make, tcc or sh was killed"
grep -a 'has not answered for' "$WORK/log" && fail "a service stopped answering during the test"

# The program and its objects, as the host's tcc makes them from the
# sources as they ended up on the disk.
for file in main.c util.c util.h; do
    "$TFSTOOL" get "$DISK" "proj/$file" > "$WORK/end-$file" || fail "proj/$file cannot be read off the disk"
done
mkdir -p "$WORK/host"
for file in main.c util.c util.h; do cp "$WORK/end-$file" "$WORK/host/$file"; done
(cd "$WORK/host" && "$TCC_DIR/cross/taaj-tcc" -c main.c && "$TCC_DIR/cross/taaj-tcc" -c util.c \
    && "$TCC_DIR/cross/taaj-tcc" main.o util.o -o prog) || fail "the cross compiler on the host failed"
grep -q 'int unused;' "$WORK/host/util.c" && grep -q 'refused' "$WORK/host/main.c" || fail "the appended lines are not in the sources"
# main.o was last built by the dry run's make; the lines appended since
# are comments, which change nothing tcc writes.
for file in main.o util.o prog; do
    "$TFSTOOL" get "$DISK" "proj/$file" > "$WORK/got-$file" || fail "proj/$file cannot be read off the disk"
    cmp -s "$WORK/host/$file" "$WORK/got-$file" || fail "proj/$file, built by make on TheTaaJ, is not what the host's tcc makes"
done
stamps=$("$TFSTOOL" dump "$DISK" | grep -c ' stamp [0-9]')
[ "$stamps" -gt 0 ] || fail "nothing on the disk has a change stamp"

echo "make on TheTaaJ ($ARCH) built prog with tcc, then only what each change put out of date; -n, -t, a failure, a refusal; main.o, util.o and prog byte for byte the host's tcc's"
