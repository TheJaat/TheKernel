/* namespace.c - a namespace per process (journal entry NNN, J62):
 * taaj_mount sees a directory at a path of this program's own, and
 * posix_spawn hands the mounts to a child. handles-test.sh boots
 * TheTaaJ and runs
 *
 *   with programs namespace    the checks; "namespace: N checks, all passed"
 *
 * which starts namespace again as a child, to be:
 *   namespace read PATH TEXT     exits 0 if the file PATH holds TEXT
 *   namespace write PATH TEXT    writes TEXT into PATH, exits 0 if it could */

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
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
        printf("namespace: failed: %s (errno %d)\n", what, errno);
    }
}

/* Whether the file `path` holds exactly `text`. */
static int holds(const char *path, const char *text)
{
    char got[128];
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

/* Makes `path` hold `text`: 0, or -1 and errno. */
static int put(const char *path, const char *text)
{
    int fd = open(path, O_CREAT | O_WRONLY | O_TRUNC);
    if (fd < 0)
        return -1;
    size_t length = strlen(text);
    int written = write(fd, text, length) == (ssize_t)length;
    close(fd);
    return written ? 0 : -1;
}

/* Whether the listing of `directory` has `name` in it. */
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

/* A child, `namespace MODE PATH TEXT`: its exit code, or -1. */
static int child(const char *mode, const char *path, const char *text)
{
    char *argv[] = {"namespace", (char *)mode, (char *)path, (char *)text, NULL};
    pid_t pid;
    if (posix_spawnp(&pid, "namespace", NULL, NULL, argv, environ) != 0)
        return -1;
    int status;
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status))
        return -1;
    return WEXITSTATUS(status);
}

static void a_directory_seen_elsewhere(void)
{
    mkdir("ns", 0755);
    mkdir("ns/real", 0755);
    check(put("ns/real/f", "inside") == 0, "write ns/real/f");
    check(taaj_mount("/m", "ns/real", 0) == 0, "mount ns/real at /m, which is not there");
    check(holds("/m/f", "inside"), "/m/f is ns/real/f");
    struct stat status;
    check(stat("/m", &status) == 0 && S_ISDIR(status.st_mode), "/m is a directory");
    check(listed("/", "m"), "the root's listing shows the mount");
    check(put("/m/new", "made through the mount") == 0 && holds("ns/real/new", "made through the mount"), "a file made under /m is in ns/real");
    check(rename("/m/f", "/m/g") == 0 && holds("ns/real/g", "inside"), "a rename under one mount");
    errno = 0;
    check(rename("/m/g", "/outside") == -1 && errno == EXDEV, "a rename out of the mount: EXDEV");
    errno = 0;
    check(rmdir("/m") == -1 && errno == EBUSY, "a mount can't be removed: EBUSY");
    errno = 0;
    check(rename("/m", "/elsewhere") == -1 && errno == EBUSY, "nor renamed");
}

static void dot_dot_and_the_current_directory(void)
{
    char where[64];
    check(chdir("/m") == 0, "chdir into the mount");
    check(getcwd(where, sizeof where) != NULL && strcmp(where, "/m") == 0, "getcwd says /m");
    check(holds("g", "inside"), "a name from there");
    check(holds("../ns/real/g", "inside"), ".. goes up out of the mount, by name");
    check(access("../m/g", F_OK) == 0, "and back down into it");
    check(chdir("/") == 0, "chdir back to the root");
}

static void what_is_under_a_mount(void)
{
    mkdir("shadow", 0755);
    check(put("shadow/x", "under") == 0, "write shadow/x");
    check(taaj_mount("/shadow", "/ns/real", 0) == 0, "mount ns/real over shadow");
    errno = 0;
    check(access("/shadow/x", F_OK) == -1 && errno == ENOENT, "what was there is out of sight");
    check(holds("/shadow/g", "inside"), "what is mounted is seen");
    check(taaj_mount("/shadow/deeper", "/ns", 0) == 0, "a mount under a mount");
    check(access("/shadow/deeper/real/g", F_OK) == 0, "the deepest mount wins");
    check(taaj_unmount("/shadow/deeper") == 0, "unmount the deeper one");
    check(taaj_unmount("/shadow") == 0 && holds("/shadow/x", "under"), "unmounted: what was there is back");
    errno = 0;
    check(taaj_unmount("/shadow") == -1 && errno == EINVAL, "nothing mounted there now: EINVAL");
}

static void read_only_and_refused(void)
{
    check(taaj_mount("/ro", "ns/real", TAAJ_MOUNT_READ_ONLY) == 0, "mount ns/real at /ro to read only");
    check(holds("/ro/g", "inside"), "it reads");
    errno = 0;
    check(put("/ro/g", "changed") == -1 && errno == EROFS, "it does not change: EROFS");
    errno = 0;
    check(unlink("/ro/g") == -1 && errno == EROFS, "nor lose a file");
    check(holds("ns/real/g", "inside"), "the directory is as it was");
    errno = 0;
    check(taaj_mount("/x", "ns/real/g", 0) == -1 && errno == ENOTDIR, "a file can't be mounted: ENOTDIR");
    errno = 0;
    check(taaj_mount("/x", "nothing", 0) == -1 && errno == ENOENT, "nor nothing: ENOENT");
    errno = 0;
    check(taaj_mount("/x", "ns", 99) == -1 && errno == EINVAL, "an unknown flag: EINVAL");
}

static void handed_to_a_child(void)
{
    check(child("read", "/m/g", "inside") == 0, "a child reads /m/g: the mount went with it");
    check(child("write", "/m/from-child", "child") == 0 && holds("ns/real/from-child", "child"), "a child writes under /m");
    check(child("write", "/ro/from-child", "no") == 1, "a child can't write under the read-only mount either");
    check(taaj_unmount("/m") == 0, "unmount /m");
    check(child("read", "/m/g", "inside") == 1, "a child started after it does not have it");
}

int main(int argc, char **argv)
{
    if (argc == 4 && strcmp(argv[1], "read") == 0)
        return holds(argv[2], argv[3]) ? 0 : 1;
    if (argc == 4 && strcmp(argv[1], "write") == 0)
        return put(argv[2], argv[3]) == 0 ? 0 : 1;
    a_directory_seen_elsewhere();
    dot_dot_and_the_current_directory();
    what_is_under_a_mount();
    read_only_and_refused();
    handed_to_a_child();
    taaj_unmount("/ro");
    unlink("ns/real/g");
    unlink("ns/real/new");
    unlink("ns/real/from-child");
    rmdir("ns/real");
    rmdir("ns");
    unlink("shadow/x");
    rmdir("shadow");
    if (failures == 0)
        printf("namespace: %d checks, all passed\n", checks);
    else
        printf("namespace: %d of %d checks failed\n", failures, checks);
    return failures != 0;
}
