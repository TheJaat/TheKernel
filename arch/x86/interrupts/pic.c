/* Includes */
#include <arch/x86/x32/arch_x32.h>
#include <arch/x86/pic.h>
// Log
#include <system/log.h>

#define PIC1_COMMAND	0x20	// Port for master PIC's command register
#define PIC2_COMMAND	0xA0	// Port for slave PIC's command register

#define PIC1_DATA	0x21 // Port for master PIC's data register
#define PIC2_DATA	0xA1 // Port for slave PIC's data register

#define PIC_EOI		0x20 // End-of-interrupt command (OCW2)
#define PIC_READ_ISR	0x0B // OCW3: select the in-service register

/* Cached mask, one bit per line, 1 = masked. The PIC data ports are
 * write-mostly in practice, so track it here rather than reading back. */
static uint16_t GlbPicMask = 0xFFFF;

/* Initializes and disables */
void PicInit(void)
{
    LogInformation("PIC", "PicInit, Initializing & Remapping the PIC.");
	// Initialize and Remap the PIC

/* The reason for Remapping is that the
 * By default when initialized:
 * 	Master is mapped to 0x08 to 0x0F
 *	Slave is mapped to 0x70 to 0x77
 * && in x86 by default interrupts from 0x00 to 0x1F (31)
 * is reserved for the exceptions. So Master PIC IRQ cause conflicts
 * with the exceptions. Thus we need to REMAP the PIC
 */

	/* Send INIT (0x10) and IC4 (0x1) Commands*/
	outb(PIC1_COMMAND, 0x11);
	outb(PIC2_COMMAND, 0x11);

	/* Remap primary PIC to 0x20 - 0x28 */
	outb(PIC1_DATA, PIC_VECTOR_BASE);

	/* Remap Secondary PIC to 0x28 - 0x30 */
	outb(PIC2_DATA, PIC_VECTOR_BASE + 8);

	/* Send initialization words, they define
	 * which PIC connects to where */
	outb(PIC1_DATA, 0x04);
	outb(PIC2_DATA, 0x02);

	/* Enable i86 mode */
	outb(PIC1_DATA, 0x01);
	outb(PIC2_DATA, 0x01);

	/* Mask all irqs in PIC (Disabling). Lines get unmasked one at a
	 * time as InterruptRegister hands out handlers for them. */
	GlbPicMask = 0xFFFF;
	outb(PIC1_DATA, 0xFF);
	outb(PIC2_DATA, 0xFF);

    LogInformation("PIC", "PicInit, Intitializing & Remapping Done.");
}

/* PicApplyMask
 * Pushes the cached mask out to both chips. */
static void PicApplyMask(void)
{
	outb(PIC1_DATA, (uint8_t)(GlbPicMask & 0xFF));
	outb(PIC2_DATA, (uint8_t)((GlbPicMask >> 8) & 0xFF));
}

/* PicUnmaskLine */
void PicUnmaskLine(int Line)
{
	if (Line < 0 || Line >= PIC_NUM_LINES) {
		return;
	}

	GlbPicMask &= (uint16_t)~(1u << Line);

	/* The slave is wired into line 2. Leave it masked and nothing from
	 * lines 8-15 ever reaches the cpu. */
	if (Line >= 8) {
		GlbPicMask &= (uint16_t)~(1u << PIC_CASCADE_LINE);
	}

	PicApplyMask();
}

/* PicMaskLine */
void PicMaskLine(int Line)
{
	if (Line < 0 || Line >= PIC_NUM_LINES || Line == PIC_CASCADE_LINE) {
		return;
	}

	GlbPicMask |= (uint16_t)(1u << Line);

	/* If every slave line is masked again, the cascade can go too. */
	if ((GlbPicMask & 0xFF00) == 0xFF00) {
		GlbPicMask |= (uint16_t)(1u << PIC_CASCADE_LINE);
	}

	PicApplyMask();
}

/* PicIsLineMasked */
int PicIsLineMasked(int Line)
{
	if (Line < 0 || Line >= PIC_NUM_LINES) {
		return 1;
	}
	return (GlbPicMask & (1u << Line)) ? 1 : 0;
}

/* PicSendEoi */
void PicSendEoi(int Line)
{
	/* Order matters: the slave has to be told first, otherwise the
	 * master re-raises the cascade. */
	if (Line >= 8) {
		outb(PIC2_COMMAND, PIC_EOI);
	}
	outb(PIC1_COMMAND, PIC_EOI);
}

/* PicReadIsr
 * Reads the in-service register of both chips as one 16-bit value. */
static uint16_t PicReadIsr(void)
{
	outb(PIC1_COMMAND, PIC_READ_ISR);
	outb(PIC2_COMMAND, PIC_READ_ISR);
	return (uint16_t)(inb(PIC1_COMMAND) | (inb(PIC2_COMMAND) << 8));
}

/* PicIsSpurious */
int PicIsSpurious(int Line)
{
	/* Only lines 7 and 15 can be spurious. */
	if (Line != 7 && Line != 15) {
		return 0;
	}

	/* If the ISR bit is set the interrupt is real. */
	if (PicReadIsr() & (1u << Line)) {
		return 0;
	}

	return 1;
}