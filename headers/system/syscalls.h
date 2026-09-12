#ifndef __KERNEL_SYSCALLS_H__
#define __KERNEL_SYSCALLS_H__

#include <defs.h>
#include <stddef.h>

/* The syscall numbers themselves live in shared/os/syscalls.h, which is
 * compiled into both the kernel and every user program. There is exactly
 * one copy so the two cannot drift. */
#include <os/syscalls.h>

#ifdef __cplusplus
extern "C" {
#endif

OsStatus_t SyscallsInitialize(void);
size_t     SyscallsGetCount(void);
void       SyscallsPrintNames(void);

#ifdef __cplusplus
}
#endif

#endif /* __KERNEL_SYSCALLS_H__ */
