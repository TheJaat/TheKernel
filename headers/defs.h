#ifndef __DEFS_H__
#define __DEFS_H__

#include <stdint.h>

#define MAX_SUPPORTED_CPUS			64

typedef uint32_t                    reg32_t;
typedef reg32_t                     reg_t;

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

typedef unsigned int	UUId_t;
typedef unsigned int	Flags_t;

#ifndef __EXTERN
	#define __EXTERN extern
#endif

#ifndef __CONST
	#define __CONST const
#endif


typedef enum {
	Success,
	Error
} OsStatus_t;

typedef enum {
    InterruptHandled,
    InterruptNotHandled
} InterruptStatus_t;

#define DIVUP(a, b)                             ((a / b) + (((a % b) > 0) ? 1 : 0))

#ifndef LOBYTE
	#define LOBYTE(l)	((uint8_t)(uint16_t)(l))
#endif

/* Data manipulation macros */
#ifndef LOWORD
	#define LOWORD(l)                               ((uint16_t)(uint32_t)(l))
#endif


/* Utils Definitions */
#define MIN(a,b)                                (((a)<(b))?(a):(b))
#define MAX(a,b)                                (((a)>(b))?(a):(b))
#define DIVUP(a, b)                             ((a / b) + (((a % b) > 0) ? 1 : 0))
#define INCLIMIT(i, limit)                      i++; if (i == limit) i = 0;
#define ADDLIMIT(Base, Current, Step, Limit)    ((Current + Step) >= Limit) ? Base : (Current + Step) 
#define ALIGN(Val, Alignment, Roundup)          ((Val & (Alignment-1)) > 0 ? (Roundup == 1 ? ((Val + Alignment) & ~(Alignment-1)) : Val & ~(Alignment-1)) : Val)
#define ISALIGNED(Val, Alignment)               ((Val & (Alignment-1)) == 0)
#define BOCHSBREAK                              __asm__ __volatile__ ("xchg %bx, %bx\n\t");


/* Time definitions that can help with 
 * conversion of the different time-units */
#define FSEC_PER_NSEC                           1000000L
#define NSEC_PER_MSEC                           1000L
#define MSEC_PER_SEC                            1000L
#define NSEC_PER_SEC                            1000000000L
#define FSEC_PER_SEC                            1000000000000000LL


#endif
