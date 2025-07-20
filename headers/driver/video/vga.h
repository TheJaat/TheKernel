#ifndef __VGA_TEST_H__
#define __VGA_TEST_H__

enum vga_color {
	BLACK = 0,
	BLUE = 1,
	GREEN = 2,
	CYAN = 3,
	RED = 4,
	MAGENTA = 5,
	BROWN = 6,
	LIGHT_GRAY = 7,
	DARK_GRAY = 8,
	LIGHT_BLUE = 9,
	LIGHT_GREEN = 10,
	LIGHT_CYAN = 11,
	LIGHT_RED = 12,
	LIGHT_MAGENTA = 13,
	YELLOW = 14,
	WHITE = 15,
};


unsigned char vga_color(enum vga_color fg, enum vga_color bg);

void VgaDrawPixel(int x, int y, unsigned char color);

void VgaClearScreen(unsigned char color);

#endif