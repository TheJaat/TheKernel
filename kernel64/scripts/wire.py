#!/usr/bin/env python3
"""wire.py PORTS VERDICT - the other end of the machine's network cable.

For the network card's test. QEMU can connect an emulated card to a
pair of UDP ports on this host: every Ethernet frame the machine sends
arrives here as one datagram, and every datagram sent back is a frame
the card receives. So this program is a second computer on the wire -
one that looks at everything the machine sends, and answers the frames
`niccheck` (user/src/bin/niccheck.rs) sends, the way it expects.

It picks two free ports and writes them to the file PORTS ("ours
theirs") for whoever starts QEMU:

    -netdev socket,id=n0,udp=127.0.0.1:OURS,localaddr=127.0.0.1:THEIRS

Then it answers until it is told to stop (SIGTERM), and writes what it
saw to VERDICT - one line beginning "wire: ok" or "wire: FAIL".

The frames it knows are of the type set aside for experiments:

    bytes 0-5 destination   6-11 source   12-13 0x88B5
    byte 14 what is asked   15 a number   16- data

    1 ECHO       sent back to the card alone, as 0x81, data flipped
    2 BURST n    n frames of 100 bytes, as 0x82, numbered from 0
    3 STRANGERS  a frame to another card, one to a group, and then one
                 to everyone, as 0x83
    4 LONG n     as BURST, of frames of 1514 bytes
    9            must never be seen: niccheck puts it in what the driver
                 is to refuse
"""
import signal
import socket
import sys

TYPE = b"\x88\xb5"
ECHO, BURST, STRANGERS, LONG_BURST, NEVER = 1, 2, 3, 4, 9
ECHOED, BURST_FRAME, AFTER_STRANGERS = 0x81, 0x82, 0x83
FLIP = 0x5A
BURST_LENGTH = 100
WIRE_MIN, FRAME_MAX = 60, 1514

MINE = bytes.fromhex("020000000001")
ANOTHER_CARD = bytes.fromhex("020000000099")
A_GROUP = bytes.fromhex("01005e000001")
EVERYONE = b"\xff" * 6


def pattern(number, i):
    return (number * 31 + i * 7 + (i >> 8)) & 0xFF


def main():
    ports_file, verdict_file = sys.argv[1], sys.argv[2]
    ours = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    ours.bind(("127.0.0.1", 0))
    # A port for QEMU: found by asking for any, and given up again.
    probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    probe.bind(("127.0.0.1", 0))
    theirs = ("127.0.0.1", probe.getsockname()[1])
    probe.close()
    with open(ports_file, "w") as file:
        file.write(f"{ours.getsockname()[1]} {theirs[1]}\n")

    stop = []
    signal.signal(signal.SIGTERM, lambda *_: stop.append(True))
    ours.settimeout(0.1)

    frames = 0
    card = None
    answered = {ECHO: 0, BURST: 0, STRANGERS: 0, LONG_BURST: 0}
    wrong = []

    def fault(what):
        if what not in wrong:
            wrong.append(what)

    while not stop:
        try:
            frame = ours.recv(4096)
        except socket.timeout:
            continue
        except InterruptedError:
            continue
        frames += 1
        if len(frame) < WIRE_MIN:
            fault(f"a frame of {len(frame)} bytes, shorter than the wire allows")
            continue
        if len(frame) > FRAME_MAX:
            fault(f"a frame of {len(frame)} bytes, longer than the wire allows")
            continue
        if frame[12:14] != TYPE:
            continue
        source = frame[6:12]
        if card is None:
            card = source
        elif source != card:
            fault("frames from two different addresses")
        kind, number = frame[14], frame[15]
        if kind == ECHO:
            data = bytes(byte ^ FLIP for byte in frame[16:])
            ours.sendto(source + MINE + TYPE + bytes([ECHOED, number]) + data, theirs)
        elif kind in (BURST, LONG_BURST):
            length = BURST_LENGTH if kind == BURST else FRAME_MAX
            for n in range(number):
                data = bytes(pattern(n, i) for i in range(16, length))
                ours.sendto(source + MINE + TYPE + bytes([BURST_FRAME, n]) + data, theirs)
        elif kind == STRANGERS:
            rest = MINE + TYPE + bytes([AFTER_STRANGERS, 0]) + bytes(BURST_LENGTH - 16)
            for to in (ANOTHER_CARD, A_GROUP, EVERYONE):
                ours.sendto(to + rest, theirs)
        elif kind == NEVER:
            fault("a frame the driver should have refused to send")
            continue
        else:
            fault(f"a frame asking for {kind}, which nobody sends")
            continue
        answered[kind] += 1

    address = ":".join(f"{byte:02x}" for byte in card) if card else "nobody"
    counted = f"{frames} frames from {address}: {answered[ECHO]} to echo, {answered[BURST] + answered[LONG_BURST]} asking for a burst, {answered[STRANGERS]} for frames to others"
    with open(verdict_file, "w") as file:
        if wrong:
            file.write(f"wire: FAIL: {'; '.join(wrong)} ({counted})\n")
        else:
            file.write(f"wire: ok: {counted}\n")
    return 1 if wrong else 0


if __name__ == "__main__":
    sys.exit(main())
