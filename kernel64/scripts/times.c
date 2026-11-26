/* times.c - TaajFS version 6 (The-Jat's step D): file times, files
 * over 256 MiB on a volume over 256 MiB, and a rename that replaces.
 * times-test.sh makes a 320 MiB volume, puts this program in /bin and a
 * 300 MiB file, "big", in the root, boots TheTaaJ and runs
 *
 *   with programs times    the checks; "times: N checks, all passed"
 *
 * (the shell finds "times" in /bin: nothing of that name is in the
 * root).
 *
 * "big" is block after block of 4 KiB: block n starts with n as four
 * bytes, low byte first, and is n's low byte for the rest. */

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#define BLOCK 4096
#define MIB (1024L * 1024)
#define BIG_SIZE (300 * MIB)

static int checks, failures;

static void check(int held, const char *what)
{
    checks++;
    if (!held) {
        failures++;
        printf("times: failed: %s (errno %d)\n", what, errno);
    }
}

/* What the byte at `offset` of "big" is. */
static unsigned char big_byte(long offset)
{
    long block = offset / BLOCK, in = offset % BLOCK;
    if (in < 4)
        return (unsigned char)(block >> (8 * in));
    return (unsigned char)block;
}

static int write_file(const char *name, const char *text)
{
    int fd = open(name, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0)
        return -1;
    ssize_t n = write(fd, text, strlen(text));
    close(fd);
    return n == (ssize_t)strlen(text) ? 0 : -1;
}

/* Whether the file `name` holds exactly `text`. */
static int holds(const char *name, const char *text)
{
    char buffer[64] = {0};
    int fd = open(name, O_RDONLY);
    if (fd < 0)
        return 0;
    ssize_t n = read(fd, buffer, sizeof buffer - 1);
    close(fd);
    return n == (ssize_t)strlen(text) && memcmp(buffer, text, n) == 0;
}

/* a is older than b. */
static int before(struct timespec a, struct timespec b)
{
    return a.tv_sec < b.tv_sec || (a.tv_sec == b.tv_sec && a.tv_nsec < b.tv_nsec);
}

static void time_checks(void)
{
    struct stat first, second, again;
    struct timespec now;
    clock_gettime(CLOCK_REALTIME, &now);

    check(write_file("t.first", "one") == 0, "a file written");
    check(stat("t.first", &first) == 0, "and its stat");
    /* 2023-11-14 or later: a real time, not a stamp's handful of
     * seconds after 1970. */
    check(first.st_mtim.tv_sec > 1700000000, "its time is a time of day");
    check(first.st_mtim.tv_sec >= now.tv_sec - 2 && first.st_mtim.tv_sec <= now.tv_sec + 60,
          "the time it was written, by TheTaaJ's clock");
    check(first.st_atim.tv_sec == first.st_mtim.tv_sec && first.st_ctim.tv_nsec == first.st_mtim.tv_nsec,
          "all three times the same");

    check(write_file("t.second", "two") == 0 && stat("t.second", &second) == 0, "a second file, after it");
    check(before(first.st_mtim, second.st_mtim), "is newer, to the nanosecond");

    check(write_file("t.first", "one more") == 0 && stat("t.first", &again) == 0, "the first written again");
    check(before(second.st_mtim, again.st_mtim), "is now the newer");

    check(utimensat(AT_FDCWD, "t.second", NULL, 0) == 0 && stat("t.second", &second) == 0, "a touch");
    check(before(again.st_mtim, second.st_mtim), "makes the other the newer");
    check(holds("t.second", "two"), "and leaves its bytes");

    /* A new name is not a change: the time stays. */
    check(rename("t.second", "t.moved") == 0 && stat("t.moved", &again) == 0, "a rename");
    check(again.st_mtim.tv_sec == second.st_mtim.tv_sec && again.st_mtim.tv_nsec == second.st_mtim.tv_nsec,
          "keeps the time");

    struct stat directory;
    check(stat("/", &directory) == 0 && S_ISDIR(directory.st_mode), "the root's stat");
    check(directory.st_mtim.tv_sec == 1 && directory.st_mtim.tv_nsec == 0, "a directory has no time: 1, before every file");

    check(unlink("t.first") == 0 && unlink("t.moved") == 0, "both removed");
}

static void rename_checks(void)
{
    check(write_file("r.old", "old") == 0 && write_file("r.new", "new") == 0, "two files");
    check(rename("r.new", "r.old") == 0, "a rename onto a file replaces it");
    check(holds("r.old", "new"), "with the moved file's bytes");
    check(access("r.new", F_OK) == -1 && errno == ENOENT, "under the one name");

    check(mkdir("r.dir", 0755) == 0 && mkdir("r.empty", 0755) == 0 && mkdir("r.full", 0755) == 0, "three directories");
    check(write_file("r.full/in", "in") == 0 && write_file("r.dir/kept", "kept") == 0, "something in two of them");
    errno = 0;
    check(rename("r.dir", "r.full") == -1 && errno == ENOTEMPTY, "not onto a directory with things in it");
    errno = 0;
    check(rename("r.old", "r.empty") == -1 && errno == EISDIR, "a file not onto a directory");
    errno = 0;
    check(rename("r.dir", "r.old") == -1 && errno == ENOTDIR, "a directory not onto a file");
    check(holds("r.old", "new") && holds("r.full/in", "in"), "and those change nothing");
    check(rename("r.dir", "r.empty") == 0, "a directory onto an empty one replaces it");
    check(holds("r.empty/kept", "kept") && access("r.dir", F_OK) == -1, "with what was in it, under the one name");

    check(unlink("r.old") == 0 && unlink("r.empty/kept") == 0 && rmdir("r.empty") == 0, "removed");
    check(unlink("r.full/in") == 0 && rmdir("r.full") == 0, "all of it");
}

static void big_checks(void)
{
    struct stat status;
    check(stat("big", &status) == 0, "the 300 MiB file's stat");
    check(status.st_size == BIG_SIZE, "300 MiB: more than a whole old volume");

    int fd = open("big", O_RDWR);
    check(fd >= 0, "opened to read and write");
    /* Places through the file, before and past 256 MiB - the most a
     * volume could hold before version 6's bigger volumes. */
    static const long places[] = {0, 4092, 255 * MIB + 17, 256 * MIB, 256 * MIB + 4093, 299 * MIB + 12345, BIG_SIZE - 8};
    int all = 1;
    for (unsigned i = 0; i < sizeof places / sizeof *places; i++) {
        unsigned char got[8], want[8];
        for (int k = 0; k < 8; k++)
            want[k] = big_byte(places[i] + k);
        if (lseek(fd, places[i], SEEK_SET) != places[i] || read(fd, got, 8) != 8 || memcmp(got, want, 8) != 0) {
            printf("times: big at %ld is not what was put\n", places[i]);
            all = 0;
        }
    }
    check(all, "its bytes, read anywhere, are what the host put");

    /* Written past 256 MiB, here. */
    static const char mark[] = "written by TheTaaJ";
    long at = 290 * MIB + 100;
    check(lseek(fd, at, SEEK_SET) == at && write(fd, mark, sizeof mark - 1) == (ssize_t)(sizeof mark - 1),
          "written past 256 MiB");
    char back[sizeof mark] = {0};
    check(lseek(fd, at, SEEK_SET) == at && read(fd, back, sizeof mark - 1) == (ssize_t)(sizeof mark - 1)
              && memcmp(back, mark, sizeof mark - 1) == 0,
          "and read back");
    unsigned char after = 0;
    check(read(fd, &after, 1) == 1 && after == big_byte(at + sizeof mark - 1), "the byte after it untouched");
    close(fd);
    check(stat("big", &status) == 0 && status.st_size == BIG_SIZE, "the size the same");
    check(status.st_mtim.tv_sec > 1700000000, "and a time of day for it");

    /* Growing it past its end, to 301 MiB. */
    fd = open("big", O_WRONLY | O_APPEND);
    char *more = malloc(MIB);
    check(fd >= 0 && more != NULL, "opened to add to");
    if (more) {
        memset(more, 'm', MIB);
        check(write(fd, more, MIB) == MIB, "a MiB more");
        free(more);
    }
    close(fd);
    check(stat("big", &status) == 0 && status.st_size == BIG_SIZE + MIB, "301 MiB");
}

int main(void)
{
    time_checks();
    rename_checks();
    big_checks();
    if (failures == 0)
        printf("times: %d checks, all passed\n", checks);
    else
        printf("times: %d of %d checks failed\n", failures, checks);
    return failures != 0;
}
