/* ground2.c - std's ground, part 2 (The-Jat's step C): the C library
 * calls Rust's std and rustc ask for. ground2-test.sh boots TheTaaJ and
 * runs
 *
 *   with programs ground2    the checks; "ground2: N checks, all passed"
 *
 * which start ground2 again as children, to be:
 *   ground2 exit-with N        exits with N
 *   ground2 raise N            sends itself signal N (and so ends)
 *   ground2 env-answer         exits with the number in $ANSWER
 *   ground2 argv0              exits 0 if its argv[0] is "custom-name"
 *   ground2 write-fd N         writes "through exec" on descriptor N
 *   ground2 fd-closed N        exits 0 if descriptor N is not open
 *   ground2 sleep-write N      sleeps 200 ms, writes "late" on N
 *   ground2 exec-to ...        execvp("ground2", the rest): exec's turn
 *   ground2 exec-closing N ... marks N close-on-exec, then as exec-to
 *   ground2 exec-env           execle("/ground2", env-answer) with
 *                              ANSWER=42 as the whole environment */

#include <dirent.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <link.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <spawn.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/auxv.h>
#include <sys/select.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#include <zlib.h>

extern char **environ;

static int checks, failures;

static void check(int held, const char *what)
{
    checks++;
    if (!held) {
        failures++;
        printf("ground2: failed: %s\n", what);
    }
}

static long milliseconds(void)
{
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    return now.tv_sec * 1000 + now.tv_nsec / 1000000;
}

static void sleep_ms(long ms)
{
    struct timespec wanted = {ms / 1000, (ms % 1000) * 1000000};
    nanosleep(&wanted, NULL);
}

/* ---- children ------------------------------------------------------- */

/* Starts ground2 with these words after its name (argv[0] `name`);
 * waits for it; its wait status, or -1 if it could not be started. */
static int run_as(const char *name, char *const words[], char *const *environment)
{
    char *argv[12] = {(char *)name};
    int n = 1;
    for (; words[n - 1] != NULL && n < 11; n++)
        argv[n] = words[n - 1];
    argv[n] = NULL;
    pid_t child;
    if (posix_spawnp(&child, "ground2", NULL, NULL, argv, environment) != 0)
        return -1;
    int status = -1;
    if (waitpid(child, &status, 0) != child)
        return -1;
    return status;
}

static int run(char *const words[])
{
    return run_as("ground2", words, environ);
}

static int exited_with(int status, int code)
{
    return status != -1 && WIFEXITED(status) && WEXITSTATUS(status) == code;
}

/* ---- poll and select ------------------------------------------------ */

static int poll_one(int fd, short events, int timeout)
{
    struct pollfd p = {fd, events, 0};
    int n = poll(&p, 1, timeout);
    return n < 0 ? -1 : p.revents;
}

static int late_fd;

static void *write_late(void *unused)
{
    (void)unused;
    sleep_ms(150);
    write(late_fd, "x", 1);
    return NULL;
}

static volatile sig_atomic_t alarms;

static void on_alarm(int signal)
{
    (void)signal;
    alarms++;
}

static void poll_checks(void)
{
    int p[2];
    if (pipe(p) != 0) {
        check(0, "pipe");
        return;
    }
    check(poll_one(p[0], POLLIN, 0) == 0, "an empty pipe: not readable, at once with timeout 0");
    check(poll_one(p[1], POLLOUT, 0) == POLLOUT, "a pipe's write end: writable");
    write(p[1], "abc", 3);
    check(poll_one(p[0], POLLIN, 0) == POLLIN, "with bytes in it: POLLIN");
    char got[8];
    check(read(p[0], got, 2) == 2, "read part");
    check(poll_one(p[0], POLLIN, 0) == POLLIN, "still POLLIN with one byte held");
    check(read(p[0], got, 8) == 1 && got[0] == 'c', "the rest");
    check(poll_one(p[0], POLLIN, 0) == 0, "empty again");

    long before = milliseconds();
    check(poll_one(p[0], POLLIN, 50) == 0, "poll with 50 ms: times out");
    long took = milliseconds() - before;
    check(took >= 40 && took < 1000, "and waits about 50 ms");

    /* Woken by another thread's write. */
    late_fd = p[1];
    pthread_t writer;
    before = milliseconds();
    pthread_create(&writer, NULL, write_late, NULL);
    check(poll_one(p[0], POLLIN, -1) == POLLIN, "poll with no timeout wakes when another thread writes");
    took = milliseconds() - before;
    check(took >= 100, "after the write, not before");
    pthread_join(writer, NULL);
    read(p[0], got, 1);

    /* Several at once: only the ready ones say so. */
    int q[2];
    pipe(q);
    write(q[1], "y", 1);
    struct pollfd many[4] = {{p[0], POLLIN, 7}, {q[0], POLLIN, 7}, {-1, POLLIN, 7}, {29, POLLIN, 7}};
    int n = poll(many, 4, 0);
    check(n == 2, "four entries, two ready: the pipe with a byte, and the descriptor not open");
    check(many[0].revents == 0 && many[1].revents == POLLIN, "the right one is readable");
    check(many[2].revents == 0, "a negative descriptor is skipped");
    check(many[3].revents == POLLNVAL, "one not open: POLLNVAL");

    /* A signal ends the wait: EINTR. */
    struct sigaction action = {0};
    action.sa_handler = on_alarm;
    sigaction(SIGALRM, &action, NULL);
    alarm(1);
    errno = 0;
    n = poll(&(struct pollfd){p[0], POLLIN, 0}, 1, -1);
    check(n == -1 && errno == EINTR && alarms == 1, "a signal's handler ends poll: -1, EINTR");
    signal(SIGALRM, SIG_DFL);

    /* The ends closed. */
    close(q[1]);
    check(poll_one(q[0], POLLIN, 0) == POLLIN, "the writer gone, a byte still there: POLLIN");
    read(q[0], got, 1);
    check(poll_one(q[0], POLLIN, 0) == POLLHUP, "then POLLHUP, unasked");
    close(q[0]);
    close(p[0]);
    check(poll_one(p[1], POLLOUT, 0) == POLLERR, "the reader gone: the write end says POLLERR");
    close(p[1]);

    /* A file is always ready. */
    int file = open("pollfile", O_RDWR | O_CREAT | O_TRUNC, 0644);
    check(file >= 0 && poll_one(file, POLLIN | POLLOUT, -1) == (POLLIN | POLLOUT), "a file: POLLIN and POLLOUT at once");
    close(file);
    unlink("pollfile");

    /* select. */
    pipe(p);
    write(p[1], "z", 1);
    fd_set readable, writable;
    FD_ZERO(&readable);
    FD_ZERO(&writable);
    FD_SET(p[0], &readable);
    FD_SET(p[1], &writable);
    struct timeval none = {0, 0};
    n = select(p[1] + 1, &readable, &writable, NULL, &none);
    check(n == 2 && FD_ISSET(p[0], &readable) && FD_ISSET(p[1], &writable), "select: readable and writable");
    read(p[0], got, 1);
    FD_ZERO(&readable);
    FD_SET(p[0], &readable);
    struct timeval short_wait = {0, 30000};
    before = milliseconds();
    n = select(p[0] + 1, &readable, NULL, NULL, &short_wait);
    took = milliseconds() - before;
    if (n != 0 || FD_ISSET(p[0], &readable) || took < 20)
        printf("ground2: select gave %d, set %d, after %ld ms\n", n, (int)FD_ISSET(p[0], &readable), took);
    check(n == 0 && !FD_ISSET(p[0], &readable) && took >= 20, "select times out, its set emptied");
    FD_ZERO(&readable);
    FD_SET(28, &readable);
    errno = 0;
    check(select(29, &readable, NULL, NULL, &none) == -1 && errno == EBADF, "select on a descriptor not open: EBADF");

    /* Across processes: a child writes, after a while, on a pipe it
     * was handed as descriptor 3 and up. */
    char number[8];
    snprintf(number, sizeof number, "%d", p[1]);
    char *late[] = {"ground2", "sleep-write", number, NULL};
    pid_t child;
    int started = posix_spawnp(&child, "ground2", NULL, NULL, late, environ) == 0;
    close(p[1]);
    check(started && poll_one(p[0], POLLIN, 5000) == POLLIN, "poll wakes for a child's write on a pipe it inherited");
    int r = (int)read(p[0], got, 8);
    check(r == 4 && memcmp(got, "late", 4) == 0, "and the child's bytes are there");
    int status;
    waitpid(child, &status, 0);
    check(poll_one(p[0], POLLIN, 1000) == POLLHUP, "the child ended: POLLHUP");
    close(p[0]);
}

/* ---- dl_iterate_phdr, getauxval, dlfcn ------------------------------- */

__thread int thread_local_number = 5;

struct seen {
    int calls;
    int phnum;
    const void *phdr;
    int code_found;
    size_t tls_size;
    size_t modid;
    void *tls_data;
};

static int look(struct dl_phdr_info *info, size_t size, void *data)
{
    struct seen *seen = data;
    seen->calls++;
    seen->phnum = info->dlpi_phnum;
    seen->phdr = info->dlpi_phdr;
    seen->modid = info->dlpi_tls_modid;
    seen->tls_data = info->dlpi_tls_data;
    uintptr_t code = (uintptr_t)&look;
    for (int i = 0; i < info->dlpi_phnum; i++) {
        const ElfW(Phdr) *h = &info->dlpi_phdr[i];
        if (h->p_type == PT_LOAD && (h->p_flags & PF_X) && code >= info->dlpi_addr + h->p_vaddr
            && code < info->dlpi_addr + h->p_vaddr + h->p_memsz)
            seen->code_found = 1;
        if (h->p_type == PT_TLS)
            seen->tls_size = h->p_memsz;
    }
    return size >= sizeof(struct dl_phdr_info) ? 77 : 1;
}

static int holds_mine(const struct seen *seen)
{
    char *mine = (char *)&thread_local_number;
    return seen->tls_data != NULL && mine >= (char *)seen->tls_data && mine < (char *)seen->tls_data + seen->tls_size;
}

static void *look_from_a_thread(void *main_block)
{
    struct seen seen = {0};
    dl_iterate_phdr(look, &seen);
    return (void *)(intptr_t)(holds_mine(&seen) && seen.tls_data != main_block);
}

static void loading_checks(void)
{
    struct seen seen = {0};
    int r = dl_iterate_phdr(look, &seen);
    check(r == 77 && seen.calls == 1, "dl_iterate_phdr: one object, its callback's answer returned");
    check(seen.phnum >= 3 && seen.code_found, "its headers have the PT_LOAD this code is in");
    check(seen.tls_size > 0 && seen.modid == 1, "and the PT_TLS, module 1");
    check(holds_mine(&seen), "dlpi_tls_data is this thread's block: a thread-local lies in it");
    pthread_t thread;
    void *result = NULL;
    pthread_create(&thread, NULL, look_from_a_thread, seen.tls_data);
    pthread_join(thread, &result);
    check(result == (void *)1, "in another thread, that thread's block");

    check(getauxval(AT_PAGESZ) == 4096, "getauxval(AT_PAGESZ): 4096");
    check(getauxval(AT_PHNUM) == (unsigned long)seen.phnum && getauxval(AT_PHDR) == (unsigned long)seen.phdr,
          "AT_PHDR and AT_PHNUM: the same headers");
    check(getauxval(AT_PHENT) == sizeof(ElfW(Phdr)), "AT_PHENT: 56");
    const unsigned char *random = (const unsigned char *)getauxval(AT_RANDOM);
    int zeros = 0;
    for (int i = 0; random && i < 16; i++)
        zeros += random[i] == 0;
    check(random != NULL && zeros < 4 && getauxval(AT_RANDOM) == (unsigned long)random, "AT_RANDOM: 16 bytes, the same each time");
    errno = 0;
    check(getauxval(9999) == 0 && errno == ENOENT, "an unknown type: 0, ENOENT");

    void *self = dlopen(NULL, RTLD_NOW);
    check(self != NULL, "dlopen(NULL): the program");
    check(dlsym(self, "main") == NULL && dlerror() != NULL, "dlsym finds nothing, and dlerror says why");
    check(dlerror() == NULL, "dlerror says it once");
    check(dlopen("libm.so.6", RTLD_LAZY) == NULL && dlerror() != NULL, "dlopen of a library: NULL");
    check(dlclose(self) == 0, "dlclose: 0");
}

/* ---- pthread attributes ---------------------------------------------- */

struct stack_seen {
    void *address;
    size_t size, guard;
    int detached, inside;
    volatile int done;
};

static void *see_own_stack(void *data)
{
    struct stack_seen *seen = data;
    int local = 0;
    pthread_attr_t attributes;
    if (pthread_getattr_np(pthread_self(), &attributes) == 0) {
        pthread_attr_getstack(&attributes, &seen->address, &seen->size);
        pthread_attr_getguardsize(&attributes, &seen->guard);
        pthread_attr_getdetachstate(&attributes, &seen->detached);
        seen->inside = (char *)&local >= (char *)seen->address && (char *)&local < (char *)seen->address + seen->size;
        pthread_attr_destroy(&attributes);
    }
    seen->done = 1;
    return NULL;
}

static void attribute_checks(void)
{
    check(sizeof(pthread_attr_t) == 56, "pthread_attr_t: glibc's 56 bytes");
    pthread_attr_t a;
    pthread_attr_init(&a);
    void *address = (void *)1;
    size_t size = 0, guard = 0;
    pthread_attr_getstack(&a, &address, &size);
    pthread_attr_getguardsize(&a, &guard);
    check(address == NULL && size == 8 << 20 && guard == 4096, "a new one: no stack of its own, 8 MiB, a page of guard");
    check(pthread_attr_setstacksize(&a, 4096) == EINVAL, "a stack below PTHREAD_STACK_MIN: EINVAL");

    /* The main thread: its stack is the program's, at the top. */
    int local = 0;
    pthread_attr_t main_attributes;
    check(pthread_getattr_np(pthread_self(), &main_attributes) == 0, "pthread_getattr_np on the main thread");
    pthread_attr_getstack(&main_attributes, &address, &size);
    check(size == 8 << 20, "the main thread's stack: 8 MiB (no PT_GNU_STACK size)");
    check((char *)&local >= (char *)address && (char *)&local < (char *)address + size, "and a local of main lies in it");

    /* A thread with a size and a guard asked for. */
    struct stack_seen seen = {0};
    pthread_attr_setstacksize(&a, 256 * 1024);
    pthread_attr_setguardsize(&a, 8192);
    pthread_t thread;
    pthread_create(&thread, &a, see_own_stack, &seen);
    pthread_join(thread, NULL);
    check(seen.size == 256 * 1024 && seen.inside, "a thread of 256 KiB: its stack, with its local in it");
    check(seen.guard == 8192 && !seen.detached, "its guard size said back; joinable");

    /* A stack the program gives. */
    size_t given_size = 128 * 1024;
    char *given = aligned_alloc(4096, given_size);
    struct stack_seen on_given = {0};
    pthread_attr_t b;
    pthread_attr_init(&b);
    check(pthread_attr_setstack(&b, given, given_size) == 0, "pthread_attr_setstack");
    pthread_attr_getstack(&b, &address, &size);
    check(address == given && size == given_size, "pthread_attr_getstack says it back");
    if (given && pthread_create(&thread, &b, see_own_stack, &on_given) == 0)
        pthread_join(thread, NULL);
    check(on_given.address == given && on_given.size == given_size && on_given.inside, "the thread ran on the given stack");
    free(given);

    /* Detached. */
    struct stack_seen loose = {0};
    pthread_attr_t c;
    pthread_attr_init(&c);
    pthread_attr_setdetachstate(&c, PTHREAD_CREATE_DETACHED);
    pthread_create(&thread, &c, see_own_stack, &loose);
    for (int i = 0; i < 200 && !loose.done; i++)
        sleep_ms(5);
    check(loose.done && loose.detached == 1, "a detached thread's attributes say so");
    errno = 0;
    check(pthread_getattr_np((pthread_t)12345, &a) == ESRCH, "a thread that is not one: ESRCH");
}

/* ---- exec, PATH, descriptors, argv[0] -------------------------------- */

static void exec_checks(void)
{
    check(exited_with(run((char *[]){"exec-to", "exit-with", "7", NULL}), 7), "execvp: the new program's exit code is this one's");
    int status = run((char *[]){"exec-to", "raise", "15", NULL});
    check(status != -1 && WIFSIGNALED(status) && WTERMSIG(status) == SIGTERM, "and its signal: killed by SIGTERM");
    check(exited_with(run((char *[]){"exec-env", NULL}), 42), "execle: the new program gets the environment given");

    char *nothing[] = {"no-such-program", NULL};
    /* This program's input is the keyboard, which cannot be handed on:
     * exec starts the program without it, and the error is the
     * program's. */
    errno = 0;
    int r = execvp("no-such-program", nothing);
    check(r == -1 && errno == ENOENT, "execvp of nothing: -1, ENOENT - and this program goes on");
    errno = 0;
    check(execl("/no-such-program", "x", (char *)NULL) == -1 && errno == ENOENT, "execl of nothing: the same");

    /* A pipe's write end at 3 and up, kept through spawn and exec. */
    int p[2];
    pipe(p);
    char number[8];
    snprintf(number, sizeof number, "%d", p[1]);
    status = run((char *[]){"exec-to", "write-fd", number, NULL});
    close(p[1]);
    char got[32] = {0};
    int n = (int)read(p[0], got, sizeof got - 1);
    check(exited_with(status, 0) && n == 12 && strcmp(got, "through exec") == 0, "a pipe at 3 and up reaches the program exec started");
    close(p[0]);

    /* Close-on-exec, set by the program that execs. */
    pipe(p);
    snprintf(number, sizeof number, "%d", p[1]);
    status = run((char *[]){"exec-closing", number, "fd-closed", number, NULL});
    check(exited_with(status, 0), "one marked FD_CLOEXEC before exec does not");
    close(p[0]);
    close(p[1]);

    /* PATH. */
    setenv("PATH", "/nowhere:/", 1);
    check(exited_with(run((char *[]){"exec-to", "exit-with", "3", NULL}), 3), "PATH=/nowhere:/ : found in the second");
    setenv("PATH", "/nowhere", 1);
    pid_t child;
    char *argv[] = {"ground2", "exit-with", "0", NULL};
    check(posix_spawnp(&child, "ground2", NULL, NULL, argv, environ) == ENOENT, "PATH=/nowhere : posix_spawnp says ENOENT");
    unsetenv("PATH");
    check(exited_with(run((char *[]){"exit-with", "4", NULL}), 4), "no PATH: the root, as before");

    /* argv[0] is what the parent said. */
    check(exited_with(run_as("custom-name", (char *[]){"argv0", NULL}, environ), 0), "argv[0] is the name the parent gave");
}

/* ---- stat, realpath, getcwd ------------------------------------------ */

static void file_checks(void)
{
    struct stat s;
    check(stat("ground2", &s) == 0 && S_ISREG(s.st_mode) && s.st_size > 10000, "stat: this program is a file");
    char where[256];
    check(getcwd(where, sizeof where) != NULL && strcmp(where, "/") == 0, "getcwd: /");
    errno = 0;
    char *real = realpath("no-such-file", NULL);
    check(real == NULL && errno == ENOENT, "realpath of nothing: ENOENT");
    real = realpath("./ground2", NULL);
    check(real != NULL && strcmp(real, "/ground2") == 0, "realpath: /ground2");
    free(real);

    /* Directories as descriptors: only with O_DIRECTORY. */
    errno = 0;
    check(open("/", O_RDONLY) == -1 && errno == EISDIR, "open of a directory without O_DIRECTORY: EISDIR, as before");
    errno = 0;
    check(open("ground2", O_RDONLY | O_DIRECTORY) == -1 && errno == ENOTDIR, "a file with O_DIRECTORY: ENOTDIR");
    mkdir("gdir", 0755);
    int fd = open("gdir", O_RDONLY | O_DIRECTORY);
    check(fd >= 0, "a directory with O_DIRECTORY opens");
    char byte;
    errno = 0;
    check(read(fd, &byte, 1) == -1 && errno == EISDIR, "reading it: EISDIR");
    check(fchdir(fd) == 0 && getcwd(where, sizeof where) && strcmp(where, "/gdir") == 0, "fchdir into it");
    chdir("/");
    int file = open("gdir/inside", O_WRONLY | O_CREAT, 0644);
    close(file);
    DIR *dir = fdopendir(fd);
    int seen_inside = 0;
    struct dirent *entry;
    while (dir && (entry = readdir(dir)) != NULL)
        seen_inside |= strcmp(entry->d_name, "inside") == 0;
    check(dir != NULL && seen_inside && dirfd(dir) == fd, "fdopendir lists it; dirfd is the descriptor");
    closedir(dir);
    errno = 0;
    check(close(fd) == -1 && errno == EBADF, "closedir closed the descriptor");
    unlink("gdir/inside");
    rmdir("gdir");
}

/* ---- zlib ------------------------------------------------------------ */

static void zlib_checks(void)
{
    check(strcmp(zlibVersion(), "1.3.2") == 0, "zlib 1.3.2");
    check(crc32(0, (const Bytef *)"123456789", 9) == 0xCBF43926, "crc32 of 123456789: CBF43926");
    check(adler32(1, (const Bytef *)"Wikipedia", 9) == 0x11E60398, "adler32 of Wikipedia: 11E60398");

    uLong size = 200000;
    Bytef *text = malloc(size), *packed = malloc(compressBound(size)), *back = malloc(size);
    if (!text || !packed || !back) {
        check(0, "memory for zlib");
        return;
    }
    for (uLong i = 0; i < size; i++)
        text[i] = "the quick brown fox jumps over the lazy dog "[i % 44] ^ (i % 997 == 0);
    uLongf packed_size = compressBound(size);
    check(compress2(packed, &packed_size, text, size, 9) == Z_OK, "compress2");
    check(packed_size < size / 20, "repeated text packs to under a twentieth");
    uLongf back_size = size;
    check(uncompress(back, &back_size, packed, packed_size) == Z_OK && back_size == size && memcmp(back, text, size) == 0,
          "uncompress gives it back");
    packed[packed_size / 2] ^= 0x55;
    back_size = size;
    check(uncompress(back, &back_size, packed, packed_size) != Z_OK, "damaged, uncompress says so");

    /* gzip files, through open/read/write. */
    gzFile out = gzopen("zt.gz", "wb");
    check(out != NULL && gzwrite(out, text, (unsigned)size) == (int)size && gzclose(out) == Z_OK, "gzwrite a file");
    FILE *raw = fopen("zt.gz", "rb");
    unsigned char magic[2] = {0};
    if (raw) {
        fread(magic, 1, 2, raw);
        fclose(raw);
    }
    check(magic[0] == 0x1f && magic[1] == 0x8b, "it starts as gzip does: 1f 8b");
    gzFile in = gzopen("zt.gz", "rb");
    memset(back, 0, size);
    check(in != NULL && gzread(in, back, (unsigned)size) == (int)size && memcmp(back, text, size) == 0, "gzread it back");
    check(in != NULL && gzread(in, back, 1) == 0 && gzeof(in), "then the end");
    if (in)
        gzclose(in);
    unlink("zt.gz");
    free(text);
    free(packed);
    free(back);
}

/* ---- the children's parts -------------------------------------------- */

static int child(int argc, char **argv)
{
    const char *what = argv[1];
    if (strcmp(what, "exit-with") == 0 && argc > 2)
        return atoi(argv[2]);
    if (strcmp(what, "raise") == 0 && argc > 2) {
        raise(atoi(argv[2]));
        return 99;
    }
    if (strcmp(what, "env-answer") == 0) {
        const char *answer = getenv("ANSWER");
        return answer && environ[0] && !environ[1] ? atoi(answer) : 98;
    }
    if (strcmp(what, "argv0") == 0)
        return strcmp(argv[0], "custom-name") == 0 ? 0 : 1;
    if (strcmp(what, "write-fd") == 0 && argc > 2)
        return write(atoi(argv[2]), "through exec", 12) == 12 ? 0 : 1;
    if (strcmp(what, "fd-closed") == 0 && argc > 2) {
        errno = 0;
        return write(atoi(argv[2]), "x", 1) == -1 && errno == EBADF ? 0 : 1;
    }
    if (strcmp(what, "sleep-write") == 0 && argc > 2) {
        sleep_ms(200);
        return write(atoi(argv[2]), "late", 4) == 4 ? 0 : 1;
    }
    if (strcmp(what, "exec-to") == 0 && argc > 2) {
        argv[1] = "ground2";
        execvp("ground2", argv + 1);
        return 97;
    }
    if (strcmp(what, "exec-closing") == 0 && argc > 3) {
        fcntl(atoi(argv[2]), F_SETFD, FD_CLOEXEC);
        argv[2] = "ground2";
        execvp("ground2", argv + 2);
        return 97;
    }
    if (strcmp(what, "exec-env") == 0) {
        execle("/ground2", "ground2", "env-answer", (char *)NULL, (char *[]){"ANSWER=42", NULL});
        return 97;
    }
    return 96;
}

int main(int argc, char **argv)
{
    if (argc > 1)
        return child(argc, argv);
    poll_checks();
    loading_checks();
    attribute_checks();
    exec_checks();
    file_checks();
    zlib_checks();
    if (failures == 0)
        printf("ground2: %d checks, all passed\n", checks);
    else
        printf("ground2: %d of %d checks failed\n", failures, checks);
    return failures != 0;
}
