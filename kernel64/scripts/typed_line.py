#!/usr/bin/env python3
"""typed_line.py KEY... - what each line typed by the boot tests says, so
that a typer can wait for that line, not just for "a line", to have run.

The typers (qemu-boot.sh, serial-type.py, qmp-type.py) send key names -
QEMU's, as key-names.py spells them. A "~" in them waits until the line
just entered has run. Counting the shell's "[shell] ran" lines cannot say
which line ran: under emulation the shell falls behind the typing, and the
"ran" of the line before arrives after the next Enter (journal entry 109).
The shell logs each line it runs as `[shell] ran "TEXT"`, TEXT quoted as
Rust quotes a string; this works out that line from the keys.

As a program: prints, for each "ret" among the keys, the line the shell
logs when it has run that line - or an empty line when the keys since the
last Enter are not plain text (arrows, F-keys), and only the old count can
be used.
"""
import sys

NAMED = {
    "spc": b" ", "ret": b"\r", "backspace": b"\x7f", "tab": b"\t", "esc": b"\x1b",
    "dot": b".", "slash": b"/", "backslash": b"\\", "apostrophe": b"'", "comma": b",", "minus": b"-",
    "shift-apostrophe": b'"', "shift-dot": b">", "shift-comma": b"<", "shift-backslash": b"|",
    "shift-7": b"&", "shift-minus": b"_", "shift-5": b"%",
    "up": b"\x1b[A", "down": b"\x1b[B", "right": b"\x1b[C", "left": b"\x1b[D",
    "f11": b"\x1b[23~", "f12": b"\x1b[24~",
}
# The rest of a US keyboard's punctuation, as key-names.py names it, so
# that anything it can spell can be typed too: plain, and shifted.
_PLAIN = {"equal": "=", "bracket_left": "[", "bracket_right": "]", "semicolon": ";", "grave_accent": "`"}
_SHIFTED = {"1": "!", "2": "@", "3": "#", "4": "$", "6": "^", "8": "*", "9": "(", "0": ")", "equal": "+",
            "bracket_left": "{", "bracket_right": "}", "semicolon": ":", "slash": "?", "grave_accent": "~"}
NAMED.update({name: text.encode() for name, text in _PLAIN.items()})
NAMED.update({"shift-" + name: text.encode() for name, text in _SHIFTED.items()})

# Tokens that are not keys: waits, and the typers' own commands.
NOT_KEYS = ("/", "~", "~~")


def key_bytes(key):
    """The bytes a key types at a terminal, or None if it is not known."""
    if key in NAMED:
        return NAMED[key]
    if len(key) == 1:
        return key.encode()
    if key.startswith("shift-") and len(key) == 7 and key[6].isalpha():
        return key[6].upper().encode()
    if key.startswith("ctrl-") and len(key) == 6 and key[5].isalpha():
        return bytes([ord(key[5].lower()) - ord("a") + 1])
    return None


def ran_line(text):
    """What the shell logs when it has run TEXT."""
    quoted = text.replace(b"\\", b"\\\\").replace(b'"', b'\\"')
    return b'[shell] ran "' + quoted + b'"'


def lines(keys):
    """For each "ret" among KEYS: the line the shell logs once it has run
    what was typed since the Enter before, or None if that was not plain
    text."""
    text, plain = b"", True
    for key in keys:
        if key in NOT_KEYS or key[:4] == "see:":
            continue
        if key[:1] in ("!", "@"):
            # The pointer, or a monitor command: whatever was typed before it
            # went to a window (the editor's, closed with a click), not to a
            # line of the shell's.
            text, plain = b"", True
            continue
        if key == "ret":
            yield ran_line(text) if plain and text else None
            text, plain = b"", True
        elif key == "backspace":
            text = text[:-1]
        elif key[:1] == "f" and key[1:].isdigit():
            # F11 and F12 are for the drivers (deliberate bugs), not the line.
            continue
        elif key.startswith("ctrl-"):
            # Ctrl+C abandons the line; Ctrl+D ends a program's input. Either
            # way the shell's next line starts here.
            text, plain = b"", True
        else:
            typed = key_bytes(key)
            if typed is None or any(b < 0x20 or b == 0x7f for b in typed):
                plain = False
            else:
                text += typed


if __name__ == "__main__":
    for line in lines(sys.argv[1:]):
        sys.stdout.buffer.write((line or b"") + b"\n")
