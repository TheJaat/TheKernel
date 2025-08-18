#ifndef __INTERRUPTS_H_
#define __INTERRUPTS_H_

#include <defs.h>
#include <os/driver/interrupt.h>

// for NULL
#include <stddef.h>

/* Special flags that are available only
 * in kernel context for special interrupts */
#define INTERRUPT_KERNEL	0x10000000
#define INTERRUPT_SOFTWARE	0x20000000

/* Structures */
typedef struct _InterruptDescriptor {
	Interrupt_t		Interrupt;
	UUId_t                  Id;
	UUId_t			Ash;
	UUId_t			Thread;
	Flags_t			Flags;
	int			Source;
	struct _InterruptDescriptor	*Link;
} InterruptDescriptor_t;

/* InterruptInitialize
 * Initializes the interrupt-manager code
 * and initializes all the resources for
 * allocating and freeing interrupts */
__EXTERN void InterruptInitialize(void);

/* InterruptGet
 * Retrieves the given interrupt source information
 * as a InterruptDescriptor_t */
__EXTERN InterruptDescriptor_t *InterruptGet(UUId_t Source);


#endif
