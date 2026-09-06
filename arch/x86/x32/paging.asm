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
	push ebp
	mov ebp, esp
	push eax

	mov	eax, dword [ebp + 8]
	cmp eax, 0
	je	.disable

	mov eax, cr0
	or eax, 0x80000000		; Set bit 31 (PG)
	mov	cr0, eax
	jmp .done

	.disable:
		mov eax, cr0
		and eax, 0x7FFFFFFF	; Clear bit 31
		mov	cr0, eax

	.done:
		pop eax
		pop ebp
		ret


;void memory_reload_cr3(void)
;Reloads the cr3 register
memory_reload_cr3:
	push eax
	mov eax, cr3
	mov cr3, eax
	pop eax
	ret

;uint32_t memory_get_cr3(void)
;Returns the cr3 register
memory_get_cr3:
	mov eax, cr3
	ret

;void memory_load_cr3(uintptr_t pda)
;Loads the cr3 register
memory_load_cr3:
	push ebp
	mov ebp, esp
	push eax

	mov	eax, dword [ebp + 8]
	mov cr3, eax

	pop eax
	pop ebp
	ret

;void memory_invalidate_addr(uintptr_t Address)
;Invalidates a single page in the TLB.
;
;The previous version was `invlpg [ebp + 8]`, which flushes the TLB entry
;for the *stack slot holding the argument*, not for the address that was
;passed in. The argument has to be loaded into a register first.
memory_invalidate_addr:
	push ebp
	mov ebp, esp
	push eax

	mov eax, dword [ebp + 8]
	invlpg [eax]

	pop eax
	pop ebp
	ret