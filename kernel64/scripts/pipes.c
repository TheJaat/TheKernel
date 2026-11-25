/* pipes.c - pipes, file actions, system and popen - and, through them, sh -
 * checked on TheTaaJ by pipe-test.sh. Run with the right to start
 * programs, in a directory with in.txt (5000 bytes) in it, and /sh on
 * the disk:
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
 *   pipes env NAME        prints NAME's value, or "(unset)"
 *   pipes fdcount N       reads descriptor N to its end: "N bytes", or
 *                         "N: closed" if it has no N
 *   pipes fdwrite N WORD  writes WORD and a new line to descriptor N */

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

/* The pipes here are pipe2(..., O_CLOEXEC): each is meant for one
 * child, which gets the end it is to have by adddup2. A plain pipe's
 * ends would go to every child as well (descriptors are inherited), and
 * a child holding the write end of its own input never sees its end -
 * as on Unix. */
static void between_programs(void)
{
    char buffer[8192];
    posix_spawn_file_actions_t actions;
    int out[2], in[2];

    /* Its output, through a pipe. */
    pipe2(out, O_CLOEXEC);
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
    pipe2(in, O_CLOEXEC);
    pipe2(out, O_CLOEXEC);
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
    pipe2(out, O_CLOEXEC);
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
    pipe2(out, O_CLOEXEC);
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
    pipe2(out, O_CLOEXEC);
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
    check(system("pipes emit a |") == 2 << 8, "a pipe to nothing: sh says it is a mistake, 2");

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

/* What needs a shell: system hands it to /sh. */
static void shell(void)
{
    check(system("pipes emit a > ab.txt && pipes emit b >> ab.txt") == 0 && strcmp(contents("ab.txt"), "a\nb\n") == 0,
          "&&: both run");
    check(system("pipes exit 2 && pipes emit no > and.txt") == 2 << 8 && strcmp(contents("and.txt"), "(none)") == 0,
          "&&: the second not run when the first fails, and its status");
    check(system("pipes exit 2 || pipes emit rescued > or.txt") == 0 && strcmp(contents("or.txt"), "rescued\n") == 0, "||");
    check(system("pipes emit one > list.txt; pipes emit two >> list.txt") == 0 && strcmp(contents("list.txt"), "one\ntwo\n") == 0,
          "; one after the other");
    check(system("X=made; pipes emit $X \"$X  x\" '$X' > var.txt") == 0 && strcmp(contents("var.txt"), "made made  x $X\n") == 0,
          "a variable, quoted and not");
    check(system("GREETING=for-one pipes env GREETING > one.txt") == 0 && strcmp(contents("one.txt"), "for-one\n") == 0,
          "a variable for one command");
    check(system("> w1.dat; > w2.dat; pipes emit w*.dat 'w*'.dat > glob.txt") == 0
              && strcmp(contents("glob.txt"), "w1.dat w2.dat w*.dat\n") == 0, "> alone makes a file; a wildcard, and a quoted one");
    check(mkdir("sub", 0755) == 0 && system("cd sub && pipes emit here > here.txt") == 0
              && strcmp(contents("sub/here.txt"), "here\n") == 0 && strcmp(contents("here.txt"), "(none)") == 0,
          "cd: in sh, not here");
    check(system("echo $(pipes emit inner) `pipes emit back` > sub.txt") == 0 && strcmp(contents("sub.txt"), "inner back\n") == 0,
          "$(...) and backquotes; echo with a redirection");
    check(system("echo piped | pipes count > echoed.txt") == 0 && strcmp(contents("echoed.txt"), "6 bytes\n") == 0,
          "a builtin in a pipeline");
    check(system("set -e; pipes exit 3; pipes emit not > e.txt") == 3 << 8 && strcmp(contents("e.txt"), "(none)") == 0,
          "set -e stops at a failure");
    check(system("[ -f in.txt ] && test -d sub && ! test -d in.txt") == 0, "test and [");
    check(system("exit 7") == 7 << 8, "exit");
    check(system("nothing && pipes emit x") == 127 << 8, "a command not found in a list: 127");
    check(system("cat << END") == 2 << 8, "a here-document with no lines: a mistake, 2");
    check(system("for w in a b c; do pipes emit $w; done > for.txt") == 0 && strcmp(contents("for.txt"), "a\nb\nc\n") == 0,
          "for, its output a file");
    check(system("if [ -f in.txt ]; then pipes emit yes > if.txt; else pipes emit no > if.txt; fi") == 0
              && strcmp(contents("if.txt"), "yes\n") == 0, "if and else");
    check(system("n=0; while [ $n -lt 3 ]; do n=$((n + 1)); done; pipes emit $n > while.txt") == 0
              && strcmp(contents("while.txt"), "3\n") == 0, "while, and $((...))");
    check(system("pipes emit one two | while read a b; do pipes emit \"$b-$a\"; done > read.txt") == 0
              && strcmp(contents("read.txt"), "two-one\n") == 0, "a loop in a pipeline, reading its input");
    check(system("f() { pipes emit \"f:$1\"; }; f x > fn.txt") == 0 && strcmp(contents("fn.txt"), "f:x\n") == 0,
          "a function, with a redirection");
    check(system("case main.c in *.h) exit 3;; *.c) exit 4;; esac") == 4 << 8, "case");
    check(system("f=a/b.txt; pipes emit ${f##*/} ${f%.txt} > strip.txt") == 0 && strcmp(contents("strip.txt"), "b.txt a/b\n") == 0,
          "${f##*/} and ${f%.txt}");
    check(system("(cd sub && pipes emit inside) > paren.txt") == 0 && strcmp(contents("paren.txt"), "inside\n") == 0
              && strcmp(contents("sub/paren.txt"), "(none)") == 0, "a subshell, its output a file here");
    FILE *stream = popen("pipes emit one; pipes emit two", "r");
    char line[64] = "", second[64] = "";
    check(stream != NULL && fgets(line, sizeof line, stream) && fgets(second, sizeof second, stream)
              && strcmp(line, "one\n") == 0 && strcmp(second, "two\n") == 0, "popen of a list, reading");
    check(stream != NULL && pclose(stream) == 0, "and its status");
}

/* Descriptors beyond 0, 1 and 2 go to the programs this one starts,
 * as exec leaves them on Unix - unless marked close-on-exec. */
static void inherited(void)
{
    posix_spawn_file_actions_t actions;
    char buffer[64];
    int out[2];

    /* A file open here, read part way: the child reads on from there. */
    int fd = open("in.txt", O_RDONLY);
    check(fd >= 3 && lseek(fd, 4990, SEEK_SET) == 4990, "in.txt open at 4990");
    char number[8];
    snprintf(number, sizeof number, "%d", fd);
    char *fdcount[] = {"pipes", "fdcount", number, NULL};
    pipe2(out, O_CLOEXEC);
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_adddup2(&actions, out[1], 1);
    pid_t child = start(&actions, fdcount);
    close(out[1]);
    drain(out[0], buffer, sizeof buffer);
    close(out[0]);
    check(child > 0 && status_of(child) == 0 && strcmp(buffer, "10 bytes\n") == 0, "a file open here: the child has it, from 4990 on");

    /* Marked close-on-exec: the child has no such descriptor. */
    check(fcntl(fd, F_GETFD) == 0 && fcntl(fd, F_SETFD, FD_CLOEXEC) == 0 && fcntl(fd, F_GETFD) == FD_CLOEXEC,
          "F_SETFD marks it close-on-exec, F_GETFD says so");
    pipe2(out, O_CLOEXEC);
    posix_spawn_file_actions_destroy(&actions);
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_adddup2(&actions, out[1], 1);
    child = start(&actions, fdcount);
    close(out[1]);
    drain(out[0], buffer, sizeof buffer);
    close(out[0]);
    char closed[32];
    snprintf(closed, sizeof closed, "%d: closed\n", fd);
    check(child > 0 && strcmp(buffer, closed) == 0, "close-on-exec: not given");
    status_of(child);
    close(fd);

    /* addclose of one that would be inherited. */
    fd = open("in.txt", O_RDONLY);
    pipe2(out, O_CLOEXEC);
    posix_spawn_file_actions_destroy(&actions);
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_adddup2(&actions, out[1], 1);
    posix_spawn_file_actions_addclose(&actions, fd);
    child = start(&actions, fdcount);
    close(out[1]);
    drain(out[0], buffer, sizeof buffer);
    close(out[0]);
    check(child > 0 && strcmp(buffer, closed) == 0, "addclose: not given");
    status_of(child);
    close(fd);
    posix_spawn_file_actions_destroy(&actions);

    /* A plain pipe's write end: the child writes on it, by number. */
    int ends[2];
    check(pipe(ends) == 0, "a pipe, not close-on-exec");
    snprintf(number, sizeof number, "%d", ends[1]);
    char *fdwrite[] = {"pipes", "fdwrite", number, "down-the-pipe", NULL};
    child = start(NULL, fdwrite);
    close(ends[1]);
    drain(ends[0], buffer, sizeof buffer);
    close(ends[0]);
    check(child > 0 && status_of(child) == 0 && strcmp(buffer, "down-the-pipe\n") == 0, "a pipe's end, inherited, written to");

    /* addopen to 3, and adddup2 to 5. */
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_addopen(&actions, 3, "three.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    char *to3[] = {"pipes", "fdwrite", "3", "into-three", NULL};
    child = start(&actions, to3);
    posix_spawn_file_actions_destroy(&actions);
    check(child > 0 && status_of(child) == 0 && strcmp(contents("three.txt"), "into-three\n") == 0, "addopen to 3");
    pipe2(out, O_CLOEXEC);
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_adddup2(&actions, out[1], 5);
    char *to5[] = {"pipes", "fdwrite", "5", "into-five", NULL};
    child = start(&actions, to5);
    posix_spawn_file_actions_destroy(&actions);
    close(out[1]);
    drain(out[0], buffer, sizeof buffer);
    close(out[0]);
    check(child > 0 && status_of(child) == 0 && strcmp(buffer, "into-five\n") == 0,
          "adddup2 to 5 of a close-on-exec end: given, as 5");

    /* Through sh: exec, here-documents, read from a descriptor, and
     * redirections of builtins and loops, done in the shell itself. */
    FILE *stream = popen("exec 3< in.txt; pipes fdcount 3", "r");
    char line[64] = "";
    check(stream && fgets(line, sizeof line, stream) && strcmp(line, "5000 bytes\n") == 0 && pclose(stream) == 0,
          "sh: exec 3< - its programs get 3");
    check(system("pipes count > here.txt <<END\nfour\nEND") == 0 && strcmp(contents("here.txt"), "5 bytes\n") == 0,
          "sh: a here-document");
    check(system("X=x; cat > here2.txt <<-'END'\n\t$X\n\tEND\n") == 0 && strcmp(contents("here2.txt"), "$X\n") == 0,
          "sh: <<- and a quoted delimiter");
    check(system("exec 4< in.txt; read -r l <&4; read -r m <&4; echo \"${#l} [$m] $?\" > read4.txt") == 0
              && strcmp(contents("read4.txt"), "4999 [] 1\n") == 0, "sh: read <&4 twice: one offset, so the second is at the end");
    check(system("n=0; while read l; do n=$((n + 1)); done <<E\na\nb\nc\nE\necho $n > loop.txt") == 0
              && strcmp(contents("loop.txt"), "3\n") == 0, "sh: while read ... done << - its variables kept");
    check(system("{ echo a; pipes emit b; echo c; } > group.txt") == 0 && strcmp(contents("group.txt"), "a\nb\nc\n") == 0,
          "sh: a group's output a file - the shell's and a program's, in order");
    check(system("{ read a; pipes count; } < in.txt > split.txt") == 0 && strcmp(contents("split.txt"), "0 bytes\n") == 0,
          "sh: a program reads on from where read got to");
    check(system("echo first > rw.txt; echo X 1<> rw.txt") == 0 && strcmp(contents("rw.txt"), "X\nrst\n") == 0,
          "sh: <> writes over the start");
    check(system("exec > exec.txt; echo one; pipes emit two") == 0 && strcmp(contents("exec.txt"), "one\ntwo\n") == 0,
          "sh: exec > - the shell's own output, for good");
    check(system("exec pipes exit 6; exit 1") == 6 << 8, "sh: exec COMMAND - the shell ends with its status");
    check(system("read x < none.txt") == 1 << 8, "sh: read < a file not there: 1");
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
    if (argc == 3 && strcmp(argv[1], "fdcount") == 0) {
        int fd = atoi(argv[2]);
        char buffer[512];
        long total = 0;
        ssize_t n;
        while ((n = read(fd, buffer, sizeof buffer)) > 0)
            total += n;
        if (n < 0)
            printf("%d: closed\n", fd);
        else
            printf("%ld bytes\n", total);
        return 0;
    }
    if (argc == 4 && strcmp(argv[1], "fdwrite") == 0) {
        char line[256];
        int length = snprintf(line, sizeof line, "%s\n", argv[3]);
        return write(atoi(argv[2]), line, (size_t)length) == length ? 0 : 1;
    }
    if (argc == 3 && strcmp(argv[1], "env") == 0) {
        const char *value = getenv(argv[2]);
        printf("%s\n", value ? value : "(unset)");
        return 0;
    }

    in_one_program();
    between_programs();
    files();
    commands();
    shell();
    inherited();
    environment();
    if (failures) {
        printf("pipes: %d of %d checks failed\n", failures, checks);
        return 1;
    }
    printf("pipes: %d checks, all passed\n", checks);
    return 0;
}
