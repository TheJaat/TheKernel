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