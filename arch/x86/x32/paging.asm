bits 32
segment .text

;Functions in this asm
global memory_set_paging
global memory_reload_cr3
global memory_get_cr3
global memory_load_cr3
global memory_invalidate_addr

;void memory_set_paging(int enable)
;Either enables or disables paging
memory_set_paging:
	; Stack Frame
	push ebp
	mov ebp, esp

	; Save stuff
	push eax

	; Get [enable]
	mov	eax, dword [ebp + 8]
	cmp eax, 0
	je	.disable

	; Enable
	mov eax, cr0
	or eax, 0x80000000		; Set bit 31
	mov	cr0, eax
	jmp .done
	
	.disable:
		mov eax, cr0
		and eax, 0x7FFFFFFF		; Clear bit 31
		mov	cr0, eax

	.done:
		; Release stack frame
		pop eax
		pop ebp
		ret 


;void memory_reload_cr3(void)
;Reloads the cr3 register
memory_reload_cr3:
	; Save stuff
	push eax

	; Reload
	mov eax, cr3
	mov cr3, eax

	; Restore
	pop eax
	ret 

;uint32_t memory_get_cr3(void)
;Returns the cr3 register
memory_get_cr3:
	; Get
	mov eax, cr3

	; Return
	ret 

;void memory_load_cr3(uintptr_t pda)
;Loads the cr3 register
memory_load_cr3:
	; Stack Frame
	push ebp
	mov ebp, esp

	; Save EAX
	push eax

	; Reload
	mov	eax, dword [ebp + 8]
	mov cr3, eax

	; Restore
	pop eax

	; Release stack frame
	pop ebp
	ret 

;void memory_invalidate_addr(uintptr_t pda)
;Invalidates a page address
memory_invalidate_addr:
	; Stack Frame
	push ebp
	mov ebp, esp

	; Reload
	invlpg [ebp + 8]

	; Release stack frame
	pop ebp
	ret 
