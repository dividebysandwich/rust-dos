# Configuring Rust-DOS

Rust-DOS takes its settings from a configuration file, from the settings
window while it runs, and from game profiles for games that need settings
of their own. See the [README](README.md) for everything else.

* [Configuration file](#configuration-file): where it is, and every setting in
  [`[emulator]`](#emulator), [`[sound]`](#sound), [`[mixer]`](#mixer),
  [`[joystick]`](#joystick), [`[network]`](#network), [`[drives]`](#drives) and
  [`[autoexec]`](#autoexec)
* [Settings window](#settings-window)
* [Game profiles](#game-profiles)
* [Mounting drives](#mounting-drives), [disk images](#disk-images) and
  [disk speed and noises](#disk-speed-and-noises)
* [Playing over a LAN](#playing-over-a-lan)
* [CRT shaders](#crt-shaders)
* [Command-line options](#command-line-options)

## Configuration file

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

Mistakes in the file are printed as warnings; the emulator still starts.

### `[emulator]`

* `scale` is the window scale factor. `-s/--scale` overrides it.
* `fullscreen=true` fills the screen instead of a window, keeping the
  picture's proportions.
* `aspect=true` stretches the picture to 4:3, the shape a monitor gave
  320x200 and 640x400.
* `filter` is how the picture is scaled up: `nearest` (the default, sharp
  pixels) or `linear` (smooth). It applies without a CRT shader.
* `shader` gives the picture a CRT look: `none` (the default),
  `scanlines`, `aperture` or `crt`. See [CRT shaders](#crt-shaders).
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
* `composite` shows a CGA's graphics (`machine=cga`) as a composite
  monitor or TV did: fine patterns of pixels come out as colours, the
  "artifact colours" that King's Quest, the Ultima games, Sierra's AGI
  games and many more have a composite option for, 16 of them at
  640x200. `auto` (the default) does so when a program turns on 640x200
  graphics with the colour burst, which only programs for composite
  monitors do; `on` always does in the graphics modes (at 320x200 the
  palette's colours blend into others); `off` shows the RGB monitor's
  colours. `composite_era` is the card: `old` (the default, IBM's first
  CGA, which the classic games were drawn for) or `new`. The decoder is
  reenigne's model of the CGA's composite output, as DOSBox Staging has
  it. It applies at once.
* `cycles` is the CPU speed in instructions per millisecond. `max`, the
  default, runs as fast as the host keeps up with in real time. Use a
  number such as `3000` for old games that run too fast. `--cycles`
  overrides it.
* `cpu` is the emulated processor: `486` (the default, a 486DX with FPU),
  `386`, or `pentium`, a Pentium as DOSBox-X has one: CPUID (a
  GenuineIntel family 5), the time stamp counter, which counts the
  instructions and so runs at the `cycles` speed, its MSRs, CMPXCHG8B and
  4 MB pages, without the virtual-8086 mode extensions. A DOSBox
  configuration's `cputype` of a Pentium or later imports as `pentium`.
* `core` is what runs the programs' instructions: `auto` (the default)
  runs the interpreter, and the dynamic recompiler for a program from the
  moment it switches to protected mode until it ends, as DOSBox's
  `core=auto` does; `dynamic` always runs the recompiler, and `normal`
  the interpreter. The recompiler translates blocks of the program's code
  into the host's own and runs CPU-bound code several times faster. Both
  run programs exactly the same way, instruction for instruction, so
  there is no need to switch back for a game that misbehaves. The
  recompiler needs an x86-64 or ARM64 host; on others and in the browser
  the interpreter runs everything. `--core` overrides it. See
  [docs/dynrec.md](docs/dynrec.md).
* `machine` is the display adapter programs find when they look for
  one, and so the graphics they choose: `svga` (the default, a VGA with
  VESA modes up to 1024x768), `svga_s3` (an S3 Trio64 on the PCI bus,
  with its VESA modes, 2D accelerator and hardware cursor, for systems
  [booted from disk images](#booting-a-disk-image) whose drivers program
  it), `vga` (an IBM VGA, without VESA modes),
  `ega` (an IBM EGA with an Enhanced Color Display: 16 of 64 colours at
  640x350, 60 Hz), `cga` (an IBM CGA: 4 colours at 320x200, 2 at
  640x200, 60 Hz), `tandy` (a Tandy 1000), `pcjr` (an IBM PCjr) or
  `hercules` (a Hercules Graphics Card on a monochrome monitor: the MDA's
  text and 720x348 graphics, 50 Hz). A change takes effect at the DOS
  prompt.
* With `tandy` and `pcjr` programs find the machine they were made for
  (the model byte, and the Tandy's BIOS name) and its video: the CGA's
  modes and 16 colours at 160x200 and 320x200 (modes 08h and 09h), and 4
  at 640x200 (0Ah), from pages of system memory the page register picks
  (INT 10h AH=05h AL=80h-83h), with the palette registers (AH=10h), and
  the three-voice SN76489 sound chip (see `tandy` below). Their video
  memory is part of the 640 KB: DOS's memory ends at 624 KB on a Tandy,
  and starts above 144 KB on a PCjr, as DOSBox has it for Sierra's
  games. Not there: the Tandy 1000 SL/TL's DAC and 640x200 in 16
  colours, and the PCjr's cartridges.
* `voodoo=true` adds a 3dfx Voodoo Graphics card to any of the display
  adapters, for games that draw with Glide (see
  [3dfx Voodoo Graphics](#3dfx-voodoo-graphics)); `false` is the default.
  `voodoo_memory` is `12` (the default: 4 MB of frame buffer and two
  texture units with 4 MB each, as DOSBox-X's card has) or `4` (a retail
  board: 2 MB and one texture unit with 2 MB). Both take effect at the DOS
  prompt. `voodoo_renderer` is what draws the card's picture in the
  window: `software` (the default, rust-dos's own rasterizer) or `opengl`,
  which draws it again at `voodoo_scale` (1 to 4, 2 by default) times the
  card's resolution (see [3dfx Voodoo Graphics](#3dfx-voodoo-graphics));
  without OpenGL 3 (in the browser, or with SDL's dummy video driver) the
  software rasterizer's picture shows. Both change at once.
* `capture_dir` is the folder screenshots and recordings go in:
  `capture` (the default) in the directory rust-dos started in, or a
  path of your own. Screenshots (Ctrl+F5, in the settings window too)
  and recordings show the picture with the monochrome look but without
  the messages at the top. With `record_ui=true` they show the settings
  window and the performance overlay (Ctrl+Shift+F12) as well while they
  are open; `false`, the default, shows the picture alone.
  `record_shader=true` has screenshots and video recordings show the
  picture through the CRT shader, as the window does and at the size it
  shows it (without the black bars around it); `false`, the default,
  shows it plain. Drawing the picture again for them and reading it back
  takes the host some time a frame while a video records, and none
  otherwise. Animations (GIF) stay plain, as their 256 colours can't
  hold the shader's.
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
  VCPI: DOS extenders use the DPMI host (`dpmi`), or run as they do
  without EMM386. A change takes effect at the DOS prompt.
* `umb` gives DOS upper memory blocks between 640 KB and 1 MB, from
  D000h to EFFFh (to DFFFh with EMS), as DOS 5 with EMM386 has them:
  `true` (the default) or `false`. `LOADHIGH` (or `LH`) at the prompt
  loads a program there, so a TSR such as a mouse or sound driver stays
  out of the conventional memory games need, and programs can allocate
  upper memory themselves (INT 21h AH=58h). A change takes effect at the
  DOS prompt.
* `dpmi` gives DOS extenders a DPMI 0.9 host, as a memory manager or
  Windows provides one: `true` (the default) or `false`. DOS/4GW,
  PMODE/W, DOS/32A, Tran's PMODE and DJGPP's programs then run their
  programs as its clients rather than switching the processor
  themselves, and take extended memory from it as they need it, so a
  DOS/4GW program can start another DOS extender's (demos such as Scoop's
  Luminous do). Off, they run as they do on plain DOS with HIMEM. A
  change takes effect for the programs started after it.
* `keyboard_layout` is the layout the keyboard types in, as DOS's KEYB
  has them: `auto` (the default) takes the host keyboard's, or one of
  `us`, `uk`, `gr` (German, also `de`), `sg` and `sf` (Swiss German and
  French), `fr`, `be`, `it`, `sp` (`es`), `la` (Latin American), `po`
  (`pt`), `dk`, `no`, `sv` (`se`) and `su` (Finnish, `fi`). Keys send the
  scan codes of where they are, as on a real keyboard, so games that read
  the keyboard themselves find WASD where it is; AltGr types the third
  characters of the keys, and dead keys put their accents on the next
  letter. Characters code page 437 doesn't have (€, ø) type nothing.
  `KEYB` at the prompt changes it too.
* `rewind=true` keeps the machine's states of the last minutes, a state
  for every half second it runs, and holding **Alt+F11** goes back
  through them. `rewind_memory` is the memory they may take in MB (256
  by default; 16 to 4096): each state takes only what changed since the
  one after it, so a game that changes little goes back a long way. The
  states start over when a game starts or ends, the hardware changes or
  a [save state](README.md#save-states) is loaded. Off by default.
* `hard_disk_speed` and `floppy_disk_speed` slow the disks down to those
  of the time; see [Disk speed and noises](#disk-speed-and-noises).

### `[sound]`

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
  installer leaves them. The drive exists only if GUS is enabled.
* `ultradir` is the DOS directory of the Ultrasound software and patches.
  It defaults to `\ULTRASND` on `gusdrive` (`X:\ULTRASND`), or to
  `C:\ULTRASND` with `gusdrive=none`. To use patches of your own, set
  `gusdrive=none` and put them in `ultradir`.
* `midisynth` picks what plays the MPU-401's MIDI: `auto` (the default:
  the SoundFont if `soundfont` is set, else the Ultrasound patches listed
  in `ULTRASND.INI` in `ultradir`), `soundfont`, `gus`, `mt32`, `host`, or
  `none`. The built-in patches play even without the Ultrasound
  (`gus=false`) unless `gusdrive` is `none`.
* `mt32` plays a Roland MT-32 or CM-32L, emulated by
  [munt](https://github.com/munt/munt). rust-dos loads munt's library
  when the MT-32 is chosen, so it has to be installed: `munt` on Arch
  (from the AUR), `libmt32emu2` on Debian and Ubuntu, and `brew install
  mt32emu` on macOS; the Windows downloads come with it (`mt32emu-2.dll`
  beside `rust-dos.exe`, under munt's LGPL 2.1). `mt32lib` gives the
  library's path if the system doesn't find it. The MT-32's ROMs are not
  part of it: `mt32roms` is the directory with a control ROM and a PCM
  ROM of the same model, whatever the files are called. Without the
  setting rust-dos looks in `mt32-roms` in its configuration directory,
  in DOSBox's `mt32-roms`, and in `/usr/share/mt32-rom-data`.
  `mt32model` picks the ROMs to play with: `auto` (the CM-32L's if they
  are there, else the MT-32's), `mt32` or `cm32l`. What a game shows on
  the MT-32's display appears over the picture.
* `host` sends the MIDI out of a MIDI port of the computer (ALSA,
  CoreMIDI or Windows MIDI): to a real MT-32 or Sound Canvas, or to a
  software synthesizer such as FluidSynth or munt's. `midiport` is a part
  of the port's name or its number; the default is the first port (on
  Linux, the first other than Midi Through). Building rust-dos with it
  needs ALSA's development files on Linux (`libasound2-dev`); without the
  `hostmidi` feature it is left out.
* `lpt_dac` puts a DAC on the parallel port LPT1 (378h), which many games
  of the late 1980s and early 1990s play digital sound through: `none`
  (the default), `disney` (the Disney Sound Source, with its 16-byte
  FIFO played at 7 kHz) or `covox` (the Covox Speech Thing). Both sound
  through filters that give them the real devices' sound, as in DOSBox
  Staging. A change takes effect at the DOS prompt.
* `tandy` is the Tandy 1000's and PCjr's sound chip at port C0h, three
  square waves and noise: `auto` (the default) on those machines, `on`
  on any (for games that play Tandy sound with VGA graphics), or `off`.
* `hard_disk_noise` and `floppy_disk_noise` add the drives' noises; see
  [Disk speed and noises](#disk-speed-and-noises).

### `[mixer]`

The volume of each sound source in percent, from 0 to 200: `speaker` (the
PC speaker and the prompt's beeps), `sb` (the Sound Blaster's digital
audio), `fm` (the FM synthesizer), `gus`, `midi`, `cdaudio`, `disknoise`,
`lptdac` (the Covox or Disney Sound Source) and `tandy` (the Tandy's and
PCjr's sound chip), and `master` for all of them together. At 100, the
default, a source plays as loud as its card makes it. The volumes apply on
top of the Sound Blaster's own mixer, which programs set.

* `speaker_filter` gives the PC speaker the sound of the small speaker in
  a PC, without the harsh edges of its square wave: `on` (the default)
  or `off`.
* `sb_filter` is the Sound Blaster's output filter: `auto` (the default)
  filters as the model in `sbtype` does, at 4.8 kHz on an SB 2.0, 3.2 kHz
  on an SB Pro and half the sample rate on an SB16; `off` leaves the
  sound as the card makes it.
* `reverb` adds a room to the music of the FM synthesizer, the Gravis
  Ultrasound and MIDI: `off` (the default), `tiny`, `small`, `medium`,
  `large` or `huge`. `chorus` thickens them: `off` (the default),
  `light`, `normal` or `strong`. The presets and how much of the music
  they get follow DOSBox Staging's. `reverb_mix` and `chorus_mix` set
  how the music and each effect are mixed, in percent: `0` is the music
  dry, without the effect, `100` the effect alone, and at `50` (the
  default) both play in full; below 50 the effect fades out, above it
  the dry music does.
* The `MIXER` command shows the mixer and changes it at the prompt, or
  from `[autoexec]`, in DOSBox Staging's syntax:
  `MIXER [CHANNEL] COMMANDS [/NOSHOW]`. The channels are `MASTER`,
  `PCSPEAKER`, `SB`, `OPL`, `GUS`, `MIDI`, `CDAUDIO`, `DISKNOISE` and
  `LPTDAC` (DOSBox Staging's names for them work too), and the commands a
  volume (`0` to `200` percent, or decibels as `d-6`), `STEREO` or
  `REVERSE`, a crossfeed (`x0` to `x100`), and how much goes to the
  reverb and chorus (`r0` to `r100`, `c0` to `c100`, which turn them on
  if they are off). Without a channel, `x`, `r` and `c` change every
  channel; `MIXER /?` has the details. For example,
  `MIXER CDAUDIO 50 SB REVERSE /NOSHOW` or `MIXER X30 OPL 150 R50 C30`.
  The volumes and effects it sets are the settings window's, which F2
  saves.

### `[joystick]`

The game port, which programs read joysticks from.

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

### `[network]`

The IPX driver of the built-in DOS and the LAN of rust-dos instances (see
[Playing over a LAN](#playing-over-a-lan)).

* `ipx` installs the IPX driver: `auto` (the default) from the first
  `LAN HOST` or `LAN JOIN` on, `true` from the start, or `false` never.
* `ipxirq` is the IRQ its completions come in: `auto` (the default: the
  first of 11, 15, 10 and 9 that no sound card has) or 3, 4, 5, 7, 9, 10,
  11 or 15.
* `ipxframe` is how its packets go into Ethernet frames: `ethernet_ii`
  (the default), `802.3`, `802.2` or `snap`. It matters only for talking
  to the IPX protocol of a system booted on another instance.
* `lan` joins a room at startup, as `LAN JOIN` does: `off` (the default),
  `discover` (the first relay that answers on this network) or a relay's
  `host[:port]`.
* `lanhost` relays rooms from startup on a UDP port, and joins one there,
  as `LAN HOST` does: `off` (the default) or a port.
* `room` is the room to join (default `lobby`), and `password` the
  password the relay's rooms need. The password is kept in the file as
  it is written.

### `[drives]`

Each line is `LETTER = PATH [more images] [floppy|hdd|cdrom] [-label NAME] [-ro] [-chs C,H,S]`,
the options as `MOUNT` takes them. A drive number 0 to 3 instead of the
letter gives the BIOS a disk image without a DOS drive (see [Booting a disk
image](#booting-a-disk-image)).

* PATH is a directory, or a disk or CD image (see [Mounting drives](#mounting-drives)).
* Relative paths are relative to the configuration file, and `~` is your
  home directory. Quote paths that contain spaces.
* `-d/--dir` overrides C:. Without either, C: is the current working
  directory.

### `[autoexec]`

Commands that run at the DOS prompt on startup, before `C:\AUTOEXEC.BAT`,
as a [batch file](README.md#batch-files). A program started here delays the
following lines until it exits. The settings window's Emulator page edits
them.

## Settings window

Press **Ctrl+F12**, or type `DOSCONFIG` at the DOS prompt, to open the
settings window over the running program. The program pauses while it is
open, except on the Mixer page, where it plays on so you hear the volumes
as you set them, and the Stats page, which shows it running. Ctrl+F12 or
Esc closes it.

* **Drives:** mount a host directory or a disk or CD image (Ins), change or
  swap the one a drive shows (Enter; this is how to change discs in the
  middle of a game), or unmount it (Del). **Browse...** picks directories
  and images from the host. **Create a disk image...** makes a new, empty
  floppy or hard disk image, as [MAKEIMG](#new-disk-images) does, and
  mounts it on the drive picked (a floppy on A: or B:, where free).
* **Display:** the scale, fullscreen, 4:3 aspect correction, the scaling
  filter, the CRT shader and the monochrome monitor.
* **Emulator:** the CPU speed, the processor, the video card, the memory
  size, expanded and upper memory, the disk speeds, the joystick, rewind,
  the capture folder and whether recordings show the settings window and
  the performance overlay. *Edit the [autoexec] commands...* opens the configuration file's
  `[autoexec]` (a game's profile's while it plays) in an editor, comments
  included; F2 writes it into the file, for the next start, and Esc leaves
  the file as it was.
* **Sound:** everything in `[sound]`, and the disk noises. A card's port,
  IRQ and DMA channels share a row: Left and Right move between them, and
  Enter lists the values of the one marked.
* **Mixer:** the volume of each sound source and the master volume
  (`[mixer]`), with a meter of how loud each one plays, and the filters,
  reverb and chorus with their dry/wet mixes.
* **Games:** the [game profiles](#game-profiles): Enter launches one, Ins
  makes one from the settings as they are, Del deletes one.
* **States:** the [save state](README.md#save-states) slots of the game playing,
  with when each was saved, in which program, and a picture of the screen:
  Enter loads one, Ins saves to one, Del empties one. Ctrl+F9 opens the
  window on this page.
* **Cheats:** finds a game's values in memory, such as its lives or money,
  and changes them. Search for the value the game shows (or for any value,
  when the game shows none, such as an energy bar), close the window and
  play on until it changes, then narrow the addresses down: by the new
  value, or by whether it changed, went up or went down. Once few are
  left, Enter sets one's value, and Ins freezes it: the machine puts the
  value back before every frame until the program ends. Values are 8, 16
  or 32 bits, typed in decimal or hex (`0x1F`, `$1F`, `1Fh`), and the
  search covers conventional memory, or all of it for games with DOS
  extenders.
* **Stats:** how the machine runs, in two big numbers: **FPS**, the
  frames a second the program draws (a retrace after which the picture
  changed, or for a game that flips pages, each flip), beside the
  display's refresh rate, and **CPU**, how much of one host core the
  emulator takes (yellow from 75%, red from 90%, where it is close to not
  keeping up). Each has a graph of the last 30 seconds, with their
  average, least and most. Below them: the emulated CPU's speed in cycles
  and in millions of instructions a second, the share of them the dynamic
  recompiler ran, the share of the time the CPU sat halted waiting for an
  interrupt, how long drawing the picture takes, and the recompiler's
  translated code. **Ctrl+Shift+F12** (here or while a program runs)
  shows the two numbers and small graphs of them at the bottom right of
  the picture while the window is closed, and hides them again.

Left and Right step a setting through its values, and move a slider (the
volumes, the effects' mixes, the CRT's curvature and glow, the memory size
and the joystick's deadzone) along its bar, as do their ◄ and ►. Enter (or
a click on a setting's value) lists the values to pick from, or types or
picks one: the list takes Up and Down, and a letter jumps to the next
value starting with it. Tab switches pages, and the mouse works too. The
display settings, the CPU speed, the disk speeds and noises, the joystick
and the volumes take effect at once. The processor, the video card,
expanded and upper memory and the sound hardware change once no program is
running, so a game isn't left without the card it set up. The memory size
takes effect the next time rust-dos starts.

**F2** (or Ctrl+S) saves the settings of every page and the drives to the
configuration file in use. Each setting gets a line: those you changed get
their new values, and those the file has no line for yet are added with
the value they had without one. Comments, `[autoexec]`, the lines of the
settings you didn't touch (and their spelling of paths) and command-line
options you didn't change stay as they are, and drives that the startup
commands mount aren't copied into `[drives]`.

## Game profiles

A game can have settings of its own, and the commands that start it, in a
profile: a file in the `games` folder beside the configuration file (or,
in the browser, in the page's storage). It is a configuration file with a
`[game]` section for the game's name, the settings that differ from
`rust-dos.conf`'s, the drives it needs, and `[autoexec]` with the commands
that start it:

```ini
[game]
name=Commander Keen 4

[emulator]
cycles=10000

[drives]
C=~/dos/keen4

[autoexec]
C:
KEEN4E
```

A game set up for DOSBox becomes a profile with its **Import** row, or
`--import PATH` at startup (which also launches it): a GOG install's folder
(its `goggame-*.info` says how GOG runs DOSBox), a folder with DOSBox
configuration files (GOG's older `dosboxGame.conf` and
`dosboxGame_single.conf`), or one such file. The profile gets the CPU,
memory, video, sound, joystick and keyboard settings the configuration has,
the drives its `[autoexec]` mounts (CD images included), and its commands
but EXIT; what doesn't come across goes to the log.

The settings window's **Games** page lists the profiles. Enter launches a
game at the DOS prompt: its settings apply on top of the configuration's,
its drives are mounted, and its commands run. When they have run and the
game has returned to the prompt, the settings and drives are back as they
were. **Ins** makes a profile from the settings as they are now: only
those that differ from the configuration file's, and the drives mounted
since, go in it, with the game's name, its directory and the command that
starts it. While a game plays, F2 saves the settings window's changes to
its profile. `--game NAME` launches a game at startup, by its file name
(`keen4`) or its name, and so does `?game=NAME` in the browser.

## Mounting drives

Dropping something onto the window uses it: a GOG game's folder or a DOSBox
configuration file is [imported](#game-profiles) as a game and launched, a
folder or hard disk image is mounted on the first free drive from D:, a CD
image goes into the CD-ROM drive (or a new one), a floppy image into A:,
and a program or batch file is started from its folder. A zip archive is
unpacked into a folder of its own in the games folder; with a single
program to start the game (setup and install programs aside) it becomes a
game profile with that folder as C: and is launched, else the folder is
mounted to make a profile from.

C: and the built-in Z: always exist, and so does X: with the Ultrasound
patches unless the configuration moves or removes it. Mount more drives at
the prompt:

```
MOUNT                                     list drives
MOUNT A ~/dos/floppy                      mount a host directory
MOUNT D ~/dos/cd -t cdrom -label GAMECD   same, with -t for the type
MOUNT D ~/dos/game.cue                    mount a CD image
MOUNT D C:\GAME\CD\GAME.CUE               the same, by its DOS path
MOUNT A disk1.img disk2.img               floppy images; Ctrl+F4 changes disks
MOUNT A disk*.img                         the same, with a wildcard
MOUNT C ~/dos/hdd.img                     a hard disk image as C:
MOUNT -u A                                unmount (MOUNT A -u too)
A:                                        switch to drive A:
```

`MOUNT` takes DOSBox Staging's syntax, in which MOUNT took in `IMGMOUNT`:
the options can go anywhere on the line, and `IMGMOUNT` is the same
command, so the batch files made for either work unchanged. An image is
looked for by its DOS path first (`C:\GAME\CD\GAME.CUE`, or `GAME.CUE` in
the current directory) and then as a host path, and a directory on the host
first. A wildcard in the last part of a path (`disk*.img`, `CD\*.CUE`)
mounts the files that match as a list, in natural order (`DISK2` before
`DISK10`). Relative host paths are relative to the emulator's working
directory, or with `-pr` to the configuration file's folder. DOSBox's
`-freesize`, `-ide` and CD-ROM access options are taken and ignored;
overlays (`-t overlay`) aren't supported. A drive number instead of a
letter (`MOUNT 2 hdd.img`) mounts an image for the BIOS alone, as in
DOSBox (see [Booting a disk image](#booting-a-disk-image)). `MOUNT /?`
lists the options. Each drive keeps its own current directory, as in DOS.

A CD image always makes a read-only CD-ROM drive, labelled with the disc's
volume name unless `-label` says otherwise. It can be a CUE sheet (`.cue`,
or GOG's `.ins`, with BINARY, MOTOROLA or WAVE files, any number of tracks
and gaps) or a bare image of an ISO 9660 data track in 2048, 2336 or
2352-byte sectors (`.iso`, `.bin`, `.img`, GOG's `.gog`). Audio tracks can
be Ogg Vorbis, FLAC or MP3 files (FILE ... MP3, OGG or FLAC, or WAVE as
many sheets call them), and WAVE files at other rates: they are decoded to
CD audio as they play. Programs run from the image as from any other drive.

The type of a drive is the word after the path in `[drives]`, or `-t` at
the prompt:

| Type | Behaves like |
|---|---|
| `hdd` or `dir` (default) | A fixed disk. BIOS unit 80h and up. |
| `floppy` or `fdd` | A removable 1.44 MB disk whose free space reflects its files. |
| `cdrom` or `iso` | A read-only drive that programs detect through MSCDEX (INT 2Fh AX=15xxh) and as a remote drive. From a CD image it is a whole disc: raw and cooked sector reads, the volume descriptors, the table of contents, and audio tracks that play through the Sound Blaster mixer's CD volume. From a directory, only its files are available. |

`-ro` makes any drive read-only.

A: and B: are always floppy drives, whatever type the mount gives, and
can't hold a CD or a partitioned hard disk image. Programs see them the way
they see a real 1.44 MB drive: in the BIOS equipment word and CMOS, as
INT 13h units 0 and 1 with the diskette parameter table of INT 1Eh, as
removable drives, and with a 1.44 MB diskette's layout in the drive
parameter block and IOCTL 440Dh (device type 07h, FAT12), or a disk image's
own layout. A disk on B: alone makes a two-drive machine with A: empty.

Host files and directories whose names aren't valid 8.3 names get short
names the way DOSBox and Windows make them: the start of the name and a
number, counted in sorted order per directory. `Day Of The Tentacle.BIN`
and `Day Of The Tentacle.cue` are `DAYOFT~1.BIN` and `DAYOFT~2.CUE`. Both the
long and the short name open the file.

### Disk images

A floppy or hard disk image holds a FAT12 or FAT16 file system that
programs read and write like any other drive's; what they write goes into
the image file. An image whose size is a floppy disk's (160 KB to 2.88 MB,
as in DOSBox's table), or named `.vfd`, `.flp`, `.360`, `.720`, `.1200` or
`.1440`, is a floppy, anything else a hard disk: an image of a
whole disk with a partition table, whose first FAT partition is the drive,
or of a single volume. The hard disk's geometry comes from its partition
table or boot sector; where it can't, `-chs C,H,S` (or DOSBox's
`-size 512,S,H,C`) gives it. `-t floppy` or `-t hdd` overrides the choice,
and an image file that can't be written, or `-ro`, makes a write-protected
disk. `MOUNT C hdd.img` puts a hard disk image in place of C:'s directory.

The BIOS sees the images as disks: INT 13h reads and writes their sectors by
cylinder, head and sector and reports their real geometry, and DOS's absolute
disk read and write (INT 25h/26h) reach the sectors of the volume. Programs
that check for their original disk with these find what's on the image.
Drives from host directories have no sectors; INT 13h reports success for
them without reading anything, which passes simple presence checks.

A list of images (`MOUNT A disk1.img disk2.img disk3.img`, `MOUNT A
disk*.img`, or the same in `[drives]`) puts the first disk in the drive.
**Ctrl+F4** changes every drive with a list to its next disk, as in DOSBox;
files a program has open keep reading the disk they were opened on, and
INT 13h's disk change line tells the program another disk went in. CD image
lists work the same way.

### New disk images

`MAKEIMG` makes an empty disk image, as DOSBox Staging's command does,
once you answer Y to where it will go:

```
MAKEIMG floppy.img -t fd_1440kb -label MYDISK   a 1.44 MB floppy
MAKEIMG hdd.img -t hd -size 500                 a 500 MB hard disk
MAKEIMG C:\IMAGES\HDD120.IMG -t hd_120mb -d     in a DOS directory
MOUNT D hdd.img                                 then mount it
```

`-t` is the kind of disk: a floppy (`fd_160kb`, `fd_180kb`, `fd_320kb`,
`fd_360kb`, `fd_720kb`, `fd_1200kb`, `fd_1440kb`, `fd_2880kb`), a hard
disk of a preset size (`hd_20mb`, `hd_40mb`, `hd_80mb`, `hd_120mb`,
`hd_250mb`, `hd_520mb`, `hd_1gb`, `hd_2gb`), or `hd` with `-size` in MB
or `-chs C,H,S`. A floppy gets the layout DOS's FORMAT gives it; a hard
disk a partition table with one active partition from its second track,
whose file system is FAT12 up to 16 MB, FAT16 up to 2 GB and FAT32 above,
with clusters as small as that allows. `-fat 12`, `-fat 16` or `-fat 32`
and `-spc` (sectors per cluster) choose for themselves, `-label` names the
volume, and `-noformat` leaves the image all zeros. The file is a host
path (relative to the directory rust-dos started in, `~` for the home
directory), or with `-d` (`-writetodos`) a DOS path on a drive mounted
from a host directory. An existing file stays unless `-force` is given,
and one a drive has mounted always does. Rust-DOS mounts the FAT12 and
FAT16 images; FAT32 ones are for systems that read FAT32. The boot code
says the disk has no system until one is put on it; a hard disk's loads
its active partition's boot sector, as FDISK's does.

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

## Booting a disk image

`BOOT` starts a system of its own from a disk image, as DOSBox's command
does: MS-DOS, Windows 95 or another system a PC boots.

```
IMGMOUNT C ~/images/win95.img    the hard disk
BOOT -l C                        boot from it
BOOT disk1.img disk2.img         floppies in A:, Ctrl+F4 changes them
```

The built-in DOS steps aside: memory is cleared, the BIOS is all that is
left, and the boot sector of the disk in the drive `-l` names (A: by
default) runs. The BIOS has what such systems look for: the disks through
INT 13h with its extensions, the keyboard's buffer, the memory map (INT 15h
E820h), a Plug and Play BIOS, APM 1.2, and with `machine=svga_s3` a PCI
BIOS. BIOS services work in virtual-8086 mode and through the page tables of
the system that calls them. The first two floppy drives are always there,
empty or not; hard disk images are the drives C: and D:.

A drive number instead of a letter mounts a disk image for the BIOS alone,
as DOSBox's `IMGMOUNT 2 hdd.img` does: 0 and 1 are the floppy drives (INT 13h
units 00h and 01h), 2 and 3 the first two hard disks (80h and 81h). The
image needs no DOS file system: a booter game's floppy, a FAT32 or blank hard
disk to install a system on. It has no drive letter, and takes its unit
before A:, B: or the hard disk drives, which fill the hard disk units it
leaves. `BOOT -l 2` boots from it, and so does `BOOT -l C` where C: isn't a
disk image; `-fs none` with A: to D: means the numbers 0 to 3, and `MOUNT -u
2` unmounts it. The built-in DOS's programs reach it through INT 13h too.
`BOOT booter.img` puts a floppy without a file system in unit 0 this way.

```
IMGMOUNT 2 ~/images/win95-fat32.img    a hard disk DOS can't read
IMGMOUNT 0 install1.img install2.img   floppies to install from
BOOT -l A
```

A CD image mounted as a CD-ROM drive (`MOUNT D ~/dos/game.cue`) is an ATAPI
CD-ROM drive on the secondary IDE channel of the booted machine (ports
170h-177h and 376h, IRQ 15), as DOSBox-X's `-ide` has it. Windows 95 finds
it through the Plug and Play BIOS with its own driver, reads the disc and
plays its CD audio; Ctrl+F4 or another `MOUNT` changes the disc, and Windows
notices. With more than one, the first drive letter with an image is the
one.

Turning the machine off (Windows' **Shut Down**) brings back the DOS prompt;
restarting (Ctrl+Alt+Del, or Windows' **Restart**) boots the disk again.
Changes to the hardware settings wait for the prompt.

What the system writes goes into the image, so try things on a copy (on
btrfs or XFS, `cp --reflink=auto` takes no room). [Save
states](README.md#save-states) and rewind keep the disks in step with the
system's memory: in a run, the disks go back with a state through a journal
of what the system wrote over (the last 256 MB of it), and a state file has
a copy of each disk beside it (`slot1.C.img` for `slot1.state`, or
`slot1.2.img` for the disk mounted as 2, which takes
no room where the filesystem shares data between files) that loading it puts
back. The browser's slots can't hold disks and refuse a booted system's
state.

### Windows 95

A Windows 95 installed in DOSBox-X on its S3 card (`machine=svga_s3`)
starts as it would there, with the settings it was installed with:

```ini
[emulator]
machine=svga_s3
memsize=64
```

The S3 driver shows the desktop up to 1024x768 in 32 bits per pixel with
its accelerated drawing and hardware cursor; the Sound Blaster 16 plays
Windows' sounds and FM music; the PS/2 mouse and APM work (Shut Down turns
the machine off). MS-DOS Prompts run in a window or full screen
(Alt+Enter), and **Restart in MS-DOS mode** works. Windows keeps the
hardware it was installed on in its registry, so an image installed on
other hardware may look for devices this machine hasn't.

## 3dfx Voodoo Graphics

`voodoo=true` in `[emulator]` puts a 3dfx Voodoo Graphics in the machine,
the 3D card DOS and Windows 95 games of 1996-1998 draw with through Glide:

```ini
[emulator]
voodoo=true
cpu=pentium
```

The card is on the PCI bus as device 0, where DOSBox-X has it, with its
16 MB of registers, frame buffer and texture memory at D0000000h (BAR0).
Glide finds it through the PCI BIOS or ports CF8h/CFCh, programs it, and
turns its output on: from then on the screen shows the card's picture
instead of the VGA's, as a monitor on its pass-through cable does, until
the game ends or turns the output off. The picture goes through the
card's gamma table, which games set brighter than DOSBox-X shows them.

* **DOS games** bring Glide with them (linked in, or `GLIDE2X.OVL` beside
  the game) and run under a DOS extender such as DOS/4GW, which maps the
  card's memory. Tomb Raider's 3dfx patch (`3DPATCH\3DFX\TOMB.EXE` on the
  Tomb Raider Gold CD) runs with `voodoo=true` and `cpu=pentium`.
* **Windows 95** [booted from a disk image](#booting-a-disk-image) finds
  the card as a "PCI Multimedia Video Device", which needs no driver:
  games bring `glide2x.dll`, or install 3dfx's Glide runtime (and with it
  `fxmemmap.vxd`). SubCulture's 3dfx version installs from its CD in the
  [CD-ROM drive](#booting-a-disk-image) and plays with its CD music, with
  `machine=svga_s3`, `memsize=64` and `cpu=pentium`.

The card swaps its buffers at the vertical retrace a game asks it to wait
for, and reports the swaps still waiting and its busy state as the real
one does, so games that pace themselves by it run at its 60 Hz. The
triangles are drawn on up to four threads of their own: rust-dos keeps
running the game meanwhile, and the picture is the same however many
there are. Save states and rewind keep the card with everything in its
memory.

With `voodoo_renderer=opengl` the window shows the card's picture drawn
again with OpenGL, at `voodoo_scale` times the card's resolution: the same
triangles through the same pixel pipeline (textures, fog, blending, the
depth buffer and the gamma table), with sharper edges and textures and 8
bits a colour without the card's dithering. What games write into the
frame buffer themselves (menus, movies) stays at the card's resolution.
The software rasterizer goes on drawing the card's memory, which games
read back and save states keep, so it costs no less of the host's CPU;
screenshots and recordings show its picture, at the card's resolution (or
through the CRT shader at the window's size, with `record_shader=true`).
The CRT shaders draw their scanlines over the bigger picture.

## Playing over a LAN

rust-dos instances on different computers play DOS games over IPX as PCs
on one network would. The instances join a room of a relay, which passes
the network traffic of each on to the others over UDP; no network driver
or special privileges are needed on the host. One instance hosts the
relay, or a server does:

```text
LAN HOST                      on one computer
LAN JOIN                      on the others, on the same network
LAN JOIN 203.0.113.7          or at the host's address, over the internet
```

Then start the game's network play as usual (for Doom and Heretic,
`IPXSETUP -nodes 2`). `LAN` shows where the instance is: the IPX driver,
the room, the members in it and the round trip to the relay.

* **The IPX driver** is Novell's interface in the built-in DOS (INT 2Fh
  AX=7A00h), installed with the first `LAN HOST` or `LAN JOIN` unless
  `ipx` in [`[network]`](#network) says otherwise. It works for DOS
  programs and for protected-mode games that call it through their DOS
  extender, such as Descent (Doom and Heretic reach it through
  IPXSETUP); it is not there for systems booted from a disk image, nor
  inside Windows.
* **`LAN HOST [port]`** relays rooms on a UDP port (21213 unless given)
  and joins one. `LAN STOP` stops relaying. Over the internet, that port
  has to reach the host through its router.
* **`LAN JOIN [host[:port]]`** joins a room at a relay, or without an
  address the first relay that answers on this network. It waits a few
  seconds for the room (a key stops waiting; joining goes on). The
  instance keeps its place with keepalives, and joins again when the
  relay comes back after a restart. `LAN LEAVE` leaves.
* **Rooms** keep several games on one relay apart: `/ROOM:name`, or `room`
  in `[network]` (default `lobby`). `/PASSWORD:text` (or `password`)
  gives a relay's rooms a password, which the joining instances prove they
  know without sending it. Nothing else crossing the relay is encrypted.
* **A relay on a server** for playing over the internet without any
  player's router set up: `rust-dos --relay [PORT]` relays without
  starting the emulator, and so does `rust-dos-relay` (`--port`,
  `--bind`, `--password`, `--name`), a small program of its own that
  needs neither SDL nor a sound library:
  `cargo build --release --no-default-features --bin rust-dos-relay`.
* **What goes over the relay** is Ethernet frames, so the IPX driver
  plays with the IPX protocol of a system booted on another instance's
  network card, set to the same frame type (`ipxframe`, Ethernet II by
  default).

rust-dos's LAN is its own protocol: it doesn't reach DOSBox's IPX servers,
and `IPXNET` lines in imported DOSBox configurations are left out.

## CRT shaders

`shader` in `[emulator]`, or the settings window's Display page, shows the
picture as a monitor of the time would have:

* `scanlines`: a flat screen with the scanlines of a VGA monitor. Bright
  lines are wider than dark ones, and light glows a little around them.
* `aperture`: a flat aperture grille monitor, with red, green and blue
  phosphor stripes over the scanlines.
* `crt`: a curved tube with a shadow mask, rounded corners and darker
  edges. The mouse follows the curve. `crt_curvature` sets how far it
  bends, in percent: `0` is flat, `100` the most, `30` the default; and
  `crt_glow` how much light glows around its bright parts: `0` none,
  `100` the most, `20` the default. The Display page has both below the
  shader while the CRT look is chosen.

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
Screenshots and video recordings show the shader with `record_shader=true`
(see `capture_dir` in [`[emulator]`](#emulator)); animations and
debug-server screenshots always show the plain picture.

## Command-line options

Options on the command line override the configuration file's settings for
that run; the settings window saves only what you change in it. `rust-dos
--help` lists them.

| Option | What it does |
|---|---|
| `-d, --dir DIR` | Make the host directory `DIR` drive C: (default: the configuration's C:, or the current directory) |
| `-c, --config FILE` | Use this configuration file (see [Configuration file](#configuration-file)) |
| `--no-config` | Read and create no configuration file |
| `-s, --scale N` | The window scale factor, 1 to 16 |
| `--cycles N\|max` | The CPU speed (`cycles`) |
| `--core auto\|dynamic\|normal` | What runs the instructions (`core`) |
| `--game NAME` | Launch a [game profile](#game-profiles) at startup, by its file name or its name |
| `--import PATH` | Import a game set up for DOSBox as a game profile, and launch it |
| `--debug-server [ADDR]` | Start the [debug server](README.md#debug--remote-control-server) (default `127.0.0.1:8086`) |
| `--trace-capacity N` | Entries in the debug server's instruction trace (default 1,000,000) |
| `--relay [PORT]` | Relay [LAN](#playing-over-a-lan) rooms on this UDP port (default 21213) instead of starting the emulator |
| `--relay-password TEXT` | The password the rooms of `--relay` need |
