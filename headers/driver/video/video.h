#ifndef _VIDEO_H
#define _VIDEO_H

#include <video/interface/video_driver_interface.h>
#include <terminal/terminal.h>
#include <boot/datastructure.h>
#include <video/vbe.h>
#include <video/vga.h>

extern Terminal BootTerminal;

void InitializeVideo(Multiboot_t* BootInfo);

#endif // _VIDEO_H