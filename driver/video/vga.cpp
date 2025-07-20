#include <video/vga.h>

#define VGA_WIDTH 80;
#define VGA_HEIGHT 25;


unsigned char vga_color(enum vga_color fg, enum vga_color bg) {
	return fg | bg << 4;

}

void VgaDrawPixel(int x, int y, unsigned char color) {
	int offset = y*80 + x;
	unsigned short* vgaMem = (unsigned short*)0xB8000;
	vgaMem[offset] = ' ' | color << 8; // char is at LSB 8 bits, while color is at MSB 8 bits
}


unsigned short vga_entry(unsigned char uc, unsigned char color) {
	return (unsigned short) uc | (unsigned short) color << 8;
}

void VgaClearScreen(unsigned char color) {
	unsigned short blank = vga_entry(' ', color);
	for(int y = 0; y < 25; y++) {
		for(int x = 0; x < 80; x++) {
			int index = y * 80 + x;
			unsigned short* vgaMem = (unsigned short*) 0xB8000;
			vgaMem[index] = blank;
		}
	}
}