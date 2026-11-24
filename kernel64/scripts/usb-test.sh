#!/bin/sh
# usb-test.sh - USB: an xHCI controller with keyboards and a mouse on
# it, typed on and moved through QEMU's monitor.
#
#   usage: usb-test.sh ISO DISK LOG
#
# Four machines, booted at once:
#   pc      QEMU's PC (BIOS) with qemu-xhci (MSI-X only), a USB keyboard,
#           a USB mouse and a USB disk. A line is typed on the keyboard;
#           a key is held down for a second, and must repeat; the mouse
#           is moved and clicked, and the display must see the clicks
#           where the pointer is; the disk must be named and left alone.
#   q35     the q35 under UEFI, with NEC's controller (MSI and MSI-X:
#           MSI is used). The firmware drives it before the kernel
#           starts, and the driver must take it over.
#   polled  NEC's controller with MSI and MSI-X off: the driver polls.
#   hot     a controller with nothing on it; a keyboard is plugged in
#           while the machine runs, typed on, and pulled out - and the
#           PS/2 keyboard types again.
#
# QEMU sends the keys typed through its monitor to the keyboard that
# came last, and pointer moves to the mouse that came last - here the
# USB ones.
# OVMF, in the environment: the UEFI firmware.

set -u
ISO=$1
DISK=$2
LOG=$3
HERE=$(cd "$(dirname "$0")" && pwd)
OVMF=${OVMF:?the UEFI firmware}

fail() {
    echo "FAIL: $1"
    exit 1
}

keys() {
    python3 -I "$HERE/key-names.py" "$1" || fail "no keys for: $1"
}
text() { tr -d '\r' < "$1"; }
has() { # has LOG PATTERN WHY
    text "$1" | grep -aEq "$2" || fail "$1: $3"
}

# What most machines do first: wait for the USB keyboard, type a line.
TYPED="see:keyboard,(0627:0001 $(keys 'echo typed on usb') ret ~"
KBD="-device usb-kbd,bus=xhci.0"

echo "Booting x86_64 four times with USB (logs: $LOG.*)..."
boot() { # boot NAME KEYS QEMU_ARGS
    DISK="$DISK.$1" SMP=2 KEYS="$2" QEMU_ARGS="$3" "$HERE/qemu-boot.sh" "$ISO" "$LOG.$1" 90 > /dev/null 2> "$LOG.$1.err"
}
# A disk each (QEMU locks an image it writes), and one for the stick.
for copy in pc q35 polled hot stick; do
    cp "$DISK" "$DISK.$copy" || fail "cannot copy the disk"
done
click() { echo "!mouse_button,$1 !mouse_button,0"; }
MOUSE="$(click 1) !mouse_move,100,50 $(click 2) !mouse_move,-120,-120 $(click 4)"
HELD="$(keys 'echo ') !sendkey,a,1000 ret ~"
# (A monitor command's commas stand for spaces, so the keyboard has no
# id: it is found where QEMU puts devices without one - after the disk,
# which qemu-boot.sh adds without one too.)
HOTPLUG="!device_add,usb-kbd see:keyboard,(0627:0001 $(keys 'echo the late one') ret ~ !device_del,/machine/peripheral-anon/device[1] see:unplugged $(keys 'echo ps2 again') ret ~"
boot pc "$TYPED $HELD $MOUSE" \
    "-device qemu-xhci,id=xhci $KBD -device usb-mouse,bus=xhci.0 -drive if=none,id=stick,format=raw,file=$DISK.stick -device usb-storage,bus=xhci.0,drive=stick" &
boot q35 "$TYPED" "-machine q35 -bios $OVMF -device nec-usb-xhci,id=xhci $KBD" &
boot polled "$TYPED" "-device nec-usb-xhci,id=xhci,msi=off,msix=off $KBD" &
boot hot "$HOTPLUG" "-device qemu-xhci,id=xhci" &
wait

for machine in pc q35 polled hot; do
    L="$LOG.$machine"
    has "$L" '^\[pci\] .* USB controller: registers 0x[0-9a-f]+\+0x[0-9a-f]+, [0-9]+ KiB DMA at 0x[0-9a-f]+, .* - handed to init$' "the kernel did not hand the xHCI controller on"
    has "$L" '^\[usb\] xHCI [0-9]+\.[0-9a-f]+ controller at ..:..\..: [0-9]+ ports, [0-9]+ scratchpad pages, ' "the driver did not start the controller"
    has "$L" '^\[usb\] port [0-9]+: keyboard \(0627:0001, [a-zA-Z ]+\) ready$' "the keyboard was not enumerated"
    text "$L" | grep -aEq 'KERNEL PANIC|CPU EXCEPTION|\[init\] usb ended|has not answered for|^\[usb\] .*(fails every time|cannot be asked)' && fail "$L: something failed"
done
for machine in pc q35 polled; do
    has "$LOG.$machine" '^typed on usb$' "the line typed on the USB keyboard did not arrive"
done
has "$LOG.pc" '^\[usb\] xHCI .*, MSI-X interrupts$' "the controller's MSI-X was not used"
has "$LOG.q35" '^\[usb\] xHCI .*, MSI interrupts$' "the controller's MSI was not used"
has "$LOG.polled" '^\[usb\] xHCI .*, polled$' "the controller without interrupts was not polled"

# Holding a key: half a second, then 25 a second - at least ten.
text "$LOG.pc" | grep -aEq '^a{10,}$' || fail "$LOG.pc: a held key did not repeat"
# The mouse: the clicks where the pointer was. It starts in the middle
# of the screen (512, 384); 100 right and 50 down; then up and left.
has "$LOG.pc" '^\[usb\] port [0-9]+: mouse \(0627:0001, [a-zA-Z ]+\) ready$' "the mouse was not enumerated"
got=$(grep -aoE '\[display\] [a-z]+ button at \([0-9]+, [0-9]+\)' "$LOG.pc" | sed -e 's/\[display\] //' -e 's/ button at//' | paste -sd ';')
[ "$got" = "left (512, 384);right (612, 434);middle (492, 314)" ] || fail "$LOG.pc: the clicks were seen at: $got"
# The disk: named, not driven.
has "$LOG.pc" '^\[usb\] port [0-9]+: 46f4:0001, [a-zA-Z ]+, mass storage \(class 0x08\) - not driven$' "the USB disk was not named"
# Plugged in while running, typed on, pulled out.
has "$LOG.hot" '^the late one$' "the line typed on the keyboard plugged in later did not arrive"
has "$LOG.hot" '^\[usb\] port [0-9]+: unplugged$' "the keyboard was not seen to go"
has "$LOG.hot" '^ps2 again$' "the PS/2 keyboard did not type after the USB one went"
echo "USB worked: a keyboard typed (MSI-X, MSI under UEFI, polled), a held key repeated, a mouse clicked where it was, a disk was named, and a keyboard came and went"
