#ifndef __RAMDISK_H__
#define __RAMDISK_H__

#include <stdint.h>

/* On-disk layout of the ramdisk image.
 *
 *   [ RamdiskHeader ]
 *   [ RamdiskEntry ] x FileCount
 *   [ file data, each padded to 4 bytes ]
 *
 * Everything is little-endian and 32-bit, matching the kernel. The image
 * is loaded whole at MEMORY_LOCATION_RAMDISK and read in place - there
 * is no decompression and no copying, so the entries point directly into
 * the loaded image. */

#define RAMDISK_MAGIC               0x5241414A  /* 'RAAJ' */
#define RAMDISK_VERSION             1
#define RAMDISK_NAME_LENGTH         64

typedef struct _RamdiskHeader {
    uint32_t    Magic;
    uint32_t    Version;
    uint32_t    FileCount;
    uint32_t    TotalSize;      /* header + entries + data */
} __attribute__((packed)) RamdiskHeader_t;

typedef struct _RamdiskEntry {
    char        Name[RAMDISK_NAME_LENGTH];
    uint32_t    Offset;         /* from the start of the image */
    uint32_t    Size;           /* actual bytes, not the padded size */
    uint32_t    Checksum;       /* simple additive sum of the bytes    */
    uint32_t    Reserved;
} __attribute__((packed)) RamdiskEntry_t;

#endif /* __RAMDISK_H__ */