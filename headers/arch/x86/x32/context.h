#ifndef __CONTEXT_H__
#define __CONTEXT_H__

/* Includes
 * - Library */
#include <defs.h>

/* The context depends on the current running 
 * architecture - and describes which kind of
 * information is stored for each context */
#if defined(_X86_32) || defined(i386)
typedef struct Context {
	uint32_t                Edi;
	uint32_t                Esi;
	uint32_t                Ebp;
	uint32_t                Esp;
	uint32_t                Ebx;
	uint32_t                Edx;
	uint32_t                Ecx;
	uint32_t                Eax;
			                
	uint32_t                Gs;
	uint32_t                Fs;
	uint32_t                Es;
	uint32_t                Ds;
			                
	uint32_t                Irq;
	uint32_t                ErrorCode;
	uint32_t                Eip;
	uint32_t                Cs;
	uint32_t                Eflags;
			                
	uint32_t                UserEsp;
	uint32_t                UserSs;
	uint32_t                UserArg;
			                
	uint32_t                Reserved[4];
} __attribute__((packed)) Context_t;

#else
	#error "context.h: Invalid architecture"
#endif

#endif /* __CONTEXT_H__ */

