/* tmp.c - /tmp in memory (journal entry 126, J64): a second file
 * service, its files in memory, mounted at /tmp in every program's
 * namespace. handles-test.sh boots TheTaaJ and runs
 *
 *   with programs tmptest    the checks; "tmp: N checks, all passed"
 *
 * which starts tmptest again as a child, to be:
 *   tmptest read PATH TEXT   exits 0 if the file PATH holds TEXT
 *   tmptest boxed            in a tree of its own: exits 0 if /tmp is not
 *                            reachable from there
 *   tmptest show PATH        prints the file PATH: what the shell put
 *                            in /tmp, as a program sees it
 *
 * The script then looks at the disk from the host: nothing written to
 * /tmp is on it. */

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
        printf("tmp: failed: %s (errno %d)\n", what, errno);
    }
}

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

static int put(const char *path, const char *text)
{
    int fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (fd < 0)
        return -1;
    size_t length = strlen(text);
    int written = write(fd, text, length) == (ssize_t)length;
    close(fd);
    return written ? 0 : -1;
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

/* A child: its exit code, or -1. With `root`, in a tree of its own. */
static int child(char **argv, const char *root)
{
    posix_spawn_file_actions_t actions;
    posix_spawn_file_actions_init(&actions);
    if (root != NULL)
        posix_spawn_file_actions_addroot_np(&actions, root);
    pid_t pid;
    int started = posix_spawnp(&pid, "tmptest", &actions, NULL, argv, environ) == 0;
    posix_spawn_file_actions_destroy(&actions);
    int status;
    if (!started || waitpid(pid, &status, 0) != pid || !WIFEXITED(status))
        return -1;
    return WEXITSTATUS(status);
}

int main(int argc, char **argv)
{
    if (argc == 4 && strcmp(argv[1], "read") == 0)
        return holds(argv[2], argv[3]) ? 0 : 1;
    if (argc == 3 && strcmp(argv[1], "show") == 0) {
        char got[64];
        int fd = open(argv[2], O_RDONLY);
        int n = fd < 0 ? -1 : (int)read(fd, got, sizeof got - 1);
        if (n < 0)
            return 1;
        got[n] = 0;
        fputs(got, stdout);
        return 0;
    }
    if (argc == 2 && strcmp(argv[1], "boxed") == 0)
        return access("/tmp/a", F_OK) == 0 ? 1 : 0;

    struct stat status;
    check(stat("/tmp", &status) == 0 && S_ISDIR(status.st_mode), "/tmp is a directory");
    check(listed("/", "tmp"), "the root's listing shows it");
    check(put("/tmp/a", "temporary") == 0 && holds("/tmp/a", "temporary"), "a file in /tmp");
    check(listed("/tmp", "a"), "listed in /tmp");
    check(mkdir("/tmp/d", 0755) == 0 && put("/tmp/d/b", "deeper") == 0 && holds("/tmp/d/b", "deeper"), "a directory in /tmp");
    check(rename("/tmp/d/b", "/tmp/c") == 0 && holds("/tmp/c", "deeper"), "a rename inside /tmp");
    errno = 0;
    check(rename("/tmp/c", "/c") == -1 && errno == EXDEV, "out of /tmp: EXDEV, another file system");
    check(chdir("/tmp") == 0 && holds("a", "temporary") && chdir("/") == 0, "chdir into /tmp");
    char *reader[] = {"tmptest", "read", "/tmp/a", "temporary", NULL};
    check(child(reader, NULL) == 0, "a child sees the same /tmp");
    mkdir("tmpbox", 0755);
    char *boxed[] = {"tmptest", "boxed", NULL};
    check(child(boxed, "tmpbox") == 0, "a child in a tree of its own has no /tmp");
    rmdir("tmpbox");
    check(unlink("/tmp/c") == 0 && rmdir("/tmp/d") == 0, "removed from /tmp");
    /* /tmp/a stays: the script looks for it on the disk, and must not
     * find it. */
    if (failures == 0)
        printf("tmp: %d checks, all passed\n", checks);
    else
        printf("tmp: %d of %d checks failed\n", failures, checks);
    return failures != 0;
}
