#ifndef __SYSTEM_H
#define __SYSTEM_H

/*

Include Guard
The include guard ensures that the contents of the header file are only included once during compilation, preventing multiple definition errors.

*/


/*
They are declared as extern to indicate that their definitions are provided elsewhere.

*/

/* IDT */
extern void idt_install();
extern void idt_set_gate(unsigned char num, unsigned long base, unsigned short sel, unsigned char flags);


/* ISRS */
extern void isrs_install();

/* Registers */
struct regs {
    unsigned int gs, fs, es, ds;
    unsigned int edi, esi, ebp, esp, ebx, edx, ecx, eax;
    unsigned int int_no, err_code;
    unsigned int eip, cs, eflags, useresp, ss;
};


#endif
