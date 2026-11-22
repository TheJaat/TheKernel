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
  see:WORD
       wait until WORD is in the log more often than it was when Enter
       was last pressed - ten seconds at most. For what a command
       started in the background says when it is over.

One key at a time, with a pause: a person typing, not a burst. Before
Ctrl+C the pause is longer: what it stops is given time to start.
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
# The rest of a US keyboard's punctuation, as key-names.py names it, so
# that anything it can spell can be typed here too: plain, and shifted.
_PLAIN = {"equal": "=", "bracket_left": "[", "bracket_right": "]", "semicolon": ";", "grave_accent": "`"}
_SHIFTED = {"1": "!", "2": "@", "3": "#", "4": "$", "6": "^", "8": "*", "9": "(", "0": ")", "equal": "+",
            "bracket_left": "{", "bracket_right": "}", "semicolon": ":", "slash": "?", "grave_accent": "~"}
NAMED.update({name: text.encode() for name, text in _PLAIN.items()})
NAMED.update({"shift-" + name: text.encode() for name, text in _SHIFTED.items()})


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
        if key not in ("/", "~", "~~") and not key.startswith("see:"):
            key_bytes(key)      # a key with no bytes: said before anything is typed
    entered = 0
    # The log as it was at the last Enter, for "see:".
    before = b""
    with open(fifo, "wb", buffering=0) as line:
        for key in keys:
            if key == "/":
                seen = count(log, READY)
                wait_until(lambda: count(log, READY) > seen, 10)
            elif key in ("~", "~~"):
                wait_until(lambda: count(log, RAN) >= entered, 90 if key == "~~" else 10)
            elif key.startswith("see:"):
                word = key[4:].encode()
                wait_until(lambda: count(log, word) > before.count(word), 10)
            else:
                if key == "ret":
                    entered += 1
                    try:
                        before = open(log, "rb").read()
                    except OSError:
                        before = b""
                if key == "ctrl-c":
                    # Ctrl+C is pressed at something seen to be running.
                    # A fiftieth of a second after Enter the program may
                    # not have said its first line yet - and with other
                    # CPUs free to run the shell meanwhile, it is ended
                    # before it does.
                    time.sleep(0.3)
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
