#include <terminal/terminal.h>
#include <video/interface/video_driver_interface.h>
#include <boot/datastructure.h>
#include <video/vbe.h>
#include <video/vga.h>

extern VideoDriver VBE_DRIVER;
extern VideoDriver VGA_DRIVER;

Terminal BootTerminal;

void InitializeVideo(Multiboot_t* BootInfo) {
    
    VideoDriver* driver;
    int width = 80, height = 25;

    if (BootInfo->VbeMode == 0 || BootInfo->VbeMode == 1) {
        driver = &VGA_DRIVER;
        height = (BootInfo->VbeMode == 1) ? 50 : 25;
    } else {
        VbeMode_t* vbe = (VbeMode_t*)BootInfo->VbeModeInfo;

        static VbeContext s_vbeContext;
        s_vbeContext.mode = *vbe;

        VBE_DRIVER.context = &s_vbeContext;

        driver = &VBE_DRIVER;
        width = vbe->XResolution;
        height = vbe->YResolution;
    }

    TerminalInit(&BootTerminal, driver, width, height);
    TerminalClear(&BootTerminal);
    // TerminalDrawPixel(&BootTerminal, 0, 0, 0x00ff0000);
}
