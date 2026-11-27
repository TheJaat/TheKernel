/* dev.c - /dev (journal entry 127, J65): null, zero, full, random and
 * urandom from a service of their own, mounted at /dev in every program
 * given files; /dev/stdin, /dev/stdout, /dev/stderr and /dev/tty from
 * libc. handles-test.sh boots TheTaaJ and runs
 *
 *   with programs devtest    the checks; "dev: N checks, all passed"
 *
 * which starts devtest again as a child, to be:
 *   devtest zero             exits 0 if /dev/zero reads zeros
 *   devtest boxed            in a tree of its own: exits 0 if /dev is
 *                            not reachable from there
 *
 * The program is devtest, not dev: a file "dev" in the root would be
 * hidden under the mount. */

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <spawn.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

static int checks, failures;

static void check(int held, const char *what)
{
    checks++;
    if (!held) {
        failures++;
        printf("dev: failed: %s (errno %d)\n", what, errno);
    }
}

static int listed(const char *directory, const char *name)
{
    DIR *dir = opendir(directory);
    if (dir == NULL)
        return 0;
    int found = 0;
    struct dirent *entry;
    while ((entry = readdir(dir)) != NULL)
        found |= strcmp(entry->d_name, name) == 0;
    closedir(dir);
    return found;
}

/* `length` bytes of `path`, read with one read: how many it gave. */
static int read_some(const char *path, unsigned char *out, int length)
{
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return -1;
    int n = (int)read(fd, out, length);
    close(fd);
    return n;
}

static int all(const unsigned char *bytes, int length, unsigned char value)
{
    for (int i = 0; i < length; i++)
        if (bytes[i] != value)
            return 0;
    return 1;
}

/* A child: its exit code, or -1. With `root`, in a tree of its own. */
static int child(char **argv, const char *root)
{
    posix_spawn_file_actions_t actions;
    posix_spawn_file_actions_init(&actions);
    if (root != NULL)
        posix_spawn_file_actions_addroot_np(&actions, root);
    pid_t pid;
    int started = posix_spawnp(&pid, "devtest", &actions, NULL, argv, environ) == 0;
    posix_spawn_file_actions_destroy(&actions);
    int status;
    if (!started || waitpid(pid, &status, 0) != pid || !WIFEXITED(status))
        return -1;
    return WEXITSTATUS(status);
}

static void the_devices(void)
{
    struct stat status;
    check(stat("/dev", &status) == 0 && S_ISDIR(status.st_mode), "/dev is a directory");
    check(listed("/", "dev"), "the root's listing shows it");
    const char *names[] = {"null", "zero", "full", "random", "urandom"};
    int every = 1;
    for (int i = 0; i < 5; i++)
        every &= listed("/dev", names[i]);
    check(every, "/dev lists null, zero, full, random and urandom");
    check(stat("/dev/null", &status) == 0 && status.st_size == 0, "a device's size is 0");

    unsigned char bytes[64];
    check(read_some("/dev/null", bytes, sizeof bytes) == 0, "/dev/null: the end at once");
    int fd = open("/dev/null", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    check(fd >= 0 && write(fd, "gone", 4) == 4, "/dev/null takes a write, opened as > does");
    close(fd);
    FILE *null = fopen("/dev/null", "w");
    check(null != NULL && fprintf(null, "%s", "nothing to see") > 0 && fclose(null) == 0, "and from stdio");

    memset(bytes, 9, sizeof bytes);
    check(read_some("/dev/zero", bytes, sizeof bytes) == 64 && all(bytes, 64, 0), "/dev/zero: zeros");
    fd = open("/dev/zero", O_RDONLY);
    check(fd >= 0 && lseek(fd, 1 << 20, SEEK_SET) == 1 << 20 && read(fd, bytes, 8) == 8 && all(bytes, 8, 0), "zeros anywhere in it");
    close(fd);

    unsigned char one[32], two[32];
    check(read_some("/dev/urandom", one, 32) == 32 && read_some("/dev/urandom", two, 32) == 32 && memcmp(one, two, 32) != 0 && !all(one, 32, 0), "/dev/urandom: different bytes each time");
    check(read_some("/dev/random", one, 32) == 32 && memcmp(one, two, 32) != 0, "/dev/random too");

    fd = open("/dev/full", O_WRONLY);
    errno = 0;
    check(fd >= 0 && write(fd, "x", 1) == -1 && errno == ENOSPC, "/dev/full: no room, ENOSPC");
    close(fd);
    memset(bytes, 9, 16);
    check(read_some("/dev/full", bytes, 16) == 16 && all(bytes, 16, 0), "it reads zeros");

    errno = 0;
    check(open("/dev/new", O_WRONLY | O_CREAT, 0644) == -1 && errno == EROFS, "nothing new in /dev: EROFS");
    errno = 0;
    check(mkdir("/dev/d", 0755) == -1 && errno == EROFS, "no directory: EROFS");
    errno = 0;
    check(unlink("/dev/null") == -1 && errno == EROFS, "/dev/null can't be removed");
    errno = 0;
    check(rename("/dev/zero", "/dev/nought") == -1 && errno == EROFS, "or a device renamed");
    errno = 0;
    check(open("/dev/null/x", O_RDONLY) == -1 && errno == ENOTDIR, "a device is no directory");
}

static void the_programs_own(void)
{
    int out = open("/dev/stdout", O_WRONLY);
    const char *line = "dev: written to /dev/stdout\n";
    check(out > 2 && write(out, line, strlen(line)) == (ssize_t)strlen(line), "/dev/stdout is standard output");
    close(out);
    int error = open("/dev/stderr", O_WRONLY);
    check(error > 2, "/dev/stderr opens");
    close(error);
    int tty = open("/dev/tty", O_WRONLY);
    check(tty > 2 && isatty(tty), "/dev/tty: the screen, and a terminal");
    close(tty);
    int in = open("/dev/stdin", O_RDONLY);
    check(in > 2, "/dev/stdin opens");
    close(in);
}

int main(int argc, char **argv)
{
    if (argc == 2 && strcmp(argv[1], "zero") == 0) {
        unsigned char bytes[8] = {9};
        return read_some("/dev/zero", bytes, 8) == 8 && all(bytes, 8, 0) ? 0 : 1;
    }
    if (argc == 2 && strcmp(argv[1], "boxed") == 0)
        return access("/dev/null", F_OK) == 0 ? 1 : 0;

    the_devices();
    the_programs_own();
    char *zero[] = {"devtest", "zero", NULL};
    check(child(zero, NULL) == 0, "a child has /dev");
    mkdir("devbox", 0755);
    char *boxed[] = {"devtest", "boxed", NULL};
    check(child(boxed, "devbox") == 0, "a child in a tree of its own has none");
    rmdir("devbox");
    if (failures == 0)
        printf("dev: %d checks, all passed\n", checks);
    else
        printf("dev: %d of %d checks failed\n", failures, checks);
    return failures != 0;
}
