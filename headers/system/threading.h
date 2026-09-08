#ifndef __THREADING_H__
#define __THREADING_H__

#include <defs.h>
#include <stddef.h>
#include <arch/x86/x32/context.h>

/* Fixed table, same reasoning as the timers and io-spaces: no list
 * implementation, and the scheduler runs in interrupt context where
 * allocating would race the heap. */
#define THREADING_MAX_THREADS       32
#define THREADING_NAME_LENGTH       16

/* How long a thread runs before it is preempted, in system ticks.
 * At 1000 Hz that is 20 ms. */
#define THREADING_TIMESLICE         20

/* The software interrupt vector used by ThreadingYield. Anything above
 * the ISA range and clear of the self-tests. */
#define THREADING_YIELD_VECTOR      0x81

/* Thread states. FREE means the slot is unused. */
typedef enum {
    ThreadStateFree = 0,
    ThreadStateReady,
    ThreadStateRunning,
    ThreadStateBlocked,
    ThreadStateZombie
} ThreadState_t;

/* Thread flags */
#define THREADING_KERNELMODE        0x00000000
#define THREADING_IDLE              0x00000001

typedef void (*ThreadEntry_t)(void*);

typedef struct _Thread {
    UUId_t          Id;
    char            Name[THREADING_NAME_LENGTH];
    ThreadState_t   State;
    Flags_t         Flags;

    /* Saved register frame. Points into this thread's own stack. */
    Context_t      *Context;

    /* Base of the kmalloc'd stack, kept so it can be freed. */
    uintptr_t       StackBase;
    size_t          StackSize;

    ThreadEntry_t   Function;
    void           *Args;

    long            SleepMsLeft;
    int             TimeSliceLeft;
} Thread_t;

#ifdef __cplusplus
extern "C" {
#endif

/* ThreadingInitialize
 * Turns the current execution context into the boot thread and creates
 * the idle thread. Must run after the heap, the interrupt manager and
 * the scheduler, and before interrupts are enabled. */
OsStatus_t ThreadingInitialize(UUId_t Cpu);

/* ThreadingCreateThread
 * Spawns a kernel thread. Returns UUID_INVALID on failure. */
UUId_t ThreadingCreateThread(const char *Name, ThreadEntry_t Function,
    void *Args, Flags_t Flags);

/* ThreadingExit
 * Ends the calling thread. Does not return. A thread function that just
 * returns lands here anyway via the trampoline. */
void ThreadingExit(void);

/* ThreadingYield
 * Gives up the rest of the current timeslice. */
void ThreadingYield(void);

/* ThreadingGetCurrentThread / ThreadingGetCurrentThreadId */
Thread_t *ThreadingGetCurrentThread(UUId_t Cpu);
UUId_t ThreadingGetCurrentThreadId(void);

/* ThreadingGetThread
 * Looks up a thread by id, or NULL. */
Thread_t *ThreadingGetThread(UUId_t ThreadId);

/* ThreadingIsEnabled
 * 1 once ThreadingInitialize has completed. The switch path checks this
 * so an early tick cannot reschedule into nothing. */
int ThreadingIsEnabled(void);

/* ThreadingReapZombies
 * Frees the stacks of exited threads. Called from the idle thread,
 * because a thread cannot free the stack it is standing on. */
void ThreadingReapZombies(void);

/* ThreadingPrint
 * Dumps the thread table through the logger. */
void ThreadingPrint(void);

/* ContextCreate
 * Builds an initial register frame at the top of a fresh kernel stack,
 * arranged so that iret enters <Eip>. Architecture specific. */
Context_t *ContextCreate(Flags_t ThreadFlags, uintptr_t Eip,
    uintptr_t StackTop);

/* _ThreadingSwitch
 * Called from InterruptEntry. Saves <Regs> into the current thread,
 * picks the next one and returns its context. Returns <Regs> unchanged
 * when there is nothing to switch to. */
Context_t *_ThreadingSwitch(Context_t *Regs, int PreEmptive);

/* enter_thread
 * Loads a saved context and iret's into it. Implemented in irq.asm.
 * Does not return. */
__EXTERN void enter_thread(Context_t *Context);

#ifdef __cplusplus
}
#endif

#endif /* __THREADING_H__ */