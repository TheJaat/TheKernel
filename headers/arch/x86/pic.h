#ifndef __X86_PIC_H__
#define __X86_PIC_H__

#include <defs.h>

/* The 8259A pair gives us 16 IRQ lines. Line 2 is the cascade: the slave
 * is wired into it, so it must stay unmasked for any of lines 8-15 to be
 * delivered, and it never has a handler of its own. */
#define PIC_NUM_LINES           16
#define PIC_CASCADE_LINE        2

/* After PicInit remaps them, IRQ n arrives on IDT vector 32 + n. */
#define PIC_VECTOR_BASE         32

#ifdef __cplusplus
extern "C" {
#endif

/* PicInit
 * Remaps both PICs to vectors 32..47 and masks every line. Lines are
 * unmasked individually as handlers are registered. */
void PicInit(void);

/* PicUnmaskLine / PicMaskLine
 * Enable or disable delivery of a single IRQ line. Unmasking a line on
 * the slave also unmasks the cascade, otherwise nothing arrives. */
void PicUnmaskLine(int Line);
void PicMaskLine(int Line);

/* PicIsLineMasked
 * Returns 1 if the line is currently masked. */
int PicIsLineMasked(int Line);

/* PicSendEoi
 * Acknowledges an interrupt. Lines 8-15 need the slave acknowledged
 * before the master. */
void PicSendEoi(int Line);

/* PicIsSpurious
 * Returns 1 if this is a spurious interrupt on line 7 or 15 - the PIC
 * raised the line but dropped it before we read the vector. These must
 * not be handled, and line 7 must not be acknowledged at all. */
int PicIsSpurious(int Line);

#ifdef __cplusplus
}
#endif

#endif /* __X86_PIC_H__ */