bits 32
segment .text

;Functions in this asm
global IdtInstall

extern Idtptr

; void IdtInstall()
; Load the given TSS descriptor
; index
IdtInstall:
	; Install IDT
	lidt [Idtptr]

	; Return
	ret 
