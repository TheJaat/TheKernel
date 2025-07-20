#include <video/interface/video_driver_interface.h>
#include <video/vga.h>

static void vga_drawPixel(void* context, int x, int y, uint32_t color) {
    // Use text mode char cells
    VgaDrawPixel(x, y, color);
}

static void vga_drawChar(void* context, int x, int y, int ch, uint32_t fg, uint32_t bg) {
    // VgaPutCharacter(ch); // VGA cursor handles pos
}

static void vga_scroll(int lines) {
    // TextScroll(lines);
}

static void vga_clear(void* context, uint32_t color) {
    VgaClearScreen(0x07); // white on black
}

VideoDriver VGA_DRIVER = {
    .drawPixel = vga_drawPixel,
    .drawChar = vga_drawChar,
    .scroll = vga_scroll,
    .clear = vga_clear,
    .context = nullptr,
};
