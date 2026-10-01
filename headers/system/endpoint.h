#ifndef __ENDPOINT_H__
#define __ENDPOINT_H__

#include <defs.h>
#include <stddef.h>
#include <system/spinlock.h>
#include <system/semaphore.h>
#include <os/syscalls.h>

/* Synchronous call/reply.
 *
 * The sender blocks until a receiver is ready and the kernel copies the
 * message straight into the receiver's buffer. Nothing is stored, so
 * there is no ring, no sizing policy, no atomicity flag and no framing -
 * a message has a length because the call has a length.
 *
 * The reply is part of the call, which is why none of the reply-channel
 * machinery pipes needed exists here. And the kernel knows who called,
 * so the caller's identity is a fact rather than a field the caller
 * fills in. */

#define ENDPOINT_MAX            16
#define ENDPOINT_QUEUE_MAX      8

typedef struct _Endpoint {
    UUId_t          Id;
    void           *Owner;          /* Process_t * that created it   */

    /* Senders blocked in SYS_CALL, in arrival order. */
    void           *Senders[ENDPOINT_QUEUE_MAX];   /* Thread_t *     */
    int             SenderCount;

    /* The receiver blocked in SYS_RECV, if any. */
    void           *Receiver;       /* Thread_t *                    */

    Semaphore_t     ReceiverWait;
    Spinlock_t      Lock;
    int             Used;
} Endpoint_t;

#ifdef __cplusplus
extern "C" {
#endif

void        EndpointInitialize(void);
Endpoint_t *EndpointCreate(void *Process);
void        EndpointDestroy(Endpoint_t *Endpoint);

/* EndpointCall / EndpointReceive / EndpointReply
 * All three block. Return the number of payload bytes, or negative. */
int         EndpointCall(Endpoint_t *, unsigned Badge, unsigned Opcode,
                         const void *Send, size_t SendLength,
                         void *Recv, size_t RecvLength, size_t TimeoutMs);
int         EndpointReceive(Endpoint_t *, void *Buffer, size_t Length,
                            unsigned *Opcode, unsigned *Badge,
                            size_t TimeoutMs);
int         EndpointReply(const void *Buffer, size_t Length);

void        EndpointPrint(void);

#ifdef __cplusplus
}
#endif

#endif /* __ENDPOINT_H__ */
