/* Includes
 * - System */
#include <system/process.h>
#include <system/threading.h>
#include <system/pipe.h>
#include <system/log.h>
#include <arch/x86/memory.h>
#include <arch/x86/x32/arch_x32.h>
#include <interrupts/interrupts.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

static Process_t GlbProcesses[PROCESS_MAX];
static UUId_t GlbProcessIds = 1;
static int GlbProcessInitialized = 0;

/* ProcessInitialize */
void ProcessInitialize(void)
{
    memset(GlbProcesses, 0, sizeof(GlbProcesses));
    GlbProcessIds = 1;
    GlbProcessInitialized = 1;
    LogInformation("Process", "Ready, %d slots, %d handles each",
        PROCESS_MAX, PROCESS_MAX_HANDLES);
}

/* ProcessSetName */
static void ProcessSetName(Process_t *Process, const char *Name)
{
    int i;

    for (i = 0; i < (PROCESS_NAME_LENGTH - 1) && Name != NULL
         && Name[i] != '\0'; i++) {
        Process->Name[i] = Name[i];
    }
    for (; i < PROCESS_NAME_LENGTH; i++) {
        Process->Name[i] = '\0';
    }
}

/* ProcessCreate */
Process_t *ProcessCreate(const char *Name, Flags_t Privileges)
{
    Process_t *Process = NULL;
    AddressSpace_t *Space;
    int State;
    int i;

    if (GlbProcessInitialized == 0) {
        LogFatal("Process", "create before ProcessInitialize");
        return NULL;
    }

    /* Outside the critical section - this allocates. */
    Space = AddressSpaceCreate(AS_TYPE_APPLICATION);
    if (Space == NULL) {
        return NULL;
    }

    State = InterruptDisable();

    for (i = 0; i < PROCESS_MAX; i++) {
        if (GlbProcesses[i].Used == 0) {
            Process = &GlbProcesses[i];
            break;
        }
    }

    if (Process == NULL) {
        InterruptRestoreState(State);
        AddressSpaceDestroy(Space);
        LogFatal("Process", "the process table is full");
        return NULL;
    }

    memset(Process, 0, sizeof(Process_t));
    Process->Id           = GlbProcessIds++;
    Process->AddressSpace = Space;
    Process->Privileges   = Privileges;
    Process->NextStackTop = MEMORY_LOCATION_RING3_HEAP;
    Process->Threads      = 0;
    Process->Used         = 1;
    ProcessSetName(Process, Name);

    InterruptRestoreState(State);

    LogInformation("Process", "created '%s' id %u%s",
        Process->Name, Process->Id,
        (Privileges & PROCESS_PRIV_HARDWARE) ? " (hardware)" : "");
    return Process;
}

/* ProcessGet */
Process_t *ProcessGet(UUId_t Id)
{
    int i;

    for (i = 0; i < PROCESS_MAX; i++) {
        if (GlbProcesses[i].Used != 0 && GlbProcesses[i].Id == Id) {
            return &GlbProcesses[i];
        }
    }
    return NULL;
}

/* ProcessGetCurrent */
Process_t *ProcessGetCurrent(void)
{
    Thread_t *Thread = ThreadingGetCurrentThread(0);

    if (Thread == NULL) {
        return NULL;
    }
    return (Process_t*)Thread->Process;
}

/* ProcessAllocateUserStack */
uintptr_t ProcessAllocateUserStack(Process_t *Process)
{
    uintptr_t Top, Base, Offset;

    if (Process == NULL || Process->AddressSpace == NULL) {
        return 0;
    }

    Top  = Process->NextStackTop;
    Base = Top - PROCESS_USER_STACK_SIZE;

    if (Base <= MEMORY_LOCATION_RING3_CODE) {
        LogFatal("Process", "'%s' is out of stack space", Process->Name);
        return 0;
    }

    for (Offset = 0; Offset < PROCESS_USER_STACK_SIZE; Offset += PAGE_SIZE) {
        PhysicalAddress_t Frame = MmPhysicalAllocateBlock(__MASK, 1);

        if (Frame == 0
            || MmVirtualMap(Process->AddressSpace->PageDirectory, Frame,
                   Base + Offset, PAGE_USER) != Success) {
            LogFatal("Process", "could not map a stack for '%s'", Process->Name);
            return 0;
        }
    }

    /* Step past a guard page so the next stack cannot touch this one. */
    Process->NextStackTop = Base - PROCESS_STACK_GUARD;
    return Top;
}

/* ProcessAddThread */
void ProcessAddThread(Process_t *Process)
{
    int State;

    if (Process == NULL) {
        return;
    }
    State = InterruptDisable();
    Process->Threads++;
    InterruptRestoreState(State);
}

/* ProcessCloseAllHandles
 * Caller must not hold the lock: closing a handle can free an object. */
static void ProcessCloseAllHandles(Process_t *Process)
{
    int i;

    for (i = 0; i < PROCESS_MAX_HANDLES; i++) {
        ProcessHandleClose(Process, i);
    }
}

/* ProcessRemoveThread */
void ProcessRemoveThread(Process_t *Process)
{
    AddressSpace_t *Space = NULL;
    int Teardown = 0;
    int State;

    if (Process == NULL) {
        return;
    }

    State = InterruptDisable();

    if (Process->Threads > 0) {
        Process->Threads--;
    }

    /* Only the last thread out tears the process down. This is exactly
     * why the address space is reference counted: with several threads
     * sharing one directory, the first to exit must not free it under
     * the others. */
    if (Process->Threads == 0 && Process->Used != 0) {
        Space = Process->AddressSpace;
        Process->AddressSpace = NULL;
        Teardown = 1;
    }

    InterruptRestoreState(State);

    if (Teardown) {
        ProcessCloseAllHandles(Process);
        Process->Used = 0;
        if (Space != NULL) {
            AddressSpaceDestroy(Space);
        }
    }
}

/* ProcessHandleAdd */
int ProcessHandleAdd(Process_t *Process, HandleType_t Type,
                     void *Object, int Owned)
{
    int Index = -1;
    int State;
    int i;

    if (Process == NULL || Object == NULL) {
        return -1;
    }

    State = InterruptDisable();

    for (i = 0; i < PROCESS_MAX_HANDLES; i++) {
        if (Process->Handles[i].Type == HandleFree) {
            Process->Handles[i].Type   = Type;
            Process->Handles[i].Object = Object;
            Process->Handles[i].Owned  = Owned;
            Index = i;
            break;
        }
    }

    InterruptRestoreState(State);
    return Index;
}

/* ProcessHandleGet
 * Type-checked on purpose. A process passing a pipe handle where an
 * io-space is expected must get an error, not a pointer the kernel then
 * treats as the wrong structure. */
Handle_t *ProcessHandleGet(Process_t *Process, int Index, HandleType_t Type)
{
    if (Process == NULL || Index < 0 || Index >= PROCESS_MAX_HANDLES) {
        return NULL;
    }
    if (Process->Handles[Index].Type != Type) {
        return NULL;
    }
    return &Process->Handles[Index];
}

/* ProcessHandleClose */
OsStatus_t ProcessHandleClose(Process_t *Process, int Index)
{
    HandleType_t Type;
    void *Object;
    int Owned;
    int State;

    if (Process == NULL || Index < 0 || Index >= PROCESS_MAX_HANDLES) {
        return Error;
    }

    State = InterruptDisable();
    Type   = Process->Handles[Index].Type;
    Object = Process->Handles[Index].Object;
    Owned  = Process->Handles[Index].Owned;
    Process->Handles[Index].Type   = HandleFree;
    Process->Handles[Index].Object = NULL;
    Process->Handles[Index].Owned  = 0;
    InterruptRestoreState(State);

    if (Type == HandleFree || Object == NULL || Owned == 0) {
        return (Type == HandleFree) ? Error : Success;
    }

    /* Destroy outside the critical section - these free memory. */
    switch (Type) {
        case HandlePipe:
            PipeDestroy((Pipe_t*)Object);
            break;
        default:
            break;
    }

    return Success;
}

/* ProcessGetCount */
size_t ProcessGetCount(void)
{
    size_t Count = 0;
    int i;

    for (i = 0; i < PROCESS_MAX; i++) {
        if (GlbProcesses[i].Used != 0) {
            Count++;
        }
    }
    return Count;
}

/* ProcessPrint */
void ProcessPrint(void)
{
    int i, j;

    for (i = 0; i < PROCESS_MAX; i++) {
        int Handles = 0;

        if (GlbProcesses[i].Used == 0) {
            continue;
        }
        for (j = 0; j < PROCESS_MAX_HANDLES; j++) {
            if (GlbProcesses[i].Handles[j].Type != HandleFree) {
                Handles++;
            }
        }
        LogInformation("Process", "  id %u  %-16s %d thread%s  %d handle%s%s",
            GlbProcesses[i].Id, GlbProcesses[i].Name,
            GlbProcesses[i].Threads, (GlbProcesses[i].Threads == 1) ? "" : "s",
            Handles, (Handles == 1) ? "" : "s",
            (GlbProcesses[i].Privileges & PROCESS_PRIV_HARDWARE) ? "  hw" : "");
    }
}
