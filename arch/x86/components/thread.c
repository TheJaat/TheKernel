/* Includes
 * - System */
#include <system/threading.h>
#include <system/log.h>
#include <arch/x86/x32/context.h>
#include <arch/x86/x32/gdt.h>

/* Includes
 * - Library */
#include <stddef.h>
#include <string.h>

/* EFLAGS for a new thread: bit 1 is reserved and must be set, bit 9 is
 * IF. Starting with interrupts enabled matters - a thread entered with
 * IF clear would never be preempted and would own the cpu forever. */
#define X86_THREAD_EFLAGS           0x202

/* ContextCreate
 * Lays out an initial register frame at the top of a fresh stack so that
 * the standard irq_common exit path - popad, pop segments, add esp 8,
 * iret - drops straight into <Eip>.
 *
 * The frame goes at the very top because iret between two ring-0
 * segments does NOT pop esp/ss: after it, esp points just past Eflags,
 * and the thread's stack grows down from there through the rest of the
 * allocation. */
Context_t *ContextCreateUser(Flags_t ThreadFlags, uintptr_t Eip,
    uintptr_t StackTop, uintptr_t UserStackTop)
{
    Context_t *Context = NULL;
    uintptr_t ContextAddress;

    if (StackTop == 0 || Eip == 0) {
        return NULL;
    }

    if (ThreadFlags & ~(Flags_t)(THREADING_IDLE | THREADING_USERMODE)) {
        LogFatal("Context", "unsupported thread flags 0x%x", ThreadFlags);
        return NULL;
    }
    if ((ThreadFlags & THREADING_USERMODE) && UserStackTop == 0) {
        LogFatal("Context", "a user thread needs a user stack");
        return NULL;
    }

    /* Keep the frame 4-byte aligned, which kmalloc_a already gives us
     * since the stack is page aligned and the struct is a whole number
     * of dwords. */
    ContextAddress = StackTop - sizeof(Context_t);
    Context = (Context_t*)ContextAddress;
    memset(Context, 0, sizeof(Context_t));

    /* Segments. irq_common reloads ds/es/fs/gs from the frame on the way
     * out, so these have to be valid selectors for the target ring.
     *
     * The low two bits are the requested privilege level. A ring-3
     * selector without them is still a ring-0 request and faults. */
    if (ThreadFlags & THREADING_USERMODE) {
        Context->Ds = GDT_UDATA_SEGMENT | 0x3;
        Context->Es = GDT_UDATA_SEGMENT | 0x3;
        Context->Fs = GDT_UDATA_SEGMENT | 0x3;
        Context->Gs = GDT_UDATA_SEGMENT | 0x3;
    }
    else {
        Context->Ds = GDT_KDATA_SEGMENT;
        Context->Es = GDT_KDATA_SEGMENT;
        Context->Fs = GDT_KDATA_SEGMENT;
        Context->Gs = GDT_KDATA_SEGMENT;
    }

    /* Ebp points at the top of the frame so a backtrace terminates
     * instead of wandering into whatever was on the page. */
    Context->Ebp = StackTop;
    Context->Esp = 0;       /* popad discards its esp slot */

    /* iret pops these three. */
    Context->Eip    = Eip;
    Context->Cs     = (ThreadFlags & THREADING_USERMODE)
                    ? (GDT_UCODE_SEGMENT | 0x3) : GDT_KCODE_SEGMENT;
    Context->Eflags = X86_THREAD_EFLAGS;

    /* irq_common does 'add esp, 8' before iret to drop these. */
    Context->Irq       = 0;
    Context->ErrorCode = 0;

    /* A ring-0 iret does not touch these. A ring-3 one pops both, and
     * this is the only way the user stack ever gets loaded - there is no
     * instruction between the iret and the first user instruction. */
    if (ThreadFlags & THREADING_USERMODE) {
        Context->UserEsp = UserStackTop;
        Context->UserSs  = GDT_UDATA_SEGMENT | 0x3;
        Context->Ebp     = UserStackTop;
    }
    else {
        Context->UserEsp = 0;
        Context->UserSs  = 0;
    }
    Context->UserArg = 0;

    return Context;
}

/* ContextCreate
 * Kernel thread wrapper, kept so existing callers do not change. */
Context_t *ContextCreate(Flags_t ThreadFlags, uintptr_t Eip, uintptr_t StackTop)
{
    return ContextCreateUser(ThreadFlags, Eip, StackTop, 0);
}