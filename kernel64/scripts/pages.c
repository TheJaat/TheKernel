/* pages.c - memory for big programs, part 1 (The-Jat's step G): pages
 * a program can put out of reach and back (mprotect), give back
 * (madvise MADV_DONTNEED, munmap of part of a mapping), a heap that
 * gives freed memory back, and system calls racing a thread that takes
 * their buffer away; and, in part 2, shared file mappings that are
 * written back, with pread and pwrite. pages-test.sh boots TheTaaJ and
 * runs
 *
 *   with programs pages    the checks; "pages: N checks, all passed"
 *
 * which starts pages again as a child, to be
 *   pages exit-shared      maps p.exit shared, writes "by exit" at 3
 *                          and exits without unmapping it */

#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <setjmp.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <spawn.h>
#include <sys/random.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

#define P 4096L
#define MIB (1024L * 1024)

uint64_t __taaj_memory_used(void);

static int checks, failures;

static void check(int held, const char *what)
{
    checks++;
    if (!held) {
        failures++;
        printf("pages: failed: %s (errno %d)\n", what, errno);
    }
}

/* ---- touching, and catching the fault ---------------------------- */

static sigjmp_buf back;
static volatile int faulted;

static void on_fault(int signal)
{
    (void)signal;
    faulted = 1;
    siglongjmp(back, 1);
}

/* Whether reading (or writing, if `write`) the byte at `at` faults.
 * A fault is caught and the program goes on. */
static int faults(volatile char *at, int write)
{
    faulted = 0;
    if (sigsetjmp(back, 1) == 0) {
        if (write)
            *at = 'w';
        else
            (void)*at;
    }
    return faulted;
}

static long used(void)
{
    return (long)__taaj_memory_used();
}

/* Whether every byte of `length` at `at` is `value`. */
static int all(const char *at, long length, char value)
{
    for (long i = 0; i < length; i++)
        if (at[i] != value)
            return 0;
    return 1;
}

/* ---- J26: mprotect ------------------------------------------------- */

static void protect_checks(void)
{
    char *p = mmap(NULL, 16 * P, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    check(p != MAP_FAILED, "16 pages mapped");
    if (p == MAP_FAILED)
        return;
    for (long n = 0; n < 16; n++)
        memset(p + n * P, 'a' + (int)n, P);

    check(mprotect(p + 3 * P, P, PROT_NONE) == 0, "a page out of reach");
    check(faults(p + 3 * P, 0), "reading it faults");
    check(faults(p + 3 * P + 100, 1), "writing it faults");
    check(!faults(p + 2 * P + 4095, 0) && !faults(p + 4 * P, 1), "its neighbours do not");
    char buffer[16];
    errno = 0;
    check(getrandom(p + 3 * P, 16, 0) == -1 && errno == EFAULT, "a system call refuses it (EFAULT)");
    check(mprotect(p + 3 * P, P, PROT_READ | PROT_WRITE) == 0, "back in reach");
    check(all(p + 3 * P, P, 'a' + 3), "with its bytes kept");
    check(!faults(p + 3 * P, 1), "and writable");

    check(mprotect(p + 5 * P, 2 * P, PROT_READ) == 0, "two pages read-only");
    check(!faults(p + 5 * P, 0) && !faults(p + 7 * P - 1, 0), "read");
    check(faults(p + 5 * P, 1) && faults(p + 6 * P + 9, 1), "but not written");
    errno = 0;
    check(getrandom(p + 6 * P, 16, 0) == -1 && errno == EFAULT, "nor written by a system call");
    int ends[2];
    check(pipe(ends) == 0 && write(ends[1], p + 5 * P, 16) == 16 && read(ends[0], buffer, 16) == 16
              && all(buffer, 16, 'a' + 5),
          "though a system call may read from them");
    close(ends[0]);
    close(ends[1]);
    check(all(p + 5 * P, P, 'a' + 5) && all(p + 6 * P, P, 'a' + 6), "their bytes as they were");

    check(mprotect(p + 8 * P, 8 * P, PROT_READ | PROT_WRITE) == 0 && mprotect(p, 16 * P, PROT_READ | PROT_WRITE) == 0,
          "all of it writable again");
    check(!faults(p + 5 * P, 1) && !faults(p + 3 * P, 1), "every page");

    errno = 0;
    check(mprotect(p, P, PROT_READ | PROT_EXEC) == -1 && errno == EACCES, "PROT_EXEC is refused (EACCES)");
    errno = 0;
    check(mprotect(p + 1, P, PROT_READ) == -1 && errno == EINVAL, "an address not on a page (EINVAL)");
    int on_stack;
    errno = 0;
    check(mprotect((void *)((uintptr_t)&on_stack & ~(P - 1)), P, PROT_READ) == -1 && errno == ENOMEM,
          "the main stack is not mapped memory (ENOMEM)");
    check(mprotect(p + 15 * P, 2 * P, PROT_READ) == -1 && errno == ENOMEM, "nor is past a mapping's end");
    check(munmap(p, 16 * P) == 0, "unmapped");

    /* Address space first, memory later: what a JIT or a big arena
     * does with PROT_NONE. An untouched page out of reach, then in. */
    char *later = mmap(NULL, 64 * P, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    check(later != MAP_FAILED, "a PROT_NONE mapping");
    if (later != MAP_FAILED) {
        check(faults(later, 0) && faults(later + 63 * P, 1), "out of reach");
        check(mprotect(later, 8 * P, PROT_READ | PROT_WRITE) == 0, "part of it brought in");
        check(!faults(later + 7 * P, 1) && all(later, P, 0), "zeros, and writable");
        check(faults(later + 8 * P, 0), "the rest still out of reach");
        check(munmap(later, 64 * P) == 0, "unmapped");
    }
    char *readable = mmap(NULL, 4 * P, PROT_READ, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    check(readable != MAP_FAILED && all(readable, 4 * P, 0), "a PROT_READ mapping reads as zeros");
    check(readable != MAP_FAILED && faults(readable + P, 1), "and cannot be written");
    if (readable != MAP_FAILED)
        munmap(readable, 4 * P);
}

/* ---- J27: madvise and munmap give memory back ---------------------- */

static void discard_checks(void)
{
    long size = 64 * MIB;
    long before = used();
    char *m = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    check(m != MAP_FAILED, "64 MiB mapped");
    if (m == MAP_FAILED)
        return;
    memset(m, 'x', size);
    long touched = used();
    check(touched - before >= size, "touched, it counts");
    check(madvise(m + 16 * MIB, 16 * MIB, MADV_DONTNEED) == 0, "16 MiB of it given back");
    check(touched - used() >= 16 * MIB && touched - used() < 17 * MIB, "16 MiB less counted");
    check(all(m + 16 * MIB, 16 * MIB, 0) && all(m + 15 * MIB, MIB, 'x') && all(m + 32 * MIB, MIB, 'x'),
          "zeros there next time, the rest as it was");
    check(madvise(m, size, MADV_WILLNEED) == 0 && madvise(m, size, MADV_SEQUENTIAL) == 0, "other advice is fine");

    long now = used();
    check(munmap(m + 40 * MIB, 8 * MIB) == 0, "8 MiB of it unmapped");
    check(now - used() >= 8 * MIB, "8 MiB less counted");
    check(faults(m + 40 * MIB, 0) && faults(m + 48 * MIB - 1, 1), "out of reach, as unmapped memory is");
    check(!faults(m + 48 * MIB, 1) && !faults(m + 40 * MIB - 1, 1), "the rest is not");
    check(munmap(m, size) == 0, "all of it unmapped");
    check(used() - before < MIB, "and all of it counted no more");
    errno = 0;
    check(madvise(m + 1, P, MADV_DONTNEED) == -1 && errno == EINVAL, "madvise on an address not on a page (EINVAL)");
}

/* ---- J31: the heap gives freed memory back -------------------------- */

static void heap_checks(void)
{
    long before = used();
    char *big = malloc(32 * MIB);
    check(big != NULL, "32 MiB from malloc");
    if (big != NULL) {
        memset(big, 'b', 32 * MIB);
        check(used() - before >= 32 * MIB, "touched, it counts");
        free(big);
        check(used() - before < MIB, "freed, it goes back");
    }

    /* Small blocks: 48 bytes each (64 with malloc's header), 200 000
     * of them - some 12.5 MiB, in 64 KiB pieces of their own size. */
    enum { COUNT = 200000 };
    static char *blocks[COUNT];
    before = used();
    int got = 1;
    for (int i = 0; i < COUNT; i++) {
        blocks[i] = malloc(48);
        if (blocks[i] == NULL) {
            got = 0;
            break;
        }
        memset(blocks[i], 's', 48);
    }
    check(got, "200 000 small blocks");
    long full = used();
    check(full - before >= 12 * MIB, "they count");
    for (int i = 0; i < COUNT; i++)
        free(blocks[i]);
    check(full - used() >= 11 * MIB, "freed, their pieces go back");
    /* Taken again: as many, and the memory comes back with them. */
    for (int i = 0; i < COUNT && got; i++)
        got = (blocks[i] = malloc(48)) != NULL;
    check(got, "and they can be had again");
    for (int i = 0; i < COUNT; i++)
        free(blocks[i]);
}

/* ---- J30: a system call copying into a page another thread takes away */

static char *target;
static volatile int stop;

static void *taker(void *unused)
{
    (void)unused;
    while (!stop) {
        madvise(target, P, MADV_DONTNEED);
        mprotect(target, P, PROT_NONE);
        mprotect(target, P, PROT_READ | PROT_WRITE);
    }
    return NULL;
}

static void race_checks(void)
{
    target = mmap(NULL, P, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    check(target != MAP_FAILED, "a page to race over");
    if (target == MAP_FAILED)
        return;
    pthread_t thread;
    check(pthread_create(&thread, NULL, taker, NULL) == 0, "a thread taking it away, over and over");
    long done = 0, refused = 0;
    for (int i = 0; i < 20000; i++) {
        ssize_t n = getrandom(target, 64, 0);
        if (n == 64)
            done++;
        else if (n == -1 && errno == EFAULT)
            refused++;
    }
    stop = 1;
    pthread_join(thread, NULL);
    check(done + refused == 20000, "every copy either went in or was refused, whole");
    check(done > 0, "some went in");
    printf("pages: %ld copies went in, %ld were refused\n", done, refused);
    munmap(target, P);
}

/* ---- J28: shared file mappings, written back; pread and pwrite ----- */

/* Makes `name` hold `size` bytes of `fill`. */
static int make_file(const char *name, long size, char fill)
{
    int fd = open(name, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0)
        return -1;
    static char block[4096];
    memset(block, fill, sizeof block);
    for (long done = 0; done < size;) {
        long n = size - done < (long)sizeof block ? size - done : (long)sizeof block;
        if (write(fd, block, (size_t)n) != n) {
            close(fd);
            return -1;
        }
        done += n;
    }
    return close(fd);
}

/* The byte at `offset` of `name`, read anew; -1 if it cannot be. */
static int byte_at(const char *name, long offset)
{
    int fd = open(name, O_RDONLY);
    unsigned char byte;
    int got = fd >= 0 && pread(fd, &byte, 1, offset) == 1;
    if (fd >= 0)
        close(fd);
    return got ? byte : -1;
}

static long size_of(const char *name)
{
    struct stat status;
    return stat(name, &status) == 0 ? (long)status.st_size : -1;
}

static void shared_checks(void)
{
    /* pread and pwrite first: mmap's write-back is made of them. */
    check(make_file("p.file", 3 * P + 100, 'f') == 0, "a file of three pages and 100 bytes");
    int fd = open("p.file", O_RDWR);
    check(fd >= 0, "opened to read and write");
    char got[4] = {0};
    check(lseek(fd, 7, SEEK_SET) == 7, "its place set to 7");
    check(pwrite(fd, "PW", 2, 2 * P) == 2 && pread(fd, got, 2, 2 * P) == 2 && memcmp(got, "PW", 2) == 0,
          "pwrite and pread at 2 pages in");
    check(lseek(fd, 0, SEEK_CUR) == 7, "and the place is still 7");
    int ends[2];
    errno = 0;
    check(pipe(ends) == 0 && pread(ends[0], got, 1, 0) == -1 && errno == ESPIPE, "pread on a pipe: ESPIPE");
    close(ends[0]);
    close(ends[1]);
    errno = 0;
    check(pwrite(fd, "x", 1, -1) == -1 && errno == EINVAL, "a negative offset: EINVAL");

    char *m = mmap(NULL, 4 * P, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    check(m != MAP_FAILED, "mapped shared and writable");
    if (m == MAP_FAILED)
        return;
    check(m[0] == 'f' && m[2 * P] == 'P' && m[3 * P + 99] == 'f' && m[3 * P + 100] == 0, "the file's bytes, then zeros");
    m[10] = 'X';
    m[2 * P + 5] = 'Y';
    m[3 * P + 200] = 'Z'; /* past the end of the file */
    check(byte_at("p.file", 10) == 'f', "not in the file before msync");
    check(msync(m, 4 * P, MS_SYNC) == 0, "msync");
    check(byte_at("p.file", 10) == 'X' && byte_at("p.file", 2 * P + 5) == 'Y', "in the file after it");
    check(size_of("p.file") == 3 * P + 100, "and the file no longer for what was past its end");
    m[20] = 'U';
    close(fd); /* the mapping keeps a descriptor of its own */
    check(munmap(m, 4 * P) == 0, "unmapped, after its descriptor was closed");
    check(byte_at("p.file", 20) == 'U', "the change reached the file all the same");

    fd = open("p.file", O_RDWR);
    char *second = fd >= 0 ? mmap(NULL, P, PROT_READ | PROT_WRITE, MAP_SHARED, fd, P) : MAP_FAILED;
    check(second != MAP_FAILED && second[0] == 'f', "a page from one page in");
    if (second != MAP_FAILED) {
        second[1] = 'O';
        check(munmap(second, P) == 0 && byte_at("p.file", P + 1) == 'O', "written back to its place in the file");
    }
    if (fd >= 0)
        close(fd);
    errno = 0;
    check(msync(m, P, MS_SYNC) == -1 && errno == ENOMEM, "msync of what is no longer mapped: ENOMEM");

    char *private = MAP_FAILED;
    fd = open("p.file", O_RDONLY);
    if (fd >= 0)
        private = mmap(NULL, P, PROT_READ | PROT_WRITE, MAP_PRIVATE, fd, 0);
    check(private != MAP_FAILED, "a private mapping");
    if (private != MAP_FAILED) {
        private[0] = 'Q';
        check(munmap(private, P) == 0 && byte_at("p.file", 0) == 'f', "is never written back");
    }
    if (fd >= 0)
        close(fd);

    /* Written back when the program exits, as on Unix. */
    check(make_file("p.exit", 2 * P, 'e') == 0, "a file for a child to map");
    pid_t child;
    char *argv[] = {"pages", "exit-shared", NULL};
    int status = 0;
    check(posix_spawnp(&child, "pages", NULL, NULL, argv, environ) == 0 && waitpid(child, &status, 0) == child
              && WIFEXITED(status) && WEXITSTATUS(status) == 0,
          "a child mapped it, wrote and exited");
    check(byte_at("p.exit", 3) == 'b' && byte_at("p.exit", 9) == 't' && byte_at("p.exit", 10) == 'e',
          "and what it wrote is in the file");
    unlink("p.file");
    unlink("p.exit");
}

/* The child: maps p.exit, writes, and exits without unmapping. */
static int exit_shared(void)
{
    int fd = open("p.exit", O_RDWR);
    char *m = fd >= 0 ? mmap(NULL, 2 * P, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0) : MAP_FAILED;
    if (m == MAP_FAILED)
        return 1;
    memcpy(m + 3, "by exit", 7);
    return 0;
}

int main(int argc, char **argv)
{
    if (argc > 1 && strcmp(argv[1], "exit-shared") == 0)
        return exit_shared();

    struct sigaction action;
    memset(&action, 0, sizeof action);
    action.sa_handler = on_fault;
    sigaction(SIGSEGV, &action, NULL);

    protect_checks();
    discard_checks();
    heap_checks();
    race_checks();
    shared_checks();
    if (failures == 0)
        printf("pages: %d checks, all passed\n", checks);
    else
        printf("pages: %d of %d checks failed\n", failures, checks);
    return failures != 0;
}
