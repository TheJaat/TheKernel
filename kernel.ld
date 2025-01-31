ENTRY(kernel_entry)  /* Entry point */

SECTIONS
{
    . = 0x100000;  /* Ensure kernel starts at 0x100000 */

    /* Place the Multiboot header first */
    .multiboot ALIGN(4K) : {
        *(.multiboot)
    }

    /* Ensure .text is right after .multiboot */
    .text ALIGN(4K) : {
        *(.text)
    }

    .rodata ALIGN(4K) : {
        *(.rodata)
    }

    .data ALIGN(4K) : {
        *(.data)
    }

    .bss ALIGN(4K) : {
        *(COMMON)
        *(.bss)
    }
}

