#!/bin/sh
# gui-test.sh - the window test: a program's window opened, clicked and
# typed into; the focus given back to the terminal; the terminal
# dragged by its title bar; the window closed by its close box.
#
#   usage: gui-test.sh ARCH IMAGE DISK LOG
#
# ARCH, IMAGE and DISK as for screen-test.sh: the PC with its PS/2
# mouse moved through QEMU's monitor, or AArch64 with ramfb, a virtio
# keyboard and a virtio tablet driven through QMP. The disk has
# `counter`, a window with a count and two buttons.
#
# Where things are, on the 1024x768 screen (the window server logs each
# of them, and the test checks the log says so):
#
#   the terminal's frame       at (19, 80); its title bar from y 81 to 102
#   counter's frame            at (200, 150), content 240x110 at (201, 173)
#     its "+1" button          content (16, 56) to (112, 92): (265, 247) is on it
#     its close box            (423, 155), 14x14: (430, 162) is in it
#
# The steps:
#
#   counter &                  window 2 opens on top, with the focus
#   click (265, 247)           "+1": clicks 1
#   type + and a               to counter, not the shell: clicks 2
#   hold b for 1.2 s           repeated by the display (K116): b, then
#                              about 17 more after half a second; one up
#   click (600, 500)           on the terminal: the focus goes back to it
#   echo back                  the shell gets exactly that - no "+a"
#   display layout dvorak      then the keys d i j s spc h . s o a v: the
#                              shell gets "echo dvorak" (K116); and back,
#                              typed on Dvorak's keys: "display layout us"
#   drag (100, 90) to (600, 340)   the terminal by its title bar, to (519, 330)
#   click (430, 162)           counter's close box: it is asked, and closes
#   echo done                  the focus is the terminal's again

set -u
ARCH=$1
IMAGE=$2
DISK=$3
LOG=$4
HERE=$(dirname "$0")

fail() {
    echo "FAIL: $1 (log: $LOG)"
    exit 1
}

said() {
    grep -aEq "$1" "$LOG" || fail "$2"
}

# keys WORDS: letters and spaces as key names.
keys() {
    echo "$1" | sed -e 's/ /_/g' -e 's/./& /g' -e 's/_/spc/g'
}

# The pointer's moves, as tokens for this machine's key list: `go X Y`
# (which leaves its tokens in MOVES), `press`, `release`, `click`.
if [ "$ARCH" = aarch64 ]; then
    go() { MOVES="@at,$1,$2"; }
    press() { echo "@press,left"; }
    release() { echo "@release,left"; }
else
    # The PS/2 mouse moves by steps, 120 at most (QEMU keeps the rest of
    # a bigger one for later): from where the pointer is - the middle,
    # at first - to where it should go. It keeps that place in X and Y,
    # so it runs in this shell, not in a $( ) of its own.
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
    press() { echo "!mouse_button,1"; }
    release() { echo "!mouse_button,0"; }
fi
click() { echo "$(press) $(release)"; }

steps="$(keys 'counter') spc shift-7 ret see:[counter],window"
[ "$ARCH" = aarch64 ] && steps="$steps @use,Tablet"
go 265 247; steps="$steps $MOVES $(click) see:clicks:,1 shift-equal a see:key,a"
# Shift alone and Control alone (K42): no byte, but the window is told.
steps="$steps shift see:key,code,42,up ctrl see:key,code,29,up"
# A held key (K116): QEMU holds b down for 1.2 s and repeats nothing
# itself; the display repeats it, the same on every keyboard.
steps="$steps !sendkey,b,1200 see:key,code,48,up"
# The wheel turned up once over counter's content (K46): on the PC
# QEMU's monitor (`mouse_move 0 0 1`: a positive dz is wheel up, which
# QEMU's PS/2 mouse sends as -1 and the decoder reads as +1, Linux's
# sign), on AArch64 the tablet's (`wheel-up`, a button QEMU turns into
# REL_WHEEL).
if [ "$ARCH" = aarch64 ]; then
    steps="$steps @btn,wheel-up see:wheel"
else
    steps="$steps !mouse_move,0,0,1 see:wheel"
fi
go 600 500; steps="$steps $MOVES $(click) see:lost,the,focus"
steps="$steps $(keys 'echo back') ret ~"
# The layout (K116): Dvorak, typed on its keys, and back to US.
# Their keys through key-names.py: `keys` above spells only letters and
# spaces, and "." and ";" are QEMU's "dot" and "semicolon".
steps="$steps $(keys 'display layout dvorak') ret ~ $(python3 -I "$HERE/key-names.py" 'dijs h.soav') ret ~"
steps="$steps $(python3 -I "$HERE/key-names.py" 'hg;rpat patsfk f;') ret ~"
go 100 90; steps="$steps $MOVES $(press)"
go 600 340; steps="$steps $MOVES $(release) see:moved,to"
go 430 162; steps="$steps $MOVES $(click) see:window,2,closed"
steps="$steps $(keys 'echo done') ret ~"

echo "Booting $ARCH to open, click, type into, drag and close windows (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    SCREEN=1 KEYS="$steps" QEMU_ARGS="-drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0 \
-device ramfb -device virtio-keyboard-device -device virtio-tablet-device" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 90
else
    DISK="$DISK" SMP=1 KEYS="$steps" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 90
fi

said '\[display\] window 1 "terminal" opened, [0-9]+x[0-9]+ at \(19, 80\)' "the terminal is not where the test expects it"
said '\[display\] window 2 "counter" opened, 240x110 at \(200, 150\)' "counter's window did not open where expected"
said '\[counter\] window 2 open, 240x110' "counter did not say its window is open"
said '\[display\] focus: window 2 "counter"' "the new window did not get the focus"
said '\[display\] left button at \(265, 247\)' "the click on +1 was not at (265, 247)"
said '\[counter\] clicks: 1' "the click on +1 did not reach counter"
said '\[counter\] key \+' "the + key did not reach counter"
said '\[counter\] clicks: 2' "+ did not count"
said '\[counter\] key a' "the a key did not reach counter"
said '\[counter\] key code 30 down \(modifiers 0\)' "counter was not told the A key went down (K42)"
said '\[counter\] key code 30 up \(modifiers 0\)' "counter was not told the A key came up (K42)"
said '\[counter\] key code 42 down \(modifiers 1\)' "counter was not told Shift alone went down (K42)"
said '\[counter\] key code 42 up \(modifiers 0\)' "counter was not told Shift came up (K42)"
said '\[counter\] key code 29 down \(modifiers 2\)' "counter was not told Control alone went down (K42)"
said '\[counter\] key code 29 up \(modifiers 0\)' "counter was not told Control came up (K42)"
# Repeat (K116): b held 1.2 s - about 17 repeats after the first half
# second; at least 5, for a loaded host, and no more than 40 (the
# keyboard's own repeats, if a PS/2 one sent them, are dropped, so a key
# does not repeat twice over). Each repeat tells the window the key went
# down again; one up.
B_KEYS=$(grep -ac '\[counter\] key b' "$LOG")
B_DOWNS=$(grep -ac '\[counter\] key code 48 down' "$LOG")
B_UPS=$(grep -ac '\[counter\] key code 48 up' "$LOG")
[ "$B_KEYS" -ge 6 ] && [ "$B_KEYS" -le 41 ] || fail "b held for 1.2 s came $B_KEYS times, not 6 to 41: the display did not repeat it (K116)"
[ "$B_DOWNS" -eq "$B_KEYS" ] || fail "b came $B_KEYS times but counter was told it went down $B_DOWNS times (K116)"
[ "$B_UPS" -eq 1 ] || fail "counter was told b came up $B_UPS times, not once (K116)"
# The Enter that started counter went down in the terminal: counter is
# not told it came up (K116; seen in entry 151's first log).
grep -aq '\[counter\] key code 28 up' "$LOG" && fail "counter was told Enter came up, but its down went to the terminal (K116)"
said '\[counter\] wheel 1 at \(64, 74\)' "counter was not told the wheel turned up once, under the pointer at (265, 247) (K46)"
said '\[display\] focus: window 1 "terminal"' "the click on the terminal did not give it the focus"
said '\[counter\] lost the focus' "counter was not told it lost the focus"
said '\[shell\] line: "echo back"' "the shell did not get exactly the line typed after the focus came back"
grep -aq '\[shell\] line: ".*[+a]echo back' "$LOG" && fail "keys typed into counter reached the shell"
said '\[display\] keyboard layout: dvorak' "the display did not take the Dvorak layout (K116)"
said '\[shell\] line: "echo dvorak"' "keys typed on Dvorak did not make \"echo dvorak\" (K116)"
said '\[display\] keyboard layout: us' "\"display layout us\", typed on Dvorak's keys, did not bring US back (K116)"
said '\[display\] window 1 moved to \(519, 330\)' "the terminal was not dragged to (519, 330)"
said '\[display\] window 2: its program is asked to close it' "the close box did not ask counter to close"
said '\[counter\] closed by its close box' "counter did not hear its close box"
said '\[display\] window 2 closed' "counter's window did not go"
said '\[process\] [0-9]+ "counter" exited with code 0' "counter did not end cleanly"
said '\[shell\] line: "echo done"' "the keys did not go back to the shell after counter closed"
grep -a 'has not answered for' "$LOG" && fail "a service stopped answering during the test"

[ -s "$LOG.ppm" ] || fail "no picture of the screen"
# The terminal's title bar, amber (it has the focus), where it was
# dragged; the desktop's gradient where counter was; the arrow on
# what was counter's close box, and nowhere else.
python3 -I "$HERE/check-pixels.py" "$LOG.ppm" \
    "900,335=E8A838:the terminal's title bar, dragged" \
    "320,228=102442:the desktop where counter was" \
    "100,90=101C37:the desktop where the terminal was" || fail "the windows are not drawn where they should be"
python3 -I "$HERE/check-pointer.py" "$LOG.ppm" 430 162 || fail "the arrow is not where the pointer is"
exit 0
