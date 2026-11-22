#!/usr/bin/env python3
"""key-names.py TEXT - the QEMU key names that type TEXT on a US
keyboard, one word per key, for the boot scripts' KEYS: "Hi!" is
"shift-h i shift-1". Spaces are "spc". Every character must have a key;
anything else (a tab, a new line, a letter beyond ASCII) is an error -
type those with their own key names."""
import sys

SHIFTED = {'!': '1', '@': '2', '#': '3', '$': '4', '%': '5', '^': '6', '&': '7', '*': '8', '(': '9', ')': '0',
           '_': 'minus', '+': 'equal', '{': 'bracket_left', '}': 'bracket_right', '|': 'backslash',
           ':': 'semicolon', '"': 'apostrophe', '<': 'comma', '>': 'dot', '?': 'slash', '~': 'grave_accent'}
PLAIN = {' ': 'spc', '-': 'minus', '=': 'equal', '[': 'bracket_left', ']': 'bracket_right', '\\': 'backslash',
         ';': 'semicolon', "'": 'apostrophe', ',': 'comma', '.': 'dot', '/': 'slash', '`': 'grave_accent'}


def key(ch):
    if ch.isascii() and (ch.isdigit() or ch.islower()):
        return ch
    if ch.isascii() and ch.isupper():
        return 'shift-' + ch.lower()
    if ch in SHIFTED:
        return 'shift-' + SHIFTED[ch]
    if ch in PLAIN:
        return PLAIN[ch]
    sys.exit(f"key-names: no key for {ch!r}")


print(' '.join(key(ch) for ch in sys.argv[1]))
