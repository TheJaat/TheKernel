//! The first instructions of the kernel on AArch64.
//!
//! A loader has put the kernel's image somewhere in memory and jumped
//! to its first byte, with the address of the device tree in register
//! x0. Nothing else is promised: the MMU is off, so every address is a
//! physical one; there is no stack; the memory the kernel's zeroed
//! variables will live in holds whatever was there.
//!
//! The Rust code is linked to run at the top of the address space
//! (`KERNEL_WINDOW`), which does not exist yet. So this file has to:
//!
//!   1. come down to exception level 1, if the loader entered at 2;
//!   2. zero the `.bss` section (a flat image carries no bytes for it);
//!   3. build translation tables that put the image where it was
//!      linked to be - wherever it was loaded;
//!   4. turn the MMU on, jump to the high addresses, set a stack;
//!   5. call `kernel_main`.
//!
//! Until step 4 the CPU runs at addresses the code was not linked for.
//! That works as long as every instruction says where things are
//! *relative to itself* - "the table 8,000 bytes after me" - and that
//! is what `adr` and `adrp` do. It is why this is assembly: Rust makes
//! no such promise.
//!
//! # The image header
//!
//! The image begins with 64 bytes in the form the Linux kernel made
//! standard for AArch64, which is what QEMU, U-Boot and the Raspberry
//! Pi's firmware all look for. The first four bytes are an instruction
//! - the loader jumps to the very start - so they jump over the rest.
//!
//! # The tables
//!
//! AArch64 has two table roots: `TTBR0_EL1` for addresses that begin
//! with zeroes (programs, later) and `TTBR1_EL1` for those that begin
//! with ones (the kernel).
//!
//! ```text
//!   TTBR1  boot_l0_high[511] -> boot_l1_high
//!            boot_l1_high[510] -> boot_l2_kernel   2 MiB steps: the image
//!            boot_l1_high[511] -> boot_l2_device   empty; see mmu.rs
//!   TTBR0  boot_l0_low[0]    -> boot_l1_low
//!            boot_l1_low[n]   = 1 GiB, mapped to itself, for the GiB
//!                               the image is in and the one the device
//!                               tree is in
//! ```
//!
//! The low mapping is there for the instruction after the MMU comes
//! on, which is still at a physical address; so that the kernel can
//! read the device tree and write its first real page tables, at
//! their physical addresses; and for the few instructions that put
//! those tables in place of these (mmu.rs). Then it is turned off.

use core::arch::global_asm;

use kernel64::aarch64::{descriptor, translation_control, CONTROL_MMU_OFF, CONTROL_MMU_ON, MEMORY_KINDS, TEXT_OFFSET};

/// The boot stack: 60 KiB, with a page below it (`__boot_stack_guard`)
/// that is there to be missing. The boot tables map 2 MiB at a time,
/// so at first the guard is mapped like everything else; the kernel's
/// own tables (mm.rs) leave it out, and from then on running off the
/// end of the stack is a fault instead of a quiet overwrite of
/// whatever lies below.
///
/// Guard and stack together are 64 KiB, starting at an address that is
/// a multiple of 64 KiB - the shape of every kernel thread's stack
/// (mm.rs). The exception vectors rely on that shape to tell, from the
/// stack pointer alone, that a stack has run out (vectors.rs).
const BOOT_STACK_SIZE: usize = 60 * 1024;

/// The stack an exception is handled on when the stack it arrived on
/// has run out: 12 KiB, one page up from a multiple of 64 KiB - so
/// that its own addresses never look like a stack that has run out.
const OVERFLOW_STACK_SIZE: usize = 12 * 1024;

global_asm!(
    r#"
    .section .text.entry, "ax"
    .global _start
_start:
    b       .Lbegin                 // "code0": over the header
    .long   0                       // "code1"
    .quad   {text_offset}           // how far into a 2 MiB step to load this
    .quad   __image_size            // how much memory it needs, .bss included
    .quad   0b1010                  // little-endian, 4 KiB pages, anywhere in memory
    .quad   0
    .quad   0
    .quad   0
    .ascii  "ARM\x64"               // the magic number loaders look for
    .long   0

.Lbegin:
    msr     daifset, #0xf           // no interrupts of any kind
    mov     x19, x0                 // the device tree; x19 to x21 survive to kernel_main
    mrs     x20, CurrentEL
    lsr     x20, x20, #2            // the level the loader entered at: 1 or 2
    cmp     x20, #1
    b.eq    .Lat_el1
    cmp     x20, #2
    b.ne    .Lhang                  // EL3 is firmware's; nothing here knows how to leave it

    // ---- 1. from EL2 down to EL1 ------------------------------------
    // EL2 is where a hypervisor lives. This kernel is not one, so it
    // sets EL2 to stay out of the way and "returns" to EL1 - an
    // exception return to a level that was never left.
    mov     x1, #(1 << 31)
    msr     hcr_el2, x1             // EL1 is 64-bit; nothing it does traps to EL2
    mov     x1, #3
    msr     cnthctl_el2, x1         // EL1 may use the timer and its counter
    msr     cntvoff_el2, xzr        // ...which reads the same as EL2's
    mov     x1, #0x33ff
    msr     cptr_el2, x1            // floating point does not trap to EL2
    msr     hstr_el2, xzr
    // If the interrupt controller is one the CPU talks to through its
    // own registers (GIC version 3), EL2 decides whether EL1 may use
    // them. It may.
    mrs     x1, id_aa64pfr0_el1
    ubfx    x1, x1, #24, #4
    cbz     x1, 1f
    mrs     x1, s3_4_c12_c9_5       // ICC_SRE_EL2
    orr     x1, x1, #0xf            // registers on, at EL2 and for EL1
    msr     s3_4_c12_c9_5, x1
    isb
1:
    ldr     x1, ={control_off}
    msr     sctlr_el1, x1           // EL1 starts with its MMU off
    mov     x1, #0x3c5              // "came from": EL1, its own stack pointer, interrupts off
    msr     spsr_el2, x1
    adr     x1, .Lat_el1
    msr     elr_el2, x1             // "return" to the next instruction
    eret

.Lat_el1:
    // Where was the image loaded? `adr` answers relative to this
    // instruction, so the answer is the truth, not what the linker
    // assumed.
    adr     x21, _start
    and     x1, x21, #0x1fffff
    cmp     x1, #{text_offset}      // the tables below map 2 MiB steps, so only
    b.ne    .Lhang                  // this position inside a step can work

    // ---- 2. zero .bss -----------------------------------------------
    adrp    x1, __bss_start
    add     x1, x1, :lo12:__bss_start
    adrp    x2, __bss_end
    add     x2, x2, :lo12:__bss_end
.Lzero:
    cmp     x1, x2
    b.hs    .Lzeroed
    str     xzr, [x1], #8
    b       .Lzero
.Lzeroed:

    // ---- 3. the tables ----------------------------------------------
    // They are in .bss, so they are all zeroes: every entry "not
    // mapped". An entry that points at a table is the table's address
    // with the two low bits set.
    ldr     x4, ={normal_block}

    // Low half: the GiB the image is in, and the one the device tree
    // is in, each mapped to itself.
    adrp    x1, boot_l0_low
    adrp    x2, boot_l1_low
    orr     x3, x2, #3
    str     x3, [x1]
    lsr     x5, x21, #30
    lsl     x6, x5, #30
    orr     x6, x6, x4
    str     x6, [x2, x5, lsl #3]
    lsr     x5, x19, #30
    cmp     x5, #512                // a tree above 512 GiB (or no tree, and
    b.hs    .Lno_tree               // rubbish in x0) is not mapped
    lsl     x6, x5, #30
    orr     x6, x6, x4
    str     x6, [x2, x5, lsl #3]
.Lno_tree:

    // High half: the image, in 2 MiB steps, from the step it starts in
    // to the one its last byte is in.
    adrp    x1, boot_l0_high
    adrp    x2, boot_l1_high
    orr     x3, x2, #3
    str     x3, [x1, #(511 * 8)]
    adrp    x5, boot_l2_kernel
    orr     x3, x5, #3
    str     x3, [x2, #(510 * 8)]
    adrp    x6, boot_l2_device
    orr     x3, x6, #3
    str     x3, [x2, #(511 * 8)]
    bic     x7, x21, #0x1fffff      // physical address of the first step
    adrp    x8, __kernel_end        // physical address of the end
    add     x8, x8, :lo12:__kernel_end
.Lmap:
    orr     x9, x7, x4
    str     x9, [x5], #8
    add     x7, x7, #0x200000
    cmp     x7, x8
    b.lo    .Lmap

    // ---- 4. the MMU -------------------------------------------------
    ldr     x1, ={memory_kinds}
    msr     mair_el1, x1
    mrs     x1, id_aa64mmfr0_el1    // how many bits of physical address this CPU has
    and     x1, x1, #0xf
    mov     x2, #5
    cmp     x1, x2
    csel    x1, x1, x2, ls          // ...but no more than the tables can express
    ldr     x2, ={translation}
    orr     x2, x2, x1, lsl #32
    msr     tcr_el1, x2
    adrp    x1, boot_l0_low
    msr     ttbr0_el1, x1
    adrp    x1, boot_l0_high
    msr     ttbr1_el1, x1
    isb                             // "the registers above are set: use them from here"
    tlbi    vmalle1                 // forget any translation remembered from before
    dsb     nsh
    isb
    ldr     x1, ={control_on}
    msr     sctlr_el1, x1
    isb
    // The MMU is on. This instruction was fetched through the low
    // mapping. The next loads an address as the linker wrote it - a
    // high one - and goes there.
    ldr     x1, =.Lhigh
    br      x1
.Lhigh:
    ldr     x1, =boot_stack_top
    mov     sp, x1

    // ---- 5. Rust ----------------------------------------------------
    mov     x0, x19                 // the device tree
    mov     x1, x20                 // the level entered at
    mov     x2, x21                 // where the image was loaded
    mov     x29, xzr                // no caller: a backtrace ends here
    mov     x30, xzr
    bl      kernel_main

.Lhang:
    wfe
    b       .Lhang

    .ltorg                          // the constants the `ldr x, =` lines above load

    // The tables: one page each, page-aligned, so `adrp` alone finds them.
    .section .bss.boot_tables, "aw", %nobits
    .balign 4096
    .global boot_l2_device
boot_l0_low:    .skip 4096
boot_l1_low:    .skip 4096
boot_l0_high:   .skip 4096
boot_l1_high:   .skip 4096
boot_l2_kernel: .skip 4096
boot_l2_device: .skip 4096

    .section .bss.boot_stack, "aw", %nobits
    .balign 0x10000
    .global __boot_stack_guard
__boot_stack_guard:
    .skip 4096
    .skip {stack_size}
boot_stack_top:
    .skip 4096                      // where a guard would be: unused
    .skip {overflow_size}
    .global overflow_stack_top
overflow_stack_top:
"#,
    text_offset = const TEXT_OFFSET,
    control_off = const CONTROL_MMU_OFF,
    control_on = const CONTROL_MMU_ON,
    normal_block = const descriptor::NORMAL_BLOCK,
    memory_kinds = const MEMORY_KINDS,
    translation = const translation_control(0),
    stack_size = const BOOT_STACK_SIZE,
    overflow_size = const OVERFLOW_STACK_SIZE,
);
