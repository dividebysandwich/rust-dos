# Rust-DOS

<img width="640" height="400" alt="image" src="https://img.playspoon.com/t9vbqf.gif" />

## Introduction

Rust-DOS is a DOS emulator. It is a work in progress and most programs don't run yet. It does however contain a good amount of CPU mnemonics and interrupts implemented, and can run simple programs.

*Rust-DOS is looking for contributors!*

## Why

I wanted to learn more about the nuances of DOS emulation. Also, there's only one other DOS emulator written in Rust, and that one hasn't seen any development in 5 years and was using lots of unsafe{} code blocks.

## What works

* Executing COM and EXE programs
* Basic disk operations
* Passthrough filesystem
* Mounting host directories as floppy, hard disk and CD-ROM drives
* Configuration file with startup commands
* CGA graphics
* FPU emulation
* Interrupt handlers

## What partially works

* AdLib sound
* VGA graphics
* Programs using OVLs
* TSRs

## What's not implemented yet

* Mounting disk images
* XMS/EMS
* IRQs and DMA
* Sound Blaster
* Gravis Ultrasound
* 640x480x16
* VESA modes

## Configuration

rust-dos reads a DOSBox-style configuration file. It uses the first file it
finds, and never merges files:

1. The file given with `-c/--config FILE`. If that file doesn't exist,
   rust-dos exits with an error.
2. `rust-dos.conf` in the current working directory.
3. `rust-dos.conf` in the per-user configuration directory:

   | Platform | Directory |
   |---|---|
   | Linux | `~/.config/rust-dos/` (or `$XDG_CONFIG_HOME/rust-dos/`) |
   | macOS | `~/Library/Application Support/rust-dos/` |
   | Windows | `%APPDATA%\rust-dos\` |

If none of these exists, rust-dos writes a commented template (a copy of
[`rust-dos.conf.example`](rust-dos.conf.example)) to the per-user directory.
The template changes nothing until you edit it. `--no-config` ignores all
configuration files.

```ini
[emulator]
scale=2
cycles=max

[drives]
C=~/dos
A=~/dos/floppy floppy -label DISK1
D="~/dos/My CD" cdrom -label GAMECD

[autoexec]
@ECHO OFF
D:
```

* **`[emulator]`:**
  * `scale` is the window scale factor. `-s/--scale` overrides it.
  * `cycles` is the CPU speed in instructions per millisecond. `max`, the
    default, runs as fast as the host keeps up with in real time. Use a
    number such as `3000` for old games that run too fast. `--cycles`
    overrides it.
  * `cpu` is the emulated processor: `486` (the default, a 486DX with FPU)
    or `386`.
  * `memsize` is the RAM in MB, from 2 to 64 (default 16). Memory above the
    first megabyte is extended memory for DOS extenders and XMS.
* **`[drives]`:** each line is `LETTER = PATH [floppy|hdd|cdrom] [-label NAME] [-ro]`.
  * Relative paths are relative to the configuration file, and `~` is your
    home directory. Quote paths that contain spaces.
  * `-d/--dir` overrides C:. Without either, C: is the current working
    directory.
* **`[autoexec]`:** commands that run at the DOS prompt on startup, before
  `C:\AUTOEXEC.BAT`. A program started here delays the following lines until
  it exits.

Mistakes in the file are printed as warnings; the emulator still starts.

## Drives

C: and the built-in Z: always exist. Mount more drives at the prompt:

```
MOUNT                                     list drives
MOUNT A ~/dos/floppy floppy               mount a host directory
MOUNT D ~/dos/cd -t cdrom -label GAMECD   same, with -t for the type
MOUNT -u A                                unmount
A:                                        switch to drive A:
```

Relative `MOUNT` paths are relative to the emulator's working directory.
Each drive keeps its own current directory, as in DOS.

| Type | Behaves like |
|---|---|
| `hdd` (default) | A fixed disk. BIOS unit 80h and up. |
| `floppy` | A removable 1.44 MB disk whose free space reflects its files. On A: and B: it is also a BIOS floppy drive (equipment word, INT 13h units 0-1). |
| `cdrom` | A read-only drive that programs detect through MSCDEX (INT 2Fh AX=15xxh) and as a remote drive. Only the directory's files are available: no raw sector reads or CD audio. |

`-ro` makes any drive read-only.

## Keyboard shortcuts

F12: Debug mode
PrintScreen: Toggle screen recording to a video file

## Debug & remote-control server

Start the emulator with `--debug-server` to expose a local HTTP/WebSocket
interface (default `127.0.0.1:8086`, or `--debug-server 127.0.0.1:9000`). It is
meant for scripted and LLM-driven debugging: taking screenshots, reading the
text screen, typing input, tracing instructions, setting breakpoints, and
inspecting registers and memory. It has no authentication and no encryption,
so keep it bound to localhost.

`GET /` lists every endpoint. Some examples:

```sh
curl localhost:8086/api/status
curl 'localhost:8086/api/screen/text?format=text'
curl -o screen.png localhost:8086/api/screenshot
curl -XPOST -H content-type:application/json -d '{"text":"dir\n"}' localhost:8086/api/input/type
curl -XPOST -H content-type:application/json -d '{"enabled":true}' localhost:8086/api/trace
curl 'localhost:8086/api/trace?last_ms=200&no_bios=true'
curl -XPOST -H content-type:application/json -d '{"addr":"1234:0100"}' localhost:8086/api/breakpoints
curl 'localhost:8086/api/control/wait?timeout_ms=10000'
curl 'localhost:8086/api/memory?addr=DS:SI&len=64'
curl -XPUT -H content-type:application/json -d '{"path":"/path/to/game"}' localhost:8086/api/drive/C
curl -XPUT -H content-type:application/json -d '{"path":"/path/to/cd","type":"cdrom"}' localhost:8086/api/drive/D
```

WebSocket streams:

* `/ws/events`: log lines, pause/resume, and video mode changes.
* `/ws/trace`: live instruction trace batches.
* `/ws/screen?fps=5`: PNG frames, sent only when the screen changes.
* `/ws/audio`: s16le 44.1 kHz mono PCM.
* `/ws/input`: input events.

To run without a window or sound device (for example under an agent), set
`SDL_VIDEODRIVER=dummy SDL_AUDIODRIVER=dummy`. The instruction trace ring holds
`--trace-capacity` entries (default 1,000,000, about 64 bytes each). It is
allocated only once tracing is enabled.
