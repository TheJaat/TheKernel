%ifndef _PRINT_32_INC_
%define _PRINT_32_INC_

BITS 32

%define VGA_DEFAULT_MEM_ADDR	0xb8000

%define VGA_ROW		80
%define VGA_COLOUMN	25

; 80*25 Colors
%define 		BLACK		0x0
%define 		BLUE 		0x1
%define 		GREEN 		0x2
%define 		CYAN 		0x3
%define 		RED 		0x4
%define 		MAGENTA 	0x5
%define 		BROWN 		0x6
%define 		LGRAY 		0x7
%define 		DGRAY		0x8
%define 		LBLUE		0x9
%define 		LGREEN		0xA
%define 		LCYAN		0xB
%define 		LRED		0xC
%define 		LMAGENTA	0xD
%define 		YELLOW		0xE
%define 		WHITE		0xF


;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
; **************************
; ClearScreen32
; IN:
	; BL: Foreground color
	; BH; Background color
;	Nothing
; Registers:
; 	- Conserves all the Registers
; **************************
ClearScreen32:
	pushad		; Push the complete state
	
	mov esi, VGA_DEFAULT_MEM_ADDR
	mov ecx, VGA_ROW * VGA_COLOUMN
.ClearScreen32Loop:
	mov byte [esi], ' '		; Space
	mov byte [esi + 1], 0x07	; default color - lightgrey on black
	
	add esi, 2			; Increment VGA memory pointer by 2 bytes.

	loop .ClearScreen32Loop		; Loop until ecx is 0
	popad		; Pop all the saved state

ret
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;


;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;
; **************************
; PrintString32
; IN:
; 	- ESI: Null-Terminated String
; Registers:
; 	- Conserves all the registers
; **************************
PrintString32:
	pushad		; push all 32 bit registers onto the stack
	
	mov ecx, VGA_ROW * VGA_COLOUMN
	mov edi, VGA_DEFAULT_MEM_ADDR
	
	shl bh, 4
	or bl, bh
	
.PrintString32Loop:
	lodsb		; it reads character from the esi to al
	test al, al
	je .PrintString32Done

	;; The first byte in 80*25 ie. mode 0x3 has to be the character
	;; followed by the color,
	;; 	which is lower 4 bits for foreground color
	;; 	upper 4 bits for background color
	mov [edi], al			; Write the character
	mov byte [edi + 1], bl;0x07	; color
	
	add edi, 2
	
	loop .PrintString32Loop
.PrintString32Done:
	
	popad		; pop all saved 32-bit register from the stack
ret
;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;


; ********************************
; print_fixed_size_string: Fixed Size String Printing Function
; It retains the line number and starts printing from the next line in subsequent calls.
;
; IN: 
;   - ESI: Pointer to String,
;   - ECX: Size of the String
; ******************************** 
print_fixed_size_string:
	pushad
	mov edi, [video_memory]		; 0xB8000

	;; This code helps in starting from the next line every time,
	;; we call this function, but we have to increment the line every time.
	; al = line, initially 0
	; dl = 80*2 	; bytes in a single line
	; al = al * dl 	; offset to the next line
	mov byte al, [cursor_line]
	mov dl, single_line_vmem_bytes	; Number of bytes in a single line 80*2
	mul dl			; multiply with the al
	add word edi, eax	; add the offset in the video_memory base

	.print_next_char:
		mov byte bl, [esi]	; Load first char in bl
		mov byte [edi], bl	; Load char from bl to video memory's first byte
		mov byte [edi + 1], 0x05	; Load the color to the next byte of vMem.
		dec ecx			; Decrement string counter
		cmp ecx, 0		; Check if it was the last char of string.
		jz .done_print		; jump to .done_print, if it was the last char

		add edi, 2	; increment the vmem by 2, point to next box of vMem
		add esi, 1	; point to next char in string.
		jmp .print_next_char	; loop to print the next char
	.done_print:		; it was the end of string.
		inc byte [cursor_line]		; increment the cursor_line and save in variable
	popad
ret
; ********************************


; ********************************
; print_hx = This function prints the hex value of the EAX register
;  - EAX: Hex value to be printed
; ******************************** 
print_hx:
	pusha
	mov edi, 0xb8000
	mov ecx, 8
.hx_loop:
	rol eax, 4
	mov bl, al
	and bl, 0x0F
	cmp bl, 10
	jl .digit
	add bl, 'A' - 10
	jmp .print
.digit:
	add bl, '0'
.print:
	mov byte [edi], bl
	mov byte [edi+1], 0x04
	add edi,2
	loop .hx_loop
	popa
ret

;; Video Memory address of 80*25 (mode 3)
video_memory 	dd 	0xB8000

cursor_line db 0


single_line_vmem_bytes equ 80*2	; Total bytes in a single line in 80*25 (Mode 3)
				; 80 boxes and each box is of 2 bytes


%endif
