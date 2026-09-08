/* Includes
 * - System */
#include <system/mutex.h>
#include <system/scheduler.h>
#include <system/threading.h>
#include <system/heap.h>
#include <system/log.h>
#include <interrupts/interrupts.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

/* MutexConstruct */
void MutexConstruct(Mutex_t *Mutex)
{
    if (Mutex == NULL) {
        return;
    }
    SpinlockReset(&Mutex->Lock);
    Mutex->Blocker = UUID_INVALID;
    Mutex->Blocks  = 0;
}

/* MutexCreate */
Mutex_t *MutexCreate(void)
{
    Mutex_t *Mutex = (Mutex_t*)kmalloc(sizeof(Mutex_t));

    if (Mutex == NULL) {
        LogFatal("Mutex", "out of memory");
        return NULL;
    }

    memset(Mutex, 0, sizeof(Mutex_t));
    MutexConstruct(Mutex);
    return Mutex;
}

/* MutexDestruct */
void MutexDestruct(Mutex_t *Mutex)
{
    if (Mutex == NULL) {
        return;
    }
    SchedulerWakeAll(Mutex);
    kfree(Mutex);
}

/* MutexTryLock */
OsStatus_t MutexTryLock(Mutex_t *Mutex)
{
    UUId_t Self;

    if (Mutex == NULL) {
        return Error;
    }

    Self = ThreadingGetCurrentThreadId();
    SpinlockAcquireIrq(&Mutex->Lock);

    if (Mutex->Blocks == 0) {
        Mutex->Blocker = Self;
        Mutex->Blocks  = 1;
        SpinlockReleaseIrq(&Mutex->Lock);
        return Success;
    }
    if (Mutex->Blocker == Self) {
        Mutex->Blocks++;
        SpinlockReleaseIrq(&Mutex->Lock);
        return Success;
    }

    SpinlockReleaseIrq(&Mutex->Lock);
    return Error;
}

/* MutexLock */
OsStatus_t MutexLock(Mutex_t *Mutex)
{
    UUId_t Self;

    if (Mutex == NULL) {
        return Error;
    }

    /* Before threading, there is nothing to contend with and nothing to
     * block on - take it and move on. */
    if (ThreadingIsEnabled() == 0) {
        Mutex->Blocker = UUID_INVALID;
        Mutex->Blocks++;
        return Success;
    }

    Self = ThreadingGetCurrentThreadId();

    for (;;) {
        SpinlockAcquireIrq(&Mutex->Lock);

        if (Mutex->Blocks == 0) {
            Mutex->Blocker = Self;
            Mutex->Blocks  = 1;
            SpinlockReleaseIrq(&Mutex->Lock);
            return Success;
        }
        if (Mutex->Blocker == Self) {
            /* Recursive. Without this, a thread taking a lock it already
             * holds would wait for itself. */
            Mutex->Blocks++;
            SpinlockReleaseIrq(&Mutex->Lock);
            return Success;
        }

        {
            int State = Mutex->Lock.SavedState;
            Mutex->Lock.SavedState = 0;
            SpinlockRelease(&Mutex->Lock);

            /* Wait indefinitely, then re-check. Re-checking rather than
             * assuming ownership on wake is what makes this correct if
             * two waiters are released at once. */
            SchedulerBlockOn(Mutex, 0);

            InterruptRestoreState(State);
        }
    }
}

/* MutexUnlock */
void MutexUnlock(Mutex_t *Mutex)
{
    if (Mutex == NULL) {
        return;
    }

    SpinlockAcquireIrq(&Mutex->Lock);

    if (Mutex->Blocks == 0) {
        SpinlockReleaseIrq(&Mutex->Lock);
        LogFatal("Mutex", "unlock of a mutex that is not held");
        return;
    }
    if (ThreadingIsEnabled() != 0
        && Mutex->Blocker != ThreadingGetCurrentThreadId()) {
        SpinlockReleaseIrq(&Mutex->Lock);
        LogFatal("Mutex", "unlock by thread %u, held by %u",
            ThreadingGetCurrentThreadId(), Mutex->Blocker);
        return;
    }

    Mutex->Blocks--;
    if (Mutex->Blocks == 0) {
        Mutex->Blocker = UUID_INVALID;
        SchedulerWakeOne(Mutex);
    }

    SpinlockReleaseIrq(&Mutex->Lock);
}