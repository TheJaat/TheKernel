#!/usr/bin/env python3
"""tcp-resets.py PCAP - each "no such connection" answer the machine sent
(a TCP segment with RST, from 10.0.2.15) in QEMU's packet dump
(-object filter-dump), with the segment it answered: the one before it
from the other side to the same ports. For tcp-test.sh, when netinfo
counted such an answer (TheTaaJ K52): was it a late retransmission
from the host - a FIN or an ACK for a connection the machine had
closed, which correct TCP answers so - or a segment for a connection
that should still have been there?

Reads only Ethernet, IPv4 and TCP; prints nothing if there is no RST.
"""
import struct
import sys

MACHINE = bytes([10, 0, 2, 15])


def flags(bits):
    return "".join(name for bit, name in ((0x02, "SYN"), (0x10, "ACK"), (0x01, "FIN"), (0x04, "RST"), (0x08, "PSH")) if bits & bit)


def segments(path):
    data = open(path, "rb").read()
    if len(data) < 24:
        return
    magic = struct.unpack("<I", data[:4])[0]
    endian = "<" if magic in (0xA1B2C3D4, 0xA1B23C4D) else ">"
    at = 24
    while at + 16 <= len(data):
        sec, frac, size, _ = struct.unpack(endian + "IIII", data[at:at + 16])
        frame = data[at + 16:at + 16 + size]
        at += 16 + size
        if len(frame) < 14 + 20 or frame[12:14] != b"\x08\x00":
            continue
        ip = frame[14:]
        if ip[9] != 6:
            continue
        header = (ip[0] & 0x0F) * 4
        total = struct.unpack(">H", ip[2:4])[0]
        tcp = ip[header:]
        if len(tcp) < 20:
            continue
        sport, dport, seq, ack = struct.unpack(">HHII", tcp[:12])
        offset = (tcp[12] >> 4) * 4
        length = total - header - offset
        yield sec + frac / 1e6, ip[12:16], sport, ip[16:20], dport, seq, ack, tcp[13], length


def main():
    seen = list(segments(sys.argv[1]))
    start = seen[0][0] if seen else 0
    def say(s):
        t, src, sp, dst, dp, seq, ack, bits, length = s
        return (f"{t - start:8.3f} s  {'.'.join(map(str, src))}:{sp} -> {'.'.join(map(str, dst))}:{dp}  "
                f"{flags(bits):<10} seq {seq} ack {ack} len {length}")
    for i, s in enumerate(seen):
        if s[1] == MACHINE and s[7] & 0x04:
            answered = next((p for p in reversed(seen[:i]) if p[3] == MACHINE and p[2] == s[4] and p[4] == s[2]), None)
            print("tcp-resets: the machine answered \"no such connection\":")
            print(f"  to   {say(answered) if answered else '(no segment from the other side before it)'}")
            print(f"  with {say(s)}")


if __name__ == "__main__":
    main()
