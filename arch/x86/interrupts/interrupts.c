#include <stdint.h>
#include <string.h>

#include <arch/x86/x32/context.h>
#include <system/log.h>

#include <interrupts/interrupts.h>
#include <os/driver/interrupt.h>
#include <arch/x86/x32/idt.h>
#include <arch/x86/x32/arch_x32.h>

/* Interrupts tables needed for
 * the x86-architecture */
InterruptDescriptor_t *InterruptTable[IDT_DESCRIPTORS];
int InterruptISATable[NUM_ISA_INTERRUPTS];
int g_InterruptInitialized = 0;
UUId_t InterruptIdGen = 0;

/* InterruptInitialize
 * Initializes the interrupt-manager code
 * and initializes all the resources for
 * allocating and freeing interrupts */
void InterruptInitialize(void)
{
	LogInformation("Interrupts", "Initializing");

	/* Null out interrupt tables */
	memset((void*)InterruptTable, 0, sizeof(InterruptDescriptor_t*) * IDT_DESCRIPTORS);
	memset((void*)&InterruptISATable, 0, sizeof(InterruptISATable));
	g_InterruptInitialized = 1;
	InterruptIdGen = 0;
}

/* InterruptGet
 * Retrieves the given interrupt source information
 * as a MCoreInterruptDescriptor_t */
InterruptDescriptor_t *InterruptGet(UUId_t Source)
{
	// Variables
	InterruptDescriptor_t *Iterator = NULL;
	uint16_t TableIndex = LOWORD(Source);

	// Iterate at the correct entry
	Iterator = InterruptTable[TableIndex];
	while (Iterator != NULL) {
		if (Iterator->Id == Source) {
			return Iterator;
		}
	}

	// We didn't find it
	return NULL;
}


/* InterruptEntry
 * The common entry point for interrupts, all
 * non-exceptions will enter here, lookup a handler
 * and execute the code
 */
void InterruptEntry(Context_t *Registers) {
// Handle the interrupts
	LogFatal("Interrupts", "InterruptEntry, Interrupt Occurred = %d", Registers->Irq);
	// For the time being, infinity and beyond
	while(1){}
}

/* ExceptionEntry
 * Common entry for all exceptions
 */
void ExceptionEntry(Context_t *Registers) {
// Handle the exceptions
	LogFatal("Interrupts", "ExceptionEntry, Exception Occurred = %d", Registers->Irq);
	// For the time being, infinity and beyond
	while(1){}
}
