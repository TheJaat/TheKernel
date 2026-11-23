#!/bin/sh
# pipe-test.sh - pipes and redirections for programs on TheTaaJ: a C
# program gives another a pipe or a file as its input or output
# (posix_spawn's file actions, the file service's streams), system runs
# pipelines with redirections, popen reads and writes a command, and make
# runs recipes that use them.
#
#   usage: pipe-test.sh ARCH IMAGE DISK LOG PIPES MAKE
#
# PIPES is pipes.c (next to this script) built for ARCH; MAKE pdpmake
# for ARCH; CAT, in the environment, the cat program for ARCH. TheTaaJ
# runs, in /t:
#
#   with programs pipes      its checks: pipes in one program and between
#                            two, file actions, system, popen, the
#                            environment passed on - to make too
#                            ("pipes: N checks, all passed")
#   with programs make       two recipes with <, >, | and 2>&1
#   with programs make       again: nothing to do
#   with files make refused  no right to start programs: refused
#
# and the files the recipes wrote are read off the disk afterwards.

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
PIPES=$5
MAKE=$6
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}
CAT=${CAT:?the cat program, built for $ARCH}

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
# Recipes that need pipes and redirections.
all: made-count.txt made-both.txt

made-count.txt: in.txt
	pipes count < in.txt > made-count.txt

made-both.txt: in.txt
	pipes emit made by make | pipes count > made-both.txt 2>&1
	pipes oops errors >> made-both.txt 2>&1

refused:
	pipes emit not run
EOF
printf 'all:\n\tpipes emit from $(WHO)\n' > "$WORK/env.mk"
python3 -I -c 'import sys; sys.stdout.write("".join(chr(97 + n % 26) for n in range(4999)) + "\n")' > "$WORK/in.txt"

put() {
    "$TFSTOOL" put "$DISK" "$1" "$2" > /dev/null || fail "cannot put $1 on the disk"
}

rm -f "$DISK"
"$TFSTOOL" mkfs "$DISK" 1024 > /dev/null && truncate -s 4M "$DISK" || fail "cannot make the disk"
put pipes "$PIPES"
put make "$MAKE"
put cat "$CAT"
"$TFSTOOL" mkdir "$DISK" t > /dev/null || fail "cannot make t on the disk"
for file in Makefile in.txt env.mk; do put "t/$file" "$WORK/$file"; done

steps="$(keys 'cd t') ret ~"
steps="$steps $(keys 'echo == checks ==') ret ~ $(keys 'with programs pipes') ret ~~ ~~ ~~"
steps="$steps $(keys 'echo == make ==') ret ~ $(keys 'with programs make') ret ~~"
steps="$steps $(keys 'echo == again ==') ret ~ $(keys 'with programs make') ret ~"
steps="$steps $(keys 'echo == refused ==') ret ~ $(keys 'with files make refused') ret ~"
steps="$steps $(keys 'echo == end ==') ret ~ $(keys 'echo done') ret ~"

echo "Booting $ARCH to give programs pipes and files as input and output (log: $LOG)..."
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

grep -aq '^== end ==$' "$WORK/log" || fail "the steps did not all run"
part checks | grep -a '^pipes: failed' | head -5
# Every check(...) in pipes.c but the function itself.
CHECKS=$(($(grep -c 'check(' "$HERE/pipes.c") - 1))
has checks "^pipes: $CHECKS checks, all passed$" "the $CHECKS checks in pipes.c did not all pass"
has checks '"pipes" exited with code 0' "pipes did not exit with code 0"
has make '^pipes count < in\.txt > made-count\.txt$' "make did not run the recipe with < and >"
has make '^pipes emit made by make \| pipes count > made-both\.txt 2>&1$' "make did not run the recipe with a pipe"
has make '"make" exited with code 0' "make did not exit with code 0"
[ "$(part make | grep -ac '\[init\] started pipes')" -eq 4 ] || fail "make did not start exactly the four commands of its recipes"
has again '^make: nothing to be done for all$' "a second make did something"
has refused "^make: \(Makefile:12\): couldn't execute 'pipes emit not run'$" \
    "make, not given the right to start programs, ran a recipe"
grep -aE '"(pipes|make|cat)" killed' "$WORK/log" && fail "a program was killed"
grep -a 'has not answered for' "$WORK/log" && fail "a service stopped answering during the test"

"$TFSTOOL" get "$DISK" t/made-both.txt > "$WORK/both.txt" || fail "t/made-both.txt cannot be read off the disk"
printf '13 bytes\nerrors\n' | cmp -s - "$WORK/both.txt" || fail "t/made-both.txt is not what the recipe wrote: $(cat "$WORK/both.txt")"
"$TFSTOOL" get "$DISK" t/again.txt > "$WORK/again.txt" || fail "t/again.txt cannot be read off the disk"
[ "$(cat "$WORK/again.txt")" = again ] || fail "t/again.txt is not what pipes wrote"
part refused | grep -aq '^not run$' && fail "make, not given the right to start programs, ran a recipe"
"$TFSTOOL" get "$DISK" t/made-count.txt > "$WORK/counted.txt" || fail "t/made-count.txt cannot be read off the disk"
[ "$(cat "$WORK/counted.txt")" = "5000 bytes" ] || fail "t/made-count.txt is not what the recipe wrote"

echo "pipes on TheTaaJ ($ARCH): $CHECKS checks of pipes, file actions, system, popen and the environment passed; make ran recipes with <, >, | and 2>&1"
