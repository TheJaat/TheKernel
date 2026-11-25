#!/bin/sh
# bigmem-test.sh - a bigger machine (Era 9, R1): TheTaaJ with 4 GB of
# memory and 4 CPUs, programs using a lot of it.
#
#   usage: bigmem-test.sh ARCH IMAGE DISK LOG BIGMEM
#
# BIGMEM is bigmem.c (next to this script) built for ARCH. TheTaaJ runs:
#
#   with files bigmem        its checks: reserved memory costs nothing
#                            until touched and munmap gives it back, a
#                            file mapped, read() into untouched pages,
#                            deep stacks (main's 8 MiB, a thread's 64),
#                            60 threads of 8 MiB stacks, a big malloc
#                            ("bigmem: N checks, all passed")
#   bigmem big               touches 3 GiB, and gives it back
#   with programs bigmem limit
#                            lowers its limit to 64 MiB; the program it
#                            starts has it too; touching 256 MiB ends it
#                            ("out of memory")
#   bigmem overflow          a thread runs off its stack: ended
#   bigmem shoot             a page unmapped while another thread, on
#                            another CPU, reads it: the reader faults
#   bigmem report            afterwards: a program's limit is the
#                            machine's again, and all is well

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
BIGMEM=$5
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}

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
"$TFSTOOL" mkfs "$DISK" 1024 > /dev/null && truncate -s 4M "$DISK" || fail "cannot make the disk"
"$TFSTOOL" put "$DISK" bigmem "$BIGMEM" > /dev/null || fail "cannot put bigmem on the disk"

steps="$(keys 'echo == checks ==') ret ~ $(keys 'with files bigmem') ret ~~ ~~"
steps="$steps $(keys 'echo == big ==') ret ~ $(keys 'bigmem big') ret ~~ ~~ ~~"
steps="$steps $(keys 'echo == limit ==') ret ~ $(keys 'with programs bigmem limit') ret ~~"
steps="$steps $(keys 'echo == overflow ==') ret ~ $(keys 'bigmem overflow') ret ~"
steps="$steps $(keys 'echo == shoot ==') ret ~ $(keys 'bigmem shoot') ret ~"
steps="$steps $(keys 'echo == after ==') ret ~ $(keys 'bigmem report') ret ~"
steps="$steps $(keys 'echo == end ==') ret ~ $(keys 'echo done') ret ~"

echo "Booting $ARCH with 4 GB to run programs that use a lot of it (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" SLOTS=4 MEMORY=4G QEMU_ARGS="-smp 4 -drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 60
else
    # The later -m wins over qemu-boot.sh's own.
    DISK="$DISK" SMP=4 KEYS="$steps" QEMU_ARGS="-m 4G" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 60
fi

tr -d '\r' < "$LOG" > "$WORK/log"
part() { # part NAME: the lines after "== NAME ==", up to the next mark
    awk -v start="== $1 ==" '$0 == start { on = 1; next } /^== [a-z-]+ ==$/ { on = 0 } on' "$WORK/log"
}
has() { # has NAME PATTERN WHY
    part "$1" | grep -aEq "$2" || fail "$3"
}

grep -aq '^== end ==$' "$WORK/log" || fail "the steps did not all run"
part checks | grep -a '^bigmem: failed' | head -10
# Every check(...) in bigmem.c but the function itself.
CHECKS=$(($(grep -c 'check(' "$HERE/bigmem.c") - 1))
has checks "^bigmem: $CHECKS checks, all passed$" "the $CHECKS checks in bigmem.c did not all pass"
has checks '"bigmem" exited with code 0' "bigmem did not exit with code 0"
has big '^bigmem: touched 3072 MiB in .*every page kept its word: yes; after munmap: 0 MiB$' "3 GiB were not touched, kept and given back"
has big '"bigmem" exited with code 0' "bigmem big did not exit with code 0"
has limit '^bigmem: limit lowered to 64 MiB, raising it refused$' "the limit was not lowered, or was raised"
has limit '^bigmem: my limit is 64 MiB$' "the program bigmem started did not get its lowered limit"
has limit '"bigmem" killed: out of memory - 64 MiB used, its limit is 64 MiB' "bigmem was not ended at its limit"
has overflow '"bigmem" killed: ' "running off a thread's stack did not end the program"
part overflow | grep -aq 'out of memory' && fail "a stack overflow was taken for running out of memory"
has shoot '"bigmem" killed: ' "the reader of an unmapped page went on"
has after '^bigmem: my limit is [0-9]{4} MiB$' "a program afterwards did not get the machine's limit"
grep -aq 'not reached' "$WORK/log" && fail "$(grep -a 'not reached' "$WORK/log" | head -1)"
grep -a 'has not answered for' "$WORK/log" && fail "a service stopped answering during the test"
grep -aiE 'panic|page fault in kernel|CPU EXCEPTION' "$WORK/log" && fail "the kernel failed"

part big | grep -a '^bigmem: touched'
part checks | grep -a '^bigmem: 60 threads'
echo "a bigger machine ($ARCH, 4 GB): $CHECKS checks of reserved memory, mmap and big stacks passed; 3 GiB touched and given back; a lowered limit was inherited and ended the program at 64 MiB; a stack overflow and a read of a page unmapped on another CPU both ended the program"
