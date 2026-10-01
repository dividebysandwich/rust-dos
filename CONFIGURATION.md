# Configuring Rust-DOS

Rust-DOS takes its settings from a configuration file, from the settings
window while it runs, and from game profiles for games that need settings
of their own. See the [README](README.md) for everything else.

* [Configuration file](#configuration-file): where it is, and every setting in
  [`[emulator]`](#emulator), [`[sound]`](#sound), [`[mixer]`](#mixer),
  [`[joystick]`](#joystick), [`[network]`](#network), [`[serial]`](#serial),
  [`[achievements]`](#achievements), [`[drives]`](#drives) and
  [`[autoexec]`](#autoexec)
* [Settings window](#settings-window)
* [Game profiles](#game-profiles) and [RetroAchievements](#retroachievements)
* [Mounting drives](#mounting-drives), [disk images](#disk-images) and
  [disk speed and noises](#disk-speed-and-noises)
* [Playing over a LAN](#playing-over-a-lan), [serial and modem
  games](#serial-and-modem-games) and [a relay on a
  server](#a-relay-on-a-server)
* [CRT shaders](#crt-shaders)
* [Command-line options](#command-line-options)

## Configuration file

rust-dos reads a DOSBox-style configuration file. It uses the first file it
finds, and never merges files:

1. The file given with `-c/--config FILE`. If that file doesn't exist,
   rust-dos exits with an error.
2. `rust-dos.conf` in the current working directory.
3. `rust-dos.conf` in the directory holding the rust-dos executable. Put one
   there to make a portable install that leaves the user profile alone.
4. `rust-dos.conf` in the per-user configuration directory:

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
cycles=auto

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
* `cycles` is the CPU speed in instructions per millisecond. `auto`, the
  default, finds the speed the running program needs from the frames it
  draws (its page flips, or the bursts of writes that copy a finished
  picture to video memory): it goes up while the frame rate follows it,
  stops where the frame rate stops following (a game that caps its own
  frame rate) or reaches the display's refresh rate (frames nobody sees,
  and some games, such as Descent, play badly at such rates), and goes
  down while the program waits for a key or calls DOS's idle interrupt.
  A real-mode program with no frames to go by (most games of the 1980s,
  which update the screen piecemeal) runs at 3000, about a 286, as in
  DOSBox's `cycles=auto`; a protected-mode one (loading, computing,
  Windows) runs at max. `auto 10000` makes that real-mode speed, and
  the least `auto` goes down to, 10000. `max` runs everything as fast as
  the host keeps up with in real time. Use a number such as `3000` for a
  fixed speed. `--cycles` overrides it.
* `cpu` is the emulated processor: `486` (the default, a 486DX with FPU),
  `386`, or `pentium`, a Pentium as DOSBox-X has one: CPUID (a
  GenuineIntel family 5), the time stamp counter, which counts the
  instructions and so runs at the `cycles` speed, its MSRs, CMPXCHG8B and
  4 MB pages, without the virtual-8086 mode extensions; or `pentium_mmx`,
  that Pentium with MMX (CPUID family 5, model 4), whose eight MM
  registers are the FPU's. A DOSBox configuration's `cputype` of a
  Pentium or Pentium Pro imports as `pentium`, of a Pentium MMX or later
  as `pentium_mmx`.
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
  it), `svga_s3virge` and `svga_s3virgevx` (an S3 ViRGE or ViRGE/VX on
  the PCI bus: the Trio64's registers with the ViRGE's own 2D engine, 3D
  engine and streams processor, which Windows 95's Direct3D uses through
  S3's driver; see [Direct3D](#direct3d-on-an-s3-virge)), `svga_et4000`
  (a Tseng Labs ET4000AX with 1 MB and a Sierra HiColor DAC, see below),
  `vga` (an IBM VGA, without VESA modes),
  `ega` (an IBM EGA with an Enhanced Color Display: 16 of 64 colours at
  640x350, 60 Hz), `cga` (an IBM CGA: 4 colours at 320x200, 2 at
  640x200, 60 Hz), `tandy` (a Tandy 1000), `pcjr` (an IBM PCjr) or
  `hercules` (a Hercules Graphics Card on a monochrome monitor: the MDA's
  text and 720x348 graphics, 50 Hz). A change takes effect at the DOS
  prompt.
* With `svga_et4000` programs find a Tseng ET4000AX on the ISA bus, as
  DOS games and Windows 3.x drivers made for it detect it: its KEY
  (3BFh/3D8h), Segment Select (3CDh) with separate read and write banks,
  the extended CRTC registers (CR33 display start, CR35 and CR3F
  overflows, clock select) and the attribute controller's register 16h.
  Its 1 MB is the VGA's planes, so the VGA's write modes and latches work
  in every mode. Tseng's BIOS modes are there through INT 10h AH=00h:
  text at 132x44, 132x25 and 132x28 (22h-24h), 80x60 (26h) and 100x40
  (2Ah); 800x600 and 1024x768 in 16 colours (29h, 37h); 640x350,
  640x480, 640x400, 800x600 and 1024x768 in 256 colours (2Dh, 2Eh, 2Fh,
  30h, 38h). The Sierra SC11487 DAC adds 32K and 64K colours at 320x200,
  640x350 to 640x480 and 800x600, through its command register (four
  reads of 3C6h) or Tseng's INT 10h AX=10F0h-10F2h. The BIOS has VBE 1.2
  as Tseng's later ones do: modes 100h-105h and 15 and 16-bit 10Dh,
  10Eh, 110h, 111h, 113h and 114h, through a 64 KB window at A0000h,
  with no linear frame buffer, 24-bit colour or VBE 2.0 functions.
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
* `memsize` is the RAM in MB (default 16), from 2 to as much as
  motherboards took for the `cpu`: 64 MB for a 386, 128 MB for a 486,
  256 MB for a Pentium and 512 MB for a Pentium MMX. More than the CPU
  takes is warned about and cut to its most. Memory above the first
  megabyte is extended memory for DOS extenders and XMS; XMS 3.0's 32-bit
  functions and INT 15h E801h/E820h report all of it, HIMEM's older calls
  and the CMOS at most 64 MB, as on a real machine. Save states and rewind
  hold all of it, so they grow with it. Windows 95 with 512 MB may need
  its file cache limited (`MaxFileCache` in SYSTEM.INI's `[vcache]`).
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
* `dos_high` packs DOS's tables (its data segment, the file table and
  the environment of programs started from the prompt) at the bottom of
  conventional memory, as `DOS=HIGH` leaves it: `true` (the default) or
  `false`. The first memory block is then at 05A9h, about 617 KB of
  conventional memory are free, and programs load below 64 KB, with a
  2 KB environment (`SET` says *Out of environment space* past it).
  `false` spreads them up to 0FFFh, where programs start at segment 1000h
  with 576 KB free, for an old program that fails below 64 KB ("Packed
  file is corrupt"). DOS keeps no code in the HMA, which stays free for
  programs through XMS. A change takes effect at the DOS prompt, unless a
  TSR is resident in conventional memory.
* `dpmi` gives DOS extenders a DPMI 0.9 host, as a memory manager or
  Windows provides one: `true` (the default) or `false`. DOS/4GW,
  PMODE/W, DOS/32A, Tran's PMODE, DJGPP and Borland's RTM (protected-mode
  Borland Pascal and C++ programs such as Jazz Jackrabbit) then run their
  programs as its clients rather than switching the processor
  themselves or bringing a host of their own, and take extended memory
  from it as they need it, so a DOS/4GW program can start another DOS
  extender's (demos such as Scoop's Luminous do). As under Windows, its
  clients can call DOS with INT 21h in protected mode, with selectors
  for their buffers. Off, they run as they do on plain DOS with HIMEM. A
  change takes effect for the programs started after it.
* `dos_version` is the DOS version programs are told (INT 21h AH=30h,
  AX=3306h and the PSP's), such as `5.00` (the default), `6.22` or
  `7.10`; `7.1` is 7.10, as DOSBox's `ver` has it. From 7.00 on, DOS
  has the FAT32 functions of MS-DOS 7 that disk utilities use: the
  extended drive parameter block and free space (INT 21h AX=7302h and
  7303h) and, from 7.10, absolute disk reads and writes (AX=7305h), and
  INT 25h and 26h refuse FAT32 drives as MS-DOS 7.1 does. FAT32 drives
  mount at any version, as programs reach their files through DOS. A
  change takes effect at once.
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
* `mouse_capture_messages` says over the picture when the mouse is
  captured (and that Ctrl+Alt lets it go) or let go: `true` (the default)
  or `false`.
* `mouse_autocapture` captures the mouse as it moves over the focused
  window while a program uses the mouse driver (INT 33h), and lets it go
  as the program's cursor reaches the edge of the screen and the mouse
  keeps going, with the host's pointer where that cursor was: `true` (the
  default) or `false`. Where the program's cursor is decides it, not
  where the host's pointer would be, since a game's cursor can move
  slower or faster than the host's. A cursor kept inside a window of the
  screen, and a game steering with the mouse's motion (AX=000Bh), keep
  the mouse until **Ctrl+Alt** lets it go; it goes too once the program
  stops using the mouse. Programs that read the PS/2 or a serial mouse
  themselves, such as Windows, still take it with a click.
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

* `sbtype` is the Sound Blaster: `sb16` (the default), `awe32`, `sbpro2`,
  `sb2` or `none`. `sbbase` (hex), `irq`, `dma` and `hdma` (the SB16's
  and AWE32's 16-bit channel) set its resources; the defaults are 220, 7,
  1 and 5. The `BLASTER` environment variable follows them.
* `awe32` is an SB16 with the AWE32's EMU8000 wavetable synthesizer at the
  base + 400h, 800h and C00h (620h, A20h and E20h), as games with AWE32
  music drivers expect it; `BLASTER` gets its `E620`. The card starts as
  Creative's `AWEUTIL /S` leaves it, and the built-in `AWEUTIL` command
  does that again. The General MIDI instruments are in the card's 1 MB
  ROM, which is Creative's and doesn't come with rust-dos: `awe32rom` is
  the file (`awe32.raw`, or a directory with it). Without the setting
  rust-dos looks for `awe32.raw` in its configuration directory (also in
  an `AWE32ROM` directory there) and in the working directory, and the
  libretro core in the frontend's system directory. The settings
  window's Sound page downloads it from the copy the libretro PCem core
  keeps, checked by its SHA-256. Without the ROM the card works and plays
  the instruments programs load into its RAM; the ROM's are silent.
  `awe32ram` is the RAM in KB: `512` (the default, as the AWE32 came),
  `0`, `1024`, `2048`, `4096`, `8192`, `12288`, `16384` or `28672`.
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
`lptdac` (the Covox or Disney Sound Source), `tandy` (the Tandy's and
PCjr's sound chip) and `awe32` (the AWE32's wavetable synthesizer), and
`master` for all of them together. At 100, the
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
* `ne2000` puts an NE2000 network card in the machine: `false` (the
  default) or `true`. `nicbase` is its ports (default `300`; also 240,
  260, 280, 2A0, 2C0, 320, 340 or 360), `nicirq` its IRQ (default 10;
  also 3, 4, 5, 7, 9, 11 or 15), and `macaddr` its Ethernet address:
  `auto` (the default, a new one each start) or one like
  `02:00:5E:12:34:56`. See [Network card](#network-card).
* `online` says where the rooms of `LAN JOIN` and `LAN LIST` without an
  address, and of the settings window's room browser, are: `false` (the
  default) on this network, where a room made in the browser is hosted in
  this instance, or `true` online, at `relay`: a `host[:port]`, by default
  `relay.rust-dos.com`, a public relay where anyone can find and make
  rooms.
* `lan` joins a room at startup, as `LAN JOIN` does: `off` (the default),
  `discover` (the first relay that answers on this network) or a relay's
  `host[:port]`.
* `lanhost` relays rooms from startup on a UDP port, and joins one there,
  as `LAN HOST` does: `off` (the default) or a port.
* `room` is the room to join (default `lobby`), and `password` the
  password to join it with, or to give it when joining makes it. The
  password is kept in the file as it is written.
* `player` is the name the others in a room see this instance's player
  by, up to 32 characters. Without one (the default) they see "Player"
  and its member number.

### `[serial]`

The serial ports COM1 to COM4 (see [Serial and modem
games](#serial-and-modem-games)). Changes take effect at the DOS prompt.

* `serial1` to `serial4` say what each port has plugged in: `off` (no
  port), `mouse` (a serial mouse, moved by the host's mouse), `modem` (a
  Hayes modem, which calls the other player in the LAN room or a host on
  the internet, and works as a null modem cable to the other player too),
  `nullmodem` (a cable to the other player in the LAN room), or `empty`
  (the port with nothing plugged in). By default COM1 has a mouse and COM2
  a modem; COM3 and COM4 are off. The ports are at the standard addresses
  3F8h, 2F8h, 3E8h and 2E8h, and the BIOS lists those from COM1 up to the
  first that is off.
* `serial1irq` to `serial4irq` are their IRQs: by default 4, 3, 4 and 3
  (COM3 shares COM1's and COM4 COM2's, as on a PC); also 5, 7, 9, 10, 11,
  12 or 15. The log says when a port's IRQ is a sound or network card's
  too.
* `uart` is the chip: `16550` (the default, with FIFOs) or `8250` (none,
  for old programs that want one).
* `mousetype` is the serial mouse: `microsoft` (the default, two
  buttons) or `logitech` (three buttons).
* `modemlisten` takes calls for the first modem on a TCP port: `off` (the
  default) or a port. A call rings the modem; `ATA` answers it, or `ATS0=1`
  answers the first ring.
* `modemtelnet` makes the modem's TCP calls speak telnet, for BBSes:
  `off` (the default: the characters as they are, as DOSBox's modem sends
  them) or `on`.

### `[printer]`

A printer on the parallel port LPT1 (378h): an Epson ESC/P 2 dot matrix
printer, as in DOSBox-X. Programs print to it through the port itself,
through the BIOS (INT 17h), or through DOS (`PRN` and `LPT1`, so `COPY
FILE.TXT PRN` and `DIR > PRN` print). Pick an Epson printer (LQ, FX or
ESC/P 2) in a program's printer setup. The printer draws text in its
typefaces, pitches and styles (bold, italic, underline, double width and
height, condensed, super- and subscript, proportional), 8- and 24-pin
bit images, and colour. Changes take effect at the DOS prompt.

A print job ends when the program has printed nothing for `timeout`
milliseconds, or when **Ctrl+Shift+F5** ejects the page. Then the page in
the printer comes out, and the job's document is finished. The on-screen
message and the log say where it went.

* `output` is where printing goes:
  * `pdf` (the default): a PDF document of each job,
    `rust-dos_print_<date>_<time>.pdf`.
  * `png`: a PNG picture of each page.
  * `printer`: the host's printer. The job becomes a PDF that goes to
    `lp` (CUPS, on Linux and macOS), or on Windows to the program that
    prints PDF files. `print_command` replaces either.
  * `file`: the bytes the program sent, as they are, in a `.prn` file:
    for a real printer that speaks the program's printer language
    (`lp -o raw`), or for a converter.
  * `none`: no printer, and no LPT1 in the BIOS.

  A DAC on LPT1 (`lpt_dac`) takes the printer's place.
* `dpi` is the pages' resolution: 360 (the default, the printer's own),
  or 60 to 720.
* `paper` is the paper's size: `letter` (the default), `a4`, `legal`, or
  `<width>x<height>` in inches (`8.5x12`).
* `multipage` puts a job's pages in one PDF document (`true`, the
  default), or each page in its own (`false`).
* `timeout` is how long a job waits for more, in milliseconds of the
  machine's time: 3000 by default. 0 waits until the page is ejected.
* `docpath` is the folder the files go in. The default is the capture
  folder (`capture_dir`).
* `fontpath` is a folder with the printer's fonts, under DOSBox-X's
  names: `roman.ttf`, `sansserif.ttf`, `courier.ttf`, `script.ttf` and
  `ocra.ttf`. Without it, or for a font it doesn't have, the host's fonts
  print: Liberation or DejaVu on Linux, and Times New Roman, Arial and
  Courier New on Windows and macOS. Without any of those, the VGA's ROM
  font prints, scaled up and smoothed. Box drawing and block characters
  are always drawn with the ROM font, so they join as on a dot matrix
  printer.
* `device` is the printer `lp` sends to (`lp -d`). The default is the
  system's default printer; `lpstat -p` lists the others.
* `print_command` prints the finished PDF in place of `lp`: a command
  line with `{file}` where the file goes, or the file goes last. For
  example `lpr -P office {file}`, or on Windows `SumatraPDF
  -print-to-default {file}` (with SumatraPDF's folder in the `PATH`).
* `open_with` opens each file written with a program, such as `xdg-open`
  or a PDF viewer.

### `[achievements]`

[RetroAchievements](#retroachievements).

* `enabled` turns it on: `false` (the default) or `true`.
* `hardcore` is hardcore mode, for the games started from then on:
  `false` (the default) or `true`.
* `username` and `token` are the account, which the settings window's
  Achievements page fills in when you log in. The token is what the site
  gives for your password, which isn't kept.

### `[drives]`

Each line is `LETTER = PATH [more images] [floppy|hdd|cdrom] [-label NAME] [-ro] [-chs C,H,S] [-boot]`,
the options as `MOUNT` takes them. `-boot` boots the disk image when
Rust-DOS starts (see [Booting at startup](#booting-at-startup)). A drive number 0 to 3 instead of the
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
  mounts it on the drive picked (a floppy on A: or B:, where free). B (or
  the mount dialog's **Boot**) [boots](#booting-a-disk-image) the disk
  image of the selected drive now, and the dialog's **Auto-boot** has it
  boot whenever Rust-DOS starts, once saved (F2).
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
* **Network:** everything in [`[network]`](#network): the IPX driver, the
  NE2000 network card, and the LAN to join or host at startup.
* **Serial:** everything in [`[serial]`](#serial): what each serial port
  has plugged in and its IRQ, the chip, the serial mouse and the modem's
  TCP calls.
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
* **Achievements:** [RetroAchievements](#retroachievements): on or off,
  hardcore mode, logging in, the game's archive, and the game's
  achievements and leaderboards.
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
value starting with it. Tab switches pages, and the mouse works too.
**F1** explains the setting under the cursor, or the page or dialog open,
in a short help for getting games going; Up and Down scroll it, and Esc
or F1 closes it. The
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

## RetroAchievements

[RetroAchievements](https://retroachievements.org) has achievements,
leaderboards and rich presence (the line the site shows of what you are
doing) for many DOS games. Turn it on and log in on the settings window's
**Achievements** page; it keeps the site's token in `[achievements]`,
never your password. The page lists the game's achievements, which are
unlocked and how far along the others are, and its leaderboards.
Unlocks, leaderboards started and entries submitted show at the bottom of
the picture, and the value of a leaderboard being attempted in the
corner.

The site knows a game by the files of the zip (or DOSBox Pure `.dosz`)
it came in, so achievements are for games launched from their
[profiles](#game-profiles), and the profile says which version of the game
it is, in `[game]`:

```ini
[game]
name=Commander Keen 4
achievements=~/dos/archives/keen4.zip
```

`achievements` is the archive, relative to the `games` folder, or its hash
(32 hex digits). A zip dropped onto the window is unpacked with a profile
that has its hash already; for other games, the Achievements page's
**Game's archive** row picks the archive and puts its hash in the profile.
The sets are made with DOSBox Pure, and Rust-DOS lays out the memory they
read as DOSBox Pure does: the game's memory from 120h bytes below the
first program's PSP, so a game run from the prompt is where the sets
expect it. A game that isn't the first program (a TSR loaded before it)
may not be.

**Hardcore mode** (`hardcore`) takes effect when the next game starts:
states can't be loaded, rewind and the Cheats page are off, and unlocks
and leaderboard entries count as hardcore. Leaderboards only run in
hardcore mode, as in RetroArch. It is off while the debug server runs.
Loading a state or rewinding in softcore mode starts the achievements'
logic over, as they wait for their conditions to be false again.

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
directory, or with `-pr` to the configuration file's folder. `-ide 1m`
to `2s` puts the image on an IDE channel of a booted system (see [Booting
a disk image](#booting-a-disk-image)). DOSBox's `-freesize` and CD-ROM
access options are taken and ignored;
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

A floppy or hard disk image holds a FAT12, FAT16 or FAT32 file system
that programs read and write like any other drive's; what they write goes
into the image file. A FAT32 drive mounts at any `dos_version`, as
programs reach its files through DOS; the functions of before FAT32
(INT 21h AH=36h, 1Ch) report no more than just under 2 GB of it, as
MS-DOS 7.1 does, and from DOS 7.00 on the FAT32 functions tell the rest
(see `dos_version`). An image whose size is a floppy disk's (160 KB to 2.88 MB,
as in DOSBox's table), or named `.vfd`, `.flp`, `.360`, `.720`, `.1200` or
`.1440`, is a floppy, anything else a hard disk: an image of a
whole disk with a partition table, whose first FAT12 or FAT16 partition
(else its first FAT32 one) is the drive, or of a single volume. The hard disk's geometry comes from its partition
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
and one a drive has mounted always does. A FAT32 partition is of type 0Bh
within the BIOS's 1024 cylinders (8 GB) and 0Ch past them; MS-DOS reads it
from version 7.10 (Windows 95 OSR2) on. The boot code
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
E820h), a Plug and Play BIOS, APM 1.2, and with an S3 card or a 3dfx one a
PCI BIOS. BIOS services work in virtual-8086 mode and through the page tables of
the system that calls them. The first two floppy drives are always there,
empty or not; hard disk images are the drives C: and D:.

A drive number instead of a letter mounts a disk image for the BIOS alone,
as DOSBox's `IMGMOUNT 2 hdd.img` does: 0 and 1 are the floppy drives (INT 13h
units 00h and 01h), 2 and 3 the first two hard disks (80h and 81h). The
image needs no DOS file system: a booter game's floppy, a blank hard disk
to install a system on, or one with a file system DOS doesn't read. It has no drive letter, and takes its unit
before A:, B: or the hard disk drives, which fill the hard disk units it
leaves. `BOOT -l 2` boots from it, and so does `BOOT -l C` where C: isn't a
disk image; `-fs none` with A: to D: means the numbers 0 to 3, and `MOUNT -u
2` unmounts it. The built-in DOS's programs reach it through INT 13h too.
`BOOT booter.img` puts a floppy without a file system in unit 0 this way.

```
IMGMOUNT 2 ~/images/linux.img          a hard disk DOS can't read
IMGMOUNT 0 install1.img install2.img   floppies to install from
BOOT -l A
```

The hard disks are ATA disks on the IDE channels of the booted machine as
well, the same images INT 13h reads and writes: the first (80h) the
primary channel's master (ports 1F0h-1F7h and 3F6h, IRQ 14), the second
its slave. A system's own IDE driver (Windows 9x's 32-bit disk access,
Linux, a DOS ATA driver) finds them there, by LBA or by cylinder, head and
sector with no more than 16 heads (1024/64/63 for INT 13h is 4096/16/63),
and the CMOS lists them as the user-defined type 47. A CD image or folder mounted
as a CD-ROM drive (`MOUNT D ~/dos/game.cue`) is an ATAPI CD-ROM drive on the
secondary channel's master (ports 170h-177h and 376h, IRQ 15), and without
one there is an empty drive (see below). Windows 95
finds the channels through the Plug and Play BIOS with its own driver,
reads the disc and plays its CD audio; Ctrl+F4 or another `MOUNT` changes
the disc, and Windows notices. With more than one CD-ROM drive, the first
drive letter with a disc is the one.

MOUNT's (and IMGMOUNT's) `-ide 1m`, `1s`, `2m` or `2s` puts the disk or CD-ROM
drive on the primary or secondary channel's master or slave instead, as
DOSBox-X's `-ide` does; the others take the free places. A hard disk past
the fourth place, or on a channel whose IRQ a sound or network card has,
stays with INT 13h alone. `ide_hard_disks=false` in `[emulator]` leaves all
hard disks to INT 13h, as before rust-dos had IDE disks.

Windows 95 and 98 take a disk over with their 32-bit disk access only once
they have seen the BIOS drive it through the IDE ports: starting, they trap
the ports and call INT 13h. So INT 13h's reads and resets, where the ports
are trapped, go through the port accesses a BIOS makes (select the drive,
READ SECTORS, wait, read the data) on their way out, as DOSBox-X's
`int13fakev86io` has them, and otherwise leave the disk's registers as
such a BIOS does, which Windows for Workgroups' 32-bit disk access checks.
The first start of a Windows installed without the IDE controller finds it
as new hardware and asks to restart; after that, **System Properties** →
**Performance** shows the hard disk without "MS-DOS compatibility mode".

### Host folders in a booted system

A booted system reads disks, not the host's folders, so a folder mounted
as a drive reaches it as a disk made from the folder: as a CD it can have
at any time, or as a hard disk it gets when it boots and whose changes go
back into the folder.

**As a CD.** A folder mounted on a CD-ROM drive letter goes into the
booted system's CD-ROM drive as a disc: ISO 9660 with Joliet names, so
Windows 95 sees the long names and DOS the 8.3 ones. Mount it while the
system runs (the settings window's Drives page, or `MOUNT E ~/stuff -t
cdrom` before booting), and Windows notices the new disc. The disc is what
the folder held when it went in; Ctrl+F4, or R on the Drives page, puts it
in again with what the folder holds now. Its files are read from the host
as the system reads them, so a big folder takes no memory. The booted
system has one CD-ROM drive: `boot_cdrom=true` in `[emulator]` (the
default) gives it one even with no CD mounted at the boot, on the first
free letter from D:, and a CD mounted on any letter later goes into it
while it is empty. With `boot_cdrom=false` it has one only when a CD is
mounted as it boots.

**As a hard disk.** A folder mounted on D: to Y: as a hard disk
(`MOUNT D ~/share`) is a hard disk of a system booted with it: a FAT16
disk of 2 GB made at the boot with the folder's files on it, long names
and all, which DOS, Windows 3.1 and Windows 95 read. It comes after the
disk images (C: stays 80h; D: is 81h) and is an ATA disk on the IDE
channels like them. When the system shuts down (Windows' **Shut Down**),
what it created, changed and deleted on the disk goes into the folder;
S on the Drives page (or `POST /api/drive/sync` of the debug server)
copies its new and changed files while it runs, without deleting anything,
as its caches may not have written everything yet, and so does quitting
Rust-DOS while it runs. What the host changes in the folder while the
system runs reaches the disk at its next boot. A file both changed keeps
the host's version, and the system's goes beside it as `name (from
guest).ext`; a file the host changed is never deleted. Windows' Recycle Bin
stays on the disk. The folder may hold up to 1536 MB, which the disk holds
in memory; files whose names FAT can't have are left off it (the log says
which). `-noshare` keeps a folder for the built-in DOS alone, and `-share`
shares C: too; a folder mounted `-ro` is shared, but nothing goes back into
it. A shared drive can't be unmounted while the system runs. Save states
and rewind keep the disk and what was copied back with the system: loading
an older state takes the disk back, and files copied into the folder since
stay there.

```
MOUNT D ~/share          Windows 95 has it as a hard disk
MOUNT E ~/downloads -t cdrom
IMGMOUNT C ~/images/win95.img
BOOT -l C
```

### Booting at startup

A disk image in `[drives]` with `-boot` boots whenever Rust-DOS starts,
after the `[autoexec]` lines (which can mount more for it) and in place of
`C:\AUTOEXEC.BAT`, as `BOOT -l` with its drive would:

```ini
[drives]
C=~/images/win95.img -boot
```

The settings window's Drives page sets it too: the mount dialog's
**Auto-boot**, saved with F2 like the other drives. One drive boots at a
time; marking another takes it from the one before. B on the Drives page,
or the dialog's **Boot**, boots a drive's image right away instead, while
no program runs (over a booted system, it starts over from that disk).
`--no-boot`, or a game profile launched at startup, starts at the DOS
prompt without booting.

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

### Direct3D on an S3 ViRGE

With `machine=svga_s3virge` (or `svga_s3virgevx`) the PCI card is an S3
ViRGE (PCI\VEN_5333&DEV_5631; the VX is DEV_883D), a 2D and 3D
accelerator with 4 MB. S3's Windows 95 driver draws the desktop with its 2D
engine and gives DirectDraw and Direct3D a hardware device, whose
triangles the ViRGE's 3D engine draws: Gouraud shaded or textured
(perspective corrected, filtered and mipmapped), Z buffered, fogged and
alpha blended, at 16 or 24 bits per pixel. Its streams processor shows
DirectDraw's overlays and S3D Toolkit games' page flips.

Windows 95 finds the card at its next start. The first release of
Windows 95 has no driver for it; S3's (version 4.03.00.2111, "S3 ViRGE,
VX, DX & GX v3.12.01", on the VOGONS driver library) installs with **Have
Disk** from a folder copied onto the disk image. DirectX's own runtime
(DirectX 5 or later) is needed for Direct3D; DxDiag's **Test Direct3D**
shows the ViRGE drawing. Direct3D needs the desktop at 16 or 24 bits per
pixel ("High Color" or "True Color", which the ViRGE's driver has in
place of 32 bits).

`/api/status` of the debug server shows what the card's engines did under
`video.s3.virge`.

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
The software rasterizer keeps the card's memory, which games read back
and save states keep, but it only draws what can still be seen there:
until something reads the memory, the drawing waits, and what a later
fastfill covers before anything read it is never drawn, so the host's CPU
only draws the frames something looks at. Frame buffer reads, save
states, screenshots and recordings see the memory as if everything had
been drawn; screenshots and recordings show the software picture, at the
card's resolution (or through the CRT shader at the window's size, with
`record_shader=true`). Drawing that is never drawn isn't counted by the
card's pixel counters, until a game reads them: from then on everything
is drawn and counted (`RUST_DOS_VOODOO_PRUNE=0` in the environment draws
everything from the start). The CRT shaders draw their scanlines over the
bigger picture.

## Playing over a LAN

rust-dos instances on different computers play DOS games over IPX as PCs
on one network would. The instances join a room of a relay, which passes
the network traffic of each on to the others over UDP; no network driver
or special privileges are needed on the host. One instance on the
network can host the relay, or the public relay at `relay.rust-dos.com`
can, or a server of your own:

```text
LAN HOST                      on one computer
LAN JOIN                      and on the others, on the same network
LAN JOIN /ONLINE /ROOM:doom   or on every computer, at the public relay
LAN JOIN 203.0.113.7          or at the host's address, over the internet
```

The settings window (Ctrl+F12) finds rooms too: *Find or make a LAN
room* on its Network page lists the rooms on this network (those of
every instance there that hosts a relay), or with Tab those online at
the relay `relay` names, the fullest first, narrowed down as a search is
typed. Which one it shows is *LAN rooms* on the page (`online`), on this
network unless set otherwise. Enter joins the room selected, asking for
its password if it has one; *Make room* (or Ins) makes one, with a
password or open to all, and joins it: on this network, on a relay this
instance hosts, as `LAN HOST` does. While the instance hosts the room it
is in, the browser shows that room instead: its players, *Leave*, after
which the one there longest hosts it, and *Disband*, which ends it for
everyone.

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
* **`LAN JOIN [host[:port] | /LOCAL | /ONLINE]`** joins a room at a
  relay, and makes the room if it isn't there. Without an address it is
  where `online` in [`[network]`](#network) says: at the first relay that
  answers on this network (the default), or at the relay `relay` names
  (`relay.rust-dos.com` unless set); `/LOCAL` and `/ONLINE` ask for one
  or the other. It waits a few seconds for the room (a key stops waiting;
  joining goes on). The instance keeps its place with keepalives, and
  joins again when the relay comes back after a restart. `LAN LEAVE`
  leaves.
* **`LAN LIST [host[:port] | /LOCAL | /ONLINE] [/ROOM:text]`** lists the
  rooms of a relay, or on this network those of every relay that
  answers, the fullest first, with their players and whether they want a
  password; with `/ROOM:text`, those with the text in their names.
* **Rooms** keep several games on one relay apart: `/ROOM:name`, or `room`
  in `[network]` (default `lobby`); a name with spaces goes in quotes,
  `/ROOM:"doom 2 dm"`. A room is there while anyone is in it. The instance
  that makes a room gives it a password with `/PASSWORD:text` (or
  `password`), and everyone joining after has to know it; a relay with a
  password of its own (`LAN HOST /PASSWORD:text`, `rust-dos-relay
  --password`) wants that one for all its rooms instead. Instances prove
  they know a password without sending it, and a relay keeps only a key
  made from it, never the password itself. Nothing else crossing the relay
  is encrypted: whoever can watch the traffic of the instance that made a
  room can join it.
* **The host** of a room is the instance that made it, and once that one
  leaves, the one there longest. `LAN DISBAND` ends the room for everyone
  in it, who aren't let back in for a minute; only the host can. `LAN`
  shows who is in the room, by `player` in [`[network]`](#network), or as
  "Player" and a number without one.
* **What goes over the relay** is Ethernet frames: the IPX driver's
  packets are on the same LAN as the network cards of other instances, in
  the frame type `ipxframe` sets (Ethernet II by default), where the IPX
  protocol of a system booted with a card can take them.

### Serial and modem games

Games that play over a serial cable or a modem (Doom's and Heretic's
SERSETUP, Descent, Warcraft, Duke Nukem 3D, Command & Conquer) play
between the first two players in a LAN room: joining a room is all it
takes, for IPX and serial games alike. A third player and later ones
aren't linked; `LAN` says so. The port that goes to the other player is
the first `modem` or `nullmodem` in [`[serial]`](#serial), COM2 by
default; set the game up for that port, at any speed.

* **Null modem games** (Doom's `SERSETUP -com2`, Descent's *Establish
  null-modem link*) just start on both computers. On a `nullmodem` port
  each end's DTR is the other's DSR and DCD, as over a cable. A `modem`
  port works too: what a game sends that isn't an AT command goes to the
  other player, and the modem is online to them until the game ends.
* **Modem games** set the modem up with AT commands and dial. Any phone
  number calls the other player in the room (`SERSETUP -com2 -dial 555`
  on one computer, `-answer` on the other), whose modem rings; `ATA`
  answers, or `ATS0=1` beforehand answers the first ring.
* **Hosts on the internet:** a number with a dot, a colon or a letter in
  it is a TCP address, port 23 unless given: `ATDT bbs.example.com` or
  `ATDT 192.0.2.7:5000`. With `modemlisten` another rust-dos, or
  DOSBox's modem, can call this one.
* The modem knows the usual commands: `ATZ`, `AT&F`, `ATE`, `ATV`, `ATQ`,
  `ATH`, `ATA`, `ATO`, `ATD`, `ATI`, `ATS0=`..`ATS15=` and `ATSn?`,
  `A/`, and the escape `+++` with a second's pause before and after; it
  answers the settings of error correction and compression with OK.
* The link is reliable and in order over the relay's UDP, and needs a
  relay that knows serial links (the relay of a rust-dos with serial
  ports, or its `rust-dos-relay`); `LAN` says when the relay is older. The
  settings window's room browser shows which player the link goes to.
* The BIOS (INT 14h) and DOS's `COM1`-`COM4` devices go through the
  ports too: `ECHO ATDT555 > COM2` dials.

### A relay on a server

A relay on a server lets players meet over the internet without setting
up their routers, as the public one at `relay.rust-dos.com` does.
`rust-dos-relay` is a small program of its own, in every release package
beside rust-dos, that needs neither a display nor SDL or a sound library
(to build it alone: `cargo build --release --no-default-features --bin
rust-dos-relay`, which needs SDL2's development files but not ALSA's).
`rust-dos --relay [PORT]` does the same without starting the emulator.

* `--port` is its UDP port (default 21213), which the server's firewall
  has to let in, and `--bind` the address it listens on (default all).
* `--name` is the name room browsers and `LAN LIST` show.
* `--password` gives all its rooms one password, for a relay of your
  own. Without it, the one who makes a room decides whether it has a
  password.
* It prints joins, leaves and wrong passwords with the time. It holds up
  to 1000 members, 200 in a room and 32 from one IP address.

On Linux with systemd, a service keeps it running
(`/etc/systemd/system/rust-dos-relay.service`, then `systemctl enable
--now rust-dos-relay`):

```ini
[Unit]
Description=rust-dos LAN relay
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/bin/rust-dos-relay --name "rust-dos public relay"
DynamicUser=yes
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

### Network card

`ne2000=true` in [`[network]`](#network) puts an NE2000 in the machine,
the network card every DOS packet driver, Windows for Workgroups and
Windows 95 know, at ports 300h and IRQ 10 unless set otherwise. It is on
the same LAN as the IPX driver: the cards of instances in one room see
each other's frames, as cards on one Ethernet would.

* **The internet:** the card sits behind a router of its own, as a PC
  behind a home router does. It hands out the address by DHCP (or BOOTP):
  the guest is 10.0.2.15, the router 10.0.2.2 and the name server
  10.0.2.3, which looks names up with the host's resolver. The guest's TCP
  connections and UDP traffic become the host's own, so it needs no
  special rights, and no setting. 10.0.2.2 is the host itself: a server on
  the host's port 8080 is `http://10.0.2.2:8080/` in the guest. The router
  answers pings to its own addresses only.
* **DOS:** load the Crynwr packet driver, `NE2000 0x60 10 0x300`
  (software interrupt, IRQ, ports), from the Crynwr packet driver
  collection or FreeDOS's `crynwr` package. Programs that use a packet
  driver then work; with mTCP, `DHCP` sets it up, and `HTGET`, `TELNET`,
  `FTP` and `IRCJR` reach the internet.
* **Windows 3.x with Trumpet Winsock:** before Windows, load the packet
  driver and the Crynwr collection's WINPKT (`NE2000 0x60 10 0x300`, then
  `WINPKT 0x60`). In Trumpet's setup, turn off its dialler's SLIP and PPP,
  set the packet vector to 60 and the IP address to `bootp` (in
  TRUMPWSK.INI: `ppp-enabled=0`, `slip-enabled=0`, `vector=60`,
  `ip=bootp`). Trumpet then gets its address from the router, and
  Netscape and the other Winsock programs reach the internet. A browser
  set up for a proxy of a dial-up service goes through that proxy, so
  turn it off in the browser's options if the proxy is gone. Trumpet
  doesn't give the packet driver back when Windows ends: to start Windows
  again with it, start rust-dos again first.
* **Windows 95:** in Control Panel, Network, add the adapter "Novell/Anthem",
  "NE2000 Compatible", and the protocol "Microsoft", "TCP/IP". In the
  adapter's properties (Resources), set the I/O address range 300-31F and
  the interrupt 10, as rust-dos has them. Windows copies its drivers from
  its setup files (CD or `C:\WINDOWS\OPTIONS\CABS`), restarts, gets its
  address from the router (`winipcfg` shows it), and Internet Explorer and
  the other programs reach the internet. Its IPX/SPX-compatible protocol
  is on the LAN of the room too; the built-in DOS's IPX driver sends
  Ethernet II frames, the frame type to give Windows' protocol for the two
  to meet.
* **Windows for Workgroups** has its own NE2000 driver for its network
  and for Microsoft's TCP/IP-32.
* **Web browsers** of the time speak plain HTTP and SSL 3.0 at most, which
  today's sites refuse: sites made for them, such as frogfind.com and
  theoldnet.com, and plain `http://` ones work.
* Instances in one [LAN](#playing-over-a-lan) room reach each other's
  cards as well, as PCs on one Ethernet do: TCP/IP between them (DirectPlay
  games, sharing files), and IPX/SPX. In a room, each router hands out an
  address of its own, 10.0.2.20 and the instance's member number (member
  2 gets 10.0.2.22), so the guests don't clash; a guest that got its
  address before joining gets the new one at its next start, or with
  `winipcfg` (Release, then Renew).

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
| `--no-boot` | Start at the DOS prompt, without booting the disk image marked `-boot` in `[drives]` |
| `-s, --scale N` | The window scale factor, 1 to 16 |
| `--cycles N\|max\|auto` | The CPU speed (`cycles`) |
| `--core auto\|dynamic\|normal` | What runs the instructions (`core`) |
| `--game NAME` | Launch a [game profile](#game-profiles) at startup, by its file name or its name |
| `--import PATH` | Import a game set up for DOSBox as a game profile, and launch it |
| `--debug-server [ADDR]` | Start the [debug server](README.md#debug--remote-control-server) (default `127.0.0.1:8086`) |
| `--trace-capacity N` | Entries in the debug server's instruction trace (default 1,000,000) |
| `--relay [PORT]` | Relay [LAN](#playing-over-a-lan) rooms on this UDP port (default 21213) instead of starting the emulator |
| `--relay-password TEXT` | One password for all the rooms of `--relay` (without it, whoever makes a room gives it one or none) |
