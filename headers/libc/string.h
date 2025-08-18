#ifndef __STRING_H__
#define __STRING_H__

#include <stdint.h>
#include <stddef.h>

#define LBLOCKSIZE (sizeof(long))
#define UNALIGNED(X)   ((long)X & (LBLOCKSIZE - 1))
#define TOO_SMALL(LEN) ((LEN) < LBLOCKSIZE)

#ifdef __cplusplus
extern "C" {
#endif

char *strcat(char *destination, const char *source);

char *strchr(const char *str, int ch);

size_t strlen(const char *str);

void *memset(void *dest, int c, size_t count);

void *memcpy(void *dest, const void *src, size_t count);

#ifdef __cplusplus
}
#endif

#endif /* __STRING_H__ */