#!/bin/sh
# tcp-test.sh - the TCP test: connections out of the machine and into
# it, from Rust and from C, through a TCP that is not mine.
#
#   usage: tcp-test.sh ARCH IMAGE DISK LOG
#
# ARCH, IMAGE and DISK as for net-test.sh. One boot, the card on QEMU's
# own network; the other side is ordinary sockets on this host
# (tcp-host.py, whose verdict goes to LOG.host).
#
# Typed at the machine's shell:
#
#   tcpecho 7 5 &                   echoes five connections from the host
#   tcpsend HOST ASK hello ...      a line to the host and back
#   tcpsend HOST ASK fill 300000    300000 bytes there and back
#   cnet send HOST ASK hello ...    the same from C, with socket calls
#   tcpsend HOST CLOSED             refused: nothing listens there
#   tcpsend 10.0.2.99 80            nobody has that address
#   cnet serve 7 1                  refused: tcpecho has port 7
#   cnet serve 8000 2               two connections from the host, in C
#   netinfo                         what the service counted
#
# Meanwhile the host connects to tcpecho and to cnet (see tcp-host.py).

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
HERE=$(dirname "$0")

fail() {
    echo "FAIL: $1 (log: $LOG)"
    exit 1
}

# keys WORDS: a command line (letters, digits, dots, spaces) as the
# names of the keys that type it.
keys() {
    echo "$1" | sed -e 's/ /_/g' -e 's/./& /g' -e 's/_/spc/g' -e 's/\./dot/g'
}

said() {
    grep -aEq "$1" "$LOG" || fail "$2"
}

rm -f "$LOG.ports" "$LOG.host" "$LOG"
"$HERE/tcp-host.py" "$LOG.ports" "$LOG.host" "$LOG" &
peer=$!
ticks=0
until [ -s "$LOG.ports" ] || [ $ticks -ge 50 ]; do
    sleep 0.1
    ticks=$((ticks + 1))
done
[ -s "$LOG.ports" ] || { kill $peer 2>/dev/null; fail "the other side (tcp-host.py) did not start"; }
read -r echo_port serve_port ask_port closed_port < "$LOG.ports"

KEYS="$(keys 'tcpecho 7 5') spc shift-7 ret ~ $(keys "tcpsend 10.0.2.2 $ask_port hello from taaj") ret ~ \
$(keys "tcpsend 10.0.2.2 $ask_port fill 300000") ret ~~ $(keys "cnet send 10.0.2.2 $ask_port hello from c") ret ~ \
$(keys "tcpsend 10.0.2.2 $closed_port") ret ~ $(keys 'tcpsend 10.0.2.99 80') ret ~ $(keys 'cnet serve 7 1') ret ~ \
$(keys 'cnet serve 8000 2') ret ~~ $(keys 'netinfo') ret ~"
NETDEV="user,id=n0,hostfwd=tcp:127.0.0.1:$echo_port-:7,hostfwd=tcp:127.0.0.1:$serve_port-:8000"

echo "Booting $ARCH on QEMU's own network, for TCP (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$KEYS" QEMU_ARGS="-drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0 -netdev $NETDEV -device virtio-net-device,netdev=n0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 60
else
    DISK="$DISK" SMP=1 KEYS="$KEYS" QEMU_ARGS="-netdev $NETDEV -device virtio-net-pci,netdev=n0" \
        "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 60
fi
kill -TERM $peer 2>/dev/null
wait $peer
cat "$LOG.host" 2>/dev/null
grep -q '^host: ok: 5 checks; 3 connections from the machine served, 300029 bytes' "$LOG.host" || fail "the host did not see what it should have"

said '^tcpsend: connected to 10\.0\.2\.2:[0-9]+ from port 49152' "tcpsend did not connect from the first free port"
said '^tcpsend: \| hello from taaj' "the line sent to the host did not come back"
said '^tcpsend: 300000 bytes sent, sum ([0-9a-f]{8})' "tcpsend did not send 300000 bytes"
sent=$(grep -ao 'tcpsend: 300000 bytes sent, sum [0-9a-f]*' "$LOG" | sed 's/.* sum //')
said "^tcpsend: 300000 bytes back, sum $sent" "the 300000 bytes did not come back as they were sent"
said '^cnet: connected to 10\.0\.2\.2:[0-9]+ from port 4915[0-9]' "cnet send did not connect"
said '^hello from c' "what cnet sent did not come back"
said '^cnet: 13 bytes back, and the other side is done' "cnet did not read to the end"
said "^tcpsend: 10\.0\.2\.2:$closed_port: nothing is listening on that port there" "a connection to a closed port was not refused"
said '^tcpsend: 10\.0\.2\.99:80: nobody on this network has that address' "a connection to nobody did not say so"
said '^cnet: port 7: another program has that port' "a port that is listened on was given out again"
# (No "^": tcpecho runs in the background, and what it says can land
# in the middle of a line being typed.)
said 'tcpecho: 1000000 bytes echoed to 10\.0\.2\.2:[0-9]+' "tcpecho did not echo the megabyte"
[ "$(grep -ac 'tcpecho: 20000 bytes echoed to 10\.0\.2\.2:[0-9]*' "$LOG")" -eq 3 ] || fail "tcpecho did not echo three connections of 20000 bytes"
said 'tcpecho: 0 bytes echoed to 10\.0\.2\.2' "tcpecho did not end the empty connection"
said '\[process\] [0-9]+ "tcpecho" exited with code 0' "tcpecho did not end after five connections"
[ "$(grep -ac 'cnet: [0-9]* bytes from 10\.0\.2\.2:[0-9]*, answered' "$LOG")" -eq 2 ] || fail "cnet serve did not answer two connections"
said '^tcp +[0-9]+ segments in, [0-9]+ sent again, 0 answered "no such connection"' "netinfo did not count the segments"
grep -a 'broke' "$LOG" && fail "a connection broke"
grep -a 'has not answered for' "$LOG" && fail "a service stopped answering during the test"
exit 0
