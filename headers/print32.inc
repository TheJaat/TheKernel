%ifndef _PRINT_32_INC_
%define _PRINT_32_INC_

BITS 32

%define VGA_DEFAULT_MEM_ADDR	0xb8000

%define VGA_ROW		80
%define VGA_COLUMN	25

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
	mov ecx, VGA_ROW * VGA_COLUMN
.ClearScreen32Loop:
	mov byte [esi], ' '		; Space
	mov al, bh
	shl al, 4
	or al, bl
	mov byte [esi + 1], al	; default color - lightgrey on black
	
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
    pushad                      ; Save all 32-bit registers onto the stack (preserving state)
    
    mov ecx, VGA_ROW * VGA_COLUMN    ; Set ECX to number of character cells in VGA text mode (80x25 = 2000 cells)
    mov edi, VGA_DEFAULT_MEM_ADDR   ; Set EDI to the starting address of VGA memory (0xB8000)

    ; Combine background and foreground colors from BL and BH
    shl bh, 4                       ; Shift the background color in BH to the upper 4 bits
    or bl, bh                        ; Combine foreground color (BL) with background color (upper 4 bits from BH)
    
.PrintString32Loop:
    lodsb                           ; Load the next byte (character) from the source string (ESI) into AL
    test al, al                     ; Test if the character is NULL (end of string)
    je .PrintString32Done           ; If it's NULL, jump to the done label

    ; Write the character to the VGA memory
    mov [edi], al                   ; Store the character (AL) at the current VGA memory address (EDI)
    mov byte [edi + 1], bl          ; Store the color byte (BL: combined foreground and background) at the next byte

    add edi, 2                      ; Move the VGA memory pointer to the next character cell (each cell is 2 bytes)

    loop .PrintString32Loop         ; Repeat the loop for each character until ECX is decremented to zero

.PrintString32Done:
    popad                           ; Restore the state of all 32-bit registers by popping from the stack
    ret                             ; Return from the function

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
