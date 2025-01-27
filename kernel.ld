ENTRY(kernel_entry) /* Entry point is the 'kernel_entry' label in stage2.asm */

SECTIONS
{
   . = 0x100000; /* Start address for the binary */

    .text : {
        *(.text)
    }

    .rodata : {
        *(.rodata)
    }

    .data : {
        *(.data)
    }

    .bss : {
        *(.bss)
    }
}

