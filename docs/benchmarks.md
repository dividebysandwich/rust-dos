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

`cargo test --benches` runs each benchmark once, in the test profile's
build (opt-level 1, with debug assertions), as a quick check that it
still works.

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
| `benches/video.rs` | VGA planar sprites and scrolling, a DOS mode 12h row-stress pattern and box animation, CGA composite and Voodoo triangles: identical generic/variant pipelines and 64-triangle frames with 0 or 4 workers in software or GL-backlog mode |
| `benches/xbrz.rs` | Native xBRZ preparation, isolated from unrelated video fixture setup |
| `benches/memory.rs` | XMS block moves (function 0Bh) between extended and conventional memory and between blocks, and an EMS move (function 57h) into conventional memory |
| `benches/cpu.rs` | A guest loop of arithmetic, memory accesses, calls and port writes on the interpreter and on the recompiler, each also with a profile running (`docs/profiling.md`) |
| `benches/host.rs` | Host-side work on the emulation thread: capturing a frame of a GIF animation, a 256-colour 640x400 picture and a high-colour 640x480 one |

## Mode 13h scenario

`video/graphics_256_sprite/13h` measures a mode 13h screen with a patterned
background and a changing 32x32 region. Each iteration writes the region to
guest video memory and runs the normal renderer into the doubled 640x400
frame.

```sh
cargo bench --bench video -- video/graphics_256_sprite/13h
```

## Voodoo scenarios

```sh
cargo bench --no-default-features --bench video -- video/voodoo
```

`video/voodoo/pipeline` draws the same bilinear, two-unit textured
triangle with Oni's colour path through the generic pipeline and its
matching specialised variant. Setup verifies that both write pixels,
produce the same counters and leave the same frame buffer. Each timed
iteration draws one triangle as 240 scanlines; texture and buffer
construction are outside the measurement. The frame cases below also
include edge walking and register setup.

`video/voodoo/frame` submits a full-screen clear and 64 untextured
triangles through the card's register interface. The software cases wait
for completion each frame. The GL-backlog cases consume the mirror's
commands and leave software jobs deferred, so later clears can prune
them. Both use persistent cards and either 0 or 4 persistent workers.
The final software catch-up and worker teardown are outside the timing.
These cases measure the CPU cost accompanying GL replay, including
recording and pruning; they need no GL context and do not measure host
GPU rendering, upload or readback. They do not replace a game benchmark.

## xBRZ preparation and rendering

The CPU cases measure `XbrzFrame::prepare_rows` with persistent buffers at
320x200 (a doubled VGA picture) and 640x480. They cover an unchanged
picture, one pixel, a sprite, distant edits, one changed native row, a full
change, and coverage changes in an overlay whose colour stays the same.
Sparse and full-screen overlays also cover stable content, pixel changes and
moving coverage. Producer bands cover every changed base and overlay row.
Input construction is outside the timing. These cases do not measure
rendering or texture uploads. Coverage is cached separately from alpha,
using one byte per screen pixel after an overlay first appears; buffers are
reused until the screen dimensions change.

```sh
cargo bench --locked --bench xbrz -- --save-baseline xbrz
```

The separate browser benchmark uses the real GLSL from
`src/video/shader/`, with no emulator or game files. It needs Node 22 or
later for its built-in WebSocket client, and Chrome on `PATH` (or set
`CHROME_BIN`). From the repository root:

```sh
node web/bench/run-xbrz.mjs /tmp/xbrz-webgl.json
```

It runs Chrome headlessly through a temporary localhost server, checks
experimental shaders against the current shader byte for byte, and
measures persistent RGBA8 framebuffer targets. Cases include palette
patterns and noise, minified, native, exact 2x and larger output, narrow
images, transparency, near colours, and source changes between draws.
Three rounds rotate the variant order; each has three warm-up draws and
six measured draws per variant. Set `XBRZ_SAMPLES` and `XBRZ_ROUNDS` to
change the sample counts.

The experiments isolate a corner-classification pass, calculation of
only the selected block pixel, a fixed 2x intermediate, their
combinations, and reuse of unchanged intermediates. The former
four-pixel calculation remains behind `XBRZ_REFERENCE` in the shared
GLSL. `production_fresh` runs the real page renderer, including its
source upload and cache invalidation; `production_reuse` retains the
unchanged base. Both must match the reference and issue the expected
number of passes. No texture storage may be allocated during timing.

Set `XBRZ_INCREMENTAL=1` for pixel, 32x16 sprite, distant 16x8 edits
and full-frame updates. `production_partial` includes bounded source
uploads and dependent cache updates; `production_fresh` forces a full
upload and rebuild, and `production_reuse` measures an unchanged source.
The pixels change during reference validation; timing forces the same
candidate regions without measuring fixture generation. To compare a
saved renderer with identical fixtures, set `XBRZ_RENDERER` to its full
path. The report hashes the renderer actually served.

The production renderer caches corner flags until source pixels or tint
change. Consecutive revisions carry at most 16 source rectangles;
neighboring kernels receive a two-pixel halo. Classify all affected corners
before resolving affected 2x pixels. Each cache accumulates its own damage
until used, including across native-sized and minified output. Missed
revisions or geometry changes rebuild the full cache. Output at least 2x in
both axes uses a reusable 2x base; smaller output resolves directly.
Minification skips the source-sized prepass. Targets must fit both the
texture and viewport limits; unsupported sizes retain the direct shader. The
corner texture includes a top and left halo to preserve the boundary rules.
Shader captures use the same pipeline. Renderer marker changes require
updating the benchmark's extraction of `makeGlScreen`.

When GPU timer queries are available, `gpuMs` contains their elapsed
time. Disjoint samples are null and excluded from the printed medians.
`wallMs` then measures CPU submission and is not rendering completion.
Without timer queries, the wall measurement includes a synchronous
one-pixel readback to wait for completion. These two metrics are not
interchangeable. Queries follow the
[Khronos timer extension](https://registry.khronos.org/webgl/extensions/EXT_disjoint_timer_query_webgl2/).

Allocation, compilation, normal readback, overlays, the emulator,
compositing and vsync are outside the steady-state draw timings.
`upload_nearest` also includes a full source upload followed by a nearest
draw, so its transfer is consumed before the query ends. Reuse cases
assume the base has not changed. The JSON retains individual samples,
source hashes, renderer, browser, command flags and logical intermediate
storage. Shader selection time and the first production frame per
workload are recorded separately; the latter can include target
allocation and uses shaders already warmed by the edge checks. In
incremental mode the first partial validation may already have allocated
the targets before the first forced full refresh is recorded.

To measure the software fallback explicitly, add `--swiftshader`.
`--no-sandbox` is available for isolated environments that require it.
Software-renderer timings do not predict integrated-GPU frame times.
Repeat on target hardware before making hardware frame-rate claims.
These benchmarks set no wall-time CI gate.

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
