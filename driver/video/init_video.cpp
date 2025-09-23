#include <terminal/terminal.h>
#include <video/interface/video_driver_interface.h>
#include <boot/datastructure.h>
#include <video/vbe.h>
#include <video/vga.h>

#include <video/interface/video_interface.h>

extern VideoDriver VBE_DRIVER;
extern VideoDriver VGA_DRIVER;

Terminal BootTerminal;
VbeContext s_vbeContext;

Terminal* VideoGetTerminal(void) {
    return &BootTerminal;
}

void InitializeVideo(Multiboot_t* BootInfo) {
    
    VideoDriver* driver;
    int width = 80, height = 25;

    if (BootInfo->VbeMode == 0 || BootInfo->VbeMode == 1) {
        // Test it by commenting two lines in the stage2.asm
        // Note: Comment Both because VbeMode is set in the VesaSetup function
        /*
            call VesaSetup
            &&
            call VesaFinish
        */
        static VgaContext s_vgaContext;
        s_vgaContext.mode.frameBufferAddress = 0xb8000;
        s_vgaContext.mode.width = 80;
        int vgaHeight = (BootInfo->VbeMode == 1) ? 50 : 25;
        s_vgaContext.mode.height = vgaHeight;
        height = vgaHeight;
        s_vgaContext.mode.fgColor = 0xF0;
        s_vgaContext.mode.bgColor = 0;

        VGA_DRIVER.context = &s_vgaContext;
        BootTerminal.videoModeType = VIDEO_TEXT;
        driver = &VGA_DRIVER;
    } else {
        VbeMode_t* vbe = (VbeMode_t*)BootInfo->VbeModeInfo;

        // static VbeContext s_vbeContext;
        s_vbeContext.mode = *vbe;

        VBE_DRIVER.context = &s_vbeContext;
        BootTerminal.videoModeType = VIDEO_GRAPHICS;

        driver = &VBE_DRIVER;
        width = vbe->XResolution;
        height = vbe->YResolution;
    }

    TerminalInit(&BootTerminal, driver, width, height);
    TerminalClear(&BootTerminal);
    // TerminalDrawPixel(&BootTerminal, 0, 0, 0x00ff0000);
}
