#include <terminal/terminal.h>
#include <terminal/font8x16.h>

#include <video/vbe.h>

void TerminalInit(Terminal* term, VideoDriver* driver, int width, int height) {
    term->driver = driver;
    term->width = width;
    term->height = height;
    term->cursorX = term->cursorY = 0;
    term->limitX = width;
    term->limitY = height;
    term->fgColor = 0x000000;
    term->bgColor = 0xFFFFFFFF;
}

void TerminalPutChar(Terminal* term, int ch) {
    if (ch == '\n') {
        term->cursorX = 0;
        term->cursorY += FontHeight;
    } else {
        term->driver->drawChar(term->driver->context, term->cursorX, term->cursorY, ch, term->fgColor, term->bgColor);
        if (term->videoModeType == VIDEO_TEXT) {
            term->cursorX++;
        } else {
            term->cursorX += FontWidth;
            if (term->cursorX + FontWidth >= term->width) {
                term->cursorX = 0;
                term->cursorY += FontHeight;
            }
        }
    }

    if (term->cursorY + FontHeight >= term->height) {
        // term->driver->scroll(1);
        TerminalScroll(term, 1);
        // term->cursorY -= FontHeight;
    }
}

void TerminalDrawString(Terminal* term, const char* str) {
    // term->driver->drawString()
    while (*str != '\0') {
        TerminalPutChar(term, *str);
        str++;
    }
}

void TerminalClear(Terminal* term) {
    term->driver->clear(term->driver->context, term->bgColor);
    term->cursorX = term->cursorY = 0;
}

void TerminalDrawPixel(Terminal* term, uint32_t X, uint32_t Y, uint32_t color) {
    term->driver->drawPixel(term->driver->context, X, Y, color);
}

void TerminalScroll(Terminal* term, int lines) {
    term->driver->scroll(term->driver->context, lines * FontHeight, term->bgColor);
    term->cursorY -= lines * FontHeight;
}
