;org 0x300000
;org 0xb000    ; Set the origin address for the code. This tells the assembler
               ; that the code should be loaded at memory address 0x0B00.
               ; So all the jmp statement and string declaration offset is
               ; calculated based on it.
;; In ELF file format org directive is invalid.
;; org directive is only for the binary output format.                     


BITS 32                  ; Specify that the code is 32-bit.

section .multiboot
align 4
multiboot_header:
    dd 0x1BADB002               ; Multiboot magic number
    dd 0x03                     ; Flags: request memory info (bit 0) and boot device (bit 1)
    dd -(0x1BADB002 + 0x03)     ; Checksum (sum must be 0)

section .text
global kernel_entry

extern kmain


kernel_entry:            ; Label for the kernel entry point.
    cli

    jmp start            ; jmp after the includes
    hlt
    jmp $


;; Include files here
%include "print32.inc"

;; Parameters passed from the bootloader through register.
; EAX - Multiboot Magic
; EBX - Contains address of the multiboot structure, but
;		it should be located in stack as well.
; EDX - Contains address of the os boot structure
start:
    cli

    ;; set up the stack
; Causing problem in grub boot, so commented
; for the time being
;    mov eax, 0x10
;    mov ss, ax
    mov esp, 0x9F000
    mov ebp, esp

    ; Place the multiboot structure and boot descriptor structure on the stack
    push edx
    push ebx

    ;; Clear the screen to white background
    mov bh, 0xf     ; White background
    mov bl, 0x0     ; Black background
    call ClearScreen32

    ;; Print the welcome message
    mov esi, sKernelWelcomeStatement
    call PrintString32

    ;; Call the kernel main
    call kmain
hlt
jmp $		; Infinite loop to halt execution after printing the message.


;section .data
section .rodata
sKernelWelcomeStatement: db 'Welcome to ELF 32-Bit Kernel Land.', 0
                         ; Define the welcome message string, terminated by a null byte (0).

times 1024 - ($ - $$) db 0	; Fill the rest of the 1 KB (1024 bytes) space with zeros.

