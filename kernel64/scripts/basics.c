/* basics.c - the core basics (journal entry 097): the date, a clock
 * finer than a tick, signals from the kernel, and files that reach the
 * disk. Checked on TheTaaJ by basics-test.sh:
 *
 *   with programs basics
 *                       the checks; "basics: N checks, all passed":
 *                       the date from the real-time clock, the clock to
 *                       the nanosecond, nanosleep never short; handlers
 *                       run by raise, by an alarm while the program
 *                       computes (every register as it was after), by
 *                       an alarm while it waits in read (which then
 *                       goes on) and in pause, sigsuspend and nanosleep
 *                       (EINTR); a blocked signal waits; SIGSEGV caught;
 *                       children ended by a signal, or catching it, or
 *                       by abort, as waitpid tells; a file written,
 *                       fsync'd and read back
 *   basics catch        waits for a line, and catches Ctrl+C (SIGINT):
 *                       "basics: caught SIGINT while waiting for a line"
 *   basics date         the date and time, UTC, and the time since boot
 *   basics speed        writes 4 MiB in 64 KiB writes - each one a
 *                       commit, with its two disk flushes - and says how
 *                       fast
 *   basics child-*      what the checks start as children */

#include <errno.h>
#include <fcntl.h>
#include <setjmp.h>
#include <signal.h>
#include <spawn.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

extern char **environ;

static int checks, failures;

static void check(int held, const char *what)
{
    checks++;
    if (!held) {
        failures++;
        printf("basics: failed: %s\n", what);
    }
}

static int64_t nanoseconds(clockid_t clock)
{
    struct timespec t;
    clock_gettime(clock, &t);
    return (int64_t)t.tv_sec * 1000000000 + t.tv_nsec;
}

/* ---- the clock ---- */

static void clock_checks(void)
{
    time_t now = time(NULL);
    /* 1 January 2025: a clock that knows the date, not one counting
     * from boot in 1970. */
    check(now > 1735689600, "time() is today's date, not the time since boot");
    struct tm *utc = gmtime(&now);
    check(utc && utc->tm_year + 1900 >= 2025, "gmtime of it is this year");
    check(llabs(nanoseconds(CLOCK_REALTIME) / 1000000000 - now) <= 1, "time() and CLOCK_REALTIME agree");

    /* Finer than a tick (10 ms): consecutive readings a few
     * microseconds apart, not stuck on one value for 10 ms. */
    int64_t last = nanoseconds(CLOCK_MONOTONIC), smallest = INT64_MAX;
    int backwards = 0;
    for (int n = 0; n < 1000; n++) {
        int64_t reading = nanoseconds(CLOCK_MONOTONIC);
        backwards += reading < last;
        if (reading > last && reading - last < smallest)
            smallest = reading - last;
        last = reading;
    }
    check(backwards == 0, "CLOCK_MONOTONIC never goes back, 1000 times");
    check(smallest < 1000000, "CLOCK_MONOTONIC moves on by less than a millisecond");

    /* The time of day is boot's plus the time since: the two clocks
     * move together. */
    int64_t boot = nanoseconds(CLOCK_REALTIME) - nanoseconds(CLOCK_MONOTONIC);
    struct timespec nap = {0, 50000000};
    nanosleep(&nap, NULL);
    int64_t later = nanoseconds(CLOCK_REALTIME) - nanoseconds(CLOCK_MONOTONIC);
    check(llabs(later - boot) < 2000000, "the time of day and the time since boot move together");

    /* nanosleep is never short - however close to a tick it starts. */
    int short_sleeps = 0;
    for (int n = 0; n < 20; n++) {
        struct timespec twenty = {0, 20000000};
        int64_t before = nanoseconds(CLOCK_MONOTONIC);
        nanosleep(&twenty, NULL);
        if (nanoseconds(CLOCK_MONOTONIC) - before < 20000000)
            short_sleeps++;
    }
    check(short_sleeps == 0, "nanosleep of 20 ms sleeps at least 20 ms, 20 times");
    char text[64];
    strftime(text, sizeof text, "%Y-%m-%d %H:%M:%S", utc);
    printf("basics: the date is %s UTC\n", text);
}

/* ---- signals in this program ---- */

static volatile sig_atomic_t caught, alarms, faults;
static int pipe_ends[2];

static void on_signal(int signal)
{
    caught = signal;
}

static void on_alarm(int signal)
{
    (void)signal;
    alarms++;
    /* Floating point and plenty of integer work, to spoil the
     * registers of whatever was interrupted - if they were not saved. */
    volatile double spoil = 1.0;
    for (int n = 0; n < 1000; n++)
        spoil = spoil * 1.5 + n;
    char byte = 'a';
    write(pipe_ends[1], &byte, 1);
}

static sigjmp_buf back;

static void on_fault(int signal, siginfo_t *info, void *context)
{
    (void)info;
    faults = signal;
    /* The context is the kernel's abi::signal::Context: 31 registers,
     * then the stack pointer and the program counter. */
    uint64_t *words = context;
    if (words && words[32] != 0)
        faults += 100;
    siglongjmp(back, 1);
}

/* Computes until an alarm comes - between two instructions, from a
 * timer interrupt, not at a system call - then does the same steps
 * again to see that every register came back as it was. */
static void computing_through_an_alarm(void)
{
    signal(SIGALRM, on_alarm);
    alarms = 0;
    alarm(1);
    uint64_t a = 1, steps = 0;
    double x = 1.0, y = 0.25;
    while (!alarms) {
        a = a * 6364136223846793005ULL + 1442695040888963407ULL;
        x = x * 1.0000001 + y;
        y = y * 0.999 + 0.001;
        steps++;
    }
    uint64_t again = 1;
    double x2 = 1.0, y2 = 0.25;
    for (uint64_t n = 0; n < steps; n++) {
        again = again * 6364136223846793005ULL + 1442695040888963407ULL;
        x2 = x2 * 1.0000001 + y2;
        y2 = y2 * 0.999 + 0.001;
    }
    check(alarms == 1, "an alarm while computing ran its handler once");
    check(a == again && x == x2 && y == y2, "every register, integer and floating point, as it was after the handler");
    char byte;
    read(pipe_ends[0], &byte, 1); /* what the handler wrote */
}

static void signal_checks(void)
{
    check(pipe(pipe_ends) == 0, "a pipe");

    /* raise: the handler runs before raise returns. */
    caught = 0;
    check(signal(SIGUSR1, on_signal) == SIG_DFL && raise(SIGUSR1) == 0 && caught == SIGUSR1, "raise runs the handler");
    check(raise(SIGCHLD) == 0, "SIGCHLD is ignored by default");

    /* Blocked: it waits, once however often it came, until unblocked. */
    sigset_t usr1, waiting;
    sigemptyset(&usr1);
    sigaddset(&usr1, SIGUSR1);
    sigprocmask(SIG_BLOCK, &usr1, NULL);
    caught = 0;
    raise(SIGUSR1);
    raise(SIGUSR1);
    sigpending(&waiting);
    check(caught == 0 && sigismember(&waiting, SIGUSR1), "a blocked signal waits");
    sigprocmask(SIG_UNBLOCK, &usr1, NULL);
    check(caught == SIGUSR1, "and comes when unblocked");

    computing_through_an_alarm();

    /* An alarm while waiting in read: the handler runs - it writes to
     * the very pipe being read - and the read goes on, and gets that. */
    alarms = 0;
    alarm(1);
    char byte = 0;
    int64_t before = nanoseconds(CLOCK_MONOTONIC);
    ssize_t got = read(pipe_ends[0], &byte, 1);
    int64_t waited = nanoseconds(CLOCK_MONOTONIC) - before;
    check(got == 1 && byte == 'a' && alarms == 1, "a read interrupted by a handler goes on, and reads what it wrote");
    check(waited > 900000000 && waited < 3000000000LL, "the alarm came after a second");

    /* pause and sigsuspend: until a handler has run, then EINTR. */
    alarms = 0;
    alarm(1);
    check(pause() == -1 && errno == EINTR && alarms == 1, "pause returns after the handler, with EINTR");
    read(pipe_ends[0], &byte, 1);
    sigset_t alrm, none;
    sigemptyset(&alrm);
    sigaddset(&alrm, SIGALRM);
    sigemptyset(&none);
    sigprocmask(SIG_BLOCK, &alrm, NULL);
    alarms = 0;
    alarm(1);
    check(sigsuspend(&none) == -1 && errno == EINTR && alarms == 1, "sigsuspend lets a blocked signal in, and returns");
    sigset_t now;
    sigprocmask(SIG_SETMASK, NULL, &now);
    check(sigismember(&now, SIGALRM), "and the mask is as it was");
    sigprocmask(SIG_UNBLOCK, &alrm, NULL);
    read(pipe_ends[0], &byte, 1);

    /* nanosleep cut short: EINTR, and what was left. */
    alarms = 0;
    alarm(1);
    struct timespec five = {5, 0}, left = {0, 0};
    check(nanosleep(&five, &left) == -1 && errno == EINTR && alarms == 1, "nanosleep interrupted: EINTR");
    check(left.tv_sec >= 3 && left.tv_sec <= 4, "with about 4 s left");
    read(pipe_ends[0], &byte, 1);

    /* alarm says what the one before had left; 0 cancels. */
    alarm(10);
    check(alarm(0) == 10, "alarm returns the seconds the one before had left");

    /* A fault, caught: the handler runs, with the context. */
    struct sigaction fault = {0};
    fault.sa_sigaction = on_fault;
    fault.sa_flags = SA_SIGINFO;
    sigaction(SIGSEGV, &fault, NULL);
    faults = 0;
    if (sigsetjmp(back, 1) == 0)
        *(volatile int *)16 = 1;
    check(faults == 100 + SIGSEGV, "a bad pointer is SIGSEGV, caught, with the context");
    sigset_t after;
    sigprocmask(SIG_SETMASK, NULL, &after);
    check(!sigismember(&after, SIGSEGV), "siglongjmp out of the handler unblocks SIGSEGV again");
    signal(SIGSEGV, SIG_DFL);
}

/* ---- children ---- */

/* Starts `basics child-NAME` with its standard output into a pipe, and
 * waits for its first byte: it is ready. Returns its number. */
static pid_t start(const char *name, int *output)
{
    int ends[2];
    pipe(ends);
    posix_spawn_file_actions_t actions;
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_adddup2(&actions, ends[1], 1);
    char *argv[] = {"basics", (char *)name, NULL};
    pid_t child = -1;
    int error = posix_spawnp(&child, "basics", &actions, NULL, argv, environ);
    posix_spawn_file_actions_destroy(&actions);
    close(ends[1]);
    char ready = 0;
    if (error != 0 || read(ends[0], &ready, 1) != 1 || ready != 'r') {
        printf("basics: %s did not start\n", name);
        return -1;
    }
    *output = ends[0];
    return child;
}

static int finish(pid_t child, int output)
{
    int status = 0;
    waitpid(child, &status, 0);
    close(output);
    return status;
}

static void children_checks(void)
{
    int output, status;
    pid_t child = start("child-wait", &output);
    check(child > 0 && kill(child, 0) == 0, "kill(child, 0): it is there");
    kill(child, SIGTERM);
    status = finish(child, output);
    check(WIFSIGNALED(status) && WTERMSIG(status) == SIGTERM, "a child that does not catch SIGTERM is ended by it");

    child = start("child-catch", &output);
    kill(child, SIGTERM);
    char said = 0;
    read(output, &said, 1);
    status = finish(child, output);
    check(said == 't' && WIFEXITED(status) && WEXITSTATUS(status) == 7, "one that catches it runs its handler, and exits as it chooses");

    child = start("child-abort", &output);
    read(output, &said, 1);
    status = finish(child, output);
    check(said == 'h' && WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT, "abort: its handler runs, and SIGABRT ends it");

    child = start("child-segv", &output);
    status = finish(child, output);
    check(WIFSIGNALED(status) && WTERMSIG(status) == SIGSEGV, "a bad pointer not caught: SIGSEGV ends it");

    check(kill(getpid() + 1000, SIGTERM) == -1 && errno == EPERM, "kill of a process not its own: EPERM");
}

static void on_term(int signal)
{
    (void)signal;
    write(1, "t", 1);
    _exit(7);
}

static void on_abort(int signal)
{
    (void)signal;
    write(1, "h", 1);
}

static int child(const char *what)
{
    if (strcmp(what, "child-catch") == 0)
        signal(SIGTERM, on_term);
    if (strcmp(what, "child-abort") == 0) {
        signal(SIGABRT, on_abort);
        write(1, "r", 1);
        abort();
    }
    write(1, "r", 1);
    if (strcmp(what, "child-segv") == 0)
        *(volatile int *)16 = 1;
    for (;;)
        pause();
}

/* ---- files ---- */

static void file_checks(void)
{
    static char data[256 * 1024], back_in[256 * 1024];
    for (size_t n = 0; n < sizeof data; n++)
        data[n] = (char)(n * 7 + n / 4096);
    int fd = open("basics-file", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    check(fd >= 0 && write(fd, data, sizeof data) == (ssize_t)sizeof data, "a file written");
    check(fsync(fd) == 0 && close(fd) == 0, "fsync'd and closed");
    fd = open("basics-file", O_RDONLY);
    ssize_t got = 0, n;
    while (fd >= 0 && (n = read(fd, back_in + got, sizeof back_in - got)) > 0)
        got += n;
    close(fd);
    check(got == (ssize_t)sizeof data && memcmp(data, back_in, sizeof data) == 0, "and read back the same");
    unlink("basics-file");
}

/* ---- basics catch, basics speed ---- */

static void on_interrupt(int signal)
{
    (void)signal;
    static const char said[] = "basics: caught SIGINT while waiting for a line - tidying up\n";
    write(1, said, sizeof said - 1);
    _exit(0);
}

static int catch_interrupt(void)
{
    signal(SIGINT, on_interrupt);
    printf("basics: waiting for a line (Ctrl+C to interrupt)\n");
    fflush(stdout);
    char line[100];
    if (fgets(line, sizeof line, stdin))
        printf("basics: read a line, no Ctrl+C\n");
    return 1;
}

static int speed(void)
{
    static char chunk[64 * 1024];
    memset(chunk, 'x', sizeof chunk);
    int fd = open("basics-speed", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        printf("basics: cannot write here (with files basics speed)\n");
        return 1;
    }
    int64_t before = nanoseconds(CLOCK_MONOTONIC);
    for (int n = 0; n < 64; n++)
        write(fd, chunk, sizeof chunk);
    close(fd);
    int64_t took = nanoseconds(CLOCK_MONOTONIC) - before;
    unlink("basics-speed");
    printf("basics: 4 MiB in 64 writes took %lld ms: %lld KiB/s\n", (long long)(took / 1000000),
           (long long)(4096LL * 1000000000 / (took ? took : 1)));
    return 0;
}

static int date(void)
{
    time_t now = time(NULL);
    char text[64];
    strftime(text, sizeof text, "%A %d %B %Y, %H:%M:%S UTC", gmtime(&now));
    int64_t up = nanoseconds(CLOCK_MONOTONIC);
    printf("%s; up %lld.%03lld s\n", text, (long long)(up / 1000000000), (long long)(up / 1000000 % 1000));
    return 0;
}

int main(int argc, char **argv)
{
    if (argc > 1 && strncmp(argv[1], "child-", 6) == 0)
        return child(argv[1]);
    if (argc > 1 && strcmp(argv[1], "catch") == 0)
        return catch_interrupt();
    if (argc > 1 && strcmp(argv[1], "date") == 0)
        return date();
    if (argc > 1 && strcmp(argv[1], "speed") == 0)
        return speed();
    clock_checks();
    signal_checks();
    children_checks();
    file_checks();
    if (failures == 0)
        printf("basics: %d checks, all passed\n", checks);
    else
        printf("basics: %d of %d checks failed\n", failures, checks);
    return failures != 0;
}
