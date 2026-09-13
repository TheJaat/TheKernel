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

/* SyscallsFindNamedPipe
 * Kernel-side lookup in the service registry. Lets in-kernel code be a
 * client of a ring-3 server - which is how the shell reads keystrokes
 * from the userspace keyboard driver. */
void      *SyscallsFindNamedPipe(const char *Name);

#ifdef __cplusplus
}
#endif

#endif /* __KERNEL_SYSCALLS_H__ */
