#!/bin/sh
# usb-test.sh - USB: an xHCI controller with keyboards and a mouse on
# it, typed on and moved through QEMU's monitor.
#
#   usage: usb-test.sh ISO DISK LOG
#
# Five machines, booted at once:
#   pc      QEMU's PC (BIOS) with qemu-xhci (MSI-X only), a USB keyboard,
#           a USB mouse and a USB stick. A line is typed on the keyboard;
#           a key is held down for a second, and must repeat; the mouse
#           is moved and clicked, and the display must see the clicks
#           where the pointer is; the stick must be started (the system's
#           disk is the virtio one, so it is not used).
#   q35     the q35 under UEFI, with NEC's controller (MSI and MSI-X:
#           MSI is used), and a stick on it. The firmware drives both
#           before the kernel starts, and the driver must take them over.
#   polled  NEC's controller with MSI and MSI-X off: the driver polls.
#           No disk at all: usb serves the disk's requests, with nothing
#           in the drive, and fs keeps its files in memory.
#   hot     a controller with nothing on it; a keyboard is plugged in
#           while the machine runs, typed on, and pulled out - and the
#           PS/2 keyboard types again.
#   stick   no disk but a USB stick: it is the system's disk. fs finds
#           TaajFS on it; `disk check`, files read, written and copied,
#           programs loaded from it; then usb goes silent (`disk stall`),
#           init's watchdog restarts it, and the new usb takes the
#           controller over and serves the stick again. Then `poweroff`:
#           usb, the disk's driver now, is stopped after fs, not with the
#           keyboards. The files written are read off the stick's image
#           on the host.
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
ROOT=$(cd "$HERE/../../.." && pwd)
TFSTOOL=${TFSTOOL:-$ROOT/tools/tfstool-rs/target/release/tfstool}
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

echo "Booting x86_64 five times with USB (logs: $LOG.*)..."
boot() { # boot NAME KEYS QEMU_ARGS [no-disk]
    [ "${4:-}" = no-disk ] && disk= || disk="$DISK.$1"
    DISK="$disk" SMP=2 KEYS="$2" QEMU_ARGS="$3" "$HERE/qemu-boot.sh" "$ISO" "$LOG.$1" 90 > /dev/null 2> "$LOG.$1.err"
}
# Each step on the stick machine is marked in the log by a line of its
# own, "== NAME ==".
step() { # step NAME COMMAND...
    out="$(keys "echo == $1 ==") ret ~"
    shift
    for command in "$@"; do out="$out $(keys "$command") ret ~"; done
    echo "$out"
}
[ -f "$DISK.long" ] || fail "no test disk at $DISK"
# A disk each (QEMU locks an image it writes), and the sticks.
for copy in pc q35 polled hot stick q35stick sole; do
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
boot q35 "$TYPED" "-machine q35 -bios $OVMF -device nec-usb-xhci,id=xhci $KBD -drive if=none,id=stick,format=raw,file=$DISK.q35stick -device usb-storage,bus=xhci.0,drive=stick" &
boot polled "$TYPED" "-device nec-usb-xhci,id=xhci,msi=off,msix=off $KBD" no-disk &
boot hot "$HOTPLUG" "-device qemu-xhci,id=xhci" &
SOLE="$(step info 'disk')"
SOLE="$SOLE $(step check 'disk check')"
SOLE="$SOLE $(step files 'cat welcome' 'wc long' 'cat docs/readme')"
SOLE="$SOLE $(step write 'write saved kept' 'cp long copy' 'wc copy')"
# Silent for 3 s: init gives a disk driver 2. Then the new usb resets
# the controller, finds the stick again, and serves it.
SOLE="$SOLE $(step stall 'disk stall 3') see:usb,ended,with,code,-1001"
SOLE="$SOLE $(step after 'disk read 0' 'disk check' 'cat saved')"
SOLE="$SOLE $(keys 'echo == end ==') ret ~ $(keys poweroff) ret"
# No DISK: the stick is the only disk. SLOTS=3: QEMU's disk thread.
# (Its QEMU options are made first: in "DISK= ... QEMU_ARGS=...$DISK",
# the second would see DISK already empty.)
SOLE_ARGS="-device qemu-xhci,id=xhci -drive if=none,id=stick,format=raw,file=$DISK.sole -device usb-storage,bus=xhci.0,drive=stick"
DISK= SMP=2 SLOTS=3 KEYS="$SOLE" QEMU_ARGS="$SOLE_ARGS" \
    "$HERE/qemu-boot.sh" "$ISO" "$LOG.stick" 90 > /dev/null 2> "$LOG.stick.err" &
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
has "$LOG.polled" '\[fs\] no disk \(nothing in the drive\): files in memory only' "fs was not told that no stick is in"

# Holding a key: half a second, then 25 a second - at least ten.
text "$LOG.pc" | grep -aEq '^a{10,}$' || fail "$LOG.pc: a held key did not repeat"
# The mouse: the clicks where the pointer was. It starts in the middle
# of the screen (512, 384); 100 right and 50 down; then up and left.
has "$LOG.pc" '^\[usb\] port [0-9]+: mouse \(0627:0001, [a-zA-Z ]+\) ready$' "the mouse was not enumerated"
got=$(grep -aoE '\[display\] [a-z]+ button at \([0-9]+, [0-9]+\)' "$LOG.pc" | sed -e 's/\[display\] //' -e 's/ button at//' | paste -sd ';')
[ "$got" = "left (512, 384);right (612, 434);middle (492, 314)" ] || fail "$LOG.pc: the clicks were seen at: $got"
# The stick: started, though the system's disk is the virtio one.
for machine in pc q35; do
    has "$LOG.$machine" '^\[usb\] port [0-9]+: disk "QEMU QEMU HARDDISK" \(46f4:0001, [a-zA-Z ]+\), 8192 sectors \(4 MiB\) ready$' "the USB stick was not started"
done
# Plugged in while running, typed on, pulled out.
has "$LOG.hot" '^the late one$' "the line typed on the keyboard plugged in later did not arrive"
has "$LOG.hot" '^\[usb\] port [0-9]+: unplugged$' "the keyboard was not seen to go"
has "$LOG.hot" '^ps2 again$' "the PS/2 keyboard did not type after the USB one went"

# The stick as the system's disk.
tr -d '\r' < "$LOG.stick" > "$LOG.stick.text"
part() { # part NAME: the lines after "== NAME ==", to the next mark
    awk -v start="== $1 ==" '$0 == start { on = 1; next } /^== [a-z-]+ ==$/ { on = 0 } on' "$LOG.stick.text"
}
step_has() { # step_has NAME PATTERN WHY
    part "$1" | grep -aEq "$2" || fail "$LOG.stick: $3"
}
S="$LOG.stick.text"
grep -aq '^== end ==$' "$S" || fail "$S: the steps did not all run"
grep -aEq 'KERNEL PANIC|CPU EXCEPTION|THE SYSTEM HAS STOPPED' "$S" && fail "$S: the kernel stopped"
grep -aEq '\[(blk|ahci|nvme)\]' "$S" && fail "$S: another disk driver ran"
grep -aE '^\[usb\] the disk: ' "$S" && fail "$S: the stick reported an error"
has "$S" '^\[usb\] port [0-9]+: disk "QEMU QEMU HARDDISK" \(46f4:0001, [a-zA-Z ]+\), 8192 sectors \(4 MiB\) ready$' "the stick was not started"
has "$S" '\[fs\] ready: TaajFS on the disk' "fs did not find TaajFS on the stick"
step_has info '^disk: 8192 sectors of 512 bytes \(4096 KiB\)$' "the stick's size, from READ CAPACITY, is wrong"
step_has check '^disk check: all 9 checks passed$' "disk check failed through usb"
step_has files '^Hello from the host$' "a file on the stick could not be read"
step_has files '^long: 300 lines, 1200 words, 4692 bytes$' "wc did not count the long file, read in many sectors at once"
step_has files '\[init\] started wc for the shell, from a file' "wc was not loaded from the stick"
step_has write '^copy: 300 lines, 1200 words, 4692 bytes$' "the copy, written to the stick, is not the file it was copied from"
step_has stall '\[usb\] asked to stall' "usb did not stall"
step_has stall '\[init\] usb has not answered for 2 s - stopping it' "init did not notice that usb stopped answering"
step_has stall '\[init\] usb ended with code -1001 - restarting it' "init did not restart usb"
order=$(grep -aoE '^\[init\] stopped (shell|kbd|fs|usb)$' "$S" | sed 's/.* //' | paste -sd ' ')
[ "$order" = "shell kbd fs usb" ] || fail "$S: at poweroff the services stopped in the order: $order"
[ "$(grep -ac '^\[usb\] xHCI ' "$S")" = 2 ] || fail "$S: the restarted usb did not take the controller over"
step_has after '^0000  4a 41 46 53 ' "the restarted usb did not read sector 0 (TaajFS's JAFS)"
step_has after '^disk check: all 9 checks passed$' "disk check failed through the restarted usb"
step_has after '^kept$' "a file written before the restart was not there after it"
[ "$("$TFSTOOL" get "$DISK.sole" saved 2>&1)" = "kept" ] || fail "the stick's image: \"saved\" does not hold \"kept\""
"$TFSTOOL" get "$DISK.sole" copy > "$LOG.copy" 2>&1 && cmp -s "$LOG.copy" "$DISK.long" || fail "the stick's image: \"copy\" is not \"long\""
echo "USB worked: a keyboard typed (MSI-X, MSI under UEFI, polled), a held key repeated, a mouse clicked where it was, a keyboard came and went, and a stick was the system's disk - across a restart of its driver"
