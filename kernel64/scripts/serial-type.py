#!/usr/bin/env python3
"""serial-type.py LOG FIFO KEY... - types at a machine's serial port.

For the AArch64 boot test: the same list of keys the PC's test types
through QEMU's monitor (`sendkey`: "shift-h i spc ret"), turned into
the bytes a terminal would send for them and written to FIFO, which
QEMU reads as the serial port's input.

LOG is the machine's serial output, read to know when to go on:

  /    wait until a new serial driver says it is ready (after a crash
       and restart) before typing on
  ~    wait until the shell has run every line typed so far - ten
       seconds at most; "~~" up to ninety

One key at a time, with a pause: a person typing, not a burst.
"""
import sys
import time

READY = b"[serial] serial port ready"
RAN = b"[shell] ran"

NAMED = {
    "spc": b" ", "ret": b"\r", "backspace": b"\x7f", "tab": b"\t", "esc": b"\x1b",
    "dot": b".", "slash": b"/", "backslash": b"\\", "apostrophe": b"'", "comma": b",", "minus": b"-",
    "shift-apostrophe": b'"', "shift-dot": b">", "shift-comma": b"<", "shift-backslash": b"|",
    "shift-7": b"&", "shift-minus": b"_", "shift-5": b"%",
    "up": b"\x1b[A", "down": b"\x1b[B", "right": b"\x1b[C", "left": b"\x1b[D",
    "f11": b"\x1b[23~", "f12": b"\x1b[24~",
}


def key_bytes(key):
    if key in NAMED:
        return NAMED[key]
    if len(key) == 1:
        return key.encode()
    if key.startswith("shift-") and len(key) == 7 and key[6].isalpha():
        return key[6].upper().encode()
    if key.startswith("ctrl-") and len(key) == 6 and key[5].isalpha():
        return bytes([ord(key[5].lower()) - ord("a") + 1])
    sys.exit(f"serial-type: no bytes known for the key '{key}'")


def count(log, what):
    try:
        return open(log, "rb").read().count(what)
    except OSError:
        return 0


def wait_until(condition, seconds):
    deadline = time.time() + seconds
    while time.time() < deadline:
        if condition():
            return True
        time.sleep(0.1)
    return False


def main():
    log, fifo, keys = sys.argv[1], sys.argv[2], sys.argv[3:]
    for key in keys:
        if key not in ("/", "~", "~~"):
            key_bytes(key)      # a key with no bytes: said before anything is typed
    entered = 0
    with open(fifo, "wb", buffering=0) as line:
        for key in keys:
            if key == "/":
                seen = count(log, READY)
                wait_until(lambda: count(log, READY) > seen, 10)
            elif key in ("~", "~~"):
                wait_until(lambda: count(log, RAN) >= entered, 90 if key == "~~" else 10)
            else:
                if key == "ret":
                    entered += 1
                line.write(key_bytes(key))
                time.sleep(0.02)
        # Each Enter runs a command; wait until the shell has run them all.
        commands = keys.count("ret")
        # (Not every Enter is a command - a line typed to a program that
        # reads one is not - so running out of time here says nothing.)
        wait_until(lambda: count(log, RAN) >= commands, 10)
        # Keep the line open a moment: closing it is an end of input.
        time.sleep(0.3)


main()
