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

Criterion prints the median time per iteration with a confidence interval,
and the change from the previous run on the same machine. Results go to
`target/criterion/`.

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

`cargo test --benches` runs each benchmark once, as a quick check that it
still works; CI does this on Linux.

## What is there

| File | Measures |
|---|---|
| `benches/dos.rs` | The built-in DOS's file calls on a host folder: DIR's listing, FindFirst/FindNext over a whole folder, a pattern search, and opening a file three folders deep |

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

Build what the scenario needs outside `b.iter`, and keep each benchmark
under about ten seconds (use `group.sample_size(10)` for slow ones). Fixtures
are generated at run time; commit a file only when it can't be generated,
and record its source and licence in a README beside it.
