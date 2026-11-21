#!/usr/bin/env python3
"""dtb-set.py IN OUT NODE PROPERTY CELL... - copies a device tree blob,
giving one property of one node new cells (32-bit numbers).

For the boot tests: a tree that says something else about a device -
that its interrupt line is read by level, not by edge - is how the
kernel's handling of that is reached on a machine whose own tree
never says it. Nothing is moved: there must be exactly as many new
cells as the property had.

NODE is the node's own name as written in the tree
("virtio_mmio@a003e00"), not a path.
"""
import struct
import sys

BEGIN, END, PROP, NOP, FINISH = 1, 2, 3, 4, 9


def main():
    source, target, wanted_node, wanted_property = sys.argv[1:5]
    cells = b"".join(struct.pack(">I", int(cell, 0)) for cell in sys.argv[5:])
    blob = bytearray(open(source, "rb").read())
    structure, strings = struct.unpack(">II", blob[8:16])
    at = structure
    names = []          # the nodes the walk is inside, outermost first
    changed = 0
    while True:
        (token,) = struct.unpack(">I", blob[at:at + 4])
        start = at
        at += 4
        if token == BEGIN:
            end = blob.index(0, at)
            names.append(blob[at:end].decode())
            at = (end + 4) & ~3
        elif token == END:
            names.pop()
        elif token == PROP:
            length, name_offset = struct.unpack(">II", blob[at:at + 8])
            value = at + 8
            at = (value + length + 3) & ~3
            name_end = blob.index(0, strings + name_offset)
            name = blob[strings + name_offset:name_end].decode()
            if names and names[-1] == wanted_node and name == wanted_property:
                if length != len(cells):
                    sys.exit(f"dtb-set: '{wanted_property}' has {length // 4} cells, and {len(cells) // 4} were given")
                blob[value:value + length] = cells
                changed += 1
        elif token == FINISH:
            break
        elif token != NOP:
            sys.exit(f"dtb-set: not a device tree token at {start:#x}: {token:#x}")
    if changed != 1:
        sys.exit(f"dtb-set: found {changed} properties '{wanted_property}' in a node '{wanted_node}', not 1")
    open(target, "wb").write(blob)


main()
