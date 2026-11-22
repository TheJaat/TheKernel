#!/bin/sh
# screen-test.sh - the screen and pointer test: a line typed, the
# pointer moved and clicked, and the screen read back.
#
#   usage: screen-test.sh ARCH IMAGE DISK LOG
#
# ARCH is x86_64 (IMAGE an ISO; the PS/2 keyboard and mouse, moved
# through QEMU's monitor) or aarch64 (IMAGE the kernel; a ramfb screen,
# a virtio keyboard, a virtio mouse and a virtio tablet, driven through
# QMP). DISK is the test disk, for the shell's programs.
#
# The pointer starts in the middle of the 1024x768 screen. The display
# service logs every button press with where the pointer was, so each
# click below checks one position; the screen saved at the end must
# show the arrow where the last click was, and only there.

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

# keys WORDS: a command line of letters and spaces, as key names.
keys() {
    echo "$1" | sed -e 's/ /_/g' -e 's/./& /g' -e 's/_/spc/g'
}

TYPED="$(keys 'echo pointer') ret ~"
echo "Booting $ARCH with a screen and a pointer (log: $LOG)..."
if [ "$ARCH" = aarch64 ]; then
    # The mouse moves by steps and the tablet jumps to places; both move
    # the one pointer. The tablet's 0..32767 is scaled to the screen:
    # 16384 x 8192 is (511, 191).
    # Buttons go to the device "@use" names (qmp-type.py says why).
    MOVES="@use,Mouse @btn,left @rel,100,50 @btn,right \
@use,Tablet @abs,16384,8192 @btn,middle @abs,0,0 @btn,right @abs,32767,32767 @btn,left \
@use,Mouse @rel,-240,-240 @btn,left"
    WANT="left (512, 384);right (612, 434);middle (511, 191);right (0, 0);left (1023, 767);left (783, 527)"
    SCREEN=1 KEYS="$TYPED $MOVES" QEMU_ARGS="-drive file=$DISK,if=none,format=raw,id=d0 -device virtio-blk-device,drive=d0 \
-device ramfb -device virtio-keyboard-device -device virtio-mouse-device -device virtio-tablet-device" \
        "$HERE/aarch64-boot.sh" "$IMAGE" "$LOG" 60
else
    # QEMU's PS/2 mouse sends at most 127 a packet and keeps the rest of
    # a bigger move for later, so the moves are 120 at most. Six of
    # them go past the corner: the pointer stops at the screen's edge.
    click() { echo "!mouse_button,$1 !mouse_button,0"; }
    far="!mouse_move,120,120"
    back="!mouse_move,-120,-120"
    MOVES="$(click 1) !mouse_move,100,50 $(click 2) $far $far $far $far $far $far $(click 4) $back $back $(click 1)"
    WANT="left (512, 384);right (612, 434);middle (1023, 767);left (783, 527)"
    DISK="$DISK" SMP=1 KEYS="$TYPED $MOVES" "$HERE/qemu-boot.sh" "$IMAGE" "$LOG" 60
fi

said '\[shell\] ran "echo pointer"' "the typed line did not run"
if [ "$ARCH" = aarch64 ]; then
    said '\[screen\] ramfb: 1024x768, 32 bits a pixel' "the kernel did not switch on the ramfb screen"
    said '\[hid\] virtio keyboard at .*"QEMU Virtio Keyboard"' "no virtio keyboard driver"
    said '\[hid\] virtio mouse at ' "no virtio mouse driver"
    said '\[hid\] virtio tablet at ' "no virtio tablet driver"
else
    said '\[irq\] PS/2 mouse on GSI 12' "the mouse's interrupt was not routed"
    said '\[kbd\] PS/2 keyboard ready .*; the mouse too' "the mouse was not started"
fi
got=$(grep -aoE '\[display\] [a-z]+ button at \([0-9]+, [0-9]+\)' "$LOG" | sed -e 's/\[display\] //' -e 's/ button at//' | paste -sd ';')
[ "$got" = "$WANT" ] || fail "the clicks were seen at: $got; expected: $WANT"
echo "clicks: $got"
[ -s "$LOG.ppm" ] || fail "no picture of the screen"
python3 -I "$HERE/check-screen.py" "$LOG.ppm" || fail "the screen is not what the display painted"
python3 -I "$HERE/check-pointer.py" "$LOG.ppm" 783 527 || fail "the arrow is not where the pointer is"
grep -a 'has not answered for' "$LOG" && fail "a service stopped answering during the test"
exit 0
