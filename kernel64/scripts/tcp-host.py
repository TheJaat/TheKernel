#!/usr/bin/env python3
"""tcp-host.py PORTS VERDICT LOG - this host, as seen from the machine
through QEMU's own network, for the TCP test.

QEMU's "user" network passes TCP between the machine and this host -
with a TCP of its own in between (slirp's, written long ago by other
people), which ends the machine's connections and makes new ones from
this host's sockets. So every byte that gets through in either
direction was understood by a TCP that is not mine.

Four ports of this host are written to PORTS:

  ECHO    QEMU passes connections made here to the machine's port 7,
          where `tcpecho` should listen
  SERVE   likewise, to the machine's port 8000 (`cnet serve`)
  ASK     this program listens here and sends back every byte it is
          sent. From the machine, this host is 10.0.2.2
  CLOSED  nothing listens here: a connection to it must be refused

Once `tcpecho` says (in LOG) that it listens:

  - one connection sends 1 MB and reads at the same time: the same
    1 MB must come back, and then the end
  - three connections at once, 20000 bytes each: tcpecho serves one at
    a time, so two wait - and all three must come back whole
  - after the machine has tried `cnet serve 7` (refused: the port is
    taken), one connection that sends nothing: it must just end

Then, once `cnet serve 8000` listens: two connections, each sending a
line and expecting "you sent N bytes: " and the line in upper case.

On SIGTERM one line is written to VERDICT: "host: ok ..." or "host:
FAIL ...".
"""
import signal
import socket
import sys
import threading
import time


def free_port():
    probe = socket.socket()
    probe.bind(("127.0.0.1", 0))
    port = probe.getsockname()[1]
    probe.close()
    return port


def pattern(length, salt):
    return bytes((n * 31 + salt + (n >> 8)) & 0xFF for n in range(length))


def main():
    ports_file, verdict_file, log = sys.argv[1:4]
    asked = socket.socket()
    asked.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    asked.bind(("127.0.0.1", 0))
    asked.listen(8)
    asked.settimeout(0.1)
    echo_port, serve_port, closed_port = free_port(), free_port(), free_port()
    with open(ports_file, "w") as file:
        file.write(f"{echo_port} {serve_port} {asked.getsockname()[1]} {closed_port}\n")

    stop = []
    signal.signal(signal.SIGTERM, lambda *_: stop.append(True))
    wrong, done = [], []
    served = {"connections": 0, "bytes": 0}
    lock = threading.Lock()

    def check(passed, what):
        with lock:
            (done if passed else wrong).append(what)

    def serve_one(connection):
        total = 0
        try:
            while True:
                data = connection.recv(65536)
                if not data:
                    break
                total += len(data)
                connection.sendall(data)
            connection.shutdown(socket.SHUT_WR)
        except OSError:
            pass
        connection.close()
        with lock:
            served["connections"] += 1
            served["bytes"] += total

    def serve_asked():
        while not stop:
            try:
                connection, _ = asked.accept()
            except (socket.timeout, InterruptedError):
                continue
            threading.Thread(target=serve_one, args=(connection,), daemon=True).start()

    threading.Thread(target=serve_asked, daemon=True).start()

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
            time.sleep(0.05)
        return False

    def exchange(port, data, seconds=120):
        """Sends `data` while reading; then says it is done and reads
        to the end. What came back, or None if the connection failed."""
        try:
            connection = socket.create_connection(("127.0.0.1", port), timeout=seconds)
        except OSError:
            return None
        back = bytearray()
        failed = []

        def send():
            try:
                for at in range(0, len(data), 8192):
                    connection.sendall(data[at:at + 8192])
                connection.shutdown(socket.SHUT_WR)
            except OSError as error:
                failed.append(error)

        sender = threading.Thread(target=send, daemon=True)
        sender.start()
        try:
            while True:
                piece = connection.recv(65536)
                if not piece:
                    break
                back.extend(piece)
        except OSError as error:
            failed.append(error)
        sender.join(seconds)
        connection.close()
        return None if failed else bytes(back)

    if said(b"tcpecho: listening on port 7"):
        big = pattern(1_000_000, 1)
        check(exchange(echo_port, big) == big, "1 MB through tcpecho comes back unchanged")
        results = [None] * 3
        datas = [pattern(20_000, 10 + n) for n in range(3)]

        def one(n):
            results[n] = exchange(echo_port, datas[n])

        threads = [threading.Thread(target=one, args=(n,)) for n in range(3)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(300)
        check(results == datas, "three connections at once to tcpecho: each comes back whole")
        if said(b"cnet: port 7: another program has that port"):
            check(exchange(echo_port, b"") == b"", "a connection that sends nothing just ends")
        else:
            wrong.append("cnet never said port 7 was taken")
        if said(b"cnet: listening on port 8000"):
            for line in [b"hello cnet\n", b"from the host " * 20]:
                expected = b"you sent %d bytes: " % len(line) + line.upper()[:255]
                check(exchange(serve_port, line) == expected, f"cnet serve answers {len(line)} bytes")
            done.append("END")
        else:
            wrong.append("cnet serve never said it was listening")
    else:
        wrong.append("tcpecho never said it was listening")

    while not stop:
        time.sleep(0.05)

    finished = "END" in done
    counted = f"{len(done) - finished} checks; {served['connections']} connections from the machine served, {served['bytes']} bytes"
    if not finished and not wrong:
        wrong.append("stopped before all of it was tried")
    with open(verdict_file, "w") as file:
        file.write(f"host: FAIL: {'; '.join(wrong)} ({counted})\n" if wrong else f"host: ok: {counted}\n")
    return 1 if wrong else 0


if __name__ == "__main__":
    sys.exit(main())
