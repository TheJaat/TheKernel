#!/bin/sh
# edit-test.sh - the text editor: a new file typed and saved, a long
# file paged through and changed, a click and the close box - and
# afterwards, on the host, the files read back off the disk byte for
# byte.
#
#   usage: edit-test.sh ARCH IMAGE DISK LOG
#
# ARCH, IMAGE and DISK as for gui-test.sh. The disk has `edit`, and
# `long`: 300 lines, "line 1 of 300" to "line 300 of 300".
#
# The editor's window is 924x512 (76 x 24 characters of 12x20, a status
# line, 6 pixels round them); its frame is 926x535. On the 1024x768
# screen the cascade puts each new window 32 pixels further down, and
# pushes it back in from the right edge: window 2 at (98, 150), window
# 3 at (98, 182), window 4 at (98, 214). The test checks the log says
# so before it relies on it.
#
# The steps:
#
#   edit note                  a new file: window 2
#     hello tajj, Backspace twice, aj, Ctrl+J,
#     secnd line, Home, Right three times, o
#                              "hello taaj" and "second line"
#     Ctrl+S, Ctrl+Q           saved (22 bytes); quits at once
#   edit long                  window 3
#     Page Down twice          the view from line 47, the cursor on it
#     XYZ and a space, Ctrl+S  "XYZ line 47 of 300", saved
#     q, Ctrl+Q                warned: q is not saved
#     Ctrl+Q                   quits, q dropped
#   edit note                  window 4
#     click after "hello "     the cursor there
#     big and a space, Ctrl+S  "hello big taaj", saved (26 bytes)
#     z, the close box         warned
#     the close box again      closes, z dropped
#   echo done                  the shell has the keys again
#
# Ctrl+J and not Enter for a new line in the editor: they are the same
# byte (10) by the time the editor gets it, but the boot script counts
# every Enter it types as a command for the shell, and waits for the
# shell to have run that many.

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
HERE=$(dirname "$0")
TFSTOOL=${TFSTOOL:-tools/tfstool-rs/target/release/tfstool}

fail() {
    echo "FAIL: $1 (log: $LOG)"
    exit 1
}

said() {
    grep -aEq "$1" "$LOG" || fail "$2"
}

# keys WORDS: lower-case letters and spaces as key names.
keys() {
    echo "$1" | sed -e 's/ /_/g' -e 's/./& /g' -e 's/_/spc/g'
}

# The pointer, as in gui-test.sh.
if [ "$ARCH" = aarch64 ]; then
    go() { MOVES="@at,$1,$2"; }
    click() { echo "@press,left @release,left"; }
else
    X=512
    Y=384
    go() {
        MOVES=""
        dx=$(($1 - X))
        dy=$(($2 - Y))
        while [ $dx -ne 0 ] || [ $dy -ne 0 ]; do
            sx=$dx; [ $sx -gt 120 ] && sx=120; [ $sx -lt -120 ] && sx=-120
            sy=$dy; [ $sy -gt 120 ] && sy=120; [ $sy -lt -120 ] && sy=-120
            MOVES="$MOVES !mouse_move,$sx,$sy"
            dx=$((dx - sx))
            dy=$((dy - sy))
        done
        X=$1
        Y=$2
    }
    click() { echo "!mouse_button,1 !mouse_button,0"; }
fi

MID="$LOG.mid.ppm"
rm -f "$MID"

steps="$(keys 'edit note') ret see:window,2,open"
steps="$steps $(keys 'hello tajj') backspace backspace a j ctrl-j"
steps="$steps $(keys 'secnd line') home right right right o"
steps="$steps ctrl-s see:note:,22,bytes !screendump,$MID ctrl-q see:window,2,closed"

steps="$steps $(keys 'edit long') ret see:window,3,open"
steps="$steps pgdn pgdn see:view,from,line,47 shift-x shift-y shift-z spc ctrl-s see:saved,long"
steps="$steps q ctrl-q see:unsaved,changes ctrl-q see:window,3,closed"

steps="$steps $(keys 'edit note') ret see:window,4,open"
[ "$ARCH" = aarch64 ] && steps="$steps @use,Tablet"
# Window 4's text starts at (99 + 6, 214 + 23 + 6): the cell after
# "hello " - column 6 of row 0 - is 72 pixels in.
go 183 253; steps="$steps $MOVES $(click) $(keys 'big ') ctrl-s see:note:,26,bytes z"
# Its close box: 14x14 at (1005, 219).
go 1012 226; steps="$steps $MOVES $(click) see:asked,again $(click) see:window,4,closed"
steps="$steps $(keys 'echo done') ret ~"

echo "Booting $ARCH to type, page through and save files in the editor (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    SCREEN=1 KEYS="$steps" QEMU_ARGS="-drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0 \
-device ramfb -device virtio-keyboard-device -device virtio-tablet-device" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 120
else
    DISK="$DISK" SMP=1 KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 120
fi

said '\[display\] window 2 "edit note" opened, 924x512 at \(98, 150\)' "the first editor's window is not where the test expects it"
said '\[edit\] new file note' "edit did not start note as a new file"
said '\[edit\] window 2 open, 924x512: 76 x 24 characters' "edit did not say its window is open"
said '\[edit\] saved note: 22 bytes, 2 lines' "the new file was not saved as typed"
said '\[display\] window 2 closed' "Ctrl+Q on a saved file did not close the editor"
said '\[edit\] opened long: 301 lines, [0-9]+ bytes' "edit did not read long (300 lines and an empty one after the last newline)"
said '\[display\] window 3 "edit long" opened, 924x512 at \(98, 182\)' "the second editor's window is not where the test expects it"
said '\[edit\] view from line 24' "Page Down did not move the view a page"
said '\[edit\] view from line 47' "a second Page Down did not move the view another page"
said '\[edit\] saved long: [0-9]+ bytes, 301 lines' "the long file was not saved"
said '\[edit\] unsaved changes: ctrl\+q again to drop them' "Ctrl+Q with unsaved changes did not warn"
said '\[display\] window 3 closed' "the second Ctrl+Q did not close the editor"
said '\[display\] window 4 "edit note" opened, 924x512 at \(98, 214\)' "the third editor's window is not where the test expects it"
said '\[edit\] opened note: 2 lines, 22 bytes' "edit did not read back the file it saved"
said '\[display\] left button at \(183, 253\)' "the click in the text was not at (183, 253)"
said '\[edit\] saved note: 26 bytes, 2 lines' "the change made after the click was not saved"
said '\[edit\] the close box: unsaved changes, asked again' "the close box with unsaved changes did not warn"
said '\[edit\] closed by its close box' "the second press of the close box did not close the editor"
said '\[display\] window 4 closed' "the editor's window did not go"
[ "$(grep -ac '\[process\] [0-9]* "edit" exited with code 0' "$LOG")" -eq 3 ] || fail "edit did not end cleanly three times"
said '\[shell\] line: "echo done"' "the keys did not go back to the shell"
grep -a 'has not answered for' "$LOG" && fail "a service stopped answering during the test"

# The files, off the disk.
printf 'hello big taaj\nsecond line' > "$LOG.note"
"$TFSTOOL" get "$DISK" note > "$LOG.note.got" || fail "note cannot be read off the disk"
cmp -s "$LOG.note" "$LOG.note.got" || fail "note on the disk is not what was typed (see $LOG.note.got)"
seq 300 | sed -e 's/.*/line & of 300/' -e '47s/^/XYZ /' > "$LOG.long"
"$TFSTOOL" get "$DISK" long > "$LOG.long.got" || fail "long cannot be read off the disk"
cmp -s "$LOG.long" "$LOG.long.got" || fail "long on the disk is not the file with XYZ on line 47 (see $LOG.long.got)"

# The screen while note was being typed: window 2's text at (105,
# 179). The cursor, a block, after "seco" on row 1 - its bottom rows,
# where no letter reaches; the status line's bar; the empty text.
[ -s "$MID" ] || fail "no picture of the screen while editing"
python3 -I "$HERE/check-pixels.py" "$MID" \
    "156,218=F0D8A0:the cursor after seco" \
    "1000,670=405880:the status line" \
    "900,400=283044:the window's empty text" || fail "the editor is not drawn as it should be"
exit 0
