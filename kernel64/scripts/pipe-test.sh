#!/bin/sh
# pipe-test.sh - pipes, redirections and the shell for programs on
# TheTaaJ: a C program gives another a pipe or a file as its input or
# output (posix_spawn's file actions, the file service's streams), system
# runs pipelines with redirections, and hands what needs more to sh;
# popen reads and writes a command, and make runs recipes that use them,
# each through sh, as POSIX says.
#
#   usage: pipe-test.sh ARCH IMAGE DISK LOG PIPES MAKE
#
# PIPES is pipes.c (next to this script) built for ARCH; MAKE pdpmake
# for ARCH; CAT and SH, in the environment, the cat and sh programs for
# ARCH. TheTaaJ runs, in /t:
#
#   with programs pipes      its checks: pipes in one program and between
#                            two, file actions, system - with lists,
#                            variables and the rest, through sh - popen,
#                            the environment passed on - to make too,
#                            names of up to 64 bytes
#                            ("pipes: N checks, all passed")
#   with programs sh -c '...'    a list, typed
#   with programs sh script.sh one 'two words'
#                            a script: its arguments, cd, $(...), test,
#                            a wildcard, ${...}, if, for, while read,
#                            case, a function, $((...)), exec 3<, read
#                            <&3, a here-document, a group's output a
#                            file, set -e
#   with programs sh -c 'a &'   refused: not in this shell yet
#   with programs make       four recipes with <, >, |, 2>&1, ;, $(...)
#                            and a for loop
#   with programs make       again: nothing to do
#   with files make refused  no right to start programs: refused
#
# and the files the recipes wrote, and the long-named ones pipes made,
# are read off the disk afterwards.

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
# Recipes that need pipes and redirections.
all: made-count.txt made-both.txt made-shell.txt made-loop.txt

made-count.txt: in.txt
	pipes count < in.txt > made-count.txt

made-both.txt: in.txt
	pipes emit made by make | pipes count > made-both.txt 2>&1
	pipes oops errors >> made-both.txt 2>&1

made-shell.txt: in.txt
	N=$$(pipes count < in.txt); test -n "$$N" && echo "counted: $$N" > made-shell.txt

made-loop.txt: in.txt
	for w in a b; do echo $$w; done > made-loop.txt

refused:
	pipes emit not run
EOF
printf 'all:\n\tpipes emit from $(WHO)\n' > "$WORK/env.mk"
cat > "$WORK/script.sh" <<'EOF'
# A script for sh on TheTaaJ.
echo "args: $# [$1] [$2] $0"
cd sub && pwd
cd ..
N=$(pipes count < in.txt)
echo "counted: $N"
[ -f in.txt ] && echo "in.txt is a file"
test -d in.txt || echo "in.txt is not a directory"
echo *.mk
pipes emit one two | pipes count
CC=tcc; echo "${CC:-none} ${LD:-none} ${#CC}"
for f in in.txt env.mk none; do
    if [ -f "$f" ]; then echo "$f: there"; else echo "$f: missing"; fi
done
count() { echo "$# arguments"; }
count a "b c"
echo "sum: $((6 * 7))"
f=script.sh; echo "${f%.sh}"
pipes emit x y | while read a b; do echo "read: $b $a"; done
case "$1" in
    one) echo "first is one" ;;
    *) echo "first is not" ;;
esac
exec 3< in.txt
read -r first <&3
echo "from 3: ${#first}"
pipes fdcount 3
while read -r w n; do echo "here: $n $w"; done <<END
one 1
two $((1 + 1))
END
{ echo grouped; pipes emit from-a-program; } > grouped.txt
cat grouped.txt
set -e
pipes exit 3
echo "not reached"
EOF
python3 -I -c 'import sys; sys.stdout.write("".join(chr(97 + n % 26) for n in range(4999)) + "\n")' > "$WORK/in.txt"

put() {
    "$TFSTOOL" put "$DISK" "$1" "$2" > /dev/null || fail "cannot put $1 on the disk"
}

rm -f "$DISK"
"$TFSTOOL" mkfs "$DISK" 2048 > /dev/null && truncate -s 8M "$DISK" || fail "cannot make the disk"
put pipes "$PIPES"
put make "$MAKE"
put cat "$CAT"
put sh "$SH"
"$TFSTOOL" mkdir "$DISK" t > /dev/null || fail "cannot make t on the disk"
for file in Makefile in.txt env.mk script.sh; do put "t/$file" "$WORK/$file"; done

steps="$(keys 'cd t') ret ~"
steps="$steps $(keys 'echo == checks ==') ret ~ $(keys 'with programs pipes') ret ~~ ~~ ~~"
steps="$steps $(keys 'echo == typed ==') ret ~ $(keys "with programs sh -c 'echo a && echo b; false || echo c'") ret ~"
steps="$steps $(keys 'echo == script ==') ret ~ $(keys "with programs sh script.sh one 'two words'") ret ~~"
steps="$steps $(keys 'echo == missing ==') ret ~ $(keys "with programs sh -c 'a &'") ret ~"
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
has typed '^a$' "sh -c did not run the first command of the list"
has typed '^b$' "sh -c did not run the command after &&"
has typed '^c$' "sh -c did not run the command after ||"
has typed '"sh" exited with code 0' "sh -c did not exit with code 0"
for line in 'args: 2 \[one\] \[two words\] script\.sh' '/t/sub' 'counted: 5000 bytes' 'in\.txt is a file' \
    'in\.txt is not a directory' 'env\.mk' '8 bytes' 'tcc none 3' 'in\.txt: there' 'env\.mk: there' 'none: missing' \
    '2 arguments' 'sum: 42' 'script' 'read: y x' 'first is one' 'from 3: 4999' '0 bytes' 'here: 1 one' 'here: 2 two' \
    'grouped' 'from-a-program'; do
    has script "^$line\$" "the script did not say: $line"
done
has script '"sh" exited with code 3' "set -e did not stop the script at pipes exit 3"
part script | grep -aq 'not reached' && fail "the script went on after a failure, with set -e"
has missing '^sh: running a command in the background, &, is not in this shell yet$' "sh did not refuse &"
has missing '"sh" exited with code 2' "sh did not exit with code 2 for &"
has make '"make" exited with code 0' "make did not exit with code 0"
[ "$(part make | grep -ac '\[init\] started pipes')" -eq 5 ] || fail "make did not start exactly the five commands of its recipes"
has again '^make: nothing to be done for all$' "a second make did something"
has refused "^make: \(Makefile:18\): couldn't execute 'pipes emit not run'$" \
    "make, not given the right to start programs, ran a recipe"
grep -aE '"(pipes|make|cat|sh)" killed' "$WORK/log" && fail "a program was killed"
grep -a 'has not answered for' "$WORK/log" && fail "a service stopped answering during the test"

"$TFSTOOL" get "$DISK" t/made-both.txt > "$WORK/both.txt" || fail "t/made-both.txt cannot be read off the disk"
printf '13 bytes\nerrors\n' | cmp -s - "$WORK/both.txt" || fail "t/made-both.txt is not what the recipe wrote: $(cat "$WORK/both.txt")"
"$TFSTOOL" get "$DISK" t/again.txt > "$WORK/again.txt" || fail "t/again.txt cannot be read off the disk"
[ "$(cat "$WORK/again.txt")" = again ] || fail "t/again.txt is not what pipes wrote"
part refused | grep -aq '^not run$' && fail "make, not given the right to start programs, ran a recipe"
"$TFSTOOL" get "$DISK" t/made-count.txt > "$WORK/counted.txt" || fail "t/made-count.txt cannot be read off the disk"
[ "$(cat "$WORK/counted.txt")" = "5000 bytes" ] || fail "t/made-count.txt is not what the recipe wrote"
"$TFSTOOL" get "$DISK" t/made-shell.txt > "$WORK/shell.txt" || fail "t/made-shell.txt cannot be read off the disk"
[ "$(cat "$WORK/shell.txt")" = "counted: 5000 bytes" ] || fail "t/made-shell.txt is not what the recipe wrote: $(cat "$WORK/shell.txt")"
"$TFSTOOL" get "$DISK" t/made-loop.txt > "$WORK/loop.txt" || fail "t/made-loop.txt cannot be read off the disk"
printf 'a\nb\n' | cmp -s - "$WORK/loop.txt" || fail "t/made-loop.txt is not what the recipe's loop wrote: $(cat "$WORK/loop.txt")"
# Names of 40 to 64 bytes, written by pipes.c on TheTaaJ (a version-5
# volume now), read here by the host's tool.
LONG=the-longest-name-a-file-may-have-here-is-sixty-four-bytes-long.c
INSIDE=a-directory-of-forty-bytes-in-its-name.d/a-file-name-of-fifty-bytes-made-for-taajfs-v5.text
[ "$("$TFSTOOL" get "$DISK" "t/$LONG")" = long ] || fail "t/$LONG cannot be read off the disk"
[ "$("$TFSTOOL" get "$DISK" "t/$INSIDE")" = inside ] || fail "t/$INSIDE cannot be read off the disk"
"$TFSTOOL" dump "$DISK" > "$WORK/dump.txt" || fail "the disk does not dump clean"
grep -q '^format version 5:' "$WORK/dump.txt" || fail "the disk did not become version 5"

echo "pipes on TheTaaJ ($ARCH): $CHECKS checks of pipes, file actions, system, sh, popen and the environment passed; sh ran a list and a script with if, for, while read, case, a function, exec 3< and a here-document; make ran recipes with <, >, |, 2>&1, \$(...) and a loop; names of up to 64 bytes on the disk"
