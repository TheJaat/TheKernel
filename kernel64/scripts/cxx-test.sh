#!/bin/sh
# cxx-test.sh - C++ on TheTaaJ: programs cross-compiled with LLVM's
# C++ runtimes (libunwind, libc++abi, libc++ - tools/build-cxx.sh) run
# there.
#
#   usage: cxx-test.sh ARCH IMAGE DISK LOG BIN_DIR
#
#   ARCH     x86_64 or aarch64
#   IMAGE    the ISO (x86_64) or the kernel image (aarch64)
#   DISK     the disk to make and boot with
#   BIN_DIR  where the C++ programs for ARCH were built
#
# The steps:
#
#   cxxlib                     every check of the runtime and the library:
#                              exceptions through frames with destructors,
#                              rethrow, std::exception_ptr, bad_alloc;
#                              virtual functions, dynamic_cast, typeid;
#                              over-aligned new; strings, containers,
#                              algorithms, streams, <format>, <regex>, ...
#                              "c++: N checks, all passed" - and after
#                              main, a destructor's last words
#   cxxlib throw               nobody catches it: std::terminate says
#                              what it was, and abort ends it: SIGABRT
#   cxxhello pear apple pear fig
#                              the first C++ program, its usual lines
#   cxxhello antidisestablishmentarianism
#                              an exception thrown and caught
#   echo done

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
BIN_DIR=$5
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
    python3 -I "$HERE/key-names.py" "$1" || fail "no keys for: $1"
}

rm -f "$DISK"
"$TFSTOOL" mkfs "$DISK" 1024 > /dev/null && truncate -s 4M "$DISK" || fail "cannot make the disk"
for program in cxxlib cxxhello; do
    "$TFSTOOL" put "$DISK" "$program" "$BIN_DIR/$program" > /dev/null || fail "cannot put $program on the disk"
done

steps="$(keys 'cxxlib') ret ~~ $(keys 'cxxlib throw') ret ~"
steps="$steps $(keys 'cxxhello pear apple pear fig') ret ~ $(keys 'cxxhello antidisestablishmentarianism') ret ~"
steps="$steps $(keys 'echo done') ret ~"

echo "Booting $ARCH to run C++ programs (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" SLOTS=2 QEMU_ARGS="-smp 2 -drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 40
else
    DISK="$DISK" SMP=2 KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 40
fi

said '\[shell\] line: "cxxlib"' "the keys did not reach the shell"
said '^c\+\+: [0-9]+ checks, all passed' "cxxlib did not pass its checks"
grep -a "cxxlib: FAILED" "$LOG" && fail "cxxlib failed checks"
said '^c\+\+: the objects outside main were destroyed' "the destructors of the objects outside main did not run at exit"
said '"cxxlib" exited with code 0' "cxxlib did not exit with 0"
said "^libc\+\+abi: terminating due to uncaught exception of type std::runtime_error: nobody catches this" \
    "an uncaught exception did not say what it was"
said '"cxxlib" ended by signal 6 \(SIGABRT\)' "an uncaught exception did not end the program by abort's SIGABRT"
said '^Hello from C\+\+' "cxxhello did not say hello"
said '^4 words, 3 different, in order: apple fig pear pear' "cxxhello did not sort its words"
said '^pear 2, apple 1, fig 1' "cxxhello did not count its words"
said '^caught: no word may be longer than 12 letters, and "antidisestablishmentarianism" is 28' \
    "cxxhello did not catch its exception"
said '\[shell\] line: "echo done"' "the shell did not get its keys back"
grep -a 'has not answered for' "$LOG" && fail "a service stopped answering during the test"
count=$(grep -aEo '^c\+\+: [0-9]+ checks' "$LOG" | grep -Eo '[0-9]+')
echo "C++ on TheTaaJ ($ARCH): cxxlib's $count checks passed, the destructors ran after main, an uncaught exception ended in std::terminate, cxxhello sorted, counted and caught"
exit 0
