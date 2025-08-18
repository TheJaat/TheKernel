bits 32
segment .text

;Functions in this asm
global TssInstall

; void TssInstall(int gdt_index)
; Load the given TSS descriptor
; index
TssInstall:
	; Create a stack frame
	push ebp
	mov ebp, esp

	; Save registers
	push eax
	push ecx

	; Calculate index
	mov eax, [ebp + 8]
	mov ecx, 0x8
	mul ecx
	or eax, 0x3

	; Load task register
	ltr ax

	; Restore
	pop ecx
	pop eax

	; Leave frame
	pop ebp
	ret

