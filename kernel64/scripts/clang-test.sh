#!/bin/sh
# clang-test.sh - Clang and lld on TheTaaJ: clang and ld.lld,
# cross-compiled from ports/llvm by tools/build-llvm.py, build TheTaaJ's
# C programs there. clang writes byte for byte what the host's clang of
# the same release (18.1.3) writes from the same files with the same
# options, and ld.lld links them into the very programs TheTaaJ's own
# build makes on the host - and one of them runs.
#
#   usage: clang-test.sh ARCH IMAGE DISK LOG BIN_DIR C_BIN
#
#   ARCH     x86_64 or aarch64
#   IMAGE    the ISO (x86_64) or the kernel image (aarch64)
#   DISK     the disk to make and boot with
#   BIN_DIR  where the LLVM tools for ARCH were built (BIN_DIR/../lib
#            holds the headers clang brings with it)
#   C_BIN    where the build put this machine's C library (libc.a) and
#            its C programs, linked on the host: what TheTaaJ's must be
#
# LLVM_TBLGEN in the environment names the host's LLVM directory, as for
# llvm-test.sh: the host's clang is taken from beside it, and must be
# 18.1.3 too.
#
# The disk is laid out as TheTaaJ's clang expects to find it:
#
#   /clang                     the program
#   /lib/clang/18/include      its headers (stddef.h, stdarg.h, the
#                              intrinsics), found from /clang's own
#                              file: TheTaaJ tells a program which it is
#   /include                   the C library's headers
#   /libc/programs/*.c         TheTaaJ's eleven C programs, under the
#                              names the build compiles them by: assert
#                              writes the file's name into the program
#   /cflags, /oflags           the options for this machine and the
#                              other, in response files (clang @cflags):
#                              they would not fit on a line the shell
#                              takes, 256 bytes
#   /sources                   the eleven names, another response file
#   /bad.c                     a mistake
#   /ld.lld                    lld, named for the linker it is to be (as
#                              on the host, it decides by its name)
#   /lib/libc.a, /user.ld      the C library and the linker script
#   /ldflags                   the options TheTaaJ's build links its C
#                              programs with (the Makefile's TAAJ_LDFLAGS
#                              or TAAJ_A64_LDFLAGS)
#   /lflags                    the same, as clang takes them when it
#                              links (clang @cflags @lflags x.c -o x)
#   /bin                       where the programs go
#   /tmp                       where clang puts the object it compiles
#                              on the way to a program
#
# The steps:
#
#   clang --version                     "TheTaaJ clang version 18.1.3"
#   clang -print-resource-dir           "/lib/clang/18"
#   clang @cflags -c @sources                     all eleven programs, with the
#                                       options TheTaaJ's own build gives
#                                       them (the Makefile's TAAJ_CFLAGS
#                                       for this machine), in one run of
#                                       clang: eleven objects
#   clang @oflags -c libc/programs/cwords.c       and for the other machine
#   clang @cflags -S libc/programs/cwords.c       assembly language
#   clang @cflags -E -P libc/programs/cstdio.c    the C after the preprocessor:
#                                       every header, as it was read
#   clang -c bad.c                      a mistake: where, what, exit 1
#   clang -c nothere.c                  no such file: "clang: error: ...",
#                                       signed with the name it was
#                                       started by (argv[0])
#   ld.lld --version                    "TheTaaJ LLD 18.1.3"
#   ld.lld @ldflags NAME.o lib/libc.a -o bin/NAME
#                                       each of the eleven, linked
#   bin/chello one two                  one of them, run
#   ld.lld cwords.o -o bin/x            without the C library: undefined
#                                       symbols, "ld.lld: error: ...", exit 1
#                                       (not with @ldflags: --gc-sections
#                                       with no _start leaves nothing to
#                                       miss, and only warns)
#   clang @cflags @lflags libc/programs/chello.c lib/libc.a -o bin/chello2
#                                       compiled and linked in one: clang
#                                       starts ld.lld (posix_spawn), as it
#                                       does on the host, and the program
#                                       is the build's chello
#   bin/chello2 one two                 run
#   clang @cflags -nostdlib libc/programs/cwords.c -o bin/y
#                                       without the C library: ld.lld's
#                                       errors, then clang's "ld.lld
#                                       command failed", exit 1
#   with files clang ... -o bin/z       given the files but not the right
#                                       to start programs: clang cannot
#                                       start ld.lld (EPERM)
#   echo done
#
# The programs are compared with the build's own: its linker script
# leaves .comment out, so a program says nothing of who linked it.
#
# -fno-ident: an object says in .comment which compiler wrote it -
# "TheTaaJ clang version 18.1.3" or "Ubuntu clang version 18.1.3
# (1ubuntu1)" - and -fno-ident leaves that out, so that what is compared
# is what was compiled.

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
BIN_DIR=$5
C_BIN=$6
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../../.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}
TBLGEN=${LLVM_TBLGEN:-llvm-tblgen}
HOST_BIN=$(dirname "$(command -v "$TBLGEN")")
RESOURCE=$BIN_DIR/../lib/clang/18/include
WORK=$LOG.work
PROGRAMS="chello cmem cprint cfile cstdio cwords cask cprimes cnet cfloat clib"

fail() {
    echo "FAIL: $1 (log: $LOG)"
    exit 1
}

said() {
    grep -aEq "$1" "$LOG" || fail "$2"
}

keys() {
    python3 -I "$HERE/key-names.py" "$1" || fail "no keys for: $1"
}

# The options TheTaaJ's build compiles its C programs with (the
# Makefile's TAAJ_CFLAGS and TAAJ_A64_CFLAGS), the include directory
# where it is on TheTaaJ's disk, or on the host's.
flags() { # flags MACHINE INCLUDE
    case $1 in
        x86_64) echo "--target=x86_64-unknown-none-elf -std=c11 -O2 -Wall -Wextra -Werror -ffreestanding" \
                     "-fno-stack-protector -fno-pic -mno-red-zone -nostdlibinc -isystem $2 -fno-ident" ;;
        aarch64) echo "--target=aarch64-unknown-none-elf -std=c11 -O2 -Wall -Wextra -Werror -ffreestanding" \
                      "-fno-stack-protector -fno-pic -nostdlibinc -isystem $2 -fno-ident" ;;
    esac
}
OTHER=aarch64
[ "$ARCH" = aarch64 ] && OTHER=x86_64

rm -rf "$WORK" "$DISK"
mkdir -p "$WORK/libc/programs"
"$HOST_BIN/clang" --version 2>/dev/null | grep -q 'version 18\.1\.3' \
    || fail "the host's clang (in $HOST_BIN) is not 18.1.3, so its output is not what TheTaaJ's must match"
[ -d "$RESOURCE" ] || fail "no headers for clang in $RESOURCE (tools/build-llvm.py puts them there)"

# 49152 blocks, 192 MiB: clang alone is 82 MB (the PC's) or 72 MB (AArch64's).
"$TFSTOOL" mkfs "$DISK" 49152 > /dev/null || fail "cannot make the disk"
"$TFSTOOL" put "$DISK" clang "$BIN_DIR/clang" > /dev/null || fail "cannot put clang on the disk"
"$TFSTOOL" put "$DISK" ld.lld "$BIN_DIR/lld" > /dev/null || fail "cannot put ld.lld on the disk"
put_tree() { # put_tree FROM TO: every file under FROM, at TO on the disk
    for directory in $(cd "$1" && find . -type d | sort); do
        directory=${directory#./}
        [ "$directory" = . ] || "$TFSTOOL" mkdir "$DISK" "$2/$directory" > /dev/null 2>&1
    done
    for file in $(cd "$1" && find . -type f | sort); do
        file=${file#./}
        # TaajFS names are at most 27 bytes; the few of clang's headers
        # with longer names (CUDA's, HIP's, Hexagon's, and one of the
        # PC's AVX-512 headers) are not on the disk. Nothing here
        # includes them.
        if [ "$(basename "$file" | wc -c)" -gt 28 ]; then
            echo "$file" >> "$WORK/left-out"
            continue
        fi
        "$TFSTOOL" put "$DISK" "$2/$file" "$1/$file" > /dev/null || fail "cannot put $2/$file on the disk"
    done
}
for directory in lib lib/clang lib/clang/18 lib/clang/18/include include libc libc/programs bin tmp; do
    "$TFSTOOL" mkdir "$DISK" $directory > /dev/null || fail "cannot make $directory on the disk"
done
put_tree "$RESOURCE" lib/clang/18/include
put_tree "$ROOT/libc/include" include
sources=""
for program in $PROGRAMS; do
    cp "$ROOT/libc/programs/$program.c" "$WORK/libc/programs/"
    "$TFSTOOL" put "$DISK" libc/programs/$program.c "$WORK/libc/programs/$program.c" > /dev/null \
        || fail "cannot put $program.c on the disk"
    sources="$sources libc/programs/$program.c"
done
flags "$ARCH" /include > "$WORK/cflags"
flags "$OTHER" /include > "$WORK/oflags"
"$TFSTOOL" put "$DISK" cflags "$WORK/cflags" > /dev/null || fail "cannot put cflags on the disk"
"$TFSTOOL" put "$DISK" oflags "$WORK/oflags" > /dev/null || fail "cannot put oflags on the disk"
echo "$sources" > "$WORK/sources"
"$TFSTOOL" put "$DISK" sources "$WORK/sources" > /dev/null || fail "cannot put sources on the disk"
# The build's linker options (TAAJ_LDFLAGS, and for AArch64 the 4 KiB
# pages of TAAJ_A64_LDFLAGS), and what they name.
echo "-static -nostdlib --gc-sections -s -T user.ld" > "$WORK/ldflags"
[ "$ARCH" = aarch64 ] && echo "-z max-page-size=4096" >> "$WORK/ldflags"
"$TFSTOOL" put "$DISK" ldflags "$WORK/ldflags" > /dev/null || fail "cannot put ldflags on the disk"
echo "-static -nostdlib -Wl,--gc-sections -s -T user.ld" > "$WORK/lflags"
[ "$ARCH" = aarch64 ] && echo "-Wl,-z,max-page-size=4096" >> "$WORK/lflags"
"$TFSTOOL" put "$DISK" lflags "$WORK/lflags" > /dev/null || fail "cannot put lflags on the disk"
"$TFSTOOL" put "$DISK" user.ld "$ROOT/user/user.ld" > /dev/null || fail "cannot put user.ld on the disk"
"$TFSTOOL" put "$DISK" lib/libc.a "$C_BIN/libc.a" > /dev/null || fail "cannot put libc.a on the disk"
printf 'int f(void) {\n    return x;\n}\n' > "$WORK/bad.c"
"$TFSTOOL" put "$DISK" bad.c "$WORK/bad.c" > /dev/null || fail "cannot put bad.c on the disk"

# What the host's clang writes, from the same names in $WORK: an object
# holds the name of the file it was compiled from.
(
    cd "$WORK" || exit 1
    "$HOST_BIN/clang" $(flags "$ARCH" "$ROOT/libc/include") -c $sources || exit 1
    "$HOST_BIN/clang" $(flags "$OTHER" "$ROOT/libc/include") -c libc/programs/cwords.c -o other.o || exit 1
    "$HOST_BIN/clang" $(flags "$ARCH" "$ROOT/libc/include") -S libc/programs/cwords.c -o cwords.s || exit 1
    "$HOST_BIN/clang" $(flags "$ARCH" "$ROOT/libc/include") -E -P libc/programs/cstdio.c -o cstdio.i || exit 1
) || fail "the host's clang failed on TheTaaJ's C programs"

steps="$(keys 'with readonly clang --version') ret ~~"
steps="$steps $(keys 'with readonly clang -print-resource-dir') ret ~~"
steps="$steps $(keys "time with files clang @cflags -c @sources") ret ~~ ~~ ~~ ~~ ~~ ~~ ~~ ~~"
steps="$steps $(keys "time with files clang @oflags -c libc/programs/cwords.c -o other.o") ret ~~ ~~"
steps="$steps $(keys "with files clang @cflags -S libc/programs/cwords.c -o cwords.s") ret ~~ ~~"
steps="$steps $(keys "with files clang @cflags -E -P libc/programs/cstdio.c -o cstdio.i") ret ~~"
steps="$steps $(keys 'with files clang -c bad.c') ret ~~"
steps="$steps $(keys 'with files clang -c nothere.c') ret ~~"
steps="$steps $(keys 'with readonly ld.lld --version') ret ~~"
# Up to six minutes each: on AArch64 one link has taken 59 to 141 s
# alone (on the PC 7.5 s, nearly all of it loading ld.lld). Each wait
# ends as soon as the link does.
for program in $PROGRAMS; do
    steps="$steps $(keys "with files ld.lld @ldflags $program.o lib/libc.a -o bin/$program") ret ~~ ~~ ~~ ~~"
done
steps="$steps $(keys 'bin/chello one two') ret ~~"
steps="$steps $(keys 'with files ld.lld cwords.o -o bin/x') ret ~~ ~~ ~~ ~~"
steps="$steps $(keys 'with programs clang @cflags @lflags libc/programs/chello.c lib/libc.a -o bin/chello2') ret ~~ ~~ ~~ ~~ ~~"
steps="$steps $(keys 'bin/chello2 one two') ret ~~"
steps="$steps $(keys 'with programs clang @cflags -nostdlib libc/programs/cwords.c -o bin/y') ret ~~ ~~ ~~ ~~ ~~"
steps="$steps $(keys 'with files clang @cflags @lflags libc/programs/cwords.c lib/libc.a -o bin/z') ret ~~ ~~"
steps="$steps $(keys 'echo done') ret ~"

echo "Booting $ARCH to compile and link TheTaaJ's C programs with clang and lld there (log: $LOG)..."
# Three slots for two CPUs: the third is for QEMU's disk thread, which
# reading these big programs keeps busy (qemu-boot.sh, SLOTS).
if [ "$ARCH" = aarch64 ]; then
    KEYS="$steps" SLOTS=3 QEMU_ARGS="-smp 2 -drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 60
else
    DISK="$DISK" SMP=2 SLOTS=3 KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 60
fi

said '\[shell\] line: "with readonly clang --version"' "the keys did not reach the shell"
said '^TheTaaJ clang version 18\.1\.3' "clang did not say its version"
said "^Target: $ARCH-unknown-taaj" "clang did not say it is for TheTaaJ on $ARCH"
# Found from the file it was started from: /clang, so /lib/clang/18.
said '^/lib/clang/18[[:space:]]*$' "clang did not find its headers from its own file (-print-resource-dir)"
[ "$(grep -ac '"clang" exited with code 0' "$LOG")" -eq 7 ] \
    || fail "clang did not finish all seven of its jobs with exit code 0"
said "^bad\.c:2:12: error: use of undeclared identifier 'x'" "clang did not say what is wrong in bad.c, and where"
# argv[0] is the name clang was started by, as on the host: "clang:".
said "^clang: error: no such file or directory: 'nothere\.c'" "clang did not sign its complaint about nothere.c with its name"
[ "$(grep -ac '"clang" exited with code 1' "$LOG")" -eq 4 ] \
    || fail "clang did not fail on bad.c, on nothere.c, linking without the C library and without the right to start ld.lld"
said '\[shell\] line: "echo done"' "the shell did not get its keys back"
said '^TheTaaJ LLD 18\.1\.3 \(compatible with GNU linkers\)' "ld.lld did not say its version"
[ "$(grep -ac '"ld.lld" exited with code 0' "$LOG")" -eq 12 ] \
    || fail "ld.lld did not say its version and link all eleven programs with exit code 0"
# The program linked on TheTaaJ, run there.
said '^Hello from C' "chello, linked on TheTaaJ, did not run"
said '^argv\[2\] two' "chello did not get its arguments"
said '"bin/chello" exited with code 0' "chello, linked on TheTaaJ, did not end well"
said "^ld\.lld: error: undefined symbol: [a-z_]+" "ld.lld did not say which symbols are missing without the C library"
said '"ld.lld" exited with code 1' "ld.lld did not fail without the C library"
# clang starting ld.lld itself: the program it was asked for, run.
said '\[init\] started /ld\.lld for a program, from a file' "clang did not start ld.lld"
[ "$(grep -ac '"/ld.lld" exited with code 0' "$LOG")" -eq 1 ] || fail "the ld.lld clang started did not link chello2"
said '"bin/chello2" exited with code 0' "chello2, compiled and linked by clang on TheTaaJ, did not run"
said '"/ld.lld" exited with code 1' "the ld.lld clang started did not fail without the C library"
# Twice: the link without the C library, and the one clang was not let
# start (clang says "exit code 1" for that too). An exit code read wrong
# would make the first "failed due to signal".
[ "$(grep -ac '^clang: error: ld\.lld command failed with exit code 1' "$LOG")" -eq 2 ] \
    || fail "clang did not say its linker failed with exit code 1, the code ld.lld exited with"
said '^clang: error: unable to execute command: posix_spawn failed: not permitted' \
    "clang, not given the right to start programs, was not refused"
grep -aE '"/?(clang|ld\.lld)" killed' "$LOG" && fail "clang or ld.lld was killed"
grep -a 'has not answered for' "$LOG" && fail "a service stopped answering during the test"

same=""
for output in $(for program in $PROGRAMS; do echo $program.o; done) other.o cwords.s cstdio.i; do
    "$TFSTOOL" get "$DISK" "$output" > "$WORK/got-$output" 2> /dev/null || fail "clang on TheTaaJ wrote no $output"
    cmp -s "$WORK/$output" "$WORK/got-$output" \
        || fail "$output, written by clang on TheTaaJ, is not what the host's clang writes"
    same="$same $output"
done
"$TFSTOOL" get "$DISK" bin/chello2 > "$WORK/got-chello2" 2> /dev/null || fail "clang on TheTaaJ wrote no bin/chello2"
cmp -s "$C_BIN/chello" "$WORK/got-chello2" || fail "bin/chello2, made by clang and ld.lld on TheTaaJ, is not the build's chello"
"$TFSTOOL" get "$DISK" bin/z > /dev/null 2>&1 && fail "clang without the right to start programs still made bin/z"
linked=""
for program in $PROGRAMS; do
    "$TFSTOOL" get "$DISK" "bin/$program" > "$WORK/got-$program" 2> /dev/null || fail "ld.lld on TheTaaJ wrote no bin/$program"
    cmp -s "$C_BIN/$program" "$WORK/got-$program" \
        || fail "bin/$program, linked by ld.lld on TheTaaJ, is not the program the build links on the host ($C_BIN/$program)"
    linked="$linked $program"
done
times=$(grep -aEo '^took [0-9.]+ s' "$LOG" | sed 's/took //' | tr '\n' ' ')
echo "clang on TheTaaJ ($ARCH) wrote, byte for byte as the host's:$same"
echo "ld.lld on TheTaaJ ($ARCH) linked, byte for byte as the build's own programs:$linked"
echo "clang on TheTaaJ ($ARCH) compiled and linked chello in one, starting ld.lld itself: the build's chello, byte for byte"
echo "the timed steps took (s): $times"
[ -s "$WORK/left-out" ] && echo "clang's headers not on the disk (names over 27 bytes): $(tr '\n' ' ' < "$WORK/left-out")"
exit 0
