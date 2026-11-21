#!/usr/bin/env python3
"""dtb-without.py IN OUT NODE PROPERTY - copies a device tree blob,
leaving out one property of one node.

For the boot tests: a tree with something missing is how the kernel's
"the tree does not say" paths are reached. Nothing is moved: the
property's bytes are overwritten with NOP tokens, which a reader skips.

NODE is the node's own name as written in the tree ("chosen",
"pl011@9000000"), not a path.
"""
import struct
import sys

BEGIN, END, PROP, NOP, FINISH = 1, 2, 3, 4, 9


def main():
    source, target, wanted_node, wanted_property = sys.argv[1:5]
    blob = bytearray(open(source, "rb").read())
    structure, strings = struct.unpack(">II", blob[8:16])
    at = structure
    names = []          # the nodes the walk is inside, outermost first
    removed = 0
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
            at = (at + 8 + length + 3) & ~3
            name_end = blob.index(0, strings + name_offset)
            name = blob[strings + name_offset:name_end].decode()
            if names and names[-1] == wanted_node and name == wanted_property:
                for offset in range(start, at, 4):
                    blob[offset:offset + 4] = struct.pack(">I", NOP)
                removed += 1
        elif token == FINISH:
            break
        elif token != NOP:
            sys.exit(f"dtb-without: not a device tree token at {start:#x}: {token:#x}")
    if removed != 1:
        sys.exit(f"dtb-without: found {removed} properties '{wanted_property}' in a node '{wanted_node}', not 1")
    open(target, "wb").write(blob)


main()
