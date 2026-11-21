#!/usr/bin/env python3
"""wire-ip.py PORTS VERDICT LOG - another machine on the wire, speaking
ARP, IPv4, ICMP and UDP.

For the network service's test. As wire.py, this is the other end of
the cable (QEMU hands every frame the machine sends to a UDP port
here, and every datagram sent back is a frame its card receives). But
this one plays the network's router, 10.0.2.2, and it is written
without a line of the system's own code: every header is put together
and taken apart here by hand, and every checksum worked out here. If
the two agree, it is because both follow the standards.

It does two things at once.

It answers, like any machine: "who has 10.0.2.2?", pings to 10.0.2.2,
and a datagram to port 4000 (with "pong: " and the same bytes). It
does not answer for any other address - 10.0.2.99 is nobody.

And it asks, once the machine's network service has said (in LOG, the
machine's serial output) that it is running:

  - who has 10.0.2.15?                      the card must answer
  - pings of 0, 56 and 1472 bytes            each answered, byte for byte
  - a ping with a wrong ICMP checksum        silence
  - a packet with a wrong IPv4 checksum      silence
  - a ping for 10.0.2.16, in a frame for     silence
    this card
  - a ping to everyone (255.255.255.255)     silence
  - a piece of a cut-up packet               silence
  - a datagram to port 9, which nobody has   "port unreachable",
                                             quoting the datagram
  - then, when `udpecho` says it listens:
    datagrams of 1, 100 and 1018 bytes to    each sent back, from
    port 7                                   port 7, checksum right
  - one of 1200 bytes                        silence (more than a
                                             program is handed)
  - and last, when a second `udpecho` says
    it listens on port 4002: one datagram    sent back - which ends
    to it                                    that program: how the
                                             test knows all of the
                                             above is over

PORTS and VERDICT are as wire.py's: the two UDP ports are written to
the first, and on SIGTERM one line - "wire: ok ..." or "wire: FAIL ..."
- to the second.
"""
import signal
import socket
import struct
import sys
import time

ME, ME_MAC = bytes([10, 0, 2, 2]), bytes.fromhex("52550a000202")
THEM = bytes([10, 0, 2, 15])
EVERYONE = b"\xff" * 6
ARP, IPV4 = b"\x08\x06", b"\x08\x00"
ICMP, UDP = 1, 17


def checksum(data):
    if len(data) % 2:
        data += b"\0"
    total = sum(struct.unpack(f"!{len(data) // 2}H", data))
    while total >> 16:
        total = (total & 0xFFFF) + (total >> 16)
    return ~total & 0xFFFF


def ip_packet(source, destination, protocol, payload, flags=0, spoil=False):
    header = struct.pack("!BBHHHBBH4s4s", 0x45, 0, 20 + len(payload), 0x4242, flags, 64, protocol, 0, source, destination)
    check = checksum(header) ^ (0x0100 if spoil else 0)
    return header[:10] + struct.pack("!H", check) + header[12:] + payload


def icmp_message(kind, code, rest, data, spoil=False):
    check = checksum(struct.pack("!BBH", kind, code, 0) + rest + data) ^ (1 if spoil else 0)
    return struct.pack("!BBH", kind, code, check) + rest + data


def udp_segment(source, destination, source_port, destination_port, data):
    length = 8 + len(data)
    pseudo = source + destination + struct.pack("!BBH", 0, UDP, length)
    check = checksum(pseudo + struct.pack("!HHHH", source_port, destination_port, length, 0) + data) or 0xFFFF
    return struct.pack("!HHHH", source_port, destination_port, length, check) + data


def padded(frame):
    return frame + bytes(max(0, 60 - len(frame)))


class Wire:
    def __init__(self, ports_file):
        self.ours = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.ours.bind(("127.0.0.1", 0))
        probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        probe.bind(("127.0.0.1", 0))
        self.theirs = ("127.0.0.1", probe.getsockname()[1])
        probe.close()
        with open(ports_file, "w") as file:
            file.write(f"{self.ours.getsockname()[1]} {self.theirs[1]}\n")
        self.ours.settimeout(0.05)
        self.card = None       # the machine's card address, once it has said
        self.wrong = []
        self.frames = 0
        self.served = {"questions": 0, "pings": 0, "datagrams": 0}
        self.unasked = 0       # questions about addresses nobody has
        self.stop = False

    def fault(self, what):
        if what not in self.wrong:
            self.wrong.append(what)

    def send(self, to, kind, payload):
        self.ours.sendto(padded(to + ME_MAC + kind + payload), self.theirs)

    def send_ip(self, packet):
        self.send(self.card, IPV4, packet)

    def read(self):
        """One frame, taken apart as far as it goes - or None. Answers
        what a router answers, on the way."""
        try:
            frame = self.ours.recv(4096)
        except (socket.timeout, InterruptedError):
            return None
        self.frames += 1
        if len(frame) < 60:
            self.fault(f"a frame of {len(frame)} bytes, shorter than the wire allows")
            return None
        to, source, kind, payload = frame[:6], frame[6:12], frame[12:14], frame[14:]
        if self.card is None:
            self.card = source
        elif source != self.card:
            self.fault("frames from two different card addresses")
        if kind == ARP:
            hardware, protocol, hlen, plen, operation = struct.unpack("!HHBBH", payload[:8])
            sender_mac, sender_ip, target_mac, target_ip = payload[8:14], payload[14:18], payload[18:24], payload[24:28]
            if (hardware, protocol, hlen, plen) != (1, 0x0800, 6, 4):
                self.fault("an ARP packet that is not for Ethernet and IPv4")
            if sender_mac != source or sender_ip != THEM:
                self.fault("an ARP packet whose sender is not the machine itself")
            if operation == 1:
                if to != EVERYONE:
                    self.fault("a question (ARP) not asked of everyone")
                if target_ip == ME:
                    self.served["questions"] += 1
                    self.send(source, ARP, struct.pack("!HHBBH", 1, 0x0800, 6, 4, 2) + ME_MAC + ME + sender_mac + sender_ip)
                else:
                    self.unasked += 1
            return ("arp", operation, sender_mac, sender_ip, target_mac, target_ip, to)
        if kind != IPV4:
            self.fault(f"a frame of type {kind.hex()}, which this machine should not send")
            return None
        if to != ME_MAC and to != EVERYONE:
            self.fault("an IPv4 packet in a frame for a card nobody has")
        version, _, total, _, flags, ttl, protocol, _, source_ip, destination_ip = struct.unpack("!BBHHHBBH4s4s", payload[:20])
        if version != 0x45 or total > len(payload) or checksum(payload[:20]) != 0:
            self.fault("an IPv4 header that is wrong (version, length or checksum)")
            return None
        if source_ip != THEM:
            self.fault("an IPv4 packet that does not come from 10.0.2.15")
        if flags & 0x3FFF:
            self.fault("a fragment")
        if ttl != 64:
            self.fault(f"a packet with a time to live of {ttl}")
        inner = payload[20:total]
        if protocol == ICMP:
            if checksum(inner) != 0:
                self.fault("an ICMP message with a wrong checksum")
                return None
            icmp_kind, code, rest, data = inner[0], inner[1], inner[4:8], inner[8:]
            if icmp_kind == 8 and destination_ip == ME:
                self.served["pings"] += 1
                self.send_ip(ip_packet(ME, THEM, ICMP, icmp_message(0, 0, rest, data)))
            return ("icmp", icmp_kind, code, rest, data, destination_ip)
        if protocol == UDP:
            source_port, destination_port, length, check = struct.unpack("!HHHH", inner[:8])
            pseudo = source_ip + destination_ip + struct.pack("!BBH", 0, UDP, length)
            if length != len(inner) or check == 0 or checksum(pseudo + inner) != 0:
                self.fault("a UDP datagram with a wrong length or checksum (or none)")
                return None
            data = inner[8:]
            if destination_ip == ME and destination_port == 4000:
                self.served["datagrams"] += 1
                self.send_ip(ip_packet(ME, THEM, UDP, udp_segment(ME, THEM, 4000, source_port, b"pong: " + data)))
            return ("udp", source_port, destination_port, data, destination_ip)
        self.fault(f"an IPv4 packet of protocol {protocol}")
        return None

    def expect(self, wanted, seconds=3.0):
        """The first frame `wanted` says yes to, within `seconds` -
        everything else is read (and answered) and passed over."""
        deadline = time.time() + seconds
        while time.time() < deadline and not self.stop:
            seen = self.read()
            if seen is not None and wanted(seen):
                return seen
        return None

    def silence(self, wanted, seconds=0.4):
        return self.expect(wanted, seconds) is None

    def wait_for_log(self, log, text, seconds=3600):
        """Until the machine has said `text` - answering meanwhile. (No
        real limit: a boot may wait a long while for its turn, and
        SIGTERM ends this either way.)"""
        deadline = time.time() + seconds
        while time.time() < deadline and not self.stop:
            try:
                if text in open(log, "rb").read():
                    return True
            except OSError:
                pass
            self.read()
        return False


def main():
    ports_file, verdict_file, log = sys.argv[1:4]
    wire = Wire(ports_file)
    signal.signal(signal.SIGTERM, lambda *_: setattr(wire, "stop", True))
    done = []

    def check(passed, what):
        if passed:
            done.append(what)
        else:
            wire.fault(what)

    def echo(identifier, sequence, data, **how):
        rest = struct.pack("!HH", identifier, sequence)
        return ip_packet(how.pop("source", ME), how.pop("to", THEM), ICMP, icmp_message(8, 0, rest, data, how.pop("spoil_icmp", False)), **how)

    def is_echo_reply(identifier):
        return lambda seen: seen[0] == "icmp" and seen[1] == 0 and seen[3][:2] == struct.pack("!H", identifier)

    if wire.wait_for_log(log, b"[net] this machine is"):
        # Who has 10.0.2.15? (The first frame to the machine goes to
        # everyone: its card's address is what is being asked.)
        wire.send(EVERYONE, ARP, struct.pack("!HHBBH", 1, 0x0800, 6, 4, 1) + ME_MAC + ME + bytes(6) + THEM)
        answer = wire.expect(lambda seen: seen[0] == "arp" and seen[1] == 2)
        check(
            answer is not None and answer[2] == wire.card and answer[3] == THEM and answer[4] == ME_MAC and answer[5] == ME and answer[6] == ME_MAC,
            "'who has 10.0.2.15?' is answered, to the asker, with the card's address",
        )
        if wire.card is not None:
            for number, length in enumerate([0, 56, 1472]):
                data = bytes((n * 13 + length) & 0xFF for n in range(length))
                wire.send_ip(echo(0x7000 + number, number, data))
                answer = wire.expect(is_echo_reply(0x7000 + number))
                check(
                    answer is not None and answer[3] == struct.pack("!HH", 0x7000 + number, number) and answer[4] == data and answer[5] == ME,
                    f"a ping of {length} bytes is answered, byte for byte",
                )
            wire.send_ip(echo(0x7100, 0, b"wrong sum", spoil_icmp=True))
            check(wire.silence(is_echo_reply(0x7100)), "a ping with a wrong ICMP checksum is not answered")
            wire.send_ip(echo(0x7101, 0, b"wrong header", spoil=True))
            check(wire.silence(is_echo_reply(0x7101)), "a packet with a wrong IPv4 checksum is not answered")
            wire.send_ip(echo(0x7102, 0, b"not for you", to=bytes([10, 0, 2, 16])))
            check(wire.silence(is_echo_reply(0x7102)), "a ping for another address is not answered")
            wire.send_ip(echo(0x7103, 0, b"everyone", to=bytes([255] * 4)))
            check(wire.silence(is_echo_reply(0x7103)), "a ping to everyone is not answered")
            wire.send_ip(echo(0x7104, 0, b"a piece of something", flags=0x2000))
            check(wire.silence(is_echo_reply(0x7104)), "a piece of a cut-up packet is not answered")

            # A port nobody has: the machine says so, and quotes the
            # datagram - its IPv4 header and its first eight bytes.
            closed = ip_packet(ME, THEM, UDP, udp_segment(ME, THEM, 5555, 9, b"anybody there?"))
            wire.send_ip(closed)
            answer = wire.expect(lambda seen: seen[0] == "icmp" and seen[1] == 3)
            check(answer is not None and answer[2] == 3 and answer[4] == closed[:28], "a datagram for a port nobody has is answered with 'port unreachable', quoting it")

            if wire.wait_for_log(log, b"udpecho: listening on port 7"):
                for length in [1, 100, 1018]:
                    data = bytes((n * 5 + length) & 0xFF for n in range(length))
                    wire.send_ip(ip_packet(ME, THEM, UDP, udp_segment(ME, THEM, 6000 + length, 7, data)))
                    answer = wire.expect(lambda seen: seen[0] == "udp" and seen[2] == 6000 + length)
                    check(answer is not None and answer[1] == 7 and answer[3] == data and answer[4] == ME, f"a datagram of {length} bytes to port 7 comes back from port 7")
                wire.send_ip(ip_packet(ME, THEM, UDP, udp_segment(ME, THEM, 6999, 7, bytes(1200))))
                check(wire.silence(lambda seen: seen[0] == "udp" and seen[2] == 6999), "a datagram longer than a program is handed is dropped")
                if wire.wait_for_log(log, b"udpecho: listening on port 4002"):
                    wire.send_ip(ip_packet(ME, THEM, UDP, udp_segment(ME, THEM, 6002, 4002, b"that is all")))
                    answer = wire.expect(lambda seen: seen[0] == "udp" and seen[2] == 6002)
                    check(answer is not None and answer[1] == 4002 and answer[3] == b"that is all", "a second program on another port gets its own datagrams")
                    done.append("END")
                else:
                    wire.fault("the second udpecho never said it was listening")
            else:
                wire.fault("udpecho never said it was listening")
    else:
        wire.fault("the network service never said it was running")

    # Whatever the machine still asks: answered until told to stop.
    while not wire.stop:
        wire.read()

    finished = "END" in done
    checks = len(done) - finished
    address = ":".join(f"{byte:02x}" for byte in wire.card) if wire.card else "nobody"
    counted = (
        f"{checks} checks; {wire.frames} frames from {address}; answered {wire.served['questions']} questions for 10.0.2.2, "
        f"{wire.served['pings']} pings and {wire.served['datagrams']} datagrams; {wire.unasked} questions about addresses nobody has"
    )
    if not finished and not wire.wrong:
        wire.fault("stopped before all of it was tried")
    with open(verdict_file, "w") as file:
        if wire.wrong:
            file.write(f"wire: FAIL: {'; '.join(wire.wrong)} ({counted})\n")
        else:
            file.write(f"wire: ok: {counted}\n")
    return 1 if wire.wrong else 0


if __name__ == "__main__":
    sys.exit(main())
