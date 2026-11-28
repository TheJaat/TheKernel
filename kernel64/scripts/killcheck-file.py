#!/usr/bin/env python3
"""killcheck-file.py NAME FILE - whether FILE holds what killcheck wrote
as NAME (user/src/bin/killcheck.rs, `contents`): its name on the first
line, then a pattern of its number, 8192 bytes in all. Exit 0 if so;
else 1, saying where it first differs. For kill-test.sh (K82)."""
import re
import sys

SIZE = 8192


def contents(name):
    number = int(re.search(r"[0-9]+$", name).group())
    data = bytearray(f"killcheck file {name}\n".encode())
    seed = (number * 2654435761) & 0xFFFFFFFF
    while len(data) < SIZE:
        at = len(data)
        data.append((((seed + at * 7) & 0xFFFFFFFF) >> 3) & 0xFF)
    return bytes(data)


def main():
    name, path = sys.argv[1], sys.argv[2]
    want = contents(name)
    with open(path, "rb") as f:
        got = f.read()
    if got == want:
        return 0
    at = next((i for i, (a, b) in enumerate(zip(got, want)) if a != b), min(len(got), len(want)))
    print(f"killcheck-file: {name}: {len(got)} bytes, the first difference at {at}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
