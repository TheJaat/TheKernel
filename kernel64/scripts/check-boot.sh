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
# ARCH=aarch64: the log is the AArch64 kernel's. What programs do is
# checked as on the PC - it is the same user space, and the point of
# checking it here is that it behaves the same. What differs is the
# machine underneath: its own boot checks are aarch64-test.sh's, the
# keys come from a serial port's driver, and the disk is not on a bus.
ARCH=${ARCH:-pc}
if [ "$ARCH" = aarch64 ]; then
    INPUT_DRIVER=serial
    INPUT_READY="\[serial\] serial port ready"
    # A CPU there is named by its affinity register, not an APIC id.
    CPU_AWAKE="named 0x"
    DISK_OFFERED="\[virtio\] .* block device: handed to init"
    # What is expected of the commands typed is the PC's list. A few
    # lines of it name the PC's drivers: there the serial port's one
    # driver is keyboard and screen both, and the disk's interrupt is
    # not a message. One line has no counterpart - a new display shows
    # again the text the old one had; a terminal still has it. And the
    # shell's note of a line it read does not begin a line of this log:
    # here the log is the screen, and the typed line is in front of it.
    # So may the next prompt be in front of what a program started in
    # the background says first: the shell does not wait for it, and
    # with several CPUs it need not have run yet - as on any terminal.
    EXPECT=$(printf '%s' "${EXPECT:-}" | sed \
        -e 's/\\\[display\\\] asked to stall/\\[serial\\] asked to stall/' \
        -e 's/\\\[init\\\] display /\\[init\\] serial /g' \
        -e 's/\\\[display\\\] the text on the screen was kept: [^;]*/\\[serial\\] serial port ready/' \
        -e 's/\^\\\[shell\\\] line:/\\[shell\\] line:/g' \
        -e 's/\^forever: yield until stopped/forever: yield until stopped/' \
        -e 's/\\\[kbd\\\] F11/\\[serial\\] F11/' \
        -e 's/\\\[init\\\] kbd /\\[init\\] serial /g' \
        -e 's/\\\[blk\\\] virtio disk\.\*MSI-X/\\[blk\\] virtio disk at 0xa003e00.*interrupts/')
else
    INPUT_DRIVER=kbd
    INPUT_READY="\[kbd\] PS/2 keyboard ready"
    CPU_AWAKE="APIC id"
    DISK_OFFERED="\[pci\] .* virtio block device: .* handed to init"
fi

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
        grep -q "\[smp\] CPU $n awake: $CPU_AWAKE" "$LOG" || fail "CPU $n did not wake"
        n=$((n + 1))
    done
    grep -q "\[smp\] $CPUS CPUs awake" "$LOG"        || fail "not all $CPUS CPUs woke"
    total=$((CPUS * 100000))
    grep -q "\[smp\] counting together: $CPUS CPUs reached $total of $total with a lock" "$LOG" \
        || fail "the CPUs did not count to $total together under a lock"
fi
! grep -qE "\[smp\].*(FAILED|did not answer|not started|staying with 1 CPU)" "$LOG" || fail "a CPU could not be started"
grep -q "\[selftest\] threads: sleep" "$LOG"      || fail "threads self-test"
if [ "$CPUS" -eq 1 ]; then
    grep -q "\[selftest\] smp: 1 CPU; 40000 hand-offs" "$LOG" || fail "SMP self-test (one CPU)"
    grep -q "\[selftest\] wake-ups: 1 CPU" "$LOG" || fail "wake-up self-test (one CPU)"
else
    # The started CPUs must join the scheduler, and threads must then
    # really be seen running on every one of them.
    grep -q "\[smp\] $CPUS CPUs run threads" "$LOG"  || fail "the started CPUs did not join the scheduler"
    grep -q "\[selftest\] smp: threads ran on all $CPUS CPUs at once; 40000 hand-offs between two threads, none lost; nothing leaked" "$LOG" \
        || fail "SMP self-test"
    # A thread woken by a busy CPU must start on an idle one because
    # that CPU was interrupted, not because its timer happened to tick.
    grep -q "\[selftest\] wake-ups: a thread woken by a busy CPU ran on an idle one" "$LOG" \
        || fail "an idle CPU was not told of a woken thread"
fi
grep -q "\[selftest\] user:" "$LOG"               || fail "user-mode self-test"
if [ "$ARCH" = pc ]; then
# The program that reads through GS must fault at its own offset from
# address 0 - twice, the self-test runs everything twice - and not in
# the kernel's memory: every swapgs is in its place.
[ "$(grep -c "user read at 0x5a5: page not present" "$LOG")" -eq 2 ] || fail "a program's GS is not its own"
fi
grep -q "\[selftest\] ipc:" "$LOG"                || fail "IPC self-test"
grep -q "\[selftest\] files:" "$LOG"              || fail "file service self-test"
grep -q "\[selftest\] kill:" "$LOG"               || fail "kill self-test"
grep -q "\[kernel\] idle" "$LOG"                  || fail "did not reach idle"
grep -q "\[init\] started:.* shell $INPUT_DRIVER" "$LOG" || fail "init did not start the services"
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
    grep -q "$INPUT_READY" "$LOG"                  || fail "keyboard driver did not start"
    grep -qF "[shell] line: \"$TYPED\"" "$LOG"     || fail "keyboard: \"$TYPED\" not echoed by the shell"
    grep -q "\[init\] $INPUT_DRIVER ended .* restarting" "$LOG" || fail "init did not restart the crashed driver"
    # The watchdog must end what was stalled on purpose, and only that:
    # fs once (its own stall) - not again while blk, which it waits on,
    # was being replaced - and nothing by mistake.
    [ "$(grep -c "\[init\] fs has not answered" "$LOG")" -le 1 ] || fail "the watchdog stopped fs more than once"
    [ "$(grep -c "\[init\] .* has not answered" "$LOG")" -le 4 ] || fail "the watchdog stopped a service that was not stalled"
    if [ "$ARCH" = aarch64 ]; then
        # Keys must arrive by interrupt. A driver whose line never came
        # back on would still read them - each time init's once-a-second
        # question woke it - and everything above would pass, slowly.
        grep -aq "\[serial\] [0-9]* interrupts taken" "$LOG" || fail "the serial driver was not interrupted for the keys"
        # The log is the screen here, so what the shell shows must be in
        # it once: a program that wrote to both the screen and the log
        # would show it twice - every key typed, echoed double.
        [ "$(grep -ac "Welcome to TheTaaJ" "$LOG")" = "$(grep -ac "\[shell\] ready" "$LOG")" ] \
            || fail "the shell's greeting is not on the terminal exactly once for each time it started"
        [ "$(grep -ac "^taaj:/> echo " "$LOG")" -ge 5 ] || fail "what was typed is not on the terminal as it was typed"
        python3 -I -c 'import re, sys
sys.exit(1 if re.search(rb"^taaj:/> .*([a-z])\1([a-z])\2([a-z])\3([a-z])\4", open(sys.argv[1], "rb").read(), re.M) else 0)' "$LOG" \
            || fail "keys typed are on the terminal twice over (a command with four doubled letters in a row)"
    fi
fi
if [ -n "${DISK:-}" ]; then
    grep -q "$DISK_OFFERED" "$LOG" || fail "the kernel did not offer the disk"
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
            # The milestone: a C program compiled on this machine, put
            # on the disk, and run four times at once. Each copy must
            # print what the same C file prints here; and while the
            # shell waited for them the CPUs must have done more work
            # than one CPU can do in that time - which is what "at
            # once" means. One and a half times is asked for, of a
            # possible four: on one CPU it cannot be more than once.
            if [ -n "${CPRIMES_HOST:-}" ] && grep -q 'line: "time wait"' "$LOG"; then
                want=$("$CPRIMES_HOST" "$CPRIMES_LIMIT")
                [ "$(grep -c "^$want\s*\$" "$LOG")" -eq 4 ] \
                    || fail "four copies of cprimes did not each print what it prints on this machine: $want"
                times=$(sed -n '/line: "time wait"/,$p' "$LOG" | grep -m1 "^took .* in all")
                took=$(echo "$times" | sed 's/^took \([0-9]*\)\.\([0-9][0-9]\) s.*/\1\2/; s/^0*//')
                worked=$(echo "$times" | sed 's/.*worked for \([0-9]*\)\.\([0-9][0-9]\) s.*/\1\2/; s/^0*//')
                [ "${took:-0}" -gt 100 ] || fail "the four copies of cprimes were over before the shell waited for them: $times"
                [ $((${worked:-0} * 2)) -ge $((took * 3)) ] \
                    || fail "four programs did not run at once: $times"
                echo "four copies of cprimes at once: $times"
            fi
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
