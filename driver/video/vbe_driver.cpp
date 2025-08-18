#include <video/interface/video_driver_interface.h>
#include <video/vbe.h>

static void vbe_drawPixel(void* context, int x, int y, uint32_t color) {
    VesaDrawPixel((VbeContext*)context, x, y, color);
}

static void vbe_drawChar(void* context, int x, int y, int ch, uint32_t fg, uint32_t bg) {
    VesaDrawCharacter((VbeContext*)context, x, y, ch, fg, bg);
}

static void vbe_scroll(void* context, int lines, uint32_t bg) {
    VesaScroll((VbeContext*)context, lines, bg);
}

static void vbe_clear(void* context, uint32_t color) {
    // fill framebuffer white
    VesaClearScreen((VbeContext*)context, color);
}

VideoDriver VBE_DRIVER = {
    .drawPixel = vbe_drawPixel,
    .drawChar = vbe_drawChar,
    .scroll = vbe_scroll,
    .clear = vbe_clear,
    .context = nullptr, // set later
};
