/* proc.c - /proc (journal entry NNN, J66): what the kernel says about the
 * machine and its processes, from a service of its own mounted at /proc;
 * and /proc/self, /proc/self/exe and /proc/self/fd/N from libc.
 * handles-test.sh boots TheTaaJ and runs
 *
 *   with programs proctest    the checks; "proc: N checks, all passed"
 *
 * which starts proctest again as a child, to be:
 *   proctest child PID        exits 0 if it sees itself and its parent
 *                             PID in /proc, and is not its parent
 *   proctest boxed            in a tree of its own: exits 0 if /proc is
 *                             not reachable from there
 *
 * The program is proctest, not proc: a file "proc" in the root would be
 * hidden under the mount. */

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
        printf("proc: failed: %s (errno %d)\n", what, errno);
    }
}

/* The file, as text: into `out`, ended by a zero; how many bytes, or -1. */
static int slurp(const char *path, char *out, int size)
{
    int fd = open(path, O_RDONLY);
    if (fd < 0)
        return -1;
    int total = 0, n;
    while (total < size - 1 && (n = (int)read(fd, out + total, size - 1 - total)) > 0)
        total += n;
    close(fd);
    out[total] = 0;
    return n < 0 ? -1 : total;
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

/* Whether `text` has a line starting with `start`. */
static int has_line(const char *text, const char *start)
{
    for (const char *line = text; line != NULL && *line; line = strchr(line, '\n') == NULL ? NULL : strchr(line, '\n') + 1)
        if (strncmp(line, start, strlen(start)) == 0)
            return 1;
    return 0;
}

/* A child: its exit code, or -1. With `root`, in a tree of its own. */
static int child(char **argv, const char *root)
{
    posix_spawn_file_actions_t actions;
    posix_spawn_file_actions_init(&actions);
    if (root != NULL)
        posix_spawn_file_actions_addroot_np(&actions, root);
    pid_t pid;
    int started = posix_spawnp(&pid, "proctest", &actions, NULL, argv, environ) == 0;
    posix_spawn_file_actions_destroy(&actions);
    int status;
    if (!started || waitpid(pid, &status, 0) != pid || !WIFEXITED(status))
        return -1;
    return WEXITSTATUS(status);
}

static void the_machine(void)
{
    struct stat status;
    char text[1024];
    check(stat("/proc", &status) == 0 && S_ISDIR(status.st_mode), "/proc is a directory");
    check(listed("/", "proc"), "the root's listing shows it");
    check(listed("/proc", "uptime") && listed("/proc", "meminfo") && listed("/proc", "cpuinfo") && listed("/proc", "stat"), "/proc lists uptime, meminfo, cpuinfo and stat");

    double up = -1, busy = -1;
    check(slurp("/proc/uptime", text, sizeof text) > 0 && sscanf(text, "%lf %lf", &up, &busy) == 2 && up > 0 && busy >= 0 && busy <= 4 * up, "/proc/uptime: seconds up, and busy");
    double up2 = -1;
    usleep(30000);
    check(slurp("/proc/uptime", text, sizeof text) > 0 && sscanf(text, "%lf", &up2) == 1 && up2 > up, "it grows: made fresh at each read");
    long total = 0, freed = 0;
    check(slurp("/proc/meminfo", text, sizeof text) > 0 && sscanf(text, "MemTotal: %ld kB\nMemFree: %ld kB", &total, &freed) == 2 && total > 0 && freed > 0 && freed < total, "/proc/meminfo: MemTotal and MemFree");
    check(slurp("/proc/cpuinfo", text, sizeof text) > 0 && has_line(text, "processor\t: 0"), "/proc/cpuinfo: processor 0");
    long processes = 0;
    check(slurp("/proc/stat", text, sizeof text) > 0 && sscanf(text, "Processes:\t%ld", &processes) == 1 && processes >= 2, "/proc/stat: at least init and this program");
    FILE *file = fopen("/proc/uptime", "r");
    check(file != NULL && fscanf(file, "%lf", &up) == 1 && up > 0, "and through stdio");
    if (file != NULL)
        fclose(file);
}

static void this_process(void)
{
    char text[256], path[64], want[64];
    pid_t me = getpid();
    snprintf(path, sizeof path, "/proc/%d", (int)me);
    check(listed("/proc", path + 6), "this process is listed in /proc");
    snprintf(path, sizeof path, "/proc/%d/status", (int)me);
    snprintf(want, sizeof want, "Pid:\t%d", (int)me);
    check(slurp(path, text, sizeof text) > 0 && has_line(text, "Name:\tproctest") && has_line(text, want) && has_line(text, "State:\trunning"), "/proc/PID/status: Name, Pid and State");
    check(slurp("/proc/self/status", text, sizeof text) > 0 && has_line(text, want), "/proc/self is /proc/PID");
    struct stat own, named;
    check(stat("/proc/self", &own) == 0 && S_ISDIR(own.st_mode), "/proc/self is a directory");
    check(listed("/proc/self", "status"), "and lists status");

    /* exe: the program's own file, the same file as the one it was
     * started from. */
    check(stat("/proc/self/exe", &own) == 0 && stat("/proctest", &named) == 0 && own.st_size == named.st_size && own.st_size > 1000, "/proc/self/exe is the program's file");
    unsigned char head[4] = {0};
    int fd = open("/proc/self/exe", O_RDONLY);
    check(fd >= 0 && read(fd, head, 4) == 4 && head[0] == 0x7f && head[1] == 'E' && head[2] == 'L' && head[3] == 'F', "and reads as an ELF file");
    close(fd);
    char link[64];
    ssize_t n = readlink("/proc/self/exe", link, sizeof link - 1);
    check(n > 0 && n < (ssize_t)sizeof link && strncmp(link, "/proctest", (size_t)n) == 0 && n == 9, "readlink(/proc/self/exe) is its path");
    errno = 0;
    check(readlink("/proctest", link, sizeof link) == -1 && errno == EINVAL, "readlink of a plain file: EINVAL");

    /* fd: the program's own descriptors. */
    int out = open("/proc/self/fd/1", O_WRONLY);
    const char *line = "proc: written to /proc/self/fd/1\n";
    check(out > 2 && write(out, line, strlen(line)) == (ssize_t)strlen(line), "/proc/self/fd/1 is standard output");
    close(out);
    errno = 0;
    check(open("/proc/self/fd/77", O_RDONLY) == -1 && errno == EBADF, "/proc/self/fd/77: no such descriptor, EBADF");

    errno = 0;
    check(open("/proc/99999/status", O_RDONLY) == -1 && errno == ENOENT, "a process that is not there: ENOENT");
    errno = 0;
    check(open("/proc/new", O_WRONLY | O_CREAT, 0644) == -1 && errno == EROFS, "nothing new in /proc: EROFS");
    errno = 0;
    check(open("/proc/uptime", O_WRONLY) == -1 && errno == EROFS, "nor written: EROFS");
    errno = 0;
    check(unlink("/proc/uptime") == -1 && errno == EROFS, "nor removed");
    errno = 0;
    check(open("/proc/uptime/x", O_RDONLY) == -1 && errno == ENOTDIR, "a file is no directory");
}

/* The child: itself and its parent in /proc, and not the same. */
static int as_child(const char *parent)
{
    char path[64], text[256], want[64];
    snprintf(path, sizeof path, "/proc/%s/status", parent);
    if (slurp(path, text, sizeof text) <= 0 || !has_line(text, "Name:\tproctest"))
        return 1;
    snprintf(want, sizeof want, "Pid:\t%d", (int)getpid());
    if (slurp("/proc/self/status", text, sizeof text) <= 0 || !has_line(text, want))
        return 2;
    if (strcmp(parent, want + 5) == 0)
        return 3;
    return 0;
}

int main(int argc, char **argv)
{
    if (argc == 3 && strcmp(argv[1], "child") == 0)
        return as_child(argv[2]);
    if (argc == 2 && strcmp(argv[1], "boxed") == 0)
        return access("/proc/uptime", F_OK) == 0 ? 1 : 0;

    the_machine();
    this_process();
    char pid[16];
    snprintf(pid, sizeof pid, "%d", (int)getpid());
    char *as[] = {"proctest", "child", pid, NULL};
    check(child(as, NULL) == 0, "a child sees itself and its parent in /proc");
    mkdir("procbox", 0755);
    char *boxed[] = {"proctest", "boxed", NULL};
    check(child(boxed, "procbox") == 0, "a child in a tree of its own has no /proc");
    rmdir("procbox");
    if (failures == 0)
        printf("proc: %d checks, all passed\n", checks);
    else
        printf("proc: %d of %d checks failed\n", failures, checks);
    return failures != 0;
}
