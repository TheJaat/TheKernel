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
    } else if (ch == '\b') {
        // Erase the previous cell rather than just moving the cursor,
        // otherwise the old glyph stays on screen and the line editor
        // looks broken even though the buffer is correct.
        int step = (term->videoModeType == VIDEO_TEXT) ? 1 : FontWidth;
        if (term->cursorX >= step) {
            term->cursorX -= step;
        } else if (term->cursorY >= FontHeight) {
            // Wrapped back onto the previous line.
            term->cursorY -= FontHeight;
            term->cursorX = term->width - (term->width % step) - step;
            if (term->cursorX < 0) {
                term->cursorX = 0;
            }
        }
        term->driver->drawChar(term->driver->context, term->cursorX,
            term->cursorY, ' ', term->fgColor, term->bgColor);
        return;
    } else if (ch == '\t') {
        int i;
        for (i = 0; i < 4; i++) {
            TerminalPutChar(term, ' ');
        }
        return;
    } else if (ch == '\r') {
        term->cursorX = 0;
        return;
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