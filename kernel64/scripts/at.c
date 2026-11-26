/* at.c - names from a directory descriptor (journal entry 120, J63):
 * openat, fstatat, mkdirat, unlinkat, renameat, faccessat, utimensat;
 * and a program given one directory as its whole tree
 * (posix_spawn_file_actions_addroot_np). handles-test.sh boots TheTaaJ
 * and runs
 *
 *   with programs at    the checks; "at: N checks, all passed"
 *
 * which starts at again as a child, to be:
 *   at sandboxed        in a tree of its own: exits 0 if it sees only
 *                       its directory, or the number of the first
 *                       check that failed */

#include <errno.h>
#include <fcntl.h>
#include <spawn.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <taaj.h>
#include <unistd.h>

extern char **environ;

static int checks, failures;

static void check(int held, const char *what)
{
    checks++;
    if (!held) {
        failures++;
        printf("at: failed: %s (errno %d)\n", what, errno);
    }
}

/* Whether the file `path` holds exactly `text`. */
static int holds(const char *path, const char *text)
{
    char got[64];
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return 0;
    int n = (int)read(fd, got, sizeof got - 1);
    close(fd);
    if (n < 0)
        return 0;
    got[n] = 0;
    return strcmp(got, text) == 0;
}

/* Makes the file `name` in `dirfd` hold `text`: 0, or -1. */
static int put_at(int dirfd, const char *name, const char *text)
{
    int fd = openat(dirfd, name, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (fd < 0)
        return -1;
    size_t length = strlen(text);
    int written = write(fd, text, length) == (ssize_t)length;
    close(fd);
    return written ? 0 : -1;
}

static void names_from_a_directory(void)
{
    mkdir("atdir", 0755);
    int dir = open("atdir", O_RDONLY | O_DIRECTORY);
    check(dir >= 0, "open the directory atdir");
    check(put_at(dir, "f", "one") == 0 && holds("atdir/f", "one"), "openat makes atdir/f");
    struct stat status;
    check(fstatat(dir, "f", &status, 0) == 0 && status.st_size == 3, "fstatat sees it");
    check(fstatat(dir, "", &status, AT_EMPTY_PATH) == 0 && S_ISDIR(status.st_mode), "fstatat with AT_EMPTY_PATH: the directory itself");
    errno = 0;
    check(fstatat(dir, "", &status, 0) == -1 && errno == ENOENT, "an empty name without it: ENOENT");
    check(mkdirat(dir, "sub", 0755) == 0 && stat("atdir/sub", &status) == 0 && S_ISDIR(status.st_mode), "mkdirat makes atdir/sub");
    check(renameat(dir, "f", dir, "sub/g") == 0 && holds("atdir/sub/g", "one"), "renameat inside it");
    check(faccessat(dir, "sub/g", R_OK | W_OK, 0) == 0, "faccessat: there, and may change");
    errno = 0;
    check(faccessat(dir, "nothing", F_OK, 0) == -1 && errno == ENOENT, "faccessat: nothing there");
    check(utimensat(dir, "sub/g", NULL, 0) == 0, "utimensat from the directory");
    check(access("atdir/../atdir/sub/g", F_OK) == 0 && faccessat(dir, "../atdir/sub/g", F_OK, 0) == 0, ".. by name, from where it was opened");
    check(fstatat(dir, "/atdir/sub/g", &status, 0) == 0, "a name from the root ignores the directory");

    /* The point of it: the directory, wherever it has gone. */
    check(rename("atdir", "moved") == 0, "rename the directory while it is open");
    check(faccessat(dir, "sub/g", F_OK, 0) == 0, "the directory still finds it, under its new name");
    int g = openat(dir, "sub/g", O_RDONLY);
    char got[8] = {0};
    check(g >= 0 && read(g, got, 3) == 3 && strcmp(got, "one") == 0, "and reads it");
    close(g);
    check(put_at(dir, "h", "two") == 0 && holds("moved/h", "two"), "a file made through it is in moved");
    errno = 0;
    check(unlinkat(dir, "sub", 0) == -1 && errno == EISDIR, "unlinkat on a directory: EISDIR");
    errno = 0;
    check(unlinkat(dir, "h", AT_REMOVEDIR) == -1 && errno == ENOTDIR, "AT_REMOVEDIR on a file: ENOTDIR");
    check(unlinkat(dir, "sub/g", 0) == 0 && unlinkat(dir, "h", 0) == 0, "unlinkat the files");
    check(unlinkat(dir, "sub", AT_REMOVEDIR) == 0 && access("moved/sub", F_OK) == -1, "unlinkat AT_REMOVEDIR the directory");

    int file = open("moved", O_RDONLY | O_DIRECTORY);
    int plain = openat(file, "x", O_CREAT | O_WRONLY, 0644);
    close(plain);
    int not_directory = openat(file, "x", O_RDONLY);
    errno = 0;
    check(openat(not_directory, "y", O_RDONLY) == -1 && errno == ENOTDIR, "a file is no directory: ENOTDIR");
    errno = 0;
    check(openat(99, "y", O_RDONLY) == -1 && errno == EBADF, "no descriptor: EBADF");
    close(not_directory);
    unlinkat(file, "x", 0);
    close(file);
    close(dir);
    rmdir("moved");
}

static void a_tree_of_its_own(void)
{
    mkdir("box", 0755);
    check(put_at(AT_FDCWD, "box/inside", "in the box") == 0, "write box/inside");
    check(put_at(AT_FDCWD, "outside", "out of reach") == 0, "write outside, next to box");
    posix_spawn_file_actions_t actions;
    posix_spawn_file_actions_init(&actions);
    check(posix_spawn_file_actions_addroot_np(&actions, "box") == 0, "addroot_np box");
    char *argv[] = {"at", "sandboxed", NULL};
    pid_t child;
    int started = posix_spawnp(&child, "at", &actions, NULL, argv, environ) == 0;
    posix_spawn_file_actions_destroy(&actions);
    check(started, "start a child in the box");
    int status = -1;
    check(started && waitpid(child, &status, 0) == child && WIFEXITED(status), "it ended");
    if (WIFEXITED(status) && WEXITSTATUS(status) != 0)
        printf("at: the sandboxed child's check %d failed\n", WEXITSTATUS(status));
    check(WIFEXITED(status) && WEXITSTATUS(status) == 0, "it saw its directory, and nothing else");
    check(holds("box/made", "from the box"), "what it wrote is in box");

    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_addroot_np(&actions, "outside");
    errno = 0;
    check(posix_spawnp(&child, "at", &actions, NULL, argv, environ) == ENOTDIR, "a file can't be a tree: ENOTDIR");
    posix_spawn_file_actions_destroy(&actions);
    unlink("box/inside");
    unlink("box/made");
    rmdir("box");
    unlink("outside");
}

/* The child, in box: what it can and can't reach. */
static int sandboxed(void)
{
    char where[16];
    if (getcwd(where, sizeof where) == NULL || strcmp(where, "/") != 0)
        return 1;
    if (!holds("/inside", "in the box"))
        return 2;
    if (access("/outside", F_OK) == 0 || errno != ENOENT)
        return 3;
    if (access("/../outside", F_OK) == 0 || access("../../outside", F_OK) == 0)
        return 4;
    if (put_at(AT_FDCWD, "/made", "from the box") != 0)
        return 5;
    int root = open("/", O_RDONLY | O_DIRECTORY);
    if (root < 0 || faccessat(root, "../outside", F_OK, 0) == 0)
        return 6;
    close(root);
    /* Without libc's namespace, nothing is left: the box was the only
     * way to the files it was given. */
    if (taaj_unmount("/") != 0)
        return 7;
    if (access("/outside", F_OK) == 0 || access("/inside", F_OK) == 0)
        return 8;
    return 0;
}

int main(int argc, char **argv)
{
    if (argc == 2 && strcmp(argv[1], "sandboxed") == 0)
        return sandboxed();
    names_from_a_directory();
    a_tree_of_its_own();
    if (failures == 0)
        printf("at: %d checks, all passed\n", checks);
    else
        printf("at: %d of %d checks failed\n", failures, checks);
    return failures != 0;
}
