"""Print the W32 save-state layout hash of a checkout, without building.

    python -I tools/win32_layout.py [checkout]

computes what `win32_layout` in src/bus/state.rs computes: FNV-1a of
the lists of saved fields of src/win32 and src/win16 (each
`state_fields!`, `state_enum!` and `state_variants!` without comments,
spaces or the fields it skips), in sorted order. It prints the hash in
the form WIN32_LAYOUTS writes it, then the last line of WIN32_LAYOUTS
and WIN32_VERSION. It exits with 1, saying what to change, when that
line isn't WIN32_VERSION with the hash, which is what the test
`a_changed_win32_layout_has_its_own_version` asserts.

A merge that moves a branch's W32 version past one main took needs the
new hash, which the test prints only after the crate's tests are
built. This gives it in a second. It ports the Rust code: a change to
one is a change to both. The checkout defaults to the one the script is
in.
"""

import os
import re
import sys

# What Rust's `char::is_whitespace` takes for a space: Unicode's
# White_Space. `str.isspace` takes U+001C to U+001F as well.
WHITESPACE = set(map(chr, [
    *range(0x09, 0x0E), 0x20, 0x85, 0xA0, 0x1680, *range(0x2000, 0x200B),
    0x2028, 0x2029, 0x202F, 0x205F, 0x3000,
]))


def macro_body(code, at):
    """The text between the parentheses of the macro call at `at`."""
    start = code.find("(", at)
    if start < 0:
        return None
    start += 1
    depth = 1
    for i in range(start, len(code)):
        if code[i] == "(":
            depth += 1
        elif code[i] == ")":
            depth -= 1
        if depth == 0:
            return code[start:i]
    return None


def saved_lists(source):
    """The lists of saved fields in the Rust source `source`."""
    code = "\n".join(line.split("//")[0] for line in source.split("\n"))
    lists = []
    for name in ("state_fields!", "state_enum!", "state_variants!"):
        at = code.find(name)
        while at >= 0:
            body = macro_body(code, at)
            if body is not None:
                body = "".join(c for c in body if c not in WHITESPACE)
                end = body.find("}skip{")
                lists.append(body if end < 0 else body[:end + 1])
            at = code.find(name, at + len(name))
    return lists


def collect(folder, lists):
    for entry in os.listdir(folder):
        path = os.path.join(folder, entry)
        if os.path.isdir(path):
            collect(path, lists)
        # As Rust's `Path::extension`, which gives a file named ".rs" none.
        elif os.path.splitext(entry)[1] == ".rs":
            with open(path, encoding="utf-8", newline="") as f:
                lists.extend(saved_lists(f.read()))


def layout(root):
    lists = []
    collect(os.path.join(root, "src", "win32"), lists)
    collect(os.path.join(root, "src", "win16"), lists)
    # Rust sorts Strings by their bytes.
    lists.sort(key=lambda s: s.encode("utf-8"))
    h = 0xCBF29CE484222325
    for byte in "\n".join(lists).encode("utf-8"):
        h = ((h ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def written(h):
    digits = "%016X" % h
    return "0x" + "_".join(digits[i:i + 4] for i in range(0, 16, 4))


def main():
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    root = sys.argv[1] if len(sys.argv) > 1 else here
    h = written(layout(root))
    print(h)
    path = os.path.join(root, "src", "bus", "state.rs")
    with open(path, encoding="utf-8") as f:
        state = f.read()
    version = re.search(r"const WIN32_VERSION: u16 = (\d+);", state)
    table = state.find("const WIN32_LAYOUTS")
    if version is None or table < 0:
        sys.exit(f"{path} has no WIN32_VERSION or no WIN32_LAYOUTS")
    version = int(version.group(1))
    entries = []
    for line in state[table:].split("\n")[1:]:
        line = line.split("//")[0]
        if "];" in line:
            break
        entries += re.findall(r"\((\d+), (0x[0-9A-F_]+)\)", line)
    if not entries:
        sys.exit(f"{path}: WIN32_LAYOUTS has no lines")
    last = (int(entries[-1][0]), entries[-1][1])
    print(f"WIN32_LAYOUTS ends with {last[0]}, {last[1]}; "
          f"WIN32_VERSION is {version}")
    # What `a_changed_win32_layout_has_its_own_version` asserts.
    if last[0] < version:
        sys.exit(f"add ({version}, {h}) to the end of WIN32_LAYOUTS")
    if last != (version, h):
        sys.exit(f"raise WIN32_VERSION past {last[0]} and add the new "
                 f"version with {h} to the end of WIN32_LAYOUTS")


if __name__ == "__main__":
    main()
