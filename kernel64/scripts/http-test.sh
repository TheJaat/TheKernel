#!/bin/sh
# http-test.sh - the HTTP test: the machine's web server, fetched from
# this host with curl.
#
#   usage: http-test.sh ARCH IMAGE DISK LOG SITE BIG
#
# ARCH, IMAGE and DISK as for net-test.sh; the disk has the site (from
# the directory SITE) in www, and the file BIG as www/big.bin. One boot,
# the card on QEMU's own network, the machine's port 80 passed to a port
# of this host (http-host.py, whose verdict goes to LOG.host).
#
# Typed at the machine's shell:
#
#   httpd 0                  refused: no such port
#   httpd 80 nosuch          refused: no such directory
#   httpd 80 welcome         refused: a file, not a directory
#   httpd 80 /www 19         serves nineteen connections, and ends
#   netinfo                  what the service counted

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
SITE=$5
BIG=$6
HERE=$(dirname "$0")

fail() {
    echo "FAIL: $1 (log: $LOG)"
    exit 1
}

# keys WORDS: a command line (letters, digits, dots, slashes, spaces) as
# the names of the keys that type it.
keys() {
    echo "$1" | sed -e 's/ /_/g' -e 's/./& /g' -e 's/_/spc/g' -e 's/\./dot/g' -e 's/\//slash/g'
}

said() {
    grep -aEq "$1" "$LOG" || fail "$2"
}

rm -f "$LOG.ports" "$LOG.host" "$LOG"
"$HERE/http-host.py" "$LOG.ports" "$LOG.host" "$LOG" "$SITE" "$BIG" &
peer=$!
ticks=0
until [ -s "$LOG.ports" ] || [ $ticks -ge 50 ]; do
    sleep 0.1
    ticks=$((ticks + 1))
done
[ -s "$LOG.ports" ] || { kill $peer 2>/dev/null; fail "the other side (http-host.py) did not start"; }
read -r port < "$LOG.ports"

KEYS="$(keys 'httpd 0') ret ~ $(keys 'httpd 80 nosuch') ret ~ $(keys 'httpd 80 welcome') ret ~ \
$(keys 'httpd 80 /www 19') ret ~~ $(keys 'netinfo') ret ~"
NETDEV="user,id=n0,hostfwd=tcp:127.0.0.1:$port-:80"

echo "Booting $ARCH on QEMU's own network, for HTTP (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    KEYS="$KEYS" QEMU_ARGS="-drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0 -netdev $NETDEV -device virtio-net-device,netdev=n0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 90
else
    DISK="$DISK" SMP=1 KEYS="$KEYS" QEMU_ARGS="-netdev $NETDEV -device virtio-net-pci,netdev=n0" \
        "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 90
fi
kill -TERM $peer 2>/dev/null
wait $peer
cat "$LOG.host" 2>/dev/null
grep -q '^host: ok: 17 checks' "$LOG.host" || fail "the host did not see what it should have"

index=$(wc -c < "$SITE/index.html" | tr -d ' ')
said '^usage: httpd \[PORT \[DIRECTORY \[COUNT\]\]\]' "httpd took port 0"
said '^httpd: /nosuch: no such file' "httpd served a directory that is not there"
said '^httpd: /welcome: not a directory' "httpd served a file as a directory"
said '^httpd: serving /www on port 80' "httpd did not start"
said "^httpd: 10\.0\.2\.2 \"GET /\" 200 $index bytes" "the log does not show / given whole"
said '^httpd: 10\.0\.2\.2 "HEAD /" 200 0 bytes' "HEAD was given a body"
said '^httpd: 10\.0\.2\.2 "GET /big\.bin" 200 1048576 bytes' "the log does not show the 1 MiB given whole"
said '^httpd: 10\.0\.2\.2 "GET /\.\./welcome" 403' "a way out of the site was not refused"
said '^httpd: 10\.0\.2\.2 "POST /" 405' "POST was not refused"
said '^httpd: 10\.0\.2\.2 "POST /form" 405' "POST with a body was not refused"
said '^httpd: 10\.0\.2\.2 "-" 400' "a broken request was not refused"
said '^httpd: 10\.0\.2\.2 "-" 408' "half a request was not timed out"
said '^httpd: 10\.0\.2\.2 "-" 431' "a long head was not refused"
said '^httpd: 10\.0\.2\.2 sent no request' "a connection with nothing in it was not seen"
said '\[process\] [0-9]+ "httpd" exited with code 0' "httpd did not end after nineteen connections"
grep -a 'has not answered for' "$LOG" && fail "a service stopped answering during the test"
exit 0
