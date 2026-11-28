#!/bin/sh
# log-test.sh - the system log (TheTaaJ K136): what the kernel and the
# programs write to the debug log, kept by logd in /var/log/system.log with
# the time, the writer and a level, read with `log`, and still there
# after the machine is switched off and started again.
#
#   usage: log-test.sh ISO DISK LOG
#
# Two boots of one PC on one disk (a test disk, make-test-disk). The
# kernel's self-test runs `hello` twice at every boot, before init and
# logd start: its lines show the ring keeps what came before the log
# service, and each boot adds two.
#   first   log -s hello; log -s kernel -n 2; poweroff - the services
#           stopped, fs's last writes on the disk;
#   second  log -a -s logd (both boots' start lines); log -b -s logd
#           (this boot's only); log -a -s hello (both boots' hellos).

set -u
ISO=$1
DISK=$2
LOG=$3
HERE=$(cd "$(dirname "$0")" && pwd)

fail() {
    echo "FAIL: $1"
    exit 1
}
text() { tr -d '\r' < "$1"; }
has() { # has LOG PATTERN WHY
    text "$1" | grep -aEq "$2" || fail "$1: $3"
}
count() { text "$1" | grep -acE "$2"; }
keys() { python3 -I "$HERE/key-names.py" "$1" || fail "no keys for: $1"; }

[ -f "$DISK" ] || fail "no test disk at $DISK"
echo "Booting x86_64 twice on one disk for the system log (logs: $LOG.first, $LOG.second)..."
# A line in the file: its date, the writer, the level, the text.
# ("early boot" before the kernel's clock runs.)
LINE='^(20[0-9][0-9]-[0-9][0-9]-[0-9][0-9] [0-9][0-9]:[0-9][0-9]:[0-9][0-9]\.[0-9]{3}|early boot {13}) '
HELLO="${LINE}hello +info +\[hello\] hello from user mode!"

DISK=$DISK KEYS="$(keys 'log -s hello') ret ~ $(keys 'log -s kernel -n 2') ret ~ $(keys 'ls /var/log/crashes') ret ~ \
$(keys 'log -s peek -n 20') ret ~ $(keys 'cat /var/log/crashes/latest') ret ~ $(keys 'poweroff') ret" \
    SMP=2 "$HERE/qemu-boot.sh" "$ISO" "$LOG.first" 60 > /dev/null 2> "$LOG.first.err"
has "$LOG.first" '^\[kernel\] system log: 256 KiB ring, handed to init$' "the kernel did not hand the system log to init"
has "$LOG.first" '^\[init\] started: .* logd' "init did not start logd"
[ "$(count "$LOG.first" "$HELLO")" -eq 2 ] || fail "$LOG.first: log -s hello did not show the self-test's two hellos, with their time, name and level"
[ "$(count "$LOG.first" "${LINE}kernel +[a-z]+ ")" -ge 2 ] || fail "$LOG.first: log -s kernel -n 2 did not show two of the kernel's lines"
text "$LOG.first" | grep -aE "$LINE" | grep -avE "${LINE}(hello|kernel|peek) +" && fail "$LOG.first: log -s showed another writer's lines"
# Crash reports (K138): the self-test's programs that break rules on
# purpose (peek reads the kernel's code, twice a boot) - a file each,
# in the log as errors, and the newest as `latest` with its last lines.
[ "$(count "$LOG.first" '^ +[0-9]+  20[0-9-]+-[0-9]{6}-peek-[0-9]+$')" -eq 2 ] || fail "$LOG.first: ls /var/log/crashes did not list peek's two reports"
has "$LOG.first" "${LINE}peek +error +crash report: peek \(pid [0-9]+\)$" "log -s peek did not show its crash report"
has "$LOG.first" "${LINE}peek +error +broke a rule: page fault at 0x[0-9a-f]+, reading 0xffffffff80100000: not allowed$" "peek's report did not say what it did"
has "$LOG.first" "${LINE}peek +error +    rip 0x[0-9a-f]{16} +rsp 0x[0-9a-f]{16}" "peek's report did not give its registers"
has "$LOG.first" "${LINE}peek +error +   0x0+400000-0x[0-9a-f]+ r-x its program$" "peek's report did not give its memory"
has "$LOG.first" '^crash report: [a-z]+ \(pid [0-9]+\), 20[0-9-]+ [0-9:.]+$' "cat /var/log/crashes/latest did not show a report"
has "$LOG.first" '^the last lines it printed \([0-9]+\):$' "the latest report did not have the program's last lines"
has "$LOG.first" '^\[init\] stopped logd' "init did not stop logd when switching off"
text "$LOG.first" | grep -aEq 'KERNEL PANIC|CPU EXCEPTION|THE SYSTEM HAS STOPPED|\[logd\] (could not|no system)' && fail "$LOG.first: something failed"

DISK=$DISK KEYS="$(keys 'log -a -s logd') ret ~ $(keys 'log -b -s logd') ret ~ $(keys 'log -a -s hello') ret ~" \
    SMP=2 "$HERE/qemu-boot.sh" "$ISO" "$LOG.second" 40 > /dev/null 2> "$LOG.second.err"
[ "$(count "$LOG.second" "${LINE}logd +info +--- the system started ---$")" -eq 3 ] \
    || fail "$LOG.second: log -a -s logd did not show both boots' start, and log -b this one's only (the first boot's log not kept?)"
[ "$(count "$LOG.second" "$HELLO")" -eq 4 ] || fail "$LOG.second: log -a -s hello did not show both boots' two hellos"
text "$LOG.second" | grep -aEq 'KERNEL PANIC|CPU EXCEPTION|THE SYSTEM HAS STOPPED|\[logd\] (could not|no system)' && fail "$LOG.second: something failed"
echo "the system log: hello's line with its time, name and level; log -s, -n, -a, -b; kept across a switch-off"
