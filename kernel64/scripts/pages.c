/* pages.c - memory for big programs, part 1 (The-Jat's step G): pages
 * a program can put out of reach and back (mprotect), give back
 * (madvise MADV_DONTNEED, munmap of part of a mapping), a heap that
 * gives freed memory back, and system calls racing a thread that takes
 * their buffer away. pages-test.sh boots TheTaaJ and runs
 *
 *   with programs pages    the checks; "pages: N checks, all passed" */

#include <errno.h>
#include <pthread.h>
#include <setjmp.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/random.h>
#include <unistd.h>

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

int main(void)
{
    struct sigaction action;
    memset(&action, 0, sizeof action);
    action.sa_handler = on_fault;
    sigaction(SIGSEGV, &action, NULL);

    protect_checks();
    discard_checks();
    heap_checks();
    race_checks();
    if (failures == 0)
        printf("pages: %d checks, all passed\n", checks);
    else
        printf("pages: %d of %d checks failed\n", failures, checks);
    return failures != 0;
}
