/* bigmem.c - a bigger machine (Era 9, R1), checked on TheTaaJ by
 * bigmem-test.sh, with 4 GB of memory and 4 CPUs:
 *
 *   with files bigmem   the checks; "bigmem: N checks, all passed":
 *                       reserved memory costs nothing until touched,
 *                       and munmap gives it back; a file mapped; a
 *                       system call reading into untouched pages; deep
 *                       stacks, the main one and a thread's of 64 MiB;
 *                       60 threads of 8 MiB stacks for little; a big
 *                       malloc
 *   bigmem big          touches 3 GiB, or what the limit allows, and
 *                       gives it back
 *   bigmem limit        lowers its limit to 64 MiB (setrlimit); a
 *                       program it starts gets it too; then touches
 *                       256 MiB - and is ended, out of memory
 *   bigmem report       says its limit
 *   bigmem overflow     a thread runs off its stack: the page below it
 *                       is never filled, and the program is ended
 *   bigmem shoot        one thread reads a page over and over on one
 *                       CPU while another unmaps it on another: the
 *                       reader must fault at once (a TLB shootdown on
 *                       x86), not go on reading memory given back */

#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <time.h>
#include <unistd.h>

#define MiB (1024UL * 1024)
#define PAGE 4096UL

/* libc/src/memory.rs: what the kernel counts against this program. */
unsigned long __taaj_memory_used(void);

static int checks, failures;

static void check(int held, const char *what)
{
    checks++;
    if (!held) {
        failures++;
        printf("bigmem: failed: %s\n", what);
    }
}

static double now(void)
{
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double)t.tv_sec + (double)t.tv_nsec / 1e9;
}

static unsigned long used_mib(void)
{
    return __taaj_memory_used() / MiB;
}

static unsigned long limit_bytes(void)
{
    struct rlimit r;
    return getrlimit(RLIMIT_AS, &r) == 0 ? r.rlim_cur : 0;
}

/* ---- reserved memory: free until touched ---- */

static void reserved(void)
{
    unsigned long before = __taaj_memory_used();
    char *big = mmap(NULL, 1024 * MiB, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    check(big != MAP_FAILED, "1 GiB mapped");
    if (big == MAP_FAILED)
        return;
    check(__taaj_memory_used() - before < MiB, "mapping 1 GiB took nothing yet");
    for (unsigned long at = 0; at < 64 * MiB; at += PAGE)
        big[at] = (char)(at / PAGE);
    unsigned long grew = __taaj_memory_used() - before;
    check(grew >= 64 * MiB && grew < 66 * MiB, "touching 64 MiB of it took 64 MiB");
    int kept = 1;
    for (unsigned long at = 0; at < 64 * MiB; at += PAGE)
        kept &= big[at] == (char)(at / PAGE);
    check(kept, "each touched page kept what was written");
    check(big[1000 * MiB] == 0 && big[1024 * MiB - 1] == 0, "an untouched page reads zero");
    check(munmap(big, 1024 * MiB) == 0, "munmap");
    check(__taaj_memory_used() - before < MiB, "munmap gave every page back");
}

/* ---- a file, and a system call writing into untouched pages ---- */

static void files(void)
{
    static char pattern[10000];
    for (int n = 0; n < 10000; n++)
        pattern[n] = (char)('a' + n % 26);
    int fd = open("bigmem.txt", O_CREAT | O_TRUNC | O_RDWR, 0644);
    check(fd >= 0 && write(fd, pattern, sizeof pattern) == (ssize_t)sizeof pattern, "bigmem.txt written");
    if (fd < 0)
        return;
    char *mapped = mmap(NULL, sizeof pattern, PROT_READ, MAP_PRIVATE, fd, 0);
    check(mapped != MAP_FAILED, "bigmem.txt mapped");
    if (mapped != MAP_FAILED) {
        check(memcmp(mapped, pattern, sizeof pattern) == 0, "the mapping holds the file");
        check(mapped[10000] == 0 && mapped[3 * PAGE - 1] == 0, "past the file's end: zeros");
        check(munmap(mapped, sizeof pattern) == 0, "the file's mapping undone");
    }
    /* read() straight into reserved pages nobody has touched: the kernel
     * fills them as it copies. */
    char *fresh = mmap(NULL, MiB, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    check(fresh != MAP_FAILED, "1 MiB mapped");
    if (fresh != MAP_FAILED) {
        /* A read may bring less than was asked; a reader goes on. */
        size_t got = 0;
        ssize_t n = 1;
        check(lseek(fd, 0, SEEK_SET) == 0, "back to the file's start");
        while (got < 5000 && n > 0) {
            n = read(fd, fresh + 512 * 1024 - 100 + got, 5000 - got);
            got += n > 0 ? (size_t)n : 0;
        }
        check(got == 5000, "read() into untouched pages, across two of them");
        check(memcmp(fresh + 512 * 1024 - 100, pattern, 5000) == 0, "what read() put there");
        munmap(fresh, MiB);
    }
    close(fd);
    unlink("bigmem.txt");
}

/* ---- stacks ---- */

/* `depth` frames of 64 KiB each, every page of each written from the
 * top down, as a stack is used: so the first page past the stack's end
 * that is touched is the unmapped one under it. (Written bottom first,
 * a frame bigger than that one page could step over it, into whatever
 * lies below - "stack clash". Rust probes each page of a big frame,
 * top down; C does with clang's -fstack-clash-protection.) */
static int deep(int depth)
{
    volatile char frame[64 * 1024];
    /* The array's address leaves the compiler's sight, so all 64 KiB of
     * it are made: else clang keeps only the bytes written, and the
     * frame is 32 bytes. */
    __asm__ volatile("" : : "r"(frame) : "memory");
    for (unsigned long at = sizeof frame; at >= PAGE; at -= PAGE)
        frame[at - 1] = (char)depth;
    frame[0] = (char)depth;
    int below = depth > 1 ? deep(depth - 1) : 0;
    return below + (frame[0] == (char)depth && frame[sizeof frame - 1] == (char)depth);
}

static void *deep_thread(void *argument)
{
    return (void *)(long)deep((int)(long)argument);
}

static pthread_mutex_t gate = PTHREAD_MUTEX_INITIALIZER;

static void *wait_at_gate(void *argument)
{
    (void)argument;
    pthread_mutex_lock(&gate);
    pthread_mutex_unlock(&gate);
    return NULL;
}

static void stacks(void)
{
    /* The main stack: 8 MiB, reserved. 6 MiB of it. */
    check(deep(96) == 96, "6 MiB deep on the main stack");

    /* A thread with a 64 MiB stack, 48 MiB of it used. */
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setstacksize(&attr, 64 * MiB);
    pthread_t t;
    void *result = NULL;
    check(pthread_create(&t, &attr, deep_thread, (void *)768L) == 0 && pthread_join(t, &result) == 0
              && (long)result == 768,
          "48 MiB deep on a thread's 64 MiB stack");

    /* 60 threads at once, each with the default 8 MiB stack: 480 MiB of
     * stacks, of which they touch a few pages each. */
    unsigned long before = __taaj_memory_used();
    pthread_mutex_lock(&gate);
    pthread_t many[60];
    int made = 0;
    for (int n = 0; n < 60; n++)
        made += pthread_create(&many[n], NULL, wait_at_gate, NULL) == 0;
    check(made == 60, "60 threads of 8 MiB stacks");
    unsigned long grew = __taaj_memory_used() - before;
    pthread_mutex_unlock(&gate);
    for (int n = 0; n < made; n++)
        pthread_join(many[n], NULL);
    printf("bigmem: 60 threads, 480 MiB of stacks, took %lu KiB\n", grew / 1024);
    check(grew < 8 * MiB, "60 threads' stacks took under 8 MiB");
}

/* ---- malloc ---- */

static void big_malloc(void)
{
    unsigned long before = __taaj_memory_used();
    char *block = malloc(512 * MiB);
    check(block != NULL, "malloc(512 MiB)");
    if (block == NULL)
        return;
    memset(block, 7, 100 * MiB);
    unsigned long grew = __taaj_memory_used() - before;
    check(grew >= 100 * MiB && grew < 110 * MiB, "512 MiB from malloc, 100 MiB of it used, took 100 MiB");
    check(block[100 * MiB - 1] == 7 && block[512 * MiB - 1] == 0, "malloc's memory: written, and not yet");
    free(block);
}

/* ---- bigmem big ---- */

static int big(void)
{
    unsigned long want = 3072 * MiB, limit = limit_bytes();
    if (limit < want + 256 * MiB)
        want = limit > 512 * MiB ? (limit - 256 * MiB) & ~(MiB - 1) : 256 * MiB;
    printf("bigmem: limit %lu MiB, touching %lu MiB\n", limit / MiB, want / MiB);
    unsigned long before = __taaj_memory_used();
    double start = now();
    unsigned long *memory = mmap(NULL, want, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (memory == MAP_FAILED) {
        printf("bigmem: mmap of %lu MiB failed\n", want / MiB);
        return 1;
    }
    unsigned long words = want / sizeof *memory, step = PAGE / sizeof *memory;
    for (unsigned long n = 0; n < words; n += step)
        memory[n] = n;
    unsigned long took = (__taaj_memory_used() - before) / MiB;
    int right = 1;
    for (unsigned long n = 0; n < words; n += step)
        right &= memory[n] == n;
    double seconds = now() - start;
    munmap(memory, want);
    unsigned long after = (__taaj_memory_used() - before) / MiB;
    printf("bigmem: touched %lu MiB in %.1f s; it took %lu MiB; every page kept its word: %s; after munmap: %lu MiB\n",
           want / MiB, seconds, took, right ? "yes" : "NO", after);
    return right && took >= want / MiB && after == 0 ? 0 : 1;
}

/* ---- bigmem limit, bigmem report ---- */

static int report(void)
{
    printf("bigmem: my limit is %lu MiB\n", limit_bytes() / MiB);
    return 0;
}

static int limit(void)
{
    struct rlimit r = { 64 * MiB, 64 * MiB };
    if (setrlimit(RLIMIT_AS, &r) != 0 || limit_bytes() != 64 * MiB) {
        printf("bigmem: setrlimit to 64 MiB did not hold\n");
        return 1;
    }
    struct rlimit more = { 128 * MiB, 128 * MiB };
    errno = 0;
    if (setrlimit(RLIMIT_AS, &more) != -1 || errno != EPERM) {
        printf("bigmem: the limit was raised\n");
        return 1;
    }
    printf("bigmem: limit lowered to 64 MiB, raising it refused\n");
    fflush(stdout);
    if (system("bigmem report") != 0)
        printf("bigmem: bigmem report did not run\n");
    char *memory = mmap(NULL, 256 * MiB, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    printf("bigmem: 256 MiB mapped: %s; touching it all\n", memory == MAP_FAILED ? "no" : "yes");
    fflush(stdout);
    for (unsigned long at = 0; at < 256 * MiB; at += PAGE)
        memory[at] = 1;
    printf("bigmem: not reached: 256 MiB touched under a limit of 64\n");
    return 1;
}

/* ---- bigmem overflow ---- */

static void *overflow(void *argument)
{
    return (void *)(long)deep((int)(long)argument);
}

static int overflow_a_stack(void)
{
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setstacksize(&attr, MiB);
    pthread_t t;
    printf("bigmem: a thread with 1 MiB of stack goes 2 MiB deep\n");
    fflush(stdout);
    pthread_create(&t, &attr, overflow, (void *)32L);
    pthread_join(t, NULL);
    printf("bigmem: not reached: the thread came back\n");
    return 1;
}

/* ---- bigmem shoot ---- */

static volatile unsigned long *target;
static volatile int reading;

static void *reader(void *argument)
{
    (void)argument;
    unsigned long sum = 0;
    reading = 1;
    for (;;)
        sum += target[0]; /* until the page goes, and this faults */
    return (void *)sum;
}

static int shoot(void)
{
    target = mmap(NULL, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    target[0] = 1;
    pthread_t t;
    pthread_create(&t, NULL, reader, NULL);
    while (!reading)
        ;
    struct timespec moment = { 0, 50 * 1000000L };
    nanosleep(&moment, NULL); /* the reader is at it, on its own CPU */
    printf("bigmem: unmapping the page another thread is reading\n");
    fflush(stdout);
    munmap((void *)target, PAGE);
    struct timespec second = { 1, 0 };
    nanosleep(&second, NULL);
    printf("bigmem: not reached: the reader went on for a second after the unmap\n");
    return 1;
}

int main(int argc, char **argv)
{
    const char *mode = argc > 1 ? argv[1] : "";
    if (strcmp(mode, "big") == 0)
        return big();
    if (strcmp(mode, "limit") == 0)
        return limit();
    if (strcmp(mode, "report") == 0)
        return report();
    if (strcmp(mode, "overflow") == 0)
        return overflow_a_stack();
    if (strcmp(mode, "shoot") == 0)
        return shoot();
    printf("bigmem: limit %lu MiB, %lu MiB used at the start\n", limit_bytes() / MiB, used_mib());
    reserved();
    files();
    stacks();
    big_malloc();
    if (failures != 0) {
        printf("bigmem: %d of %d checks failed\n", failures, checks);
        return 1;
    }
    printf("bigmem: %d checks, all passed\n", checks);
    return 0;
}
