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

## Building on Windows: SDL3 on LIB and PATH

The `rust-dos` program links SDL3, and with MSVC the linker finds
`SDL3.lib` only through `LIB`. Without it, `cargo build`, `cargo test`
and `cargo clippy --all-targets` stop at the link step with
`LNK1181: cannot open input file 'SDL3.lib'`. That error is the
environment, not the code: don't change the code or the build to get
around it.

- Set up SDL3's Visual C++ package as the README's build section says,
  and add its `lib\x64` to `LIB` before building.
- A new shell, a git worktree, or a subagent's shell may not have the
  `LIB` you set elsewhere, so set it in the same command as the build,
  for example `LIB='C:\SDL3-3.4.10\lib\x64' cargo test` in Git Bash.
- The tests start the built `rust-dos.exe`, which needs `SDL3.dll` from
  the same folder on `PATH`.

## Checking a change: build what you touched, CI runs the rest

`tests/` holds seven test programs, one for each folder (`cpu`, `dos`,
`devices`, `video`, `win16`, `win32`, `program`), and each links the
whole crate. A file in a folder is a module of the folder's `main.rs`, so
its tests are named `<file>::<test>`. `cargo build --all-targets`,
`cargo clippy --all-targets` and a plain `cargo test` build and link all
seven. CI does that on every push, on Linux, Windows and macOS, so you
don't need to before you push.

- Build the library and the program with `cargo build`, and lint them
  with `cargo clippy`.
- Run the tests that cover what you changed: for a file in `tests/`,
  `cargo nextest run --test <folder> <file>::` (or
  `cargo test --test <folder> <file>::`), and `cargo test --lib <module>`
  for unit tests. When you change a test file, lint its folder with
  `cargo clippy --test <folder>`.
- A new test file goes in the folder of the code it tests, with a
  `mod <file>;` line in that folder's `main.rs`. Helpers that one folder
  uses live in it; helpers that several use live in `tests/common/` and
  each folder's `main.rs` declares them with `#[path]`.
- Push, and fix the branch if CI fails. Run the whole suite locally only
  when you can't use CI.

## A pull request's lifecycle: draft until it is proven better than main

A pull request opens as a draft and stays one until there is evidence
that it improves on main. Steps, in this order:

1. Open the pull request as a draft and implement the change.
2. Review it, with the fixes applied, by running this against it:

   ```
   /code-review high --fix Make judgement calls based on long-term quality and robust architecture rather than small scope or simplicity, do not postpone decisions to later unless there is no clearly better option (in which case create a GitHub issue to address this later), explain judgement calls made in comments to the reviewed PR. Commit and push review fixes to the PR branch. Treat claims in the PR description as unverified: check each one against the code, and report any the code doesn't support.
   ```

   Then check each finding against the code before keeping its fix: a
   review at this effort doesn't verify its own findings, and `--fix`
   applies them all. Confirm a finding with a test, a trace or a read of
   the code path. Revert the fix of one the code doesn't bear out, but
   only with evidence: the code path that rules it out, a test that
   would fail if the bug were real, or the specification or real
   Windows behaviour it contradicts. Disagreeing isn't evidence. List
   each rejected finding and its evidence in the comment that explains
   the review's judgement calls, so a later reader can disagree.

   A confirmed finding the pull request doesn't fix ends one of two
   ways. Either the comment explains why the code is right as it is,
   matching the real hardware or Windows, or the finding gets an issue
   that says what the real system does, what the emulator does, and why
   it waits, and the comment links it. "Left as is" or "out of scope"
   with neither is a decision put off with no record of it.
3. Validate it, after the review and with its fixes: run the real game
   or program the change is for, where there is one, on the build of the
   branch, and compare with main.
4. Mark it ready for review only when that shows a clear improvement on
   main: a speed-up you measured, against main on the same machine and
   scene; a bug that main has and the branch fixes, seen in the game; or
   behaviour closer to the real hardware or the real Windows, shown by a
   test or a comparison with it. Put the evidence in the description.

If the evidence isn't there yet, improve the change and go through the
steps again. A pull request stays a draft only when the next step needs
a decision with no clearly better option, or is far beyond the change's
scope; say which in a comment on it. When several agents work together,
the one coordinating them marks pull requests ready, not the agents
doing the work.

## Comparing what two builds draw: deterministic runs

A comparison of the pictures or the behaviour of two runs (main and a
branch, OpenGL and the software renderer, two settings) runs both with
`--deterministic`. Without it, the runs reach each scene at a different
emulated moment: input sent after waits on the wall clock, screenshots
taken on a wall-clock timer, and `cycles=auto` changing the speed with
the host's. Their pictures then differ whether the change drew anything
differently or not, and matching each shot with the closest one of the
other run hides real differences among those that timing makes.

- Start both runs with `--deterministic`, the same `--cycles` (a
  number), the same `--start-time`, and the same configuration apart
  from what is compared.
- Send every key, click and command while the machine is paused, at
  emulated times the script fixes: resume with `until_ms`, wait for the
  pause, send the input, resume. "Deterministic runs" in
  `docs/llm-debugging.md` has the requests.
- Take each screenshot while paused at a fixed emulated time, never
  after a wall-clock wait.
- Compare the frames of equal emulated times. Where the change should
  draw the same, they are identical: explain every pixel that differs,
  from the difference image, before calling the change exact.
- Two renderers may differ by rounding. Give the differing pixels and
  the largest channel difference for each frame, and compare main's pair
  at the same frames, which tells what the change added.

Two kinds of run can't be deterministic:

- Timing measurements measure how fast the host runs the emulator.
- Code that runs only outside deterministic mode can't be compared in
  it. With `--deterministic` the Voodoo cards draw and take their
  screenshots in software, and their 2D operations stay on the CPU,
  because the GPU's pixels depend on the host's GPU. A change to OpenGL
  drawing (`src/voodoo/gpu.rs`, `src/voodoo/gl`) is never run there.
  Check that the code compared runs (`/api/status` tells which renderer
  draws) before trusting identical pictures.

A comparison that can't be deterministic keeps as much of it as it can:
a fixed `--cycles` rather than `auto`, input sent at the same points of
the program, and many shots close together. Match each shot with the
other run's closest one, and log both emulated times. Say in the result
that the runs weren't in step, and explain each match that still
differs from the difference image: a different moment of the animation,
or a different picture.

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
- Every emulator run is heavy unless it is a timing measurement. The
  test is what the claim quotes: if it contains a number that comes from
  how fast the host ran the emulator, the run is a timing measurement,
  and only then does it need the quiet machine.
- Timing measurements, which run alone: a speed ratio or speed factor, a
  frame rate or frame time, a CPU busy or halted share, a benchmark score
  (3DMark, a bench loop, the Unreal or Giants matrix), or a time taken
  between two points in the emulated run. Check the running processes by
  command line at the start and at the end, and retake the run if another
  one started.
- Heavy runs, which run alongside builds: a cursor position, a window's
  or a saver's behaviour, a screenshot, an audio capture checked for
  correctness (levels, rail counts, clicks), a log or port-write count, a
  trace, a breakpoint, a reproduction boot, and a data unpack. Their
  results don't depend on the host's speed.
- A run that does both takes the timing measurement form only when the
  timing is part of the claim. A reproduction boot that also logs how
  long something took is still heavy, unless that time is quoted.
- Never use SDL's dummy audio driver (`SDL_AUDIODRIVER=dummy`) for a
  measurement of any kind. It doesn't keep real time and its rate changes
  from run to run, so the sound queue may run dry however well the host
  keeps up, or may not, and a count from it means nothing either way. A
  timing run or an underrun count uses a real device, muted
  (`SDL_AUDIODRIVER=wasapi` and `--mute` on Windows), or `--audio-output
  null` on a machine without one. Other runs with no window need nothing:
  `SDL_VIDEODRIVER=dummy` or `--hidden` takes the null output. Check
  `audio_driver` in `/api/underruns` or `/api/stats` before quoting a
  result.
- Run jobs under the machine-wide run lock with `rust-dos-lock`,
  installed once on the machine, outside every checkout: build it
  (`cargo build --release --bin rust-dos-lock`, which links no SDL) and
  copy the program to a folder on `PATH` such as `~/bin`. A copy inside
  a target directory can't wrap a build into that directory, since
  Windows can't overwrite a running program. Builds, tests and every
  heavy emulator run take the lock shared, so they run together:
  `rust-dos-lock shared --name NAME -- cargo test ...`. Only timing
  measurements take it exclusive:
  `rust-dos-lock exclusive --name NAME -- CMD...` keeps new shared jobs
  out from the moment it asks, waits for the running ones to end, and
  then runs alone. Either form lets go when the command ends, however it
  ends, and passes on its exit code. `--wait SECONDS` limits the wait
  (there is none by default); exit code 3 then means the command did not
  run. `rust-dos-lock status` lists the holders and the jobs waiting,
  with their names, states and debug servers.
- Every rust-dos run takes the lock shared by itself, a run started with
  `--run-lock-exclusive` takes it exclusive, and `--no-run-lock` takes
  none. A program inside a wrapper inherits its hold (through
  `RUST_DOS_RUN_LOCK_HELD` and `RUST_DOS_RUN_LOCK_DIR`): shared under
  either form and exclusive under exclusive take nothing, so a
  measurement script under `exclusive` may start emulators and wrappers.
  Exclusive under shared exits with 3 instead of waiting for itself. A
  job that loses its environment on the way, such as one started over
  ssh, deadlocks; its `run-lock: waiting for ...` line on stderr shows
  it.
- A script that starts an emulator asks for a free port with
  `--debug-server 127.0.0.1:0` and reads the address the emulator
  prints ("Free port" in `docs/llm-debugging.md`). Heavy runs share the
  lock and run side by side, so with a fixed port two agents running
  the same script reach each other's emulators. An emulator waiting for
  the lock hasn't opened its port yet either, and its script then talks
  to whichever emulator has that port, which can be another agent's
  measurement. Files clash the same way: send the emulator's stdout to
  a file of this run's own, not a fixed name in the script's folder,
  and give the run its own `--log FILE`. Otherwise two runs of the
  script read each other's port and replace each other's log.
- The lock lives in `run-lock` in the per-user profile directory
  (`%APPDATA%\rust-dos`, `~/.config/rust-dos`). `RUST_DOS_RUN_LOCK_DIR`
  moves it for every program that takes it; set it for the whole
  machine or not at all. A hold is an OS file lock, which ends with its
  process, so a killed job leaves nothing to clear. On Windows the
  wrapper runs the command in a job object, and killing the wrapper ends
  the command and its children; on Linux and macOS a process the command
  leaves in the background isn't held, so check by command line as
  above. The protocol is in `crates/runlock`'s module comment.
- `tools/run-lock.sh` hands over to an installed `rust-dos-lock`
  (`heavy` takes it shared, `measure` exclusive) and keeps its own
  `mkdir` lock only for a machine without one. The two locks don't see
  each other, so a machine uses one or the other. Read the script's
  header before using the fallback: its rules come from clashes that
  spoiled measurements.
- If CI's step times are enough for the claim, use them instead of a local
  timing.

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
