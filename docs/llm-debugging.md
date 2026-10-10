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
  and only then should you use `/api/screenshot` (a PNG, including the text
  and mouse cursors). Its size is the picture's: 640x400 in text modes, and
  the mode's own size in graphics modes, doubled where it is small (mode 13h
  is 640x400, mode 12h and 320x240 "mode X" 640x480). `/api/status` →
  `video.frame` has it.
- **Check state with `/api/status`.** It reports `shell_idle` (true at the
  DOS prompt), `cs_ip`, the video mode and `video.adapter` (the display
  adapter `machine` gives programs), `paused`, `input_queue`, and
  `cycles_per_ms`, the current CPU speed. `video.vga` shows the CRTC Start
  Address the program set and the one on screen, which helps when a game
  that flips pages flickers or shows half-drawn frames. `video.crt` is the
  display timing the VGA registers describe (refresh rate, total and
  displayed scanlines): port 3DAh reports retrace and display enable from
  it in emulated time, and the Start Address is latched at each retrace. In
  a VESA mode (`video.name` is `Vesa`), `video.vbe` shows the VBE mode
  number, size and bits per pixel, the bank of the window at A0000h, the
  bytes per scan line, the displayed start offset and the DAC width; the
  log has a `Switch to VESA mode` line for each mode set. With
  `machine=svga_et4000`, `video.et4000` has the chip's CR30-3Fh, whether
  the KEY is set, Segment Select (write bank low, read bank high), the
  Sierra DAC's command register and HiColor bits, and the VESA mode set;
  its Super VGA modes are VGA modes (`Graphics320x200` for 256 colours or
  HiColor, `Vga640x480` for 16) at the size `video.width`/`height` give,
  and the log has a `Switch to Tseng mode` line for each. `pic` shows both
  interrupt controllers' vector bases and their request (`irr`), mask
  (`imr`) and in-service (`isr`) registers, master first: a request that
  stays set under a mask bit is an IRQ the program has turned off.
- **Check sound with `/api/status` → `audio`.** `peak` is the loudest
  sample since the previous status request (0 means silence), `underruns`
  counts how often the output device ran dry, and `sound_blaster` is the
  card's BLASTER string (null without one). To listen or analyse, record
  `/ws/audio`: a JSON format header, then s16le stereo PCM at 44.1 kHz
  (`curl -s --max-time 3 ws://localhost:8086/ws/audio -o audio.bin`).
  `ultrasound` is the ULTRASND string, and `midi_synth` says what plays the
  MPU-401 (`soundfont`, `gus` or `none`), with its active voices and any
  patches it couldn't load. `cd` shows the CD audio a CD image drive plays
  through MSCDEX: the drive, `playing`, `paused` or `stopped`, the track
  and the position (MM:SS:FF), or null when nothing was played; the log
  has a `[MSCDEX] Play audio` line for each Play request.
- **Gravis Ultrasound:** the built-in Gravis patch set is on the
  read-only drive X: in `X:\ULTRASND`, where `ULTRADIR` points, so a game
  needs no copy of the Ultrasound software on C:. `/api/gus` shows the IRQ
  and DMA the driver latched, the reset and mix registers, IRQ status and
  line, timers, DMA, the number of active and playing voices, and every
  voice's registers. A card that plays nothing often has `reset` without
  bit 1 (DAC off) or voices whose volume stays 0.
- **Kill a stuck program** with `POST /api/control/reboot_shell`. This is
  better than restarting the emulator. `POST /api/control/reboot` resets
  the machine as Ctrl+Alt+Del does: DOS starts over and runs its startup
  again.

## 3. Input details

| Want | Request body (`POST /api/input/...`) |
|---|---|
| Type a command | `type` `{"text":"dir /w\n"}` |
| Special key | `key` `{"key":"f10"}`, or `"esc"`, `"up"`, `"enter"`, `"pagedown"`, ... |
| Key combination | `key` `{"key":"x","mods":["alt"]}`. Ctrl+letter sends ASCII 1-26. |
| Hold a key | `key` `{"key":"right","hold_ms":500}`, or `"action":"down"` then later `"up"` |
| Click | `mouse` `{"action":"click","x":320,"y":200}` |
| Sequence | `batch` `[{"type":"type","text":"cd game\n"},{"type":"wait","ms":500},{"type":"type","text":"game\n"}]` |

- **Mouse coordinates** are pixels of the screenshot by default.
  Pass `"coords":"virtual"` to use the INT 33h driver's own coordinates.
  `"dx"` and `"dy"` instead move by that much, as a captured mouse does:
  programs that read the PS/2 mouse themselves, such as Windows, only
  follow motion, so steer their pointer with these.
  `/api/status` → `mouse` shows the driver's position and whether it is
  installed.
- **Keys arrive at most one scan code per frame** (about 16 ms). A long
  `type` string takes roughly 2 frames per character, or 4 for shifted
  characters.
- **Unknown key names** return an error that lists all valid names.
- **Stop pending input:** `DELETE /api/input` clears whatever is still
  queued.
- **The settings window** opens with `DOSCONFIG` at the prompt or
  `key {"key":"f12","mods":["ctrl"]}`, and closes with the same key or
  `esc`. While it is open the machine is paused, `/api/status` shows
  `"settings_window": true`, screenshots show the window, and keys and
  mouse clicks go to the window instead of the machine. Recordings and
  Ctrl+F5's screenshots leave it out, unless `record_ui` is on.

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
- **Trace a stretch of instructions:** `POST /api/trace {"count":5000}`
  records the next 5000 instructions and stops, so the ring keeps what
  came right after a breakpoint instead of being overwritten.
  `/api/status` shows the instructions still to record as `remaining`, and
  `{"count":0}` stops the trace at once. An open `/ws/trace` stream keeps
  the ring recording past the count.
- **Read a long trace in pages:** `GET /api/trace?since=0&limit=2000&format=json`
  gives the entries after cursor 0, oldest first, and `next`, the cursor
  for the following page (text replies have it in the `x-trace-next`
  header). `dropped` counts entries after the cursor that were
  overwritten before they were read. `/api/trace` without `since` has a
  `next` too, which pages from there on.
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

To stop at a program's first instruction, start it with `run` instead of
typing it:

```sh
curl -s $J -XPOST -d '{"command":"GAME.EXE /nosound","stop_at_entry":true}' $H/api/control/run
```

It runs the command line at the DOS prompt as if typed, and replies once
the machine is paused at the entry point of the program it started, with
`"reason":"program_start"`, the registers and
`"program":{"name","entry","psp"}`. Nothing of the program has run yet, so
breakpoints set now catch its startup code. Without `stop_at_entry` it
replies as soon as the program started. A command line that starts no
program (a typo, a built-in command such as `DIR`) gets HTTP 422 once the
prompt is back, and `run` while a program runs, or while PAUSE, CHOICE or
EDIT waits, gets 409. So does a `stop_at_entry` run whose program was
closed before it reached its entry point.

- **Every program:** `POST /api/breakpoints {"program_start":true}` pauses
  at the entry point of each program DOS starts, a child that a game's
  launcher starts with EXEC too. `{"program_exit":true}` pauses after each
  program ends, with `"reason":"program_exit"` and
  `"exit":{"name","code","resident","aborted"}` (`resident` for a TSR);
  the machine is then back in the parent or the shell. A program the
  emulator ended without an exit of its own (a divide overflow,
  `reboot_shell`, a reboot) has `"aborted":true` and `"code":null`.
  `false` turns either off.
- **How the last program ended:** `/api/status` has `program`, the program
  running (empty at the prompt), and `last_exit`.

Otherwise, find out where the program was loaded. The log says so:

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
- **Step over:** `POST /api/control/step_over` runs a `CALL`, `INT`, `LOOP`
  or `REP` string instruction through to the instruction after it, and
  steps any other instruction. It replies when the machine stops there; a
  call that never returns times the request out and leaves the machine
  running.
- **Stepping and interrupts:** a hardware interrupt that is due is taken
  before the next instruction. A step can then run the emulator's handler
  (an `FE 38` trap, which returns at once) and report the same CS:IP.
- **Watchpoints:** `POST /api/watchpoints {"addr":"DS:0100","len":2}`
  pauses right after an instruction changes those 1, 2 or 4 bytes. The
  `paused` reply and event carry `"reason":"watchpoint"` and
  `"watch":{"addr","len","old","new"}`. `GET` lists them with their values;
  `DELETE ?addr=` removes one, `DELETE` without it removes all. Like
  breakpoints they are physical addresses, and they slow the machine down
  to the interpreter while any is set.
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
    The reply has the bytes the write replaced (`old`) and what reads back
    (`new`), which can differ from the data in VGA memory.
  - Write only if the memory holds what you expect with `"expect"`:
    `PUT /api/memory {"addr":"DS:0200","hex":"21 43","expect":"CD AB"}`.
    When the bytes there differ, nothing is written and the reply is HTTP
    409 with `found` (the bytes there) and `expected`. The check and the write run between two
    instructions, so the program can't change the bytes in between, paused
    or not. `expect`, `old` and `new` are what `GET /api/memory` reads. In
    the planar VGA modes (A000 outside mode 13h) that read sees one plane
    through the VGA's read mode, while the write goes through its write
    logic as a CPU write does (write mode, Map Mask, Bit Mask), so a match
    says nothing about the other planes.
- **Interrupt vectors:** `GET /api/ivt`. `hle:true` means the vector still
  points at the emulator's built-in handler, so a program has not hooked it.
- **Disassembly as data:** `GET /api/disasm?format=json` has, next to the
  text `lines`, `rows` of `{label, phys, bytes, asm, len, current,
  breakpoint}`.

### Switch to another program directory, or mount more drives

`PUT /api/drive/C {"path":"/abs/path"}` remounts C: without restarting.
Files open on C: are closed and its current directory resets to `C:\`. Do
this at the shell prompt, not while a program is running.

The same call mounts other drives. Add `"type"` (`floppy`, `hdd` or `cdrom`),
`"label"` or `"read_only":true`, for example
`PUT /api/drive/D {"path":"/abs/cd","type":"cdrom","label":"GAMECD"}`.
A path to a disk or CD image (`.img`, `.ima`, `.vfd`, `.flp`, `.dsk`, `.cue`,
`.iso`, `.bin`) mounts the image, as a floppy, hard disk or CD-ROM drive by
what it holds unless `"type"` says; `GET /api/drive` then shows
`"image":true` and the image's path. `"images":["/abs/disk2.img"]` adds
more images to the drive, and `POST /api/drive/swap` puts the next one in
every drive with a list, as Ctrl+F4 does (`images` and `image_index` in
`GET /api/drive` show which). A: and B: are always floppies, whatever
`"type"` says, and refuse CD images. `DELETE /api/drive/D` unmounts a drive, and `GET /api/drive` lists
them. At the DOS prompt, the `MOUNT` and `IMGMOUNT` commands do the same.

### Save states

`POST /api/state/save {"path":"/abs/before-boss.state"}` saves the whole
machine to a file, and `POST /api/state/load` with the same body goes back
to it, as the Ctrl+F1 and Ctrl+F2 slots do. Save before a step that is
slow to reach (a menu path, a level) and load to try it again. The load's
reply has the state's header: when it was saved, the program, and the
hardware settings it applies first. The files and disk images on the host
aren't part of a state: what a program wrote since stays written, except
for a booted system's disks (next section).

### Deterministic runs

`--deterministic` makes two runs of the same program with the same input
come out the same: the same instructions, memory and pictures at the same
emulated time, however fast the host is.

```sh
SDL_VIDEODRIVER=dummy SDL_AUDIODRIVER=dummy \
  ./target/release/rust-dos --no-config -d /path/to/game --debug-server \
  --deterministic --cycles 10000 --start-time "1995-04-11 12:34:56" &
curl -s $J -XPOST -d '{"until_ms":5000}' $H/api/control/resume
curl -s "$H/api/control/wait?timeout_ms=600000"      # "reason":"time"
curl -s $J -XPOST -d '{"key":"enter"}' "$H/api/input/key?wait=false"
curl -s $J -XPOST -d '{"until_ms":8000}' $H/api/control/resume
```

- **Speed:** a fixed number of cycles. `--cycles` has to be a number;
  `auto` or `max` from the configuration, a game's profile or the
  settings window become 3000 or the speed the machine has, and
  `/api/speed` takes only a number.
- **Clock:** the real-time clock, DOS's date and time, file times and the
  BIOS tick count start at `--start-time` (default 1995-04-11 12:34:56)
  and run with emulated time.
- **Starts paused:** with the debug server the machine starts paused
  (`"reason":"startup"`), so the input can be sent before anything runs.
- **Input:** input is taken only while the machine is paused (HTTP 409
  otherwise). It is delivered at fixed points in emulated time, every 10
  ms, from the time the machine resumes: one scan code per point, and
  `wait` and `hold_ms` count emulated milliseconds. Send it with
  `?wait=false`, as the reply waits for its delivery.
- **Stopping at a time:** `resume {"until_ms":N}` pauses when emulated time
  reaches N ms since power-on, with `"reason":"time"`. Compare runs there:
  `/api/status` has `icount` and `activity.emulated_ns`, and
  `deterministic` (null when off) has the start time, the clock now and
  `emulated_ms`.
- **Stops are part of the script:** breakpoints, steps and `until_ms`
  stop at the same instruction on every run, so the same stops give the
  same run. `POST /api/control/pause` stops wherever the host's frame got
  to, and the machine can go on slightly differently from there.
- **What it leaves out:** pictures from `/api/screenshot` have no
  on-screen messages, which come and go with the host's time, and host
  game controllers aren't connected. Input from the window's keyboard and
  mouse, network and serial links, and the end of a game launched from
  its profile still follow the host.

### Booted systems (Windows 95)

`IMGMOUNT C /abs/copy.img` then `BOOT -l C` starts the system on the
image (see CONFIGURATION.md's "Booting a disk image"). Work on a copy:
the system writes its disk (`cp --reflink=auto` on btrfs takes no room).

- **Status:** `/api/status` has `cpu_mode` `v86` while a DOS box or the
  BIOS runs under Windows, `video.s3` with the S3's extended CRTC
  registers and hardware cursor position (with `machine=svga_s3`, and a
  ViRGE's), and with `svga_s3virge`/`svga_s3virgevx` `video.s3.virge`:
  the 2D engine's BitBLTs, rectangles, lines and polygons, the 3D
  engine's `triangles` and `lines_3d`, the last 3D command (`cmd_3d`),
  the subsystem status, and the overlay, if one is shown,
  `video.vga.crtc` with the standard CRTC registers, and `kbc` with the
  keyboard controller's output buffer and queues.
- **The mouse:** Windows reads the PS/2 mouse's motion and accelerates
  it, so move in small relative steps (`{"action":"move","dx":10,"dy":-6}`)
  a few tenths of a second apart and read where the pointer went from
  `video.s3.cursor`. Absolute `x`/`y` moves go to the built-in DOS's
  driver, which a booted system doesn't use.
- **Faults:** Windows handles most exceptions itself (#GP for trapped
  port I/O and in 16-bit code, #PF for paging, #NP for segments loaded
  on demand), so `/api/exceptions` is busy. A "fatal exception" screen or
  an "illegal operation" names where Windows gave up, not always the
  first fault: break on the vector (`{"exception":"0D"}`) and look for the
  fault in the application's own code (CS with RPL 3, not in V86 mode),
  then read the descriptors (`/api/ldt`) of the segments it used.
- **Memory:** `lin:` addresses go through the current page tables, which
  are those of the machine (DOS box) running at that moment; a breakpoint
  at a BIOS entry (`phys:F1008` for INT 10h) stops in the caller's
  context. Watchpoints on physical addresses find who writes a page table
  entry or descriptor.
- **Save states** of a booted system bring its disks back with it: in a
  run through a journal of the writes, and from the `.C.img` copies
  beside a state file in another.
- **IDE:** the log's `[IDE]` lines at the boot say which hard disk and
  CD-ROM drive are on which channel, and name unknown commands. The
  system's own driver's traffic shows in the port log
  (`{"enabled":true,"ports":"1F0-1F7"}` for the primary channel): READ
  SECTORS (20h) and WRITE SECTORS (30h) at 1F7h, the address at 1F3h-1F6h
  (1F6h's bit 6 set for LBA), the data words at 1F0h.

### 3dfx (Glide) games

With `voodoo=true` (CONFIGURATION.md's "3dfx Voodoo Graphics"),
`/api/status` → `video.voodoo` shows the card: `output` (true while its
picture is on the screen instead of the VGA's; `/api/screenshot` then
shows it), `clock` (the video clock Glide turns on first), `base` (BAR0),
`width`/`height` and `hz`, the `front` and `back` buffers, `pending_swaps`
(swaps waiting for the retrace), `fifo_writes` behind them, `triangles`
drawn, the `fbiInit` registers and `init_enable`, and `opengl` (true
while `voodoo_renderer=opengl` draws the window's picture again). While it
shows, `fps` counts its buffer swaps. `/api/screenshot` is the software
rasterizer's picture at the card's resolution even with OpenGL; to see
OpenGL's, run rust-dos in a headless compositor and capture that (for
sway: `WLR_BACKENDS=headless sway`, then `grim`). A game that detects no card
usually left `fbiInit` at their power-on values (`00000410 00201102
80000040 001E4000 00000001`); one that drew nothing has `triangles` 0.

- **Memory:** `/api/mem` reads the card's window (`phys:D0000000`, 16 MB):
  registers at +0, the frame buffer at +400000h (2048 bytes a row of
  16-bit pixels), texture memory at +800000h (write-only; reads give FFh).
- **Speed:** the triangles are drawn on worker threads; `cycles_per_ms`
  shows what is left for the game's own code.

### PowerVR games

With `powervr=pcx2` (CONFIGURATION.md's "PowerVR PCX2"), `/api/status` →
`video.powervr` shows the card: `bar0` (registers) and `bar1` (texture
memory), `irq`, `renders` started, and the registers a frame sets
(`OBJECT_OFFSET`, `SOFADDR` where the pixels go, `PACKMODE`, `LSTRIDE`,
`INTSTATUS` with bit 1 for a render done). Its picture is in the VGA's
frame buffer, so `/api/screenshot` shows it. A game that renders nothing
has `renders` 0; one whose picture stays black likely set `SOFADDR`
outside the VESA linear frame buffer.

`RUST_DOS_POWERVR_TRACE=<dir>` in the environment writes every access to
the card into `<dir>/trace.txt`, and the registers, texture memory and
the machine's RAM at the first four renders and every thousandth (every
`RUST_DOS_POWERVR_TRACE_EVERY`th) as
`render-N.regs`, `.tex` and `.ram`. `RUST_DOS_POWERVR_SNAPSHOT=<dir>/render-N
cargo test --release --test powervr_tests -- --ignored renders_a_snapshot`
renders one into `render-N.png`, and prints the time it took.

### Rendition Vérité games

With `machine=svga_verite`, `/api/status` → `video.verite` shows the
card: `running` (the microcode started through INT 10h AX=1583h),
`fifo` (words waiting for a whole command) and `output` (answers the game
hasn't read), `commands` (each opcode/vertex type seen and how often) and
the drawing state (`draw`: destination, texture, source and blend modes).
A game that gives up at start says "Could not open Verite" with its
library's error code. `RUST_DOS_VERITE_TRACE=<dir>` writes every access
to the card's ports, BIOS call, DMA block and FIFO word to
`<dir>/trace.txt`.

### CPU speed (`cycles=auto`)

`POST /api/speed {"cycles":"auto"}` changes the speed as the settings
window does (`auto`, `auto 5000`, `max` or a number); `/api/status` has
`cycles_per_ms`, the speed now. `auto` (the default) picks the speed from
what the program does, and `/api/status` → `activity` has its counts since
start: `bursts` (writes to video memory the size of a frame, the frames a
program copies there), `flips` (display start changes), `polls` (key
checks that found nothing, idle calls) and `status_reads` (port 3DAh),
with `emulated_ns` to make rates of them. Sample it twice and divide:
a game's frame rate at a few fixed speeds shows where it stops growing.
Start the emulator with `RUST_DOS_AUTO_TRACE=1` to have every measurement
and the speed it chose printed to stderr.

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
- **The DPMI host.** With `dpmi` on (the default), extenders that look for
  a DPMI host (DOS/4GW, PMODE/W, DOS/32A, Tran's PMODE, DJGPP, Borland's
  RTM, HX's DPMILD32, which places Win32 programs without relocations at
  their image base with the DPMI 1.0 function 0504h) run their programs
  as its clients, at CPL 3 with LDT selectors
  (`/api/ldt`), and go to real mode through the host's code in the ROM at
  F000:2000 (level 0 selector 0008h there is its IDT's handlers). An INT
  21h a client doesn't handle itself goes to DOS through the host, which
  copies its buffers through a transfer buffer at the start of the
  client's private data (the "MS-DOS" extensions of INT 2Fh AX=168Ah, as
  Windows has them); DOS's log lines show what DOS got. The video BIOS
  functions with a pointer in ES (palettes, DAC blocks, fonts, AH=13h's
  string, AX=1B00h) go down the same way. The log's
  `[DPMI]` lines say when a client enters and ends, INT 31h functions it
  doesn't have, and why it ended a program (`DPMI host: exception 0Dh
  ...` on the screen too): an exception the program didn't handle. `--no-config`
  has the host on; `dpmi=false` in a configuration file turns it off, to
  compare with an extender's own mode switching.
- **Exceptions:** `GET /api/exceptions` lists the last 64 (vector, error
  code, `CS:EIP`, CR2 for page faults). Page faults (`#PF`, vector 0E) are
  normal under DOS/4GW, whose virtual memory manager loads pages on demand.
  The log also has a line for each of the first 200 exceptions
  (`?grep=%23GP`; `#` must be written `%23` in a URL).
- **Break on an exception:** `POST /api/breakpoints {"exception":"0D"}`
  (or `"any"`) pauses at the first instruction of the handler after the CPU
  raises it; the pause reply includes the exception. `{"mode_switch":true}`
  pauses after each switch between real and protected mode.
- **Single-step traps:** a program that sets TF (some loaders decrypt
  themselves this way) gets an INT 1 after every instruction. These traps
  aren't in `/api/exceptions`, the log or exception breakpoints. To stop in
  the handler, find it in `/api/ivt` (or `/api/idt`) and set a breakpoint.
- **Tables:** `/api/gdt`, `/api/ldt` and `/api/idt` decode descriptors and
  gates; `/api/tss` shows the ring stacks and saved registers;
  `/api/pagewalk?addr=lin:00401000` shows the page directory and table
  entries; `/api/xms` lists XMS handles (where extenders put their memory).
- **Network:** `/api/net` shows the IPX driver (node, IRQ, open sockets,
  listening ECBs, events, completions waiting for its IRQ, held packets and
  counts of what became of packets) and the LAN (state, room, frames in and
  out). Completions that stay there mean the driver's IRQ doesn't reach its
  handler: look at the PICs in `/api/status` and the vector in `/api/ivt`.
  Two instances on one machine make a LAN: `LAN HOST 29931` in one and
  `LAN JOIN 127.0.0.1:29931` in the other, each with a debug port of its
  own. Stop each by its PID.
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
