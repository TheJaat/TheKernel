#!/usr/bin/env python3
"""disk-pattern.py make FILE SECTORS
disk-pattern.py check FILE SECTORS written|untouched

The disk the block driver's test (user/src/bin/blkcheck.rs) expects,
and a look at it afterwards, from outside the machine.

make: byte i of sector s is original(s, i) - every sector different
from every other, so that one read from the wrong place shows.

check: after a boot. "written" - blkcheck wrote written(s, i) to
sectors 100 to 132 and to the last one, and nothing else may differ
from the original; "untouched" - nothing at all may (a read-only disk,
or one no driver was started for).

That the program inside read back what it wrote proves little about
the disk: a driver that kept the data and never sent it would pass.
The file, read here, is the disk.
"""
import sys

SECTOR = 512


def original(s):
    return bytes((s * 31 + i * 7 + (i >> 8)) & 0xFF for i in range(SECTOR))


def written(s):
    return bytes(((s * 13 + i * 3 + 1) & 0xFF) ^ 0xA5 for i in range(SECTOR))


def main():
    command, path, sectors = sys.argv[1], sys.argv[2], int(sys.argv[3])
    if command == "make":
        with open(path, "wb") as image:
            for s in range(sectors):
                image.write(original(s))
        return
    changed = set(range(100, 133)) | {sectors - 1} if sys.argv[4] == "written" else set()
    data = open(path, "rb").read()
    if len(data) != sectors * SECTOR:
        sys.exit(f"disk-pattern: the image is {len(data)} bytes, not {sectors} sectors")
    wrong = [s for s in range(sectors)
             if data[s * SECTOR:(s + 1) * SECTOR] != (written(s) if s in changed else original(s))]
    if wrong:
        sys.exit(f"disk-pattern: {len(wrong)} sectors are not what they should be; the first: {wrong[:5]}")
    print(f"the disk image, read from outside: {len(changed)} sectors written, {sectors - len(changed)} as they were")


main()
