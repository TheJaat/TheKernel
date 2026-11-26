/* schedbench.c - the threads and scheduling benchmark (MaK-WSL/sched's K67
 * and K95, journal entry NNN). It measures; it does not judge: every line
 * is a number for the journal, and the next scheduling item is measured
 * against it, on the same machine, before and after.
 *
 *   schedbench all CPUS      every measurement below; CPUS is how many CPUs
 *                            the machine was booted with (TheTaaJ's sysconf
 *                            does not know), only printed with the results
 *   schedbench exit          returns at once: what "spawn" starts
 *   schedbench busy FROM TO  waits until FROM, spins until TO (both in
 *                            nanoseconds of the monotonic clock), then
 *                            prints the CPU time it got in between: what
 *                            "fair" starts four of
 *
 * The measurements, each printed as "schedbench: NAME VALUE UNIT ...":
 *
 *   yield     two threads taking turns with sched_yield: the cost of a
 *             switch between two threads of one process (on one CPU each
 *             yield is a switch; on more, a yield may find nobody waiting)
 *   handoff   two threads passing a turn back and forth through a mutex
 *             and a condition: a wake-up through the kernel's futexes, and
 *             the woken thread's way back to a CPU - one way, in ns
 *   create    pthread_create and pthread_join of a thread that returns
 *   spawn     posix_spawn of a program that exits at once, and waitpid
 *   wake      wake-to-run latency: the time from pthread_cond_signal to
 *             the woken thread running, median, 99th centile and worst
 *   scale     a fixed amount of arithmetic shared among 1, 2, 4 and 8
 *             threads: the time each took, and the speed-up over one
 *   fair      four busy programs over the same two seconds: the CPU time
 *             each got (they print it themselves: "schedbench: busy ...
 *             cpu"). The window is the same for all four - they are started
 *             one after another, and a window from each one's own start
 *             would give the first more time alone
 *
 * The clock is CLOCK_MONOTONIC (the CPU's counter, to the nanosecond);
 * CPU time is CLOCK_PROCESS_CPUTIME_ID (journal entry 114). */

#include <errno.h>
#include <pthread.h>
#include <sched.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

extern char **environ;

static long long now_ns(void)
{
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (long long)t.tv_sec * 1000000000LL + t.tv_nsec;
}

static long long cpu_ns(void)
{
    struct timespec t;
    clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &t);
    return (long long)t.tv_sec * 1000000000LL + t.tv_nsec;
}

/* ---- yield: two threads taking turns ---------------------------------- */

#define YIELDS 20000
static volatile int yield_go;

static void *yielder(void *unused)
{
    (void)unused;
    while (!yield_go)
        sched_yield();
    for (int i = 0; i < YIELDS; i++)
        sched_yield();
    return NULL;
}

static void bench_yield(void)
{
    pthread_t other;
    yield_go = 0;
    pthread_create(&other, NULL, yielder, NULL);
    long long start = now_ns();
    yield_go = 1;
    for (int i = 0; i < YIELDS; i++)
        sched_yield();
    pthread_join(other, NULL);
    long long took = now_ns() - start;
    printf("schedbench: yield %lld ns each (2 threads, %d yields each)\n", took / (2LL * YIELDS), YIELDS);
}

/* ---- handoff: a turn passed through a mutex and a condition ----------- */

#define HANDOFFS 2000
static pthread_mutex_t turn_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t turn_changed = PTHREAD_COND_INITIALIZER;
static int turn; /* 0: main's turn, 1: the other's */

static void *handoff_partner(void *unused)
{
    (void)unused;
    for (int i = 0; i < HANDOFFS; i++) {
        pthread_mutex_lock(&turn_lock);
        while (turn != 1)
            pthread_cond_wait(&turn_changed, &turn_lock);
        turn = 0;
        pthread_cond_signal(&turn_changed);
        pthread_mutex_unlock(&turn_lock);
    }
    return NULL;
}

static void bench_handoff(void)
{
    pthread_t other;
    turn = 0;
    pthread_create(&other, NULL, handoff_partner, NULL);
    long long start = now_ns();
    for (int i = 0; i < HANDOFFS; i++) {
        pthread_mutex_lock(&turn_lock);
        turn = 1;
        pthread_cond_signal(&turn_changed);
        while (turn != 0)
            pthread_cond_wait(&turn_changed, &turn_lock);
        pthread_mutex_unlock(&turn_lock);
    }
    long long took = now_ns() - start;
    pthread_join(other, NULL);
    printf("schedbench: handoff %lld ns one way (%d round trips)\n", took / (2LL * HANDOFFS), HANDOFFS);
}

/* ---- create: a thread made and joined ---------------------------------- */

#define CREATES 200

static void *returns(void *unused)
{
    return unused;
}

static void bench_create(void)
{
    long long start = now_ns();
    for (int i = 0; i < CREATES; i++) {
        pthread_t t;
        if (pthread_create(&t, NULL, returns, NULL) != 0) {
            printf("schedbench: create failed at %d: errno %d\n", i, errno);
            return;
        }
        pthread_join(t, NULL);
    }
    long long took = now_ns() - start;
    printf("schedbench: create %lld us each (%d threads, made and joined)\n", took / CREATES / 1000, CREATES);
}

/* ---- spawn: a program started and waited for --------------------------- */

#define SPAWNS 20

static pid_t start(char *const argv[])
{
    pid_t pid = -1;
    if (posix_spawnp(&pid, "schedbench", NULL, NULL, argv, environ) != 0)
        return -1;
    return pid;
}

static void bench_spawn(void)
{
    char *argv[] = {"schedbench", "exit", NULL};
    long long start_ns = now_ns();
    for (int i = 0; i < SPAWNS; i++) {
        int status;
        pid_t pid = start(argv);
        if (pid < 0 || waitpid(pid, &status, 0) != pid) {
            printf("schedbench: spawn failed at %d\n", i);
            return;
        }
    }
    long long took = now_ns() - start_ns;
    printf("schedbench: spawn %lld us each (%d programs, started and waited for)\n", took / SPAWNS / 1000, SPAWNS);
}

/* ---- wake: from the signal to the woken thread running ----------------- */

#define WAKES 200
static pthread_mutex_t wake_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t wake_up = PTHREAD_COND_INITIALIZER;
static pthread_cond_t woken = PTHREAD_COND_INITIALIZER;
static long long signalled_at, ran_at;
static int wake_round, wake_seen;

static void *sleeper(void *unused)
{
    (void)unused;
    for (int round = 1; round <= WAKES; round++) {
        pthread_mutex_lock(&wake_lock);
        while (wake_round < round)
            pthread_cond_wait(&wake_up, &wake_lock);
        ran_at = now_ns();
        wake_seen = round;
        pthread_cond_signal(&woken);
        pthread_mutex_unlock(&wake_lock);
    }
    return NULL;
}

static int by_value(const void *a, const void *b)
{
    long long x = *(const long long *)a, y = *(const long long *)b;
    return (x > y) - (x < y);
}

static void bench_wake(void)
{
    static long long samples[WAKES];
    pthread_t other;
    wake_round = wake_seen = 0;
    pthread_create(&other, NULL, sleeper, NULL);
    for (int round = 1; round <= WAKES; round++) {
        /* Long enough for the sleeper to be asleep again, off its CPU. */
        struct timespec gap = {0, 2000000};
        nanosleep(&gap, NULL);
        pthread_mutex_lock(&wake_lock);
        wake_round = round;
        signalled_at = now_ns();
        pthread_cond_signal(&wake_up);
        while (wake_seen < round)
            pthread_cond_wait(&woken, &wake_lock);
        samples[round - 1] = ran_at - signalled_at;
        pthread_mutex_unlock(&wake_lock);
    }
    pthread_join(other, NULL);
    qsort(samples, WAKES, sizeof samples[0], by_value);
    printf("schedbench: wake median %lld us, 99%% %lld us, worst %lld us (%d wake-ups)\n", samples[WAKES / 2] / 1000,
           samples[WAKES * 99 / 100] / 1000, samples[WAKES - 1] / 1000, WAKES);
}

/* ---- scale: the same arithmetic on 1, 2, 4 and 8 threads --------------- */

#define WORK 400000000ULL /* multiply-adds in all */

struct share {
    unsigned long long count;
    unsigned long long result;
};

static void *crunch(void *arg)
{
    struct share *s = arg;
    unsigned long long x = 1;
    for (unsigned long long i = 0; i < s->count; i++)
        x = x * 6364136223846793005ULL + 1442695040888963407ULL;
    s->result = x;
    return NULL;
}

static long long run_shared(int threads)
{
    pthread_t t[8];
    struct share shares[8];
    long long start = now_ns();
    for (int i = 0; i < threads; i++) {
        shares[i].count = WORK / threads;
        pthread_create(&t[i], NULL, crunch, &shares[i]);
    }
    for (int i = 0; i < threads; i++)
        pthread_join(t[i], NULL);
    return now_ns() - start;
}

static void bench_scale(void)
{
    long long one = run_shared(1);
    printf("schedbench: scale 1 thread %lld ms\n", one / 1000000);
    for (int threads = 2; threads <= 8; threads *= 2) {
        long long took = run_shared(threads);
        printf("schedbench: scale %d threads %lld ms, %lld.%02lld times one\n", threads, took / 1000000, one / took,
               (one * 100 / took) % 100);
    }
}

/* ---- fair: four busy programs, the CPU time each got ------------------- */

static void bench_fair(void)
{
    char from[24], to[24];
    long long begin = now_ns() + 300000000LL; /* time for all four to start */
    snprintf(from, sizeof from, "%lld", begin);
    snprintf(to, sizeof to, "%lld", begin + 2000000000LL);
    char *argv[] = {"schedbench", "busy", from, to, NULL};
    pid_t pids[4];
    for (int i = 0; i < 4; i++)
        pids[i] = start(argv);
    for (int i = 0; i < 4; i++) {
        int status;
        if (pids[i] > 0)
            waitpid(pids[i], &status, 0);
    }
    printf("schedbench: fair done (4 programs over the same 2 s; their lines say what each got)\n");
}

static int busy(long long from, long long to)
{
    while (now_ns() < from) {
        struct timespec nap = {0, 1000000};
        nanosleep(&nap, NULL);
    }
    long long cpu0 = cpu_ns();
    volatile unsigned long spins = 0;
    while (now_ns() < to)
        spins++;
    printf("schedbench: busy %lld ms, %lld ms cpu\n", (to - from) / 1000000, (cpu_ns() - cpu0) / 1000000);
    return 0;
}

int main(int argc, char **argv)
{
    if (argc >= 2 && strcmp(argv[1], "exit") == 0)
        return 0;
    if (argc >= 4 && strcmp(argv[1], "busy") == 0)
        return busy(atoll(argv[2]), atoll(argv[3]));
    if (argc >= 3 && strcmp(argv[1], "all") == 0) {
        long long cpu0 = cpu_ns(), start = now_ns();
        printf("schedbench: on %s CPUs\n", argv[2]);
        bench_yield();
        bench_handoff();
        bench_create();
        bench_spawn();
        bench_wake();
        bench_scale();
        bench_fair();
        printf("schedbench: done in %lld ms, %lld ms cpu\n", (now_ns() - start) / 1000000, (cpu_ns() - cpu0) / 1000000);
        return 0;
    }
    printf("usage: schedbench all CPUS | exit | busy FROM TO\n");
    return 2;
}
