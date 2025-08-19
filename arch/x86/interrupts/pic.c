/* Includes */
#include <arch/x86/x32/arch_x32.h>
#include <arch/x86/pic.h>
// Log
#include <system/log.h>

#define PIC1_COMMAND	0x20	// Port for master PIC's command register
#define PIC2_COMMAND	0xA0	// Port for slave PIC's command register

#define PIC1_DATA	0x21 // Port for master PIC's data register
#define PIC2_DATA	0xA1 // Port for slave PIC's data register

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
	outb(PIC1_DATA, 0x20);

	/* Remap Secondary PIC to 0x28 - 0x30 */
	outb(PIC2_DATA, 0x28);

	/* Send initialization words, they define
	 * which PIC connects to where */
	outb(PIC1_DATA, 0x04);
	outb(PIC2_DATA, 0x02);

	/* Enable i86 mode */
	outb(PIC1_DATA, 0x01);
	outb(PIC2_DATA, 0x01);

	/* Mask all irqs in PIC (Disabling) */
	outb(PIC1_DATA, 0xFF);
	outb(PIC2_DATA, 0xFF);

    LogInformation("PIC", "PicInit, Intitializing & Remapping Done.");
}
