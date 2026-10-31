# Notes for coding agents

How code and documentation are laid out in this repository, so that
changes don't need reformatting after they are merged. They follow what
the history does; where it disagrees with a tool's or an agent's own
defaults, these win.

## Prose: hard-wrapped at about 76 columns

Markdown files (`README.md`, `CONFIGURATION.md`, `docs/`) and comments
in Rust code (`//`, `///`, `//!`) are wrapped by hand. A line ends
before column 77 or so, counting indentation, and never goes past 80.

- Wrap what you write the same way. Don't write a paragraph as one long
  line, even if your own instructions say not to hard-wrap Markdown.
- When you add or change a sentence in a paragraph, rewrap the whole
  paragraph, so no line runs long and none is left short in the middle.
  Leave paragraphs you didn't touch as they are.
- Tables, code blocks, headings, URLs and other things that can't be
  broken stay on one line.

## Rust: rustfmt's default style for new code, no reformatting

There is no `rustfmt.toml`: code is formatted with rustfmt's defaults
(edition 2024, 100 columns). The tree as a whole isn't formatted, so:

- Format a new file with plain `rustfmt path/to/file.rs`.
- In an existing file, write the code you add or change in rustfmt's
  default style, and leave the lines around it as they are.
- Don't run `cargo fmt` over the crate or `rustfmt` over a file you
  only touched, and don't add a rustfmt configuration: either rewrites
  hundreds of lines nobody changed.
- Don't widen lines past 100 columns to match older code nearby that is
  wider.

Run `cargo clippy` on what you change and fix the warnings it gives for
your code.

## Building on Windows: SDL2 on LIB and PATH

The `rust-dos` program links SDL2, and with MSVC the linker finds
`SDL2.lib` only through `LIB`. Without it, `cargo build`, `cargo test`
and `cargo clippy --all-targets` stop at the link step with
`LNK1181: cannot open input file 'SDL2.lib'`. That error is the
environment, not the code: don't change the code or the build to get
around it.

- Set up SDL2's Visual C++ package as the README's build section says,
  and add its `lib\x64` to `LIB` before building.
- A new shell, a git worktree, or a subagent's shell may not have the
  `LIB` you set elsewhere, so set it in the same command as the build,
  for example `LIB='C:\SDL2-2.30.0\lib\x64' cargo test` in Git Bash.
- The tests start the built `rust-dos.exe`, which needs `SDL2.dll` from
  the same folder on `PATH`.

## Checking a change: build what you touched, CI runs the rest

`tests/` holds about 128 test programs, and each links the whole crate.
`cargo build --all-targets`, `cargo clippy --all-targets` and a plain
`cargo test` build and link every one of them. CI does that on every
push, on Linux, Windows and macOS, so you don't need to before you push.

- Build the library and the program with `cargo build`, and lint them
  with `cargo clippy`.
- Run the tests that cover what you changed: `cargo test --test <name>`
  for the files in `tests/`, and `cargo test --lib <module>` for unit
  tests. When you change a test file, lint it with
  `cargo clippy --test <name>`.
- Push, and fix the branch if CI fails. Run the whole suite locally only
  when you can't use CI.

## Several agents on one machine

Agents often work on several branches at once, each in its own git
worktree. Their builds compete for the same processors, memory and
disk.

- Give each worktree its own target directory outside the repository
  (`CARGO_TARGET_DIR`), on a drive with room. Builds that share a target
  directory wait for each other ("Blocking waiting for file lock"), and
  each one overwrites the binaries the one before it built, so
  concurrent builds need separate ones.
- Reuse the target directory of a finished task instead of starting an
  empty one. It keeps the compiled dependencies, so a build of another
  worktree compiles only the rust-dos crates again. A compiler cache
  such as sccache doesn't do this across directories: its cache keys
  include the target directory's path, so a new directory gets no hits.
- Cargo starts as many jobs as there are processors, and every agent's
  build does. With more than two or three builds at once, pass `-j 2`
  or so. Otherwise the machine thrashes, and on Windows rustc and the
  linker can run out of memory (`os error 1455`, the paging file is too
  small).
- Stop the emulators you start, by their process ID, before you finish:
  one left running holds its files, its debug server's port and a
  processor.
- A timing measurement is one whose number is quoted as evidence: a
  speed ratio, a CPU or halted share, a frame time, or a benchmark score.
  It runs alone. No build, test or other emulator may run during it, and
  the run is retaken if one starts. Check the running processes by command
  line at the start and at the end of each run.
- Everything else runs alongside builds: a cursor position, a window's
  behaviour, a screenshot, an audio capture checked for correctness, a log
  count, a trace, or a reproduction boot. Its result doesn't depend on the
  host's speed, so it can run during a build. Only a number you will quote
  as evidence needs the quiet machine.
- Run jobs through `tools/run-lock.sh` (bash on Linux and macOS, Git
  Bash on Windows). Builds, tests and every emulator run that isn't a
  timing measurement take the heavy form, so they run together:
  `bash tools/run-lock.sh heavy NAME 0 -- cargo test ...`. Only timing
  measurements take the exclusive form:
  `bash tools/run-lock.sh measure NAME 0 -- CMD...` blocks new heavy jobs,
  waits for the running ones to end, and then runs alone, in the order
  measurements asked. Either form releases when the command ends and
  passes on its exit code. The third argument is the longest wait in
  seconds; `0` waits without limit, which is the right value for queued
  work. If the script exits with code 3, the command did not run: check
  `status`, and if the job is still wanted, run the same command again at
  once under the same NAME. Never call one form inside the other: that
  deadlocks.
  `status` shows what runs and waits, and `clear NAME` cleans up after a
  stopped worker. NAME is yours alone. The lock lives in
  `RUST_DOS_RUN_LOCK_DIR` (by default `~/.rust-dos-run-lock`), so all
  callers on the machine need the same one. Read the script's header
  before using it: its rules come from clashes that spoiled
  measurements.

## Behaviour: closer to the real machine beats unchanged results

A fix that makes the emulator behave more like the real hardware or the
real Windows is welcome even when it changes recorded results: the game
suite's golden hashes, test assertions that captured the old behaviour,
and save states or recorded input that no longer play out the same way.
Don't hold such a fix back, or bend it, to keep them as they were.

- Update what it changes in the same pull request: re-bless the goldens
  after looking at the new shots (see `docs/game-suite.md`), and change
  the assertions that encoded the old behaviour, saying in the commit
  what the real system does.
- Say in the pull request which recorded results changed and why the
  new ones are right.
- Reproducibility still has to hold. The determinism tests
  (`src/debug/determinism_tests.rs`) run one build twice with different
  host timing and compare the two runs; they don't compare against an
  older build. A fix that makes them fail has made the emulator depend
  on the host's timing, and that is a bug in the fix.
