"""Resolve the numbers a merge of main into a branch leaves in conflict.

    python -I tools/merge_numbers.py [checkout]

run while a `git merge origin/main` (or any merge into the branch) has
stopped with conflicts. Two kinds of number are handed out by "the next
free one", so two branches that each take one clash on it:

- The W32 save-state version (`WIN32_VERSION` in src/bus/state.rs).
  As AGENTS.md says, the branch takes the next free number and main's
  joins the versions the branch loads: main's lines of WIN32_LAYOUTS
  stay and the branch's line follows with the new number and the new
  hash (`win32_layout.py`'s), main's version goes into WIN32_LOADS,
  the comment above WIN32_LOADS gets main's clause and then the
  branch's, and the gates the branch added for its version
  (`*_SAVED: u16 = N`, `Since<N, _>`, `Until<N, _>`) move with it.
- Places in an append-only table: a `static` whose doc comment says
  "Append only" (the `THENS` of src/win16/thunk.rs and
  src/win32/call.rs), and the `pub const NAME: u16 = N;` lines that
  name its places. Main's entries keep their places, and the branch's
  follow them in the same order, their constants renumbered to match.

Only conflicts of these shapes are touched; the rest stay as they are
for a person to resolve, and the script lists the files that still
have them. It stages the files it left with no conflict. It handles a
branch that added one W32 version; for more it changes nothing and
says so. The checkout defaults to the one the script is in.
"""

import importlib.util
import os
import re
import subprocess
import sys
import textwrap

OURS = "<<<<<<<"
BASE = "|||||||"
SPLIT = "======="
THEIRS = ">>>>>>>"

# The prose of comments ends before column 77 (AGENTS.md).
WRAP = 76


class Conflict:
    """One conflict: the branch's lines (ours) and main's (theirs), each
    with its line ending."""

    def __init__(self, ours, theirs, raw):
        self.ours = ours
        self.theirs = theirs
        self.raw = raw  # the lines as the file has them, markers too
        self.resolved = None  # the lines that replace it, once resolved


def split(lines):
    """The lines of a file as a list of plain lines and Conflicts."""
    parts = []
    i = 0
    while i < len(lines):
        if not lines[i].startswith(OURS):
            parts.append(lines[i])
            i += 1
            continue
        raw = [lines[i]]
        ours, theirs = [], []
        side = ours
        i += 1
        while i < len(lines) and not lines[i].startswith(THEIRS):
            line = lines[i]
            raw.append(line)
            if line.startswith(BASE):
                side = None  # diff3's base section: skipped
            elif line.rstrip("\r\n") == SPLIT:
                side = theirs
            elif side is not None:
                side.append(line)
            i += 1
        if i == len(lines):
            sys.exit("a conflict has no end marker")
        raw.append(lines[i])
        parts.append(Conflict(ours, theirs, raw))
        i += 1
    return parts


def join(parts):
    out = []
    for part in parts:
        if isinstance(part, Conflict):
            out.extend(part.raw if part.resolved is None else part.resolved)
        else:
            out.append(part)
    return "".join(out)


def text_before(parts, index):
    """The file's lines before part `index`, main's side of any conflict."""
    out = []
    for part in parts[:index]:
        if isinstance(part, Conflict):
            out.extend(part.theirs if part.resolved is None else part.resolved)
        else:
            out.append(part)
    return out


def meaningful(lines):
    return [line for line in lines if line.strip()]


# Append-only tables.

STATIC = re.compile(r"\bstatic \w+: &\[[^\]]*\] = &\[")
CONST = re.compile(r"^(\s*pub const \w+: u16 = )(\d+)(;.*)$", re.S)


def in_append_only_table(before):
    """Whether the lines `before` end inside an append-only table."""
    for at in range(len(before) - 1, -1, -1):
        line = before[at]
        if line.strip().startswith("];"):
            return False
        if STATIC.search(line):
            docs = []
            for doc in reversed(before[:at]):
                if not doc.strip().startswith("///"):
                    break
                docs.append(doc)
            return "append only" in " ".join(docs).lower()
    return False


def is_entries(lines):
    entries = meaningful(lines)
    return bool(entries) and all(
        e.rstrip().endswith(",") and not e.strip().startswith("//")
        for e in entries)


def numbers(lines):
    """The numbers of `pub const NAME: u16 = N;` lines, or None if some
    other line is among them or they don't count up by one."""
    found = []
    for line in meaningful(lines):
        m = CONST.match(line)
        if m is None:
            return None
        found.append(int(m.group(2)))
    if not found or found != list(range(found[0], found[0] + len(found))):
        return None
    return found


def has_append_only_table(lines):
    return any(STATIC.search(line) and in_append_only_table(lines[:i + 1])
               for i, line in enumerate(lines))


def resolve_tables(path, parts):
    """Resolve the conflicts of append-only tables and their constants in
    one file. Returns what was done, a line for each."""
    done = []
    plain = text_before(parts, len(parts))
    if not has_append_only_table(plain):
        return done
    for index, part in enumerate(parts):
        if not isinstance(part, Conflict):
            continue
        if (is_entries(part.ours) and is_entries(part.theirs)
                and in_append_only_table(text_before(parts, index))):
            # Main's entries keep their places; the branch's follow.
            part.resolved = part.theirs + part.ours
            done.append(f"{path}: the branch's {len(meaningful(part.ours))} "
                        f"table entries put after main's")
            continue
        ours, theirs = numbers(part.ours), numbers(part.theirs)
        if ours is None or theirs is None or ours[0] != theirs[0]:
            continue
        shift = theirs[-1] + 1 - ours[0]
        moved = []
        for line in part.ours:
            m = CONST.match(line)
            if m is None:
                moved.append(line)
            else:
                moved.append(f"{m.group(1)}{int(m.group(2)) + shift}"
                             f"{m.group(3)}")
        part.resolved = part.theirs + moved
        done.append(f"{path}: the branch's places {ours[0]}..{ours[-1]} "
                    f"moved to {ours[0] + shift}..{ours[-1] + shift}")
    return done


# The W32 save-state version.

LAYOUT = re.compile(r"^(\s*\()(\d+)(, )(0x[0-9A-F_]+)(\),.*)$", re.S)
VERSION = re.compile(r"(const WIN32_VERSION: u16 = )(\d+)(;)")


def layout_lines(lines):
    """The (version, line) of each WIN32_LAYOUTS line in `lines`, or None
    if another line is among them."""
    found = []
    for line in meaningful(lines):
        m = LAYOUT.match(line)
        if m is None:
            return None
        found.append((int(m.group(2)), line))
    return found or None


def rewrap(lines, text):
    """`text` as `///` lines indented as the first of `lines`."""
    indent = lines[0][:len(lines[0]) - len(lines[0].lstrip())]
    ending = "\r\n" if lines[0].endswith("\r\n") else "\n"
    prefix = indent + "/// "
    wrapped = textwrap.wrap(text, WRAP - len(prefix), break_long_words=False,
                            break_on_hyphens=False)
    return [prefix + line + ending for line in wrapped]


def doc_text(lines):
    return " ".join(line.strip()[3:].strip() for line in lines)


def merged_clauses(ours, theirs, old, new):
    """The comment above WIN32_LOADS ends with a clause per version that
    lacks something, the last after ", and ". Main's text, with the
    branch's last clause added after main's, its range ending at the
    version before the new one."""
    if not (all(line.strip().startswith("///") for line in meaningful(ours))
            and all(line.strip().startswith("///")
                    for line in meaningful(theirs))):
        return None
    mine, main = doc_text(meaningful(ours)), doc_text(meaningful(theirs))
    if not mine.endswith(".") or not main.endswith("."):
        return None
    at_mine, at_main = mine.rfind(", and "), main.rfind(", and ")
    if at_mine < 0 or at_main < 0:
        return None
    clause = mine[at_mine + len(", and "):-1]
    # "71's to 117's" becomes "71's to 118's" (or "117's" alone).
    clause, n = re.subn(rf"\b{old - 1}'s\b", f"{new - 1}'s", clause, count=1)
    if n == 0:
        return None
    return (main[:at_main] + ", " + main[at_main + len(", and "):-1]
            + ", and " + clause + ".")


def branch_gates(root, base, old):
    """The lines the branch added that gate on its version `old`, by file:
    `*_SAVED: u16 = old`, `Since<old, ...>` and `Until<old, ...>`."""
    diff = git(root, "diff", "-U0", "--no-color", base, "HEAD", "--", "src",
               "crates", "tests")
    gate = re.compile(rf"(_SAVED: u16 = {old};|\b(Since|Until)<{old},)")
    found = {}
    path = None
    for line in diff.split("\n"):
        if line.startswith("+++ "):
            path = line[6:] if line.startswith("+++ b/") else None
        elif line.startswith("+") and path and gate.search(line):
            found.setdefault(path, []).append(line[1:])
    return found


def move_gates(root, gates, old, new):
    done = []
    pattern = re.compile(rf"(_SAVED: u16 = ){old}(;)|\b(Since|Until)<{old},")

    def bump(m):
        if m.group(1):
            return f"{m.group(1)}{new}{m.group(2)}"
        return f"{m.group(3)}<{new},"

    for path, added in gates.items():
        full = os.path.join(root, path)
        with open(full, encoding="utf-8", newline="") as f:
            lines = f.readlines()
        wanted = set(added)
        count = 0
        for i, line in enumerate(lines):
            if line.rstrip("\r\n") in wanted:
                lines[i] = pattern.sub(bump, line)
                count += lines[i] != line
        with open(full, "w", encoding="utf-8", newline="") as f:
            f.write("".join(lines))
        done.append(f"{path}: {count} gate(s) moved from {old} to {new}")
    return done


def add_to_loads(text, version):
    """Main's version among those WIN32_LOADS lists, before
    WIN32_VERSION, and the array's length to match."""
    m = re.search(r"const WIN32_LOADS: \[u16; (\d+)\] = \[\n(.*?)\];", text,
                  re.S)
    if m is None:
        sys.exit("src/bus/state.rs has no WIN32_LOADS")
    entries = m.group(2).split("\n")
    listed = [int(e.strip().rstrip(",")) for e in entries
              if e.strip().rstrip(",").isdigit()]
    if version not in listed:
        at = next(i for i, e in enumerate(entries)
                  if e.strip() == "WIN32_VERSION,")
        indent = entries[at][:len(entries[at]) - len(entries[at].lstrip())]
        entries.insert(at, f"{indent}{version},")
    body = "\n".join(entries)
    count = sum(1 for e in entries if e.strip())
    return (text[:m.start()] + f"const WIN32_LOADS: [u16; {count}] = [\n"
            + body + "];" + text[m.end():])


def resolve_version(root, base):
    """Resolve the W32 version clash in src/bus/state.rs. Returns what was
    done, or None if the file has no clash of that shape."""
    path = os.path.join(root, "src", "bus", "state.rs")
    with open(path, encoding="utf-8", newline="") as f:
        parts = split(f.read().splitlines(keepends=True))
    conflicts = [p for p in parts if isinstance(p, Conflict)]
    table = None
    for part in conflicts:
        ours, theirs = layout_lines(part.ours), layout_lines(part.theirs)
        if ours is not None and theirs is not None:
            table = (part, ours, theirs)
    if table is None:
        return None
    part, ours, theirs = table
    main_versions = {v for v, _ in theirs}
    # Main's lines as the branch has them too are main's; the branch's
    # own line may have main's number, with another hash.
    main_lines = {line.strip() for _, line in theirs}
    added = [(v, line) for v, line in ours if line.strip() not in main_lines]
    if len(added) != 1:
        sys.exit(f"the branch adds {len(added)} W32 versions to "
                 "WIN32_LAYOUTS; this script handles one. Nothing changed.")
    old, line = added[0]
    main = max(main_versions)
    new = main + 1
    m = LAYOUT.match(line)
    part.resolved = part.theirs + [
        f"{m.group(1)}{new}{m.group(3)}{m.group(4)}{m.group(5)}"]

    for other in conflicts:
        if other is part:
            continue
        joined = "".join(other.ours + other.theirs)
        if VERSION.search(joined) and len(meaningful(other.ours)) == 1:
            other.resolved = other.ours
            continue
        text = merged_clauses(other.ours, other.theirs, old, new)
        if text is not None:
            other.resolved = rewrap(meaningful(other.theirs), text)

    text = join(parts)
    text = VERSION.sub(lambda v: f"{v.group(1)}{new}{v.group(3)}", text,
                       count=1)
    text = add_to_loads(text, main)
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(text)
    done = [f"src/bus/state.rs: main keeps W32 version {main}, the branch "
            f"moves from {old} to {new} and loads {main}"]
    if any(p.resolved is None for p in conflicts):
        done.append("src/bus/state.rs: a conflict there isn't one this "
                    "script knows; the comment above WIN32_LOADS may be "
                    "among them")
    if old != new:
        done += move_gates(root, branch_gates(root, base, old), old, new)
    return done, new


def set_hash(root, new):
    """The new layout hash on the branch's WIN32_LAYOUTS line."""
    script = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                          "win32_layout.py")
    spec = importlib.util.spec_from_file_location("win32_layout", script)
    layout = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(layout)
    h = layout.written(layout.layout(root))
    path = os.path.join(root, "src", "bus", "state.rs")
    with open(path, encoding="utf-8", newline="") as f:
        text = f.read()
    text, n = re.subn(rf"(\(\s*{new}, )0x[0-9A-F_]+(\))", rf"\g<1>{h}\2",
                      text, count=1)
    if n != 1:
        sys.exit(f"WIN32_LAYOUTS has no line for {new}")
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(text)
    return f"src/bus/state.rs: the layout hash of {new} is {h}"


# Git.

def git(root, *args):
    result = subprocess.run(["git", "-C", root, *args], capture_output=True,
                            text=True, encoding="utf-8")
    if result.returncode != 0:
        sys.exit(f"git {' '.join(args)}: {result.stderr.strip()}")
    return result.stdout


def has_markers(path):
    with open(path, encoding="utf-8", newline="") as f:
        return any(line.startswith((OURS, THEIRS)) for line in f)


def main():
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    root = sys.argv[1] if len(sys.argv) > 1 else here
    git_dir = git(root, "rev-parse", "--absolute-git-dir").strip()
    if not os.path.exists(os.path.join(git_dir, "MERGE_HEAD")):
        sys.exit("no merge is in progress")
    base = git(root, "merge-base", "HEAD", "MERGE_HEAD").strip()
    conflicted = [p for p in git(root, "diff", "--name-only",
                                 "--diff-filter=U").split("\n") if p]

    # The version first: it refuses before anything is written.
    done = []
    new = None
    if "src/bus/state.rs" in conflicted:
        result = resolve_version(root, base)
        if result is not None:
            steps, new = result
            done += steps

    for path in conflicted:
        if not path.endswith(".rs"):
            continue
        full = os.path.join(root, path)
        with open(full, encoding="utf-8", newline="") as f:
            parts = split(f.read().splitlines(keepends=True))
        steps = resolve_tables(path, parts)
        if steps:
            with open(full, "w", encoding="utf-8", newline="") as f:
                f.write(join(parts))
            done += steps

    left = [p for p in conflicted if has_markers(os.path.join(root, p))]
    if new is not None:
        done.append(set_hash(root, new))
        if any(p.startswith(("src/win32/", "src/win16/")) for p in left):
            done.append("conflicts are left in src/win32 or src/win16: once "
                        "they are resolved, run tools/win32_layout.py, which "
                        "says if the hash has to change again")
    for line in done:
        print(line)
    resolved = [p for p in conflicted if p not in left]
    if resolved:
        git(root, "add", "--", *resolved)
    if left:
        print("still in conflict: " + ", ".join(left))
        sys.exit(1)
    if not done:
        print("nothing to resolve")


if __name__ == "__main__":
    main()
