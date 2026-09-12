/* Includes
 * - System */
#include <system/threading.h>
#include <system/scheduler.h>
#include <system/heap.h>
#include <system/garbagecollector.h>
#include <system/log.h>
#include <interrupts/interrupts.h>
#include <arch/x86/x32/arch_x32.h>
#include <arch/x86/x32/gdt.h>
#include <arch/x86/memory.h>
#include <arch/x86/address_space.h>
#include <system/process.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

/* Every kernel thread gets one page of stack. Generous for what these
 * do now; revisit if you start recursing. */
#define THREADING_STACK_SIZE        0x1000

/* Globals */
static Thread_t GlbThreads[THREADING_MAX_THREADS];
static Thread_t *GlbCurrentThread = NULL;
static Thread_t *GlbIdleThread = NULL;
static UUId_t GlbThreadIds = 0;
static int GlbThreadingEnabled = 0;

/* Set once the collector is available. Until then the idle thread keeps
 * doing the reaping, because a thread can exit before the gc exists. */
static UUId_t GlbThreadGcHandler = UUID_INVALID;

/* ThreadingIsEnabled */
int ThreadingIsEnabled(void)
{
    return GlbThreadingEnabled;
}

/* ThreadingGetCurrentThread */
Thread_t *ThreadingGetCurrentThread(UUId_t Cpu)
{
    (void)Cpu;
    return GlbCurrentThread;
}

/* ThreadingGetCurrentThreadId */
UUId_t ThreadingGetCurrentThreadId(void)
{
    if (GlbCurrentThread == NULL) {
        return UUID_INVALID;
    }
    return GlbCurrentThread->Id;
}

/* ThreadingGetThread */
Thread_t *ThreadingGetThread(UUId_t ThreadId)
{
    int i;

    for (i = 0; i < THREADING_MAX_THREADS; i++) {
        if (GlbThreads[i].State != ThreadStateFree
            && GlbThreads[i].Id == ThreadId) {
            return &GlbThreads[i];
        }
    }
    return NULL;
}

/* ThreadingAllocateSlot
 * Caller must hold the lock. */
static Thread_t *ThreadingAllocateSlot(void)
{
    int i;

    for (i = 0; i < THREADING_MAX_THREADS; i++) {
        if (GlbThreads[i].State == ThreadStateFree) {
            return &GlbThreads[i];
        }
    }
    return NULL;
}

/* ThreadingSetName */
static void ThreadingSetName(Thread_t *Thread, const char *Name)
{
    int i;

    for (i = 0; i < (THREADING_NAME_LENGTH - 1) && Name != NULL
         && Name[i] != '\0'; i++) {
        Thread->Name[i] = Name[i];
    }
    for (; i < THREADING_NAME_LENGTH; i++) {
        Thread->Name[i] = '\0';
    }
}

/* ThreadingEntryPoint
 * Every thread starts here rather than at its function directly. A
 * kernel thread whose function simply returns would otherwise pop a
 * garbage return address off a stack that has nothing on it. This gives
 * it somewhere to land. */
static void ThreadingEntryPoint(void)
{
    Thread_t *Thread = GlbCurrentThread;

    if (Thread != NULL && Thread->Function != NULL) {
        Thread->Function(Thread->Args);
    }

    ThreadingExit();
}

/* ThreadingIdle
 * Runs when nothing else is runnable. hlt parks the cpu until the next
 * interrupt instead of spinning, which matters a lot under emulation. */
static void ThreadingIdle(void *Args)
{
    (void)Args;

    for (;;) {
        /* Once the gc is running it owns this; polling here as well
         * would race it for the same stacks. */
        if (GlbThreadGcHandler == UUID_INVALID) {
            ThreadingReapZombies();
        }
        __asm__ volatile ("hlt");
    }
}

/* ThreadingCreateThread */
UUId_t ThreadingCreateThread(const char *Name, ThreadEntry_t Function,
    void *Args, Flags_t Flags)
{
    Thread_t *Thread = NULL;
    uintptr_t Stack;
    UUId_t Id;
    int State;

    if (Function == NULL) {
        return UUID_INVALID;
    }

    /* Allocate the stack outside the critical section - kmalloc takes
     * the heap lock itself, and holding two locks in a fixed order is a
     * habit worth keeping even before it can deadlock. */
    Stack = (uintptr_t)kmalloc_a(THREADING_STACK_SIZE);
    if (Stack == 0) {
        LogFatal("Threading", "no memory for a thread stack");
        return UUID_INVALID;
    }
    memset((void*)Stack, 0, THREADING_STACK_SIZE);

    State = InterruptDisable();

    Thread = ThreadingAllocateSlot();
    if (Thread == NULL) {
        InterruptRestoreState(State);
        kfree((void*)Stack);
        LogFatal("Threading", "thread table is full");
        return UUID_INVALID;
    }

    memset(Thread, 0, sizeof(Thread_t));
    Id = GlbThreadIds++;

    Thread->Id            = Id;
    Thread->Flags         = Flags;
    Thread->Function      = Function;
    Thread->Args          = Args;
    Thread->StackBase     = Stack;
    Thread->StackSize     = THREADING_STACK_SIZE;
    Thread->TimeSliceLeft = THREADING_TIMESLICE;
    Thread->SleepMsLeft   = 0;
    ThreadingSetName(Thread, Name);

    Thread->Context = ContextCreate(Flags,
        (uintptr_t)&ThreadingEntryPoint, Stack + THREADING_STACK_SIZE);
    if (Thread->Context == NULL) {
        Thread->State = ThreadStateFree;
        InterruptRestoreState(State);
        kfree((void*)Stack);
        return UUID_INVALID;
    }

    Thread->State = ThreadStateReady;

    InterruptRestoreState(State);

    LogInformation("Threading", "created '%s' id %u, stack 0x%x",
        Thread->Name, Thread->Id, Stack);
    return Id;
}

/* ThreadingCreateUserThread
 * A thread that starts in ring 3. The caller has already mapped the code
 * PAGE_USER; this adds the user stack and marks that too. */
UUId_t ThreadingCreateUserThread(const char *Name, uintptr_t Entry,
    Flags_t Flags)
{
    Thread_t *Thread = NULL;
    uintptr_t Stack, UserStack;
    UUId_t Id;
    size_t i;
    int State;

    if (Entry == 0) {
        return UUID_INVALID;
    }

    Stack = (uintptr_t)kmalloc_a(THREADING_STACK_SIZE);
    if (Stack == 0) {
        LogFatal("Threading", "no memory for a kernel stack");
        return UUID_INVALID;
    }
    UserStack = (uintptr_t)kmalloc_a(THREADING_USER_STACK_SIZE);
    if (UserStack == 0) {
        kfree((void*)Stack);
        LogFatal("Threading", "no memory for a user stack");
        return UUID_INVALID;
    }
    memset((void*)Stack, 0, THREADING_STACK_SIZE);
    memset((void*)UserStack, 0, THREADING_USER_STACK_SIZE);

    /* The user stack has to be reachable from ring 3, or the very first
     * push after iret faults. The kernel stack deliberately is not. */
    for (i = 0; i < THREADING_USER_STACK_SIZE; i += PAGE_SIZE) {
        MmVirtualSetPageFlags(NULL, UserStack + i, PAGE_USER);
    }

    State = InterruptDisable();

    Thread = ThreadingAllocateSlot();
    if (Thread == NULL) {
        InterruptRestoreState(State);
        kfree((void*)Stack);
        kfree((void*)UserStack);
        LogFatal("Threading", "thread table is full");
        return UUID_INVALID;
    }

    memset(Thread, 0, sizeof(Thread_t));
    Id = GlbThreadIds++;

    Thread->Id            = Id;
    Thread->Flags         = Flags | THREADING_USERMODE;
    Thread->Function      = NULL;
    Thread->Args          = NULL;
    Thread->StackBase     = Stack;
    Thread->StackSize     = THREADING_STACK_SIZE;
    Thread->UserStackBase = UserStack;
    Thread->UserStackSize = THREADING_USER_STACK_SIZE;
    Thread->TimeSliceLeft = THREADING_TIMESLICE;
    ThreadingSetName(Thread, Name);

    /* No trampoline for user threads: the entry point is user code and
     * cannot be called from here. It exits with the exit syscall, and if
     * it simply returns it lands on a zeroed user stack and faults -
     * which is a user bug, reported as a fault, not a kernel one. */
    Thread->Context = ContextCreateUser(Thread->Flags, Entry,
        Stack + THREADING_STACK_SIZE,
        UserStack + THREADING_USER_STACK_SIZE);

    if (Thread->Context == NULL) {
        Thread->State = ThreadStateFree;
        InterruptRestoreState(State);
        kfree((void*)Stack);
        kfree((void*)UserStack);
        return UUID_INVALID;
    }

    Thread->State = ThreadStateReady;
    InterruptRestoreState(State);

    LogInformation("Threading", "created user thread '%s' id %u, entry 0x%x",
        Thread->Name, Thread->Id, Entry);
    return Id;
}

/* ThreadingCreateUserThreadInSpace
 * A ring-3 thread in its own address space. The image and stack are
 * already mapped there by the caller. */
UUId_t ThreadingCreateUserThreadInSpace(const char *Name, uintptr_t Entry,
    AddressSpace_t *Space, uintptr_t UserStackTop, Flags_t Flags)
{
    Thread_t *Thread = NULL;
    uintptr_t Stack;
    UUId_t Id;
    int State;

    if (Entry == 0 || Space == NULL || UserStackTop == 0) {
        return UUID_INVALID;
    }

    /* The kernel stack stays in the kernel heap: it must be reachable
     * from every address space, because an interrupt taken in this
     * thread lands on it before CR3 has changed. */
    Stack = (uintptr_t)kmalloc_a(THREADING_STACK_SIZE);
    if (Stack == 0) {
        LogFatal("Threading", "no memory for a kernel stack");
        return UUID_INVALID;
    }
    memset((void*)Stack, 0, THREADING_STACK_SIZE);

    State = InterruptDisable();

    Thread = ThreadingAllocateSlot();
    if (Thread == NULL) {
        InterruptRestoreState(State);
        kfree((void*)Stack);
        LogFatal("Threading", "thread table is full");
        return UUID_INVALID;
    }

    memset(Thread, 0, sizeof(Thread_t));
    Id = GlbThreadIds++;

    Thread->Id            = Id;
    Thread->Flags         = Flags | THREADING_USERMODE;
    Thread->StackBase     = Stack;
    Thread->StackSize     = THREADING_STACK_SIZE;
    Thread->UserStackBase = 0;      /* lives in the process space */
    Thread->UserStackSize = 0;
    Thread->AddressSpace  = Space;
    Thread->TimeSliceLeft = THREADING_TIMESLICE;
    ThreadingSetName(Thread, Name);

    Thread->Context = ContextCreateUser(Thread->Flags, Entry,
        Stack + THREADING_STACK_SIZE, UserStackTop);

    if (Thread->Context == NULL) {
        Thread->State = ThreadStateFree;
        InterruptRestoreState(State);
        kfree((void*)Stack);
        return UUID_INVALID;
    }

    Thread->State = ThreadStateReady;
    InterruptRestoreState(State);

    LogInformation("Threading", "created '%s' id %u in its own space, entry 0x%x",
        Thread->Name, Thread->Id, Entry);
    return Id;
}

/* ThreadingCreateProcessThread */
UUId_t ThreadingCreateProcessThread(const char *Name, uintptr_t Entry,
    void *ProcessPtr, Flags_t Flags)
{
    Process_t *Process = (Process_t*)ProcessPtr;
    uintptr_t UserStackTop;
    Thread_t *Thread;
    UUId_t Id;

    if (Process == NULL || Entry == 0) {
        return UUID_INVALID;
    }

    UserStackTop = ProcessAllocateUserStack(Process);
    if (UserStackTop == 0) {
        return UUID_INVALID;
    }

    Id = ThreadingCreateUserThreadInSpace(Name, Entry,
        Process->AddressSpace, UserStackTop, Flags);
    if (Id == UUID_INVALID) {
        return UUID_INVALID;
    }

    Thread = ThreadingGetThread(Id);
    if (Thread != NULL) {
        Thread->Process = Process;
    }
    ProcessAddThread(Process);
    return Id;
}

/* ThreadingExit */
void ThreadingExit(void)
{
    int State = InterruptDisable();

    if (GlbCurrentThread != NULL) {
        GlbCurrentThread->State = ThreadStateZombie;
    }

    InterruptRestoreState(State);

    /* Hand the corpse to the collector. If the gc is not up yet this
     * fails harmlessly and the idle thread picks it up instead. */
    if (GlbThreadGcHandler != UUID_INVALID) {
        GcSignal(GlbThreadGcHandler, (void*)GlbCurrentThread);
    }

    /* Give the cpu up. The scheduler will not pick a zombie again, and
     * the idle thread frees the stack later - this thread is still
     * standing on it. */
    ThreadingYield();

    /* Not reached. If it ever is, something picked a zombie. */
    for (;;) {
        __asm__ volatile ("hlt");
    }
}

/* ThreadingGcReap
 * Runs on the gc thread. Frees the stack of one exited thread and
 * releases its slot. */
static OsStatus_t ThreadingGcReap(void *Data)
{
    Thread_t *Thread = (Thread_t*)Data;
    uintptr_t Stack = 0;
    uintptr_t UserStack = 0;
    AddressSpace_t *Space = NULL;
    void *Process = NULL;
    int State;

    if (Thread == NULL) {
        return Error;
    }

    State = InterruptDisable();
    if (Thread->State == ThreadStateZombie && Thread != GlbCurrentThread) {
        Stack = Thread->StackBase;
        UserStack = Thread->UserStackBase;
        Space = Thread->AddressSpace;
        Process = Thread->Process;
        Thread->StackBase = 0;
        Thread->UserStackBase = 0;
        Thread->AddressSpace = NULL;
        Thread->Process = NULL;
        Thread->Context = NULL;
        Thread->State = ThreadStateFree;
    }
    InterruptRestoreState(State);

    /* Tearing down the address space is safe here because this runs on
     * the gc thread, which has no address space of its own and therefore
     * already switched CR3 back to the kernel's when it was scheduled.
     * Doing it from the dying thread would be pulling the directory out
     * from under the cpu that is executing. */
    /* A thread in a process does not destroy the space itself: it tells
     * the process it is gone, and the last one out tears it down. */
    if (Process != NULL) {
        ProcessRemoveThread((Process_t*)Process);
    }
    else if (Space != NULL) {
        AddressSpaceDestroy(Space);
    }

    /* Free outside the critical section - kfree takes the heap lock. */
    if (Stack != 0) {
        kfree((void*)Stack);
    }
    if (UserStack != 0) {
        kfree((void*)UserStack);
    }
    return Success;
}

/* ThreadingEnableGc
 * Switches zombie collection over to the garbage collector. Called once
 * GcInitialize has run. */
OsStatus_t ThreadingEnableGc(void)
{
    UUId_t Handler = GcRegister(ThreadingGcReap);

    if (Handler == UUID_INVALID) {
        return Error;
    }

    GlbThreadGcHandler = Handler;
    LogInformation("Threading", "zombie reaping moved to the gc");
    return Success;
}

/* ThreadingReapZombies */
void ThreadingReapZombies(void)
{
    int i;

    for (i = 0; i < THREADING_MAX_THREADS; i++) {
        uintptr_t Stack = 0;
        int State = InterruptDisable();

        if (GlbThreads[i].State == ThreadStateZombie
            && &GlbThreads[i] != GlbCurrentThread) {
            Stack = GlbThreads[i].StackBase;
            GlbThreads[i].StackBase = 0;
            GlbThreads[i].Context = NULL;
            GlbThreads[i].State = ThreadStateFree;
        }

        InterruptRestoreState(State);

        /* Free outside the critical section. */
        if (Stack != 0) {
            kfree((void*)Stack);
        }
    }
}

/* ThreadingYield */
void ThreadingYield(void)
{
    if (GlbThreadingEnabled == 0) {
        return;
    }
    __asm__ volatile ("int %0" :: "i"(THREADING_YIELD_VECTOR));
}

/* ThreadingYieldHandler
 * The software interrupt itself does nothing - the reschedule happens
 * in InterruptEntry, which has the register frame. This just stops the
 * vector being reported as unhandled. */
static InterruptStatus_t ThreadingYieldHandler(void *Data)
{
    (void)Data;
    return InterruptHandled;
}

/* ThreadingInitialize */
OsStatus_t ThreadingInitialize(UUId_t Cpu)
{
    Interrupt_t Yield;
    Thread_t *Boot = NULL;
    UUId_t IdleId;
    int i;

    LogInformation("Threading", "Initializing");

    memset(GlbThreads, 0, sizeof(GlbThreads));
    GlbCurrentThread = NULL;
    GlbIdleThread = NULL;
    GlbThreadIds = 0;

    /* Register the yield vector before anything can call yield. */
    memset(&Yield, 0, sizeof(Interrupt_t));
    for (i = 0; i < INTERRUPT_MAXVECTORS; i++) {
        Yield.Vectors[i] = INTERRUPT_NONE;
    }
    Yield.Vectors[0]  = THREADING_YIELD_VECTOR;
    Yield.Line        = INTERRUPT_NONE;
    Yield.Pin         = INTERRUPT_NONE;
    Yield.FastHandler = ThreadingYieldHandler;
    Yield.Data        = (void*)&GlbThreads[0];  /* non-NULL, unused */

    if (InterruptRegister(&Yield,
            INTERRUPT_KERNEL | INTERRUPT_SOFTWARE | INTERRUPT_FAST)
        == UUID_INVALID) {
        LogFatal("Threading", "could not register the yield vector");
        return Error;
    }

    /* The code running right now becomes a thread. It already has a
     * stack - the one kernel_entry set up at 0x9F000 - so there is
     * nothing to allocate, and no context to build: the first switch
     * away from it will save one. */
    Boot = &GlbThreads[0];
    memset(Boot, 0, sizeof(Thread_t));
    Boot->Id            = GlbThreadIds++;
    Boot->State         = ThreadStateRunning;
    Boot->Flags         = THREADING_KERNELMODE;
    Boot->Context       = NULL;
    Boot->StackBase     = 0;        /* not heap memory, never freed */
    Boot->TimeSliceLeft = THREADING_TIMESLICE;
    ThreadingSetName(Boot, "boot");

    GlbCurrentThread = Boot;
    GlbThreadingEnabled = 1;

    /* The idle thread must exist before the first tick, or a preemption
     * with nothing runnable would have nowhere to go. */
    IdleId = ThreadingCreateThread("idle", ThreadingIdle, NULL,
        THREADING_KERNELMODE | THREADING_IDLE);
    if (IdleId == UUID_INVALID) {
        GlbThreadingEnabled = 0;
        LogFatal("Threading", "could not create the idle thread");
        return Error;
    }
    GlbIdleThread = ThreadingGetThread(IdleId);

    LogInformation("Threading", "Ready, boot thread id %u, idle id %u",
        Boot->Id, IdleId);
    (void)Cpu;
    return Success;
}

/* ThreadingGetIdleThread
 * Used by the scheduler. */
Thread_t *ThreadingGetIdleThread(void)
{
    return GlbIdleThread;
}

/* ThreadingSetCurrentThread
 * Used by the switch path. */
void ThreadingSetCurrentThread(Thread_t *Thread)
{
    GlbCurrentThread = Thread;
}

/* ThreadingGetTable
 * Used by the scheduler for its round-robin walk. */
Thread_t *ThreadingGetTable(void)
{
    return &GlbThreads[0];
}

/* _ThreadingSwitch
 * Called from InterruptEntry with the interrupted register frame. */
Context_t *_ThreadingSwitch(Context_t *Regs, int PreEmptive)
{
    Thread_t *Current = GlbCurrentThread;
    Thread_t *Next = NULL;

    if (GlbThreadingEnabled == 0 || Current == NULL || Regs == NULL) {
        return Regs;
    }

    /* Save where this thread was interrupted. Regs points into that
     * thread's own stack, so keeping the pointer is enough. */
    Current->Context = Regs;

    if (PreEmptive) {
        Current->TimeSliceLeft--;
        if (Current->TimeSliceLeft > 0
            && Current->State == ThreadStateRunning) {
            /* Still has time and is still runnable - leave it alone. */
            return Regs;
        }
    }

    Next = SchedulerGetNextTask(0, Current, PreEmptive);
    if (Next == NULL || Next == Current) {
        /* Nothing better to run. Re-arm the slice so a thread that was
         * alone does not get re-examined every single tick. */
        Current->TimeSliceLeft = THREADING_TIMESLICE;
        if (Current->State == ThreadStateRunning) {
            return Regs;
        }
        /* The current thread is no longer runnable and there is no
         * replacement - that should be impossible while idle exists. */
        LogFatal("Threading", "no runnable thread");
        return Regs;
    }

    /* Retire the outgoing thread. Blocked and zombie states are set by
     * whoever changed them and must not be clobbered. */
    if (Current->State == ThreadStateRunning) {
        Current->State = ThreadStateReady;
    }
    Current->TimeSliceLeft = THREADING_TIMESLICE;

    Next->State = ThreadStateRunning;
    Next->TimeSliceLeft = THREADING_TIMESLICE;
    GlbCurrentThread = Next;

    /* Point the TSS at this thread's kernel stack. When a ring-3 thread
     * takes an interrupt the cpu switches stacks using esp0, so a stale
     * value here means the next interrupt pushes its frame onto whatever
     * thread ran last - corrupting it, and usually not crashing until
     * much later. Set unconditionally: a kernel thread that later
     * spawns a user one must not inherit a stale pointer either. */
    if (Next->StackBase != 0) {
        TssUpdateStack(0, Next->StackBase + Next->StackSize);
    }

    /* Load the incoming thread's address space, if it differs.
     *
     * Only on a change: reloading CR3 flushes the whole TLB, and doing
     * that on every tick between two kernel threads would be pure loss.
     *
     * This is safe to do here, mid-switch, precisely because every
     * address space shares the kernel half - the code executing right
     * now is mapped identically in both, so the instruction after the
     * CR3 write is still there. */
    {
        AddressSpace_t *NextSpace = (Next->AddressSpace != NULL)
            ? Next->AddressSpace : AddressSpaceGetKernel();
        AddressSpace_t *CurrentSpace = (Current->AddressSpace != NULL)
            ? Current->AddressSpace : AddressSpaceGetKernel();

        if (NextSpace != CurrentSpace) {
            AddressSpaceSwitch(NextSpace);
        }
    }

    return Next->Context;
}

/* ThreadingPrint */
void ThreadingPrint(void)
{
    static const char *States[] = {
        "free", "ready", "running", "blocked", "zombie"
    };
    int i;

    for (i = 0; i < THREADING_MAX_THREADS; i++) {
        Thread_t *Thread = &GlbThreads[i];
        if (Thread->State == ThreadStateFree) {
            continue;
        }
        LogInformation("Threading", "  id %u  %s  %s  stack 0x%x",
            Thread->Id, Thread->Name, States[Thread->State],
            Thread->StackBase);
    }
}