;org 0x300000
;org 0xb000    ; Set the origin address for the code. This tells the assembler
               ; that the code should be loaded at memory address 0x0B00.
               ; So all the jmp statement and string declaration offset is
               ; calculated based on it.
;; In ELF file format org directive is invalid.
;; org directive is only for the binary output format.                     


BITS 32                  ; Specify that the code is 32-bit.

section .text
global kernel_entry
kernel_entry:            ; Label for the kernel entry point.

jmp start		; jmp after the includes

;; Include files
%include "print32.inc"

;; Parameters passed from the bootloader through register.
; EAX - Multiboot Magic
; EBX - Contains address of the multiboot structure, but
;		it should be located in stack as well.
; EDX - Contains address of the os boot structure
start:
	cli
	;; set up the stack
	mov eax, 0x10
	mov ss, ax
	mov esp, 0x9F000
	mov ebp, esp

    mov esi, sKernelWelcomeStatement
    call PrintString32

jmp $		; Infinite loop to halt execution after printing the message.

;section .data
sKernelWelcomeStatement: db 'Welcome to ELF 32-Bit Kernel Land.', 0
                         ; Define the welcome message string, terminated by a null byte (0).

times 1024 - ($ - $$) db 0	; Fill the rest of the 1 KB (1024 bytes) space with zeros.

