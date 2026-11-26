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

- Set up SDL3's Visual C++ package as `docs/building.md` says, and add
  its `lib\x64` to `LIB` before building.
- A new shell, a git worktree, or a subagent's shell may not have the
  `LIB` you set elsewhere, so set it in the same command as the build,
  for example `LIB='C:\SDL3-3.4.20\lib\x64' cargo test` in Git Bash.
- The tests start the built `rust-dos.exe`, which needs `SDL3.dll` from
  the same folder on `PATH`.

## Checking a change: build what you touched, CI runs the rest

`tests/` holds seven test programs, one for each folder (`cpu`, `dos`,
`devices`, `video`, `win16`, `win32`, `program`), and each links the
whole crate. A file in a folder is a module of the folder's `main.rs`, so
its tests are named `<file>::<test>`. `cargo build --all-targets`,
`cargo clippy --all-targets` and a plain `cargo test` build and link all
seven. CI does that on every push, and runs the whole suite on Linux
(Windows and macOS run a few tests of their own), so you don't need to
before you push.

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

   The review runs in a fresh agent, not in the context that wrote the
   change or in a fork of it: one that knows the author's reading of the
   bug inherits its blind spots. Give the reviewer the pull request and
   its worktree, not the author's explanation of the change.

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

   An issue filed this way, or from reading the specification or the
   code, is worked on when a game or program shows the problem: a wrong
   picture or sound, a crash or a hang, lost input, a feature it uses
   that fails, a cost measured in it. A program shows it when running
   that program brings the problem up: the one the work is for, another
   on the machine, or one an issue or a user's report names. A test, or
   a program written to reach the case, doesn't count. Until then the
   issue waits with the `low-priority` label, and isn't queued as a task
   of its own unless the user asks for that issue. Once a program shows
   it, the issue names that program and its label is raised to the
   priority the program's problem calls for.

   This rule decides which issues are taken up. A pull request already
   open, whatever started it, is marked ready on the evidence step 4
   asks for, and a test of the real system's behaviour is such
   evidence.
3. Validate it, after the review and with its fixes: run the real game
   or program the change is for, where there is one, on the build of the
   branch, and compare with main.
4. Mark it ready for review only when that shows a clear improvement on
   main: a speed-up you measured, against main on the same machine and
   scene; less work for the same output, counted in deterministic runs;
   a bug that main has and the branch fixes, seen in the game; or
   behaviour closer to the real hardware or the real Windows, shown by
   a test or a comparison with it. Put the evidence in the description.
   How much checking that takes depends on what the change can break
   ("Checks in proportion to the change", below).

   Before it is marked ready, check that it still merges with main,
   which moves on while a pull request is in draft. Fetch both and run
   `git merge-tree --write-tree origin/main origin/<branch>`: it exits
   with 1 and names the files when the two conflict. Don't go by
   GitHub's `mergeable`, which reads `UNKNOWN` while GitHub computes it
   and can describe the old head for minutes after a push. If it
   doesn't merge, the agent that has the branch's worktree merges main
   into the branch (no rebase, no force-push), keeps what both sides
   meant, and pushes.

   A resolution that keeps both sides' lines as they were, or moves a
   save-state version, is pushed without a local build: CI builds and
   tests it, and a build of the crate's tests takes many minutes. Lines
   that compile on each side can still clash once both are kept: two
   imports of one name, two items or match arms for the same thing, or
   one side's call to what the other renamed. Read the merged code
   around each conflict for these before pushing, and fix the branch if
   CI fails. Build and run the tests the conflict touches only where the
   resolution writes code that neither side had, such as a call changed
   to fit the other side's new signature.

   A save-state version both sides raised (`WIN32_VERSION` and the
   like): the branch takes the next free number, and main's joins the
   versions the branch loads (`WIN32_LOADS`, its comment, and a line in
   `WIN32_LAYOUTS`). Main's saves of that number don't hold what the
   branch added, so the branch's own gates that named it move to the new
   number too: a `Since<N, _>`, an `Until<N, _>`, a `*_SAVED` constant.
   `python -I tools/win32_layout.py` prints the new `WIN32_LAYOUTS`
   hash without a build, and exits with 1, saying what to change, while
   the table's last line isn't `WIN32_VERSION` with that hash.

If the evidence isn't there yet, improve the change and go through the
steps again. A pull request stays a draft only when the next step needs
a decision with no clearly better option, or is far beyond the change's
scope; say which in a comment on it. When several agents work together,
the one coordinating them marks pull requests ready, not the agents
doing the work.

## Checks in proportion to the change

Check a change as much as what it can break calls for, and no more.
Start from two questions: what could go wrong if the change is wrong,
and where would it show? The answers pick the checks; a habit of
running everything, many times, doesn't. A long check has a cost
beyond its own time: an exclusive measurement keeps every other
agent's new builds and emulators from starting while it waits and
while it runs.

- A change that removes work and keeps the output (a read-back, an
  upload or a copy no longer made, fewer calls or hand-offs) is shown
  by a deterministic comparison: identical frames at equal emulated
  times, and the counter of the work removed lower on the branch
  (`/api/stats`, or the card's and its renderer's counts under `video`
  in `/api/status`). That is the evidence it is better. The comparison
  has to run the changed code. `--deterministic` draws the cards in
  software, so a change to the ToadForce's OpenGL renderer is compared
  with `RUST_DOS_TOADFORCE_GL_DETERMINISTIC=1` and `renderer` checked
  ("The ToadForce card" in `docs/llm-debugging.md`), and one to the
  Voodoo cards' OpenGL path, which no deterministic run reaches, takes
  the form for runs that can't be deterministic (end of "Comparing
  what two builds draw", below). A timing run then only puts a number
  on the gain: one run per build on one scene that shows it is enough,
  main first and at a fixed `--cycles`, and the description calls the
  figure indicative. Later runs in a session tend to read slower
  whatever the build, so main first makes that drift count against
  the branch.
- A timing series (several runs per build, the order alternated, the
  spread given) is for claims whose truth is the timing itself: a
  change that does the same work faster with no counter to show it, a
  change that trades one cost for another, a gain close to the spread
  of the runs, a change to how `cycles=auto` picks its speed, or a
  figure the description quotes as a size. Keep even a series short: a
  fixed window of a minute or so after a fixed warm-up, the builds and
  states prepared under the shared lock first, and the exclusive lock
  asked for only when no emulator holds the lock, with `--wait` so a
  long holder makes the run give up and retry instead of stalling
  everyone behind it. A waiting exclusive request already keeps new
  shared jobs out, so without `--wait` a measurement queued behind a
  long run holds up every build and test that comes after it.
- A save state made by main can carry main's state of the thing the
  change moves into the branch's run: a texture's placement in the
  card's memory, for one. Compare from states each build saved itself,
  or from a plan that replays from power-on.
- Other programs are run when the change is in a path they share with
  the program the work is for. A change one program reaches needs that
  program's check and the tests. A change to shared code (GDI's palette
  and bitmaps, DirectDraw's blits, the CPU, a renderer's common path)
  needs every test file that covers it run locally, and deterministic
  runs of a few other programs that use the path. Only CI's Linux jobs
  run the whole suite: the Windows and macOS jobs run the recompiler's
  and the deterministic mode's tests (and Windows a start of the debug
  build) and none of `tests/win32`, so their passing says nothing about
  it, and a pull request can be marked ready before the Linux jobs end
  (#2024 merged with them red).
- A run of another program counts only if it reaches the changed code:
  check the counter or the log that shows it did. Of five games run for
  a change to the ToadForce's depth fills (#2066), one reached it; the
  others showed only that nothing else moved. When one of them reaches
  the code and draws the same, stop: searching every program for more
  scenes that reach it adds little.
- A question of what the real system does is answered by the real
  system: a small probe program run on real Windows or on an emulated
  real PC (86Box) settles it sooner and more surely than reasoning
  from documentation, as one did for #2046 in #2076. Its source and
  output go in the pull request.
- When the machine is busy (the user says so, or `rust-dos-lock status`
  shows others' emulators running for long), take no timing
  measurements. The deterministic evidence above doesn't depend on the
  host's speed and goes on. For a change that removes work it is
  enough for step 4, and the indicative figure joins the description
  once the machine is quiet; a change whose claim is the timing itself
  stays a draft until its series is run.

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
  build does. Share the processors among the builds that run at once.
  `rust-dos-lock status` (below) lists every job under the lock: count
  the builds and tests among them by their names, and your own. A build
  takes no `-j` only when no other agent works on the machine, since it
  keeps the jobs it started with when other builds start after it.
  Otherwise each takes about the processors divided by the number of
  builds, fewer while emulators run, and `-j 2` once the builds' memory
  nears what the machine has free, since rustc's grows with its jobs.
  Past these limits the machine thrashes, and on Windows rustc and the
  linker can run out of memory (`os error 1455`, the paging file is too
  small). A fixed `-j 2` for every build leaves a build that runs alone
  with two of the processors for the rust-dos crate, which is most of
  what a build in a reused target directory compiles.
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
- Never use SDL's dummy audio driver (`SDL_AUDIO_DRIVER=dummy`) for a
  measurement of any kind. It doesn't keep real time and its rate changes
  from run to run, so the sound queue may run dry however well the host
  keeps up, or may not, and a count from it means nothing either way. A
  timing run or an underrun count uses a real device, muted
  (`SDL_AUDIO_DRIVER=wasapi` and `--mute` on Windows), or `--audio-output
  null` on a machine without one. Other runs with no window need nothing:
  `SDL_VIDEO_DRIVER=dummy` or `--hidden` takes the null output. Check
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
