# Debugging rust-dos with an LLM agent

This guide is for AI agents (and humans scripting the emulator) using the
built-in debug server to run DOS programs, see what they do, and find
emulator bugs. For the full endpoint reference, run `curl localhost:8086/`.
This file covers how to use the endpoints well.

## 1. Start the emulator

```sh
cargo build --release
SDL_VIDEODRIVER=dummy SDL_AUDIODRIVER=dummy \
  ./target/release/rust-dos --no-config -d /path/to/dos/files --debug-server &
until curl -sf localhost:8086/api/status >/dev/null; do sleep 0.2; done
```

- **Build mode:** use a release build. A debug build runs about 5x slower,
  which changes timing-sensitive behaviour.
- **Dummy drivers:** they need no window or sound device. Audio is still
  mixed and streamed.
- **C: drive:** `-d` sets it. `AUTOEXEC.BAT` in that directory runs at
  startup.
- **`--no-config`:** without it, rust-dos reads `rust-dos.conf` from the
  working directory or the per-user config directory. That file can mount
  drives and run commands at startup, and on first start rust-dos writes a
  template into the user's config directory. Pass `--no-config` for
  reproducible runs, or `--config FILE` to test a specific configuration.
- **Port conflict:** if port 8086 is in use, the emulator exits at startup
  with a `cannot bind` error. Kill the old instance (`kill $(pgrep -x rust-dos)`)
  or pass `--debug-server 127.0.0.1:<port>`.
- **Output:** the emulator log doesn't go to stdout. It goes to
  `rust-dos.log` in the per-user config directory, replaced on every start.
  Read it through `/api/log` instead, which can filter it.

All examples below use `H=localhost:8086` and `J='-H content-type:application/json'`.

## 2. The basic loop: act, then look

```sh
curl -s $J -XPOST -d '{"text":"t\n"}' $H/api/input/type    # run T.COM
curl -s "$H/api/screen/text?format=text"                    # read the screen
```

- **Input calls block until the input is delivered.** When they return, the
  program has received it, so you can look at the screen right away. The
  program may still need a moment to react; if the screen hasn't changed
  yet, wait briefly and check again. Pass `?wait=false` to return
  immediately.
- **Use text mode where you can.** `/api/screen/text` is a few hundred
  tokens; a screenshot costs far more. It returns HTTP 409 in graphics modes,
  and only then should you use `/api/screenshot` (a 640x400 PNG, including
  the text and mouse cursors).
- **Check state with `/api/status`.** It reports `shell_idle` (true at the
  DOS prompt), `cs_ip`, the video mode, `paused`, `input_queue`, and
  `cycles_per_ms`, the current CPU speed. `video.vga` shows the CRTC Start
  Address the program set and the one on screen, which helps when a game
  that flips pages flickers or shows half-drawn frames.
- **Check sound with `/api/status` → `audio`.** `peak` is the loudest
  sample since the previous status request (0 means silence), `underruns`
  counts how often the output device ran dry, and `sound_blaster` is the
  card's BLASTER string (null without one). To listen or analyse, record
  `/ws/audio`: a JSON format header, then s16le stereo PCM at 44.1 kHz
  (`curl -s --max-time 3 ws://localhost:8086/ws/audio -o audio.bin`).
  `ultrasound` is the ULTRASND string, and `midi_synth` says what plays the
  MPU-401 (`soundfont`, `gus` or `none`), with its active voices and any
  patches it couldn't load.
- **Gravis Ultrasound:** the built-in Gravis patch set is on the
  read-only drive X: in `X:\ULTRASND`, where `ULTRADIR` points, so a game
  needs no copy of the Ultrasound software on C:. `/api/gus` shows the IRQ
  and DMA the driver latched, the reset and mix registers, IRQ status and
  line, timers, DMA, the number of active and playing voices, and every
  voice's registers. A card that plays nothing often has `reset` without
  bit 1 (DAC off) or voices whose volume stays 0.
- **Kill a stuck program** with `POST /api/control/reboot_shell`. This is
  better than restarting the emulator.

## 3. Input details

| Want | Request body (`POST /api/input/...`) |
|---|---|
| Type a command | `type` `{"text":"dir /w\n"}` |
| Special key | `key` `{"key":"f10"}`, or `"esc"`, `"up"`, `"enter"`, `"pagedown"`, ... |
| Key combination | `key` `{"key":"x","mods":["alt"]}`. Ctrl+letter sends ASCII 1-26. |
| Hold a key | `key` `{"key":"right","hold_ms":500}`, or `"action":"down"` then later `"up"` |
| Click | `mouse` `{"action":"click","x":320,"y":200}` |
| Sequence | `batch` `[{"type":"type","text":"cd game\n"},{"type":"wait","ms":500},{"type":"type","text":"game\n"}]` |

- **Mouse coordinates** are pixels of the 640x400 screenshot by default.
  Pass `"coords":"virtual"` to use the INT 33h driver's own coordinates.
  `/api/status` → `mouse` shows the driver's position and whether it is
  installed.
- **Keys arrive at most one scan code per frame** (about 16 ms). A long
  `type` string takes roughly 2 frames per character, or 4 for shifted
  characters.
- **Unknown key names** return an error that lists all valid names.
- **Stop pending input:** `DELETE /api/input` clears whatever is still
  queued.

## 4. Recipes

### A program crashes, hangs, or prints garbage

```sh
curl -s $J -XPOST -d '{"enabled":true,"clear":true}' $H/api/trace
# ...reproduce the problem...
curl -s "$H/api/trace?last_n=200&no_bios=true"
curl -s "$H/api/log?limit=50&format=text"
```

- **Start the trace first.** It only records while enabled; it does not
  look back in time.
- **Reading the trace.** Each line is: instruction count, milliseconds since
  start, `CS:IP`, bytes, disassembly, and the registers *before* the
  instruction ran. `HLE INT 21h (AX=4C00)` marks a BIOS/DOS service call
  handled by the emulator. `AX` tells you which function was called.
- **Filters:**
  - `no_bios=true` hides code running in the BIOS segment F000, but keeps
    the HLE service calls.
  - `cs=1000` keeps only one code segment.
  - `last_ms=500`, or `from_ms`/`to_ms`, select a time window.
- **Log markers to look for:**
  - `[TRIPWIRE]`: execution jumped into the interrupt table, almost always
    through a corrupted far pointer. The emulator then reloads the shell.
  - `[Unhandled IO Write]`: a port write for a device the emulator doesn't
    model.
  - `[BIOS] Unhandled INT xx AH=yy`: a BIOS function the emulator doesn't
    implement.
  - `[DOS] ...`: file operations and program loading.
  - Filter with `?grep=Unhandled` or `?grep=TRIPWIRE`.

### Stop a program at a specific point

Find out where the program was loaded. The log says so:

```sh
curl -s "$H/api/log?grep=Loaded&format=text"
#   [DOS] Loaded COM file at 1000:0100          (COM file)
#   [DOS] Loaded. Entry CS:IP = 1234:0000        (EXE file)
```

A COM file's code starts at offset 0100 of the segment shown (usually 1000).
Set the breakpoint before triggering the code path, then wait for it:

```sh
curl -s $J -XPOST -d '{"addr":"1000:010B"}' $H/api/breakpoints
curl -s $J -XPOST -d '{"key":"q"}' "$H/api/input/key?wait=false"
curl -s "$H/api/control/wait?timeout_ms=10000"   # returns registers when the breakpoint hits
curl -s "$H/api/disasm?count=10"                 # "=>" marks CS:IP, "*" marks breakpoints
curl -s "$H/api/memory?addr=DS:SI&len=64"
curl -s $J -XPOST -d '{"count":5}' $H/api/control/step
curl -s -XDELETE $H/api/breakpoints              # remove all breakpoints
curl -s -XPOST $H/api/control/resume
```

- **Why `wait=false`:** use it for input that should trigger a breakpoint.
  Otherwise the input call can block until it times out, because the
  emulator stops at the breakpoint before all of the input is delivered.
- **Run to an address:** `POST /api/control/resume {"until":"1000:0120"}`
  runs to that address once, without adding a permanent breakpoint.
- **If a wait times out,** the breakpoint wasn't hit. Check
  `/api/breakpoints` and `/api/status`, and whether the program was loaded
  at a different segment.

### Inspect or patch state

- **Registers:** `GET /api/registers`, or
  `PUT /api/registers {"ax":"1234","flags":"0246"}`. Strings are hex; JSON
  numbers are decimal.
- **Memory:**
  - Read with `GET /api/memory?addr=B800:0000&len=160`.
  - Write with `PUT /api/memory {"addr":"1000:0200","hex":"90 90"}`. Writes
    also invalidate the decoded-instruction cache, so patching code works.
- **Interrupt vectors:** `GET /api/ivt`. `hle:true` means the vector still
  points at the emulator's built-in handler, so a program has not hooked it.

### Switch to another program directory, or mount more drives

`PUT /api/drive/C {"path":"/abs/path"}` remounts C: without restarting.
Files open on C: are closed and its current directory resets to `C:\`. Do
this at the shell prompt, not while a program is running.

The same call mounts other drives. Add `"type"` (`floppy`, `hdd` or `cdrom`),
`"label"` or `"read_only":true`, for example
`PUT /api/drive/D {"path":"/abs/cd","type":"cdrom","label":"GAMECD"}`.
`DELETE /api/drive/D` unmounts a drive, and `GET /api/drive` lists them. At
the DOS prompt, the `MOUNT` command does the same.

### Protected-mode programs (DOS extenders)

Programs built with DOS/4GW, DOS/32A, PMODE or Borland's RTM switch the CPU
to protected mode. `/api/status` shows `cpu_mode` (`real`, `protected` or
`v86`), and `/api/registers` adds a `system` object: CPL, CR2, CR3, the
GDTR, IDTR, LDTR and TR, and every segment register's base, limit and
access rights.

- **Extenders switch modes constantly.** DOS/4GW goes back to real mode
  for every DOS or BIOS call and for hardware interrupts it reflects, so a
  paused program in real mode inside the extender is normal. A crash back
  to DOS shows as `shell_idle: true` or a text mode.
- **Exceptions:** `GET /api/exceptions` lists the last 64 (vector, error
  code, `CS:EIP`, CR2 for page faults). Page faults (`#PF`, vector 0E) are
  normal under DOS/4GW, whose virtual memory manager loads pages on demand.
  The log also has a line for each of the first 200 exceptions
  (`?grep=%23GP`; `#` must be written `%23` in a URL).
- **Break on an exception:** `POST /api/breakpoints {"exception":"0D"}`
  (or `"any"`) pauses at the first instruction of the handler after the CPU
  raises it; the pause reply includes the exception. `{"mode_switch":true}`
  pauses after each switch between real and protected mode.
- **Tables:** `/api/gdt`, `/api/ldt` and `/api/idt` decode descriptors and
  gates; `/api/tss` shows the ring stacks and saved registers;
  `/api/pagewalk?addr=lin:00401000` shows the page directory and table
  entries; `/api/xms` lists XMS handles (where extenders put their memory).
- **Disassembly and traces** use the code segment's size (16 or 32-bit), and
  the trace records it per instruction.

## 5. Addresses

Addresses are always hex, as in DEBUG.COM:

- `SEG:OFF` with numbers: `B800:0000`, `1000:10B`. In protected mode the
  first part is a selector, looked up in the GDT or LDT, and the offset can
  be 32 bits: `0268:100CA541`.
- `SEG:OFF` with register names: `CS:IP`, `DS:SI`, `ES:DI`, `SS:SP`, or the
  32-bit ones: `CS:EIP`, `DS:ESI`. A segment register name uses that
  register's current base.
- `lin:ADDR`: a linear address, translated through the page tables when
  paging is on.
- `phys:ADDR`, or just `ADDR`: a physical address (`B8000`, `0x12345`).

A decimal-looking number like `100` means 0x100. Breakpoints are kept by
physical address, so a breakpoint on a paged address follows the page the
address mapped to when it was set.

## 6. Pitfalls

- **Idle-looking traces.** A program waiting for a key loops between
  `int 16h` and `HLE INT 16h` forever. Traces taken while it waits are full
  of that loop, so filter with `cs=`, or trace only across the interesting
  action.
- **Paused inside a BIOS stub.** When `CS` is F000, execution is inside an
  emulator service stub. `step` once or twice to get back to program code.
- **Emulated time.** Timers run on emulated time, which advances with
  executed instructions at `cycles_per_ms` (see `/api/status`) and stops
  while paused. Pausing, stepping and tracing don't change when a program
  sees its timer interrupts (`HLE INT 08h` or its own handler) in
  instruction terms, but a timer interrupt can arrive between any two steps.
- **Measuring speed.** `/api/stats` reports `mips` (host speed while
  executing guest code), `emulated_mips` (guest instructions per wall-clock
  second) and the decode-cache hit rate, averaged over about a second. Use a
  release build and a CPU-bound scene, such as the Stunts intro, when
  comparing emulator changes.
- **Coarse timestamps.** Trace times are sampled once per frame (about
  16 ms). Use `icount` for exact ordering.
- **Ring buffer size.** The trace ring holds 1,000,000 instructions by
  default (`--trace-capacity`). That is only a few frames of a busy
  program, so query soon after reproducing a problem.
- **Streams are for tools, not agents.** `/ws/trace` sends about 7 MB/s and
  `/ws/screen` sends PNGs. Don't pipe them into your context. Prefer HTTP
  polling, or `/ws/events` for pause, log, and video-mode notifications.
- **Emulator gaps.** Many DOS features are incomplete; see the README. When a
  program misbehaves, the emulator is the likelier culprit. The trace plus
  the log usually shows which interrupt function or port is missing.
