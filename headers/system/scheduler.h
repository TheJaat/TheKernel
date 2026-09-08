#ifndef __SCHEDULER_H__
#define __SCHEDULER_H__

#include <defs.h>
#include <stddef.h>
#include <system/threading.h>

#ifdef __cplusplus
extern "C" {
#endif

/* SchedulerInit
 * Prepares the run queue for the given cpu. */
void SchedulerInit(UUId_t Cpu);

/* SchedulerReadyThread
 * Marks a thread runnable. Safe from interrupt context. */
void SchedulerReadyThread(Thread_t *Thread);

/* SchedulerBlockThread
 * Marks the current thread blocked for <MilliSeconds>, or forever when
 * 0, and yields. Returns once woken. */
void SchedulerSleepThread(size_t MilliSeconds);

/* SchedulerWakeThread
 * Moves a blocked thread back to ready. */
void SchedulerWakeThread(UUId_t ThreadId);

/* SchedulerBlockOn
 * Blocks the current thread on <Object> until it is signalled, or until
 * <TimeoutMs> elapses. A timeout of 0 waits forever. Returns Success if
 * signalled, Error if it timed out.
 *
 * Must not be called from interrupt context - it yields. */
OsStatus_t SchedulerBlockOn(void *Object, size_t TimeoutMs);

/* SchedulerWakeOne / SchedulerWakeAll
 * Releases threads blocked on <Object>. Safe from interrupt context:
 * neither allocates nor yields. SchedulerWakeOne returns the number
 * woken, so a caller can tell whether anyone was waiting. */
int SchedulerWakeOne(void *Object);
int SchedulerWakeAll(void *Object);

/* SchedulerApplyMs
 * Advances sleep timers. Called from TimersTick, so it runs in
 * interrupt context and must not allocate. */
void SchedulerApplyMs(size_t MilliSeconds);

/* SchedulerGetNextTask
 * Round-robin pick starting after <Current>. Returns the idle thread
 * when nothing else is runnable, and never NULL once threading is up. */
Thread_t *SchedulerGetNextTask(UUId_t Cpu, Thread_t *Current, int PreEmptive);

#ifdef __cplusplus
}
#endif

#endif /* __SCHEDULER_H__ */