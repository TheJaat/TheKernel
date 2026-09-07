#ifndef __INTERRUPTS_H_
#define __INTERRUPTS_H_

#include <defs.h>
#include <os/driver/interrupt.h>
#include <arch/x86/x32/context.h>

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

#ifdef __cplusplus
extern "C" {
#endif

/* InterruptInitialize
 * Initializes the interrupt-manager code
 * and initializes all the resources for
 * allocating and freeing interrupts */
__EXTERN void InterruptInitialize(void);

/* InterruptRegister
 * Claims an interrupt source described by <Interrupt> and installs its
 * handler. Returns the id of the interrupt, or UUID_INVALID on failure.
 *
 * Two cases are supported at the moment:
 *   INTERRUPT_KERNEL                  - a device line; Interrupt->Line
 *                                       selects the IRQ, which is then
 *                                       unmasked on the PIC.
 *   INTERRUPT_KERNEL|INTERRUPT_SOFTWARE - a software vector; the IDT
 *                                       entry is taken from
 *                                       Interrupt->Vectors[0] and no
 *                                       hardware line is touched.
 *
 * The handler goes in Interrupt->FastHandler and is passed
 * Interrupt->Data, or the Context_t if Data is NULL. */
__EXTERN UUId_t InterruptRegister(Interrupt_t *Interrupt, Flags_t Flags);

/* InterruptUnregister
 * Removes a previously registered handler. The line is masked again once
 * its last handler is gone. */
__EXTERN OsStatus_t InterruptUnregister(UUId_t Source);

/* InterruptGet
 * Retrieves the given interrupt source information
 * as a InterruptDescriptor_t */
__EXTERN InterruptDescriptor_t *InterruptGet(UUId_t Source);

/* InterruptDisable / InterruptEnable
 * Clear or set EFLAGS.IF. Both return the state as it was *before* the
 * call, so it can be handed to InterruptRestoreState. */
__EXTERN int InterruptDisable(void);
__EXTERN int InterruptEnable(void);

/* InterruptSaveState / InterruptRestoreState
 * The pair to bracket a critical section with. */
__EXTERN int InterruptSaveState(void);
__EXTERN void InterruptRestoreState(int State);

/* InterruptIsEnabled
 * Returns 1 if EFLAGS.IF is currently set. */
__EXTERN int InterruptIsEnabled(void);

#ifdef __cplusplus
}
#endif

#endif