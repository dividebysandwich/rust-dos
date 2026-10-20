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
- `callers`: ring-0 and 16-bit samples charged to the code that made them
  run (see [Work Windows does for the game](#work-windows-does-for-the-game)):
  their share of busy time, ring 0's share (`ring0_busy_pct`) and how much
  of it has an entry (`ring0_charged_pct`), and in `top` each chain: the
  32-bit callers root first (`win32`, empty when there are none), the
  entry to ring 0 (`entry`: its place, mode and `via`, such as `int 30h`,
  `#GP`, `irq int 50h`, `call gate` or `sysenter`; null for 16-bit code
  that didn't enter ring 0), the samples, and the places they were at.
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

  A profile starts with no translated code (see [Cost](#cost)), so the
  program's code is translated again as it runs. `blocks_translated` and
  the `dynrec.translate` zone count that as well as code the program
  runs for the first time, and in the first seconds the machine can fall
  behind (`window.emulated_speed` below 1). In a 30-second profile of a
  Windows 95 game's battle map the recompiler translated 22,574 blocks
  (3.5% of the busy samples); without a profile the same game, played
  through new parts of a level, translated a median of 18 blocks a
  second. `timeline=true` shows when the translations came; a longer
  profile, or the seconds after the first few, shows the program's own.

### Zones

| Group | Zones |
|---|---|
| `frame` | `frame.wait` (waiting for the next frame), `frame.events` (the host's events), `frame.compose`, `frame.present` |
| `vga` | `vga.render`: the picture from video memory |
| `frontend`, `audio`, `rewind`, `debug`, `ui` | `frontend` (the rest of the front end's frame), `audio.pump` (sound to the host), `audio.synth` (FM, sample and wavetable synthesis as the machine runs), `rewind`, `debug` (the debug server's requests), `ui` (menus and overlays) |
| `cpu` | `cpu.interp`, `cpu.jit` (translated code) |
| `dynrec` | `dynrec.dispatch` (between translated blocks), `dynrec.translate`, `dynrec.fallback` (an instruction translated code hands to the interpreter), `dynrec.slow_mem` (translated code's memory operands checked in Rust, where its TLB or segment checks miss, and its accesses across a page or outside plain RAM) |
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

A sample charged to its callers (`callers`) has them at the root of its
stack, the 32-bit code that switched to 16-bit code marked `(thunk)` and
the entry to ring 0 with how it came in:

```
pm32 0137:10008E52;pm32 0137:BFE3621D;pm32 0137:BFF7E916;pm32 0137:BFF713E2 (thunk);pm16 003B:03D2 (int 30h);cpu.jit;pm32 0028:C023127C 416
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

## Profiling Windows games

A game under a booted Windows shares the machine with the operating
system, and the profile counts both. Most of the split comes from the
CPU state each sample already carries; telling the game's own code from
the Windows code it calls takes a few more steps. The examples come from
Enemy Infestation on Windows 95 (`machine=svga_s3`, recompiler), 30
seconds of its intro.

### By privilege level

`segments` splits the samples by code segment, and the selector's low two
bits are the privilege level. On Windows 95:

| Code | What runs there |
|---|---|
| `pm32`, CS `0028` (ring 0) | The VMM and the VxDs: scheduling, paging, timers, disk and CD drivers |
| `pm32`, CS `0137` (ring 3, flat) | Win32 code: the game's EXE and DLLs, and the system DLLs it calls (KERNEL32, USER32, GDI32, DirectX) |
| `pm16` (ring 3) | Win16 code: the 16-bit halves of USER, GDI and KERNEL, which Win32 calls reach through thunks |
| `v86` | DOS and the BIOS in a virtual machine |

The intro spent its busy time as 56% ring-3 flat code, 31% ring 0, 13%
16-bit code and almost nothing in v86.

### By module

`bucket=65536` counts guest code by 64 KB blocks, and Windows loads each
PE image on a 64 KB boundary. On Windows 95 the address also says whose
code it is:

| Linear address | |
|---|---|
| below 80000000h | The running process: the game's EXE (usually at 00400000h) and its DLLs |
| 80000000h-BFFFFFFFh | Shared by all processes: the system DLLs, KERNEL32 at BFF70000h |
| C0000000h and up | The VMM and the VxDs |

Windows 95 keeps one page directory for every process (CR3 stays the
same), so the profile can't tell processes apart. With a full-screen game
in front, the code below 80000000h is the game's.

`tools/profile_modules.py` names the module behind each block. It needs
Python 3 and nothing else, and reads a running profile from the debug
server:

```
python tools/profile_modules.py --host 127.0.0.1:8086 --files game --system system --save modules.json
```

`--files` is a folder with the game's EXE and DLLs, `--system` one with
Windows' DLLs (`WINDOWS\SYSTEM`). Each file's PE header says where the
image loads, and the script compares the file's code with guest memory at
the block's hottest instruction to confirm it, or finds where Windows moved
a DLL it had to relocate. An image without a file is named from its header
in guest memory, where Windows left the header's page in memory
(KERNEL32's usually is; most others read as `FF` bytes). The script prints
the busy time by module, the time Windows spent for each module (below)
and the modules it found; `--save` keeps them for `--map` in later runs of
the same game. In the intro:

```
Busy time by module (26332 busy samples):
   32.66%  KERNEL32.DLL
   31.00%  ring 0: the VMM and VxDs
   16.68%  SMACKW32.DLL [program]
   13.36%  16-bit ring 3: Win16 (USER, GDI, KERNEL)
    1.25%  WINMM.DLL
    0.37%  EI.EXE [program]
```

`[program]` marks the modules from `--files`. SMACKW32.DLL is the Smacker
video decoder the intro plays with.

#### Getting the files

The files are on the game's disk image, and the emulator can copy them out
without booting Windows. On a copy of the image:

```
rust-dos --no-boot --debug-server 127.0.0.1:0 --config copy.conf
```

with `copy.conf` mounting the copy as C:. Then type into the DOS prompt
(`POST /api/input/type`):

```
mount e C:\host\folder\game
copy c:\progra~1\game\*.exe e:\
copy c:\progra~1\game\*.dll e:\
mount e C:\host\folder\system
copy c:\windows\system\*.dll e:\
```

The built-in shell sees FAT drives with their short names (`dir c:\progra~1`
finds the game's folder). An agent running the profile can do all of this
before the profiled run, and keep the module map for later runs.

### Work Windows does for the game

Ring-0 and 16-bit samples are Windows' code, but the profile also notes
which code made Windows run it, and charges the samples to that code
(`callers`):

- Each entry to ring 0 from an outer level (an interrupt, an exception, a
  hardware interrupt, a call gate or SYSENTER) is noted with the place it
  came from. Every ring-0 sample is charged to the last such entry.
- Each switch from 32-bit to 16-bit ring-3 code (a Win32 call thunking to
  the 16-bit half of USER, GDI or KERNEL) is noted with the Win32 code that
  switched and the callers above it on the stack. 16-bit samples, and
  ring-0 samples entered from that 16-bit code, are charged to that Win32
  code until it runs again.

The callers are found up the EBP chain, and on the stack above ESP where a
routine keeps no frame pointer (KERNEL32's thunk stubs don't); only
addresses right after a CALL instruction count. The report's `callers`
lists each chain, root first, and the script names its modules:

```
Ring-0 and 16-bit time by the code that caused it (44.19% of busy):
   34.38%  SMACKW32.DLL [program]
    7.12%  Win32: KERNEL32.DLL

By chain, root first:
   24.41%  SMACKW32.DLL [program] -> WINMM.DLL -> KERNEL32.DLL -> pm16 int 30h -> ring 0: the VMM and VxDs
    6.57%  KERNEL32.DLL -> 16-bit code -> 16-bit ring 3: Win16 (USER, GDI, KERNEL)
    5.54%  SMACKW32.DLL [program] -> WINMM.DLL -> KERNEL32.DLL -> 16-bit code -> 16-bit ring 3: Win16 (USER, GDI, KERNEL)
```

The video decoder calls WINMM, which thunks through KERNEL32 to 16-bit
code that enters the VMM with `int 30h`: a time query, 139,000 times a
second. With the decoder's own 17%, the intro's video costs about half of
the busy time. Samples with no Win32 caller (hardware interrupts, the
KERNEL32 code that thunks on its own) are charged to the entry alone.

The counters say what the ring-0 code did:

- `ports`: port 40h read 279,000 times a second and port 43h written
  139,000 times: the VTD latching the PIT for each time query.
- `dynrec.calls.fallback_top`: `Int`, `Iretd`, `Lar`, `Popfd` and
  `Cmpsb` each at about the rate of the time queries. Each time query
  changes rings in and out, and the recompiler hands both changes to the
  interpreter.
- `ports` again: port 170h, the secondary IDE channel's data port, read
  230,000 times a second: Windows' IDE driver reads the CD with PIO.
  `zone=io.ide` shows the driver's loop (`0028:C1051C27`).
- `exceptions`: #GP and #UD are how Windows traps port I/O and V86 calls,
  #PF its paging. A high rate here points at Windows working on the
  game's behalf.

Before profiling a game, profile the idle desktop for 10 seconds as a
baseline. Here Windows 95 alone was busy 7% of the time, against 40% with
the game.

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

A profile only watches. Two tests run the same program with and without
one, on the interpreter and (with `RUST_DOS_CORE=dynamic`) the recompiler,
and require the same machine: `a_profile_leaves_the_machine_as_it_would_be`
in `src/debug/determinism_tests.rs` starts a profile partway through the
program that uses every device and compares the whole save state, the
picture, the sound and the instruction count at each stop;
`a_profile_leaves_ring_changes_as_they_would_be` in `tests/pm_tests.rs`
does the same for the ring changes, call gates and thunks the profile
notes in protected mode.

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

Two more words carry what `callers` charges samples to. `profile::ENTRY`
holds the last entry to ring 0 from an outer level: the place it came from
(EIP, CS and the mode, as in the word above), the cause (an interrupt, an
exception, a hardware interrupt, or a call gate or SYSENTER) and the
vector. `profile::THUNK` holds the last switch from 32-bit to 16-bit ring-3
code: the place after the instruction that switched, then up to three
callers' return addresses. A switch back to 32-bit code clears it. The
callers come from the EBP chain, or, where that finds none, from the
32-bit stack above ESP, and only an address right after a CALL counts. The
walk reads RAM through the page tables without faulting, so it changes
nothing in the machine, and runs once per switch.

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

## Voodoo software pipeline states from a saved scene

The ignored `profile_saved_voodoo_scene` library test records the states
submitted by a saved game. It requires the `sdl` feature and is absent
from normal builds. Use an optimized test build; emulating a saved
Windows scene in an unoptimized build can take hours.

Set `RUST_DOS_VOODOO_STATE` to the saved state. Create a separate output
directory and copy its C-drive sidecar there as `run.C.img`. Set
`RUST_DOS_VOODOO_PROFILE_DIR` to that directory. The test restores and
writes the scratch image, so keep it separate from the original. The
original state and its sidecars must remain available for restoration.

```sh
cargo test --release --no-default-features --features sdl --lib \
  profile_saved_voodoo_scene -- --ignored --nocapture --test-threads 1
```

`RUST_DOS_VOODOO_BATCHES` and `RUST_DOS_VOODOO_INSTRUCTIONS` override the
observation length, which defaults to 60 batches of 1,600,000 guest
instructions. The output contains `voodoo-states.json` and
`last-frame.png`. Each key fixes the same register bits as a full software
variant. The report includes the selected pipeline, submitted triangle
count and unclipped projected area. These are usage counts, not shaded
pixels, CPU time or a game performance comparison. Inspect the picture
to establish which part of the game the counts cover.
