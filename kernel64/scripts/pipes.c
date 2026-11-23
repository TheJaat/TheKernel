/* pipes.c - pipes, file actions, system and popen, checked on TheTaaJ
 * by pipe-test.sh. Run with the right to start programs, in a
 * directory with in.txt (5000 bytes) in it:
 *
 *   with programs pipes           the checks; "pipes: N checks, all passed"
 *
 * It is also the program at the other end of each pipe, started by
 * itself from /pipes:
 *
 *   pipes emit WORD...    prints the words, a line
 *   pipes lines N         prints N numbered lines
 *   pipes oops WORD       prints WORD on standard error
 *   pipes count           reads standard input to its end: "N bytes"
 *   pipes exit N          exits with N
 *   pipes env NAME        prints NAME's value, or "(unset)" */

#include <errno.h>
#include <fcntl.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

static int checks, failures;

static void check(int held, const char *what)
{
    checks++;
    if (!held) {
        failures++;
        printf("pipes: failed: %s (errno %d)\n", what, errno);
    }
}

/* All of a descriptor, to its end, into `out` (at most `room` - 1). */
static size_t drain(int fd, char *out, size_t room)
{
    size_t got = 0;
    for (;;) {
        ssize_t n = read(fd, out + got, room - 1 - got);
        if (n <= 0)
            break;
        got += (size_t)n;
        if (got == room - 1)
            break;
    }
    out[got] = '\0';
    return got;
}

/* A file's contents, as text. */
static const char *contents(const char *path)
{
    static char buffer[65536];
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return "(none)";
    drain(fd, buffer, sizeof buffer);
    close(fd);
    return buffer;
}

static pid_t start(posix_spawn_file_actions_t *actions, char *const argv[])
{
    pid_t child = -1;
    int code = posix_spawnp(&child, "pipes", actions, NULL, argv, NULL);
    if (code != 0) {
        errno = code;
        return -1;
    }
    return child;
}

static int status_of(pid_t child)
{
    int status = -1;
    if (waitpid(child, &status, 0) != child)
        return -1;
    return status;
}

static void in_one_program(void)
{
    int ends[2];
    check(pipe(ends) == 0 && ends[0] >= 3 && ends[1] == ends[0] + 1, "pipe gives the two lowest free descriptors");
    check(write(ends[1], "hello", 5) == 5, "a pipe takes what is written");
    char buffer[16];
    check(read(ends[0], buffer, sizeof buffer) == 5 && memcmp(buffer, "hello", 5) == 0, "and gives it back");
    struct stat status;
    check(fstat(ends[0], &status) == 0 && S_ISFIFO(status.st_mode), "an end is a FIFO");
    int copy = dup(ends[1]);
    check(copy >= 0, "an end can be copied");
    close(ends[1]);
    check(write(copy, "ab", 2) == 2 && read(ends[0], buffer, 2) == 2, "the copy still writes when the first is closed");
    close(copy);
    check(read(ends[0], buffer, sizeof buffer) == 0, "the end: every write end closed");
    close(ends[0]);
    check(pipe(ends) == 0, "a second pipe");
    close(ends[0]);
    errno = 0;
    check(write(ends[1], "x", 1) == -1 && errno == EPIPE, "nobody reading: EPIPE");
    close(ends[1]);
}

static void between_programs(void)
{
    char buffer[8192];
    posix_spawn_file_actions_t actions;
    int out[2], in[2];

    /* Its output, through a pipe. */
    pipe(out);
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_adddup2(&actions, out[1], 1);
    char *emit[] = {"pipes", "emit", "through", "a", "pipe", NULL};
    pid_t child = start(&actions, emit);
    posix_spawn_file_actions_destroy(&actions);
    close(out[1]);
    check(child > 0, "started with its output a pipe");
    drain(out[0], buffer, sizeof buffer);
    check(strcmp(buffer, "through a pipe\n") == 0, "its output, read from the pipe");
    check(status_of(child) == 0, "and it ended well");
    close(out[0]);

    /* Its input, through a pipe, and its output through another. */
    pipe(in);
    pipe(out);
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_adddup2(&actions, in[0], 0);
    posix_spawn_file_actions_adddup2(&actions, out[1], 1);
    char *count[] = {"pipes", "count", NULL};
    child = start(&actions, count);
    posix_spawn_file_actions_destroy(&actions);
    close(in[0]);
    close(out[1]);
    check(child > 0, "started with a pipe for its input");
    memset(buffer, 'x', 3000);
    check(write(in[1], buffer, 3000) == 3000, "3000 bytes written to it");
    close(in[1]);
    drain(out[0], buffer, sizeof buffer);
    check(strcmp(buffer, "3000 bytes\n") == 0, "it read them all, and saw the end");
    check(status_of(child) == 0, "and it ended well");
    close(out[0]);

    /* Standard error onto standard output: 2 a copy of 1. */
    pipe(out);
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_adddup2(&actions, out[1], 1);
    posix_spawn_file_actions_adddup2(&actions, 1, 2);
    char *oops[] = {"pipes", "oops", "complaint", NULL};
    child = start(&actions, oops);
    posix_spawn_file_actions_destroy(&actions);
    close(out[1]);
    drain(out[0], buffer, sizeof buffer);
    check(strcmp(buffer, "complaint\n") == 0, "2 made a copy of 1: its errors in the pipe");
    status_of(child);
    close(out[0]);

    /* A reader that goes away: the writer is stopped, SIGPIPE. */
    pipe(out);
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_adddup2(&actions, out[1], 1);
    char *lines[] = {"pipes", "lines", "100000", NULL};
    child = start(&actions, lines);
    posix_spawn_file_actions_destroy(&actions);
    close(out[1]);
    check(read(out[0], buffer, 10) > 0, "a writer that writes a lot");
    close(out[0]);
    int status = status_of(child);
    check(WIFSIGNALED(status) && WTERMSIG(status) == 13, "stopped with SIGPIPE when nobody reads");
}

static void files(void)
{
    posix_spawn_file_actions_t actions;
    char *lines[] = {"pipes", "lines", "5000", NULL};

    /* Its output into a file: all of it there once it has ended - 55 KB,
     * tens of messages. (The file service writes a stream's messages
     * before it answers a request; the test cannot make it late enough
     * to show that it must - journal entry 079.) */
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_addopen(&actions, 1, "out.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    pid_t child = start(&actions, lines);
    check(child > 0 && status_of(child) == 0, "started with its output a file");
    const char *got = contents("out.txt");
    check(strlen(got) == 5000 * 11 && strncmp(got, "line 00001\n", 11) == 0 && strcmp(got + 54989, "line 05000\n") == 0,
          "every line in the file once it ended");
    /* Again, O_APPEND: added. */
    posix_spawn_file_actions_destroy(&actions);
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_addopen(&actions, 1, "out.txt", O_WRONLY | O_CREAT | O_APPEND, 0644);
    char *emit[] = {"pipes", "emit", "more", NULL};
    child = start(&actions, emit);
    check(child > 0 && status_of(child) == 0, "and again, to add");
    got = contents("out.txt");
    check(strlen(got) == 55005 && strcmp(got + 55000, "more\n") == 0, "O_APPEND kept what was there");
    posix_spawn_file_actions_destroy(&actions);

    /* Its input from a file. */
    int out[2];
    pipe(out);
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_addopen(&actions, 0, "in.txt", O_RDONLY, 0);
    posix_spawn_file_actions_adddup2(&actions, out[1], 1);
    char *count[] = {"pipes", "count", NULL};
    child = start(&actions, count);
    posix_spawn_file_actions_destroy(&actions);
    close(out[1]);
    char buffer[64];
    drain(out[0], buffer, sizeof buffer);
    check(strcmp(buffer, "5000 bytes\n") == 0, "its input a file: all 5000 bytes, and the end");
    status_of(child);
    close(out[0]);

    /* A file this program has open, given as its output. */
    int fd = open("given.txt", O_WRONLY | O_CREAT | O_TRUNC);
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_adddup2(&actions, fd, 1);
    char *given[] = {"pipes", "emit", "given", NULL};
    child = start(&actions, given);
    posix_spawn_file_actions_destroy(&actions);
    close(fd);
    check(child > 0 && status_of(child) == 0 && strcmp(contents("given.txt"), "given\n") == 0, "a file open here, given");

    /* Not there, and not to be made: not started. */
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_addopen(&actions, 0, "none.txt", O_RDONLY, 0);
    errno = 0;
    check(start(&actions, count) == -1 && errno == ENOENT, "input from no file: ENOENT, not started");
    posix_spawn_file_actions_destroy(&actions);
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_addopen(&actions, 1, "none.txt", O_WRONLY, 0);
    errno = 0;
    check(start(&actions, emit) == -1 && errno == ENOENT, "output to no file, without O_CREAT: ENOENT");
    posix_spawn_file_actions_destroy(&actions);

    /* addclose: it gets no output at all, and runs. */
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_addclose(&actions, 1);
    child = start(&actions, emit);
    check(child > 0 && status_of(child) == 0, "1 closed: it runs without an output");
    posix_spawn_file_actions_destroy(&actions);
    posix_spawn_file_actions_init(&actions);
    check(posix_spawn_file_actions_adddup2(&actions, -1, 1) == EBADF, "adddup2 of no descriptor: EBADF");
    posix_spawn_file_actions_adddup2(&actions, 20, 1);
    errno = 0;
    check(start(&actions, emit) == -1 && errno == EBADF, "a descriptor not open here: EBADF");
    posix_spawn_file_actions_destroy(&actions);
}

static void commands(void)
{
    check(system("pipes emit one two three | pipes count > counted.txt") == 0, "system: a pipe and a redirection");
    check(strcmp(contents("counted.txt"), "14 bytes\n") == 0, "what came through the pipe, in the file");
    check(system("pipes count < in.txt > n.txt") == 0 && strcmp(contents("n.txt"), "5000 bytes\n") == 0, "< and >");
    system("pipes emit first > log.txt");
    system("pipes emit second >> log.txt");
    check(strcmp(contents("log.txt"), "first\nsecond\n") == 0, "> then >>");
    check(system("pipes oops trouble > all.txt 2>&1") == 0 && strcmp(contents("all.txt"), "trouble\n") == 0, "2>&1");
    check(system("pipes oops alone 2> errors.txt") == 0 && strcmp(contents("errors.txt"), "alone\n") == 0, "2>");
    check(system("pipes emit a | pipes count | pipes count > three.txt") == 0 && strcmp(contents("three.txt"), "8 bytes\n") == 0,
          "three commands, two pipes");
    check(system("cat in.txt | pipes count > cat.txt") == 0 && strcmp(contents("cat.txt"), "5000 bytes\n") == 0,
          "a program of TheTaaJ's own (cat) in a pipe");
    check(system("pipes emit x | pipes exit 3") == 3 << 8, "a pipeline's status is its last command's");
    check(system("nothing | pipes count") == 127 << 8, "a command not found: 127");
    check(system("pipes count < none.txt") == 127 << 8, "a file not there for <: 127");
    /* More than the file service keeps at once (16): each stream is
     * let go when its program ends. */
    int all = 1;
    for (int n = 0; n < 40 && all; n++)
        all = system("pipes emit again > again.txt") == 0;
    check(all && strcmp(contents("again.txt"), "again\n") == 0, "40 redirections one after another: none kept open");
    errno = 0;
    check(system("pipes emit a && pipes emit b") == -1 && errno == ENOSYS, "&& still needs a shell");
    check(system("pipes emit a |") == -1 && errno == ENOSYS, "a pipe to nothing is refused");

    FILE *stream = popen("pipes emit from popen", "r");
    char line[64] = "";
    check(stream != NULL && fgets(line, sizeof line, stream) != NULL && strcmp(line, "from popen\n") == 0, "popen, reading");
    check(stream != NULL && pclose(stream) == 0, "pclose: its status");
    stream = popen("pipes count > popened.txt", "w");
    check(stream != NULL && fputs("ten bytes\n", stream) >= 0, "popen, writing");
    check(stream != NULL && pclose(stream) == 0 && strcmp(contents("popened.txt"), "10 bytes\n") == 0, "what was written, read");
    stream = popen("pipes exit 4", "r");
    check(stream != NULL && pclose(stream) == 4 << 8, "pclose gives the exit code");
    check(popen("nothing", "r") == NULL, "popen of nothing: NULL");
}

static void environment(void)
{
    check(getenv("GREETING") == NULL, "started by the shell: no environment");
    setenv("GREETING", "hello there", 1);
    check(system("pipes env GREETING > env.txt") == 0 && strcmp(contents("env.txt"), "hello there\n") == 0,
          "system passes the environment on");
    FILE *stream = popen("pipes env GREETING", "r");
    char line[64] = "";
    check(stream != NULL && fgets(line, sizeof line, stream) && strcmp(line, "hello there\n") == 0, "popen does too");
    if (stream != NULL)
        pclose(stream);
    /* posix_spawn: envp, and only envp. */
    posix_spawn_file_actions_t actions;
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_addopen(&actions, 1, "envp.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    char *only[] = {"ONLY=this one", NULL};
    char *ask[] = {"pipes", "env", "ONLY", NULL};
    pid_t child = -1;
    check(posix_spawnp(&child, "pipes", &actions, NULL, ask, only) == 0 && status_of(child) == 0
              && strcmp(contents("envp.txt"), "this one\n") == 0, "posix_spawn gives envp");
    char *greeting[] = {"pipes", "env", "GREETING", NULL};
    check(posix_spawnp(&child, "pipes", &actions, NULL, greeting, only) == 0 && status_of(child) == 0
              && strcmp(contents("envp.txt"), "(unset)\n") == 0, "and nothing of this program's but envp");
    posix_spawn_file_actions_destroy(&actions);
    /* make takes a macro from the environment, and its commands get it. */
    setenv("WHO", "the environment", 1);
    check(system("make -f env.mk > made.txt") == 0 && strcmp(contents("made.txt"), "pipes emit from the environment\nfrom the environment\n") == 0,
          "make: a macro from the environment");
    unsetenv("GREETING");
    check(system("pipes env GREETING > env.txt") == 0 && strcmp(contents("env.txt"), "(unset)\n") == 0, "unsetenv: not passed");
    /* Too much to travel in init's request. */
    static char big[1100];
    memset(big, 'x', sizeof big - 1);
    setenv("BIG", big, 1);
    pid_t none = -1;
    check(posix_spawnp(&none, "pipes", NULL, NULL, ask, NULL) == E2BIG, "an environment too big: E2BIG");
    unsetenv("BIG");
}

int main(int argc, char **argv)
{
    if (argc >= 2 && strcmp(argv[1], "emit") == 0) {
        for (int n = 2; n < argc; n++)
            printf(n + 1 < argc ? "%s " : "%s\n", argv[n]);
        return 0;
    }
    if (argc == 3 && strcmp(argv[1], "lines") == 0) {
        int count = atoi(argv[2]);
        for (int n = 1; n <= count; n++)
            printf("line %05d\n", n);
        return 0;
    }
    if (argc == 3 && strcmp(argv[1], "oops") == 0) {
        fprintf(stderr, "%s\n", argv[2]);
        return 0;
    }
    if (argc == 2 && strcmp(argv[1], "count") == 0) {
        char buffer[512];
        long total = 0;
        ssize_t n;
        while ((n = read(0, buffer, sizeof buffer)) > 0)
            total += n;
        printf("%ld bytes\n", total);
        return 0;
    }
    if (argc == 3 && strcmp(argv[1], "exit") == 0)
        return atoi(argv[2]);
    if (argc == 3 && strcmp(argv[1], "env") == 0) {
        const char *value = getenv(argv[2]);
        printf("%s\n", value ? value : "(unset)");
        return 0;
    }

    in_one_program();
    between_programs();
    files();
    commands();
    environment();
    if (failures) {
        printf("pipes: %d of %d checks failed\n", failures, checks);
        return 1;
    }
    printf("pipes: %d checks, all passed\n", checks);
    return 0;
}
