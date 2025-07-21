#ifndef __VGA_TEST_H__
#define __VGA_TEST_H__

#include <stdint.h>

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

typedef struct VgaMode {
	uint32_t width;
	uint32_t height;
	uintptr_t frameBufferAddress;

	uint32_t fgColor;
	uint32_t bgColor;
} __attribute__((packed)) VgaMode_t;

typedef struct {
    VgaMode_t mode;
} VgaContext;

	unsigned char vga_color(enum vga_color fg, enum vga_color bg);

	void VgaDrawPixel(int x, int y, unsigned char color);

	void VgaClearScreen(unsigned char color);

	void VgaDrawCharacter(VgaContext* ctx, int character, unsigned cursorX, unsigned cursorY, uint8_t color);

#endif