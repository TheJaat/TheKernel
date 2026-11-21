#!/bin/sh
# net-test.sh - the network card's test: boots a machine whose card is
# wired to a program on this host (wire.py), types `niccheck` at its
# shell, and asks both ends what they saw.
#
#   usage: net-test.sh ARCH IMAGE DISK LOG
#
# ARCH is x86_64 (IMAGE is the ISO; the card is on the PCI bus) or
# aarch64 (IMAGE is the kernel; the card is a virtio-mmio device).
# DISK is the test disk, with niccheck on it.
#
# Then the same machine with the cable going nowhere: niccheck must say
# that nobody answers, and send one frame only.

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
HERE=$(dirname "$0")
# `niccheck`, and when it is over `niccheck restart`: the driver made
# to hang, stopped by init, started again.
KEYS="n i c c h e c k ret ~ n i c c h e c k spc r e s t a r t ret ~~"

fail() {
    echo "FAIL: $1 (log: $LOG)"
    exit 1
}

# boot LOG OURS THEIRS: the machine, its card's frames going to UDP
# port OURS on this host and coming from whoever sends to THEIRS.
boot() {
    net="-netdev socket,id=n0,udp=127.0.0.1:$2,localaddr=127.0.0.1:$3"
    if [ "$ARCH" = aarch64 ]; then
        KEYS="$KEYS" QEMU_ARGS="-drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0 $net -device virtio-net-device,netdev=n0" \
            "$HERE/aarch64-boot.sh" "$IMAGE" "$1" 40
    else
        DISK="$DISK" SMP=1 KEYS="$KEYS" QEMU_ARGS="$net -device virtio-net-pci,netdev=n0" \
            "$HERE/qemu-boot.sh" "$IMAGE" "$1" 40
    fi
}

rm -f "$LOG.ports" "$LOG.wire"
"$HERE/wire.py" "$LOG.ports" "$LOG.wire" &
wire=$!
ticks=0
until [ -s "$LOG.ports" ] || [ $ticks -ge 50 ]; do
    sleep 0.1
    ticks=$((ticks + 1))
done
[ -s "$LOG.ports" ] || { kill $wire 2>/dev/null; fail "the other end of the wire did not start"; }
read -r ours theirs < "$LOG.ports"

echo "Booting $ARCH with a network card wired to this host (log: $LOG)..."
boot "$LOG" "$ours" "$theirs"
kill -TERM $wire 2>/dev/null
wait $wire

grep -aq '\[nic\] virtio network card at .*: address ..:..:..:..:..:.., link up' "$LOG" || fail "the card's driver did not start"
grep -aq '\[niccheck\] FAILED' "$LOG" && { grep -a '\[niccheck\] FAILED' "$LOG"; fail "niccheck found a fault"; }
grep -aq '\[niccheck\] [0-9]* checks, all passed: card' "$LOG" || fail "niccheck did not finish"
cat "$LOG.wire" 2>/dev/null
grep -q '^wire: ok' "$LOG.wire" || fail "the other end of the wire saw something wrong"
# Both ends name the card: the address the driver read from it is the
# one its frames carry.
said=$(grep -ao 'all passed: card ..:..:..:..:..:..' "$LOG" | sed 's/.*card //')
grep -q "frames from $said:" "$LOG.wire" || fail "the card says it is $said, and its frames say otherwise"
# What niccheck counted as sent is what arrived here - nothing lost on
# the way out, and nothing sent that was not counted.
# (and two more by the second run, one to each driver.)
sent=$(grep -ao '[0-9]* frames out' "$LOG" | sed 's/ .*//')
grep -q "^wire: ok: $((sent + 2)) frames from" "$LOG.wire" || fail "niccheck counted $sent frames out (and 2 more later), the wire another number"

# The second run: the driver hung, init stopped it and started another,
# and the card worked with that one.
grep -aq '\[nic\] asked to stall' "$LOG" || fail "the driver was not asked to hang"
grep -aq '\[init\] nic has not answered for 2 s - stopping it' "$LOG" || fail "init did not notice a driver that hangs"
grep -aq '\[init\] nic ended with code -1001 - restarting it' "$LOG" || fail "init did not start the driver again"
[ "$(grep -ac '\[nic\] virtio network card at ' "$LOG")" -eq 2 ] || fail "there should have been two drivers, one after the other"
grep -aq '\[niccheck\] restart: [0-9]* checks, all passed' "$LOG" || fail "the card did not work again after its driver was restarted"

echo "Booting it again with the cable going nowhere (log: $LOG.alone)..."
boot "$LOG.alone" "$ours" "$theirs"
grep -aq '\[niccheck\] nobody answers on this network' "$LOG.alone" || { LOG="$LOG.alone"; fail "with nobody on the wire, niccheck did not say so"; }
grep -aq 'niccheck.* exited with code 77' "$LOG.alone" || { LOG="$LOG.alone"; fail "with nobody on the wire, niccheck did not end with code 77"; }
exit 0
