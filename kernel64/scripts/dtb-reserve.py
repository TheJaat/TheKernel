#!/usr/bin/env python3
"""dtb-reserve.py IN OUT START SIZE - copies a device tree blob, adding
one entry to its list of reserved memory.

For the boot tests: a loader that keeps some memory for itself says so
in this list, and the kernel must then leave that memory alone. QEMU's
own trees reserve nothing.

The list is pairs of 64-bit numbers (start, size) and ends with a pair
of zeroes. The new pair goes in front, and everything after it - the
tree itself and its strings - moves up by sixteen bytes, so the three
numbers in the header that say where those are and how long the blob is
grow by sixteen too.
"""
import struct
import sys


def main():
    source, target = sys.argv[1:3]
    start, size = int(sys.argv[3], 0), int(sys.argv[4], 0)
    blob = bytearray(open(source, "rb").read())
    magic, total, structure, strings, reserved = struct.unpack(">IIIII", blob[:20])
    if magic != 0xD00DFEED:
        sys.exit(f"dtb-reserve: {source} is not a device tree blob")
    if not reserved < structure < strings:
        sys.exit("dtb-reserve: the blob's parts are not in the usual order")
    blob[reserved:reserved] = struct.pack(">QQ", start, size)
    blob[4:16] = struct.pack(">III", total + 16, structure + 16, strings + 16)
    open(target, "wb").write(blob)


main()
