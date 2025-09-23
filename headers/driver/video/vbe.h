#ifndef __VBE_H__
#define __VBE_H__

#include <stdint.h>
#include <stddef.h>
#include <defs.h>


// Font stuff defined in font8x16.c file.
// extern const uint8_t FontBitmaps[];
// extern const uint32_t FontNumChars;
// extern const uint32_t FontHeight;
// extern const uint32_t FontWidth;

/* Video Type Definitions
 *  */
#define VIDEO_NONE		    0x00000000
#define VIDEO_TEXT		    0x00000001
#define VIDEO_GRAPHICS		0x00000002

/* Definitions 
 * VGA Memory Address
 */
#define STD_VIDEO_MEMORY	0xB8000

/* This is the VBE Graphic Information
 * Descriptor which we have setup in
 * the bootloader */
typedef struct VbeMode {
	uint16_t        ModeAttributes;
	uint8_t         WinAAttributes;
	uint8_t         WinBAttributes;
	uint16_t        WinGranularity;
	uint16_t        WinSize;
	uint16_t        WinASegment;
	uint16_t        WinBSegment;
	uint32_t        WinFuncPtr;
	uint16_t        BytesPerScanLine;
	uint16_t        XResolution;
	uint16_t        YResolution;
	uint8_t         XCharSize;
	uint8_t         YCharSize;
	uint8_t         NumberOfPlanes;
	uint8_t         BitsPerPixel;
	uint8_t         NumberOfBanks;
	uint8_t         MemoryModel;
	uint8_t         BankSize;
	uint8_t         NumberOfImagePages;
	uint8_t         Reserved_page;
	uint8_t         RedMaskSize;
	uint8_t         RedMaskPos;
	uint8_t         GreenMaskSize;
	uint8_t         GreenMaskPos;
	uint8_t         BlueMaskSize;
	uint8_t         BlueMaskPos;
	uint8_t         ReservedMaskSize;
	uint8_t         ReservedMaskPos;
	uint8_t         DirectColorModeInfo;

	/* VBE 2.0 Extensions */
	uint32_t        PhysBasePtr;
	uint32_t        OffScreenMemOffset;
	uint16_t        OffScreenMemSize;
} __attribute__((packed)) VbeMode_t;

typedef struct {
    VbeMode_t mode;
} VbeContext;

extern VbeContext s_vbeContext;


#ifdef __cplusplus
extern "C" {
#endif


void set_vbe_mode(const VbeMode_t* mode);

// Update the frame buffer address.
void updatePhysBasePtr(uint32_t virtualPhysBasePtr);

void VesaClearScreen(VbeContext* ctx, uint32_t color);

OsStatus_t VesaDrawPixel(VbeContext* ctx, unsigned X, unsigned Y, uint32_t Color);
OsStatus_t VesaDrawCharacter(VbeContext* ctx, unsigned CursorX, unsigned CursorY, int Character, uint32_t FgColor, uint32_t BgColor);
void VesaScroll(VbeContext* ctx, int ByLines, uint32_t bg);

#ifdef __cplusplus
}
#endif

#endif /* __VBE_H__ */