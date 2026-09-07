#include <stdint.h>
#include <string.h>

#include <arch/x86/x32/context.h>
#include <system/log.h>
#include <system/heap.h>

#include <interrupts/interrupts.h>
#include <os/driver/interrupt.h>
#include <arch/x86/x32/idt.h>
#include <arch/x86/x32/arch_x32.h>
#include <arch/x86/pic.h>
#include <system/timers.h>

/* Assembly helpers from irq.asm */
__EXTERN void ___cli(void);
__EXTERN void ___sti(void);
__EXTERN uint32_t ___getflags(void);
__EXTERN uint32_t ___getcr2(void);

#define EFLAGS_IF	0x200

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

	LogInformation("Interrupts", "Ready, %d vectors, %d ISA lines",
		IDT_DESCRIPTORS, NUM_ISA_INTERRUPTS);
}

/* InterruptIsEnabled */
int InterruptIsEnabled(void)
{
	return (___getflags() & EFLAGS_IF) ? 1 : 0;
}

/* InterruptDisable */
int InterruptDisable(void)
{
	int Was = InterruptIsEnabled();
	___cli();
	return Was;
}

/* InterruptEnable */
int InterruptEnable(void)
{
	int Was = InterruptIsEnabled();
	___sti();
	return Was;
}

/* InterruptSaveState */
int InterruptSaveState(void)
{
	return InterruptDisable();
}

/* InterruptRestoreState */
void InterruptRestoreState(int State)
{
	if (State) {
		___sti();
	}
	else {
		___cli();
	}
}

/* InterruptGet
 * Retrieves the given interrupt source information
 * as a InterruptDescriptor_t */
InterruptDescriptor_t *InterruptGet(UUId_t Source)
{
	// Variables
	InterruptDescriptor_t *Iterator = NULL;
	uint16_t TableIndex = LOWORD(Source);

	if (TableIndex >= IDT_DESCRIPTORS) {
		return NULL;
	}

	// Iterate at the correct entry
	Iterator = InterruptTable[TableIndex];
	while (Iterator != NULL) {
		if (Iterator->Id == Source) {
			return Iterator;
		}
		/* The original never advanced the iterator, so any lookup that
		 * did not match on the first entry span forever. */
		Iterator = Iterator->Link;
	}

	// We didn't find it
	return NULL;
}

/* InterruptRegister */
UUId_t InterruptRegister(Interrupt_t *Interrupt, Flags_t Flags)
{
	InterruptDescriptor_t *Entry = NULL;
	InterruptDescriptor_t *Iterator = NULL;
	UUId_t TableIndex = 0;
	UUId_t Id;
	int State;

	if (g_InterruptInitialized != 1) {
		LogFatal("Interrupts", "register before InterruptInitialize");
		return UUID_INVALID;
	}
	if (Interrupt == NULL || Interrupt->FastHandler == NULL) {
		LogFatal("Interrupts", "register with no handler");
		return UUID_INVALID;
	}

	/* Work out which IDT vector this belongs on. Only kernel handlers
	 * are supported until there is a process model. */
	if (Flags & INTERRUPT_SOFTWARE) {
		if (Interrupt->Vectors[0] == INTERRUPT_NONE) {
			LogFatal("Interrupts", "software interrupt with no vector");
			return UUID_INVALID;
		}
		TableIndex = (UUId_t)Interrupt->Vectors[0];
	}
	else {
		if (Interrupt->Line < 0 || Interrupt->Line >= NUM_ISA_INTERRUPTS) {
			LogFatal("Interrupts", "line %d is out of range", Interrupt->Line);
			return UUID_INVALID;
		}
		if (Interrupt->Line == PIC_CASCADE_LINE) {
			LogFatal("Interrupts", "line 2 is the cascade, it cannot be claimed");
			return UUID_INVALID;
		}
		TableIndex = (UUId_t)(PIC_VECTOR_BASE + Interrupt->Line);
	}

	if (TableIndex >= IDT_DESCRIPTORS) {
		LogFatal("Interrupts", "vector %u is out of range", TableIndex);
		return UUID_INVALID;
	}

	/* A non-sharable source must be the only one on its vector. */
	if ((Flags & INTERRUPT_NOTSHARABLE) && InterruptTable[TableIndex] != NULL) {
		LogFatal("Interrupts", "vector %u is already taken and not sharable",
			TableIndex);
		return UUID_INVALID;
	}

	Entry = (InterruptDescriptor_t*)kmalloc(sizeof(InterruptDescriptor_t));
	if (Entry == NULL) {
		LogFatal("Interrupts", "out of memory registering vector %u", TableIndex);
		return UUID_INVALID;
	}

	/* Everything below touches the table the dispatcher walks, so keep
	 * interrupts off for it. */
	State = InterruptDisable();

	Id = InterruptIdGen++;
	memcpy(&Entry->Interrupt, Interrupt, sizeof(Interrupt_t));
	Entry->Id     = (Id << 16) | TableIndex;
	Entry->Ash    = UUID_INVALID;
	Entry->Thread = UUID_INVALID;   /* no threading yet */
	Entry->Flags  = Flags;
	Entry->Link   = NULL;
	Entry->Source = (Flags & INTERRUPT_SOFTWARE)
		? INTERRUPT_NONE : Interrupt->Line;

	/* Append, so handlers run in registration order. */
	if (InterruptTable[TableIndex] == NULL) {
		InterruptTable[TableIndex] = Entry;
	}
	else {
		Iterator = InterruptTable[TableIndex];
		while (Iterator->Link != NULL) {
			Iterator = Iterator->Link;
		}
		Iterator->Link = Entry;
	}

	/* Now that a handler exists, let the line through. */
	if (Entry->Source != INTERRUPT_NONE) {
		InterruptISATable[Entry->Source]++;
		PicUnmaskLine(Entry->Source);
	}

	InterruptRestoreState(State);

	LogInformation("Interrupts", "registered id 0x%x on vector %u (line %d)",
		Entry->Id, TableIndex, Entry->Source);
	return Entry->Id;
}

/* InterruptUnregister */
OsStatus_t InterruptUnregister(UUId_t Source)
{
	InterruptDescriptor_t *Iterator = NULL;
	InterruptDescriptor_t *Previous = NULL;
	uint16_t TableIndex = LOWORD(Source);
	int State;

	if (TableIndex >= IDT_DESCRIPTORS) {
		return Error;
	}

	State = InterruptDisable();

	Iterator = InterruptTable[TableIndex];
	while (Iterator != NULL) {
		if (Iterator->Id == Source) {
			break;
		}
		Previous = Iterator;
		Iterator = Iterator->Link;
	}

	if (Iterator == NULL) {
		InterruptRestoreState(State);
		LogFatal("Interrupts", "unregister of unknown id 0x%x", Source);
		return Error;
	}

	if (Previous == NULL) {
		InterruptTable[TableIndex] = Iterator->Link;
	}
	else {
		Previous->Link = Iterator->Link;
	}

	/* Mask the line again once nothing is listening on it. */
	if (Iterator->Source != INTERRUPT_NONE) {
		InterruptISATable[Iterator->Source]--;
		if (InterruptISATable[Iterator->Source] <= 0) {
			InterruptISATable[Iterator->Source] = 0;
			PicMaskLine(Iterator->Source);
		}
	}

	InterruptRestoreState(State);

	kfree(Iterator);
	return Success;
}

/* InterruptEntry
 * The common entry point for interrupts, all
 * non-exceptions will enter here, lookup a handler
 * and execute the code
 */
void InterruptEntry(Context_t *Registers)
{
	InterruptDescriptor_t *Entry = NULL;
	InterruptStatus_t Result = InterruptNotHandled;
	int TableIndex = (int)Registers->Irq + PIC_VECTOR_BASE;
	int Line = INTERRUPT_NONE;

	if (TableIndex < 0 || TableIndex >= IDT_DESCRIPTORS) {
		LogFatal("Interrupts", "vector %d out of range", TableIndex);
		return;
	}

	/* Only vectors 32..47 come from the PIC. Anything else is a
	 * software vector and must not be acknowledged to the chip. */
	if (TableIndex >= PIC_VECTOR_BASE
		&& TableIndex < (PIC_VECTOR_BASE + PIC_NUM_LINES)) {
		Line = TableIndex - PIC_VECTOR_BASE;

		/* A spurious line 7 or 15 has no ISR bit set. Line 7 must not
		 * be acknowledged at all; line 15 needs the master acknowledged
		 * because it did accept the cascade. */
		if (PicIsSpurious(Line)) {
			if (Line == 15) {
				PicSendEoi(0);
			}
			return;
		}
	}

	Entry = InterruptTable[TableIndex];
	while (Entry != NULL) {
		/* A handler with no Data gets the register context, which is
		 * what a kernel handler such as the scheduler tick wants. */
		if (Entry->Interrupt.Data == NULL) {
			Result = Entry->Interrupt.FastHandler((void*)Registers);
		}
		else {
			Result = Entry->Interrupt.FastHandler(Entry->Interrupt.Data);
		}

		if (Result == InterruptHandled) {
			/* Let the timer registry see it. It only acts if this is
			 * the active tick source, so the cost on every other
			 * interrupt is one comparison. */
			TimersInterrupt(Entry->Id);
			break;
		}
		Entry = Entry->Link;
	}

	/* Acknowledge before complaining, so an unclaimed line does not
	 * wedge the chip and stop every later interrupt. */
	if (Line != INTERRUPT_NONE) {
		PicSendEoi(Line);
	}

	if (Result != InterruptHandled) {
		LogFatal("Interrupts", "unhandled interrupt, vector %d (line %d)",
			TableIndex, Line);
	}
}

/* ExceptionName
 * Human-readable name for the first 32 vectors. */
static const char *ExceptionName(int Vector)
{
	switch (Vector) {
		case 0:  return "divide by zero";
		case 1:  return "debug";
		case 2:  return "non-maskable interrupt";
		case 3:  return "breakpoint";
		case 4:  return "overflow";
		case 5:  return "bound range exceeded";
		case 6:  return "invalid opcode";
		case 7:  return "device not available";
		case 8:  return "double fault";
		case 10: return "invalid tss";
		case 11: return "segment not present";
		case 12: return "stack-segment fault";
		case 13: return "general protection fault";
		case 14: return "page fault";
		case 16: return "x87 floating-point";
		case 17: return "alignment check";
		case 18: return "machine check";
		case 19: return "simd floating-point";
		default: return "reserved";
	}
}

/* ExceptionEntry
 * Common entry for all exceptions
 */
void ExceptionEntry(Context_t *Registers)
{
	int Vector = (int)Registers->Irq;

	LogFatal("Interrupts", "EXCEPTION %d: %s", Vector, ExceptionName(Vector));
	LogFatal("Interrupts", "eip 0x%x  cs 0x%x  eflags 0x%x  err 0x%x",
		Registers->Eip, Registers->Cs, Registers->Eflags, Registers->ErrorCode);
	LogFatal("Interrupts", "eax 0x%x  ebx 0x%x  ecx 0x%x  edx 0x%x",
		Registers->Eax, Registers->Ebx, Registers->Ecx, Registers->Edx);
	LogFatal("Interrupts", "esi 0x%x  edi 0x%x  ebp 0x%x  esp 0x%x",
		Registers->Esi, Registers->Edi, Registers->Ebp, Registers->Esp);

	/* A page fault says far more once you can see the faulting address
	 * and why it faulted. CR2 holds the address; the error code bits
	 * are present/write/user/reserved/instruction-fetch. */
	if (Vector == 14) {
		uint32_t Cr2 = ___getcr2();
		LogFatal("Interrupts", "faulting address 0x%x", Cr2);
		LogFatal("Interrupts", "  %s, on a %s, in %s mode%s",
			(Registers->ErrorCode & 0x1) ? "protection violation"
			                             : "page not present",
			(Registers->ErrorCode & 0x2) ? "write" : "read",
			(Registers->ErrorCode & 0x4) ? "user" : "kernel",
			(Registers->ErrorCode & 0x10) ? ", instruction fetch" : "");
	}

	// For the time being, infinity and beyond
	while(1){}
}