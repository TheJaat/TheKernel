#ifndef __IDT_H__
#define __IDT_H__

/* Includes
 * - System */
#include <defs.h>

/* IDT Definitions 
 * In X86 it's possible to have up to 256 interrupt entries
 * we use static allocation for the descriptors */
#define IDT_DESCRIPTORS			256

/* IDT Entry Types
 * These are the possible interrupt gate types, we have:
 * Interrupt-Gates 16/32 Bit - They automatically disable interrupts
 * Trap-Gates 16/32 Bit - They don't disable interrupts 
 * Task-Gates 32 Bit - Hardware Task Switching */
#define IDT_INTERRUPT_GATE16		0x6
#define IDT_INTERRUPT_GATE32		0xE
#define IDT_TRAP_GATE16			0x7
#define IDT_TRAP_GATE32			0xF
#define IDT_TASK_GATE32			0x5

/* IDT Priveliege Types
 * This specifies which ring can use/be interrupt by
 * the idt-entry, we usually specify RING3 */
#define IDT_RING0			0x00
#define IDT_RING1			0x20
#define IDT_RING2			0x40
#define IDT_RING3			0x60

/* IDT Flags
 * Specifies any special attributes about the IDT entry
 * Present must be set for all valid idt-entries */
#define IDT_STORAGE_SEGMENT		0x10
#define IDT_PRESENT			0x80

/* The IDT base structure, this is what the hardware
 * will poin to, that describes the memory range where
 * all the idt-descriptors reside */
typedef struct _IdtObject {
	uint16_t			Limit;
	uint32_t			Base;
} __attribute__((packed)) Idt_t;

/* The IDT descriptor structure, this is the actual entry
 * in the idt table, and keeps information about the 
 * interrupt structure. */
typedef struct _IdtEntry
{
	uint16_t			BaseLow;	/* Base 0:15 */
	uint16_t			Selector;	/* Selector */
	uint8_t				Zero;		/* Reserved */

	/* IDT Entry Flags
	 * Bits 0-3: Descriptor Entry Type
	 * Bits   4: Storage Segment
	 * Bits 5-6: Priveliege Level
	 * Bits   7: Present */
	uint8_t				Flags;
	uint16_t			BaseHigh;	/* Base 16:31 */
} __attribute__((packed)) IdtEntry_t;

/* Initialize the idt table with the 256 default
 * descriptors for entering shared interrupt handlers
 * and shared exception handlers */
#ifdef __cplusplus
extern "C" {
#endif

void IdtInitialize(void);
/* This installs the current idt-object in the
 * idt register for the calling cpu, use to setup idt */
void IdtInstall(void);

#ifdef __cplusplus
}
#endif

/* Extern to the syscall-handler */
extern void syscall_entry(void);

/* Irq-Handlers, all extenr assembly
 * that point to shared entry handlers */
extern void irq_handler0(void);
extern void irq_handler1(void);
extern void irq_handler2(void);
extern void irq_handler3(void);
extern void irq_handler4(void);
extern void irq_handler5(void);
extern void irq_handler6(void);
extern void irq_handler7(void);
extern void irq_handler8(void);
extern void irq_handler9(void);
extern void irq_handler10(void);
extern void irq_handler11(void);
extern void irq_handler12(void);
extern void irq_handler13(void);
extern void irq_handler14(void);
extern void irq_handler15(void);
extern void irq_handler16(void);
extern void irq_handler17(void);
extern void irq_handler18(void);
extern void irq_handler19(void);
extern void irq_handler20(void);
extern void irq_handler21(void);
extern void irq_handler22(void);
extern void irq_handler23(void);
extern void irq_handler24(void);
extern void irq_handler25(void);
extern void irq_handler26(void);
extern void irq_handler27(void);
extern void irq_handler28(void);
extern void irq_handler29(void);
extern void irq_handler30(void);
extern void irq_handler31(void);
extern void irq_handler32(void); 
extern void irq_handler33(void);
extern void irq_handler34(void);
extern void irq_handler35(void);
extern void irq_handler36(void);
extern void irq_handler37(void);
extern void irq_handler38(void);
extern void irq_handler39(void);
extern void irq_handler40(void);
extern void irq_handler41(void);
extern void irq_handler42(void);
extern void irq_handler43(void);
extern void irq_handler44(void);
extern void irq_handler45(void);
extern void irq_handler46(void);
extern void irq_handler47(void);
extern void irq_handler48(void);
extern void irq_handler49(void);
extern void irq_handler50(void);
extern void irq_handler51(void);
extern void irq_handler52(void);
extern void irq_handler53(void);
extern void irq_handler54(void);
extern void irq_handler55(void);
extern void irq_handler56(void);
extern void irq_handler57(void);
extern void irq_handler58(void);
extern void irq_handler59(void);
extern void irq_handler60(void);
extern void irq_handler61(void);
extern void irq_handler62(void);
extern void irq_handler63(void);
extern void irq_handler64(void);
extern void irq_handler65(void);
extern void irq_handler66(void);
extern void irq_handler67(void);
extern void irq_handler68(void);
extern void irq_handler69(void);
extern void irq_handler70(void);
extern void irq_handler71(void);
extern void irq_handler72(void);
extern void irq_handler73(void);
extern void irq_handler74(void);
extern void irq_handler75(void);
extern void irq_handler76(void);
extern void irq_handler77(void);
extern void irq_handler78(void);
extern void irq_handler79(void);
extern void irq_handler80(void);
extern void irq_handler81(void);
extern void irq_handler82(void);
extern void irq_handler83(void);
extern void irq_handler84(void);
extern void irq_handler85(void);
extern void irq_handler86(void);
extern void irq_handler87(void);
extern void irq_handler88(void);
extern void irq_handler89(void);
extern void irq_handler90(void);
extern void irq_handler91(void);
extern void irq_handler92(void);
extern void irq_handler93(void);
extern void irq_handler94(void);
extern void irq_handler95(void);
extern void irq_handler96(void);
extern void irq_handler97(void);
extern void irq_handler98(void);
extern void irq_handler99(void);
extern void irq_handler100(void);
extern void irq_handler101(void);
extern void irq_handler102(void);
extern void irq_handler103(void);
extern void irq_handler104(void);
extern void irq_handler105(void);
extern void irq_handler106(void);
extern void irq_handler107(void);
extern void irq_handler108(void);
extern void irq_handler109(void);
extern void irq_handler110(void);
extern void irq_handler111(void);
extern void irq_handler112(void);
extern void irq_handler113(void);
extern void irq_handler114(void);
extern void irq_handler115(void);
extern void irq_handler116(void);
extern void irq_handler117(void);
extern void irq_handler118(void);
extern void irq_handler119(void);
extern void irq_handler120(void);
extern void irq_handler121(void);
extern void irq_handler122(void);
extern void irq_handler123(void);
extern void irq_handler124(void);
extern void irq_handler125(void);
extern void irq_handler126(void);
extern void irq_handler127(void);
extern void irq_handler128(void);
extern void irq_handler129(void);
extern void irq_handler130(void);
extern void irq_handler131(void);
extern void irq_handler132(void);
extern void irq_handler133(void);
extern void irq_handler134(void);
extern void irq_handler135(void);
extern void irq_handler136(void);
extern void irq_handler137(void);
extern void irq_handler138(void);
extern void irq_handler139(void);
extern void irq_handler140(void);
extern void irq_handler141(void);
extern void irq_handler142(void);
extern void irq_handler143(void);
extern void irq_handler144(void);
extern void irq_handler145(void);
extern void irq_handler146(void);
extern void irq_handler147(void);
extern void irq_handler148(void);
extern void irq_handler149(void);
extern void irq_handler150(void);
extern void irq_handler151(void);
extern void irq_handler152(void);
extern void irq_handler153(void);
extern void irq_handler154(void);
extern void irq_handler155(void);
extern void irq_handler156(void);
extern void irq_handler157(void);
extern void irq_handler158(void);
extern void irq_handler159(void);
extern void irq_handler160(void);
extern void irq_handler161(void);
extern void irq_handler162(void);
extern void irq_handler163(void);
extern void irq_handler164(void);
extern void irq_handler165(void);
extern void irq_handler166(void);
extern void irq_handler167(void);
extern void irq_handler168(void);
extern void irq_handler169(void);
extern void irq_handler170(void);
extern void irq_handler171(void);
extern void irq_handler172(void);
extern void irq_handler173(void);
extern void irq_handler174(void);
extern void irq_handler175(void);
extern void irq_handler176(void);
extern void irq_handler177(void);
extern void irq_handler178(void);
extern void irq_handler179(void);
extern void irq_handler180(void);
extern void irq_handler181(void);
extern void irq_handler182(void);
extern void irq_handler183(void);
extern void irq_handler184(void);
extern void irq_handler185(void);
extern void irq_handler186(void);
extern void irq_handler187(void);
extern void irq_handler188(void);
extern void irq_handler189(void);
extern void irq_handler190(void);
extern void irq_handler191(void);
extern void irq_handler192(void);
extern void irq_handler193(void);
extern void irq_handler194(void);
extern void irq_handler195(void);
extern void irq_handler196(void);
extern void irq_handler197(void);
extern void irq_handler198(void);
extern void irq_handler199(void);
extern void irq_handler200(void);
extern void irq_handler201(void);
extern void irq_handler202(void);
extern void irq_handler203(void);
extern void irq_handler204(void);
extern void irq_handler205(void);
extern void irq_handler206(void);
extern void irq_handler207(void);
extern void irq_handler208(void);
extern void irq_handler209(void);
extern void irq_handler210(void);
extern void irq_handler211(void);
extern void irq_handler212(void);
extern void irq_handler213(void);
extern void irq_handler214(void);
extern void irq_handler215(void);
extern void irq_handler216(void);
extern void irq_handler217(void);
extern void irq_handler218(void);
extern void irq_handler219(void);
extern void irq_handler220(void);
extern void irq_handler221(void);
extern void irq_handler222(void);
extern void irq_handler223(void);
extern void irq_handler224(void);
extern void irq_handler225(void);
extern void irq_handler226(void);
extern void irq_handler227(void);
extern void irq_handler228(void);
extern void irq_handler229(void);
extern void irq_handler230(void);
extern void irq_handler231(void);
extern void irq_handler232(void);
extern void irq_handler233(void);
extern void irq_handler234(void);
extern void irq_handler235(void);
extern void irq_handler236(void);
extern void irq_handler237(void);
extern void irq_handler238(void);
extern void irq_handler239(void);
extern void irq_handler240(void);
extern void irq_handler241(void);
extern void irq_handler242(void);
extern void irq_handler243(void);
extern void irq_handler244(void);
extern void irq_handler245(void);
extern void irq_handler246(void);
extern void irq_handler247(void);
extern void irq_handler248(void);
extern void irq_handler249(void);
extern void irq_handler250(void);
extern void irq_handler251(void);
extern void irq_handler252(void);
extern void irq_handler253(void);
extern void irq_handler254(void);
extern void irq_handler255(void);

#endif /* __IDT_H__ */

