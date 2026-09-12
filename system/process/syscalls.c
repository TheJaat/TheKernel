/* Includes
 * - System */
#include <os/syscalls.h>
#include <system/syscalls.h>
#include <system/process.h>
#include <system/threading.h>
#include <system/pipe.h>
#include <system/timers.h>
#include <system/heap.h>
#include <system/log.h>
#include <interrupts/interrupts.h>
#include <arch/x86/memory.h>
#include <arch/x86/x32/arch_x32.h>
#include <arch/x86/x32/context.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <stdio.h>
#include <string.h>

static Interrupt_t GlbSyscallInterrupt;
static volatile size_t GlbSyscallCount = 0;
static int GlbSyscallsInitialized = 0;

/* --- name registry ------------------------------------------------ */

typedef struct _NameEntry {
    char        Name[NAME_MAX_LENGTH];
    Pipe_t     *Pipe;
    UUId_t      Owner;
    int         Used;
} NameEntry_t;

#define NAME_MAX_ENTRIES    16
static NameEntry_t GlbNames[NAME_MAX_ENTRIES];

/* --- pointer validation ------------------------------------------- */

/* SyscallValidateBuffer
 * Confirms that [Buffer, Buffer+Length) is entirely inside the user half
 * and mapped user-accessible in the current address space.
 *
 * This is the single most important function in the syscall layer. A
 * missing check here is not a crash - it is a user program persuading
 * the kernel to read or write an arbitrary address in ring 0 on its
 * behalf. Every syscall taking a pointer must call it. */
static OsStatus_t SyscallValidateBuffer(const void *Buffer, size_t Length,
                                        int NeedWrite)
{
    uintptr_t Start, End, Page;

    if (Buffer == NULL || Length == 0) {
        return Error;
    }

    Start = (uintptr_t)Buffer;
    End   = Start + Length;

    /* Overflow, and anything reaching outside the user half. The upper
     * bound matters as much as the lower: a buffer starting in user
     * space and running into kernel space must be refused, not clamped. */
    if (End < Start) {
        return Error;
    }
    if (Start < MEMORY_LOCATION_RING3_CODE || End > MEMORY_LOCATION_RING3_HEAP) {
        return Error;
    }

    for (Page = Start & PAGE_MASK; Page < End; Page += PAGE_SIZE) {
        if (MmVirtualGetMapping(NULL, Page) == 0) {
            return Error;
        }
        /* The page must be reachable from ring 3. A kernel page that
         * happens to sit in this range would otherwise be readable. */
        if (MmVirtualGetPageFlags(NULL, Page, PAGE_USER) != Success) {
            return Error;
        }
        if (NeedWrite
            && MmVirtualGetPageFlags(NULL, Page, PAGE_WRITE) != Success) {
            return Error;
        }
    }

    return Success;
}

/* SyscallCopyInString
 * Copies a NUL-terminated user string into a kernel buffer, bounded.
 * Validates page by page as it goes, because the length is not known in
 * advance and the string may run off the end of a mapped page. */
static OsStatus_t SyscallCopyInString(const char *User, char *Out, size_t Max)
{
    size_t i;

    for (i = 0; i < (Max - 1); i++) {
        if (SyscallValidateBuffer(User + i, 1, 0) != Success) {
            return Error;
        }
        Out[i] = User[i];
        if (Out[i] == '\0') {
            return Success;
        }
    }

    Out[Max - 1] = '\0';
    return Success;
}

/* --- individual calls --------------------------------------------- */

static int SyscallWrite(const char *Buffer, size_t Length)
{
    size_t i;

    if (Length > 4096) {
        Length = 4096;
    }
    if (SyscallValidateBuffer(Buffer, Length, 0) != Success) {
        return SYSCALL_BADPOINTER;
    }

    for (i = 0; i < Length; i++) {
        char c = Buffer[i];
        if (c == '\0') {
            break;
        }
        printf("%c", c);
    }
    return (int)i;
}

static int SyscallPipeCreate(size_t Size, Flags_t Flags)
{
    Process_t *Process = ProcessGetCurrent();
    Pipe_t *Pipe;
    int Handle;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    if (Size < 2 || Size > 0x10000) {
        return SYSCALL_ERROR;
    }

    Pipe = PipeCreate(Size, Flags);
    if (Pipe == NULL) {
        return SYSCALL_ERROR;
    }

    Handle = ProcessHandleAdd(Process, HandlePipe, Pipe, 1);
    if (Handle < 0) {
        PipeDestroy(Pipe);
        return SYSCALL_ERROR;
    }
    return Handle;
}

static int SyscallPipeWrite(int Handle, const void *Buffer, size_t Length)
{
    Process_t *Process = ProcessGetCurrent();
    Handle_t *Entry;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    Entry = ProcessHandleGet(Process, Handle, HandlePipe);
    if (Entry == NULL) {
        return SYSCALL_BADHANDLE;
    }
    if (SyscallValidateBuffer(Buffer, Length, 0) != Success) {
        return SYSCALL_BADPOINTER;
    }

    return (int)PipeWrite((Pipe_t*)Entry->Object, (const uint8_t*)Buffer, Length);
}

static int SyscallPipeRead(int Handle, void *Buffer, size_t Length)
{
    Process_t *Process = ProcessGetCurrent();
    Handle_t *Entry;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    Entry = ProcessHandleGet(Process, Handle, HandlePipe);
    if (Entry == NULL) {
        return SYSCALL_BADHANDLE;
    }
    if (SyscallValidateBuffer(Buffer, Length, 1) != Success) {
        return SYSCALL_BADPOINTER;
    }

    /* This blocks. That is correct and is the whole point: a server
     * waiting for a request costs nothing while idle. */
    return (int)PipeRead((Pipe_t*)Entry->Object, (uint8_t*)Buffer, Length, 0);
}

static int SyscallPipeAvailable(int Handle)
{
    Process_t *Process = ProcessGetCurrent();
    Handle_t *Entry;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    Entry = ProcessHandleGet(Process, Handle, HandlePipe);
    if (Entry == NULL) {
        return SYSCALL_BADHANDLE;
    }
    return (int)PipeBytesAvailable((Pipe_t*)Entry->Object);
}

static int SyscallRegisterName(const char *UserName, int PipeHandle)
{
    Process_t *Process = ProcessGetCurrent();
    char Name[NAME_MAX_LENGTH];
    Handle_t *Entry;
    int State, i, Slot = -1;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    if (SyscallCopyInString(UserName, Name, NAME_MAX_LENGTH) != Success) {
        return SYSCALL_BADPOINTER;
    }
    Entry = ProcessHandleGet(Process, PipeHandle, HandlePipe);
    if (Entry == NULL) {
        return SYSCALL_BADHANDLE;
    }

    State = InterruptDisable();
    for (i = 0; i < NAME_MAX_ENTRIES; i++) {
        if (GlbNames[i].Used != 0 && strcmp(GlbNames[i].Name, Name) == 0) {
            InterruptRestoreState(State);
            return SYSCALL_ERROR;       /* already taken */
        }
        if (GlbNames[i].Used == 0 && Slot < 0) {
            Slot = i;
        }
    }
    if (Slot < 0) {
        InterruptRestoreState(State);
        return SYSCALL_ERROR;
    }

    memcpy(GlbNames[Slot].Name, Name, NAME_MAX_LENGTH);
    GlbNames[Slot].Pipe  = (Pipe_t*)Entry->Object;
    GlbNames[Slot].Owner = Process->Id;
    GlbNames[Slot].Used  = 1;
    InterruptRestoreState(State);

    LogInformation("Syscall", "'%s' registered by process %u",
        Name, Process->Id);
    return SYSCALL_OK;
}

static int SyscallLookupName(const char *UserName)
{
    Process_t *Process = ProcessGetCurrent();
    char Name[NAME_MAX_LENGTH];
    Pipe_t *Pipe = NULL;
    int i;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    if (SyscallCopyInString(UserName, Name, NAME_MAX_LENGTH) != Success) {
        return SYSCALL_BADPOINTER;
    }

    for (i = 0; i < NAME_MAX_ENTRIES; i++) {
        if (GlbNames[i].Used != 0 && strcmp(GlbNames[i].Name, Name) == 0) {
            Pipe = GlbNames[i].Pipe;
            break;
        }
    }
    if (Pipe == NULL) {
        return SYSCALL_NOTFOUND;
    }

    /* Owned = 0: the looked-up pipe belongs to the server. Closing this
     * handle must not destroy it, or any client could take down the
     * server's request channel by exiting. */
    return ProcessHandleAdd(Process, HandlePipe, Pipe, 0);
}

/* --- dispatch ------------------------------------------------------ */

static InterruptStatus_t SyscallHandler(void *Data)
{
    Context_t *Registers = (Context_t*)Data;
    uint32_t Number;
    int Result = SYSCALL_ERROR;

    if (Registers == NULL) {
        return InterruptNotHandled;
    }

    Number = Registers->Eax;
    GlbSyscallCount++;

    switch (Number) {
        case SYS_EXIT:
            LogInformation("Syscall", "thread %u exited with %d",
                ThreadingGetCurrentThreadId(), (int)Registers->Ebx);
            ThreadingExit();
            break;

        case SYS_WRITE:
            Result = SyscallWrite((const char*)Registers->Ebx,
                (size_t)Registers->Ecx);
            break;

        case SYS_SLEEP:
            SleepMs((size_t)Registers->Ebx);
            Result = SYSCALL_OK;
            break;

        case SYS_GETMS:
            Result = (int)TimersGetSystemMs();
            break;

        case SYS_GETTID:
            Result = (int)ThreadingGetCurrentThreadId();
            break;

        case SYS_GETPID: {
            Process_t *Process = ProcessGetCurrent();
            Result = (Process != NULL) ? (int)Process->Id : SYSCALL_ERROR;
            break;
        }

        case SYS_YIELD:
            ThreadingYield();
            Result = SYSCALL_OK;
            break;

        case SYS_HANDLE_CLOSE: {
            Process_t *Process = ProcessGetCurrent();
            Result = (Process != NULL
                && ProcessHandleClose(Process, (int)Registers->Ebx) == Success)
                ? SYSCALL_OK : SYSCALL_BADHANDLE;
            break;
        }

        case SYS_PIPE_CREATE:
            Result = SyscallPipeCreate((size_t)Registers->Ebx,
                (Flags_t)Registers->Ecx);
            break;

        case SYS_PIPE_WRITE:
            Result = SyscallPipeWrite((int)Registers->Ebx,
                (const void*)Registers->Ecx, (size_t)Registers->Edx);
            break;

        case SYS_PIPE_READ:
            Result = SyscallPipeRead((int)Registers->Ebx,
                (void*)Registers->Ecx, (size_t)Registers->Edx);
            break;

        case SYS_PIPE_AVAILABLE:
            Result = SyscallPipeAvailable((int)Registers->Ebx);
            break;

        case SYS_REGISTER_NAME:
            Result = SyscallRegisterName((const char*)Registers->Ebx,
                (int)Registers->Ecx);
            break;

        case SYS_LOOKUP_NAME:
            Result = SyscallLookupName((const char*)Registers->Ebx);
            break;

        default:
            LogFatal("Syscall", "thread %u made unknown call %u",
                ThreadingGetCurrentThreadId(), Number);
            Result = SYSCALL_ERROR;
            break;
    }

    Registers->Eax = (uint32_t)Result;
    return InterruptHandled;
}

/* SyscallsInitialize */
OsStatus_t SyscallsInitialize(void)
{
    int i;

    if (GlbSyscallsInitialized) {
        return Success;
    }

    memset(GlbNames, 0, sizeof(GlbNames));
    memset(&GlbSyscallInterrupt, 0, sizeof(Interrupt_t));
    for (i = 0; i < INTERRUPT_MAXVECTORS; i++) {
        GlbSyscallInterrupt.Vectors[i] = INTERRUPT_NONE;
    }
    GlbSyscallInterrupt.Vectors[0]  = SYSCALL_VECTOR;
    GlbSyscallInterrupt.Line        = INTERRUPT_NONE;
    GlbSyscallInterrupt.Pin         = INTERRUPT_NONE;
    GlbSyscallInterrupt.FastHandler = SyscallHandler;
    /* NULL on purpose: InterruptEntry passes the Context_t when Data is
     * NULL, and the register frame is how arguments arrive. */
    GlbSyscallInterrupt.Data        = NULL;

    if (InterruptRegister(&GlbSyscallInterrupt,
            INTERRUPT_KERNEL | INTERRUPT_SOFTWARE | INTERRUPT_FAST)
        == UUID_INVALID) {
        LogFatal("Syscall", "could not register vector %d", SYSCALL_VECTOR);
        return Error;
    }

    GlbSyscallsInitialized = 1;
    LogInformation("Syscall", "Ready, %d calls on vector %d",
        SYS_MAX, SYSCALL_VECTOR);
    return Success;
}

size_t SyscallsGetCount(void) { return GlbSyscallCount; }

/* SyscallsPrintNames */
void SyscallsPrintNames(void)
{
    int i;

    for (i = 0; i < NAME_MAX_ENTRIES; i++) {
        if (GlbNames[i].Used != 0) {
            LogInformation("Syscall", "  %-16s owned by process %u",
                GlbNames[i].Name, GlbNames[i].Owner);
        }
    }
}
