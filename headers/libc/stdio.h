#ifndef __STDIO_H__
#define __STDIO_H__

#include <stdarg.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

int vsprintf(char *str, const char *format, va_list ap);

void printf(const char* format, ...);

#ifdef __cplusplus
}
#endif

#endif /* __STDIO_H__ */