/* Includes
 * - System */
#include <system/scheduler.h>
#include <system/threading.h>
#include <system/log.h>
#include <interrupts/interrupts.h>

/* Includes
 * - Library */
#include <stddef.h>

/* From threading.c - the scheduler walks the same table rather than
 * keeping a second one, which would have to be kept in sync. */
__EXTERN Thread_t *ThreadingGetTable(void);
__EXTERN Thread_t *ThreadingGetIdleThread(void);

/* Where the round-robin walk resumes. Keeping this across calls is what
 * makes it fair - starting from index 0 every time would starve
 * everything after the first runnable thread. */
static int GlbScheduleCursor = 0;
static int GlbSchedulerInitialized = 0;

/* SchedulerInit */
void SchedulerInit(UUId_t Cpu)
{
    (void)Cpu;
    LogInformation("Scheduler", "Initializing");
    GlbScheduleCursor = 0;
    GlbSchedulerInitialized = 1;
    LogInformation("Scheduler", "Ready, round-robin, %d ms slice",
        THREADING_TIMESLICE);
}

/* SchedulerReadyThread */
void SchedulerReadyThread(Thread_t *Thread)
{
    int State;

    if (Thread == NULL) {
        return;
    }

    State = InterruptDisable();
    if (Thread->State == ThreadStateBlocked) {
        Thread->SleepMsLeft = 0;
        Thread->State = ThreadStateReady;
    }
    InterruptRestoreState(State);
}

/* SchedulerWakeThread */
void SchedulerWakeThread(UUId_t ThreadId)
{
    SchedulerReadyThread(ThreadingGetThread(ThreadId));
}

/* SchedulerSleepThread */
void SchedulerSleepThread(size_t MilliSeconds)
{
    Thread_t *Current;
    int State;

    if (ThreadingIsEnabled() == 0) {
        return;
    }

    State = InterruptDisable();
    Current = ThreadingGetCurrentThread(0);

    if (Current == NULL) {
        InterruptRestoreState(State);
        return;
    }

    /* The idle thread must never block - there would be nothing left to
     * run and the machine would wedge. */
    if (Current->Flags & THREADING_IDLE) {
        InterruptRestoreState(State);
        LogFatal("Scheduler", "the idle thread tried to sleep");
        return;
    }

    Current->SleepMsLeft = (long)MilliSeconds;
    Current->State = ThreadStateBlocked;

    InterruptRestoreState(State);

    /* Yield outside the critical section, so the switch happens with
     * interrupts in whatever state the caller had. */
    ThreadingYield();
}

/* SchedulerApplyMs
 * Runs from TimersTick, in interrupt context. Must not allocate and must
 * not yield. */
void SchedulerApplyMs(size_t MilliSeconds)
{
    Thread_t *Table;
    int i;

    if (GlbSchedulerInitialized == 0 || ThreadingIsEnabled() == 0) {
        return;
    }

    Table = ThreadingGetTable();

    for (i = 0; i < THREADING_MAX_THREADS; i++) {
        Thread_t *Thread = &Table[i];

        if (Thread->State != ThreadStateBlocked) {
            continue;
        }
        /* A sleep of 0 means blocked indefinitely - only an explicit
         * wake brings it back. */
        if (Thread->SleepMsLeft <= 0) {
            continue;
        }

        Thread->SleepMsLeft -= (long)MilliSeconds;
        if (Thread->SleepMsLeft <= 0) {
            Thread->SleepMsLeft = 0;
            Thread->State = ThreadStateReady;
        }
    }
}

/* SchedulerGetNextTask
 * Round-robin from just after the cursor. Returns the idle thread only
 * when nothing else is runnable, and never returns a blocked or zombie
 * thread. */
Thread_t *SchedulerGetNextTask(UUId_t Cpu, Thread_t *Current, int PreEmptive)
{
    Thread_t *Table = ThreadingGetTable();
    Thread_t *Idle = ThreadingGetIdleThread();
    int i;

    (void)Cpu;
    (void)PreEmptive;

    if (GlbSchedulerInitialized == 0) {
        return Current;
    }

    for (i = 1; i <= THREADING_MAX_THREADS; i++) {
        int Index = (GlbScheduleCursor + i) % THREADING_MAX_THREADS;
        Thread_t *Candidate = &Table[Index];

        /* Skip idle here - it is the fallback, not a peer. Letting it
         * into the rotation would give it an equal share of the cpu. */
        if (Candidate == Idle) {
            continue;
        }
        if (Candidate->State != ThreadStateReady
            && Candidate->State != ThreadStateRunning) {
            continue;
        }

        GlbScheduleCursor = Index;
        return Candidate;
    }

    /* Nothing ready. If the current thread can keep running, let it. */
    if (Current != NULL && Current->State == ThreadStateRunning
        && Current != Idle) {
        return Current;
    }

    return Idle;
}