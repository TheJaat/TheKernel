#ifndef __SHELL_H__
#define __SHELL_H__

#include <defs.h>
#include <stddef.h>
#include <system/pipe.h>

/* Longest line the editor will accept. Anything beyond this is refused
 * with a beep rather than silently truncated - a command that quietly
 * loses its tail is worse than one that will not run. */
#define SHELL_LINE_MAX              128

#ifdef __cplusplus
extern "C" {
#endif

/* ShellStart
 * Spawns the shell thread, reading from <Input>. Requires threading and
 * a keystroke pipe. */
OsStatus_t ShellStart(Pipe_t *Input);

#ifdef __cplusplus
}
#endif

#endif /* __SHELL_H__ */