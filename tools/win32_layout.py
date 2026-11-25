"""Print the W32 save-state layout hash of a checkout, without building.

    python -I tools/win32_layout.py [checkout]

computes what `win32_layout` in src/bus/state.rs computes: FNV-1a of
the lists of saved fields of src/win32 and src/win16 (each
`state_fields!`, `state_enum!` and `state_variants!` without comments,
spaces or the fields it skips), in sorted order. It prints the hash in
the form WIN32_LAYOUTS writes it, and whether the last line of
WIN32_LAYOUTS has it.

A merge that moves a branch's W32 version past one main took needs the
new hash, which the test `a_changed_win32_layout_has_its_own_version`
prints only after the crate's tests are built. This gives it in a
second. It ports the Rust code: a change to one is a change to both.
The checkout defaults to the one the script is in.
"""

import os
import re
import sys


def macro_body(code, at):
    """The text between the parentheses of the macro call at `at`."""
    start = code.find('(', at)
    if start < 0:
        return None
    start += 1
    depth = 1
    for i in range(start, len(code)):
        if code[i] == '(':
            depth += 1
        elif code[i] == ')':
            depth -= 1
        if depth == 0:
            return code[start:i]
    return None


def saved_lists(source):
    """The lists of saved fields in the Rust source `source`."""
    code = '\n'.join(line.split('//')[0] for line in source.split('\n'))
    lists = []
    for name in ('state_fields!', 'state_enum!', 'state_variants!'):
        at = code.find(name)
        while at >= 0:
            body = macro_body(code, at)
            if body is not None:
                body = ''.join(c for c in body if not c.isspace())
                end = body.find('}skip{')
                lists.append(body if end < 0 else body[:end + 1])
            at = code.find(name, at + len(name))
    return lists


def collect(folder, lists):
    for entry in os.listdir(folder):
        path = os.path.join(folder, entry)
        if os.path.isdir(path):
            collect(path, lists)
        elif entry.endswith('.rs'):
            with open(path, encoding='utf-8', newline='') as f:
                lists.extend(saved_lists(f.read()))


def layout(root):
    lists = []
    collect(os.path.join(root, 'src', 'win32'), lists)
    collect(os.path.join(root, 'src', 'win16'), lists)
    # Rust sorts Strings by their bytes.
    lists.sort(key=lambda s: s.encode('utf-8'))
    h = 0xCBF29CE484222325
    for byte in '\n'.join(lists).encode('utf-8'):
        h = ((h ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def written(h):
    digits = '%016X' % h
    return '0x' + '_'.join(digits[i:i + 4] for i in range(0, 16, 4))


def main():
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    root = sys.argv[1] if len(sys.argv) > 1 else here
    h = written(layout(root))
    print(h)
    with open(os.path.join(root, 'src', 'bus', 'state.rs'), encoding='utf-8') as f:
        lines = re.findall(r'\((\d+), (0x[0-9A-F_]+)\)', f.read())
    if lines:
        version, last = lines[-1]
        state = 'has it' if last == h else 'has another'
        print(f'WIN32_LAYOUTS ends with ({version}, {last}): {state}')


if __name__ == '__main__':
    main()
