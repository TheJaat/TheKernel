#ifndef __ARCH_X32_H__
#define __ARCH_X32_H__

#include <stdint.h>

/* X86-32 Address Space */
#define ADDRESSSPACE_MEMBERS		uintptr_t Cr3; void *PageDirectory;

#define MEMORY_SEGMENT_KERNEL_DATA_LIMIT	0xFFFFFFFF

#define MEMORY_LOCATION_RING3_ARGS		0x1F000000	/* Base for ring3 arguments */
#define MEMORY_LOCATION_RING3_CODE		0x20000000	/* Base for ring3 code */
#define MEMORY_LOCATION_RING3_HEAP		0x30000000	/* Base for ring3 heap */
#define MEMORY_LOCATION_RING3_SHM		0xA0000000	/* Base for ring3 shm */
#define MEMORY_LOCATION_RING3_IOSPACE		0xB0000000	/* Base for ring3 io-space (1gb) */
#define MEMORY_LOCATION_RING3_IOSPACE_END	0xF0000000

#define MEMORY_SEGMNET_RING3_CODE_LIMIT		MEMORY_LOCATION_RING3_HEAP
#define MEMORY_SEGMENT_RING3_DATA_LIMIT		0xFFFFFFFF

#define MEMORY_SEGMENT_STACK_BASE		0xFFFFFFFF	/* RING3 Stack Initial */
#define MEMORY_SEGMENT_STACK_LIMIT		0xFFFFFF	/* RING3 Stack Space: 16 mB */
#define MEMORY_LOCATION_STACK_END		(MEMORY_SEGMENT_STACK_BASE - MEMORY_SEGMENT_STACK_LIMIT)


/* Interrupt stuff */
#define NUM_ISA_INTERRUPTS			16

/* Port IO */
extern uint8_t inb(uint16_t port);
extern uint16_t inw(uint16_t port);
extern uint32_t inl(uint16_t port);

extern void outb(uint16_t port, uint8_t data);
extern void outw(uint16_t port, uint16_t data);
extern void outl(uint16_t port, uint32_t data);


/* Architecture Memory Layout, this
 * gives you an idea how memory layout
 * is on the x86-32 platform in MollenOS 
 * 0x00			=>			0x10000000 (Kernel Memory Space 256 mb)
 * 0x10000000		=>			0xB0000000 (Application Memory Space 2.5gb) 
 * 0xB0000000		=>			0xF0000000 (Driver Io Memory Space, 1gb)
 * 0xF0000000		=>			0xFF000000 (Empty)
 * 0xFF000000		=>			0xFFFFFFFF (Application Stack Space, 16mb) 
 */
#define MEMORY_LOCATION_KERNEL		0x100000 	/* Kernel Image Space: 1024 kB */
#define MEMORY_LOCATION_RAMDISK		0x200000 	/* RamDisk Image Space: 1024 kB */
#define MEMORY_LOCATION_BITMAP		0x300000 	/* Bitmap Space: 12 mB */
#define MEMORY_LOCATION_HEAP		0x1000000	/* Heap Space: 64 mB */
#define MEMORY_LOCATION_HEAP_END	0x4000000
#define MEMORY_LOCATION_VIDEO		0x4000000	/* Video Space: 16 mB */
#define MEMORY_LOCATION_RESERVED	0x5000000	/* Driver Space: 190~ mB */
#define MEMORY_LOCATION_KERNEL_END	0x10000000

// Memory
#ifndef PAGE_SIZE
	#define PAGE_SIZE 0x1000
#endif

#ifndef PAGE_MASK
	#define PAGE_MASK 0xFFFFF000
#endif

#ifndef ATTRIBUTE_MASK
	#define ATTRIBUTE_MASK 0x00000FFF
#endif


#define INTERRUPT_SYSCALL				0x80

#endif /* __ARCH_X32_H__ */

