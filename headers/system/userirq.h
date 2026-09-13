#ifndef __USERIRQ_H__
#define __USERIRQ_H__

#include <defs.h>
#include <stddef.h>
#include <system/semaphore.h>

/* Interrupt forwarding.
 *
 * An interrupt arrives in ring 0 and must be acknowledged within
 * microseconds. The driver that should handle it is a process that may
 * not be scheduled for milliseconds. Those two facts conflict, and how
 * the conflict is resolved is the whole design:
 *
 *   1. the kernel stub MASKS the line
 *   2. it signals the owning driver
 *   3. InterruptEntry sends the EOI
 *   4. the driver eventually runs, talks to the device, and acks
 *   5. the ack unmasks the line
 *
 * Masking first is the load-bearing step. The device is still asserting
 * - nothing has talked to it yet - so without the mask the line re-fires
 * the instant the EOI goes out, and keeps firing for the entire time the
 * driver waits to be scheduled. The machine makes no forward progress
 * and it looks like a hang, not a driver problem. */

#define USERIRQ_MAX                 8

typedef struct _UserInterrupt {
    int             Line;
    UUId_t          Id;             /* from InterruptRegister */
    void           *Owner;          /* Process_t *            */
    Semaphore_t     Signal;
    volatile size_t Fired;
    volatile int    Masked;
    int             Used;
} UserInterrupt_t;

#ifdef __cplusplus
extern "C" {
#endif

void             UserIrqInitialize(void);

/* UserIrqRegister
 * Claims <Line> for <Process>. The caller must already have been
 * checked for hardware privileges. Returns NULL on failure. */
UserInterrupt_t *UserIrqRegister(void *Process, int Line);

/* UserIrqWait
 * Blocks until the line fires. Returns the number of times it has fired
 * in total, or a negative value. */
int              UserIrqWait(UserInterrupt_t *Entry);

/* UserIrqAcknowledge
 * Re-enables the line. Until this is called the device is silent - which
 * is deliberate, and is what stops an interrupt storm. */
OsStatus_t       UserIrqAcknowledge(UserInterrupt_t *Entry);

/* UserIrqUnregister
 * Releases the line. Called when the handle is closed or the process
 * exits, so a driver that crashes does not leave its line masked
 * forever. */
OsStatus_t       UserIrqUnregister(UserInterrupt_t *Entry);

void             UserIrqPrint(void);

#ifdef __cplusplus
}
#endif

#endif /* __USERIRQ_H__ */
