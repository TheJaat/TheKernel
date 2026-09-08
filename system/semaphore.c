/* Includes
 * - System */
#include <system/semaphore.h>
#include <system/scheduler.h>
#include <system/threading.h>
#include <system/heap.h>
#include <system/log.h>
#include <interrupts/interrupts.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

/* SemaphoreConstruct */
void SemaphoreConstruct(Semaphore_t *Semaphore, int InitialValue)
{
    if (Semaphore == NULL) {
        return;
    }

    SpinlockReset(&Semaphore->Lock);
    Semaphore->Value   = InitialValue;
    Semaphore->Creator = ThreadingGetCurrentThreadId();
}

/* SemaphoreCreate */
Semaphore_t *SemaphoreCreate(int InitialValue)
{
    Semaphore_t *Semaphore = (Semaphore_t*)kmalloc(sizeof(Semaphore_t));

    if (Semaphore == NULL) {
        LogFatal("Semaphore", "out of memory");
        return NULL;
    }

    memset(Semaphore, 0, sizeof(Semaphore_t));
    SemaphoreConstruct(Semaphore, InitialValue);
    return Semaphore;
}

/* SemaphoreDestroy */
void SemaphoreDestroy(Semaphore_t *Semaphore)
{
    if (Semaphore == NULL) {
        return;
    }

    /* Anything still queued would block forever otherwise. They wake to
     * find the semaphore gone, which is a caller bug - but a hang is a
     * far worse way to report it. */
    SchedulerWakeAll(Semaphore);
    kfree(Semaphore);
}

/* SemaphoreP */
OsStatus_t SemaphoreP(Semaphore_t *Semaphore, size_t Timeout)
{
    OsStatus_t Result = Success;

    if (Semaphore == NULL) {
        return Error;
    }

    SpinlockAcquireIrq(&Semaphore->Lock);
    Semaphore->Value--;

    if (Semaphore->Value < 0) {
        /* Release before blocking. SchedulerBlockOn yields, and yielding
         * while holding a lock that the signaller needs is a deadlock.
         *
         * There is no lost-wakeup race here despite the gap: interrupts
         * are still disabled from SpinlockAcquireIrq until the block
         * completes, so nothing can run in between on this cpu. On SMP
         * this needs the state set before the release instead. */
        int State = Semaphore->Lock.SavedState;
        Semaphore->Lock.SavedState = 0;
        SpinlockRelease(&Semaphore->Lock);

        Result = SchedulerBlockOn(Semaphore, Timeout);

        InterruptRestoreState(State);

        if (Result != Success) {
            /* Timed out - undo the decrement, we never got the count. */
            SpinlockAcquireIrq(&Semaphore->Lock);
            Semaphore->Value++;
            SpinlockReleaseIrq(&Semaphore->Lock);
        }
        return Result;
    }

    SpinlockReleaseIrq(&Semaphore->Lock);
    return Success;
}

/* SemaphoreV */
void SemaphoreV(Semaphore_t *Semaphore, int Value)
{
    int i;

    if (Semaphore == NULL || Value <= 0) {
        return;
    }

    SpinlockAcquireIrq(&Semaphore->Lock);

    for (i = 0; i < Value; i++) {
        Semaphore->Value++;
        /* Only wake when the count was negative, i.e. somebody was
         * actually queued. Waking on a positive count would hand out a
         * permit nobody is waiting for. */
        if (Semaphore->Value <= 0) {
            SchedulerWakeOne(Semaphore);
        }
    }

    SpinlockReleaseIrq(&Semaphore->Lock);
}