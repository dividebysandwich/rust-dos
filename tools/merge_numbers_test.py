"""Tests of merge_numbers.py, on small repositories made for each.

    python -I tools/merge_numbers_test.py

Each test makes a repository with a base commit, a "main" branch and a
"topic" branch that each take the next W32 version or the next place of
an append-only table, merges main into the topic branch, runs
merge_numbers.py and checks what it left.
"""

import os
import subprocess
import sys
import tempfile
import textwrap
import unittest

TOOLS = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(TOOLS, "merge_numbers.py")
LAYOUT = os.path.join(TOOLS, "win32_layout.py")

STATE = """\
const WIN32_VERSION: u16 = {version};
/// The W32 versions this rust-dos loads: its own, and those an older one
/// saved. 71's keeps less, and 71's to {last}'s no {missing}.
const WIN32_LOADS: [u16; {count}] = [
{loads}    WIN32_VERSION,
];

mod tests {{
    const WIN32_LAYOUTS: &[(u16, u64)] = &[
{layouts}    ];
}}
"""

THUNK = """\
/// The thens, by their place. Append only.
pub static THENS: &[Then] = &[
    then_keep,
    {proc},
{entries}];

// As in the real tables, the constants are a few lines below the
// entries, so git gives the two conflicts of their own.
fn before_a() {{}}
fn before_b() {{}}
fn before_c() {{}}

pub const THEN_PROC: u16 = 1;
{consts}
fn after() {{}}
"""


def state(version, missing, loads, layouts):
    return STATE.format(
        version=version, last=version - 1, missing=missing,
        count=len(loads) + 1,
        loads="".join(f"    {v},\n" for v in loads),
        layouts="".join(f"        ({v}, {h}), // {what}\n"
                        for v, h, what in layouts))


def thunk(entries, consts, proc="then_proc"):
    return THUNK.format(
        proc=proc, entries="".join(f"    {e},\n" for e in entries),
        consts="".join(f"pub const {name}: u16 = {n};\n"
                       for name, n in consts))


def fields(name, *names):
    return (f"crate::state_fields!({name} {{ {', '.join(names)} }});\n")


# The tests' git, and the script's, read no configuration but the
# repository's own: a conflict style, hooks or line endings set for the
# user would change what the merges leave.
ENV = {**os.environ, "GIT_CONFIG_GLOBAL": os.devnull,
       "GIT_CONFIG_NOSYSTEM": "1"}


class Repo:
    def __init__(self, root):
        self.root = root
        self.git("init", "-q", "-b", "topic")
        self.git("config", "user.name", "Test")
        self.git("config", "user.email", "test@example.com")
        self.git("config", "commit.gpgsign", "false")

    def git(self, *args, check=True):
        return subprocess.run(["git", "-C", self.root, *args], check=check,
                              capture_output=True, text=True, env=ENV)

    def write(self, path, text):
        full = os.path.join(self.root, path)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "w", encoding="utf-8", newline="") as f:
            f.write(text)

    def read(self, path):
        with open(os.path.join(self.root, path), encoding="utf-8") as f:
            return f.read()

    def commit(self, files, message):
        for path, text in files.items():
            self.write(path, text)
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message)

    def hash(self):
        """The layout hash win32_layout.py gives the working tree."""
        out = subprocess.run([sys.executable, "-I", LAYOUT, self.root],
                             capture_output=True, text=True)
        return out.stdout.split("\n")[0]

    def run_script(self):
        return subprocess.run([sys.executable, "-I", SCRIPT, self.root],
                              capture_output=True, text=True, env=ENV)


BASE_FILES = {
    "src/win16/base.rs": fields("Base", "a"),
    "src/win32/base.rs": fields("Base32", "b"),
    "src/win16/thunk.rs": thunk([], []),
}


class Test(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.repo = Repo(self.dir.name)

    def tearDown(self):
        self.dir.cleanup()

    def start(self, extra=(), main_entries=("main_then",)):
        """A base at version 2, then main at 3 with its own field and
        place 2 of THENS (and any other `main_entries` after it, with no
        constants). Returns main's hash of version 3."""
        repo = self.repo
        repo.write("src/bus/state.rs", "")
        for path, text in BASE_FILES.items():
            repo.write(path, text)
        h2 = repo.hash()
        repo.commit({"src/bus/state.rs": state(2, "base", [1], [
            (1, "0x0000_0000_0000_0001", "one"),
            (2, h2, "two"),
        ])}, "base")
        repo.git("branch", "main")
        repo.git("checkout", "-q", "main")
        repo.write("src/win32/main.rs", fields("Main", "m"))
        h3 = repo.hash()
        repo.commit({
            "src/bus/state.rs": state(3, "main's", [1, 2], [
                (1, "0x0000_0000_0000_0001", "one"),
                (2, h2, "two"),
                (3, h3, "main's"),
            ]),
            "src/win16/thunk.rs": thunk(list(main_entries),
                                        [("THEN_MAIN", 2)]),
            **dict(extra),
        }, "main")
        repo.git("checkout", "-q", "topic")
        return h2, h3

    def topic(self, h2, layouts, gates=True, proc="then_proc"):
        """The topic branch at version 3 with its own field, a gate on
        it and places 2 and 3 of THENS, its place 1 named `proc`."""
        repo = self.repo
        files = {
            "src/win16/topic.rs": fields("Topic", "t")
            + ("pub const TOPIC_SAVED: u16 = 3;\n"
               "type Saved = Since<3, Topic>;\n" if gates else ""),
        }
        for path, text in files.items():
            repo.write(path, text)
        h3 = repo.hash()
        files["src/bus/state.rs"] = state(3, "topic's", [1, 2], [
            (1, "0x0000_0000_0000_0001", "one"),
            (2, h2, "two"),
            *[(v, h3 if h is None else h, what) for v, h, what in layouts],
        ])
        files["src/win16/thunk.rs"] = thunk(
            ["topic_then", "topic_other"],
            [("THEN_TOPIC", 2), ("THEN_OTHER", 3)], proc=proc)
        repo.commit(files, "topic")

    def merge(self):
        out = self.repo.git("merge", "--no-edit", "main", check=False)
        self.assertIn("CONFLICT", out.stdout)

    def test_the_branch_takes_the_next_version_and_places(self):
        h2, h3 = self.start()
        self.topic(h2, [(3, None, "topic's")])
        self.merge()
        out = self.repo.run_script()
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)

        state_rs = self.repo.read("src/bus/state.rs")
        self.assertNotIn("<<<<<<<", state_rs)
        self.assertIn("const WIN32_VERSION: u16 = 4;", state_rs)
        self.assertIn("const WIN32_LOADS: [u16; 4] = [\n    1,\n    2,\n"
                      "    3,\n    WIN32_VERSION,\n];", state_rs)
        lines = [line.strip() for line in state_rs.split("\n")]
        at = lines.index(f"(2, {h2}), // two")
        self.assertEqual(lines[at + 1], f"(3, {h3}), // main's")
        self.assertEqual(lines[at + 2], f"(4, {self.repo.hash()}), // topic's")
        self.assertIn("71's to 2's no main's, and 71's to 3's no topic's.",
                      " ".join(line.removeprefix("/// ") for line in lines))
        check = subprocess.run([sys.executable, "-I", LAYOUT,
                                self.repo.root], capture_output=True)
        self.assertEqual(check.returncode, 0, check.stdout)

        topic = self.repo.read("src/win16/topic.rs")
        self.assertIn("pub const TOPIC_SAVED: u16 = 4;", topic)
        self.assertIn("Since<4, Topic>", topic)

        thunk_rs = self.repo.read("src/win16/thunk.rs")
        self.assertIn("    then_proc,\n    main_then,\n    topic_then,\n"
                      "    topic_other,\n];", thunk_rs)
        self.assertIn("pub const THEN_MAIN: u16 = 2;\n"
                      "pub const THEN_TOPIC: u16 = 3;\n"
                      "pub const THEN_OTHER: u16 = 4;\n", thunk_rs)

        # Everything resolved is staged, the moved gates of a file with
        # no conflict too, so the merge can be committed.
        status = self.repo.git("diff", "--name-only", "--diff-filter=U")
        self.assertEqual(status.stdout, "")
        unstaged = self.repo.git("diff", "--name-only")
        self.assertEqual(unstaged.stdout, "")

    def test_crlf_line_endings_are_kept(self):
        # A checkout on Windows may have CRLF endings.
        h2, h3 = self.start()
        self.topic(h2, [(3, None, "topic's")])
        self.merge()
        path = os.path.join(self.repo.root, "src", "bus", "state.rs")
        with open(path, encoding="utf-8", newline="") as f:
            text = f.read()
        with open(path, "w", encoding="utf-8", newline="") as f:
            f.write(text.replace("\n", "\r\n"))
        out = self.repo.run_script()
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        with open(path, encoding="utf-8", newline="") as f:
            text = f.read()
        self.assertNotIn("<<<<<<<", text)
        self.assertEqual(text.count("\n"), text.count("\r\n"))
        self.assertIn("const WIN32_LOADS: [u16; 4] = [\r\n    1,\r\n"
                      "    2,\r\n    3,\r\n    WIN32_VERSION,\r\n];", text)

    def test_places_move_past_main_entries_with_no_constant(self):
        # Main's second entry has no constant: the branch's still follow
        # both of main's, at 4 and 5.
        h2, _ = self.start(main_entries=("main_then", "main_bare"))
        self.topic(h2, [(3, None, "topic's")])
        self.merge()
        out = self.repo.run_script()
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        thunk_rs = self.repo.read("src/win16/thunk.rs")
        self.assertIn("    main_then,\n    main_bare,\n    topic_then,\n"
                      "    topic_other,\n];", thunk_rs)
        self.assertIn("pub const THEN_MAIN: u16 = 2;\n"
                      "pub const THEN_TOPIC: u16 = 4;\n"
                      "pub const THEN_OTHER: u16 = 5;\n", thunk_rs)

    def test_a_changed_entry_is_left(self):
        # The branch renamed an entry as well as appending: its side of
        # the conflict isn't only new entries, so the table is left.
        h2, _ = self.start()
        self.topic(h2, [(3, None, "topic's")], proc="then_renamed")
        self.merge()
        out = self.repo.run_script()
        self.assertEqual(out.returncode, 1, out.stdout + out.stderr)
        self.assertIn("still in conflict: src/win16/thunk.rs", out.stdout)
        thunk_rs = self.repo.read("src/win16/thunk.rs")
        self.assertIn("<<<<<<<", thunk_rs)
        self.assertIn("pub const THEN_TOPIC: u16 = 2;", thunk_rs)

    def test_a_conflict_with_no_markers_is_left_unstaged(self):
        # Git writes no markers into a binary file: staging it would take
        # the branch's side.
        h2, _ = self.start({"tests/data.bin": "\0main\0"})
        self.repo.commit({"tests/data.bin": "\0topic\0"}, "data")
        self.topic(h2, [(3, None, "topic's")])
        self.merge()
        out = self.repo.run_script()
        self.assertEqual(out.returncode, 1, out.stdout + out.stderr)
        self.assertIn("left unstaged", out.stdout)
        status = self.repo.git("diff", "--name-only", "--diff-filter=U")
        self.assertEqual(status.stdout, "tests/data.bin\n")

    def test_a_gate_main_has_too_is_left(self):
        # Main's own gate on its version 3 isn't among the branch's
        # added lines, so it stays at 3.
        h2, _ = self.start({"src/win32/gate.rs": "pub const MAIN_SAVED: u16"
                            " = 3;\n"})
        self.topic(h2, [(3, None, "topic's")])
        self.merge()
        out = self.repo.run_script()
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        self.assertIn("MAIN_SAVED: u16 = 3;",
                      self.repo.read("src/win32/gate.rs"))

    def test_two_versions_on_the_branch_change_nothing(self):
        h2, _ = self.start()
        self.topic(h2, [(3, "0x0000_0000_0000_0003", "topic's"),
                        (4, None, "topic's second")])
        self.merge()
        before = {p: self.repo.read(p) for p in
                  ("src/bus/state.rs", "src/win16/thunk.rs",
                   "src/win16/topic.rs")}
        out = self.repo.run_script()
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("Nothing changed", out.stdout + out.stderr)
        for path, text in before.items():
            self.assertEqual(self.repo.read(path), text, path)

    def test_other_conflicts_are_left(self):
        h2, _ = self.start({"src/win16/other.rs": "fn main_side() {}\n"})
        self.repo.commit({"src/win16/other.rs": "fn topic_side() {}\n"},
                         "other")
        self.topic(h2, [(3, None, "topic's")])
        self.merge()
        out = self.repo.run_script()
        self.assertEqual(out.returncode, 1)
        self.assertIn("still in conflict: src/win16/other.rs", out.stdout)
        self.assertIn("<<<<<<<", self.repo.read("src/win16/other.rs"))
        self.assertNotIn("<<<<<<<", self.repo.read("src/bus/state.rs"))

    def test_no_merge_is_refused(self):
        self.start()
        out = self.repo.run_script()
        self.assertNotEqual(out.returncode, 0)
        self.assertIn("no merge is in progress", out.stderr)


if __name__ == "__main__":
    unittest.main()
