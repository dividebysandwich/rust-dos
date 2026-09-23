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
* Mounting CD images (CUE sheets with BIN or WAV tracks, ISO, BIN and IMG)
  as CD-ROM drives, including DOSBox's `IMGMOUNT` command
* Configuration file with startup commands
* Environment variables (`SET`, `PATH`)
* XMS 3.0 extended memory and the A20 gate
* 386/486 protected mode, paging and virtual-8086 mode: DOS extenders such
  as DOS/4GW (Descent, Heretic)
* Sound Blaster 16, SB Pro 2 and SB 2.0 digital audio, OPL3 FM music, and
  General MIDI through the MPU-401 with a SoundFont or the Gravis patches
* Gravis Ultrasound: 32 wavetable voices, 1 MB of DRAM, DMA and timers, for
  both digital audio and music. The Gravis MIDI patch set is built in and
  appears on drive X:, where `ULTRADIR` points, so games need no Ultrasound
  installation
* CGA graphics
* FPU emulation
* Interrupt handlers

## What partially works

* VGA graphics
* Programs using OVLs
* TSRs

## What's not implemented yet

* Mounting floppy and hard disk images
* EMS
* GUS MAX / Interwave codec, and SB emulation on the GUS (SBOS, MegaEm)
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
* **`[sound]`:**
  * `sbtype` is the Sound Blaster: `sb16` (the default), `sbpro2`, `sb2` or
    `none`. `sbbase` (hex), `irq`, `dma` and `hdma` (the SB16's 16-bit
    channel) set its resources; the defaults are 220, 7, 1 and 5. The
    `BLASTER` environment variable follows them.
  * `opl` is the FM synthesizer: `opl3` (the default) or `opl2`.
  * `soundfont` is a General MIDI SoundFont (`.sf2`) for music programs
    send to the MPU-401 at 330h.
  * `gus` installs the Gravis Ultrasound: `true` (the default) or `false`.
    `gusbase` (hex: 210, 220, 240, 250 or 260), `gusirq` and `gusdma` set
    its resources; the defaults are 240, 5 and 3. The `ULTRASND` and
    `ULTRADIR` environment variables follow them; drivers that program the
    card's IRQ and DMA latches themselves get what they ask for.
  * `gusdrive` is the drive letter (D to Y) of the Gravis patch set built
    into rust-dos, or `none`. The default is `X`. The drive is read-only and
    holds the patches with `ULTRASND.INI` in `\ULTRASND`, as the Gravis
    installer leaves them. It is there when the Ultrasound is.
  * `ultradir` is the DOS directory of the Ultrasound software and patches.
    It defaults to `\ULTRASND` on `gusdrive` (`X:\ULTRASND`), or to
    `C:\ULTRASND` with `gusdrive=none`. To use patches of your own, set
    `gusdrive=none` and put them in `ultradir`.
  * `midisynth` picks what plays the MPU-401's General MIDI: `auto` (the
    default: the SoundFont if `soundfont` is set, else the Ultrasound
    patches listed in `ULTRASND.INI` in `ultradir`), `soundfont`, `gus`, or
    `none`. The built-in patches play even without the Ultrasound
    (`gus=false`) unless `gusdrive` is `none`.
* **`[drives]`:** each line is `LETTER = PATH [floppy|hdd|cdrom] [-label NAME] [-ro]`.
  * PATH is a directory or a CD image (see [Drives](#drives)).
  * Relative paths are relative to the configuration file, and `~` is your
    home directory. Quote paths that contain spaces.
  * `-d/--dir` overrides C:. Without either, C: is the current working
    directory.
* **`[autoexec]`:** commands that run at the DOS prompt on startup, before
  `C:\AUTOEXEC.BAT`. A program started here delays the following lines until
  it exits.

Mistakes in the file are printed as warnings; the emulator still starts.

## Drives

C: and the built-in Z: always exist, and so does X: with the Ultrasound
patches unless the configuration moves or removes it. Mount more drives at
the prompt:

```
MOUNT                                     list drives
MOUNT A ~/dos/floppy floppy               mount a host directory
MOUNT D ~/dos/cd -t cdrom -label GAMECD   same, with -t for the type
MOUNT D ~/dos/game.cue                    mount a CD image
IMGMOUNT D C:\GAME\CD\GAME.CUE -t cdrom   the same, as DOSBox writes it
MOUNT -u A                                unmount
A:                                        switch to drive A:
```

Relative `MOUNT` paths are relative to the emulator's working directory.
Each drive keeps its own current directory, as in DOS.

A CD image always makes a read-only CD-ROM drive, labelled with the disc's
volume name unless `-label` says otherwise. It can be a CUE sheet (with
BINARY, MOTOROLA or 16-bit stereo 44.1 kHz WAVE files, any number of tracks
and gaps) or a bare image of an ISO 9660 data track in 2048, 2336 or
2352-byte sectors (`.iso`, `.bin`, `.img`). `IMGMOUNT` takes DOSBox's
syntax, so the batch files made for DOSBox work unchanged: it looks for the
image by its DOS path first (`C:\GAME\CD\GAME.CUE`) and then as a host
path. Programs run from the image as from any other drive.

Host files and directories whose names aren't valid 8.3 names get short
names the way DOSBox and Windows make them: the start of the name and a
number, counted in sorted order per directory. `Day Of The Tentacle.BIN`
and `Day Of The Tentacle.cue` are `DAYOFT~1.BIN` and `DAYOFT~2.CUE`. Both the
long and the short name open the file.

| Type | Behaves like |
|---|---|
| `hdd` (default) | A fixed disk. BIOS unit 80h and up. |
| `floppy` | A removable 1.44 MB disk whose free space reflects its files. On A: and B: it is also a BIOS floppy drive (equipment word, INT 13h units 0-1). |
| `cdrom` | A read-only drive that programs detect through MSCDEX (INT 2Fh AX=15xxh) and as a remote drive. From a CD image it is a whole disc: raw and cooked sector reads, the volume descriptors, the table of contents, and audio tracks that play through the Sound Blaster mixer's CD volume. From a directory, only its files are available. |

`-ro` makes any drive read-only.

## Log file

rust-dos logs what it does to `rust-dos.log` in the per-user configuration
directory (see the table above): the programs it loads and runs, DOS and
BIOS functions and I/O ports it doesn't emulate, failed file operations, CPU
exceptions and configuration warnings. Attach this file to bug reports. It
is replaced on every start and stops growing at 64 MB.

## Keyboard shortcuts

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
* `/ws/audio`: s16le 44.1 kHz stereo PCM, after a JSON format header.
* `/ws/input`: input events.

To run without a window or sound device (for example under an agent), set
`SDL_VIDEODRIVER=dummy SDL_AUDIODRIVER=dummy`. The instruction trace ring holds
`--trace-capacity` entries (default 1,000,000, about 64 bytes each). It is
allocated only once tracing is enabled.
