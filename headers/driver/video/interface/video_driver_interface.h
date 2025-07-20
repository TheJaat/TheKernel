#pragma once

#include <stdint.h>

typedef struct {
    void (*drawPixel)(void*, int x, int y, uint32_t color);
    void (*drawChar)(void*, int x, int y, int ch, uint32_t fg, uint32_t bg);
    void (*scroll)(int lines);
    void (*clear)(void*, uint32_t color);
    void* context;
} VideoDriver;
