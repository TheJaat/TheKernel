#ifndef __STD_INT_H__
#define __STD_INT_H__

// Fixed-width integer types
// 8 bits = 1 byte
typedef signed char int8_t;
typedef unsigned char uint8_t;


// 16 bits = 2 bytes
typedef short int16_t;
typedef unsigned short uint16_t;


// 32 bits = 4 bytes
typedef int int32_t;
typedef unsigned int uint32_t;

// 64 bits = 8 bytes
typedef long long int int64_t;
typedef unsigned long long uint64_t;


// uintptr_t declaration
/*
* uintptr_t is used for casting pointer to integer types
* and is defined to be large enough to hold a pointer on
* the target platform.
* On 32-bit systems: It is typically a 32-bit unsigned int.
* On 64-bit systems: It is typically a 64-bit unsigned int.
*/
// __SIZEOF_POINTER__ is used by GCC and clang (but not by MSVC - Microsoft Visual C++) to indicate the size of pointer in bytes.
#ifdef __SIZEOF_POINTER__
	#if __SIZEOF_POINTER__ == 8
		typedef uint64_t uintptr_t;	// 64-bit pointer size
	#elif __SIZEOF_POINTER__ == 4
		typedef uint32_t uintptr_t;	// 32-bit pointer size
	#else
		#error "Unsupported pointer size"
	#endif
#else
	// GCC/Clang: _x86_64 is predefined macro to determine the target is 32-bit or 64-bit
	// MSVC: _WIN64 for 64-bit targets and absence of _WIN64 for 32-bit targets.
	#if defined(__x86_64__) || defined(_WIN64)
		typedef uint64_t uintptr_t; // 64-bit
	#else
		typedef uint32_t uintptr_t; // 32-bit
	#endif
#endif

#endif /* __STDINT_H__ */