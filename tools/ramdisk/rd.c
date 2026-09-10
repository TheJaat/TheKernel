/* rd - builds a ramdisk image for TheTaaJ.
 *
 * Usage: rd <output.mdr> <file> [file ...]
 *
 * Host tool: compiled with the system gcc, not the cross compiler. It
 * shares ramdisk.h with the kernel so the two can never disagree about
 * the layout - that is the whole reason the header lives outside the
 * kernel tree. */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "ramdisk.h"

#define ALIGN4(x)   (((x) + 3u) & ~3u)

/* Basename
 * Store only the final path component. The kernel has no notion of
 * directories, and 'tools/build/hello.txt' would be a surprising name to
 * have to type at the shell. */
static const char *Basename(const char *Path)
{
    const char *Slash = strrchr(Path, '/');
    return (Slash != NULL) ? (Slash + 1) : Path;
}

int main(int argc, char **argv)
{
    RamdiskHeader_t Header;
    RamdiskEntry_t *Entries;
    unsigned char **Data;
    unsigned int *Sizes;
    unsigned int Count, i;
    unsigned int Offset;
    FILE *Out;

    if (argc < 3) {
        fprintf(stderr, "usage: %s <output.mdr> <file> [file ...]\n", argv[0]);
        return 1;
    }

    Count = (unsigned int)(argc - 2);
    Entries = calloc(Count, sizeof(RamdiskEntry_t));
    Data = calloc(Count, sizeof(unsigned char*));
    Sizes = calloc(Count, sizeof(unsigned int));
    if (Entries == NULL || Data == NULL || Sizes == NULL) {
        fprintf(stderr, "rd: out of memory\n");
        return 1;
    }

    /* Read every input first, so a missing file is reported before any
     * output is written rather than leaving a half-built image. */
    for (i = 0; i < Count; i++) {
        const char *Path = argv[2 + i];
        FILE *In = fopen(Path, "rb");
        long Length;

        if (In == NULL) {
            fprintf(stderr, "rd: cannot open %s\n", Path);
            return 1;
        }
        fseek(In, 0, SEEK_END);
        Length = ftell(In);
        fseek(In, 0, SEEK_SET);

        if (Length < 0) {
            fprintf(stderr, "rd: cannot size %s\n", Path);
            return 1;
        }

        Data[i] = malloc((size_t)Length ? (size_t)Length : 1);
        if (Data[i] == NULL || fread(Data[i], 1, (size_t)Length, In)
            != (size_t)Length) {
            fprintf(stderr, "rd: short read on %s\n", Path);
            return 1;
        }
        fclose(In);

        Sizes[i] = (unsigned int)Length;

        if (strlen(Basename(Path)) >= RAMDISK_NAME_LENGTH) {
            fprintf(stderr, "rd: name too long: %s\n", Basename(Path));
            return 1;
        }
        strncpy(Entries[i].Name, Basename(Path), RAMDISK_NAME_LENGTH - 1);
        Entries[i].Size = Sizes[i];
    }

    /* Lay the image out. */
    Offset = (unsigned int)(sizeof(RamdiskHeader_t)
           + (Count * sizeof(RamdiskEntry_t)));

    for (i = 0; i < Count; i++) {
        unsigned int j;
        unsigned int Sum = 0;

        Entries[i].Offset = Offset;
        for (j = 0; j < Sizes[i]; j++) {
            Sum += Data[i][j];
        }
        Entries[i].Checksum = Sum;
        Offset += ALIGN4(Sizes[i]);
    }

    Header.Magic     = RAMDISK_MAGIC;
    Header.Version   = RAMDISK_VERSION;
    Header.FileCount = Count;
    Header.TotalSize = Offset;

    Out = fopen(argv[1], "wb");
    if (Out == NULL) {
        fprintf(stderr, "rd: cannot create %s\n", argv[1]);
        return 1;
    }

    fwrite(&Header, sizeof(Header), 1, Out);
    fwrite(Entries, sizeof(RamdiskEntry_t), Count, Out);

    for (i = 0; i < Count; i++) {
        static const unsigned char Pad[4] = { 0, 0, 0, 0 };
        unsigned int Padding = ALIGN4(Sizes[i]) - Sizes[i];

        fwrite(Data[i], 1, Sizes[i], Out);
        if (Padding > 0) {
            fwrite(Pad, 1, Padding, Out);
        }
    }

    fclose(Out);

    printf("rd: %s, %u file%s, %u bytes\n",
        argv[1], Count, (Count == 1) ? "" : "s", Header.TotalSize);
    for (i = 0; i < Count; i++) {
        printf("    %-32s %8u bytes at +%u\n",
            Entries[i].Name, Entries[i].Size, Entries[i].Offset);
    }
    return 0;
}