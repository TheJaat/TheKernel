#!/bin/sh
# thread-test.sh - threads in a program on TheTaaJ, on 4 CPUs: pthreads
# for real, the kernel's futexes, thread pointers and per-thread errno,
# and a program that ends while some of its threads are waiting.
#
#   usage: thread-test.sh ARCH IMAGE DISK LOG THREADS THREADCHECK
#
# THREADS is threads.c (next to this script) built for ARCH, and
# THREADCHECK the Rust program (user/src/bin/threadcheck.rs). TheTaaJ
# runs:
#
#   with files threads     its checks: create and join, a mutex 8
#                          threads want, the kinds of mutex, a queue
#                          with conditions, a rwlock, once, keys,
#                          _Thread_local and errno for each thread,
#                          200 detached threads, a pipe one thread
#                          reads while another writes, and 4 threads
#                          printing to one FILE
#                          ("threads: N checks, all passed")
#   threads exit           exit(7) from a thread while others wait on a
#                          mutex, a condition and a pipe, and one spins:
#                          the program ends, with 7
#   threads last           main calls pthread_exit; the last thread
#                          ends the program, its printf flushed
#   threads speed          the same work on 1 thread and on 4
#   threadcheck            Rust: 4 threads allocate through one heap and
#                          add under a taaj::sync::Mutex

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
THREADS=$5
THREADCHECK=$6
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
"$TFSTOOL" put "$DISK" threads "$THREADS" > /dev/null || fail "cannot put threads on the disk"
"$TFSTOOL" put "$DISK" threadcheck "$THREADCHECK" > /dev/null || fail "cannot put threadcheck on the disk"

steps="$(keys 'echo == checks ==') ret ~ $(keys 'with files threads') ret ~~ ~~"
steps="$steps $(keys 'echo == exit ==') ret ~ $(keys 'threads exit') ret ~"
steps="$steps $(keys 'echo == last ==') ret ~ $(keys 'threads last') ret ~"
steps="$steps $(keys 'echo == speed ==') ret ~ $(keys 'threads speed') ret ~~"
steps="$steps $(keys 'echo == rust ==') ret ~ $(keys 'threadcheck') ret ~~"
steps="$steps $(keys 'echo == end ==') ret ~ $(keys 'echo done') ret ~"

echo "Booting $ARCH to run a program's threads on 4 CPUs (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" SLOTS=4 QEMU_ARGS="-smp 4 -drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 40
else
    DISK="$DISK" SMP=4 KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 40
fi

tr -d '\r' < "$LOG" > "$WORK/log"
part() { # part NAME: the lines after "== NAME ==", up to the next mark
    awk -v start="== $1 ==" '$0 == start { on = 1; next } /^== [a-z-]+ ==$/ { on = 0 } on' "$WORK/log"
}
has() { # has NAME PATTERN WHY
    part "$1" | grep -aEq "$2" || fail "$3"
}

grep -aq '^== end ==$' "$WORK/log" || fail "the steps did not all run"
part checks | grep -a '^threads: failed' | head -10
# Every check(...) in threads.c but the function itself.
CHECKS=$(($(grep -c 'check(' "$HERE/threads.c") - 1))
has checks "^threads: $CHECKS checks, all passed$" "the $CHECKS checks in threads.c did not all pass"
has checks '"threads" exited with code 0' "threads did not exit with code 0"
has exit '^threads: exit\(7\) from a thread, with others waiting and one spinning$' "the thread did not get to exit(7)"
has exit '"threads" exited with code 7' "exit(7) from a thread did not end the program with 7"
part exit | grep -aq 'not reached' && fail "main went on after another thread's exit"
has last 'threads: the last thread ends the program' "the last thread's printf was not flushed when the program ended"
has last '"threads" exited with code 0' "a program whose main called pthread_exit did not end with 0"
has speed '^threads: the same work: 1 thread' "the speed run did not finish"
has rust '^threadcheck: 4 threads, 20000 additions and as many blocks, all passed$' "threadcheck did not pass"
has rust '"threadcheck" exited with code 0' "threadcheck did not exit with code 0"
grep -aE '"(threads|threadcheck)" killed' "$WORK/log" && fail "a threads program was killed"
grep -a 'has not answered for' "$WORK/log" && fail "a service stopped answering during the test"
grep -aiE 'panic|page fault in kernel' "$WORK/log" && fail "the kernel failed"

part speed | grep -a '^threads: the same work'
echo "threads on TheTaaJ ($ARCH): $CHECKS checks of pthreads on 4 CPUs passed; exit from a thread ended the program while others waited; the last thread ended it after main's pthread_exit; Rust threads shared one heap and a Mutex"
