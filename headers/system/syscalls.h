#ifndef __SYSCALLS_H__
#define __SYSCALLS_H__

#include <defs.h>
#include <stddef.h>

/* The syscall vector. IDT entry 128 is already installed as an
 * IDT_RING3 gate, which is what lets ring 3 raise it at all - a ring-0
 * gate would deliver a general protection fault instead. */
#define SYSCALL_VECTOR          128

/* Numbers are ABI: user binaries are compiled against them, so they may
 * be appended to but never reordered. */
#define SYS_EXIT                0
#define SYS_WRITE               1
#define SYS_SLEEP               2
#define SYS_GETMS               3
#define SYS_GETTID              4
#define SYS_MAX                 5

#ifdef __cplusplus
extern "C" {
#endif

/* SyscallsInitialize
 * Registers the handler on the syscall vector. */
OsStatus_t SyscallsInitialize(void);

/* SyscallsGetCount */
size_t SyscallsGetCount(void);

#ifdef __cplusplus
}
#endif

#endif /* __SYSCALLS_H__ */