#!/usr/bin/env python3
"""fill-symbols.py KERNEL_ELF - writes the kernel's own symbols into it.

The kernel keeps a block of symbols::BLOCK_SIZE bytes in its read-only
data, named TAAJ_SYMBOLS and starting with symbols::MAGIC; a panic looks
up the addresses of its backtrace there (src/symbols.rs, the format).
This fills the block in, in the linked file, in place: the block's size
does not change, so no address moves, and nothing needs linking again.

Run after every `cargo build` of the kernel, before the file is copied
or stripped. A second run writes the same bytes. The functions come from
`nm -C` (which demangles Rust's names), without their generic arguments.
"""
import struct
import subprocess
import sys

MAGIC = b"TAAJSYM1"
BLOCK_SIZE = 64 * 1024


def functions(path):
    """(address, name) for each function, sorted, one name per address."""
    out = subprocess.run(["nm", "-C", "--defined-only", "-n", path], capture_output=True, text=True, check=True).stdout
    found, block = [], None
    for line in out.splitlines():
        parts = line.split(" ", 2)
        if len(parts) != 3:
            continue
        address, kind, name = int(parts[0], 16), parts[1], parts[2]
        if name == "TAAJ_SYMBOLS":
            block = address
        # Functions only; not AArch64's mapping symbols ($x code, $d data).
        if kind in "Tt" and not name.startswith("$"):
            # The end of the code, where the linker starts the read-only
            # data: anything there is not in a function (a jump into
            # data, as the nx fault test makes).
            if name in ("__text_end", "__rodata_start"):
                name = "(past the code)"
            found.append((address, short(name)))
    table, last = [], None
    for address, name in found:
        if address != last:
            table.append((address, name))
            last = address
    return table, block


def short(name):
    """The name without generic argument lists: `quicksort::<u64, ...>` is
    `quicksort`. Only lists after `::` - in `<A as B>::f` the brackets are
    the name itself."""
    out, i = [], 0
    while i < len(name):
        if name.startswith("::<", i):
            depth, i = 0, i + 2
            while i < len(name):
                depth += {"<": 1, ">": -1}.get(name[i], 0)
                i += 1
                if depth == 0:
                    break
            continue
        out.append(name[i])
        i += 1
    return "".join(out)


def file_offset(elf, address):
    """Where in the file the loaded address `address` comes from: the
    program header (PT_LOAD) that holds it."""
    if elf[:4] != b"\x7fELF" or elf[4] != 2 or elf[5] != 1:
        sys.exit("fill-symbols: not a 64-bit little-endian ELF file")
    phoff, = struct.unpack_from("<Q", elf, 0x20)
    phentsize, phnum = struct.unpack_from("<HH", elf, 0x36)
    for i in range(phnum):
        kind, _, offset, vaddr, _, filesz, _, _ = struct.unpack_from("<IIQQQQQQ", elf, phoff + i * phentsize)
        if kind == 1 and vaddr <= address < vaddr + filesz:
            return offset + (address - vaddr)
    sys.exit(f"fill-symbols: {address:#x} is in no loaded part of the file")


def block_bytes(table):
    names = b"".join(name.encode() for _, name in table)
    data = MAGIC + struct.pack("<II", len(table), len(names))
    at = 0
    for address, name in table:
        data += struct.pack("<QII", address, at, len(name.encode()))
        at += len(name.encode())
    data += names
    if len(data) > BLOCK_SIZE:
        sys.exit(f"fill-symbols: {len(table)} functions need {len(data)} bytes; the block has {BLOCK_SIZE} (symbols::BLOCK_SIZE)")
    return data + bytes(BLOCK_SIZE - len(data))


def main():
    path = sys.argv[1]
    table, block = functions(path)
    if block is None:
        sys.exit(f"fill-symbols: {path} has no TAAJ_SYMBOLS")
    with open(path, "r+b") as f:
        elf = f.read()
        at = file_offset(elf, block)
        if elf[at:at + 8] != MAGIC:
            sys.exit("fill-symbols: TAAJ_SYMBOLS does not start with the magic - not the block the kernel keeps")
        f.seek(at)
        f.write(block_bytes(table))
    print(f"  SYMBOLS {len(table)} functions")


main()
