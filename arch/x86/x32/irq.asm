bits 32
segment .text

;Functions in this asm
global exception_common
global irq_common
global syscall_entry
global ___cli
global ___sti
global ___hlt
global ___getflags
global ___getcr2

;Externs, common entry points
extern ExceptionEntry
extern InterruptEntry

; void __cli(void)
; Disables interrupts
___cli:
	; Disable interrupts
	cli

	; Return
	ret 

; void __sti(void)
; Enables interrupts
___sti:
	; Enable interrupts
	sti

	; Return
	ret 

; void __hlt(void)
; Enables interrupts
___hlt:
	; Idle
	hlt

	; Return
	ret 

; uint32_t __getflags(void)
; Gets Eflags
___getflags:
	; Get flags
	pushfd
	pop eax

	; Return
	ret 

; uint32_t __getcr2(void)
; Gets CR2 register
___getcr2:
	; Get cr2
	mov eax, cr2

	; Return
	ret 

; Common entry point for exceptions
exception_common:
	
	; Save Segments
	push ds
	push es
	push fs
	push gs

	; Save Registers
	pushad

	; Switch to kernel segment
	mov ax, 0x10
	mov ds, ax
	mov es, ax
	mov fs, ax
	mov gs, ax

	; Push registers as pointer to struct
	push esp

	; Call C-code exception handler, defined in interrupts/interrupts.c
	call ExceptionEntry

	; Cleanup
	add esp, 0x4

	; _IF_ we return, restore state
	popad

	; Restore segments
	pop gs
	pop fs
	pop es
	pop ds

	; Cleanup IrqNum & IrqErrorCode from stack
	add esp, 0x8

	; Return
	iret

;Common entry point for interrupts
irq_common:
	
	; Save Segments
	push ds
	push es
	push fs
	push gs

	; Save Registers
	pushad

	; Switch to kernel segment
	mov ax, 0x10
	mov ds, ax
	mov es, ax
	mov fs, ax
	mov gs, ax

	; Push registers as pointer to struct
	push esp

	; Call C-code exception handler, defined in interrupts/interrupts.c
	call InterruptEntry

	; Cleanup
	add esp, 0x4

	; When we return, restore state
	popad

	; Restore segments
	pop gs
	pop fs
	pop es
	pop ds

	; Cleanup IrqNum & IrqErrorCode from stack
	add esp, 0x8

	; Return
	iret

; Macros

; Exception with no error code
%macro irq_no_error 1
	global irq_handler%1
	irq_handler%1:
		push 0
		push %1
		jmp exception_common
%endmacro

%macro irq_error 1
	global irq_handler%1
	irq_handler%1:
		push %1
		jmp exception_common
%endmacro

%macro irq_normal 2
	global irq_handler%1
	irq_handler%1:
		push 0
		push %2
		jmp irq_common
%endmacro

;Define excetions!
irq_no_error 0
irq_no_error 1
irq_no_error 2
irq_no_error 3
irq_no_error 4
irq_no_error 5
irq_no_error 6
irq_no_error 7
irq_error    8
irq_no_error 9
irq_error    10
irq_error    11
irq_error    12
irq_error    13
irq_error    14
irq_no_error 15
irq_no_error 16
irq_error    17
irq_no_error 18
irq_no_error 19
irq_no_error 20
irq_no_error 21
irq_no_error 22
irq_no_error 23
irq_no_error 24
irq_no_error 25
irq_no_error 26
irq_no_error 27
irq_no_error 28
irq_no_error 29
irq_no_error 30
irq_no_error 31

;Base IRQs 0 - 15
irq_normal 32, 0
irq_normal 33, 1
irq_normal 34, 2
irq_normal 35, 3
irq_normal 36, 4
irq_normal 37, 5
irq_normal 38, 6
irq_normal 39, 7
irq_normal 40, 8
irq_normal 41, 9
irq_normal 42, 10
irq_normal 43, 11
irq_normal 44, 12
irq_normal 45, 13
irq_normal 46, 14
irq_normal 47, 15
irq_normal 48, 16
irq_normal 49, 17
irq_normal 50, 18
irq_normal 51, 19
irq_normal 52, 20
irq_normal 53, 21
irq_normal 54, 22
irq_normal 55, 23
irq_normal 56, 24
irq_normal 57, 25
irq_normal 58, 26
irq_normal 59, 27
irq_normal 60, 28
irq_normal 61, 29
irq_normal 62, 30
irq_normal 63, 31
irq_normal 64, 32
irq_normal 65, 33
irq_normal 66, 34
irq_normal 67, 35
irq_normal 68, 36
irq_normal 69, 37
irq_normal 70, 38
irq_normal 71, 39
irq_normal 72, 40
irq_normal 73, 41
irq_normal 74, 42
irq_normal 75, 43
irq_normal 76, 44
irq_normal 77, 45
irq_normal 78, 46
irq_normal 79, 47
irq_normal 80, 48
irq_normal 81, 49
irq_normal 82, 50
irq_normal 83, 51
irq_normal 84, 52
irq_normal 85, 53
irq_normal 86, 54
irq_normal 87, 55
irq_normal 88, 56
irq_normal 89, 57
irq_normal 90, 58
irq_normal 91, 58
irq_normal 92, 59
irq_normal 93, 60
irq_normal 94, 61
irq_normal 95, 62
irq_normal 96, 63
irq_normal 97, 64
irq_normal 98, 65
irq_normal 99, 66
irq_normal 100, 67
irq_normal 101, 67
irq_normal 102, 68
irq_normal 103, 69
irq_normal 104, 70
irq_normal 105, 71
irq_normal 106, 72
irq_normal 107, 73
irq_normal 108, 74
irq_normal 109, 75
irq_normal 110, 76
irq_normal 111, 77
irq_normal 112, 78
irq_normal 113, 79
irq_normal 114, 80
irq_normal 115, 81
irq_normal 116, 82
irq_normal 117, 83
irq_normal 118, 84
irq_normal 119, 85
irq_normal 120, 86
irq_normal 121, 87
irq_normal 122, 88
irq_normal 123, 89
irq_normal 124, 90
irq_normal 125, 91
irq_normal 126, 92
irq_normal 127, 95
irq_normal 128, 96
irq_normal 129, 97
irq_normal 130, 98
irq_normal 131, 99
irq_normal 132, 100
irq_normal 133, 101
irq_normal 134, 102
irq_normal 135, 103
irq_normal 136, 104
irq_normal 137, 105
irq_normal 138, 106
irq_normal 139, 107
irq_normal 140, 108
irq_normal 141, 109
irq_normal 142, 110
irq_normal 143, 111
irq_normal 144, 112
irq_normal 145, 113
irq_normal 146, 114
irq_normal 147, 115
irq_normal 148, 116
irq_normal 149, 117
irq_normal 150, 118
irq_normal 151, 119
irq_normal 152, 120
irq_normal 153, 121
irq_normal 154, 122
irq_normal 155, 123
irq_normal 156, 124
irq_normal 157, 125
irq_normal 158, 126
irq_normal 159, 127
irq_normal 160, 128
irq_normal 161, 129
irq_normal 162, 130
irq_normal 163, 131
irq_normal 164, 132
irq_normal 165, 133
irq_normal 166, 134
irq_normal 167, 135
irq_normal 168, 136
irq_normal 169, 137
irq_normal 170, 138
irq_normal 171, 139
irq_normal 172, 140
irq_normal 173, 141
irq_normal 174, 142
irq_normal 175, 143
irq_normal 176, 144
irq_normal 177, 145
irq_normal 178, 146
irq_normal 179, 147
irq_normal 180, 148
irq_normal 181, 149
irq_normal 182, 150
irq_normal 183, 151
irq_normal 184, 152
irq_normal 185, 153
irq_normal 186, 154
irq_normal 187, 155
irq_normal 188, 156
irq_normal 189, 157
irq_normal 190, 158
irq_normal 191, 159
irq_normal 192, 160
irq_normal 193, 161
irq_normal 194, 162
irq_normal 195, 163
irq_normal 196, 164
irq_normal 197, 165
irq_normal 198, 166
irq_normal 199, 167
irq_normal 200, 168
irq_normal 201, 169
irq_normal 202, 170
irq_normal 203, 171
irq_normal 204, 172
irq_normal 205, 173
irq_normal 206, 174
irq_normal 207, 175
irq_normal 208, 176
irq_normal 209, 177
irq_normal 210, 178
irq_normal 211, 179
irq_normal 212, 180
irq_normal 213, 181
irq_normal 214, 182
irq_normal 215, 183
irq_normal 216, 184
irq_normal 217, 185
irq_normal 218, 186
irq_normal 219, 187
irq_normal 220, 188
irq_normal 221, 189
irq_normal 222, 190
irq_normal 223, 191
irq_normal 224, 192
irq_normal 225, 193
irq_normal 226, 194
irq_normal 227, 195
irq_normal 228, 196
irq_normal 229, 197
irq_normal 230, 198
irq_normal 231, 199
irq_normal 232, 200
irq_normal 233, 201
irq_normal 234, 202
irq_normal 235, 203
irq_normal 236, 204
irq_normal 237, 205
irq_normal 238, 206
irq_normal 239, 207
irq_normal 240, 208
irq_normal 241, 209
irq_normal 242, 210
irq_normal 243, 211
irq_normal 244, 212
irq_normal 245, 213
irq_normal 246, 214
irq_normal 247, 215
irq_normal 248, 216
irq_normal 249, 217
irq_normal 250, 218
irq_normal 251, 219
irq_normal 252, 220
irq_normal 253, 221
irq_normal 254, 222
irq_normal 255, 223
