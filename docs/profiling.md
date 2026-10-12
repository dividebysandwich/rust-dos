# Profiling

The profiler answers "where does the emulator's time go while this game
runs?" It samples which part of the emulator the emulation thread is in
(a zone: the interpreter, translated code, a device's ports, drawing the
picture, mixing sound, ...) together with the guest code it is running
(CS:EIP and the CPU mode), and it counts the events that explain a hot
spot: port accesses, BIOS and DOS calls, interrupts, exceptions, accesses
to memory-mapped devices, and the recompiler's translations.

A profile runs only when asked for. Without one the emulator runs the same
machine code as a build without the profiler (see [Cost](#cost)).

## Starting a profile

From the debug server (`--debug-server`, see
[llm-debugging.md](llm-debugging.md)):

```sh
curl -X POST localhost:8086/api/profile -H 'content-type: application/json' \
     -d '{"enabled":true,"hz":2000,"duration_ms":30000}'
curl 'localhost:8086/api/profile?wait_ms=40000&format=text'
```

`POST /api/profile` takes:

| Field | |
|---|---|
| `enabled` | `true` starts a profile (over again, if one runs), `false` stops it |
| `hz` | Samples a second, 10 to 20000 (default 2000) |
| `timer` | `sleep` (default) or `spin`: how the sampler waits between samples. `spin` busy-waits and takes a host core, for rates the host's sleep can't keep |
| `duration_ms` | Stop after this long. Without it the profile runs until stopped |

The answer, and `GET /api/profile` while it runs, have `running`, `samples`
and `wall_ms`. A finished profile stays readable until the next one
starts.

From the command line, with no debug server:

```sh
rust-dos --profile run.json -d ~/dos/game
```

profiles from startup until the emulator exits, then writes `run.json`
(the report), `run.txt` (the table) and `run.folded` (flame graph stacks).
`--profile-hz` sets the rate.

## Reading the report

`GET /api/profile` answers with JSON; `?format=text` gives a table of the
same, and `?format=folded` the stacks for a flame graph.

| Query | |
|---|---|
| `format` | `json` (default), `text` or `folded` |
| `top` | How many places, ports and services to list (default 25) |
| `zone` | Only the samples in this zone (`io.vga`) or in this group of zones (`io`, `hle`, `dynrec`), for the places and the folded stacks |
| `bucket` | Count guest code by blocks of this many bytes (a power of two), each named by its hottest instruction |
| `disasm` | Lines of disassembly for each place (default 4, 0 for none) |
| `timeline` | `true` adds the zones of each second |
| `wait_ms` | Wait up to this long for a profile with a duration to end |

The JSON's sections:

- `sampler`: the rate asked for and the rate reached (`achieved_hz`), and
  the spacing of the samples (`interval_us`: mean, min, p50, p90, p99,
  max).
- `window`: what the machine did meanwhile: wall and emulated time,
  instructions and MIPS, how much ran as translated code, how much of the
  emulated time the CPU was halted, frames drawn, the decode cache's hit
  rate, and the `activity` counters (`/api/status`) for the window.
- `busy`: the samples outside `frame.wait`. At real speed the emulator
  waits for the next frame most of the time, so every zone's share of all
  samples is small. Each share also comes as `busy_pct`, its share of the
  busy samples: of the time the emulator works, where it goes.
- `engines`: the share of samples in the interpreter, in translated code,
  and in the front end.
- `groups` and `zones`: the samples by zone (below), most first.
- `top`: the guest places with the most samples: the address (`CS:IP`, or
  `CS:EIP` in 32-bit code), the CPU mode (`real`, `v86`, `pm16`, `pm32`),
  the engine, the shares, the zones the samples there were in (the ports
  it touched, the services it called), its linear address in real and
  virtual-8086 mode, and a few lines of disassembly from the memory as it
  is now.
- `segments`: the samples by code segment.
- `ports`: port accesses by device and by port, with rates a second. A
  word or doubleword access to byte ports counts once.
- `hle`: calls of the emulator's BIOS and DOS functions by interrupt and
  function (AH, or AL for INT 33h), with names for the common ones, and
  the emulator's other services (the DPMI host's entries, inline and
  far-call traps).
- `irqs`, `exceptions`: hardware interrupts delivered by IRQ line, and
  exceptions by vector.
- `mapped_memory`: reads and writes of memory that isn't plain RAM:
  `video` (A0000h-BFFFFh), `upper` (C0000h-FFFFFh: the ROMs, the EMS page
  frame, upper memory blocks) and `device` (above RAM: linear frame
  buffers, the 3D cards). A word access made of byte accesses counts once.
- `dynrec`: the recompiler in the window: blocks translated, thrown away
  as stale or self-modified, flushes, block exits by reason, the
  instructions it handed to the interpreter (`calls.fallback_top`, by
  mnemonic) and its slow paths, and the pages with the most translations
  and invalidations.

### Zones

| Group | Zones |
|---|---|
| `frame` | `frame.wait` (waiting for the next frame), `frame.events` (the host's events), `frame.compose`, `frame.present` |
| `vga` | `vga.render`: the picture from video memory |
| `frontend`, `audio`, `rewind`, `debug`, `ui` | `frontend` (the rest of the front end's frame), `audio.pump` (sound to the host), `audio.synth` (FM, sample and wavetable synthesis as the machine runs), `rewind`, `debug` (the debug server's requests), `ui` (menus and overlays) |
| `cpu` | `cpu.interp`, `cpu.jit` (translated code) |
| `dynrec` | `dynrec.dispatch` (between translated blocks), `dynrec.translate`, `dynrec.fallback` (an instruction translated code hands to the interpreter), `dynrec.slow_mem` (translated code's memory accesses that leave plain RAM) |
| `timers`, `irq`, `shell`, `idle` | Timer events, delivering hardware interrupts, the built-in shell, `idle.proof` (proving a loop only waits) |
| `mem` | `mem.video`, `mem.upper`, `mem.device` |
| `sb`, `gus`, `voodoo`, `powervr` | `sb.dma` (the Sound Blaster's DMA), `gus.advance` (the Ultrasound), `voodoo` and `voodoo.frame` (the 3dfx card's work on the emulation thread, and its frame), `powervr` |
| `hle` | `hle.int08` ... `hle.int67`, `hle.other`, `hle.inline`, `hle.far`, `hle.dpmi` |
| `io` | One zone per device: `io.vga`, `io.pic`, `io.pit`, `io.sb`, `io.opl`, `io.ide`, `io.s3`, ... and `io.unhandled` |

A zone inside another counts only its own time: a port write from
translated code is `io.vga`, and the guest place is the instruction that
made it.

### Flame graphs

`?format=folded` gives a line per stack, root first, then the count:

```
frame.wait 29914
cpu.interp;pm32 0008:00002C84;hle.dpmi 608
cpu.jit;pm32 01FF:00232432;dynrec.fallback 371
```

Feed it to [inferno](https://github.com/jonhoo/inferno)
(`inferno-flamegraph < run.folded > run.svg`) or open it in
[speedscope](https://www.speedscope.app/). Add `zone=cpu` (or
`zone=dynrec`, ...) to leave the waiting and the front end out.

## Comparing builds

1. Load a save state at the scene to measure, so each build starts from
   the same place.
2. `POST /api/profile {"enabled":true,"duration_ms":30000}`.
3. Send the same input (`POST /api/input/batch`).
4. `GET /api/profile?wait_ms=40000` once, and keep the JSON.
5. Do the same with the other build and compare `busy`, the zones'
   `busy_pct`, `window.mips` and the counts. The `build` section says
   which build made each.

The counts are exact, but the input's timing depends on the host, so
compare shares and rates rather than single samples. A difference of a
few tenths of a percent in a zone with few samples is noise.

## The sampler's timing

The sampler waits a random 0.5 to 1.5 times the interval between samples,
so its samples don't line up with the emulator's frames. With `timer=sleep`
it asks the host to sleep for less than the wait by the amount its last
sleeps overran. On Windows 11 at 2000 Hz it reaches about 1980 Hz, with
waits of 525 µs at the median and about 1 ms at p99. The report's
`sampler` section gives both for each profile.

## Cost

Without a profile the emulator runs the same machine code as one built
without the profiler: the interpreter's loop has a copy for profiling that
it picks once per batch, translated blocks note themselves only when they
were translated during a profile (starting and stopping one throws the
translations away), and the rest of the marks are a load of a flag and a
branch. `benches/cpu.rs` measures a guest loop on both engines; compared
with a build without the profiler, its times are within noise.

While a profile runs, the interpreter stores the place of each instruction
and translated code stores it at each block's start. In `benches/cpu.rs`,
a loop of blocks of a few instructions, this costs 13% on the interpreter
and 17% on the recompiler. In games most of the host's time is elsewhere
(drawing, sound, waiting) and the cost is lower; compare `window.mips`
with the debug server's `/api/stats` from before the profile to see it.

## How it works

The emulation thread keeps one 64-bit word up to date, `profile::WORD`:

| Bits | |
|---|---|
| 0-31 | EIP |
| 32-47 | CS (the selector in protected mode) |
| 48-54 | The zone |
| 55 | The word holds guest code |
| 56-57 | The engine: front end, interpreter, translated code |
| 58-59 | The CPU mode: real, virtual-8086, 16-bit or 32-bit protected |

Every update is a relaxed store: no clock read, lock or allocation. A zone
is a guard (`profile::zone`) that puts the word back as it was when it
ends, so zones nest. The sampler thread reads the word and counts each
value in a hash map of its own. The bus counts events in fixed arrays it
gets when the profile starts (`profile::Counters`), with plain increments.

Code: `src/profile` (the word, zones, counters, sampler, session and
reports), `src/bus/profile.rs` (which device has each port),
`src/debug/profile.rs` (the endpoint's report).

### Adding a zone

Add it to the `zones!` list in `src/profile/mod.rs` (its name's part before
the first dot is its group), then mark the code:

```rust
let _zone = profile::zone(profile::Zone::MyDevice);
```

In a hot loop, keep the guard out of the copy that runs without a profile,
as `exec::run` does with its `PROF` parameter: a guard's drop code alone
changes how the compiler lays out a loop.
