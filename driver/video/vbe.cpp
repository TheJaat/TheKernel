#include <stdint.h>
#include <stddef.h>
#include <video/vbe.h>
#include <boot/datastructure.h>
#include <defs.h>
#include <terminal/font8x16.h>

// static VbeMode_t s_vbeMode;

/*
* static global variables = persists the value within this 
* file only. Unlike to global variables its scope is limited to this file only.
* It is not accessible from other files in the same program.
* In order to access it in other fies, we can have getter function()
* which returns the reference to this static global variable.
* 
* Note: Unlike global variables it would not be accessible even with by using extern keyword.
*/

/* set_vbe_mode
 * Sets the s_vbeMode from the boot_terminal
 */
void set_vbe_mode(const VbeMode_t* mode) {
	// s_vbeMode = *mode;
}

// Updates the frame buffer address (used when virtual memory is implemented)
void updatePhysBasePtr(uint32_t virtualPhysBasePtr) {
	// s_vbeMode.PhysBasePtr = virtualPhysBasePtr;
	s_vbeContext.mode.PhysBasePtr = virtualPhysBasePtr;
	// ((VbeContext*)(VideoGetTerminal()->driver->context))->mode.PhysBasePtr
}

void VesaClearScreen(VbeContext* ctx, uint32_t color) {
	uint32_t* framebuffer = (uint32_t*)(uintptr_t)ctx->mode.PhysBasePtr;
    uint32_t width = ctx->mode.XResolution;
    uint32_t height = ctx->mode.YResolution;
    uint32_t total_pixels = width * height;

    for (uint32_t i = 0; i < total_pixels; ++i) {
        framebuffer[i] = color;
    }
}

/* VesaDrawPixel
 * Uses the vesa-interface to plot a single pixel
 */
OsStatus_t VesaDrawPixel(VbeContext* ctx, unsigned X, unsigned Y, uint32_t Color) {
	// Variables
	uint32_t *VideoPtr = NULL;

	// Calculate the video-offset
	VideoPtr = (uint32_t*)
		(ctx->mode.PhysBasePtr 
			+ ((Y * ctx->mode.BytesPerScanLine)
		+ (X * (ctx->mode.BitsPerPixel / 8))));
	
	// Set the pixel
	(*VideoPtr) = (0xFF000000 | Color);

	// No error
	return Success;
}


/* VesaDrawCharacter
 * Renders a ASCII/UTF16 character at the given pixel-position
 * on the screen
 */
OsStatus_t VesaDrawCharacter(VbeContext* ctx, unsigned CursorX, unsigned CursorY, int Character, uint32_t FgColor, uint32_t BgColor) {
	// Variables
	uint32_t *vPtr = NULL;
	uint8_t *ChPtr = NULL;
	unsigned Row, i = (unsigned)Character;

	// Calculate the video-offset
	vPtr = (uint32_t*)(ctx->mode.PhysBasePtr 
		+ ((CursorY * ctx->mode.BytesPerScanLine)
		+ (CursorX * (ctx->mode.BitsPerPixel / 8))));


	// Lookup bitmap
	ChPtr = (uint8_t*)&FontBitmaps[i * FontHeight];

	// Iterate bitmap rows
	for (Row = 0; Row < FontHeight; Row++) {
		uint8_t BmpData = ChPtr[Row];
		uint32_t offset;

		// Render data in row
		for (i = 0; i < 8; i++) {
			vPtr[i] = (BmpData >> (7 - i)) & 0x1 ? (0xFF000000 | FgColor) : (0xFF000000 | BgColor);
		}

		// Increase the memory pointer by row
		offset = (uint32_t)vPtr;
		offset += ctx->mode.BytesPerScanLine;
		vPtr = (uint32_t*)offset;
	}

	// Done - no errors
	return Success;
}

void VesaScroll(VbeContext* ctx, int pixelLines, uint32_t background) {
    VbeContext* vbe = (VbeContext*)ctx;

    if (pixelLines <= 0 || pixelLines >= vbe->mode.YResolution) {
        // Clear screen if too many lines requested
        uint32_t* fb = (uint32_t*)(uintptr_t)vbe->mode.PhysBasePtr;
        size_t totalPixels = (size_t)vbe->mode.XResolution * vbe->mode.YResolution;
        for (size_t i = 0; i < totalPixels; i++) {
            fb[i] = background;
        }
        return;
    }

    size_t bytesPerPixel = vbe->mode.BitsPerPixel / 8;
    size_t bytesPerScanline = vbe->mode.BytesPerScanLine;
    size_t copyBytesPerRow = vbe->mode.XResolution * bytesPerPixel;

    uint8_t* fbBase = (uint8_t*)(uintptr_t)vbe->mode.PhysBasePtr;

    // Copy scanlines upward
    for (int row = 0; row < (int)vbe->mode.YResolution - pixelLines; row++) {
        uint8_t* dst = fbBase + (row * bytesPerScanline);
        uint8_t* src = fbBase + ((row + pixelLines) * bytesPerScanline);
        for (size_t i = 0; i < copyBytesPerRow; i++) {
            dst[i] = src[i];
        }
    }

    // Clear bottom pixelLines rows
    uint8_t* clearStart = fbBase + ((vbe->mode.YResolution - pixelLines) * bytesPerScanline);
    for (int row = 0; row < pixelLines; row++) {
        uint32_t* px = (uint32_t*)(clearStart + (row * bytesPerScanline));
        for (uint32_t col = 0; col < vbe->mode.XResolution; col++) {
            px[col] = background;
        }
    }
}