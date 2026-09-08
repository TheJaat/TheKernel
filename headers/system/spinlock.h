#ifndef __SPINLOCK_H__
#define __SPINLOCK_H__

#include <defs.h>

/* A spinlock is just a word. 0 is unlocked. */
typedef struct _Spinlock {
    volatile int    Value;
    int             SavedState;     /* used by the Irq variants */
} Spinlock_t;

#define SPINLOCK_INIT               { 0, 0 }

#ifdef __cplusplus
extern "C" {
#endif

/* SpinlockReset
 * Puts a lock into the unlocked state. */
void SpinlockReset(Spinlock_t *Lock);

/* SpinlockAcquire
 * Busy-waits until the lock is free and takes it.
 *
 * On a single cpu this is only safe between threads. If an interrupt
 * handler tries to take a lock the interrupted thread already holds,
 * the handler spins forever waiting for a thread that cannot run.
 * Use the Irq variants for anything an interrupt handler touches. */
void SpinlockAcquire(Spinlock_t *Lock);

/* SpinlockTryAcquire
 * Returns Success if the lock was taken, Error if it was busy. */
OsStatus_t SpinlockTryAcquire(Spinlock_t *Lock);

/* SpinlockRelease */
void SpinlockRelease(Spinlock_t *Lock);

/* SpinlockAcquireIrq / SpinlockReleaseIrq
 * Disables interrupts before taking the lock and restores the previous
 * state on release. This is the pair to use for any lock that is also
 * taken from interrupt context. */
void SpinlockAcquireIrq(Spinlock_t *Lock);
void SpinlockReleaseIrq(Spinlock_t *Lock);

#ifdef __cplusplus
}
#endif

#endif /* __SPINLOCK_H__ */