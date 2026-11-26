#!/usr/bin/env python3
"""serial-type.py LOG FIFO KEY... - types at a machine's serial port.

For the AArch64 boot test: the same list of keys the PC's test types
through QEMU's monitor (`sendkey`: "shift-h i spc ret"), turned into
the bytes a terminal would send for them and written to FIFO, which
QEMU reads as the serial port's input.

LOG is the machine's serial output, read to know when to go on:

  /    wait until a new serial driver says it is ready (after a crash
       and restart) before typing on
  ~    wait until the shell has run the line just entered - ten
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

import typed_line

READY = b"[serial] serial port ready"
RAN = b"[shell] ran"
# init, starting a program a line asked for.
STARTED = b"for the shell"

def key_bytes(key):
    typed = typed_line.key_bytes(key)
    if typed is None:
        sys.exit(f"serial-type: no bytes known for the key '{key}'")
    return typed


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
    # Counted before a key is sent, not when a wait begins: a driver that
    # crashes is back, and a line has run, within moments under KVM, and a
    # count taken afterwards already has the new line in it - the wait is
    # then for one more that never comes (journal entry NNN).
    ready_before = 0
    ran_before = 0
    started_before = 0
    # For each Enter, what the shell logs once it has run that line.
    expected = list(typed_line.lines(keys))
    entered = 0
    last = None
    last_before = 0

    def has_run(seconds):
        # The line just entered has run: its own "[shell] ran" line if its
        # text is known, else one "ran" more than at its Enter.
        if last is not None:
            return wait_until(lambda: count(log, last) > last_before, seconds)
        return wait_until(lambda: count(log, RAN) > ran_before, seconds)

    # The log as it was at the last Enter, for "see:".
    before = b""
    with open(fifo, "wb", buffering=0) as line:
        for key in keys:
            if key == "/":
                wait_until(lambda: count(log, READY) > ready_before, 10)
            elif key in ("~", "~~"):
                # The line just entered has run. (Not every line entered so
                # far: one typed to a program that reads it is not the
                # shell's, and that count is never reached.)
                has_run(90 if key == "~~" else 10)
            elif key.startswith("see:"):
                word = key[4:].encode()
                wait_until(lambda: count(log, word) > before.count(word), 10)
            else:
                ready_before = count(log, READY)
                if key == "ret":
                    ran_before = count(log, RAN)
                    started_before = count(log, STARTED)
                    last = expected[entered]
                    entered += 1
                    last_before = count(log, last) if last is not None else 0
                    try:
                        before = open(log, "rb").read()
                    except OSError:
                        before = b""
                if key == "ctrl-c":
                    # Ctrl+C is pressed at something seen to be running:
                    # a program started since the last Enter. A fixed
                    # pause was not enough - an emulated machine sharing
                    # the host with other tests took longer to start cat,
                    # and the Ctrl+C meant for it stopped nothing (journal
                    # entry NNN). Then a moment more: just started, it may
                    # not have said its first line yet.
                    wait_until(lambda: count(log, STARTED) > started_before, 10)
                    time.sleep(0.3)
                line.write(key_bytes(key))
                time.sleep(0.02)
        # Wait until the last line typed has run.
        has_run(10)
        # Keep the line open a moment: closing it is an end of input.
        time.sleep(0.3)


main()
