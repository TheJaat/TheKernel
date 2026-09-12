#ifndef __PROCESS_H__
#define __PROCESS_H__

#include <defs.h>
#include <stddef.h>
#include <arch/x86/address_space.h>
#include <arch/x86/x32/gdt.h>

#define PROCESS_MAX                 16
#define PROCESS_NAME_LENGTH         32
#define PROCESS_MAX_HANDLES         32

/* Each thread gets its own ring-3 stack, carved downward from
 * MEMORY_LOCATION_RING3_HEAP with an unmapped guard page between them.
 * The gap is not wasted space: a thread overrunning its stack hits a
 * missing page and faults, rather than silently writing into the stack
 * of whichever thread was allocated before it. */
#define PROCESS_USER_STACK_SIZE     (PAGE_SIZE * 4)
#define PROCESS_STACK_GUARD         PAGE_SIZE

/* Privileges. A microkernel that lets any process claim IRQ 1 has moved
 * the drivers out of the kernel without moving the trust out. Servers
 * are loaded from the ramdisk at boot and marked; an application loaded
 * later is not. */
#define PROCESS_PRIV_NONE           0x0
#define PROCESS_PRIV_HARDWARE       0x1     /* may claim irqs and io-spaces */

typedef enum {
    HandleFree = 0,
    HandlePipe,
    HandleIoSpace,
    HandleInterrupt
} HandleType_t;

typedef struct _Handle {
    HandleType_t    Type;
    void           *Object;
    int             Owned;      /* close destroys the object */
} Handle_t;

typedef struct _Process {
    UUId_t          Id;
    char            Name[PROCESS_NAME_LENGTH];
    AddressSpace_t *AddressSpace;
    Flags_t         Privileges;

    uintptr_t       NextStackTop;
    int             Threads;

    Handle_t        Handles[PROCESS_MAX_HANDLES];
    int             Used;
} Process_t;

#ifdef __cplusplus
extern "C" {
#endif

void       ProcessInitialize(void);
Process_t *ProcessCreate(const char *Name, Flags_t Privileges);
Process_t *ProcessGet(UUId_t Id);
Process_t *ProcessGetCurrent(void);

/* ProcessAllocateUserStack
 * Maps a ring-3 stack into the process and returns its top, or 0. */
uintptr_t  ProcessAllocateUserStack(Process_t *Process);

/* Thread accounting. The last thread out destroys the process and its
 * address space - which is what the reference counting on
 * AddressSpace_t exists for. */
void       ProcessAddThread(Process_t *Process);
void       ProcessRemoveThread(Process_t *Process);

/* Handles. An index is only meaningful inside the owning process, which
 * is what stops one process naming another's objects. */
int        ProcessHandleAdd(Process_t *Process, HandleType_t Type,
                            void *Object, int Owned);
Handle_t  *ProcessHandleGet(Process_t *Process, int Index, HandleType_t Type);
OsStatus_t ProcessHandleClose(Process_t *Process, int Index);

size_t     ProcessGetCount(void);
void       ProcessPrint(void);

#ifdef __cplusplus
}
#endif

#endif /* __PROCESS_H__ */
