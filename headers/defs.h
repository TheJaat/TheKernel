#ifndef _DEFS_H
#define _DEFS_H

typedef enum {
    Success,
    Error
} OsStatus_t;


// Variable Width
#if defined(_X86_32) || defined(i386)
	#define __BITS                      32
	#define __MASK                      0xFFFFFFFF
	typedef unsigned int                PhysicalAddress_t;
	typedef unsigned int                VirtualAddress_t;
	//typedef reg32_t                     reg_t;
#elif defined(_X86_64)
	#define __BITS                      64
	#define __MASK                      0xFFFFFFFFFFFFFFFF
	typedef unsigned long long          PhysicalAddress_t;
	typedef unsigned long long          VirtualAddress_t;
	//typedef reg64_t                     reg_t;
#endif

#endif // _DEFS_H