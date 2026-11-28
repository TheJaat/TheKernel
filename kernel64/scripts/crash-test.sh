#!/bin/sh
# crash-test.sh - the crash record (TheTaaJ K188): a kernel that
# panics leaves its last words in memory a reset does not clear; the
# next boot finds them and puts them in the system log.
#
#   usage: crash-test.sh STAGE1_BIN STAGE2_BIN DISK BUILD_DIR
#
# The kernel is built with `fault-panic`: a boot that finds no crash
# record panics on purpose, right after starting init, and resets the
# machine (0xCF9) - QEMU told to reset rather than quit. The next boot,
# in the same memory, finds the record, says so, and goes on as usual;
# `log -s crash` shows the last words, each line `crit`, with the time
# the first kernel began to stop. One CPU: init must not get as far as
# the keyboard before the first boot panics.

set -eu
STAGE1=$1
STAGE2=$2
DISK=$3
BUILD=$4
HERE=$(cd "$(dirname "$0")/.." && pwd)
LOG=$BUILD/crash64.log

fail() {
    echo "FAIL: $1 (log: $LOG)"
    exit 1
}
text() { tr -d '\r' < "$LOG"; }
has() { text | grep -aEq "$1" || fail "$2"; }
keys() { python3 -I "$HERE/scripts/key-names.py" "$1" || fail "no keys for: $1"; }

target="$HERE/target/fault-panic"
(cd "$HERE" && cargo build --release --quiet --features fault-panic --target-dir "$target")
python3 "$HERE/scripts/fill-symbols.py" "$target/x86_64-unknown-none/release/kernel64" > /dev/null
iso_dir="$BUILD/crash64"
rm -rf "$iso_dir"
mkdir -p "$iso_dir/kernel"
cp "$STAGE1" "$STAGE2" "$iso_dir/"
strip --strip-debug -o "$iso_dir/kernel/kernel.elf" "$target/x86_64-unknown-none/release/kernel64"
xorriso -as mkisofs -R -J -b stage1.bin -iso-level 3 -no-emul-boot \
    -boot-load-size 4 -o "$BUILD/crash64.iso" "$iso_dir" 2>/dev/null

echo "Booting x86_64 to panic, reset, and find the crash record (log: $LOG)..."
# Finished at the second boot's idle: the first never gets there.
set +e
DISK=$DISK SMP=1 FINISHED_ONLY='\[kernel\] idle' QEMU_ARGS="-action reboot=reset" \
    KEYS="see:[shell],ready $(keys 'log -s crash') ret ~" \
    "$HERE/scripts/qemu-boot.sh" "$BUILD/crash64.iso" "$LOG" 90 > /dev/null 2> "$LOG.err"
set -e

LINE='^20[0-9][0-9]-[0-9][0-9]-[0-9][0-9] [0-9][0-9]:[0-9][0-9]:[0-9][0-9]\.[0-9]{3} '
has '^\*\*\* KERNEL PANIC: panicked at ' "the first boot did not panic"
has '^\[test\] resetting, for the next boot to find the crash record$' "the first boot did not reset"
has '^\[kernel\] the boot before this one stopped: "\*\*\* KERNEL PANIC: panicked at .*" - its last words \([0-9]+ bytes\) are in the system log \(log -s crash\)$' \
    "the second boot did not find the crash record"
has '^\[test\] the boot before this one left its last words: going on as usual$' "the second boot did not go on"
has "${LINE}crash +crit +\*\*\* KERNEL PANIC: panicked at " "log -s crash did not show the panic, crit, with its time"
has "${LINE}crash +crit +a deliberate panic, for the crash record \(fault-panic\)$" "log -s crash did not show the panic's message"
has "${LINE}crash +crit +\[test\] resetting, for the next boot" "log -s crash did not show the last words to the end"
[ "$(text | grep -acE '^\*\*\* KERNEL PANIC')" -eq 1 ] || fail "the kernel panicked more than once"
echo "the crash record: a panic's last words found by the next boot, in the system log as crit"
