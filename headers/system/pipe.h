#ifndef __PIPE_H__
#define __PIPE_H__

#include <defs.h>
#include <stddef.h>
#include <stdint.h>
#include <system/spinlock.h>
#include <system/semaphore.h>

/* Behaviour flags. By default both ends block; these make either side
 * return short instead of waiting. */
#define PIPE_NOBLOCK_READ           0x1
#define PIPE_NOBLOCK_WRITE          0x2
#define PIPE_NOBLOCK                (PIPE_NOBLOCK_READ | PIPE_NOBLOCK_WRITE)

/* A byte ring buffer with a blocking reader and writer. */
typedef struct _Pipe {
    Flags_t         Flags;
    uint8_t        *Buffer;
    size_t          Length;
    size_t          IndexWrite;
    size_t          IndexRead;

    Spinlock_t      Lock;
    Semaphore_t     ReadQueue;      /* readers wait here for data  */
    Semaphore_t     WriteQueue;     /* writers wait here for room  */
    int             ReadWaiting;
    int             WriteWaiting;

    int             Owned;          /* buffer was allocated by PipeCreate */
} Pipe_t;

#ifdef __cplusplus
extern "C" {
#endif

/* PipeCreate / PipeConstruct / PipeDestroy */
Pipe_t *PipeCreate(size_t Size, Flags_t Flags);
void PipeConstruct(Pipe_t *Pipe, uint8_t *Buffer, size_t Length, Flags_t Flags);
void PipeDestroy(Pipe_t *Pipe);

/* PipeWrite
 * Writes <Length> bytes, blocking while the pipe is full unless
 * PIPE_NOBLOCK_WRITE. Returns bytes written.
 *
 * Safe from interrupt context ONLY with PIPE_NOBLOCK_WRITE - otherwise
 * it can block, and an interrupt handler that blocks never returns. */
size_t PipeWrite(Pipe_t *Pipe, const uint8_t *Data, size_t Length);

/* PipeRead
 * Reads up to <Length> bytes, blocking while empty unless
 * PIPE_NOBLOCK_READ. Returns bytes read. <Peek> leaves them in place. */
size_t PipeRead(Pipe_t *Pipe, uint8_t *Buffer, size_t Length, int Peek);

/* PipeBytesAvailable / PipeBytesFree */
size_t PipeBytesAvailable(Pipe_t *Pipe);
size_t PipeBytesFree(Pipe_t *Pipe);

#ifdef __cplusplus
}
#endif

#endif /* __PIPE_H__ */