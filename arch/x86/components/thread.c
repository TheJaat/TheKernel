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
Context_t *ContextCreate(Flags_t ThreadFlags, uintptr_t Eip, uintptr_t StackTop)
{
    Context_t *Context = NULL;
    uintptr_t ContextAddress;

    if (StackTop == 0 || Eip == 0) {
        return NULL;
    }

    /* Only kernel threads exist so far. Ring 3 needs the user selectors
     * plus UserEsp/UserSs filled in, and a TSS esp0 that points at this
     * frame - see the reference's ContextCreate for the shape of it. */
    if (ThreadFlags & ~(Flags_t)(THREADING_IDLE)) {
        LogFatal("Context", "unsupported thread flags 0x%x", ThreadFlags);
        return NULL;
    }

    /* Keep the frame 4-byte aligned, which kmalloc_a already gives us
     * since the stack is page aligned and the struct is a whole number
     * of dwords. */
    ContextAddress = StackTop - sizeof(Context_t);
    Context = (Context_t*)ContextAddress;
    memset(Context, 0, sizeof(Context_t));

    /* Segments. irq_common reloads ds/es/fs/gs from the frame on the way
     * out, so these have to be valid kernel selectors. */
    Context->Ds = GDT_KDATA_SEGMENT;
    Context->Es = GDT_KDATA_SEGMENT;
    Context->Fs = GDT_KDATA_SEGMENT;
    Context->Gs = GDT_KDATA_SEGMENT;

    /* Ebp points at the top of the frame so a backtrace terminates
     * instead of wandering into whatever was on the page. */
    Context->Ebp = StackTop;
    Context->Esp = 0;       /* popad discards its esp slot */

    /* iret pops these three. */
    Context->Eip    = Eip;
    Context->Cs     = GDT_KCODE_SEGMENT;
    Context->Eflags = X86_THREAD_EFLAGS;

    /* irq_common does 'add esp, 8' before iret to drop these. */
    Context->Irq       = 0;
    Context->ErrorCode = 0;

    /* Ring-0 iret does not touch these. Left zero. */
    Context->UserEsp = 0;
    Context->UserSs  = 0;
    Context->UserArg = 0;

    return Context;
}