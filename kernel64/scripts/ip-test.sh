#!/bin/sh
# ip-test.sh - the network service's test: ARP, IPv4, ping and UDP,
# against two other implementations of them.
#
#   usage: ip-test.sh ARCH IMAGE DISK LOG
#
# ARCH, IMAGE and DISK as for net-test.sh. Two boots:
#
#   1. The card wired to wire-ip.py, which plays the network's router
#      with headers and checksums made by hand - and sends the machine
#      what a network should not (wrong checksums, packets for others,
#      pieces of packets). Log: LOG; its verdict: LOG.wire
#   2. The card on QEMU's own small network, where the other side is
#      ordinary sockets on this host (udp-host.py). Log: LOG.user; its
#      verdict: LOG.host
#
# In both, the same commands are typed at the machine's shell: an echo
# program on port 7, pings to the router, a datagram to it, a ping to
# an address nobody has; a second program asking for a port that is
# taken, and for one whose program has ended. Last, the card's driver
# is made to hang (`niccheck stall`): init replaces it, and a ping must
# work again - the network service has to notice, and ask the new
# driver for its frames.

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
HERE=$(dirname "$0")
# NIC (x86_64 only): the card, as for net-test.sh - virtio-net-pci, or
# e1000 or e1000e (driven by `e1000`).
DRIVER=nic
case "${NIC:-}" in e1000*) DRIVER=e1000 ;; esac

fail() {
    echo "FAIL: $1 (log: $LOG)"
    exit 1
}

# keys WORDS: a command line (letters, digits, dots, spaces) as the
# names of the keys that type it.
keys() {
    echo "$1" | sed -e 's/ /_/g' -e 's/./& /g' -e 's/_/spc/g' -e 's/\./dot/g'
}

# typed PORT: everything the test types. PORT is where the datagram to
# the router goes.
typed() {
    echo "$(keys 'udpecho') spc shift-7 ret $(keys 'ping 10.0.2.2 2') ret ~ $(keys "udpsend 10.0.2.2 $1 hello from taaj") ret ~" \
         "$(keys 'ping 10.0.2.99 1') ret ~ $(keys 'udpecho 4002 1') ret ~~ $(keys 'netinfo') ret ~" \
         "$(keys 'udpecho 7 1') ret ~ $(keys 'udpecho 4002 0') ret ~" \
         "$(keys 'niccheck stall') ret ~~ $(keys 'ping 10.0.2.2 1') ret ~"
}

# boot LOG NETDEV KEYS
boot() {
    if [ "$ARCH" = aarch64 ]; then
        KEYS="$3" QEMU_ARGS="-drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0 -netdev $2 -device virtio-net-device,netdev=n0" \
            "$HERE/aarch64-boot.sh" "$IMAGE" "$1" 60
    else
        DISK="$DISK" SMP=1 KEYS="$3" QEMU_ARGS="-netdev $2 -device ${NIC:-virtio-net-pci},netdev=n0" \
            "$HERE/qemu-boot.sh" "$IMAGE" "$1" 60
    fi
}

# peer SCRIPT PORTS VERDICT LOG: starts one of the two peers and waits
# for its port numbers.
peer() {
    # The peer reads the machine's log to know when to begin: a log
    # left by an earlier run would tell it to begin before the machine
    # is there.
    rm -f "$2" "$3" "$4"
    "$HERE/$1" "$2" "$3" "$4" &
    peer=$!
    ticks=0
    until [ -s "$2" ] || [ $ticks -ge 50 ]; do
        sleep 0.1
        ticks=$((ticks + 1))
    done
    [ -s "$2" ] || { kill $peer 2>/dev/null; fail "the other side ($1) did not start"; }
}

# said LOG PATTERN WHAT: the machine must have printed PATTERN.
said() {
    grep -aEq "$2" "$1" || { LOG=$1; fail "$3"; }
}

# What both boots must show, whoever is on the other side.
common() {
    said "$1" '\[net\] this machine is 10\.0\.2\.15/24 on card ..:..:..:..:..:.., with its router at 10\.0\.2\.2' "the network service did not start"
    said "$1" "^56 bytes from 10\.0\.2\.2: seq=1 ttl=$2 time" "the first ping to the router was not answered"
    said "$1" "^56 bytes from 10\.0\.2\.2: seq=2 ttl=$2 time" "the second ping to the router was not answered"
    said "$1" '^2 sent to 10\.0\.2\.2, 2 answered' "ping did not count two answers"
    said "$1" '^udpsend: 15 bytes from port 49152 to 10\.0\.2\.2:' "udpsend did not send from the first free port"
    said "$1" '^udpsend: 21 bytes back from 10\.0\.2\.2:[0-9]+: pong: hello from taaj' "the datagram to the router was not answered"
    said "$1" '^10\.0\.2\.99 cannot be reached: seq=1' "a ping to an address nobody has did not say so"
    # (No "^": udpecho runs in the background, and what it says can
    # land in the middle of a line being typed.)
    said "$1" 'udpecho: 1018 bytes from 10\.0\.2\.2:' "udpecho did not get the longest datagram"
    said "$1" '^address  10\.0\.2\.15/24' "netinfo did not say who this machine is"
    said "$1" '^udpecho: port 7: another program has that port' "a port that is taken was given out again"
    [ "$(grep -ac 'udpecho: listening on port 4002' "$1")" -eq 2 ] || { LOG=$1; fail "the port of a program that ended was not free again"; }
    said "$1" '\[niccheck\] stall: the driver was stopped, and another answers' "the card's driver was not replaced"
    said "$1" '\[net\] the card.s driver was replaced: listening to the new one' "the network service did not notice the new driver"
    said "$1" '^1 sent to 10\.0\.2\.2, 1 answered' "with a new driver for the card, a ping was not answered"
    [ "$(grep -ac '\[net\] this machine is' "$1")" -eq 1 ] || { LOG=$1; fail "the network service itself was restarted"; }
    grep -a 'has not answered for' "$1" | grep -avq "\[init\] $DRIVER has not answered" && { LOG=$1; fail "a service other than the card's driver stopped answering during the test"; }
    return 0
}

peer wire-ip.py "$LOG.ports" "$LOG.wire" "$LOG"
read -r ours theirs < "$LOG.ports"
echo "Booting $ARCH with its card wired to a router made by hand (log: $LOG)..."
boot "$LOG" "socket,id=n0,udp=127.0.0.1:$ours,localaddr=127.0.0.1:$theirs" "$(typed 4000)"
kill -TERM $peer 2>/dev/null
wait $peer
cat "$LOG.wire" 2>/dev/null
grep -q '^wire: ok' "$LOG.wire" || fail "the router made by hand saw something wrong"
common "$LOG" 64
# The machine's own count of what the router sent it: every packet that
# should have been dropped was, and for the reason it should have been.
said "$LOG" '^answered 1 questions for this address, 3 pings' "the service did not count one question and three pings answered"
said "$LOG" '^datagrams 7 in; 1 for a port no program has, 1 a program did not take' "the service's count of datagrams is not what was sent"
said "$LOG" '^dropped  2 damaged, 1 for others, 2 of a kind not spoken here' "the service's count of what it dropped is not what was sent"
# Both ends name the card.
card=$(grep -ao 'on card ..:..:..:..:..:..' "$LOG" | head -1 | sed 's/on card //')
grep -q "frames from $card;" "$LOG.wire" || fail "the service says its card is $card, and its frames say otherwise"

peer udp-host.py "$LOG.ports" "$LOG.host" "$LOG.user"
read -r echo_port done_port ask_port < "$LOG.ports"
echo "Booting it again on QEMU's own network (log: $LOG.user)..."
boot "$LOG.user" "user,id=n0,hostfwd=udp:127.0.0.1:$echo_port-:7,hostfwd=udp:127.0.0.1:$done_port-:4002" "$(typed "$ask_port")"
kill -TERM $peer 2>/dev/null
wait $peer
cat "$LOG.host" 2>/dev/null
grep -q '^host: ok: 5 checks; 1 datagrams from the machine answered' "$LOG.host" || { LOG="$LOG.user"; fail "over QEMU's own network, datagrams did not get through both ways"; }
common "$LOG.user" 255
exit 0
