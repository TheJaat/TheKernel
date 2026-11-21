/* linker-aarch64.ld - where the kernel lives on AArch64.
 *
 * Linked high, like the x86_64 kernel: every address the code uses is
 * in the top 2 GiB of the address space. Unlike it, there is no fixed
 * physical address. A PC always has memory at 1 MiB; AArch64 machines
 * put their memory wherever they like (QEMU's virt machine at 1 GiB, a
 * Raspberry Pi at 0), so the kernel finds out where it was loaded when
 * it starts, and maps
 *
 *     virtual KERNEL_WINDOW + x  ->  physical (the 2 MiB step loaded in) + x
 *
 * TEXT_OFFSET is the one thing agreed with the loader: how far into a
 * 2 MiB step the image begins. Both numbers are repeated in
 * src/aarch64.rs.
 *
 * What is booted is not this ELF file but a flat image made from it
 * (`objcopy -O binary`): the bytes of the sections, in order, starting
 * with the header in .text.entry. The ELF file is kept for its symbols.
 */

ENTRY(_start)

KERNEL_WINDOW = 0xFFFFFFFF80000000;
TEXT_OFFSET   = 0x80000;

PHDRS
{
    text   PT_LOAD FLAGS(5);    /* R-X */
    rodata PT_LOAD FLAGS(4);    /* R-- */
    data   PT_LOAD FLAGS(6);    /* RW- */
}

SECTIONS
{
    . = KERNEL_WINDOW + TEXT_OFFSET;
    __kernel_start = .;

    __text_start = .;
    .text :
    {
        KEEP(*(.text.entry))    /* the image header must be the first bytes */
        *(.text .text.*)
    } :text

    . = ALIGN(4K);
    __text_end = .;
    __rodata_start = .;
    .rodata :
    {
        *(.rodata .rodata.*)
    } :rodata

    . = ALIGN(4K);
    __rodata_end = .;
    __data_start = .;
    .data :
    {
        *(.data .data.*)
        *(.got .got.*)
    } :data

    /* Not in the flat image at all: the image ends with .data, and the
     * kernel's first instructions zero this range themselves - eight
     * bytes at a time, which is why both ends are aligned. The header
     * tells the loader how much memory to leave for it. */
    . = ALIGN(4K);
    __bss_start = .;
    .bss (NOLOAD) :
    {
        KEEP(*(.bss.boot_tables))
        KEEP(*(.bss.boot_stack))
        *(.bss .bss.*)
        *(COMMON)
    } :data

    . = ALIGN(4K);
    __bss_end = .;
    __data_end = .;
    __kernel_end = .;

    __image_size = __kernel_end - __kernel_start;

    /DISCARD/ :
    {
        *(.eh_frame .eh_frame_hdr)
        *(.comment)
        *(.note .note.*)
    }
}
