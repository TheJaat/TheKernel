#!/usr/bin/env python3
"""udp-host.py PORTS VERDICT LOG - this host, as seen from the machine
through QEMU's own network.

For the network service's test. QEMU's "user" network is a whole
small network inside the emulator, written by other people long ago:
a router at 10.0.2.2 that answers ARP and pings, and passes UDP
between the machine and this host. So here the other side is ordinary
programs using the host's own sockets - no frames, no headers. If
datagrams get through in both directions, the machine's headers and
checksums were good enough for a network stack that is not mine.

Three ports of this host are chosen and written to PORTS:

  ECHO    QEMU is asked to pass what arrives here on to the machine's
          port 7 (`hostfwd`), where `udpecho` should be listening
  DONE    likewise, to the machine's port 4002
  ASK     this program listens here, and answers each datagram with
          "pong: " and the same bytes. From the machine, this host is
          10.0.2.2

Once `udpecho` has said (in LOG, the machine's serial output) that it
listens, datagrams of 1, 100 and 1018 bytes are sent to ECHO and each
must come back unchanged; one of 1200 bytes must not. Then, when a
second `udpecho` listens on 4002, one datagram to DONE ends it.

On SIGTERM one line is written to VERDICT: "host: ok ..." or "host:
FAIL ...".
"""
import signal
import socket
import sys
import time


def free_port():
    probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    probe.bind(("127.0.0.1", 0))
    port = probe.getsockname()[1]
    probe.close()
    return port


def main():
    ports_file, verdict_file, log = sys.argv[1:4]
    asked = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    asked.bind(("127.0.0.1", 0))
    asked.settimeout(0.05)
    echo_port, done_port = free_port(), free_port()
    with open(ports_file, "w") as file:
        file.write(f"{echo_port} {done_port} {asked.getsockname()[1]}\n")
    out = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    out.bind(("127.0.0.1", 0))
    out.settimeout(0.05)

    stop = []
    signal.signal(signal.SIGTERM, lambda *_: stop.append(True))
    wrong, done, answered = [], [], [0]

    def serve():
        try:
            data, sender = asked.recvfrom(4096)
        except (socket.timeout, InterruptedError):
            return
        answered[0] += 1
        asked.sendto(b"pong: " + data, sender)

    # (No limit of its own on waiting for the machine: a boot may wait
    # a long while for its turn, and SIGTERM ends this either way.)
    def said(text, seconds=3600):
        deadline = time.time() + seconds
        while time.time() < deadline and not stop:
            try:
                if text in open(log, "rb").read():
                    return True
            except OSError:
                pass
            serve()
        return False

    def back(seconds):
        """A datagram sent back by the machine, or None."""
        deadline = time.time() + seconds
        while time.time() < deadline and not stop:
            serve()
            try:
                return out.recv(4096)
            except (socket.timeout, InterruptedError):
                pass
        return None

    def check(passed, what):
        (done if passed else wrong).append(what)

    if said(b"udpecho: listening on port 7"):
        for length in [1, 100, 1018]:
            data = bytes((n * 3 + length) & 0xFF for n in range(length))
            out.sendto(data, ("127.0.0.1", echo_port))
            check(back(3) == data, f"a datagram of {length} bytes comes back from the machine unchanged")
        out.sendto(bytes(1200), ("127.0.0.1", echo_port))
        check(back(1) is None, "a datagram of 1200 bytes does not come back")
        if said(b"udpecho: listening on port 4002"):
            out.sendto(b"that is all", ("127.0.0.1", done_port))
            check(back(3) == b"that is all", "the second program answers on its own port")
            done.append("END")
        else:
            wrong.append("the second udpecho never said it was listening")
    else:
        wrong.append("udpecho never said it was listening")

    while not stop:
        serve()

    finished = "END" in done
    counted = f"{len(done) - finished} checks; {answered[0]} datagrams from the machine answered"
    if not finished and not wrong:
        wrong.append("stopped before all of it was tried")
    with open(verdict_file, "w") as file:
        file.write(f"host: FAIL: {'; '.join(wrong)} ({counted})\n" if wrong else f"host: ok: {counted}\n")
    return 1 if wrong else 0


if __name__ == "__main__":
    sys.exit(main())
