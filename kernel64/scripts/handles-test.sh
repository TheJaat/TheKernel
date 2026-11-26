#!/bin/sh
# handles-test.sh - the files lane's boot test, one boot for all of it:
#   - open files as handles (journal entry 115, J60 and J61): a
#     descriptor follows its file through a rename, outlives a removal
#     and a replacing rename, and is handed to a child as the file;
#   - a namespace per process (journal entry 118, J62): a directory
#     mounted at a path of the program's own, and handed to a child;
#   - names from a directory descriptor (journal entry 120, J63): the
#     *at calls, and a child given one directory as its whole tree;
#   - /tmp in memory (journal entry NNN, J64): a second file service,
#     mounted at /tmp everywhere, and nothing of it on the disk.
#
#   usage: handles-test.sh ARCH IMAGE DISK LOG HANDLES NAMESPACE AT TMP
#
# HANDLES, NAMESPACE, AT and TMP are handles.c, namespace.c, at.c and
# tmp.c (next to this script) built for ARCH. One boot; TheTaaJ runs
#   with programs handles     "handles: N checks, all passed"
#   with programs namespace   "namespace: N checks, all passed"
#   with programs at          "at: N checks, all passed"
#   with programs tmptest     "tmp: N checks, all passed"
#   echo tmp-says-hello > /tmp/s, then with programs tmptest show
#   /tmp/s: the shell's /tmp is the programs'

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
HANDLES=$5
NAMESPACE=$6
AT=$7
TMP=$8
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

rm -f "$DISK"
"$TFSTOOL" mkfs "$DISK" 1024 > /dev/null && truncate -s 16M "$DISK" || fail "cannot make the disk"
"$TFSTOOL" put "$DISK" handles "$HANDLES" > /dev/null || fail "cannot put handles on the disk"
"$TFSTOOL" put "$DISK" namespace "$NAMESPACE" > /dev/null || fail "cannot put namespace on the disk"
"$TFSTOOL" put "$DISK" at "$AT" > /dev/null || fail "cannot put at on the disk"
"$TFSTOOL" put "$DISK" tmptest "$TMP" > /dev/null || fail "cannot put tmptest on the disk"

steps="$(keys 'echo == checks ==') ret ~ $(keys 'with programs handles') ret ~~"
steps="$steps $(keys 'with programs namespace') ret ~~"
steps="$steps $(keys 'with programs at') ret ~~"
steps="$steps $(keys 'with programs tmptest') ret ~~"
steps="$steps $(keys 'echo tmp-says-hello > /tmp/s') ret ~ $(keys 'with programs tmptest show /tmp/s') ret ~~"
steps="$steps $(keys 'echo == end ==') ret ~ $(keys 'echo done') ret ~"
echo "Booting $ARCH to check open files as handles (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" QEMU_ARGS="-drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 60
else
    DISK="$DISK" KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 60
fi
WORK=$LOG.clean
tr -d '\r' < "$LOG" > "$WORK"

grep -aq '^== end ==$' "$WORK" || fail "the steps did not all run"
grep -aE '^(handles|namespace|at|tmp): (failed|the sandboxed)' "$WORK" | head -20
# Every check(...) in each program but the function itself.
CHECKS=$(($(grep -c 'check(' "$HERE/handles.c") - 1))
grep -aq "^handles: $CHECKS checks, all passed$" "$WORK" || fail "the $CHECKS checks in handles.c did not all pass"
MOUNTS=$(($(grep -c 'check(' "$HERE/namespace.c") - 1))
grep -aq "^namespace: $MOUNTS checks, all passed$" "$WORK" || fail "the $MOUNTS checks in namespace.c did not all pass"
ATS=$(($(grep -c 'check(' "$HERE/at.c") - 1))
grep -aq "^at: $ATS checks, all passed$" "$WORK" || fail "the $ATS checks in at.c did not all pass"
TMPS=$(($(grep -c 'check(' "$HERE/tmp.c") - 1))
grep -aq "^tmp: $TMPS checks, all passed$" "$WORK" || fail "the $TMPS checks in tmp.c did not all pass"
grep -aq '^\[init\] started: .* fs tmp ' "$WORK" || fail "init did not start /tmp's file service"
grep -aq '^tmp-says-hello$' "$WORK" || fail "the shell's /tmp: tmptest show /tmp/s did not print what echo put there"
# /tmp is in memory: the disk has none of it, not even the directory.
"$TFSTOOL" ls "$DISK" tmp > /dev/null 2>&1 && fail "/tmp's files went to the disk"
grep -aq 'not reached' "$WORK" && fail "$(grep -a 'not reached' "$WORK" | head -1)"
grep -a 'has not answered for' "$WORK" && fail "a service stopped answering during the test"
grep -aiE 'panic|page fault in kernel|CPU EXCEPTION' "$WORK" && fail "the kernel failed"
grep -a 'killed by the kernel' "$WORK" && fail "a program was killed by the kernel"
rm -f "$WORK"
echo "open files as handles ($ARCH): $CHECKS checks of descriptors through renames, removals and a child passed"
echo "a namespace per process ($ARCH): $MOUNTS checks of mounts, .., EXDEV, a read-only mount and a child passed"
echo "names from a directory ($ARCH): $ATS checks of the *at calls, a directory moved while open, and a child in a tree of its own passed"
echo "/tmp in memory ($ARCH): $TMPS checks, the shell's /tmp, and nothing of it on the disk"
