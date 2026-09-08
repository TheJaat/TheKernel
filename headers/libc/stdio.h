#ifndef __STDIO_H__
#define __STDIO_H__

#include <stdarg.h>
#include <stdint.h>

/* Everything here is implemented in stdio.cpp, which is C++. Without
 * these guards the definitions get C++ mangling while any C caller
 * looks for the plain name - the link fails with 'undefined reference
 * to printf' from C files only, which is a confusing way to find out. */
#ifdef __cplusplus
extern "C" {
#endif

int vsprintf(char *str, const char *format, va_list ap);

void printf(const char* format, ...);

#ifdef __cplusplus
}
#endif

#endif /* __STDIO_H__ */