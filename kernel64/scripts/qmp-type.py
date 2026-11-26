#!/usr/bin/env python3
"""qmp-type.py LOG SOCKET SCREEN TOKEN... - types on a machine's virtio
keyboard and moves its pointers, through QEMU's QMP socket.

For the AArch64 screen test. The machine's keyboard is a virtio input
device, so keys go in as QEMU's own key events (the monitor's
`sendkey`, run through QMP) - the same names the PC's boot test types:
"shift-h i spc ret". The other tokens:

  ~    wait until the shell has run the line just entered - ten
       seconds at most; "~~" up to ninety
  !COMMAND,ARG,...
       a monitor command, its commas standing for spaces
  @use,NAME
       make the pointing device whose name (in the monitor's
       `info mice`) has NAME in it the one buttons go to
  @rel,DX,DY
       move the relative pointer (the mouse)
  @abs,X,Y
       put the absolute pointer (the tablet) at X, Y - from 0 to 32767
       across the screen, as QEMU counts
  @btn,BUTTON
       press and let go BUTTON (left, right or middle)
  @press,BUTTON and @release,BUTTON
       only press it, or only let it go - a drag is a press, moves,
       and a release
  @at,X,Y
       put the tablet's pointer on pixel (X, Y) of a 1024x768 screen:
       the position in 0..32767 that the display turns into that pixel
  see:TEXT
       wait (ten seconds at most) until TEXT is in the log, its commas
       standing for spaces

Why QMP and not the monitor alone: the monitor's `mouse_move` moves
only relative pointers, so a tablet cannot be moved with it; QMP's
`input-send-event` sends absolute events too.

That command cannot be told which device to send to: its `device`
names a display the device is bound to, and these are bound to none
(QEMU 8.2 refuses that, or - with the default consoles - aborts). So
QEMU picks the device by the event: a relative move only the mouse
takes, an absolute one only the tablet. A button either takes - and
it goes to the device `mouse_set` made current, hence "@use". It
matters: a tablet's button report says where the tablet is, and the
pointer goes there.

At the end, the screen is saved to SCREEN (a binary PPM). Then it waits
until the shell has run every line typed.
"""
import json
import re
import socket
import sys
import time

import typed_line

RAN = b"[shell] ran"


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


class Qmp:
    """One QMP connection: QEMU greets, is told to start taking
    commands, then answers each command with a line of JSON (events it
    sends unasked, in between, are skipped)."""

    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.connect(path)
        self.lines = self.socket.makefile("rb")
        json.loads(self.lines.readline())          # the greeting
        self.run("qmp_capabilities")

    def run(self, command, **arguments):
        message = {"execute": command}
        if arguments:
            message["arguments"] = arguments
        self.socket.sendall(json.dumps(message).encode() + b"\n")
        while True:
            answer = json.loads(self.lines.readline())
            if "return" in answer:
                return answer["return"]
            if "error" in answer:
                sys.exit(f"qmp-type: {command}: {answer['error'].get('desc')}")

    def monitor(self, line):
        said = self.run("human-monitor-command", **{"command-line": line})
        if said.strip():
            sys.exit(f"qmp-type: the monitor said, to '{line}': {said.strip()}")

    def events(self, events):
        self.run("input-send-event", events=events)

    def use(self, name):
        # "* Mouse #2: QEMU Virtio Tablet (absolute)", one line a device.
        listing = self.run("human-monitor-command", **{"command-line": "info mice"})
        found = re.findall(r"Mouse #(\d+): (.*)", listing)
        index = [number for number, what in found if name in what]
        if len(index) != 1:
            sys.exit(f"qmp-type: no one pointing device named '{name}' in: {found}")
        self.monitor(f"mouse_set {index[0]}")


def pointer(qmp, token):
    what, *values = token[1:].split(",")
    if what == "use" and len(values) == 1:
        qmp.use(values[0])
    elif what in ("rel", "abs") and len(values) == 2:
        x, y = int(values[0]), int(values[1])
        qmp.events([
            {"type": what, "data": {"axis": "x", "value": x}},
            {"type": what, "data": {"axis": "y", "value": y}},
        ])
    elif what == "btn" and len(values) == 1:
        for down in (True, False):
            qmp.events([{"type": "btn", "data": {"down": down, "button": values[0]}}])
            time.sleep(0.1)
    elif what in ("press", "release") and len(values) == 1:
        qmp.events([{"type": "btn", "data": {"down": what == "press", "button": values[0]}}])
    elif what == "at" and len(values) == 2:
        x, y = absolute(int(values[0]), SCREEN[0]), absolute(int(values[1]), SCREEN[1])
        qmp.events([
            {"type": "abs", "data": {"axis": "x", "value": x}},
            {"type": "abs", "data": {"axis": "y", "value": y}},
        ])
    else:
        sys.exit(f"qmp-type: '{token}' is not a pointer token")


# The screen the "@at" positions are on.
SCREEN = (1024, 768)
ABSOLUTE_MAX = 32767


def absolute(pixel, size):
    """The smallest tablet position the display maps to `pixel`: it
    computes position * (size - 1) // ABSOLUTE_MAX."""
    position = -(-pixel * ABSOLUTE_MAX // (size - 1))
    assert position * (size - 1) // ABSOLUTE_MAX == pixel, (pixel, size)
    return position


def main():
    log, path, screen, tokens = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4:]
    qmp = Qmp(path)
    # Counted at the last Enter, not when a wait begins: the line may have
    # run already, and "every line so far" is never reached once a line has
    # gone to a program that reads it (journal entry 109).
    ran_before = 0
    # For each Enter, what the shell logs once it has run that line.
    expected = list(typed_line.lines(tokens))
    entered = 0
    last = None
    last_before = 0

    def has_run(seconds):
        # The line just entered has run: its own "[shell] ran" line if its
        # text is known, else one "ran" more than at its Enter.
        if last is not None:
            return wait_until(lambda: count(log, last) > last_before, seconds)
        return wait_until(lambda: count(log, RAN) > ran_before, seconds)

    for token in tokens:
        if token in ("~", "~~"):
            has_run(90 if token == "~~" else 10)
        elif token.startswith("see:"):
            want = token[4:].replace(",", " ").encode()
            if not wait_until(lambda: count(log, want) > 0, 10):
                print(f"qmp-type: never saw {want.decode()!r}", file=sys.stderr)
        elif token.startswith("!"):
            qmp.monitor(token[1:].replace(",", " "))
            time.sleep(0.2)
        elif token.startswith("@"):
            pointer(qmp, token)
            # The device's interrupt, the driver, the display: give the
            # arrow time to be drawn where the event put it.
            time.sleep(0.2)
        else:
            if token == "ret":
                ran_before = count(log, RAN)
                last = expected[entered]
                entered += 1
                last_before = count(log, last) if last is not None else 0
            qmp.monitor(f"sendkey {token}")
            time.sleep(0.05)
    if not has_run(10):
        print("qmp-type: the last line typed did not run", file=sys.stderr)
    time.sleep(0.3)
    qmp.monitor(f"screendump {screen}")
    time.sleep(0.3)


if __name__ == "__main__":
    main()
