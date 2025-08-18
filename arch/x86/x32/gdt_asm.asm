bits 32
segment .text

;Functions in this asm
global GdtInstall

extern Gdtptr

; void GdtInstall()
; Load the given gdt into gdtr
GdtInstall:
	; Save EAX
	push eax

	; Install GDT
	lgdt [Gdtptr]

	; Jump into correct descriptor
	xor eax, eax
	mov ax, 0x10
	mov ds, ax
	mov es, ax
	mov fs, ax
	mov gs, ax
	mov ss, ax

	jmp 0x08:done

	done:
	; Restore EAX
	pop eax
	ret

