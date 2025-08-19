; x86 Direct Port IO Code
;
bits 32
segment .text

;Functions in this asm
global inb
global inw
global inl

global outb
global outw
global outl

; uint8_t inb(uint16_t port)
; Recieves a byte from a port
inb:
	; Setup frame
	push ebp
	mov ebp, esp

	; Save stuff
	push edx

	; Get byte
	xor al, al
	xor edx, edx
	mov dx, [ebp + 8]
	in al, dx

	; Restore
	pop edx

	; Leave frame
	pop ebp
	ret

; uint16_t inw(uint16_t port)
; Recieves a word from a port
inw:
	; Setup frame
	push ebp
	mov ebp, esp

	; Save stuff
	push edx

	; Get word
	xor ax, ax
	xor edx, edx
	mov dx, [ebp + 8]
	in ax, dx

	; Restore
	pop edx

	; Leave
	pop ebp
	ret

; uint32_t inl(uint16_t port)
; Recieves a long from a port
inl:
	; Setup frame
	push ebp
	mov ebp, esp

	; Save stuff
	push edx

	; Get dword
	xor eax, eax
	xor edx, edx
	mov dx, [ebp + 8]
	in eax, dx

	; Restore
	pop edx

	; Leave
	pop ebp
	ret

; void outb(uint16_t port, uint8_t data)
; Sends a byte to a port
outb:
	; Setup frame
	push ebp
	mov ebp, esp

	; Save stuff
	push eax
	push edx

	; Get data
	xor eax, eax
	xor edx, edx
	mov dx, [ebp + 8]
	mov al, [ebp + 12]
	out dx, al

	; Restore
	pop edx
	pop eax

	; Leave
	pop ebp
	ret

; void outw(uint16_t port, uint16_t data)
; Sends a word to a port
outw:
	; Stack Frame
	push ebp
	mov ebp, esp

	; Save stuff
	push eax
	push edx

	; Get data
	xor eax, eax
	xor edx, edx
	mov dx, [ebp + 8]
	mov ax, [ebp + 12]
	out dx, ax

	; Restore
	pop edx
	pop eax

	; Release stack frame
	pop ebp
	ret

; void outl(uint16_t port, uint32_t data)
; Sends a long to a port
outl:
	; Stack Frame
	push ebp
	mov ebp, esp

	; Save stuff
	push eax
	push edx

	; Get data
	xor edx, edx
	mov dx, [ebp + 8]
	mov eax, [ebp + 12]
	out dx, eax

	; Restore
	pop edx
	pop eax

	; Release stack frame
	pop ebp
	ret
