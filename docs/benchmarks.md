# Benchmarks

`benches/` holds microbenchmarks of the emulator's hot paths, run by
[Criterion](https://github.com/criterion-rs/criterion.rs). They build the
scenarios they measure at start-up (a game folder of a few hundred files,
for example) in the target directory, so they need no game files and run
the same on Windows, Linux and macOS.

```sh
cargo bench                          # every benchmark
cargo bench --bench dos              # one subsystem
cargo bench --bench dos -- find      # the benchmarks whose name contains "find"
```

Criterion prints its estimate of the time per iteration (the mean, or the
slope of a linear fit when it times growing batches) with a confidence
interval, and the change from the previous run on the same machine.
Results go to `target/criterion/`.

## Before and after a change

Save a baseline on the unchanged code, then compare the changed code with
it:

```sh
git switch main
cargo bench --bench dos -- --save-baseline main
git switch my-branch
cargo bench --bench dos -- --baseline main
```

Criterion reports each benchmark as improved, regressed or within noise.
Close other programs while it runs, and run it twice when a result is
borderline: a laptop's power management, a virus scanner or a build in
another terminal moves file system numbers by 10% or more.

`cargo test --benches` runs each benchmark once, in a debug build, as a
quick check that it still works.

## In CI

The regular CI doesn't build or run the benchmarks. The Benchmarks
workflow (`.github/workflows/benchmarks.yml`) runs them on demand, on a
Linux runner:

- On a PR with the `performance` label. Adding the label starts it, and
  every push to the PR runs it again while the label is on. It benchmarks
  the base commit, saves that as a baseline, benchmarks the PR merged
  into the base, and puts a table of both times and the change in the
  run's summary. When the base has no benchmarks it shows the PR's times
  only.
- By hand, with "Run workflow" on the Actions tab: it benchmarks the
  chosen branch and puts the times in the summary.

Shared runners are noisy: treat a difference of a few percent as noise,
and confirm a large one on a quiet machine before relying on it.

## What is there

| File | Measures |
|---|---|
| `benches/dos.rs` | The built-in DOS's file calls on a host folder: DIR's listing, FindFirst/FindNext over a whole folder, a pattern search, and opening a file three folders deep |
| `benches/audio.rs` | One video frame of sound: the AWE32 playing 32 voices with the filter open, fixed, swept by the modulation envelope and wobbled by LFO1 |
| `benches/video.rs` | The VGA renderer: a sprite moving over modes 0Dh and 12h, a full-screen scroll in mode 12h, and a DOS program animating a box in mode 12h run frame by frame through the emulator |
| `benches/memory.rs` | XMS block moves (function 0Bh) between extended and conventional memory and between blocks, and an EMS move (function 57h) into conventional memory |
| `benches/host.rs` | Host-side work on the emulation thread: capturing a frame of a GIF animation, a 256-colour 640x400 picture and a high-colour 640x480 one |

## Adding benchmarks

One file per subsystem (`benches/video.rs`, `benches/audio.rs`, ...), each
registered in `Cargo.toml`:

```toml
[[bench]]
name = "video"
harness = false
```

Shared helpers are in `benches/common/mod.rs` (scratch folders, a machine
with C: mounted, INT 21h calls); a bench file takes them with `mod common;`.
Name benchmarks `subsystem/scenario[/size]` so a filter on the subsystem
picks them all.

Benchmarks see the `rust-dos` library's public API only, as the tests in
`tests/` do. Most modules and their types are public; for something
private, prefer a small public entry point that a test could use as well
over a benchmark-only feature.

Build what the scenario needs outside `b.iter`. Criterion takes 100
samples unless told otherwise, and never fewer than ten samples of one
iteration each, so give a benchmark that takes over about 50 ms an
iteration `group.sample_size(10)`; one of a second an iteration still
runs for about half a minute (`dos/find_first_next/218` on main). Fixtures
are generated at run time; commit a file only when it can't be generated,
and record its source and licence in a README beside it.
