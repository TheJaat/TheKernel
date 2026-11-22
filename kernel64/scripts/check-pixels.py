#!/usr/bin/env python3
"""check-pixels.py - checks the colours of chosen pixels of a QEMU
screendump (binary PPM).

    usage: check-pixels.py SCREEN.ppm X,Y=RRGGBB[:WHAT] ...

Each X,Y must be RRGGBB (hexadecimal), within 8 in each channel. WHAT
names the pixel in the verdict ("the terminal's title bar"). For the
window test: a window's title bar where the window was dragged to, the
desktop where a closed window was.

Exits 0 if all match, 1 (with the first that does not) if not.
"""
import sys


def read_ppm(path):
    """Width, height and the RGB bytes of a binary PPM ("P6", width,
    height, 255, one whitespace byte, pixels)."""
    with open(path, "rb") as f:
        data = f.read()
    fields, pos = [], 0
    while len(fields) < 4:
        while data[pos:pos + 1].isspace():
            pos += 1
        start = pos
        while not data[pos:pos + 1].isspace():
            pos += 1
        fields.append(data[start:pos])
    if fields[0] != b"P6" or fields[3] != b"255":
        raise ValueError("not an 8-bit binary PPM")
    return int(fields[1]), int(fields[2]), data[pos + 1:]


def main():
    width, height, pixels = read_ppm(sys.argv[1])
    for check in sys.argv[2:]:
        where, _, rest = check.partition("=")
        colour, _, what = rest.partition(":")
        x, y = (int(n) for n in where.split(","))
        want = tuple(int(colour[i:i + 2], 16) for i in (0, 2, 4))
        if not (0 <= x < width and 0 <= y < height):
            print(f"check-pixels: ({x}, {y}) is off the {width}x{height} screen")
            return 1
        o = 3 * (y * width + x)
        got = tuple(pixels[o:o + 3])
        if any(abs(g - w) > 8 for g, w in zip(got, want)):
            print(f"check-pixels: {what or 'the pixel'} at ({x}, {y}) is {bytes(got).hex()}, not {colour}")
            return 1
    print(f"check-pixels: {len(sys.argv) - 2} pixels as expected")
    return 0


if __name__ == "__main__":
    sys.exit(main())
