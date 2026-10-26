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
  directory wait for each other ("Blocking waiting for file lock"), so
  concurrent builds need separate ones.
- Cargo starts as many jobs as there are processors, and every agent's
  build does. With more than two or three builds at once, pass `-j 2`
  or so. Otherwise the machine thrashes, and on Windows rustc and the
  linker can run out of memory (`os error 1455`, the paging file is too
  small).
- Stop the emulators you start, by their process ID, before you finish:
  one left running holds its files, its debug server's port and a
  processor.
- Don't take timings while other builds run. Measure on a quiet machine,
  or compare CI's step times instead.

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
