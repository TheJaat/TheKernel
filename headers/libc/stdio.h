#ifndef __STDIO_H__
#define __STDIO_H__

#include <stdarg.h>
#include <stdint.h>

int vsprintf(char *str, const char *format, va_list ap);

void printf(const char* format, ...);

#endif /* __STDIO_H__ */