#!/usr/bin/env python3
"""http-host.py PORTS VERDICT LOG SITE BIG - this host, fetching pages
from the machine's web server, for the HTTP test.

A free port of this host is written to PORTS: QEMU passes connections
made to it to the machine's port 80, where `httpd` should listen. SITE
is the directory the site on the machine's disk was made from
(user/www), and BIG the 1 MiB file put there as www/big.bin: what comes
back is compared with them byte for byte.

Once `httpd` says (in LOG) that it is serving, 19 connections, most of
them with curl - a client that is not mine - and the odd ones by hand
with a socket, since curl will not send a broken request:

  - /, /style.css, /about/ and /about: the files, with the right type
  - HEAD /: the head alone, with the length GET would have
  - /nothing.html: 404
  - /../welcome and /%2e%2e/welcome: 403, never the file outside
  - POST: 405, saying what is allowed; and with a body of 300 KB,
    which the server must read to the end before closing, or the 405
    is lost to a reset
  - "hello": 400
  - an HTTP/1.0 request with no Host: answered
  - /big.bin: 1 MiB, exactly
  - three connections at once: served one after another, all three
  - a request that stops halfway: 408, after the server's 10 seconds
  - 9000 bytes of head: 431
  - a connection that sends nothing and goes: no answer, no harm

On SIGTERM one line is written to VERDICT: "host: ok ..." or "host:
FAIL ...".
"""
import os
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time


def free_port():
    probe = socket.socket()
    probe.bind(("127.0.0.1", 0))
    port = probe.getsockname()[1]
    probe.close()
    return port


def main():
    ports_file, verdict_file, log, site, big_file = sys.argv[1:6]
    port = free_port()
    with open(ports_file, "w") as file:
        file.write(f"{port}\n")
    url = f"http://127.0.0.1:{port}"
    index = open(os.path.join(site, "index.html"), "rb").read()
    style = open(os.path.join(site, "style.css"), "rb").read()
    about = open(os.path.join(site, "about", "index.html"), "rb").read()
    big = open(big_file, "rb").read()

    stop = []
    signal.signal(signal.SIGTERM, lambda *_: stop.append(True))
    wrong, done = [], []
    lock = threading.Lock()
    scratch = tempfile.mkdtemp(prefix="http-host-")

    def check(passed, what):
        with lock:
            (done if passed else wrong).append(what)

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

    def curl(path, *options, name="out"):
        """(status, content type, body) of one fetch with curl."""
        out = os.path.join(scratch, name)
        try:
            result = subprocess.run(
                ["curl", "-s", "--max-time", "60", "-o", out, "-w", "%{http_code} %{content_type}", *options, url + path],
                capture_output=True,
                timeout=90,
            )
            status, _, content_type = result.stdout.decode().partition(" ")
            body = open(out, "rb").read() if os.path.exists(out) else b""
            return int(status or 0), content_type, body
        except (OSError, subprocess.TimeoutExpired, ValueError):
            return 0, "", b""

    def raw(request, pause=None, seconds=30):
        """Sends `request` by hand (and, with `pause`, then waits that
        long before saying it is done); everything that came back."""
        try:
            connection = socket.create_connection(("127.0.0.1", port), timeout=seconds)
            connection.sendall(request)
            if pause is None:
                connection.shutdown(socket.SHUT_WR)
            back = bytearray()
            while True:
                piece = connection.recv(65536)
                if not piece:
                    break
                back.extend(piece)
            connection.close()
            return bytes(back)
        except OSError:
            return b""

    def answer(back):
        """The status, the headers (names in lower case) and the body."""
        head, _, body = back.partition(b"\r\n\r\n")
        lines = head.decode("latin-1").split("\r\n")
        try:
            status = int(lines[0].split(" ")[1])
        except (IndexError, ValueError):
            status = 0
        headers = {}
        for line in lines[1:]:
            name, _, value = line.partition(":")
            headers[name.strip().lower()] = value.strip()
        return status, headers, body

    if said(b"httpd: serving /www on port 80"):
        status, kind, body = curl("/")
        check(status == 200 and kind == "text/html; charset=utf-8" and body == index, "/ is index.html")
        status, kind, body = curl("/style.css")
        check(status == 200 and kind == "text/css; charset=utf-8" and body == style, "/style.css")
        status, kind, body = curl("/about/")
        check(status == 200 and body == about, "/about/ is about/index.html")
        status, kind, body = curl("/about")
        check(status == 200 and body == about, "/about is about/index.html")

        status, headers, body = answer(raw(b"HEAD / HTTP/1.1\r\nHost: test\r\n\r\n"))
        check(status == 200 and headers.get("content-length") == str(len(index)) and body == b"", "HEAD / is the head alone")

        status, kind, body = curl("/nothing.html")
        check(status == 404 and b"404 Not Found" in body, "/nothing.html is 404")
        for path in [b"/../welcome", b"/%2e%2e/welcome"]:
            status, headers, body = answer(raw(b"GET " + path + b" HTTP/1.1\r\nHost: test\r\n\r\n"))
            check(status == 403 and b"Hello from the host" not in body, f"{path.decode()} is 403")

        status, kind, body = curl("/", "-X", "POST", "-d", "x=1", "-D", os.path.join(scratch, "head"))
        allow = b"Allow: GET, HEAD" in open(os.path.join(scratch, "head"), "rb").read()
        check(status == 405 and allow, "POST is 405, with Allow")
        # A body far bigger than the server will read: the answer goes
        # out after the head, and 300 KB more is still on its way - more
        # than the program's channel holds, so some of it is in TCP when
        # the server is done. Closed then, that connection would end in a
        # reset, and this host's TCP throws away the 405 it has not
        # handed to this program yet. The server must read on to the end.
        body = b"x" * 300_000
        status, headers, _ = answer(raw(b"POST /form HTTP/1.1\r\nHost: test\r\nContent-Length: 300000\r\n\r\n" + body))
        check(status == 405, "POST with 300 KB: the 405 arrives")

        status, headers, body = answer(raw(b"hello\r\n\r\n"))
        check(status == 400 and headers.get("connection") == "close", "hello is 400")
        status, headers, body = answer(raw(b"GET /style.css HTTP/1.0\r\n\r\n"))
        check(status == 200 and body == style, "an HTTP/1.0 request with no Host")

        status, kind, body = curl("/big.bin", name="big")
        check(status == 200 and kind == "application/octet-stream" and body == big, "/big.bin is the 1 MiB, exactly")

        results = [None] * 3

        def one(n):
            results[n] = curl("/", name=f"at-once-{n}")

        threads = [threading.Thread(target=one, args=(n,)) for n in range(3)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(120)
        check(all(result is not None and result[0] == 200 and result[2] == index for result in results), "three at once, all served")

        started = time.time()
        status, headers, body = answer(raw(b"GET / HTTP/1.1\r\nHost: te", pause=True, seconds=30))
        waited = time.time() - started
        check(status == 408 and 8 <= waited <= 20, f"half a request is 408 after the wait ({waited:.1f} s)")

        status, headers, body = answer(raw(b"GET / HTTP/1.1\r\nHost: test\r\nX-Long: " + b"a" * 9000 + b"\r\n\r\n"))
        check(status == 431, "9000 bytes of head is 431")

        try:
            socket.create_connection(("127.0.0.1", port), timeout=10).close()
            check(True, "a connection that sends nothing")
        except OSError:
            check(False, "a connection that sends nothing")
        done.append("END")
    else:
        wrong.append("httpd never said it was serving")

    while not stop:
        time.sleep(0.05)

    finished = "END" in done
    counted = f"{len(done) - finished} checks"
    if not finished and not wrong:
        wrong.append("stopped before all of it was tried")
    with open(verdict_file, "w") as file:
        file.write(f"host: FAIL: {'; '.join(wrong)} ({counted})\n" if wrong else f"host: ok: {counted}\n")
    return 1 if wrong else 0


if __name__ == "__main__":
    sys.exit(main())
