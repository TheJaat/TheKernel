#ifndef __STD_DEF_H__
#define __STD_DEF_H__

#include <stdint.h>

#if defined(__x86_64__) || defined (_WIN64)
	typedef uint64_t size_t;	// 64-bit for 64-bit architectures
#else
	typedef uint32_t size_t;	// 32-bit for 32-bit architectures
#endif


#ifndef _SSIZE_T_DEFINED
#define _SSIZE_T_DEFINED
#undef ssize_t
#if defined(_WIN64) || defined(__x86_64__)
	#if defined(__GNUC__) && defined(__STRICT_ANSI__)
		typedef signed int ssize_t __attribute__((mode(DI)));
	#else
		typedef signed long long ssize_t;
	#endif
#else
		typedef signed int ssize_t;
#endif
#endif

#define offsetof(type, member) ((size_t)&(((type *)0)->member))

 /*
 * For C++: NULL might be defined as 0 because C++ supports 0 as a null pointer constant.
 * For C: NULL is typically defined as '(void*)0' to ensure that it is treated as a
 *	null pointer constant.
 */
#if defined (__cplusplus)
	#define NULL 0
#else
	#define NULL ((void*)0)
#endif

#endif /* __STD_DEF_H__ */