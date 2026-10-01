/* Includes
 * - System */
#include <system/endpoint.h>
#include <system/process.h>
#include <system/threading.h>
#include <system/scheduler.h>
#include <system/log.h>
#include <interrupts/interrupts.h>
#include <arch/x86/memory.h>
#include <arch/x86/x32/arch_x32.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

static Endpoint_t GlbEndpoints[ENDPOINT_MAX];
static UUId_t GlbEndpointIds = 1;

/* A window in kernel space used to reach another address space's pages
 * for the duration of one copy.
 *
 * This is what makes a single copy possible. Without it the kernel can
 * only touch the currently loaded directory, so a message would have to
 * go user -> kernel buffer -> user: two copies, which is what a pipe
 * already does. Mapping the target's frames here lets the data move
 * straight from the sender's buffer to the receiver's. */
#define COPY_WINDOW_PAGES       2
static VirtualAddress_t GlbCopyWindow = 0;

/* EndpointInitialize */
void EndpointInitialize(void)
{
    memset(GlbEndpoints, 0, sizeof(GlbEndpoints));
    GlbEndpointIds = 1;

    GlbCopyWindow = MmReserveMemory(COPY_WINDOW_PAGES);
    if (GlbCopyWindow == 0) {
        LogFatal("Endpoint", "no virtual space for the copy window");
    }

    LogInformation("Endpoint", "Ready, %d endpoints, copy window at 0x%x",
        ENDPOINT_MAX, GlbCopyWindow);
}

/* EndpointCopyAcross
 * Copies <Length> bytes from <Source> in the current address space to
 * <Destination> in <Space>.
 *
 * Page by page: look up the destination's physical frame in its own
 * directory, map it into the copy window, copy the part of the message
 * that falls in that page, unmap.
 *
 * Interrupts are off throughout. The window is a single global
 * resource, and a preemption in the middle would let another call
 * remap it underneath this copy. */
static OsStatus_t EndpointCopyAcross(AddressSpace_t *Space,
    uintptr_t Destination, const void *Source, size_t Length)
{
    const uint8_t *In = (const uint8_t*)Source;
    size_t Done = 0;
    int State;

    if (Space == NULL || Destination == 0 || GlbCopyWindow == 0) {
        return Error;
    }

    State = InterruptDisable();

    while (Done < Length) {
        uintptr_t Page   = (Destination + Done) & PAGE_MASK;
        uintptr_t Offset = (Destination + Done) & ATTRIBUTE_MASK;
        size_t Chunk     = PAGE_SIZE - Offset;
        PhysicalAddress_t Frame;

        if (Chunk > (Length - Done)) {
            Chunk = Length - Done;
        }

        Frame = MmVirtualGetMapping(Space->PageDirectory, Page);
        if (Frame == 0) {
            InterruptRestoreState(State);
            return Error;           /* receiver's buffer is not mapped */
        }

        /* The destination must be a page the receiver could write
         * itself. Copying into a kernel page on its behalf would let a
         * receiver nominate an address it has no right to. */
        if (MmVirtualGetPageFlags(Space->PageDirectory, Page,
                PAGE_USER | PAGE_WRITE) != Success) {
            InterruptRestoreState(State);
            return Error;
        }

        if (MmVirtualMap(NULL, Frame, GlbCopyWindow, PAGE_WRITE) != Success) {
            InterruptRestoreState(State);
            return Error;
        }

        memcpy((void*)(GlbCopyWindow + Offset), In + Done, Chunk);
        MmVirtualUnmap(NULL, GlbCopyWindow, 0);

        Done += Chunk;
    }

    InterruptRestoreState(State);
    return Success;
}

/* EndpointCopyFrom
 * The mirror of EndpointCopyAcross: reads <Length> bytes from <Source>
 * in <Space> into <Destination> in the current space. */
static OsStatus_t EndpointCopyFrom(AddressSpace_t *Space,
    void *Destination, uintptr_t Source, size_t Length)
{
    uint8_t *Out = (uint8_t*)Destination;
    size_t Done = 0;
    int State;

    if (Space == NULL || Source == 0 || GlbCopyWindow == 0) {
        return Error;
    }

    State = InterruptDisable();

    while (Done < Length) {
        uintptr_t Page   = (Source + Done) & PAGE_MASK;
        uintptr_t Offset = (Source + Done) & ATTRIBUTE_MASK;
        size_t Chunk     = PAGE_SIZE - Offset;
        PhysicalAddress_t Frame;

        if (Chunk > (Length - Done)) {
            Chunk = Length - Done;
        }

        Frame = MmVirtualGetMapping(Space->PageDirectory, Page);
        if (Frame == 0
            || MmVirtualGetPageFlags(Space->PageDirectory, Page, PAGE_USER)
               != Success) {
            InterruptRestoreState(State);
            return Error;
        }
        if (MmVirtualMap(NULL, Frame, GlbCopyWindow, PAGE_WRITE) != Success) {
            InterruptRestoreState(State);
            return Error;
        }

        memcpy(Out + Done, (void*)(GlbCopyWindow + Offset), Chunk);
        MmVirtualUnmap(NULL, GlbCopyWindow, 0);

        Done += Chunk;
    }

    InterruptRestoreState(State);
    return Success;
}

/* EndpointCreate */
Endpoint_t *EndpointCreate(void *Process)
{
    Endpoint_t *Endpoint = NULL;
    int State;
    int i;

    State = InterruptDisable();

    for (i = 0; i < ENDPOINT_MAX; i++) {
        if (GlbEndpoints[i].Used == 0) {
            Endpoint = &GlbEndpoints[i];
            break;
        }
    }

    if (Endpoint == NULL) {
        InterruptRestoreState(State);
        LogFatal("Endpoint", "no free endpoints");
        return NULL;
    }

    memset(Endpoint, 0, sizeof(Endpoint_t));
    Endpoint->Id    = GlbEndpointIds++;
    Endpoint->Owner = Process;
    Endpoint->Used  = 1;
    SpinlockReset(&Endpoint->Lock);
    SemaphoreConstruct(&Endpoint->ReceiverWait, 0);

    InterruptRestoreState(State);
    return Endpoint;
}

/* EndpointDestroy */
void EndpointDestroy(Endpoint_t *Endpoint)
{
    int State;
    int i;

    if (Endpoint == NULL || Endpoint->Used == 0) {
        return;
    }

    State = InterruptDisable();

    /* Release every blocked sender with an error rather than leaving
     * them waiting on an endpoint that no longer exists. A client
     * calling a dead server must come back, not hang. */
    for (i = 0; i < Endpoint->SenderCount; i++) {
        Thread_t *Sender = (Thread_t*)Endpoint->Senders[i];
        if (Sender != NULL) {
            Sender->IpcResult = -1;
            SemaphoreV(&Sender->IpcWait, 1);
        }
    }
    Endpoint->SenderCount = 0;

    if (Endpoint->Receiver != NULL) {
        Thread_t *Receiver = (Thread_t*)Endpoint->Receiver;
        Receiver->IpcResult = -1;
        Endpoint->Receiver = NULL;
        SemaphoreV(&Receiver->IpcWait, 1);
    }

    Endpoint->Used = 0;
    InterruptRestoreState(State);
}

/* EndpointDeliver
 * Hands one queued sender's message to the waiting receiver. Caller
 * holds interrupts off.
 *
 * <SenderIsCurrent> says whose address space is loaded right now, and it
 * is what keeps this to a SINGLE copy. Delivery happens either from
 * EndpointCall (the sender is running) or from EndpointReceive (the
 * receiver is running); in each case one of the two buffers is directly
 * reachable and only the other needs the window. Staging through a
 * kernel buffer would work for both cases and cost two copies, which is
 * what a pipe already does. */
static int EndpointDeliver(Endpoint_t *Endpoint, Thread_t *Receiver,
                           int SenderIsCurrent)
{
    Thread_t *Sender;
    Process_t *SenderProcess, *RecvProcess;
    size_t Length;
    OsStatus_t Status;
    int i;

    if (Endpoint->SenderCount == 0) {
        return 0;
    }

    Sender = (Thread_t*)Endpoint->Senders[0];
    for (i = 1; i < Endpoint->SenderCount; i++) {
        Endpoint->Senders[i - 1] = Endpoint->Senders[i];
    }
    Endpoint->SenderCount--;

    SenderProcess = (Process_t*)Sender->Process;
    RecvProcess   = (Process_t*)Receiver->Process;

    if (SenderProcess == NULL || RecvProcess == NULL) {
        Sender->IpcResult = -1;
        SemaphoreV(&Sender->IpcWait, 1);
        return 0;
    }

    Length = Sender->IpcSendLength;
    if (Length > Receiver->IpcRecvLength) {
        Length = Receiver->IpcRecvLength;
    }

    if (Length == 0) {
        Status = Success;
    }
    else if (SenderIsCurrent) {
        /* The sender's buffer is directly readable; reach into the
         * receiver through the window. */
        Status = EndpointCopyAcross(RecvProcess->AddressSpace,
            (uintptr_t)Receiver->IpcRecvBuffer,
            Sender->IpcSendBuffer, Length);
    }
    else {
        /* The receiver's buffer is directly writable; reach into the
         * sender through the window. */
        Status = EndpointCopyFrom(SenderProcess->AddressSpace,
            Receiver->IpcRecvBuffer,
            (uintptr_t)Sender->IpcSendBuffer, Length);
    }

    if (Status != Success) {
        Sender->IpcResult = -1;
        SemaphoreV(&Sender->IpcWait, 1);
        return 0;
    }

    Receiver->IpcOpcode  = Sender->IpcOpcode;
    Receiver->IpcBadge   = Sender->IpcBadge;
    Receiver->IpcPartner = Sender;
    Receiver->IpcResult  = (int)Length;

    /* The sender stays blocked: it is waiting for the reply now, not
     * for delivery. */
    return 1;
}

/* EndpointCall */
int EndpointCall(Endpoint_t *Endpoint, unsigned Badge, unsigned Opcode,
                 const void *Send, size_t SendLength,
                 void *Recv, size_t RecvLength, size_t TimeoutMs)
{
    Thread_t *Self = ThreadingGetCurrentThread(0);
    Thread_t *Receiver = NULL;
    int State;
    int i;

    if (Endpoint == NULL || Endpoint->Used == 0 || Self == NULL) {
        return -1;
    }
    if (SendLength > IPC_MESSAGE_MAX || RecvLength > IPC_MESSAGE_MAX) {
        return -1;
    }

    State = InterruptDisable();

    if (Endpoint->SenderCount >= ENDPOINT_QUEUE_MAX) {
        InterruptRestoreState(State);
        return -1;
    }

    Self->IpcEndpoint   = Endpoint;
    Self->IpcSendBuffer = (void*)Send;
    Self->IpcSendLength = SendLength;
    Self->IpcRecvBuffer = Recv;
    Self->IpcRecvLength = RecvLength;
    Self->IpcOpcode     = Opcode;
    Self->IpcBadge      = Badge;
    Self->IpcResult     = 0;
    Self->IpcPartner    = NULL;
    SemaphoreConstruct(&Self->IpcWait, 0);

    Endpoint->Senders[Endpoint->SenderCount++] = Self;

    /* If a receiver is already waiting, hand it over now. */
    if (Endpoint->Receiver != NULL) {
        Receiver = (Thread_t*)Endpoint->Receiver;
        Endpoint->Receiver = NULL;
        if (EndpointDeliver(Endpoint, Receiver, 1)) {
            SemaphoreV(&Receiver->IpcWait, 1);
        }
    }

    InterruptRestoreState(State);

    /* Block until the reply arrives, the endpoint dies, or we give up. */
    if (SemaphoreP(&Self->IpcWait, TimeoutMs) != Success) {
        int Found = 0;

        /* Timed out. Two cases, and they must be distinguished under
         * the lock:
         *
         *   still queued  - nobody has taken the message, so withdraw
         *                   it and return. Leaving it would let a
         *                   receiver deliver into a buffer whose owner
         *                   has moved on.
         *
         *   already taken - a server holds us as its IpcPartner and
         *                   will reply into our buffer. We cannot
         *                   withdraw; the buffer must stay valid, so
         *                   wait without a deadline. Giving up here
         *                   would be a use-after-free in the server's
         *                   reply path.
         */
        State = InterruptDisable();

        for (i = 0; i < Endpoint->SenderCount; i++) {
            if (Endpoint->Senders[i] == Self) {
                int j;
                for (j = i + 1; j < Endpoint->SenderCount; j++) {
                    Endpoint->Senders[j - 1] = Endpoint->Senders[j];
                }
                Endpoint->SenderCount--;
                Found = 1;
                break;
            }
        }

        InterruptRestoreState(State);

        if (Found) {
            Self->IpcEndpoint = NULL;
            return -1;                  /* withdrawn cleanly */
        }

        /* In flight - see above. */
        SemaphoreP(&Self->IpcWait, 0);
    }

    Self->IpcEndpoint = NULL;
    return Self->IpcResult;
}

/* EndpointReceive */
int EndpointReceive(Endpoint_t *Endpoint, void *Buffer, size_t Length,
                    unsigned *Opcode, unsigned *Badge, size_t TimeoutMs)
{
    Thread_t *Self = ThreadingGetCurrentThread(0);
    int State;

    if (Endpoint == NULL || Endpoint->Used == 0 || Self == NULL) {
        return -1;
    }
    if (Length > IPC_MESSAGE_MAX) {
        Length = IPC_MESSAGE_MAX;
    }

    State = InterruptDisable();

    Self->IpcRecvBuffer = Buffer;
    Self->IpcRecvLength = Length;
    Self->IpcResult     = 0;
    Self->IpcPartner    = NULL;
    SemaphoreConstruct(&Self->IpcWait, 0);

    if (Endpoint->SenderCount > 0) {
        /* Somebody is already waiting - take it without blocking. */
        int Delivered = EndpointDeliver(Endpoint, Self, 0);
        InterruptRestoreState(State);
        if (!Delivered) {
            return -1;
        }
    }
    else {
        if (Endpoint->Receiver != NULL) {
            InterruptRestoreState(State);
            LogFatal("Endpoint", "two receivers on one endpoint");
            return -1;
        }
        Endpoint->Receiver = Self;
        InterruptRestoreState(State);

        if (SemaphoreP(&Self->IpcWait, TimeoutMs) != Success) {
            /* Timed out. Withdraw as the receiver - but only if nobody
             * has already claimed us. A sender that found us in the
             * window between the timeout and the lock has already
             * delivered, and dropping that message would lose a call
             * the sender is still blocked on. */
            State = InterruptDisable();
            if (Endpoint->Receiver == Self) {
                Endpoint->Receiver = NULL;
                InterruptRestoreState(State);
                return -1;
            }
            InterruptRestoreState(State);
            /* Delivery happened; fall through and take it. */
        }
    }

    if (Self->IpcResult < 0) {
        return -1;
    }
    if (Opcode != NULL) {
        *Opcode = Self->IpcOpcode;
    }
    if (Badge != NULL) {
        *Badge = Self->IpcBadge;
    }
    return Self->IpcResult;
}

/* EndpointReply */
int EndpointReply(const void *Buffer, size_t Length)
{
    Thread_t *Self = ThreadingGetCurrentThread(0);
    Thread_t *Partner;
    Process_t *PartnerProcess;
    size_t Copy;
    int State;

    if (Self == NULL || Self->IpcPartner == NULL) {
        return -1;      /* replying without an outstanding call */
    }
    if (Length > IPC_MESSAGE_MAX) {
        return -1;
    }

    Partner = (Thread_t*)Self->IpcPartner;
    PartnerProcess = (Process_t*)Partner->Process;

    Copy = Length;
    if (Copy > Partner->IpcRecvLength) {
        Copy = Partner->IpcRecvLength;
    }

    if (Copy > 0 && PartnerProcess != NULL) {
        /* Straight into the caller's reply buffer, in its own space. */
        if (EndpointCopyAcross(PartnerProcess->AddressSpace,
                (uintptr_t)Partner->IpcRecvBuffer, Buffer, Copy) != Success) {
            Partner->IpcResult = -1;
            SemaphoreV(&Partner->IpcWait, 1);
            Self->IpcPartner = NULL;
            return -1;
        }
    }

    State = InterruptDisable();
    Partner->IpcResult = (int)Copy;
    Self->IpcPartner = NULL;
    InterruptRestoreState(State);

    SemaphoreV(&Partner->IpcWait, 1);
    return (int)Copy;
}

/* EndpointPrint */
void EndpointPrint(void)
{
    int i;

    for (i = 0; i < ENDPOINT_MAX; i++) {
        if (GlbEndpoints[i].Used == 0) {
            continue;
        }
        LogInformation("Endpoint", "  id %u  owner %u  %d queued%s",
            GlbEndpoints[i].Id,
            (GlbEndpoints[i].Owner != NULL)
                ? ((Process_t*)GlbEndpoints[i].Owner)->Id : 0,
            GlbEndpoints[i].SenderCount,
            (GlbEndpoints[i].Receiver != NULL) ? ", receiver waiting" : "");
    }
}
