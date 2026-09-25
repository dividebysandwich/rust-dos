# Rust-DOS

<img width="640" height="400" alt="image" src="https://img.playspoon.com/t9vbqf.gif" />

## Introduction

Rust-DOS is a DOS emulator aimed at the golden age of DOS gaming from the early days up to the dawn of Windows95. It is a work in progress and some games and programs may not run yet.

*Rust-DOS is looking for contributors!*

## Why

I wanted to learn more about the nuances of DOS emulation. Also, there's only one other DOS emulator written in Rust, and that one hasn't seen any development in 5 years and was using lots of unsafe{} code blocks.

## What works

* FPU emulation
* Interrupt handlers
* XMS 3.0 extended memory and the A20 gate, and EMS 4.0 expanded memory
* 386/486 protected mode, paging and virtual-8086 mode: DOS extenders such
  as DOS/4GW (Descent, Heretic)
* Sound Blaster 16, SB Pro 2 and SB 2.0 digital audio, OPL3 FM music, and
  General MIDI through the MPU-401 with a SoundFont or the Gravis patches
* Gravis Ultrasound: 32 wavetable voices, 1 MB of DRAM, DMA and timers, for
  both digital audio and music with built-in patch set.
* Graphics: text, CGA, EGA and VGA modes with Mode X support
* VESA VBE 2.0: 256-color, 15/16-bit and 32-bit modes from 320x200 to
  1024x768 with 4 MB of video memory, bank switching and linear frame
  buffer.
* Mounting host directories as floppy, hard disk and CD-ROM drives
* Mounting CD images (CUE sheets with BIN or WAV tracks, ISO, BIN and IMG)
  as CD-ROM drives, including DOSBox's `IMGMOUNT` command
* Mounting floppy and hard disk images (FAT12 and FAT16) for reading and
  writing, with sector access through INT 13h and INT 25h/26h, and lists of
  disks to change with Ctrl+F4
* Emulated disk speeds and disk drive noises
* CRT shaders: scanlines, an aperture grille or a curved shadow mask tube
  (see [CRT shaders](#crt-shaders))
* Monochrome monitors: white, amber or green phosphor
* Joysticks on the game port from game controllers (two sticks and four
  buttons), or the mouse as a joystick
* Configuration file with startup commands
* Environment variables (`SET`, `PATH`)
* Running in a web browser as WebAssembly, with C: kept in the browser's
  storage (see [Running in a browser](#running-in-a-browser))

## What partially works

* Programs using OVLs
* TSRs

## What's not implemented yet

* GUS MAX / Interwave codec, and SB emulation on the GUS (SBOS, MegaEm)

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
  * `fullscreen=true` fills the screen instead of a window, keeping the
    picture's proportions.
  * `aspect=true` stretches the picture to 4:3, the shape a monitor gave
    320x200 and 640x400.
  * `filter` is how the picture is scaled up: `nearest` (the default, sharp
    pixels) or `linear` (smooth). It applies without a CRT shader.
  * `shader` gives the picture a CRT look: `none` (the default),
    `scanlines`, `aperture` or `curved`. See [CRT shaders](#crt-shaders).
  * `monochrome` shows the picture on a monochrome monitor: `off` (the
    default, a colour monitor), `white`, `amber` or `green`. Each colour
    shows as bright as it is, in the phosphor's colour, and the CRT
    shaders leave out their colour mask. Screenshots and recordings are
    monochrome as well; the settings window stays in colour. With a VGA or
    an EGA (`machine`), programs see the monochrome monitor too, from the
    next DOS prompt on, and those that support one choose their monochrome
    graphics: a VGA reports an analog monochrome display (INT 10h AH=1Ah
    gives 07h), sums the colours its BIOS loads to grey and starts in the
    monochrome text mode 7; an EGA has IBM's Monochrome Display, and only
    modes 7 and 0Fh. On a CGA it is the look alone, and a Hercules card is
    always monochrome.
  * `cycles` is the CPU speed in instructions per millisecond. `max`, the
    default, runs as fast as the host keeps up with in real time. Use a
    number such as `3000` for old games that run too fast. `--cycles`
    overrides it.
  * `cpu` is the emulated processor: `486` (the default, a 486DX with FPU)
    or `386`.
  * `machine` is the display adapter programs find when they look for
    one, and so the graphics they choose: `svga` (the default, a VGA with
    VESA modes up to 1024x768), `vga` (an IBM VGA, without VESA modes),
    `ega` (an IBM EGA with an Enhanced Color Display: 16 of 64 colours at
    640x350, 60 Hz), `cga` (an IBM CGA: 4 colours at 320x200, 2 at
    640x200, 60 Hz) or `hercules` (a Hercules Graphics Card on a monochrome
    monitor: the MDA's text and 720x348 graphics, 50 Hz). A change takes
    effect at the DOS prompt.
  * `capture_dir` is the folder screenshots and recordings go in:
    `capture` (the default) in the directory rust-dos started in, or a
    path of your own. Screenshots show the picture as the recordings do:
    with the monochrome look, without a CRT shader or the settings window.
    Video recordings are AVI files in DOSBox's lossless ZMBV codec, which
    ffmpeg, VLC and mpv play, at 60 frames a second of the machine's time
    with its sound, so they keep in step through pauses and fast forward;
    a recording stops at 1 GB.
  * `memsize` is the RAM in MB, from 2 to 64 (default 16). Memory above the
    first megabyte is extended memory for DOS extenders and XMS.
  * `ems` gives programs expanded memory (LIM EMS 4.0, INT 67h), which
    many games of the early 1990s want: `true` (the default) or `false`.
    As with EMM386, 16 KB pages of extended memory show through a page
    frame at E000h, and EMS and XMS share the same memory. There is no
    VCPI: DOS extenders run as they do without EMM386. A change takes
    effect at the DOS prompt.
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
* **`[emulator]`** also has `hard_disk_speed` and `floppy_disk_speed`, and
  **`[sound]`** `hard_disk_noise` and `floppy_disk_noise` (see
  [Disk speed and noises](#disk-speed-and-noises)).
* **`[mixer]`:** the volume of each sound source in percent, from 0 to 200:
  `speaker` (the PC speaker and the prompt's beeps), `sb` (the Sound
  Blaster's digital audio), `fm` (the FM synthesizer), `gus`, `midi`,
  `cdaudio` and `disknoise`, and `master` for all of them together. At 100,
  the default, a source plays as loud as its card makes it. The volumes
  apply on top of the Sound Blaster's own mixer, which programs set.
* **`[joystick]`:** the game port, which programs read joysticks from.
  * `joysticktype` is what is plugged in: `auto` (the default), `4axis`,
    `2axis`, `mouse` or `none` (no game port).
    * `auto` depends on the game controllers connected (Xbox or
      PlayStation style, through SDL or the browser's Gamepad API). One
      controller is both joysticks and all four buttons, two controllers
      are a joystick each, and with none the mouse is joystick A.
    * `4axis` is one controller: the left stick is joystick A and the
      right stick joystick B (a flight simulator's rudder and throttle),
      and A, B, X and Y are buttons 1 to 4.
    * `2axis` is two controllers, each a joystick with two buttons (A and
      B).
    * `mouse` makes the mouse joystick A, its buttons the fire buttons.
    * On every controller the D-pad moves the left stick while the stick
      is at rest.
  * `deadzone` is how far a stick moves, in percent of its travel (0 to 90,
    default 10), before it counts, so a controller at rest reads as
    centred.
* **`[drives]`:** each line is `LETTER = PATH [more images] [floppy|hdd|cdrom] [-label NAME] [-ro] [-chs C,H,S]`.
  * PATH is a directory, or a disk or CD image (see [Drives](#drives)).
  * Relative paths are relative to the configuration file, and `~` is your
    home directory. Quote paths that contain spaces.
  * `-d/--dir` overrides C:. Without either, C: is the current working
    directory.
* **`[autoexec]`:** commands that run at the DOS prompt on startup, before
  `C:\AUTOEXEC.BAT`. A program started here delays the following lines until
  it exits.

Mistakes in the file are printed as warnings; the emulator still starts.

### Settings window

Press **Ctrl+F12**, or type `DOSCONFIG` at the DOS prompt, to open the
settings window over the running program. The program pauses while it is
open, except on the Mixer page, where it plays on so you hear the volumes
as you set them. Ctrl+F12 or Esc closes it.

* **Drives:** mount a host directory or a disk or CD image (Ins), change or
  swap the one a drive shows (Enter; this is how to change discs in the
  middle of a game), or unmount it (Del). **Browse...** picks directories
  and images from the host.
* **Display:** the scale, fullscreen, 4:3 aspect correction, the scaling
  filter, the CRT shader and the monochrome monitor.
* **Emulator:** the CPU speed, the processor, the video card, the memory
  size, expanded memory, the disk speeds and the joystick.
* **Sound:** everything in `[sound]`, and the disk noises.
* **Mixer:** the volume of each sound source and the master volume
  (`[mixer]`), with a meter of how loud each one plays.

Left and Right change a setting, Enter types or picks a value, Tab switches
pages, and the mouse works too. The display settings, the CPU speed, the
disk speeds and noises, the joystick and the volumes take effect at once. The processor, the video card, expanded memory and the sound hardware change once no
program is running, so a game isn't left without the card it set up. The memory size
takes effect the next time rust-dos starts.

**F2** (or Ctrl+S) saves the settings and drives to the configuration file
in use. Only what changed is written: comments, `[autoexec]`, the settings
you didn't touch and the file's own spelling of paths stay as they are, and
drives that the startup commands mount aren't copied into `[drives]`.

### CRT shaders

`shader` in `[emulator]`, or the settings window's Display page, shows the
picture as a monitor of the time would have:

* `scanlines`: a flat screen with the scanlines of a VGA monitor. Bright
  lines are wider than dark ones, and light glows a little around them.
* `aperture`: a flat aperture grille monitor, with red, green and blue
  phosphor stripes over the scanlines.
* `curved`: a curved tube with a shadow mask, rounded corners and darker
  edges. The mouse follows the curve.

With `monochrome`, the aperture grille and the shadow mask are left out, as
a monochrome tube has a single phosphor; the scanlines, glow and curve stay.

A VGA shows its 200-line modes double-scanned, so each of the 400 lines is a
scanline. The looks need a few screen pixels per line: at scale 1 the
scanlines and phosphors fade out, and they look best at scale 3 or more, or
in fullscreen on a large screen. At exactly 2x or 3x without 4:3 aspect
correction the scanlines are sharpest.

The shaders need OpenGL 3 (WebGL 2 in the browser). Without it, as with
`SDL_VIDEODRIVER=dummy`, rust-dos draws the picture with SDL's renderer as
before and says why at startup; the log names what draws the picture.
Recordings and debug-server screenshots always show the plain picture.

## Drives

C: and the built-in Z: always exist, and so does X: with the Ultrasound
patches unless the configuration moves or removes it. Mount more drives at
the prompt:

```
MOUNT                                     list drives
MOUNT A ~/dos/floppy                      mount a host directory
MOUNT D ~/dos/cd -t cdrom -label GAMECD   same, with -t for the type
MOUNT D ~/dos/game.cue                    mount a CD image
IMGMOUNT D C:\GAME\CD\GAME.CUE -t cdrom   the same, as DOSBox writes it
IMGMOUNT A disk1.img disk2.img            floppy images; Ctrl+F4 changes disks
IMGMOUNT C ~/dos/hdd.img                  a hard disk image as C:
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

### Disk images

A floppy or hard disk image holds a FAT12 or FAT16 file system that
programs read and write like any other drive's; what they write goes into
the image file. An image whose size is a floppy disk's (160 KB to 2.88 MB,
as in DOSBox's table) is a floppy, anything else a hard disk: an image of a
whole disk with a partition table, whose first FAT partition is the drive,
or of a single volume. The hard disk's geometry comes from its partition
table or boot sector; where it can't, `-chs C,H,S` (or DOSBox's
`-size 512,S,H,C`) gives it. `-t floppy` or `-t hdd` overrides the choice,
and an image file that can't be written, or `-ro`, makes a write-protected
disk. `IMGMOUNT C` puts a hard disk image in place of C:'s directory.

The BIOS sees the images as disks: INT 13h reads and writes their sectors by
cylinder, head and sector and reports their real geometry, and DOS's absolute
disk read and write (INT 25h/26h) reach the sectors of the volume. Programs
that check for their original disk with these find what's on the image.
Drives from host directories have no sectors; INT 13h reports success for
them without reading anything, which passes simple presence checks.

A list of images (`IMGMOUNT A disk1.img disk2.img disk3.img`, or the same
in `[drives]`) puts the first disk in the drive. **Ctrl+F4** changes every
drive with a list to its next disk, as in DOSBox; files a program has open
keep reading the disk they were opened on, and INT 13h's disk change line
tells the program another disk went in. CD image lists work the same way.

### Disk speed and noises

By default disks are as fast as the host. As in DOSBox Staging,
`hard_disk_speed` in `[emulator]` slows hard disks down to those of the
mid-1990s (`fast`, ~15 MB/s), the early 1990s (`medium`, ~2.5 MB/s) or the
1980s (`slow`, ~600 kB/s), and `floppy_disk_speed` floppies to extra-high
(`fast`, ~120 kB/s), high (`medium`, ~60 kB/s) or double density (`slow`,
~30 kB/s). Reading, writing, opening and loading files and INT 13h and
INT 25h/26h sector transfers take that long in emulated time; the machine
runs on meanwhile, so music and animations don't stop. CD-ROMs are not
slowed down.

`hard_disk_noise` and `floppy_disk_noise` in `[sound]` add the drives'
noises, with DOSBox Staging's recordings: `seek-only` plays the heads
moving on each access, and `on` adds a hard disk spinning up and humming
and a floppy's motor running while it is in use. They sound best with a
disk speed below `maximum`.

Host files and directories whose names aren't valid 8.3 names get short
names the way DOSBox and Windows make them: the start of the name and a
number, counted in sorted order per directory. `Day Of The Tentacle.BIN`
and `Day Of The Tentacle.cue` are `DAYOFT~1.BIN` and `DAYOFT~2.CUE`. Both the
long and the short name open the file.

| Type | Behaves like |
|---|---|
| `hdd` (default) | A fixed disk. BIOS unit 80h and up. |
| `floppy` | A removable 1.44 MB disk whose free space reflects its files. |
| `cdrom` | A read-only drive that programs detect through MSCDEX (INT 2Fh AX=15xxh) and as a remote drive. From a CD image it is a whole disc: raw and cooked sector reads, the volume descriptors, the table of contents, and audio tracks that play through the Sound Blaster mixer's CD volume. From a directory, only its files are available. |

`-ro` makes any drive read-only.

A: and B: are always floppy drives, whatever type the mount gives, and
can't hold a CD or a partitioned hard disk image. Programs see them the way they see a real 1.44 MB drive:
in the BIOS equipment word and CMOS, as INT 13h units 0 and 1 with the
diskette parameter table of INT 1Eh, as removable drives, and with a
1.44 MB diskette's layout in the drive parameter block and IOCTL 440Dh
(device type 07h, FAT12), or a disk image's own layout. A disk on B: alone makes a two-drive machine
with A: empty.

## Log file

rust-dos logs what it does to `rust-dos.log` in the per-user configuration
directory (see the table above): the programs it loads and runs, DOS and
BIOS functions and I/O ports it doesn't emulate, failed file operations, CPU
exceptions and configuration warnings. Attach this file to bug reports. It
is replaced on every start and stops growing at 64 MB.

## Keyboard shortcuts

| Keys | What they do |
|---|---|
| Ctrl+F12 | Open or close the [settings window](#settings-window) |
| Ctrl+F4 | Put the next disk in the drives mounted from lists of images |
| Alt+Pause | Pause the machine, and resume it |
| Alt+F12 (held) | Fast forward: the machine runs up to eight times as fast, without sound |
| Ctrl+F11 | Slow the CPU down by a tenth (from `max`, from the speed it reached) |
| Ctrl+Shift+F11 | Speed the CPU up by a tenth |
| Ctrl+F8 | Turn the sound off and on (in the browser, the page's Sound button) |
| Ctrl+F10 | Capture the mouse for the program, and let it go; a click captures it too once a program uses the mouse |
| Alt+Enter | Switch between the window and fullscreen |
| Ctrl+F5 | Save a screenshot (PNG) |
| Ctrl+F6 | Start and stop recording the sound (WAV) |
| Ctrl+F7 | Start and stop recording video with sound (AVI) |
| PrintScreen | Start and stop recording an animation (GIF) |

A message at the top of the picture says what they did. A captured mouse
moves the program's cursor by its motion, so a game that turns with the
mouse keeps turning at the edge of the screen; the settings window, Alt+Pause
and leaving the window let it go.

## Running in a browser

`web/` builds Rust-DOS for the browser: the emulator as a WebAssembly module
and a page that runs it, in `web/www`. It needs the `wasm32-unknown-unknown`
Rust target and the `wasm-bindgen` program in the version `web/Cargo.lock`
has (the build script says which, and how to install it). `wasm-opt` from
binaryen is used if it is installed.

```sh
rustup target add wasm32-unknown-unknown
./web/build.sh
python3 -m http.server -d web/www
```

Then open http://localhost:8000/. `web/www` is the whole site: put it on any
web server that serves `.wasm` files as `application/wasm` (GitHub Pages,
nginx, a CDN). Releases include it as `rust-dos-<version>-web.zip`.

In the browser:

* C: is a 250 MB hard disk image that the browser keeps in its storage
  (IndexedDB), so it is there again the next time the page is opened. Only
  the parts of the disk that hold something take memory and storage.
* Drop files, folders or `.zip` archives on the page, or use *Add files* and
  *Add folder*, to copy them to C:. An archive goes in a directory named
  after it, unless its files are in one directory already. Long file names
  get DOSBox-style short names (`LONGRE~1.TXT`).
* Dropping a floppy image (`.img`, `.ima`, `.vfd`, `.flp`, `.dsk`) puts it
  in A:, a CD image (`.iso`) in a CD-ROM drive and a hard disk image in the
  next free drive. *Drives* inserts an image into a drive of your choice,
  ejects it, downloads C: or a floppy as an image file (which the rust-dos
  program can mount), and erases C:.
* *Settings* (or Ctrl+F12, or `DOSCONFIG`) opens the
  [settings window](#settings-window), as in the rust-dos program. Its
  Drives page inserts disk and CD images you pick (Ins, or Enter for a
  drive's next disk) and ejects them (Del); there is no window scale or
  fullscreen setting, as the page has its own *Fullscreen*. The CRT shaders
  need WebGL 2. F2 saves the settings to the `rust-dos.conf` that the
  browser keeps.
* *Config file* edits that `rust-dos.conf` as text, `[autoexec]` included,
  and restarts the machine with it. `[drives]` has no host directories to
  mount there.
* *Screenshot* (or Ctrl+F5) downloads the screen as a PNG image, and
  *Record* (or Ctrl+F7) records it with its sound until pressed again,
  then downloads the video: WebM, or MP4 where the browser records no
  WebM. The recording shows the screen as the page does, CRT look
  included, and keeps the sound even with *Sound off*.
* Gamepads work as joysticks once a button is pressed on them, as browsers
  require (see `[joystick]`).
* Clicking the screen while a program uses the mouse captures the mouse;
  Ctrl+F10 or Esc releases it. Sound starts with the first key press or
  click, as browsers require. The other [keyboard shortcuts](#keyboard-shortcuts)
  work in the page too.

The page takes parameters: `?zip=URL` copies an archive to C: at startup,
`?run=COMMAND` types a command at the first prompt (both can repeat), for
example `?zip=games/keen.zip&run=cd%20keen&run=keen1`. `?persist=0` keeps C:
in memory only, `?log` sends the emulator's log to the browser console, and
`?renderer=2d` draws the screen without WebGL 2, and so without the CRT
shaders.

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
