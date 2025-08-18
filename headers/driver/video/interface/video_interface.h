#ifndef VIDEO_INTERFACE_H
#define VIDEO_INTERFACE_H

#include <terminal/terminal.h>

#ifdef __cplusplus
extern "C" {
#endif

// Declare the global terminal
extern Terminal BootTerminal;

// Getter for the terminal
Terminal* VideoGetTerminal(void);

#ifdef __cplusplus
}
#endif

#endif
