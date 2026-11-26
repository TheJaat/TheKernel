/* ground.c - std's ground, part 1 (The-Jat's step B): random numbers,
 * real thread-local storage, signal stacks, and SIGCHLD with waitid.
 * ground-test.sh boots TheTaaJ and runs it:
 *
 *   ground random            16 random bytes, in hex: two boots must differ
 *   with programs ground     the checks; "ground: N checks, all passed"
 *
 * and these, which the checks start as children:
 *   ground child-exit        exits with code 5
 *   ground child-sleep       sleeps 300 ms, exits 0
 *   ground child-wait        waits to be killed
 *   ground overflow          overflows its stack with a SIGSEGV handler on
 *                            a signal stack: the handler says so, exit 3
 *   ground overflow-plain    the same with no signal stack: killed */

#include <errno.h>
#include <pthread.h>
#include <setjmp.h>
#include <signal.h>
#include <spawn.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
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
        printf("ground: failed: %s\n", what);
    }
}

/* ---- random numbers ------------------------------------------------ */

static int compare_blocks(const void *a, const void *b)
{
    return memcmp(a, b, 16);
}

static void random_checks(void)
{
    unsigned char a[64], b[64];
    check(getrandom(a, sizeof a, 0) == (ssize_t)sizeof a, "getrandom gives the bytes asked for");
    check(getrandom(b, sizeof b, GRND_NONBLOCK) == (ssize_t)sizeof b, "getrandom with GRND_NONBLOCK");
    check(memcmp(a, b, sizeof a) != 0, "two getrandom calls give different bytes");

    /* A megabyte: half its bits ones, every byte value about as common
     * as every other, and no 16-byte block twice. Not a proof of
     * randomness - a check that nothing is stuck or repeating. */
    size_t size = 1 << 20;
    unsigned char *mega = malloc(size);
    check(mega && getrandom(mega, size, 0) == (ssize_t)size, "getrandom fills a megabyte");
    if (mega) {
        long ones = 0, counts[256] = {0};
        for (size_t i = 0; i < size; i++) {
            ones += __builtin_popcount(mega[i]);
            counts[mega[i]]++;
        }
        long bits = (long)size * 8, off = ones * 2 - bits; /* 0 if exactly half */
        if (off < 0)
            off = -off;
        /* The standard deviation of `off` is sqrt(bits), 2896: 20 times that. */
        check(off < 60000, "half the bits of a megabyte are ones");
        long expected = (long)size / 256, chi = 0;
        for (int i = 0; i < 256; i++)
            chi += (counts[i] - expected) * (counts[i] - expected);
        chi /= expected;
        /* 255 degrees of freedom: about 255, rarely above 330; 450 is
         * far into the tail. */
        check(chi < 450, "every byte value about equally common (chi-squared)");
        qsort(mega, size / 16, 16, compare_blocks);
        int repeated = 0;
        for (size_t i = 16; i < size; i += 16)
            repeated |= memcmp(mega + i - 16, mega + i, 16) == 0;
        check(!repeated, "no 16-byte block of a megabyte twice");
        free(mega);
    }

    unsigned char most[257];
    check(getentropy(most, 256) == 0, "getentropy of 256 bytes");
    errno = 0;
    check(getentropy(most, 257) == -1 && errno == EIO, "getentropy of 257 bytes: EIO");
    errno = 0;
    check(getrandom(a, 8, 0x100) == -1 && errno == EINVAL, "getrandom with an unknown flag: EINVAL");
    errno = 0;
    check(getrandom(NULL, 8, 0) == -1 && errno == EFAULT, "getrandom into nothing: EFAULT");
    check(getrandom(a, 0, 0) == 0, "getrandom of no bytes");

    int seen[6] = {0}, bad = 0;
    for (int i = 0; i < 6000; i++) {
        uint32_t n = arc4random_uniform(6);
        if (n < 6)
            seen[n]++;
        else
            bad = 1;
    }
    int even = !bad;
    for (int i = 0; i < 6; i++)
        even &= seen[i] > 800 && seen[i] < 1200;
    check(even, "arc4random_uniform(6): each of 0-5 about 1000 times in 6000");
    check(arc4random_uniform(1) == 0 && arc4random_uniform(0) == 0, "arc4random_uniform below 2 is 0");
    uint32_t x = arc4random(), y = arc4random(), z = arc4random();
    check(!(x == y && y == z), "arc4random changes");
}

/* ---- thread-local storage ------------------------------------------ */

/* In .tdata: the template's initial bytes. */
static _Thread_local int answer = 42;
static _Thread_local char name[16] = "template";
/* In .tbss: zeros. */
static _Thread_local long counter;
static _Thread_local long big[512];
/* Aligned beyond the TCB's 16 bytes: the area must honour it. */
static _Thread_local _Alignas(64) char aligned[100];

struct tls_seen {
    int answer;
    char name[16];
    long counter, big_sum;
    uintptr_t aligned_at, answer_at;
    int kept;
    int errno_kept;
};

static void *tls_thread(void *argument)
{
    struct tls_seen *seen = argument;
    seen->answer = answer;
    memcpy(seen->name, name, sizeof name);
    seen->counter = counter;
    long sum = 0;
    for (int i = 0; i < 512; i++)
        sum += big[i];
    seen->big_sum = sum;
    seen->aligned_at = (uintptr_t)aligned;
    seen->answer_at = (uintptr_t)&answer;
    /* Its own copies, changed while the others change theirs. */
    long mine = (long)(uintptr_t)argument;
    answer = (int)(mine & 0x7fff);
    counter = mine;
    big[511] = mine;
    errno = (int)(mine & 0xff);
    for (int i = 0; i < 20; i++) {
        struct timespec pause = {0, 1000000};
        nanosleep(&pause, NULL);
    }
    seen->kept = answer == (int)(mine & 0x7fff) && counter == mine && big[511] == mine;
    seen->errno_kept = errno == (int)(mine & 0xff);
    return NULL;
}

static void tls_checks(void)
{
    check(answer == 42 && strcmp(name, "template") == 0, "the main thread's thread-locals start as the template says");
    check(counter == 0 && big[100] == 0, "the main thread's .tbss starts at zero");
    check((uintptr_t)aligned % 64 == 0, "a 64-byte aligned thread-local is 64-byte aligned");
    /* Changed here before the threads start: they must not see it. */
    answer = 7;
    strcpy(name, "main");
    counter = -1;

    enum { THREADS = 4 };
    pthread_t threads[THREADS];
    struct tls_seen seen[THREADS];
    memset(seen, 0, sizeof seen);
    int started = 1;
    for (int i = 0; i < THREADS; i++)
        started &= pthread_create(&threads[i], NULL, tls_thread, &seen[i]) == 0;
    check(started, "four threads started");
    for (int i = 0; i < THREADS; i++)
        if (started)
            pthread_join(threads[i], NULL);
    int fresh = 1, zeros = 1, alignment = 1, kept = 1, own_errno = 1, distinct = 1;
    for (int i = 0; i < THREADS; i++) {
        fresh &= seen[i].answer == 42 && strcmp(seen[i].name, "template") == 0;
        zeros &= seen[i].counter == 0 && seen[i].big_sum == 0;
        alignment &= seen[i].aligned_at % 64 == 0;
        kept &= seen[i].kept;
        own_errno &= seen[i].errno_kept;
        for (int j = 0; j < i; j++)
            distinct &= seen[i].answer_at != seen[j].answer_at;
        distinct &= seen[i].answer_at != (uintptr_t)&answer;
    }
    check(fresh, "a new thread's thread-locals start from the template, not the main thread's values");
    check(zeros, "a new thread's .tbss is zeros");
    check(alignment, "each thread's 64-byte aligned thread-local is aligned");
    check(kept, "each thread keeps its own values while the others change theirs");
    check(own_errno, "each thread keeps its own errno");
    check(distinct, "every thread's thread-local is at its own address");
    check(answer == 7 && strcmp(name, "main") == 0 && counter == -1, "the main thread's values are untouched by the threads");
}

/* ---- signal stacks ------------------------------------------------- */

static char signal_stack_memory[3][SIGSTKSZ] __attribute__((aligned(16)));
static volatile uintptr_t handler_at;
static volatile int handler_flags, handler_change;

static void on_usr1(int signal)
{
    (void)signal;
    char here;
    handler_at = (uintptr_t)&here;
    stack_t now;
    sigaltstack(NULL, &now);
    handler_flags = now.ss_flags;
    /* Changing it while on it: refused. */
    stack_t other = {signal_stack_memory[1], 0, SIGSTKSZ};
    errno = 0;
    handler_change = sigaltstack(&other, NULL) == -1 && errno == EPERM;
}

static int on_stack(uintptr_t at, int which)
{
    uintptr_t base = (uintptr_t)signal_stack_memory[which];
    return at >= base && at < base + SIGSTKSZ;
}

/* Recursion that cannot be turned into a loop: each frame keeps a
 * kilobyte it uses after the call. */
static volatile int bottomless = 1;

static int deeper(int depth)
{
    volatile char frame[1024];
    frame[0] = (char)depth;
    frame[1023] = (char)depth;
    if (!bottomless) /* always 1: the compiler can't know */
        return 0;
    return deeper(depth + 1) + frame[0] + frame[1023];
}

static sigjmp_buf thread_back;
static volatile uintptr_t overflow_at;

static void on_overflow(int signal, siginfo_t *info, void *context)
{
    (void)info;
    (void)context;
    char here;
    overflow_at = (uintptr_t)&here;
    if (signal == SIGSEGV)
        siglongjmp(thread_back, 1);
}

/* A thread with a small stack and a signal stack of its own overflows;
 * the handler jumps back out, and the thread - and the program - go on. */
static void *overflowing_thread(void *argument)
{
    (void)argument;
    stack_t mine = {signal_stack_memory[2], 0, SIGSTKSZ};
    if (sigaltstack(&mine, NULL) != 0)
        return (void *)1;
    if (sigsetjmp(thread_back, 1) == 0) {
        deeper(0);
        return (void *)2; /* not reached */
    }
    return on_stack(overflow_at, 2) ? (void *)0 : (void *)3;
}

static void signal_stack_checks(void)
{
    stack_t now;
    check(sigaltstack(NULL, &now) == 0 && now.ss_flags == SS_DISABLE, "no signal stack at first");
    stack_t small = {signal_stack_memory[0], 0, 1024};
    errno = 0;
    check(sigaltstack(&small, NULL) == -1 && errno == ENOMEM, "a signal stack below MINSIGSTKSZ: ENOMEM");
    stack_t odd = {signal_stack_memory[0], 0x40, SIGSTKSZ};
    errno = 0;
    check(sigaltstack(&odd, NULL) == -1 && errno == EINVAL, "an unknown flag: EINVAL");

    stack_t mine = {signal_stack_memory[0], 0, SIGSTKSZ};
    check(sigaltstack(&mine, NULL) == 0, "a signal stack");
    check(sigaltstack(NULL, &now) == 0 && now.ss_sp == signal_stack_memory[0] && now.ss_size == SIGSTKSZ && now.ss_flags == 0,
          "sigaltstack says what it was given");

    struct sigaction action = {0};
    action.sa_handler = on_usr1;
    action.sa_flags = SA_ONSTACK;
    sigaction(SIGUSR1, &action, NULL);
    raise(SIGUSR1);
    check(on_stack(handler_at, 0), "an SA_ONSTACK handler runs on the signal stack");
    check(handler_flags == SS_ONSTACK, "on it, sigaltstack says SS_ONSTACK");
    check(handler_change, "on it, the signal stack cannot be changed: EPERM");

    action.sa_flags = 0;
    sigaction(SIGUSR1, &action, NULL);
    raise(SIGUSR1);
    check(!on_stack(handler_at, 0), "a handler without SA_ONSTACK runs on the thread's own stack");

    stack_t off = {NULL, SS_DISABLE, 0};
    check(sigaltstack(&off, NULL) == 0 && sigaltstack(NULL, &now) == 0 && now.ss_flags == SS_DISABLE, "SS_DISABLE removes it");

    struct sigaction fault = {0};
    fault.sa_sigaction = on_overflow;
    fault.sa_flags = SA_SIGINFO | SA_ONSTACK;
    sigaction(SIGSEGV, &fault, NULL);
    pthread_attr_t attributes;
    pthread_attr_init(&attributes);
    pthread_attr_setstacksize(&attributes, 64 * 1024);
    pthread_t thread;
    void *result = (void *)9;
    if (pthread_create(&thread, &attributes, overflowing_thread, NULL) == 0)
        pthread_join(thread, &result);
    check(result == (void *)0, "a thread overflows its stack, its handler runs on its own signal stack and jumps out");
    signal(SIGSEGV, SIG_DFL);
}

/* ---- children: SIGCHLD and waitid --------------------------------- */

static pid_t start(const char *what)
{
    char *argv[] = {"ground", (char *)what, NULL};
    pid_t child = -1;
    if (posix_spawnp(&child, "ground", NULL, NULL, argv, environ) != 0)
        return -1;
    return child;
}

static volatile sig_atomic_t children_ended;

static void on_child(int signal)
{
    (void)signal;
    children_ended++;
}

static void children_checks(void)
{
    siginfo_t info;
    errno = 0;
    check(waitid(P_ALL, 0, &info, WEXITED) == -1 && errno == ECHILD, "waitid with no children: ECHILD");

    signal(SIGCHLD, on_child);
    pid_t child = start("child-exit");
    check(child > 0, "a child started");
    memset(&info, 0xff, sizeof info);
    int r = waitid(P_PID, (id_t)child, &info, WEXITED);
    check(r == 0 && info.si_pid == child && info.si_signo == SIGCHLD && info.si_code == CLD_EXITED && info.si_status == 5,
          "waitid: the child exited with code 5 (CLD_EXITED)");
    check(children_ended == 1, "its end sent SIGCHLD, handled before waitid returned");
    /* The same, a hundred times over. Whether the handler has run by the
     * time waitid returns is decided on two CPUs at once - the child
     * ending, the parent waiting - so one try says little: under KVM the
     * kernel once let waitid return first, once in twenty runs of
     * test64 (journal entry 105). A hundred children show it every time
     * the order is wrong. `expected` counts the children ended so far,
     * so a late handler that runs during the next child is not counted
     * twice. */
    int late = 0, expected = children_ended;
    for (int i = 0; i < 100 && late >= 0; i++) {
        pid_t again = start("child-exit");
        if (again <= 0 || waitid(P_PID, (id_t)again, &info, WEXITED) != 0)
            late = -1;
        else if (children_ended < ++expected)
            late++;
    }
    check(late == 0, "a hundred more children: each SIGCHLD handled before its waitid returned");
    if (late > 0)
        printf("ground: %d of 100 SIGCHLDs were handled after waitid returned\n", late);
    errno = 0;
    check(waitid(P_PID, (id_t)child, &info, WEXITED) == -1 && errno == ECHILD, "a child waited for is gone");

    child = start("child-sleep");
    memset(&info, 0xff, sizeof info);
    check(waitid(P_PID, (id_t)child, &info, WEXITED | WNOHANG) == 0 && info.si_pid == 0,
          "WNOHANG with the child still running: 0, si_pid 0");
    int before = children_ended;
    /* pause() ends when SIGCHLD's handler has run. */
    int paused = pause();
    check(paused == -1 && errno == EINTR && children_ended == before + 1, "pause ends at the child's SIGCHLD");
    check(waitid(P_ALL, 0, &info, WEXITED | WNOWAIT) == 0 && info.si_pid == child && info.si_status == 0,
          "WNOWAIT reports the child");
    int status = -1;
    check(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0,
          "after WNOWAIT the child can still be waited for");

    child = start("child-wait");
    kill(child, SIGTERM);
    check(waitid(P_PID, (id_t)child, &info, WEXITED) == 0 && info.si_code == CLD_KILLED && info.si_status == SIGTERM,
          "waitid: a child ended by SIGTERM (CLD_KILLED)");

    errno = 0;
    check(waitid(P_PGID, 0, &info, WEXITED) == -1 && errno == EINVAL, "waitid of a process group: EINVAL");
    errno = 0;
    check(waitid(P_ALL, 0, &info, WNOHANG) == -1 && errno == EINVAL, "waitid without WEXITED: EINVAL");

    /* The stack overflows, in children: with a signal stack the
     * handler runs; without one the kernel has nowhere to put the
     * signal, and the child ends. */
    child = start("overflow");
    status = -1;
    waitpid(child, &status, 0);
    check(WIFEXITED(status) && WEXITSTATUS(status) == 3, "a stack overflow is caught on the signal stack (exit 3)");
    child = start("overflow-plain");
    status = -1;
    waitpid(child, &status, 0);
    check(WIFSIGNALED(status) && WTERMSIG(status) == SIGSEGV, "without a signal stack, a stack overflow ends the child (SIGSEGV)");
    signal(SIGCHLD, SIG_DFL);
}

/* ---- the children's side ------------------------------------------- */

static void on_main_overflow(int signal, siginfo_t *info, void *context)
{
    (void)signal;
    (void)info;
    (void)context;
    char here;
    static const char said[] = "ground: caught the stack overflow on the signal stack\n";
    write(1, said, sizeof said - 1);
    _exit(on_stack((uintptr_t)&here, 0) ? 3 : 4);
}

static int overflow(int with_stack)
{
    if (with_stack) {
        stack_t mine = {signal_stack_memory[0], 0, SIGSTKSZ};
        sigaltstack(&mine, NULL);
        struct sigaction action = {0};
        action.sa_sigaction = on_main_overflow;
        action.sa_flags = SA_SIGINFO | SA_ONSTACK;
        sigaction(SIGSEGV, &action, NULL);
    }
    return deeper(0);
}

int main(int argc, char **argv)
{
    const char *what = argc > 1 ? argv[1] : "";
    if (strcmp(what, "random") == 0) {
        unsigned char bytes[16];
        getrandom(bytes, sizeof bytes, 0);
        printf("ground: random ");
        for (int i = 0; i < 16; i++)
            printf("%02x", bytes[i]);
        printf("\n");
        return 0;
    }
    if (strcmp(what, "child-exit") == 0)
        return 5;
    if (strcmp(what, "child-sleep") == 0) {
        struct timespec t = {0, 300000000};
        nanosleep(&t, NULL);
        return 0;
    }
    if (strcmp(what, "child-wait") == 0) {
        for (;;)
            pause();
    }
    if (strcmp(what, "overflow") == 0)
        return overflow(1);
    if (strcmp(what, "overflow-plain") == 0)
        return overflow(0);

    random_checks();
    tls_checks();
    signal_stack_checks();
    children_checks();
    if (failures)
        printf("ground: %d of %d checks failed\n", failures, checks);
    else
        printf("ground: %d checks, all passed\n", checks);
    return failures != 0;
}
