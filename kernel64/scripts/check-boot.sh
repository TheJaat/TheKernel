#!/bin/sh
# check-boot.sh - checks a 64-bit kernel boot log for everything a good
# boot prints, and nothing a bad one does.
#
#   usage: check-boot.sh LOG [BIOS|UEFI]
#
# With a boot method, also checks the kernel saw that method - proof the
# boot info really came from that loader. With $TYPED set, checks the
# kernel echoed that line from the keyboard. $SMP is how many CPUs the
# machine was given (default 4, as in qemu-boot.sh): all must wake.

LOG=$1
METHOD=${2:-}
TYPED=${TYPED:-}

fail() {
    echo "FAIL: $1"
    cat "$LOG"
    exit 1
}

grep -q "Hello from TheTaaJ" "$LOG"                || fail "no greeting"
grep -q "resumed after the breakpoint" "$LOG"      || fail "exception path broken"
grep -q "\[selftest\] memory: all passed" "$LOG"   || fail "memory self-test"
grep -q "\[selftest\] timer" "$LOG"               || fail "timer self-test"
CPUS=${SMP:-4}
if [ "$CPUS" -eq 1 ]; then
    grep -q "\[smp\] 1 CPU: no others to start" "$LOG" || fail "one CPU: the kernel did not say there were no others"
else
    # Every CPU says so itself, with the id the hardware gave it; then
    # all of them count at once, and with a lock nothing may be lost.
    n=1
    while [ "$n" -lt "$CPUS" ]; do
        grep -q "\[smp\] CPU $n awake: APIC id" "$LOG" || fail "CPU $n did not wake"
        n=$((n + 1))
    done
    grep -q "\[smp\] $CPUS CPUs awake" "$LOG"        || fail "not all $CPUS CPUs woke"
    total=$((CPUS * 100000))
    grep -q "\[smp\] counting together: $CPUS CPUs reached $total of $total with a lock" "$LOG" \
        || fail "the CPUs did not count to $total together under a lock"
fi
! grep -qE "\[smp\].*(FAILED|did not answer|not started|staying with 1 CPU)" "$LOG" || fail "a CPU could not be started"
grep -q "\[selftest\] threads: sleep" "$LOG"      || fail "threads self-test"
grep -q "\[selftest\] user:" "$LOG"               || fail "user-mode self-test"
# The program that reads through GS must fault at its own offset from
# address 0 - twice, the self-test runs everything twice - and not in
# the kernel's memory: every swapgs is in its place.
[ "$(grep -c "user read at 0x5a5: page not present" "$LOG")" -eq 2 ] || fail "a program's GS is not its own"
grep -q "\[selftest\] ipc:" "$LOG"                || fail "IPC self-test"
grep -q "\[selftest\] files:" "$LOG"              || fail "file service self-test"
grep -q "\[selftest\] kill:" "$LOG"               || fail "kill self-test"
grep -q "\[kernel\] idle" "$LOG"                  || fail "did not reach idle"
grep -q "\[init\] started:.* shell kbd" "$LOG" || fail "init did not start the services"
if grep -q "framebuffer: .*handed to init" "$LOG"; then
    grep -q "\[display\] painted" "$LOG"            || fail "the display service did not paint"
    [ -f "$LOG.ppm" ]                                || fail "no screenshot"
    python3 -I "$(dirname "$0")/check-screen.py" "$LOG.ppm" ${SCREEN_MAX_INK:-} || fail "the screen is not what the display painted"
fi
! grep -qE "KERNEL PANIC|CPU EXCEPTION" "$LOG"     || fail "panic or exception"
if [ -n "$METHOD" ]; then
    grep -qE "loader +TheTaaJBoot.*\($METHOD\)" "$LOG" || fail "not booted via $METHOD"
fi
if [ -n "$TYPED" ]; then
    grep -q "\[kbd\] PS/2 keyboard ready" "$LOG"     || fail "keyboard driver did not start"
    grep -qF "[shell] line: \"$TYPED\"" "$LOG"     || fail "keyboard: \"$TYPED\" not echoed by the shell"
    grep -q "\[init\] kbd ended .* restarting" "$LOG" || fail "init did not restart the crashed driver"
    # The watchdog must end what was stalled on purpose, and only that:
    # fs once (its own stall) - not again while blk, which it waits on,
    # was being replaced - and nothing by mistake.
    [ "$(grep -c "\[init\] fs has not answered" "$LOG")" -le 1 ] || fail "the watchdog stopped fs more than once"
    [ "$(grep -c "\[init\] .* has not answered" "$LOG")" -le 4 ] || fail "the watchdog stopped a service that was not stalled"
fi
if [ -n "${DISK:-}" ]; then
    grep -q "\[pci\] .* virtio block device: .* handed to init" "$LOG" || fail "the kernel did not offer the disk"
    grep -q "\[blk\] virtio disk" "$LOG"           || fail "the disk driver did not start"
    if [ -n "${TFSTOOL:-}" ]; then
        # Proof that files reached the disk itself, in a form another
        # implementation of TaajFS reads: the host tool, on the image,
        # after QEMU is gone.
        [ "$("$TFSTOOL" get "$DISK" saved 2>&1)" = "kept" ] \
            || fail "tfstool does not find \"kept\" in the file \"saved\" on the disk"
        ! "$TFSTOOL" get "$DISK" notes > /dev/null 2>&1 || fail "the removed file \"notes\" is still on the disk"
        # And directories: a file the OS wrote two directories down, in
        # a directory it made inside one the host made; and the empty
        # directory it made and removed again.
        [ "$("$TFSTOOL" get "$DISK" docs/deep/note 2>&1)" = "nested" ] \
            || fail "tfstool does not find \"nested\" in docs/deep/note on the disk"
        ! "$TFSTOOL" ls "$DISK" work > /dev/null 2>&1 || fail "the removed directory \"work\" is still on the disk"
        # And the current directory: a file written by a bare name while
        # in docs, added to from docs/deep by its whole path; a file
        # written by a bare name after climbing back to the root (and
        # one written after a bare `cd`: "home", which the moves below
        # turn into "cottage"); and the directory removed while the
        # shell stood in it.
        [ "$("$TFSTOOL" get "$DISK" docs/here 2>&1)" = "inside
more" ] || fail "tfstool does not find \"inside\" and \"more\" in docs/here on the disk"
        [ "$("$TFSTOOL" get "$DISK" rooted 2>&1)" = "yes" ] || fail "\"rooted\" is not in the root: cd .. did not climb back"
        ! "$TFSTOOL" ls "$DISK" tools/gone > /dev/null 2>&1 || fail "the removed directory \"tools/gone\" is still on the disk"
        # And moving and copying: a file renamed, moved into docs, and
        # out again by a relative path; copies, one of them several
        # messages long, byte for byte what the host put on the disk; and
        # the directory that was moved away and back, with its file.
        [ "$("$TFSTOOL" get "$DISK" cottage 2>&1)" = "yes" ] || fail "\"cottage\" is not in the root with what \"home\" held: a bare cd, or the moves, went wrong"
        ! "$TFSTOOL" get "$DISK" home > /dev/null 2>&1       || fail "\"home\" is still there after it was renamed"
        ! "$TFSTOOL" get "$DISK" docs/house > /dev/null 2>&1 || fail "\"docs/house\" is still there after it was moved out"
        [ "$("$TFSTOOL" get "$DISK" docs/welcome 2>&1)" = "Hello from the host" ] || fail "the copy docs/welcome is not the file"
        "$TFSTOOL" get "$DISK" docs/long2 2>/dev/null | cmp -s - "$DISK.long" || fail "the copy docs/long2 differs from the file it was made from"
        ! "$TFSTOOL" ls "$DISK" tools/deep > /dev/null 2>&1  || fail "the directory moved back is still in tools"
        # And quoting: the two spaces that double quotes kept, the one
        # that is left without them, and names with spaces in them.
        [ "$("$TFSTOOL" get "$DISK" quoted 2>&1)" = "two  words" ] || fail "the quoted text did not keep its two spaces"
        [ "$("$TFSTOOL" get "$DISK" plain 2>&1)" = "two words" ]   || fail "the unquoted text is not two words and one space"
        [ "$("$TFSTOOL" get "$DISK" "my docs/a file" 2>&1)" = "x" ] || fail "no file \"a file\" in the directory \"my docs\""
        # And redirection: what > and >> put in a file; two files joined;
        # the shell's own output; a text, and a file longer than a
        # channel holds, sent through a program's input and output
        # both, which must come out the same byte for byte; a program's complaint, which went where its
        # output went; and the file a command that did not exist left.
        get() { "$TFSTOOL" get "$DISK" "$1" 2>/dev/null; }
        [ "$(get out)" = "first
second" ] || fail "\"out\" does not hold what > and >> put there"
        [ "$(get both)" = "Hello from the host
first
second" ] || fail "\"both\" is not the two files, one after the other"
        get procs | grep -q "init  *running" || fail "the output of ps did not go into \"procs\""
        get long3 | cmp -s - "$DISK.long" || fail "\"long3\" is not \"long\": cat < long > long3 changed it"
        { get ls; get ls; } > "$LOG.twice"
        get big | cmp -s - "$LOG.twice"  || fail "\"big\" is not the program ls twice over: cat ls ls > big lost or changed bytes"
        get big2 | cmp -s - "$LOG.twice" || fail "\"big2\" is not \"big\": 50 messages did not come through an input and an output whole"
        # A complaint is not output: it went to standard error, the
        # screen, and the file the output went to holds nothing.
        [ "$("$TFSTOOL" ls "$DISK" 2>&1 | grep -c '^ *err  *0 *$')" -eq 1 ]   || fail "\"err\" is not empty: cat's complaint went into the file"
        [ "$("$TFSTOOL" ls "$DISK" 2>&1 | grep -c '^ *lsout  *0 *$')" -eq 1 ] || fail "\"lsout\" is not empty: ls's complaint went into the file"
        [ "$("$TFSTOOL" ls "$DISK" 2>&1 | grep -c '^ *empty  *0 *$')" -eq 1 ] || fail "\"empty\" is not an empty file"
        # And pipes: a count that went from one program through another
        # into a file, twice - the second time with the first program
        # reading a file as its input, and >> - and 50 KB through three
        # programs.
        [ "$(get piped)" = "300 lines, 1200 words, 4692 bytes
300 lines, 1200 words, 4692 bytes" ] || fail "\"piped\" does not hold what wc counted of cat's output, twice"
        get big3 | cmp -s - "$LOG.twice" || fail "\"big3\" is not \"big\": cat ls ls | cat | cat > big3 lost or changed bytes"
        # And background jobs: 50 KB written by a pipeline nobody
        # waited for, while other lines ran; the one line a program
        # printed into a file before it was killed; and a count made
        # from what the shell itself printed.
        get big4 | cmp -s - "$LOG.twice" || fail "\"big4\" is not \"big\": the background job's output was lost or changed"
        [ "$(get flog)" = "forever: sleep until stopped (Ctrl+C)" ] || fail "\"flog\" does not hold what the background job printed"
        [ "$(get bgcount)" = "1 line, 1 word, 3 bytes" ] || fail "\"bgcount\" does not hold the count of the shell's own output"
        # And the keyboard as a program's input: the line typed to
        # `cat > typed`, ended with Ctrl+D, is what the file holds.
        if grep -q 'line: "cat > typed"' "$LOG"; then
            [ "$(get typed)" = "kept line" ] || fail "\"typed\" does not hold the line typed to cat: the keyboard did not reach a program's standard input"
        fi
        # And the C program, if there was one: what it wrote with
        # write() went where the shell pointed its standard output.
        if grep -q 'line: "chello a b > cout"' "$LOG"; then
            [ "$(get cout)" = "Hello from C
argc 3
argv[1] a
argv[2] b" ] || fail "\"cout\" does not hold what the C program wrote"
            [ "$(get cerr)" = "Hello from C
argc 2
argv[1] err" ] || fail "\"cerr\" holds more, or less, than the C program's standard output"
            [ "$(get pout)" = "printf: 42 -7 ff 0X2A [   hi] [hi   ] [00042] x 100%
sizes: 18446744073709551615 -9223372036854775808
12345
printf returned 6
snprintf: wanted 12, kept <hello, >
puts: a line of its own
fputs and putchar" ] || fail "\"pout\" is not, byte for byte, what the C program printed with printf"
            [ "$(get cnote)" = "made by c" ] || fail "\"cnote\" does not hold what the C program wrote with open and write"
            get clong | cmp -s - "$DISK.long" || fail "\"clong\" is not \"long\": a C program's open and read lost or changed it"
            get clong2 | cmp -s - "$DISK.long" || fail "\"clong2\" is not \"long\": a C program's read of standard input lost or changed it"
            "$TFSTOOL" ls "$DISK" 2>&1 | grep -q '^ *cfile\.tmp ' && fail "the C program's scratch file was left behind: unlink did not remove it"
            "$TFSTOOL" ls "$DISK" 2>&1 | grep -q '^ *cno ' && fail "a C program made a file through a read-only channel"
            # And its streams: lines read with fgets and numbered with
            # printf, a copy made with fread and fwrite, and what was
            # still in standard output's buffer when the program ended -
            # sent if it ended by returning, lost if by _exit.
            awk '{ printf "%4d  %s\n", NR, $0 }' "$DISK.long" > "$LOG.numbered"
            get cnum | cmp -s - "$LOG.numbered" || fail "\"cnum\" is not \"long\" with its lines numbered: fgets or a buffered printf lost or changed it"
            get ccopy | cmp -s - "$DISK.long" || fail "\"ccopy\" is not \"long\": a C program's fread and fwrite lost or changed it"
            printf 'no new line, and still here' > "$LOG.want"
            get cend | cmp -s - "$LOG.want" || fail "\"cend\" does not hold what was in the buffer when main returned: exit did not flush it"
            printf 'kept\n' > "$LOG.want"
            get clost | cmp -s - "$LOG.want" || fail "\"clost\" is not the one flushed line: _exit must not send what is still in the buffer"
            # And a whole program: the same C file was compiled for
            # this machine with this machine's C library, and both were
            # given the same input - the program's own 60 KB of machine
            # code and a text file; and the text file alone, as standard
            # input. What the two printed must be the same bytes.
            if [ -n "${CWORDS_HOST:-}" ]; then
                "$CWORDS_HOST" -n 25 "$CWORDS_TAAJ" "$DISK.long" > "$LOG.want"
                get cw1 | cmp -s - "$LOG.want" || fail "\"cw1\" is not what the same C program prints on this machine from the same two files"
                "$CWORDS_HOST" -n 5 < "$DISK.long" > "$LOG.want"
                get cw2 | cmp -s - "$LOG.want" || fail "\"cw2\" is not what the same C program prints on this machine from the same standard input"
            fi
            "$TFSTOOL" ls "$DISK" 2>&1 | grep -q '^ *cstdio\.\(tmp\|new\) ' && fail "the C program's scratch files were left behind: remove or rename did not work"
        fi
        "$TFSTOOL" dump "$DISK" > "$LOG.dump" || fail "tfstool cannot read the volume the OS left"
    fi
fi
if [ -n "${EXPECT:-}" ] && echo "$EXPECT" | grep -q "registry"; then
    # The registry was made to crash once. The one init started in its
    # place must have been told everything again: both namespaces are
    # announced a second time.
    [ "$(grep -c "^\[registry\] shell may ask for:" "$LOG")" -ge 2 ] || fail "the new registry was not told the shell's namespace"
    [ "$(grep -c "^\[registry\] fs may ask for:" "$LOG")" -ge 2 ]    || fail "the new registry was not told fs's namespace"
fi
if [ -n "${EXPECT:-}" ]; then
    # The output of the commands typed: each pattern must appear.
    old_ifs=$IFS
    IFS=';'
    for pattern in $EXPECT; do
        grep -qE "$pattern" "$LOG" || fail "no output matching \"$pattern\""
    done
    IFS=$old_ifs
    # `ps` is typed before any program is run and again after the last:
    # the same processes, threads and channels both times, or running a
    # program left something behind - a handle the shell or init forgot.
    [ "$(grep -cE "^[0-9]+ processes, [0-9]+ threads, [0-9]+ channels" "$LOG")" -ge 2 ] \
        || fail "ps did not run twice"
    [ "$(grep -E "^[0-9]+ processes, [0-9]+ threads, [0-9]+ channels" "$LOG" | sort -u | wc -l)" -eq 1 ] \
        || fail "running programs left processes, threads or channels behind"
fi
sed -n '/Hello from TheTaaJ/,$p' "$LOG"
