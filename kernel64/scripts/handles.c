/* handles.c - open files as handles (journal entry 115, J60 and J61):
 * a descriptor is the file, not its name. handles-test.sh boots TheTaaJ
 * and runs
 *
 *   with programs handles    the checks; "handles: N checks, all passed"
 *
 * which starts handles again as a child, to be:
 *   handles write-fd N TEXT  writes TEXT on descriptor N, then exits 0 */

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
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
        printf("handles: failed: %s (errno %d)\n", what, errno);
    }
}

/* The whole file `path` names into `out` (at most size - 1 bytes, and a
 * zero); its length, or -1 if it cannot be opened. */
static int contents(const char *path, char *out, int size)
{
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return -1;
    int n = (int)read(fd, out, size - 1);
    close(fd);
    out[n < 0 ? 0 : n] = 0;
    return n;
}

/* Whether the root's listing has `name` in it. */
static int listed(const char *name)
{
    DIR *dir = opendir("/");
    if (dir == NULL)
        return -1;
    int found = 0;
    struct dirent *entry;
    while ((entry = readdir(dir)) != NULL)
        found |= strcmp(entry->d_name, name) == 0;
    closedir(dir);
    return found;
}

static void follows_a_rename(void)
{
    char text[64];
    unlink("a");
    int fd = open("a", O_CREAT | O_RDWR | O_TRUNC);
    check(fd >= 0, "open a, made");
    check(write(fd, "hello", 5) == 5, "write hello");
    check(rename("a", "b") == 0, "rename a to b while open");
    check(write(fd, " world", 6) == 6, "write after the rename");
    check(contents("b", text, sizeof text) == 11 && strcmp(text, "hello world") == 0, "both writes are in b");
    check(access("a", F_OK) == -1 && errno == ENOENT, "nothing was made at the old name");

    mkdir("d", 0755);
    check(rename("b", "d/c") == 0, "rename b into a directory");
    struct stat status;
    check(fstat(fd, &status) == 0 && status.st_size == 11, "fstat asks the file, wherever it is");
    check(lseek(fd, 0, SEEK_END) == 11, "SEEK_END finds its end");
    check(ftruncate(fd, 5) == 0, "ftruncate through the descriptor");
    check(contents("d/c", text, sizeof text) == 5 && strcmp(text, "hello") == 0, "the file under its new name is cut");
    check(rename("d", "e") == 0, "rename the directory it is in");
    check(pread(fd, text, 5, 0) == 5 && memcmp(text, "hello", 5) == 0, "pread after the directory moved");
    close(fd);
    unlink("e/c");
    rmdir("e");
}

static void outlives_a_removal(void)
{
    char text[64];
    int fd = open("doomed", O_CREAT | O_RDWR | O_TRUNC);
    check(fd >= 0 && write(fd, "still here", 10) == 10, "open and write doomed");
    check(unlink("doomed") == 0, "unlink while open");
    check(access("doomed", F_OK) == -1 && errno == ENOENT, "the name is gone");
    check(pread(fd, text, 10, 0) == 10 && memcmp(text, "still here", 10) == 0, "the file is not: it reads");
    check(pwrite(fd, "!", 1, 10) == 1, "and it writes");
    struct stat status;
    check(fstat(fd, &status) == 0 && status.st_size == 11, "and fstat sees it");
    check(listed(".taaj-removed") == 0, "the place it waits in is not listed");
    int fresh = open("doomed", O_CREAT | O_EXCL | O_WRONLY);
    check(fresh >= 0, "a new file can take the name");
    close(fresh);
    check(contents("doomed", text, sizeof text) == 0, "and is a new, empty one");
    close(fd);
    check(listed(".taaj-removed") == 0 && listed("doomed") == 1, "closed: the old one is gone for good");
    unlink("doomed");
}

static void outlives_a_replacing_rename(void)
{
    char text[64];
    int out = open("x", O_CREAT | O_WRONLY | O_TRUNC);
    check(out >= 0 && write(out, "old", 3) == 3, "write x");
    close(out);
    out = open("y", O_CREAT | O_WRONLY | O_TRUNC);
    check(out >= 0 && write(out, "new", 3) == 3, "write y");
    close(out);
    int reader = open("x", O_RDONLY);
    check(reader >= 0, "open x to read");
    check(rename("y", "x") == 0, "rename y over x");
    check(read(reader, text, sizeof text) == 3 && memcmp(text, "old", 3) == 0, "the reader still has the old x");
    check(contents("x", text, sizeof text) == 3 && strcmp(text, "new") == 0, "the name has the new one");
    close(reader);
    unlink("x");
}

static void passed_to_a_child(void)
{
    char text[64];
    unlink("passed");
    unlink("moved");
    int fd = open("passed", O_CREAT | O_RDWR | O_TRUNC);
    check(fd >= 0 && write(fd, "parent ", 7) == 7, "write passed");
    check(rename("passed", "moved") == 0, "rename it before the child starts");
    posix_spawn_file_actions_t actions;
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_adddup2(&actions, fd, 3);
    char *argv[] = {"handles", "write-fd", "3", "child", NULL};
    pid_t child;
    int started = posix_spawnp(&child, "handles", &actions, NULL, argv, environ) == 0;
    posix_spawn_file_actions_destroy(&actions);
    check(started, "start a child with the file as its descriptor 3");
    int status = -1;
    check(started && waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 0, "the child wrote and ended");
    check(contents("moved", text, sizeof text) == 12 && strcmp(text, "parent child") == 0, "its write is in the file, under its new name");
    close(fd);
    unlink("moved");
}

static void what_open_refuses(void)
{
    int fd = open("f", O_CREAT | O_WRONLY | O_TRUNC);
    check(fd >= 0, "make f");
    char byte;
    check(read(fd, &byte, 1) == -1 && errno == EBADF, "no reading what was opened to write");
    close(fd);
    fd = open("f", O_RDONLY);
    check(write(fd, "x", 1) == -1 && errno == EBADF, "no writing what was opened to read");
    check(futimens(fd, NULL) == 0, "but it may be stamped, as POSIX lets anyone who may write it");
    close(fd);
    check(open("f", O_CREAT | O_EXCL | O_WRONLY) == -1 && errno == EEXIST, "O_EXCL: it is there");
    check(open("f", O_RDONLY | O_DIRECTORY) == -1 && errno == ENOTDIR, "O_DIRECTORY on a file");
    mkdir("g", 0755);
    check(open("g", O_RDONLY) == -1 && errno == EISDIR, "a directory, without O_DIRECTORY");
    check(open("g", O_WRONLY | O_DIRECTORY) == -1 && errno == EISDIR, "a directory, to write");
    int dir = open("g", O_RDONLY | O_DIRECTORY);
    check(dir >= 0, "a directory, with O_DIRECTORY");
    DIR *listing = fdopendir(dir);
    check(listing != NULL, "fdopendir on it");
    if (listing != NULL)
        closedir(listing);
    check(open("nothing", O_RDONLY) == -1 && errno == ENOENT, "nothing there");
    unlink("f");
    rmdir("g");
}

int main(int argc, char **argv)
{
    if (argc == 4 && strcmp(argv[1], "write-fd") == 0) {
        int fd = atoi(argv[2]);
        size_t length = strlen(argv[3]);
        return write(fd, argv[3], length) == (ssize_t)length ? 0 : 1;
    }
    follows_a_rename();
    outlives_a_removal();
    outlives_a_replacing_rename();
    passed_to_a_child();
    what_open_refuses();
    if (failures == 0)
        printf("handles: %d checks, all passed\n", checks);
    else
        printf("handles: %d of %d checks failed\n", failures, checks);
    return failures != 0;
}
