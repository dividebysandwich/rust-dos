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
- **Free port:** `--debug-server 127.0.0.1:0` lets the system pick a free
  port. The emulator prints the address it got on stdout, as
  `[DEBUG] Debug server listening on http://127.0.0.1:<port>/`. Use this to
  run several instances side by side, or when 8086 may be taken:

  ```sh
  : > rust-dos.out
  SDL_VIDEODRIVER=dummy SDL_AUDIODRIVER=dummy \
    ./target/release/rust-dos --no-config -d /path/to/dos/files \
    --debug-server 127.0.0.1:0 > rust-dos.out &
  until H=$(grep -om1 '127\.0\.0\.1:[0-9][0-9]*' rust-dos.out) &&
        curl -sf $H/api/status >/dev/null; do
    kill -0 $! 2>/dev/null || { echo 'rust-dos exited'; break; }
    sleep 0.2
  done
  ```

  The address line prints before the emulator starts answering requests,
  so the loop also waits for `/api/status`. Emptying `rust-dos.out` first
  keeps the loop from reading a port left there by an earlier run, and
  from failing to find the file before the shell creates it.
- **Output:** the emulator log doesn't go to stdout. It goes to
  `rust-dos.log` in the per-user config directory, replaced on every start.
  Read it through `/api/log` instead, which can filter it. When running
  several instances, give each its own file with `--log FILE`, or they
  replace each other's log.

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
- **Find the highlighted item in a text-mode menu.** The text alone doesn't
  show which item is selected. The JSON reply of `/api/screen/text` (without
  `format=text`) has `attrs`: for each row, runs of cells with the same
  attribute as `{"col":19,"len":21,"attr":"71"}`. `col` and `len` count
  cells, one character of `lines` each; a box-drawing character is three
  bytes in UTF-8, so slice a line by characters. A selected item or field
  is usually a run of its own, such as blue on light grey (`71`) in a row
  of light grey on blue (`17`). `blink` says whether attribute bit 7
  blinks the character (when false it selects a bright background, so `F1`
  is blue on bright white), and `mono` is true for the MDA and Hercules,
  whose attributes are `07` normal, `70` reverse, `01` underline and `08`
  bright.
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
  card's BLASTER string for where it answers now, after a driver or a Plug
  and Play configuration moved it (null without a card, or while a
  ViBRA16C's audio device is inactive). To listen or analyse, record
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
- **Kill a stuck program** with `POST /api/control/reboot_shell`. It does
  what the settings window's close does: the batch file that started the
  program ends (with any `[autoexec]` lines not yet run), held keys come
  up, the launched game and its autoinput stop, and the prompt comes back
  on C: (else the lowest drive mounted) with every drive at its root
  directory. This is better than restarting the emulator.
  `POST /api/control/reboot` resets the machine as Ctrl+Alt+Del does: DOS
  starts over and runs its startup again.

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
- **Unknown fields** are refused with HTTP 400 naming the field and the
  ones the event has, and nothing in the request is queued:
  `{"key":"o","modifiers":["alt"]}` gets 400 and presses nothing, with
  "(did you mean `mods`?)" in the error. The other JSON bodies (control,
  breakpoints, watchpoints, memory, trace, ports, drives, state, speed)
  refuse unknown fields the same way.
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
  `HLE service 08h` marks one of the emulator's own ROM services, here the
  BIOS timer handler counting the tick. `HLE far service` lines are the
  Plug and Play BIOS's and APM's entry points, `HLE DPMI` lines the DPMI
  host's. `/api/disasm` names them the same.
- **Filters:**
  - `no_bios=true` hides real-mode and virtual-8086 code running in the
    BIOS segment F000, but keeps the `HLE` lines. Protected-mode code is
    never hidden, as its CS is a selector.
  - `cs=1000` keeps only one code segment.
  - `last_ms=500`, or `from_ms`/`to_ms`, select a time window.
  - `last_n=50000` gets the newest 50000 entries, or `limit` of them if
    `limit` is smaller. Without `last_n` a reply holds up to `limit`
    entries, 10000 unless given. When `limit` leaves out matching entries,
    the reply says `"truncated": true` (the `x-trace-truncated` header in
    text replies); with `since` it means the page stopped before the newest
    entry. A full ring of 1,000,000 entries is up to about 230 MB as text
    and 430 MB as JSON.
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
prompt is back, and `run` while a program runs, a batch file such as
AUTOEXEC.BAT has lines left, or PAUSE, CHOICE or EDIT waits, gets 409. So
does a `stop_at_entry` run whose program was closed before it reached its
entry point.

- **Queued behind a busy prompt:** `"queue":true` takes the command line
  while the prompt is busy and adds it after the batch lines waiting. The
  shell runs it once those lines have run and the prompt is back, and the
  request waits for its program as before (raise `timeout_ms`, or pass
  `"wait":false` and wait on the operation). Programs the batch lines start
  before it don't count: `stop_at_entry` stops at the entry of the
  program this command line starts. The run's `resumed` entry has
  `"queued":true` when the prompt was busy. Any stop before the line runs
  (a breakpoint, `program_start` stops on every program, a pause) ends the
  run's operation; the line still runs. A line dropped with the batch
  files (Ctrl+C at a batch line, the settings window closing the program)
  gets 422. `reboot_shell` drops the line too, and as a control request
  it supersedes the run.

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
S=$(curl -s $H/api/status | jq .seq)            # the journal's last entry
curl -s $J -XPOST -d '{"key":"q"}' "$H/api/input/key?wait=false"
curl -s "$H/api/control/wait?after=$S&timeout_ms=10000"   # the first stop after it, with the registers
curl -s "$H/api/disasm?count=10"                 # "=>" marks CS:IP, "*" marks breakpoints
curl -s "$H/api/memory?addr=DS:SI&len=64"
curl -s $J -XPOST -d '{"count":5}' $H/api/control/step
curl -s -XDELETE $H/api/breakpoints              # remove all breakpoints
curl -s -XPOST $H/api/control/resume
```

- **Why `wait=false`:** use it for input that should trigger a breakpoint.
  Otherwise the input call can block until it times out, because the
  emulator stops at the breakpoint before all of the input is delivered.
- **Why `after=`:** `/api/control/wait` without it answers with the stop
  the machine is in, or the next one, so a stop that came before the wait
  arrived is missed while the machine runs on. With `after=S` it answers
  with the first stop after journal entry S, at once if there is one. See
  "The journal" below.
- **A breakpoint for one stop:** `{"addr":"1000:010B","once":true}` goes
  once it stops the machine. The stop has `"breakpoint"` and `"once":true`.
- **Run to an address:** `POST /api/control/resume {"until":"1000:0120"}`
  runs to that address once, without adding a permanent breakpoint.
- **Step over:** `POST /api/control/step_over` runs a `CALL`, `INT`, `LOOP`
  or `REP` string instruction through to the instruction after it, and
  steps any other instruction. It replies when the machine stops there; a
  call that never returns times the request out (504, with the `op` to go
  on waiting on) and leaves the machine running.
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

### The journal: what happened, in order

The debug server keeps a numbered record of what the machine and the
debugger did: every stop, resume, program start and end, input delivered,
memory and register write, breakpoint change, mount, save and load, speed
change and the shutdown. A script, a test harness or an agent reads it to
tie a stop to the request that caused it, to catch stops that came before
it asked, and to go on after a dropped connection.

```sh
curl -s "$H/api/journal?after=0&limit=50"
curl -s "$H/api/journal?after=$S&type=paused&wait=30000"   # long poll for the next stop
```

- **Entries** have `seq` (1, 2, 3 ... with no gaps), `type`, `t_ms` (host
  time since start), `icount`, `emu_ns` (emulated time), `cause` (the
  request that did it, null for the machine or the window), `request_id`
  (that request's `X-Request-Id`) and `op` (the control request under way),
  then their own fields. A `paused` entry has what the stop's reply has:
  `reason`, `registers`, `cs_ip`, `breakpoint`, `watch`, `program`, `exit`
  and so on. `/api` lists the types.
- **Requests have numbers.** Every response has `X-Debug-Req`, and JSON
  object bodies `"req"`. Send `X-Request-Id: <your label>` (up to 128
  printable ASCII characters) to find your requests' entries by it. Answers
  from the machine have `state_seq` (the last entry that changed the
  machine), `stop_seq` (the stop it is in, or null), and, when the request
  appended entries, `seq`.
- **Operations.** `resume`, `step`, `step_over`, `run`, `reboot`,
  `reboot_shell` and input requests start an operation named by the
  request's number (`"op"` in the reply). `GET /api/ops/N?wait=30000` tells
  how it ended: `stopped` (with the stop), `done` (input delivered, or a
  `run` whose program started), `failed`, `superseded` (another control
  request took over, `superseded_by`; a superseded `run` no longer stops
  at its program's entry) or `cancelled`. `step`, `step_over`
  and `run` wait for their operation, unless the body has `"wait":false`
  (then 202 with `op`). `GET /api/control/wait?op=N` waits for one too.
  The latest 1024 ended operations are kept, an older one reads as 410;
  one still pending is kept until it ends. With 1024 input requests still
  being typed, the next is refused with 429.
- **Who stopped it:** a stop by `POST /api/control/pause` has `by`, the
  pause's number, so a client whose resume another client ended can tell.
- **Change only what you read.** `If-State-Seq: N` on a request (or
  `"if_state_seq":N` in a `/ws/input` message) carries it out only while
  the machine is still paused in the state of entry N: no entry since then
  changed it, no operation is under way, no input is being typed, the
  settings window is closed and Alt+F11 isn't rewinding. Otherwise the reply is 409 with
  `"precondition":"state"`. Read with `GET /api/memory` (its
  `X-Debug-State-Seq` header, in every format, or `state_seq` in the body
  with `format=json` or `format=base64`), then write with `If-State-Seq`;
  the write's reply has the new `state_seq` for the next one. Every refused precondition
  (deterministic mode's `paused`, strict match's `quiescent`, `state`) has
  the same body: `error`, `precondition`, `paused`, `stop_seq`,
  `state_seq`, `pending_ops`, `input_queued` and `settings_window`.
- **Streams that go on.** `ws://$H/ws/events?after=S` sends the entries
  after S, then new ones as they come, with no gap and no repeat; reconnect
  with the last `seq` seen. Log lines and video mode changes come too
  unless `volatile=false` (`log_after=N` replays the log lines after line
  N). If entries after S were dropped from the ring (it keeps 4096), the
  first message is `{"type":"gone","first":F}`, and `/api/journal` answers
  410 unless `allow_gap=true`. So does `GET /api/control/wait?after=S`,
  whose stop may be among them. An `after` past the last entry is a 400
  with `last` and `instance`: it is a cursor from another instance.
- **Headers once.** `X-Request-Id` or `If-State-Seq` sent twice is a 400.
  `If-State-Seq` on a request the server answers itself (the journal,
  `/api/ops`, `/api/log`, `/api/version`, waits with `after` or `op`, the
  page and the streams) is a 400 too.
- **Log lines** in `/api/log` have `n` (their number; `after_n=N&wait=MS`
  pages and long-polls from there), `at` (the journal's last entry then)
  and `cause`.
- **Instance:** `/api/version` has `instance`, which is new at every
  start. A journal from another instance numbers its entries anew.
- **Build:** `/api/version` and the journal's `started` entry have
  `build: {"commit", "commit_short", "dirty"}`: the git commit the program
  was built from, and `dirty: true` when a tracked file differed from it.
  `version` stays the same between releases, so record `build` to tell
  which source a session ran. All three are null when the program was
  built outside a git checkout (a crate tarball, a copied tree, no git).
  `rust-dos --version` prints the same commit.
- **Quit:** `POST /api/control/quit` turns the emulator off. The journal
  ends with `shutdown`, which long polls and streams get before the
  connection closes, and the process exits with code 0.

### Inspect or patch state

- **Registers:** `GET /api/registers`, or
  `PUT /api/registers {"ax":"1234","flags":"0246"}`. Strings are hex; JSON
  numbers are decimal.
- **Memory:**
  - Read with `GET /api/memory?addr=B800:0000&len=160`. `format` picks the
    reply: `hex` (the default) is a text dump in DEBUG's layout, `json` is
    `{"addr","len","hex":"41 1F ..."}` with the bytes in the form `PUT`
    takes, `base64` is `{"addr","len","base64"}`, and `raw` is the bytes as
    `application/octet-stream`. The JSON formats have `state_seq` in the
    body; every format has it in the `X-Debug-State-Seq` header, which is
    what `If-State-Seq` on the next write wants. Their `addr` is the linear
    address (physical only when the request gave a bare address), and a
    bare address in `PUT` is physical, so with paging on write back to the
    `addr` the read was given, not the one in the reply.
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
to it. Save before a step that is slow to reach (a menu path, a level) and
load to try it again. The files are the ones the Ctrl+F1 and Ctrl+F2 slots
keep; the slots and the settings window's States page load them as a plain
load does, without a reply to show `differences` in.

There are two kinds of save and load:

- **Plain** (the default) is for trying things by hand. It works while the
  machine runs, keeps the breakpoints so you can load again to hit one,
  and loads a state of other hardware or media, listing what differs.
- **Strict match** (`"strict_match":true` in the body) is for a script that
  goes back to a checkpoint and has to know the machine there is the one
  it saved. The load is refused unless the state's hardware settings and
  media match the running machine's, and it starts the debugger afresh, so
  nothing set before the load stops or types into the restored machine.
  Both work only while paused. With `--deterministic`, a run from a
  strict-match load repeats the run that saved the state (see
  "Deterministic runs").

Neither kind keeps the files on the host. A state records which folders
and images the drives were mounted from, and a load mounts them again,
but not what is in them: what a program wrote since stays written,
except for a booted system's disks (see "Booted systems" below). A
strict-match load checks the paths, that the images are there, and the
size and start of read-only images. For more, ask for `"check_files":
true`: a save then also records what is on the writable drives (each
file's size and SHA-256, and each writable image's SHA-256), and a load
with it names the files that changed since (see "What counts as the
same media"). A strict-match load refuses the state then; put the files
back or load another state.

#### Plain save and load

A plain save or load is carried out at the end of the frame, whether the
machine runs or is paused (in deterministic mode a load waits for a pause,
see "Deterministic runs"). A load puts in the hardware settings the state
was saved with, then the machine, which mounts the drives as the state had
them and unmounts the others; a state of another `memsize` is refused.
Breakpoints, watchpoints and `break_on` stops stay, and keys held down
through `/api/input` or the window go up.

When the state's hardware settings, media or deterministic start time
differ from the running machine's, the state is loaded all the same and
the reply lists what was different in `differences`. Here D: was mounted
and the speed set to 20000 after the save (paths shortened):

```sh
curl -s $J -XPOST -d '{"path":"/work/a.state"}' $H/api/state/load
```

```json
{
  "loaded": "/work/a.state",
  "format": 1,
  "emulator": "1.6.0",
  "header": {
    "version": "1.6.0",
    "saved": "2026-10-11 02:22:06",
    "game": null,
    "game_name": null,
    "program": "",
    "machine": "",
    "memsize": 16,
    "emulated_ns": 23917375333,
    "media": [
      {"drive": "C", "kind": "hdd", "read_only": false, "folder": "/work/c"}
    ]
  },
  "differences": [
    "emulator.cycles: the state has the default, this machine has 20000",
    "drive D: is mounted here (hdd /work/d); the state hasn't it"
  ]
}
```

After the load the machine runs at the state's speed (`auto`, the
default) and D: is unmounted, as the state had it.

#### Strict match

A strict-match save or load guarantees:

- **The machine is stopped between two instructions.** Both are refused
  with 409 unless the machine is paused (`POST /api/control/pause`) and no
  step or `run` command is still to stop it. A save is also refused while
  queued input (`/api/input`, typed text) is still being typed, which goes
  on while paused. The refusal has the reason and `"paused"`:

  ```json
  {"error": "can't do a strict-match load: the machine runs; pause it first (POST /api/control/pause)", "paused": false}
  ```

  The other reasons are `a step is pending`, `a run command waits for its
  program` and, for a save, `queued input is still being typed`.

- **The same hardware and media.** A load is refused with 409, and nothing
  is loaded, when the state's hardware settings or media differ from the
  running machine's, or the state has no record of its media (a state
  saved by a rust-dos from before media were recorded). The hardware
  settings compared are those a state brings: `machine`, `cpu`, `cycles`,
  `memsize`, `ems`, `umb`, `dos_high`, `monochrome`, the sound cards, and
  the Voodoo and PowerVR cards. The deterministic mode counts as well: a
  state saved with `--deterministic` differs from a machine without it,
  and the other way round, and two `--start-time`s differ. The reply
  lists every difference in the words a plain load uses:

  ```json
  {
    "error": "/work/a.state doesn't match this machine; nothing was loaded",
    "loaded": false,
    "unchanged": true,
    "differences": [
      "emulator.cycles: the state has the default, this machine has 20000",
      "drive D: is mounted here (hdd /work/d); the state hasn't it"
    ]
  }
  ```

  Set the speed with `/api/speed`, mount or unmount the drives, then load
  again.

- **A fresh debugger.** A load that goes through clears the breakpoints,
  watchpoints and `break_on` stops, the target of a step-over or of
  `resume {"until":...}`, and queued input, and drops a `resume`'s
  `until_ms` in deterministic mode. The operations under way (a `run`
  waiting for its program, queued input) are cancelled, and requests still
  waiting on any of it (`/api/control/wait`, input sent with `wait`) get
  409 with `{"error":"a save state was loaded"}`. The journal has
  `state_loaded`, then `debugger_reset` with what was cleared and the
  operations cancelled, then a `paused` entry with `"reason":"load"` and
  the loaded machine's registers. Keys held down through
  `/api/input` or the window are forgotten without a key-up, so the
  machine keeps the keys and mouse buttons it was saved with. The machine
  stays paused. The reply's `debugger_reset` says what was cleared; set
  the breakpoints you need again before resuming:

  ```sh
  curl -s $J -XPOST -d '{"path":"/work/a.state","strict_match":true}' $H/api/state/load
  ```

  ```json
  {
    "loaded": "/work/a.state",
    "format": 1,
    "emulator": "1.6.0",
    "header": {"version": "1.6.0", "saved": "2026-10-11 02:22:06", "...": "..."},
    "debugger_reset": {
      "breakpoints": 1,
      "watchpoints": 0,
      "break_on": [],
      "run_to": false,
      "step": false,
      "pause_waits": 0,
      "run": false,
      "input": 0,
      "held_keys": 0,
      "until_ms": false
    }
  }
  ```

- **Media read from the files.** A strict-match save or load reads every
  CUE sheet and hashes every read-only image again. Other saves and loads
  use the record made when the drives were last mounted or changed, and
  keep a read-only image's hash while the file's size and modification
  time stay the same, so they miss an image replaced by one of the same
  size and time (`cp -p`, `rsync -t`, an unpacked archive), or a CUE
  track file that appeared after the mount.

With `"check_files": true` on the save and on the load, a strict match
also guarantees:

- **The files on the writable drives as they were.** A `check_files` save
  (strict or plain) reads every file of each writable folder drive and
  every writable image, and records their sizes and SHA-256 hashes in
  the state. A `check_files` load of that state reads them again and
  compares: a save game written, a configuration changed or a file
  deleted since the save is a difference. A strict-match load is
  refused; a plain one goes through and lists them:

  ```json
  {
    "error": "/work/a.state doesn't match this machine; nothing was loaded",
    "loaded": false,
    "unchanged": true,
    "differences": [
      "drive C: file CONFIG.DAT: its contents differ from when it was saved",
      "drive C: file SAVES/GAME1.SAV: new since the save"
    ]
  }
  ```

  The check is off unless asked for. What a program wrote to a file
  rarely matters to the machine's state, which a state holds whole;
  reading and hashing every file of a large game folder on each save and
  load takes time; and other files on the drive (a log, another tool's
  output, a state saved there) show up as changes. Without
  `check_files` no file is read or hashed, a strict match is as
  described above, and a state's header has no file list. The window's
  slots (Ctrl+F keys), rewind and the libretro core's states never
  check.

  A `check_files` load of a state without the record (one saved without
  `check_files`, or by an older rust-dos) goes through, and its reply
  lists the drives it couldn't check in `unchecked`:

  ```json
  "unchecked": [
    "drive C: wasn't checked: the state doesn't record what is on it (saved without check_files, or by an older rust-dos)"
  ]
  ```

  A file that couldn't be read (another program had it locked, say), at
  the save or at the load, is a difference too, as its contents can't be
  compared.

#### What counts as the same media

Each mounted drive is compared by its letter (or number, for a disk
mounted by number) and:

- its type (`floppy`, `hdd`, `cdrom`) and whether it is read-only;
- the host folder it shows and its overlay folder or delta file, by path;
- the paths of its disk or CD images, the whole swap list in order, each
  CUE sheet followed by the track files it names;
- that each image is there, and for a read-only image (a CD's, a
  `read_only` mount's) its size and the SHA-256 of its first 64 KiB.

A writable image changes as the machine writes it (a dynamic VHD grows),
so its size is recorded but not compared. When the state was saved and
is loaded with `check_files`, these are compared as well:

- on a writable folder drive, every file and folder as the drive shows
  it (under an overlay, the folder below with the overlay's changes), by
  its path from the drive's root, its size and the SHA-256 of its
  contents: a file new, gone, of another size or with other contents is
  named, up to 20 a drive, and the rest are counted;
- the SHA-256 of each writable image, the whole file, and of a writable
  image's delta file.

A booted system's disks are left out, as the state keeps a copy of them
that the load puts back (see "Booted systems"). A state saved on a
writable drive leaves its own files out (`a.state`, and `a.C.img` and the
like beside it), but another state file there is a file like any other:
keep states outside the drives' folders, or the next save to them
counts as a change. Labels, the files' times
and read-only folders' contents aren't compared. Z: and X:, which the
machine makes itself, aren't listed.

#### Loading a state in another run

A state records a folder drive by its host folder, so a load in another
run whose C: shows another folder (a fresh copy for each run, say) finds
a different folder: a strict-match load refuses it, and a plain load
mounts the state's folder again. `"folders"` on a load names the folder
drives to take from other host folders, by letter, each an absolute path:

```sh
cp -r /runs/1/c /runs/2/c   # C: as it was when the state was saved
curl -s $J -XPOST -d '{"path":"/work/a.state","strict_match":true,"check_files":true,"folders":{"C":"/runs/2/c"}}' $H/api/state/load
```

The load then takes the state as if it had been saved with C: mounted
from `/runs/2/c`: the media are compared with that folder, a
`check_files` load compares the files in it with the state's record, and
the load leaves C: mounted from it, with the files the state had open
opened again there. A strict-match load with `check_files` thereby
loads the state only onto a copy holding what C: held at the save. The
reply and the journal's `state_loaded` entry have `folders`, the folders
taken; the reply's header keeps the folders the state records. A drive
the state hasn't as a folder drive (none, or one of disk images or with
an overlay), a path that isn't an existing folder, and a drive named twice
(`"C"` and `"c"`) are refused with 400 and nothing is loaded, as is a save
with `folders`. The folder is compared by its canonical path, as a
mounted drive's is, so a symbolic link to the copy matches it.

#### Replies

A save replies `{"saved": path, "format", "emulator", "header"}` and a load
`{"loaded": path, "format", "emulator", "header"}`, with `differences`
after a plain load that found some, `debugger_reset` after a strict-match
load, `unchecked` after a `check_files` load of a state without a
record of the files, and `folders` after a load that took some.

- `format`: the version of the file's layout.
- `emulator`: the version of the running rust-dos.
- `header`: what the file says about the state.
  - `version`: the rust-dos that saved it.
  - `saved`: when, in local time (`YYYY-MM-DD HH:MM:SS`).
  - `game`, `game_name`: the game launched from its profile, or null.
  - `program`: the program that ran, as DOS started it; empty at the prompt.
  - `machine`: the hardware settings, as configuration text with only the
    settings that aren't the defaults (empty if all are).
  - `memsize`: the memory in MB, which the loading machine must have.
  - `emulated_ns`: the emulated time.
  - `media`: a list of the mounted drives, each with `drive`, `kind`,
    `read_only`, and `folder`, `overlay` and `images` where it has them;
    each image has `path`, `size` (null if the file couldn't be read) and,
    if read-only, `head_sha256`. Null in a state saved before media were
    recorded. In a `check_files` save's state, a writable folder drive has
    `files_recorded`, the number of files and folders recorded (the list
    itself stays in the file), a writable image `sha256`, and a writable
    image's delta file `overlay_sha256` (`none` before the first write
    made the file).
  - `deterministic`: the start time of the mode's clock, only in a state
    saved with `--deterministic`.
- `differences`: a sentence for each setting, drive, image or file that
  differs.
- `unchecked`: a sentence for each writable drive whose files a
  `check_files` load couldn't compare, as the state doesn't record them.
- `debugger_reset`: `breakpoints`, `watchpoints`, `input` (queued input
  events) and `held_keys` count what was cleared; `break_on` lists the
  stops that were on (`exceptions`, `mode_switch`, `program_start`,
  `program_exit`); `run_to` (a step-over's or `resume until` target),
  `until_ms` and `step` are true if one was set; `pause_waits` counts
  `/api/control/wait` requests answered with 409. `step` and `run` are
  always false, as a strict-match load is refused while either is pending.

Errors:

- **400** `{"error", "loaded":false, "unchanged":true}`: the load failed (no
  such file, which reads `empty`, a file that isn't a state, a state of
  another `memsize`, a drive of the state that can't be mounted, a
  `folders` drive the state hasn't as a folder drive or a `folders`
  path that isn't a folder,
  RetroAchievements hardcore mode) and the machine, its
  settings and its hardware are as they were. A save that can't write its
  file, and a body without `path`, get a 400 with only `error`.
- **409** `{"error", "paused"}`: a strict-match save or load sent while
  the machine wasn't stopped, or a load of any kind in deterministic mode
  while the machine runs.
- **409** `{"error", "loaded":false, "unchanged":true, "differences"}`: a
  strict-match load of a state that doesn't match.

### Deterministic runs

`--deterministic` makes two runs of the same program with the same input
come out the same: the same instructions, memory and pictures at the same
emulated time, however fast the host is.

```sh
SDL_VIDEODRIVER=dummy SDL_AUDIODRIVER=dummy \
  ./target/release/rust-dos --no-config -d /path/to/game --debug-server \
  --deterministic --cycles 10000 --start-time "1995-04-11 12:34:56" &
OP=$(curl -s $J -XPOST -d '{"until_ms":5000}' $H/api/control/resume | jq .op)
curl -s "$H/api/control/wait?op=$OP&timeout_ms=600000"   # "reason":"time"
curl -s $J -XPOST -d '{"key":"enter"}' "$H/api/input/key?wait=false"
curl -s $J -XPOST -d '{"until_ms":8000}' $H/api/control/resume
```

- **Speed:** a fixed number of cycles. `--cycles` has to be a number;
  `auto` or `max` from the configuration, a game's profile or the
  settings window become 3000 or the speed the machine has, and
  `/api/speed` takes only a number.
- **Clock:** the real-time clock, DOS's date and time, file times and the
  BIOS tick count start at `--start-time` (default 1995-04-11 12:34:56)
  and run with emulated time, as does the creation date of a CD a booted
  system gets from a host folder.
- **Addresses:** the NE2000's address (without `mac=`) and the IPX
  driver's node address come from a generator seeded with the start
  time, so two instances on one LAN need different start times.
- **Starts paused:** with the debug server the machine starts paused
  (`"reason":"startup"`), so the input can be sent before anything runs.
  AUTOEXEC.BAT hasn't run yet, so a plain `run` gets 409. `run` with
  `"queue":true` resumes the machine and runs the command line right after
  AUTOEXEC.BAT ends, at the same emulated moment on every run:

  ```sh
  curl -s $J -XPOST -d '{"command":"GAME.EXE","queue":true,"stop_at_entry":true,"timeout_ms":600000}' $H/api/control/run
  ```

  Without `stop_at_entry` the machine runs on after the reply, and the
  next request is taken wherever the host's frame got to, so its journal
  entry (`icount`, `emu_ns`) differs from run to run.
- **Input:** input, and requests that change the machine (`run`, memory
  and register writes, mounts, `/api/speed`, loading a state, reboots),
  are taken only while the machine is paused or the settings window is
  open, which takes its input at once (HTTP 409 otherwise). Input is delivered at fixed points in emulated time, every 10
  ms, from the time the machine resumes: one scan code per point, and
  `wait` and `hold_ms` count emulated milliseconds. While the machine is
  paused the reply comes at once, with `"held":true`, as if `?wait=false`
  had been passed; `/api/ops/N` (N is the reply's `op`) is `done` once the
  input was delivered. Two runs with the same input and stops give the
  same journal, but for `t_ms`.
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
  mouse, network and serial links, the modification times of files a
  program writes to a host folder, and the end of a game launched from
  its profile still follow the host.
- **Save states:** a load is a change to the machine, so it is taken only
  while paused (409 with `{"error", "paused":false}` otherwise); a plain
  save is taken while running too. The header's `deterministic` has the
  start time. The machine's clock is that plus emulated time, so a state
  saved with another start time, or outside the mode, reads another time:
  a load lists it in `differences` (`deterministic: the state has no
  deterministic mode, this machine has a clock started at 1995-04-11
  12:34:56`) and a strict-match load refuses it. After a load the clock
  reads the start time plus the state's emulated time, input points come
  every 10 ms from there as in the run that saved the state, and the speed
  becomes the state's (for a state saved at `auto` or `max`, the speed it
  ran at then). Save with a strict match at an `until_ms` stop and load
  with a strict match, and the run from the state reaches the next stop as
  the run that saved it did.

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

### Where the emulator's time goes (profiling)

`POST /api/profile {"enabled":true,"duration_ms":30000}` samples which
part of the emulator runs and which guest code, 2000 times a second, and
counts port accesses, BIOS and DOS calls, interrupts and the recompiler's
work meanwhile. Send the input to measure, then read it once:

```sh
curl -s -X POST $H/api/profile -H 'content-type: application/json'      -d '{"enabled":true,"duration_ms":30000}'
curl -s -X POST $H/api/input/batch -H 'content-type: application/json' -d @input.json
curl -s "$H/api/profile?wait_ms=40000&format=text"
```

Read the `busy_pct` shares (of the time the emulator works, not waiting
for the next frame), then the hottest guest places with their
disassembly, the ports and the services. `?zone=io` (or `dynrec`, `hle`,
`io.vga`) narrows the places to what spent time there; `?format=folded`
gives flame graph stacks. To compare two builds, load the same save state
in each and send the same input. [profiling.md](profiling.md) has the
whole report.

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
  The log also has lines for exceptions (`?grep=%23GP`; `#` must be
  written `%23` in a URL): the first 3 at each place (`CS:EIP`), at the
  first 200 places for each exception. Systems raise many exceptions on
  purpose from a few places, such as Windows 9x's ARPL breakpoints, the
  ports and IOPL-sensitive instructions it emulates in virtual-8086 mode
  and the pages it fills on demand; the limit per place keeps them from
  hiding the rest.
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
  sees its timer interrupts (`HLE service 08h` or its own handler) in
  instruction terms, but a timer interrupt can arrive between any two steps.
- **Measuring speed.** `/api/stats` reports `mips` (host speed while
  executing guest code), `emulated_mips` (guest instructions per wall-clock
  second) and the decode-cache hit rate, averaged over about a second. Its
  `frame` object has the settings window's Stats page numbers and the
  emulated speed over the last quarter second: the game runs at full speed
  when `emulated_speed` (emulated seconds a wall second) stays near 1,
  `frame.fps` is the frames the program draws a second (the top-level `fps`
  counts the window's frames), and `cpu_use` is the percent of the wall
  time the emulator's main thread works (running the machine and drawing
  the picture), so 100 minus it is that thread's time to spare, on one host
  core. Use a
  release build and a CPU-bound scene, such as the Stunts intro, when
  comparing emulator changes.
- **Coarse timestamps.** Trace times are sampled once per frame (about
  16 ms). Use `icount` for exact ordering.
- **Ring buffer size.** The trace ring holds 1,000,000 instructions by
  default (`--trace-capacity`). That is only a few frames of a busy
  program, so query soon after reproducing a problem.
- **Streams are for tools, not agents.** `/ws/trace` sends about 7 MB/s and
  `/ws/screen` sends PNGs. Don't pipe them into your context. Prefer HTTP
  polling, `/api/journal?wait=` long polls, or `/ws/events` for the
  journal's entries, log lines and video-mode changes.
- **Emulator gaps.** Many DOS features are incomplete; see the README. When a
  program misbehaves, the emulator is the likelier culprit. The trace plus
  the log usually shows which interrupt function or port is missing.
