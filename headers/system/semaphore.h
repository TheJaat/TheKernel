#ifndef __SEMAPHORE_H__
#define __SEMAPHORE_H__

#include <defs.h>
#include <stddef.h>
#include <system/spinlock.h>

/* A counting semaphore. Value may go negative - when it does, its
 * magnitude is the number of threads queued on it. */
typedef struct _Semaphore {
    Spinlock_t      Lock;
    int             Value;
    UUId_t          Creator;
} Semaphore_t;

#ifdef __cplusplus
extern "C" {
#endif

/* SemaphoreConstruct
 * Initializes a semaphore the caller already has storage for. */
void SemaphoreConstruct(Semaphore_t *Semaphore, int InitialValue);

/* SemaphoreCreate
 * Allocates and initializes one. NULL on failure. */
Semaphore_t *SemaphoreCreate(int InitialValue);

/* SemaphoreDestroy
 * Wakes anything still waiting, then frees. */
void SemaphoreDestroy(Semaphore_t *Semaphore);

/* SemaphoreP
 * Wait / down. Blocks until signalled or until <Timeout> ms elapse; a
 * timeout of 0 waits forever. Returns Success if it acquired, Error on
 * timeout.
 *
 * Must not be called from interrupt context. */
OsStatus_t SemaphoreP(Semaphore_t *Semaphore, size_t Timeout);

/* SemaphoreV
 * Signal / up by <Value>. Safe from interrupt context - it never blocks
 * and never allocates, which is what lets a timer tick use it. */
void SemaphoreV(Semaphore_t *Semaphore, int Value);

#ifdef __cplusplus
}
#endif

#endif /* __SEMAPHORE_H__ */