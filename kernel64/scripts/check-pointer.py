#!/usr/bin/env python3
"""check-pointer.py - checks a QEMU screendump (binary PPM) shows the
pointer's arrow with its tip at (X, Y), and nowhere else.

    usage: check-pointer.py SCREEN.ppm X Y

The arrow is drawn by the display service with the `gfx` crate: the
picture below is a copy of gfx's (gfx/src/lib.rs, ARROW) - '#' black
outline, '.' white inside. Every pixel of it that is on the screen must
be exactly that colour; and no other pixel on the screen may be pure
white (the console's text is a paler grey-blue), so a second arrow, or
one left behind where the pointer was before, is found too.

Exits 0 if it matches, 1 (with the reason) if not.
"""
import sys

ARROW = [
    "#           ",
    "##          ",
    "#.#         ",
    "#..#        ",
    "#...#       ",
    "#....#      ",
    "#.....#     ",
    "#......#    ",
    "#.......#   ",
    "#........#  ",
    "#.........# ",
    "#......#####",
    "#...#..#    ",
    "#..# #..#   ",
    "#.#  #..#   ",
    "##    #..#  ",
    "#     #..#  ",
    "       ##   ",
    "            ",
]


def read_ppm(path):
    """Width, height and the RGB bytes of a binary PPM (as check-screen.py
    reads one: "P6", width, height, 255, one whitespace byte, pixels)."""
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
    path, x0, y0 = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    width, height, pixels = read_ppm(path)

    def at(x, y):
        i = (y * width + x) * 3
        return tuple(pixels[i:i + 3])

    wrong, arrow = [], set()
    for dy, row in enumerate(ARROW):
        for dx, c in enumerate(row):
            x, y = x0 + dx, y0 + dy
            if c == " " or not (0 <= x < width and 0 <= y < height):
                continue
            arrow.add((x, y))
            want = (0, 0, 0) if c == "#" else (255, 255, 255)
            if at(x, y) != want:
                wrong.append(f"({x}, {y}) is {at(x, y)}, not {want}")
    if not arrow:
        print(f"check-pointer: ({x0}, {y0}) puts no part of the arrow on a {width}x{height} screen")
        return 1
    if wrong:
        print(f"check-pointer: the arrow at ({x0}, {y0}) is not there: {len(wrong)} pixels wrong, the first {wrong[0]}")
        return 1
    stray = [(x, y) for y in range(height) for x in range(width) if (x, y) not in arrow and at(x, y) == (255, 255, 255)]
    if stray:
        print(f"check-pointer: {len(stray)} white pixels outside the arrow, the first at {stray[0]}: another arrow, or one left behind")
        return 1
    print(f"check-pointer: the arrow is at ({x0}, {y0}), {len(arrow)} pixels of it on the screen, and nowhere else")
    return 0


if __name__ == "__main__":
    sys.exit(main())
