/* threads.c - threads in a C program, checked on TheTaaJ by
 * thread-test.sh, with 4 CPUs:
 *
 *   with files threads        the checks; "threads: N checks, all passed"
 *   threads exit              a thread calls exit(7) while the others
 *                             wait: on a mutex, a condition, a pipe, a
 *                             join. The program ends, with 7
 *   threads last              main calls pthread_exit; the program ends
 *                             with the last thread, as exit(0) would,
 *                             its printf flushed
 *   threads speed             the same work on 1 thread and on 4, timed
 *
 * A _Thread_local variable is a real one (since journal entry 099): in
 * each thread's own TLS block, at a fixed offset from its thread
 * pointer (libc/src/threads.rs makes the blocks). */

#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static int checks, failures;

static void check(int held, const char *what)
{
    checks++;
    if (!held) {
        failures++;
        printf("threads: failed: %s\n", what);
    }
}

static void pause_ms(long ms)
{
    struct timespec t = { ms / 1000, (ms % 1000) * 1000000L };
    nanosleep(&t, NULL);
}

static double now(void)
{
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (double)t.tv_sec + (double)t.tv_nsec / 1e9;
}

/* ---- create, join, what a thread returns ---- */

static void *twice(void *argument)
{
    return (void *)((long)argument * 2);
}

static void *exits_early(void *argument)
{
    pthread_exit((void *)((long)argument + 100));
    return NULL; /* not reached */
}

static void create_and_join(void)
{
    pthread_t threads[16];
    int made = 0;
    for (long n = 0; n < 16; n++)
        made += pthread_create(&threads[n], NULL, twice, (void *)n) == 0;
    check(made == 16, "16 threads made");
    int right = 0;
    for (long n = 0; n < made; n++) {
        void *result = NULL;
        right += pthread_join(threads[n], &result) == 0 && (long)result == n * 2;
    }
    check(right == 16, "each thread's return value, through pthread_join");

    pthread_t early;
    void *result = NULL;
    check(pthread_create(&early, NULL, exits_early, (void *)5) == 0 && pthread_join(early, &result) == 0
              && (long)result == 105,
          "pthread_exit's value, through pthread_join");
    check(pthread_equal(pthread_self(), pthread_self()) && !pthread_equal(pthread_self(), early),
          "pthread_self and pthread_equal");

    /* Many rounds: the stacks and records of ended threads are given
     * back, or this would run out. */
    int rounds = 0;
    for (long n = 0; n < 300; n++) {
        pthread_t t;
        void *r;
        if (pthread_create(&t, NULL, twice, (void *)n) != 0 || pthread_join(t, &r) != 0 || (long)r != n * 2)
            break;
        rounds++;
    }
    check(rounds == 300, "300 threads made and joined, one after another");
}

/* ---- a lock many threads want ---- */

static pthread_mutex_t counter_lock = PTHREAD_MUTEX_INITIALIZER;
static long counter;
#define ADDS 100000

static void *add_many(void *unused)
{
    (void)unused;
    for (int n = 0; n < ADDS; n++) {
        pthread_mutex_lock(&counter_lock);
        counter++;
        pthread_mutex_unlock(&counter_lock);
    }
    return NULL;
}

/* malloc and free from many threads at once: the heap has one lock. A
 * block given to two threads would show the other's mark. */
static void *allocate_many(void *argument)
{
    unsigned char mark = (unsigned char)(long)argument;
    unsigned char *kept[32] = { 0 };
    long wrong = 0;
    for (int n = 0; n < 20000; n++) {
        int slot = n % 32;
        size_t size = (size_t)(n % 200 + 1);
        if (kept[slot] != NULL) {
            for (size_t i = 0; i < (size_t)((n - 32) % 200 + 1); i++)
                wrong += kept[slot][i] != mark;
            free(kept[slot]);
        }
        kept[slot] = malloc(size);
        if (kept[slot] == NULL)
            return (void *)1L;
        memset(kept[slot], mark, size);
    }
    for (int slot = 0; slot < 32; slot++)
        free(kept[slot]);
    return (void *)wrong;
}

static void contention(void)
{
    pthread_t allocators[4];
    for (long n = 0; n < 4; n++)
        pthread_create(&allocators[n], NULL, allocate_many, (void *)(n + 1));
    long wrong = 0;
    for (int n = 0; n < 4; n++) {
        void *result;
        pthread_join(allocators[n], &result);
        wrong += (long)result;
    }
    check(wrong == 0, "4 threads malloc and free 20,000 blocks each at once: every block kept its own bytes");

    pthread_t threads[8];
    counter = 0;
    for (int n = 0; n < 8; n++)
        pthread_create(&threads[n], NULL, add_many, NULL);
    for (int n = 0; n < 8; n++)
        pthread_join(threads[n], NULL);
    check(counter == 8L * ADDS, "8 threads add 100,000 each under one mutex: 800,000, none lost");
}

/* ---- the kinds of mutex ---- */

static pthread_mutex_t held_elsewhere = PTHREAD_MUTEX_INITIALIZER;
static int tried, unlocked_by_other;

static void *try_held(void *unused)
{
    (void)unused;
    tried = pthread_mutex_trylock(&held_elsewhere);
    unlocked_by_other = pthread_mutex_unlock(&held_elsewhere);
    return NULL;
}

static void mutex_kinds(void)
{
    pthread_mutex_lock(&held_elsewhere);
    pthread_t t;
    pthread_create(&t, NULL, try_held, NULL);
    pthread_join(t, NULL);
    check(tried == EBUSY, "trylock of a mutex another thread holds: EBUSY");
    check(unlocked_by_other == EPERM, "unlock by a thread that doesn't hold it: EPERM");
    check(pthread_mutex_lock(&held_elsewhere) == EDEADLK, "a normal mutex locked again by its holder: EDEADLK");
    pthread_mutex_unlock(&held_elsewhere);

    pthread_mutexattr_t attr;
    pthread_mutex_t recursive;
    pthread_mutexattr_init(&attr);
    pthread_mutexattr_settype(&attr, PTHREAD_MUTEX_RECURSIVE);
    pthread_mutex_init(&recursive, &attr);
    int twice_locked = pthread_mutex_lock(&recursive) == 0 && pthread_mutex_lock(&recursive) == 0;
    pthread_mutex_unlock(&recursive);
    check(twice_locked && pthread_mutex_destroy(&recursive) == EBUSY, "a recursive mutex counts: held once more");
    pthread_mutex_unlock(&recursive);
    check(pthread_mutex_destroy(&recursive) == 0, "and is free after as many unlocks");
}

/* ---- conditions: a queue between producers and consumers ---- */

#define ITEMS 20000
static pthread_mutex_t queue_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t not_empty = PTHREAD_COND_INITIALIZER;
static pthread_cond_t not_full = PTHREAD_COND_INITIALIZER;
static int queue[16], queue_head, queue_count;
static long taken_sum;
static int taken_count;

static void *produce(void *argument)
{
    long first = (long)argument;
    for (long n = first; n < first + ITEMS / 2; n++) {
        pthread_mutex_lock(&queue_lock);
        while (queue_count == 16)
            pthread_cond_wait(&not_full, &queue_lock);
        queue[(queue_head + queue_count++) % 16] = (int)n;
        pthread_cond_signal(&not_empty);
        pthread_mutex_unlock(&queue_lock);
    }
    return NULL;
}

static void *consume(void *unused)
{
    (void)unused;
    for (;;) {
        pthread_mutex_lock(&queue_lock);
        while (queue_count == 0 && taken_count < ITEMS)
            pthread_cond_wait(&not_empty, &queue_lock);
        if (taken_count == ITEMS) {
            pthread_mutex_unlock(&queue_lock);
            return NULL;
        }
        int item = queue[queue_head];
        queue_head = (queue_head + 1) % 16;
        queue_count--;
        taken_sum += item;
        if (++taken_count == ITEMS)
            pthread_cond_broadcast(&not_empty); /* the other consumers: done */
        pthread_cond_signal(&not_full);
        pthread_mutex_unlock(&queue_lock);
    }
}

static void conditions(void)
{
    pthread_t producers[2], consumers[3];
    pthread_create(&producers[0], NULL, produce, (void *)0L);
    pthread_create(&producers[1], NULL, produce, (void *)(long)(ITEMS / 2));
    for (int n = 0; n < 3; n++)
        pthread_create(&consumers[n], NULL, consume, NULL);
    for (int n = 0; n < 2; n++)
        pthread_join(producers[n], NULL);
    for (int n = 0; n < 3; n++)
        pthread_join(consumers[n], NULL);
    check(taken_count == ITEMS && taken_sum == (long)ITEMS * (ITEMS - 1) / 2,
          "2 producers, 3 consumers, a queue of 16: all 20,000 items taken once");

    /* Nobody signals: it waits out its time, and not less. */
    pthread_cond_t never = PTHREAD_COND_INITIALIZER;
    pthread_mutex_lock(&queue_lock);
    struct timespec deadline;
    clock_gettime(CLOCK_REALTIME, &deadline);
    deadline.tv_nsec += 80 * 1000000L;
    if (deadline.tv_nsec >= 1000000000L) {
        deadline.tv_sec++;
        deadline.tv_nsec -= 1000000000L;
    }
    double start = now();
    int waited = pthread_cond_timedwait(&never, &queue_lock, &deadline);
    double took = now() - start;
    pthread_mutex_unlock(&queue_lock);
    check(waited == ETIMEDOUT && took >= 0.07, "pthread_cond_timedwait with nobody to signal: ETIMEDOUT, after its time");
}

/* ---- many readers or one writer ---- */

static pthread_rwlock_t pair_lock = PTHREAD_RWLOCK_INITIALIZER;
static long pair_a, pair_b;
static int torn;

static void *write_pairs(void *unused)
{
    (void)unused;
    for (int n = 0; n < 20000; n++) {
        pthread_rwlock_wrlock(&pair_lock);
        pair_a++;
        pair_b++;
        pthread_rwlock_unlock(&pair_lock);
    }
    return NULL;
}

static void *read_pairs(void *unused)
{
    (void)unused;
    for (int n = 0; n < 20000; n++) {
        pthread_rwlock_rdlock(&pair_lock);
        if (pair_a != pair_b)
            torn++;
        pthread_rwlock_unlock(&pair_lock);
    }
    return NULL;
}

static void rwlocks(void)
{
    pthread_t threads[4];
    pthread_create(&threads[0], NULL, write_pairs, NULL);
    pthread_create(&threads[1], NULL, write_pairs, NULL);
    pthread_create(&threads[2], NULL, read_pairs, NULL);
    pthread_create(&threads[3], NULL, read_pairs, NULL);
    for (int n = 0; n < 4; n++)
        pthread_join(threads[n], NULL);
    check(torn == 0 && pair_a == 40000 && pair_b == 40000, "a rwlock: readers never see half a write, no write lost");
}

/* ---- once, keys, thread-local variables, errno ---- */

static pthread_once_t once = PTHREAD_ONCE_INIT;
static int once_calls;
static pthread_key_t key;
static int destroyed, destroyed_right;
static _Thread_local int mine = 5;
static int locals_right, errnos_right;

static void once_only(void)
{
    once_calls++;
}

static void destroy(void *value)
{
    pthread_mutex_lock(&counter_lock);
    destroyed++;
    destroyed_right += (long)value >= 1 && (long)value <= 8;
    pthread_mutex_unlock(&counter_lock);
}

static void *per_thread(void *argument)
{
    long me = (long)argument;
    pthread_once(&once, once_only);
    pthread_setspecific(key, (void *)me);
    int fresh = mine == 5; /* every thread's copy starts from 5 */
    mine = (int)me * 10;
    errno = (int)me;
    pause_ms(20); /* the others write theirs meanwhile */
    pthread_mutex_lock(&counter_lock);
    locals_right += fresh && mine == me * 10;
    errnos_right += errno == me && (long)pthread_getspecific(key) == me;
    pthread_mutex_unlock(&counter_lock);
    return NULL;
}

static void per_thread_things(void)
{
    check(pthread_key_create(&key, destroy) == 0, "a key, with a destructor");
    pthread_t threads[8];
    for (long n = 0; n < 8; n++)
        pthread_create(&threads[n], NULL, per_thread, (void *)(n + 1));
    for (int n = 0; n < 8; n++)
        pthread_join(threads[n], NULL);
    check(once_calls == 1, "pthread_once from 8 threads: called once");
    check(locals_right == 8, "a _Thread_local variable: each of 8 threads its own, from its starting value");
    check(errnos_right == 8, "errno and a key's value: each thread its own");
    check(destroyed == 8 && destroyed_right == 8, "the key's destructor, once for each thread, with its value");
    check(mine == 5 && pthread_getspecific(key) == NULL, "the main thread's own copies untouched");
}

/* ---- detached threads ---- */

static pthread_mutex_t detached_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t detached_done = PTHREAD_COND_INITIALIZER;
static int detached_ended;

static void *detached_work(void *unused)
{
    (void)unused;
    pthread_mutex_lock(&detached_lock);
    detached_ended++;
    pthread_cond_signal(&detached_done);
    pthread_mutex_unlock(&detached_lock);
    return NULL;
}

static void detached(void)
{
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
    pthread_attr_setstacksize(&attr, 64 * 1024);
    unsigned long size = 0;
    pthread_attr_getstacksize(&attr, &size);
    check(size == 64 * 1024, "a thread's stack size, in its attributes");
    int made = 0;
    /* More than the kernel lets a process have at once (THREADS_MAX,
     * 64): an ended detached thread must be given back. */
    for (int n = 0; n < 200; n++) {
        pthread_t t;
        if (pthread_create(&t, &attr, detached_work, NULL) != 0)
            break;
        made++;
        if (n % 20 == 19) {
            pthread_mutex_lock(&detached_lock);
            while (detached_ended < made)
                pthread_cond_wait(&detached_done, &detached_lock);
            pthread_mutex_unlock(&detached_lock);
        }
    }
    check(made == 200 && detached_ended == 200, "200 detached threads, made 20 at a time, all ran");
    pthread_t t;
    check(pthread_create(&t, NULL, twice, (void *)1L) == 0 && pthread_join(t, NULL) == 0,
          "and threads can still be made after them");
}

/* ---- I/O from threads: one thread waits on a pipe while another
 * writes to it, and printf from many threads ---- */

static int pipe_ends[2];
static char heard[64];

static void *listen_pipe(void *unused)
{
    (void)unused;
    ssize_t n = read(pipe_ends[0], heard, sizeof heard - 1);
    if (n > 0)
        heard[n] = '\0';
    return NULL;
}

static FILE *shared;

static void *write_lines(void *argument)
{
    long me = (long)argument;
    for (int n = 0; n < 200; n++)
        fprintf(shared, "thread %ld line %03d: the quick brown fox jumps over the lazy dog\n", me, n);
    return NULL;
}

static void input_output(void)
{
    /* The reader waits in read() first; the write has to get past it.
     * (With the descriptor table locked during a read, as it was, the
     * write would wait for the read and the read for the write.) */
    check(pipe(pipe_ends) == 0, "a pipe");
    pthread_t reader;
    pthread_create(&reader, NULL, listen_pipe, NULL);
    pause_ms(50);
    check(write(pipe_ends[1], "across threads", 14) == 14, "a write to a pipe another thread is reading");
    pthread_join(reader, NULL);
    check(strcmp(heard, "across threads") == 0, "and that thread read it");
    close(pipe_ends[0]);
    close(pipe_ends[1]);

    shared = fopen("lines.txt", "w");
    check(shared != NULL, "a file to share");
    if (shared == NULL)
        return;
    pthread_t writers[4];
    for (long n = 0; n < 4; n++)
        pthread_create(&writers[n], NULL, write_lines, (void *)n);
    for (int n = 0; n < 4; n++)
        pthread_join(writers[n], NULL);
    fclose(shared);
    FILE *back = fopen("lines.txt", "r");
    char line[128];
    int lines = 0, whole = 0, next[4] = { 0, 0, 0, 0 }, in_order = 1;
    while (back != NULL && fgets(line, sizeof line, back) != NULL) {
        long who;
        int number;
        char tail[80];
        lines++;
        if (sscanf(line, "thread %ld line %d: %79[^\n]", &who, &number, tail) == 3 && who >= 0 && who < 4
            && strcmp(tail, "the quick brown fox jumps over the lazy dog") == 0) {
            whole++;
            in_order &= number == next[who]++;
        }
    }
    if (back != NULL)
        fclose(back);
    check(lines == 800 && whole == 800, "4 threads fprintf 200 lines each to one FILE: 800 whole lines");
    check(in_order, "and each thread's lines in its own order");
    unlink("lines.txt");
}

/* ---- threads exit: the program ends with whoever calls exit ---- */

static pthread_mutex_t never_free = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t never_signalled = PTHREAD_COND_INITIALIZER;

static void *wait_mutex(void *unused)
{
    (void)unused;
    pthread_mutex_lock(&never_free);
    return NULL;
}

static void *wait_condition(void *unused)
{
    (void)unused;
    pthread_mutex_t mutex = PTHREAD_MUTEX_INITIALIZER;
    pthread_mutex_lock(&mutex);
    pthread_cond_wait(&never_signalled, &mutex);
    return NULL;
}

static void *wait_pipe(void *unused)
{
    (void)unused;
    char byte;
    read(pipe_ends[0], &byte, 1);
    return NULL;
}

static void *spin(void *unused)
{
    (void)unused;
    for (volatile unsigned long n = 0;; n++)
        ;
    return NULL;
}

static void *end_it(void *unused)
{
    (void)unused;
    pause_ms(100);
    printf("threads: exit(7) from a thread, with others waiting and one spinning\n");
    exit(7);
}

static int exit_from_thread(void)
{
    pthread_t t;
    pipe(pipe_ends);
    pthread_mutex_lock(&never_free);
    pthread_create(&t, NULL, wait_mutex, NULL);
    pthread_create(&t, NULL, wait_condition, NULL);
    pthread_create(&t, NULL, wait_pipe, NULL);
    pthread_create(&t, NULL, spin, NULL);
    pthread_create(&t, NULL, end_it, NULL);
    pthread_join(t, NULL); /* end_it never returns */
    printf("threads: not reached\n");
    return 1;
}

/* ---- threads last: main leaves first ---- */

static void *outlive_main(void *unused)
{
    (void)unused;
    pause_ms(100);
    printf("threads: the last thread ends the program");
    /* No new line, and no fflush: exit's flush must send it. */
    return NULL;
}

static int main_leaves_first(void)
{
    pthread_t t;
    pthread_create(&t, NULL, outlive_main, NULL);
    pthread_detach(t);
    pthread_exit(NULL);
}

/* ---- threads speed ---- */

/* xorshift: each step needs the last, and no compiler can skip to the
 * end. The answer is kept (`sink`), so the work can't be dropped. */
static volatile unsigned long sink;

static void *work(void *argument)
{
    unsigned long n = (unsigned long)argument, x = 88172645463325252UL;
    for (unsigned long i = 0; i < n; i++) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
    }
    sink = x;
    return NULL;
}

static int speed(void)
{
    const unsigned long total = 200000000UL;
    double start = now();
    work((void *)total);
    double one = now() - start;
    pthread_t threads[4];
    start = now();
    for (int n = 0; n < 4; n++)
        pthread_create(&threads[n], NULL, work, (void *)(total / 4));
    for (int n = 0; n < 4; n++)
        pthread_join(threads[n], NULL);
    double four = now() - start;
    printf("threads: the same work: 1 thread %.2f s, 4 threads %.2f s (%.1f times as fast)\n", one, four, one / four);
    return 0;
}

int main(int argc, char **argv)
{
    if (argc > 1 && strcmp(argv[1], "exit") == 0)
        return exit_from_thread();
    if (argc > 1 && strcmp(argv[1], "last") == 0)
        return main_leaves_first();
    if (argc > 1 && strcmp(argv[1], "speed") == 0)
        return speed();
    create_and_join();
    contention();
    mutex_kinds();
    conditions();
    rwlocks();
    per_thread_things();
    detached();
    input_output();
    if (failures != 0) {
        printf("threads: %d of %d checks failed\n", failures, checks);
        return 1;
    }
    printf("threads: %d checks, all passed\n", checks);
    return 0;
}
