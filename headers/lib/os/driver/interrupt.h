#ifndef _INTERRUPT_INTERFACE_H_
#define _INTERRUPT_INTERFACE_H_

/* Interrupt handler signature, this is only used for
 * fast-interrupts that does not need interrupts enabled
 * or does any processing work */
#ifndef __INTERRUPTHANDLER
#define __INTERRUPTHANDLER
	typedef InterruptStatus_t(*InterruptHandler_t)(void*);
#endif

/* Specail interrupt constants, use these when allocating
 * interrupts if neccessary */
#define INTERRUPT_NONE					(int)-1
#define INTERRUPT_MAXVECTORS			8

/* Interrupt allocation flags, interrupts are initially
 * always shareable */
#define INTERRUPT_NOTSHARABLE			0x00000001
#define INTERRUPT_FAST				0x00000002
#define INTERRUPT_MSI				0x00000004
#define INTERRUPT_VECTOR			0x00000008

/* The interrupt descriptor structure, this contains
 * information about the interrupt that needs to be registered
 * and special handling. */
typedef struct _Interrupt {
	// General information, note that these can change
	// after the RegisterInterruptSource, always use the value
	// in <Line> to see your allocated interrupt-line
	Flags_t		AcpiConform;
	int		Line;
	int		Pin;

	// If the system should choose the best available
	// between all directs, fill all unused entries with 
	// INTERRUPT_NONE. Specify INTERRUPT_VECTOR to use this.
	int		Vectors[INTERRUPT_MAXVECTORS];

	// Interrupt-handler and context for INTERRUPT_FAST
	InterruptHandler_t	FastHandler;
	void			*Data;

	// Read-Only
	uintptr_t		MsiAddress;	// INTERRUPT_MSI - The address of MSI
	uintptr_t		MsiValue;	// INTERRUPT_MSI - The value of MSI
} Interrupt_t;

#endif
