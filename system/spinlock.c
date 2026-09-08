/* Includes
 * - System */
#include <system/spinlock.h>
#include <interrupts/interrupts.h>

/* SpinlockExchange
 * Atomically writes <Value> into *Target and returns what was there.
 *
 * xchg with a memory operand asserts the bus lock implicitly - no lock
 * prefix needed, and unlike a compare-exchange it cannot fail spuriously.
 * "memory" in the clobber list stops the compiler moving loads or stores
 * across the lock, which matters far more than the instruction itself. */
static inline int SpinlockExchange(volatile int *Target, int Value)
{
    int Result = Value;

    __asm__ volatile (
        "xchgl %0, %1"
        : "+r"(Result), "+m"(*Target)
        :
        : "memory");

    return Result;
}

/* SpinlockReset */
void SpinlockReset(Spinlock_t *Lock)
{
    if (Lock == NULL) {
        return;
    }
    Lock->Value = 0;
    Lock->SavedState = 0;
}

/* SpinlockTryAcquire */
OsStatus_t SpinlockTryAcquire(Spinlock_t *Lock)
{
    if (Lock == NULL) {
        return Error;
    }
    if (SpinlockExchange(&Lock->Value, 1) == 0) {
        return Success;
    }
    return Error;
}

/* SpinlockAcquire */
void SpinlockAcquire(Spinlock_t *Lock)
{
    if (Lock == NULL) {
        return;
    }

    for (;;) {
        if (SpinlockExchange(&Lock->Value, 1) == 0) {
            return;
        }

        /* Spin on a plain read rather than hammering xchg. A locked
         * read-modify-write on every iteration keeps the cache line
         * bouncing between cores; reading until it looks free and only
         * then retrying the exchange does not. `pause` tells the cpu
         * this is a spin loop. */
        while (Lock->Value != 0) {
            __asm__ volatile ("pause" ::: "memory");
        }
    }
}

/* SpinlockRelease */
void SpinlockRelease(Spinlock_t *Lock)
{
    if (Lock == NULL) {
        return;
    }
    /* Exchange rather than a plain store, for the barrier. */
    SpinlockExchange(&Lock->Value, 0);
}

/* SpinlockAcquireIrq */
void SpinlockAcquireIrq(Spinlock_t *Lock)
{
    int State = InterruptDisable();

    if (Lock == NULL) {
        InterruptRestoreState(State);
        return;
    }

    SpinlockAcquire(Lock);

    /* Store the state inside the lock, not on the caller's stack. It can
     * only be written once the lock is held, otherwise two acquirers
     * would race on this field. */
    Lock->SavedState = State;
}

/* SpinlockReleaseIrq */
void SpinlockReleaseIrq(Spinlock_t *Lock)
{
    int State;

    if (Lock == NULL) {
        return;
    }

    /* Read it out before releasing - after the release another cpu may
     * take the lock and overwrite the field. */
    State = Lock->SavedState;
    Lock->SavedState = 0;

    SpinlockRelease(Lock);
    InterruptRestoreState(State);
}