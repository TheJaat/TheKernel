#pragma once

#include <stdint.h>

typedef struct {
    void (*drawPixel)(void*, int x, int y, uint32_t color);
    void (*drawChar)(void*, int x, int y, int ch, uint32_t fg, uint32_t bg);
    void (*scroll)(void*, int lines, uint32_t bg);
    void (*clear)(void*, uint32_t color);
    void* context;
} VideoDriver;

enum VideoModeType {
    VIDEO_NONE = 0x0,
    VIDEO_TEXT = 0x1,
    VIDEO_GRAPHICS = 0x2
};
