#ifndef _TERMINAL_H
#define _TERMINAL_H

#include <stdint.h>
#include <video/interface/video_driver_interface.h>

typedef struct {
    int width, height;
    int cursorX, cursorY;
    uint32_t fgColor, bgColor;
    uint32_t videoModeType;
    VideoDriver* driver; // renderer
} Terminal;

#ifdef __cplusplus
extern "C" {
#endif

    void TerminalInit(Terminal* term, VideoDriver* driver, int width, int height);

    void TerminalDrawPixel(Terminal* term, uint32_t x, uint32_t y, uint32_t color);

    void TerminalPutChar(Terminal* term, int ch);
    void TerminalDrawString(Terminal* term, const char* str);

    void TerminalClear(Terminal* term);
    void TerminalScroll(Terminal* term, int lines);

#ifdef __cplusplus
}
#endif

#endif // _TERMINAL_H