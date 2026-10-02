/* Includes
 * - System */
#include <os/syscalls.h>
#include <system/syscalls.h>
#include <system/process.h>
#include <system/userirq.h>
#include <system/endpoint.h>
#include <system/shm.h>
#include <os/registry.h>
#include <system/moduleloader.h>
#include <system/threading.h>
#include <system/pipe.h>
#include <boot/ramdisk.h>
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

/* --- bootstrap ------------------------------------------------------ */

/* The endpoint every new process is handed at HANDLE_REGISTRY.
 *
 * This is all the kernel knows about naming, and it is one pointer. A
 * process holding no capabilities can reach nothing, so something must
 * be given to it at birth - that much is unavoidable mechanism. Which
 * names exist, who may claim them, and when they expire are decisions,
 * and decisions live in the registry server. */
static Endpoint_t *GlbRegistryEndpoint = NULL;

/* SyscallSetRegistry
 * Nominates the caller's endpoint as the one new processes inherit.
 *
 * Servers only, and only once. A second nomination would let a later
 * process replace every subsequent process's view of the world, which
 * is the one thing the bootstrap must not permit. */
static int SyscallSetRegistry(int EndpointHandle)
{
    Process_t *Process = ProcessGetCurrent();
    Handle_t *Entry;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    if (!(Process->Privileges & PROCESS_PRIV_HARDWARE)) {
        return SYSCALL_DENIED;
    }
    if (GlbRegistryEndpoint != NULL) {
        LogFatal("Syscall", "a registry is already nominated");
        return SYSCALL_DENIED;
    }

    Entry = ProcessHandleGet(Process, EndpointHandle, HandleEndpoint);
    if (Entry == NULL) {
        return SYSCALL_BADHANDLE;
    }

    GlbRegistryEndpoint = (Endpoint_t*)Entry->Object;
    LogInformation("Syscall", "process %u is now the registry", Process->Id);
    return SYSCALL_OK;
}

/* SyscallsGrantRegistry
 * Hands a new process its registry capability. Called by the loader
 * before the process runs: a process cannot ask for this, because
 * asking would need a capability it does not have yet.
 *
 * The badge is the new process's id, so the registry learns who is
 * calling without the caller having any say in it. */
void SyscallsGrantRegistry(void *ProcessPtr)
{
    Process_t *Process = (Process_t*)ProcessPtr;

    if (GlbRegistryEndpoint == NULL || Process == NULL) {
        return;
    }

    /* Not owned - a process closing handle 0 must not destroy the
     * registry's endpoint for everyone else. */
    ProcessHandleAddBadged(Process, HandleEndpoint, GlbRegistryEndpoint,
        0, (unsigned int)Process->Id);
}

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
    /* Anything at or above the end of the kernel half may be a legal
     * user address. Naming the specific ring-3 sub-regions here was a
     * mistake: the bound said "code and stacks", so every pointer into
     * a shared region or a device mapping - both of which live at
     * MEMORY_LOCATION_RING3_IOSPACE, far above it - was rejected as a
     * bad pointer.
     *
     * The real protection is per page, below: a page is only acceptable
     * if it is mapped and carries PAGE_USER. A kernel page in this range
     * does not, so widening the bound costs nothing. The bound exists
     * only to reject addresses that are obviously the kernel's. */
    if (Start < MEMORY_LOCATION_KERNEL_END) {
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



/* SyscallSetReply
 * Nominates one of the caller's pipes as the channel replies arrive on.
 * A process has exactly one, which is what makes SYS_OPEN_REPLY
 * unambiguous. */
static int SyscallSetReply(int PipeHandle)
{
    Process_t *Process = ProcessGetCurrent();
    Handle_t *Entry;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    Entry = ProcessHandleGet(Process, PipeHandle, HandlePipe);
    if (Entry == NULL) {
        return SYSCALL_BADHANDLE;
    }
    if (ProcessSetReplyPipe(Process, Entry->Object) != Success) {
        return SYSCALL_ERROR;
    }
    return SYSCALL_OK;
}

/* SyscallOpenReply
 * Returns a handle to <ProcessId>'s reply channel.
 *
 * This is the only way one process obtains a reference to another's
 * pipe, and it is deliberately narrow: the target must have nominated a
 * reply channel itself, and the handle is not owned, so closing it
 * cannot destroy the other process's pipe.
 *
 * It is also why a client's identity in an RPC header cannot be forged
 * usefully - the kernel fills nothing in, but a server replying to a
 * claimed pid reaches that process's own channel and nobody else's. */
static int SyscallOpenReply(UUId_t ProcessId)
{
    Process_t *Process = ProcessGetCurrent();
    void *Pipe;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }

    Pipe = ProcessGetReplyPipe(ProcessId);
    if (Pipe == NULL) {
        return SYSCALL_NOTFOUND;
    }

    return ProcessHandleAdd(Process, HandlePipe, Pipe, 0);
}

/* --- hardware delegation ------------------------------------------- */

/* SyscallIrqRegister
 * Claims an interrupt line for the calling process.
 *
 * The privilege check is the whole point of PROCESS_PRIV_HARDWARE.
 * Without it any program could claim IRQ 1 and starve the real keyboard
 * driver - the drivers would have left the kernel but the trust would
 * not have. */
static int SyscallIrqRegister(int Line)
{
    Process_t *Process = ProcessGetCurrent();
    UserInterrupt_t *Entry;
    int Handle;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    if (!(Process->Privileges & PROCESS_PRIV_HARDWARE)) {
        LogFatal("Syscall", "process %u may not claim interrupts",
            Process->Id);
        return SYSCALL_DENIED;
    }

    Entry = UserIrqRegister(Process, Line);
    if (Entry == NULL) {
        return SYSCALL_ERROR;
    }

    Handle = ProcessHandleAdd(Process, HandleInterrupt, Entry, 1);
    if (Handle < 0) {
        UserIrqUnregister(Entry);
        return SYSCALL_ERROR;
    }
    return Handle;
}

static int SyscallIrqWait(int HandleIndex)
{
    Process_t *Process = ProcessGetCurrent();
    Handle_t *Entry;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    Entry = ProcessHandleGet(Process, HandleIndex, HandleInterrupt);
    if (Entry == NULL) {
        return SYSCALL_BADHANDLE;
    }
    return UserIrqWait((UserInterrupt_t*)Entry->Object);
}

static int SyscallIrqAck(int HandleIndex)
{
    Process_t *Process = ProcessGetCurrent();
    Handle_t *Entry;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    Entry = ProcessHandleGet(Process, HandleIndex, HandleInterrupt);
    if (Entry == NULL) {
        return SYSCALL_BADHANDLE;
    }
    return (UserIrqAcknowledge((UserInterrupt_t*)Entry->Object) == Success)
        ? SYSCALL_OK : SYSCALL_ERROR;
}

/* SyscallIoRequest
 * Opens a port range in the calling process's I/O permission bitmap.
 * After this the process executes in/out directly - the cpu checks the
 * bitmap, so there is no further syscall and no cost per access. */
static int SyscallIoRequest(unsigned Port, unsigned Count)
{
    Process_t *Process = ProcessGetCurrent();

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    if (!(Process->Privileges & PROCESS_PRIV_HARDWARE)) {
        LogFatal("Syscall", "process %u may not request io ports",
            Process->Id);
        return SYSCALL_DENIED;
    }
    if (Port > 0xFFFF || Count == 0 || Count > 32) {
        return SYSCALL_ERROR;
    }

    return (ProcessGrantPorts(Process, (uint16_t)Port, Count) == Success)
        ? SYSCALL_OK : SYSCALL_ERROR;
}

/* SyscallIoMap
 * Maps a device aperture into the calling process. */
static int SyscallIoMap(unsigned Physical, unsigned Length)
{
    Process_t *Process = ProcessGetCurrent();

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    if (!(Process->Privileges & PROCESS_PRIV_HARDWARE)) {
        LogFatal("Syscall", "process %u may not map devices", Process->Id);
        return SYSCALL_DENIED;
    }
    if (Length == 0 || Length > 0x100000) {
        return SYSCALL_ERROR;
    }

    /* Refusing anything below the end of the kernel half is the check
     * that stops a "driver" mapping ordinary RAM - or the kernel's own
     * image - and reading it from ring 3. */
    if (Physical < MEMORY_LOCATION_KERNEL_END) {
        LogFatal("Syscall", "process %u asked to map 0x%x, which is not "
            "device memory", Process->Id, Physical);
        return SYSCALL_DENIED;
    }

    return (int)ProcessMapDevice(Process, (uintptr_t)Physical, Length);
}

/* SyscallSpawn
 * Starts another server from the ramdisk. Servers only: this is how a
 * supervisor restarts a driver that died, and it must not be something
 * an application can do. */
static int SyscallSpawn(const char *UserName)
{
    Process_t *Process = ProcessGetCurrent();
    char Name[RAMDISK_NAME_LENGTH];
    UUId_t Id;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    if (!(Process->Privileges & PROCESS_PRIV_HARDWARE)) {
        return SYSCALL_DENIED;
    }
    if (SyscallCopyInString(UserName, Name, RAMDISK_NAME_LENGTH) != Success) {
        return SYSCALL_BADPOINTER;
    }

    Id = ModuleLoadServerId(Name);
    return (Id == UUID_INVALID) ? SYSCALL_ERROR : (int)Id;
}

/* SyscallProcessAlive */
static int SyscallProcessAlive(UUId_t Id)
{
    return (ProcessGet(Id) != NULL) ? 1 : 0;
}



/* The pipe the shell reads keystrokes from.
 *
 * With naming in user space the kernel cannot look a driver up by name,
 * and it should not: a name table is exactly what was moved out. So the
 * keyboard driver nominates its pipe explicitly, the same shape as the
 * registry nomination - one pointer, handed over deliberately, with no
 * namespace behind it. */
static Pipe_t *GlbConsoleInput = NULL;

static int SyscallSetConsoleInput(int PipeHandle)
{
    Process_t *Process = ProcessGetCurrent();
    Handle_t *Entry;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    if (!(Process->Privileges & PROCESS_PRIV_HARDWARE)) {
        return SYSCALL_DENIED;
    }

    Entry = ProcessHandleGet(Process, PipeHandle, HandlePipe);
    if (Entry == NULL) {
        return SYSCALL_BADHANDLE;
    }

    GlbConsoleInput = (Pipe_t*)Entry->Object;
    LogInformation("Syscall", "process %u now drives console input",
        Process->Id);
    return SYSCALL_OK;
}

/* SyscallsGetConsoleInput */
void *SyscallsGetConsoleInput(void)
{
    return GlbConsoleInput;
}

/* --- synchronous ipc ----------------------------------------------- */

static int SyscallEndpointCreate(void)
{
    Process_t *Process = ProcessGetCurrent();
    Endpoint_t *Endpoint;
    int Handle;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }

    Endpoint = EndpointCreate(Process);
    if (Endpoint == NULL) {
        return SYSCALL_ERROR;
    }

    Handle = ProcessHandleAdd(Process, HandleEndpoint, Endpoint, 1);
    if (Handle < 0) {
        EndpointDestroy(Endpoint);
        return SYSCALL_ERROR;
    }
    return Handle;
}

static int SyscallCall(const SysCallArgs_t *UserArgs)
{
    Process_t *Process = ProcessGetCurrent();
    SysCallArgs_t Args;
    Handle_t *Entry;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    if (SyscallValidateBuffer(UserArgs, sizeof(Args), 0) != Success) {
        return SYSCALL_BADPOINTER;
    }
    memcpy(&Args, UserArgs, sizeof(Args));

    Entry = ProcessHandleGet(Process, Args.Endpoint, HandleEndpoint);
    if (Entry == NULL) {
        return SYSCALL_BADHANDLE;
    }
    if (Args.SendLength > IPC_MESSAGE_MAX
        || Args.RecvLength > IPC_MESSAGE_MAX) {
        return SYSCALL_ERROR;
    }
    if (Args.SendLength > 0
        && SyscallValidateBuffer(Args.SendBuffer, Args.SendLength, 0) != Success) {
        return SYSCALL_BADPOINTER;
    }
    if (Args.RecvLength > 0
        && SyscallValidateBuffer(Args.RecvBuffer, Args.RecvLength, 1) != Success) {
        return SYSCALL_BADPOINTER;
    }

    /* The badge comes from the capability, not from the caller. A
     * client cannot claim to be someone else because it never supplies
     * the value. */
    return EndpointCall((Endpoint_t*)Entry->Object, Entry->Badge,
        Args.Opcode, Args.SendBuffer, Args.SendLength,
        Args.RecvBuffer, Args.RecvLength, (size_t)Args.Timeout);
}

static int SyscallRecv(SysRecvArgs_t *UserArgs)
{
    Process_t *Process = ProcessGetCurrent();
    SysRecvArgs_t Args;
    Handle_t *Entry;
    unsigned Opcode = 0, Badge = 0;
    int Result;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    if (SyscallValidateBuffer(UserArgs, sizeof(Args), 1) != Success) {
        return SYSCALL_BADPOINTER;
    }
    memcpy(&Args, UserArgs, sizeof(Args));

    Entry = ProcessHandleGet(Process, Args.Endpoint, HandleEndpoint);
    if (Entry == NULL) {
        return SYSCALL_BADHANDLE;
    }
    if (Args.Length > 0
        && SyscallValidateBuffer(Args.Buffer, Args.Length, 1) != Success) {
        return SYSCALL_BADPOINTER;
    }

    Result = EndpointReceive((Endpoint_t*)Entry->Object, Args.Buffer,
        Args.Length, &Opcode, &Badge, (size_t)Args.Timeout);

    if (Result >= 0) {
        UserArgs->Length = (unsigned)Result;
        UserArgs->Opcode = Opcode;
        UserArgs->Badge  = Badge;
    }
    return Result;
}

static int SyscallReply(const SysReplyArgs_t *UserArgs)
{
    SysReplyArgs_t Args;

    if (ProcessGetCurrent() == NULL) {
        return SYSCALL_DENIED;
    }
    if (SyscallValidateBuffer(UserArgs, sizeof(Args), 0) != Success) {
        return SYSCALL_BADPOINTER;
    }
    memcpy(&Args, UserArgs, sizeof(Args));

    if (Args.Length > IPC_MESSAGE_MAX) {
        return SYSCALL_ERROR;
    }
    if (Args.Length > 0
        && SyscallValidateBuffer(Args.Buffer, Args.Length, 0) != Success) {
        return SYSCALL_BADPOINTER;
    }

    return EndpointReply(Args.Buffer, Args.Length);
}

/* --- capabilities --------------------------------------------------- */

/* SyscallCapGrant
 * Hands a copy of one of the caller's capabilities to another process,
 * stamped with a badge of the granter's choosing.
 *
 * The copy is not Owned: the receiver closing it must not destroy the
 * granter's object. And only endpoints and shared memory can be
 * granted - handing over an interrupt or an io-space would transfer
 * hardware access that the privilege check was supposed to gate. */
static int SyscallCapGrant(const SysGrantArgs_t *UserArgs)
{
    Process_t *Process = ProcessGetCurrent();
    Process_t *Target;
    SysGrantArgs_t Args;
    Handle_t *Entry;
    int Index;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    if (SyscallValidateBuffer(UserArgs, sizeof(Args), 0) != Success) {
        return SYSCALL_BADPOINTER;
    }
    memcpy(&Args, UserArgs, sizeof(Args));

    Target = ProcessGet((UUId_t)Args.Process);
    if (Target == NULL) {
        return SYSCALL_NOTFOUND;
    }

    Entry = ProcessHandleGet(Process, Args.Handle, HandleEndpoint);
    if (Entry == NULL) {
        Entry = ProcessHandleGet(Process, Args.Handle, HandlePipe);
        if (Entry != NULL) {
            /* A pipe is as grantable as an endpoint. The copy is not
             * owned, so the receiver closing it cannot destroy the
             * granter's pipe - the same rule that makes a looked-up
             * service safe to hold. */
            Index = ProcessHandleAddBadged(Target, HandlePipe, Entry->Object,
                0, Args.Badge);
            if (Index < 0) {
                return SYSCALL_ERROR;
            }
            return Index;
        }
        Entry = ProcessHandleGet(Process, Args.Handle, HandleShm);
        if (Entry == NULL) {
            return SYSCALL_BADHANDLE;
        }
        {
            SharedMemory_t *Region = (SharedMemory_t*)Entry->Object;
            int State = InterruptDisable();
            Region->References++;
            InterruptRestoreState(State);
        }
        Index = ProcessHandleAddBadged(Target, HandleShm, Entry->Object,
            1, Args.Badge);
    }
    else {
        Index = ProcessHandleAddBadged(Target, HandleEndpoint, Entry->Object,
            0, Args.Badge);
    }

    if (Index < 0) {
        return SYSCALL_ERROR;
    }

    LogInformation("Syscall", "process %u granted a capability to %u, badge %u",
        Process->Id, Target->Id, Args.Badge);
    return Index;
}

/* --- shared memory -------------------------------------------------- */

static int SyscallShmCreate(unsigned Length)
{
    Process_t *Process = ProcessGetCurrent();
    SharedMemory_t *Region;
    int Handle;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }

    Region = ShmCreate(Process, (size_t)Length);
    if (Region == NULL) {
        return SYSCALL_ERROR;
    }

    Handle = ProcessHandleAdd(Process, HandleShm, Region, 1);
    if (Handle < 0) {
        ShmRelease(Region);
        return SYSCALL_ERROR;
    }
    return Handle;
}

static int SyscallShmMap(int HandleIndex)
{
    Process_t *Process = ProcessGetCurrent();
    Handle_t *Entry;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    Entry = ProcessHandleGet(Process, HandleIndex, HandleShm);
    if (Entry == NULL) {
        return SYSCALL_BADHANDLE;
    }
    return (int)ShmMap((SharedMemory_t*)Entry->Object, Process);
}

static int SyscallShmSize(int HandleIndex)
{
    Process_t *Process = ProcessGetCurrent();
    Handle_t *Entry;

    if (Process == NULL) {
        return SYSCALL_DENIED;
    }
    Entry = ProcessHandleGet(Process, HandleIndex, HandleShm);
    if (Entry == NULL) {
        return SYSCALL_BADHANDLE;
    }
    return (int)((SharedMemory_t*)Entry->Object)->Length;
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

        case SYS_SET_REPLY:
            Result = SyscallSetReply((int)Registers->Ebx);
            break;

        case SYS_OPEN_REPLY:
            Result = SyscallOpenReply((UUId_t)Registers->Ebx);
            break;

        case SYS_IRQ_REGISTER:
            Result = SyscallIrqRegister((int)Registers->Ebx);
            break;

        case SYS_IRQ_WAIT:
            Result = SyscallIrqWait((int)Registers->Ebx);
            break;

        case SYS_IRQ_ACK:
            Result = SyscallIrqAck((int)Registers->Ebx);
            break;

        case SYS_IO_REQUEST:
            Result = SyscallIoRequest((unsigned)Registers->Ebx,
                (unsigned)Registers->Ecx);
            break;

        case SYS_IO_MAP:
            Result = SyscallIoMap((unsigned)Registers->Ebx,
                (unsigned)Registers->Ecx);
            break;

        case SYS_SPAWN:
            Result = SyscallSpawn((const char*)Registers->Ebx);
            break;

        case SYS_PROCESS_ALIVE:
            Result = SyscallProcessAlive((UUId_t)Registers->Ebx);
            break;



        case SYS_SET_CONSOLE:
            Result = SyscallSetConsoleInput((int)Registers->Ebx);
            break;

        case SYS_SET_REGISTRY:
            Result = SyscallSetRegistry((int)Registers->Ebx);
            break;

        case SYS_GETPPID: {
            Process_t *Self = ProcessGetCurrent();
            Result = (Self != NULL) ? (int)Self->ParentId : SYSCALL_ERROR;
            break;
        }

        case SYS_ENDPOINT_CREATE:
            Result = SyscallEndpointCreate();
            break;

        case SYS_CALL:
            Result = SyscallCall((const SysCallArgs_t*)Registers->Ebx);
            break;

        case SYS_RECV:
            Result = SyscallRecv((SysRecvArgs_t*)Registers->Ebx);
            break;

        case SYS_REPLY:
            Result = SyscallReply((const SysReplyArgs_t*)Registers->Ebx);
            break;

        case SYS_CAP_GRANT:
            Result = SyscallCapGrant((const SysGrantArgs_t*)Registers->Ebx);
            break;

        case SYS_SHM_CREATE:
            Result = SyscallShmCreate((unsigned)Registers->Ebx);
            break;

        case SYS_SHM_MAP:
            Result = SyscallShmMap((int)Registers->Ebx);
            break;

        case SYS_SHM_SIZE:
            Result = SyscallShmSize((int)Registers->Ebx);
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

/* SyscallsFindNamedPipe
 * There is no kernel name table any more, so this can no longer find
 * anything. Kept as a stub rather than deleted so the one caller - the
 * keyboard handover - fails visibly instead of silently compiling
 * against something that moved. */
void *SyscallsFindNamedPipe(const char *Name)
{
    (void)Name;
    return NULL;
}

/* SyscallsPrintNames */
void SyscallsPrintNames(void)
{
    LogInformation("Syscall", "naming lives in the registry server");
}
