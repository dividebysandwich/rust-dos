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
* [Networking at a glance](#networking-at-a-glance): what the IPX driver,
  rooms, relays and the network card each do
* [Playing over a LAN](#playing-over-a-lan), [serial and modem
  games](#serial-and-modem-games) and [a relay on a
  server](#a-relay-on-a-server)
* [CRT shaders](#crt-shaders)
* [3D scene and VR](#3d-scene-and-vr): the picture on a screen in a Blender
  scene, in the window or in a VR headset
* [Command-line options](#command-line-options)

## Configuration file

rust-dos reads a DOSBox-style configuration file. It uses the first file it
finds, and never merges files:

1. The file given with `-c/--config FILE`. If that file doesn't exist,
   rust-dos exits with an error.
2. `rust-dos.conf` in the current working directory.
3. `rust-dos.conf` in the directory holding the rust-dos executable. Put one
   there to make a portable install that leaves the user profile alone: its
   configuration directory is then the executable's, which gets the log,
   the shell history, the downloaded ROMs and the other files below that
   go to the configuration directory.
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
* `aspect` selects the picture's display ratio: `none` (native pixels),
  `4:3`, `5:4`, `16:10` or `16:9`. The picture expands to that shape
  without cropping; `4:3` is the usual choice for DOS games.
* `vrr=true` is for a display with a variable refresh rate (G-Sync,
  FreeSync). Each of the machine's frames is shown when its display
  draws it, so the window refreshes at the machine's own rate: 70 Hz for
  VGA's text and 320x200 modes, 60 Hz for 640x480. Without it, frames
  come at a steady 60 Hz, which judders at 70 Hz. VRR must also be on
  for the display, and for windows, in the graphics driver or the
  desktop (sway's `adaptive_sync on`, for example). Where the display
  is slower than the machine's rate, frames come at 60 Hz as without
  it. `--vrr` turns it on.
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
* `monitor_effects` plays a CRT monitor's own effects: `off` (the
  default), `picture`, `sound` or `on` (both). At power on, the picture
  warms up, slightly too big at first, and wobbles with colour fringes as
  the degaussing coil's field dies away, to its thump and hum and the
  crackle of the high voltage; when the card changes to another line or
  frame rate (640x480 at 60 Hz after the 70 Hz text mode, say) the
  monitor's relay clicks and the picture blanks and comes back with a
  jolt, as a multisync monitor did; and when rust-dos quits, the picture
  collapses to a line and a fading dot. **Ctrl+Shift+D** degausses. The
  sounds have their own volume, `monitor`, in [`[mixer]`](#mixer).
* `post` shows a BIOS power-on self test before the built-in DOS starts,
  and at each reboot: `off` (the default), `quick` (about a second) or
  `full` (about three, the memory counted up as it is tested). It lists the
  CPU, the memory, the drives and the sound cards. **Del** while it shows
  opens the settings as a period CMOS setup utility, whose pages are
  changed with PgUp/PgDn or +/-; nothing changes until *Save & Exit Setup*
  (**F10**), which applies and saves the settings and restarts the
  machine. **Esc** skips the memory test. A booted disk image has no POST
  of rust-dos's.
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
  DOSBox's `cycles=auto`. A protected-mode one runs at max while it
  reads and writes (loading, computing), and otherwise goes down to
  20000, a 486's speed, or a quarter of a faster `cpu` model's clock
  (50000 on a Pentium, 112500 on a Pentium II), no more than the host
  has. `auto 10000` makes that real-mode speed, and the least `auto`
  goes down to, 10000. `max` runs everything as fast as the host keeps
  up with in real time. Use a number such as `3000` for a fixed speed.
  `--cycles` overrides it.
* `auto_fps` is the frame rate `cycles=auto` aims for: `auto`, the
  default, is the display's refresh rate (the 3dfx card's while it
  shows), and a number from 10 to 240 takes its place. A lower one, such
  as `30`, gives a game that would draw faster only the speed for that
  many frames a second; a higher one lets a game go past the display's
  rate. It applies at once, and the search for the speed starts over.
* `auto_busy_os` (`true`, the default, or `false`): whether
  `cycles=auto` gives a program all the host has while it keeps a
  system that idles busy. Windows 95 and 98 halt the processor while
  nothing runs; once `auto` has seen the processor idle for half a
  measurement in protected mode (halted, waiting for a key in the BIOS
  or in a busy-wait loop the emulator skips), a program that keeps it
  from idling (under 5% idle, no frames, such as a 3D benchmark
  building its next scene) runs at max instead of slowing down as a
  spinning menu would. The built-in Windows (`win32`) idles so too,
  and `auto` knows it from the start: a Win32 program that computes
  without drawing, such as a game unpacking its level, runs at max.
  What `auto` has seen goes into save states, and is forgotten when the
  machine goes back to real mode (a reboot, a DOS program's end). It
  applies at once.
* `cpu` is the emulated processor: `486` (the default, a 486DX with FPU),
  `386`, or `pentium`, a Pentium as DOSBox-X has one: CPUID (a
  GenuineIntel family 5), the time stamp counter, which counts the
  instructions and so runs at a fixed `cycles` speed (at `max` and
  `auto`, whose speed changes as they run, it runs at a clock rate of the
  model's own: 200 MHz on a Pentium or Pentium Pro, 233 on a Pentium MMX,
  450 on a Pentium II, 550 on a K6-2 and 600 on a Pentium III, so that
  programs which time with it, such as 3DMark 2000, measure right), its
  MSRs, CMPXCHG8B and 4 MB pages, without the virtual-8086 mode extensions; `pentium_mmx`,
  that Pentium with MMX (CPUID family 5, model 4), whose eight MM
  registers are the FPU's; `k6_2`, an AMD K6-2 (CPUID AuthenticAMD
  family 5, model 8, with AMD's extended leaves): the Pentium MMX's with
  3DNow!, SYSCALL/SYSRET and the K6's MSRs (EFER, STAR, the write
  handling control and test register 12), without the P6 instructions.
  Its 3DNow! arithmetic follows AMD's manual: it rounds to nearest, gives
  a zero below 2^-126 and the largest normal from 2^128, and has no
  exceptions; PFRCP and PFRSQRT give the exact results rounded to a
  single where the real one's tables give 14 and 15 bits.
  `pentium_pro`, a Pentium Pro as DOSBox-X's
  `ppro_slow` (CPUID family 6, model 1): the Pentium's with CMOVcc,
  FCMOVcc, FCOMI and FUCOMI, the P6 hinting NOPs and the local APIC base
  and microcode revision MSRs Windows 98 and ME read, without MMX;
  `pentium_ii`, a Pentium II as DOSBox-X's (family 6, model 3): the
  Pentium Pro's with MMX and SYSENTER/SYSEXIT; or `pentium_iii`, a
  Pentium III (Katmai, family 6, model 7): the Pentium II's with SSE,
  FXSAVE/FXRSTOR, CR4's OSFXSR and OSXMMEXCPT, the processor serial
  number (CPUID leaf 3, until MSR 119h switches it off) and the cache
  descriptors of CPUID leaf 2. Its SSE arithmetic rounds as MXCSR says,
  flushes to zero, and sets and masks the six exception flags as a
  Pentium III does: an unmasked one raises #XM, or #UD without
  OSXMMEXCPT. RCPPS and RSQRTPS give the exact reciprocal rounded to a
  single, within the precision Intel specifies for them. SSE needs
  CR4.OSFXSR, which a system that saves the SSE state sets; rust-dos's
  DPMI host sets it for its clients on a Pentium III. None of them has
  RDPMC, a local APIC or memory type range registers, as in DOSBox-X. A
  DOSBox configuration's `cputype` of a Pentium imports as `pentium`, of
  a Pentium MMX as `pentium_mmx`, of a Pentium Pro as `pentium_pro`, of
  a Pentium II as `pentium_ii`, and of a Pentium III or DOSBox-X's
  `experimental` as `pentium_iii`.
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
* `fpu` is how the FPU adds and subtracts: `exact` (the default) on 80
  bits, as a real 387 does, or `fast` on the host's 64-bit doubles, as
  it multiplies and divides anyway. `fast` saves host CPU in programs that
  compute a lot with the FPU (Quake), on both cores, but isn't exact:
  sums can differ from a real machine's in the last bits, they are always
  rounded to nearest whatever the control word says, and FXCH keeps only
  the doubles of the registers it exchanges (a value loaded as 80 bits
  loses its last bits there). Saved states load in either mode. In both,
  products, quotients, square roots and the transcendental functions are
  the doubles', so they keep 53 bits of significand where a 387 keeps 64,
  rounded to nearest whatever the control word says, and the control
  word's precision field isn't used (a program that sets it to 53 bits,
  as Win32 programs built with MSVC do, gets close to what a 387
  gives). Loads, stores, integer conversions, compares and FRNDINT use
  all 80 bits.
* `machine` is the display adapter programs find when they look for
  one, and so the graphics they choose: `svga` (the default, a VGA with
  VESA modes up to 1024x768), `svga_s3` (an S3 Trio64 on the PCI bus,
  with its VESA modes, 2D accelerator and hardware cursor, for systems
  [booted from disk images](#booting-a-disk-image) whose drivers program
  it), `svga_s3virge` and `svga_s3virgevx` (an S3 ViRGE or ViRGE/VX on
  the PCI bus: the Trio64's registers with the ViRGE's own 2D engine, 3D
  engine and streams processor, which Windows 95's Direct3D uses through
  S3's driver; see [Direct3D](#direct3d-on-an-s3-virge)), `svga_verite`
  (a Rendition Vérité V1000 with 4 MB on the PCI bus, for DOS games'
  Vérité versions; see [Rendition Vérité](#rendition-vérité)),
  `svga_ati_3drage` and `svga_ati_3drage2` (an ATI 3D RAGE or 3D RAGE
  II with 4 MB on the PCI bus, whose 2D and 3D engines ATI's Windows
  drivers use; see [ATI 3D RAGE](#ati-3d-rage)),
  `svga_voodoo3` and `svga_banshee` (a 3dfx Voodoo3 3000 or Voodoo
  Banshee with 16 MB on the PCI bus, which 3dfx's Windows drivers use for
  the desktop, Direct3D and OpenGL; see
  [3dfx Voodoo3](#3dfx-voodoo3-and-banshee)), `svga_et4000`
  (a Tseng Labs ET4000AX with 1 MB and a Sierra HiColor DAC, see below),
  `vga` (an IBM VGA, without VESA modes),
  `ega` (an IBM EGA with an Enhanced Color Display: 16 of 64 colours at
  640x350, 60 Hz), `cga` (an IBM CGA: 4 colours at 320x200, 2 at
  640x200, 60 Hz), `tandy` (a Tandy 1000), `pcjr` (an IBM PCjr) or
  `hercules` (a Hercules Graphics Card on a monochrome monitor: the MDA's
  text and 720x348 graphics, 50 Hz). A change takes effect at the DOS
  prompt. A DOSBox configuration's `svga_s3`, DOSBox's default, imports
  as `svga`, which is what DOS games get from it there; one that boots a
  hard disk, or runs in a Windows of files (a package's `os=win311`),
  keeps `svga_s3`, as that Windows was installed with the S3's driver
  (unless the package's `os` boots another system in its place).
  `machine=` in its `[rust-dos]` or `[emulator]` section goes over the
  import's, as rust-dos's other settings there do (see
  [GAME-PACKAGES.md](GAME-PACKAGES.md#packages-made-for-dosbox)).
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
  `voodoo_model` is `voodoo1` (the Voodoo Graphics, the default) or
  `voodoo2` (see [3dfx Voodoo2](#3dfx-voodoo2)). `voodoo_memory` is `12`
  (the default: 4 MB of frame buffer and two texture units with 4 MB each,
  as DOSBox-X's card has), `8` (a Voodoo2 board: 4 MB and two texture
  units with 2 MB each; it picks `voodoo2` when `voodoo_model` is unset) or
  `4` (a retail Voodoo Graphics: 2 MB and one texture unit with 2 MB; a
  Voodoo2 takes it as `8`). All three take effect at the DOS prompt.
  `voodoo_texture_sampling` is `default` (preserves the game's filtering) or `unfiltered` (nearest-neighbour sampling). With supported
  OpenGL anisotropy enabled, `unfiltered` also blends between mip levels
  and allows anisotropic filtering to reduce shimmer, which may soften
  textures; magnification stays nearest-neighbour. With anisotropy off,
  or with software rendering, sampling stays strictly nearest-neighbour.
  It works with both renderers and changes immediately; mip levels,
  wrapping and clamping still follow the game.
  `voodoo_renderer` is what draws the card's picture in the
  window: `software` (the default, rust-dos's own rasterizer) or `opengl`,
  which draws it again at `voodoo_scale` (1 to 4, 2 by default) times the
  card's resolution (see [3dfx Voodoo Graphics](#3dfx-voodoo-graphics)),
  with `voodoo_scale_shader` (`true`, the default, or `false`) whether the
  CRT looks keep one scanline per line of the card's resolution (`false`
  gives a line per pixel of the bigger picture, finer lines; it matters
  only above 1x and with a CRT look),
  with `voodoo_dither` (`true`, the default, or `false`) whether it
  dithers to 16 bits a pixel as the card does where the game dithers
  (`false` gives a smoother picture; see
  [3dfx Voodoo Graphics](#3dfx-voodoo-graphics)),
  with `voodoo_msaa` (`off`, the default, `2`, `4` or `8`) samples a
  pixel of multisampled antialiasing and `voodoo_anisotropy` (`off`, the
  default, `2`, `4`, `8` or `16`) filters textures at oblique angles;
  `voodoo_fps_cap` (`off`, the default, or 10 to 240) is the most frames a
  second games may show on the card (see
  [3dfx Voodoo Graphics](#3dfx-voodoo-graphics));
  `voodoo_gamma` (`off`, the default, or 0.1 to 2) is the gamma Glide
  gives the card's picture (see [3dfx gamma](#3dfx-gamma));
  without OpenGL 3 (in the browser, or with SDL's dummy video driver) the
  software rasterizer's picture shows. Both change at once.
* `powervr=pcx1` or `pcx2` adds a PowerVR 3D card beside the display
  adapter, for games with a PowerVR version: the PCX1 (VideoLogic
  Apocalypse 3D) or the PCX2 (Matrox m3D, VideoLogic Apocalypse 3Dx; see
  [PowerVR PCX1 and PCX2](#powervr-pcx1-and-pcx2)). `off` is the default.
  It takes effect at the DOS prompt. `powervr_filter` is how it filters
  textures: `auto` (the default, as the game sets the card), `point` or
  `bilinear`; it changes at once.
* `capture_dir` is the folder screenshots and recordings go in:
  `capture` (the default) in the directory rust-dos started in, or a
  path of your own. Screenshots (Ctrl+F5, in the settings window too)
  and recordings show the picture with the monochrome look but without
  the messages at the top. With `record_ui=true` they show the settings
  window and the performance overlay (Ctrl+Shift+F12) as well while they
  are open; `false`, the default, shows the picture alone. A [manual's
  page](#manuals-and-extras) shows either way, a screenshot of it as sharp
  as the window shows it.
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
  256 MB for a Pentium and 512 MB for a Pentium MMX, K6-2, Pentium Pro,
  Pentium II or Pentium III. More than the CPU
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
* `win32` turns on the built-in Windows: `true` or `false` (the default). A
  Win32 program, a Windows 95 .EXE, then runs from the DOS prompt in
  protected mode with paging, as Windows 95 runs it, and ends back at the
  prompt. Its system DLLs are written into the emulator: KERNEL32, ADVAPI32
  and MSVCRT for heaps and virtual memory, files, modules, threads,
  structured exceptions, child processes, the registry and the C runtime;
  USER32 and GDI32 for windows, dialogs and drawing on a 640x480 desktop in
  a VESA mode; DDRAW, DSOUND, DINPUT, WINMM and OLE32 for DirectX 3 to 6
  games: DirectDraw in full screen on the card's VESA modes or in a window,
  DirectSound and wave audio in the `windows` mixer channel, MIDI to the
  MPU-401's synthesizer, CD audio and MIDI files through MCI, and the
  keyboard, mouse and joystick through DirectInput; DMUSIC, DMIME, DMLOADER,
  DMSTYLE, DMBAND and DMCOMPOS play DirectX 6.1's DirectMusic on the same
  synthesizer; D3D8 is DirectX 8.1's Direct3D on a Banshee or Voodoo3, and
  OPENGL32 is OpenGL 1.1 there as 3dfx's ICD had it, with GDI32's pixel
  formats, SwapBuffers and gamma ramp. A program that calls a function they
  don't have ends with a message naming it, and one that needs a DLL that is
  neither among them nor beside it is refused. The registry is kept in
  `C:\REGISTRY.REG`, which is made the first time a program uses it, so a
  host folder mounted as C: gets that file. The built-in DLLs show as
  read-only files in `C:\WINDOWS\SYSTEM`, to DOS programs too, and
  `GLIDE2X.DLL` is among them only with a 3dfx card. A Windows 3.1 program
  (an NE .EXE) runs too, as Windows 95 runs it: its KERNEL, USER, GDI and
  other 16-bit modules are written into the emulator as well and go on to
  the 32-bit ones, and its own DLLs, such as Visual Basic's runtimes, load
  from beside it. Off, such a program runs its DOS stub, which says it needs
  Windows. The plan is in [docs/native-windows.md](docs/native-windows.md).
* `win32_dlls` names a host folder of Win32 DLLs that programs under the
  built-in Windows may load as their own code, such as Wine's 32-bit
  `lib/wine/i386-windows` folder. A DLL a program needs that is neither
  built in nor on the search path is read from there, as if it were in
  `C:\WINDOWS\SYSTEM`. A function such a DLL imports that the built-in
  DLLs don't have fails where it is called, returning 0, instead of
  ending the program; the folder's import libraries (`lib<name>.a`) say
  how many bytes of arguments each takes. Empty (the default), there is
  no such folder. Wine's MSACM32.DLL with its ADPCM codecs runs this
  way; see "Wine's DLLs as guest code" in
  [docs/native-windows.md](docs/native-windows.md).
* `win32_dlls_replace` lists the built-in DLLs that the `win32_dlls`
  folder's DLLs of the same name replace, separated by commas, such as
  `comctl32,comdlg32`. Empty (the default), the built-in ones are used.
* `win32_version` is the version of Windows the built-in Windows is:
  `95` (the default), `98` or `98se` (Windows 98 Second Edition). It is
  what GetVersion and GetVersionEx say (4.00.950, 4.10.1998 or 4.10.2222
  with " A " as its service pack), and what the registry's version
  values and DirectX's version say. Under `98` and `98se` the built-in
  DLLs have the functions Windows 98 added, such as MonitorFromWindow
  and GetFileAttributesExA, and there is MSIMG32.DLL, with AlphaBlend,
  GradientFill and TransparentBlt; under `95` a program that imports
  one of those doesn't start, as on Windows 95, and its message names
  the setting. These include USER32's GetAncestor, SendInput,
  TrackMouseEvent and GetWindowInfo, which earlier releases of rust-dos
  had under Windows 95 too, so a program that imports them needs `98`.
  A change takes effect for the next program that starts when none
  runs.
* `win32_trace` picks which calls of the built-in Windows' functions
  get a `[W32]` line in the [log file](README.md#log-file) (and `[W16]`
  for the 16-bit modules), with the caller's address, the arguments and
  the result. `collapsed` (the default) writes the first 3 calls from
  each place in a program in a second of emulated time and counts the
  rest, then writes a line with the count, such as
  `[W32] 004D72B6: WINMM.timeGetTime x49812 more in 1.0 s`; a loop that
  goes on gets one line and a count a second. A game that polls tens of
  thousands of times a second thus leaves room in the log for what
  comes later. `all` writes every call, and `off` none. The debug
  server's `POST /api/win32 {"trace":"all"}` changes it while a program
  runs.
* `win32_depth` is the depth of the built-in Windows' desktop: `auto`
  (the default) is the display adapter's own, 32 bits a pixel (24 on an
  S3 ViRGE, 16 on a 3dfx Banshee or Voodoo3); `8` is 640x480 with 256
  colours from a palette, as a Windows 3.1 or 95 display driver of 256
  colours gave it. Programs that say they need 256 colours, such as
  Iron Helix, need `8`. GetDeviceCaps then reports a palette device
  (RC_PALETTE, 256 palette entries, 20 of them static), logical palettes
  take entries of the system palette when they are realized, and
  AnimatePalette changes the colors on the screen at once. Like
  `win32_version`, a change takes effect for the next program that
  starts when none runs.
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
* `mouse_sensitivity` multiplies captured mouse movement on both axes:
  `0.1` to `10.0`, default `1.0`. The settings slider adjusts in `0.1`
  steps. Uncaptured positioning is unchanged.
* `mouse_capture_messages` says over the picture when the mouse is
  captured (and that Ctrl+Alt lets it go) or let go: `true` (the default)
  or `false`.
* `mouse_menu_hold` opens the settings window when the right mouse button
  is held for 1.5 seconds without moving the mouse, for a TV or a remote
  with no F12 or no keyboard at all: `true` (the default) or `false`. The
  program sees the press, and a release as the window opens; moving the
  mouse while holding the button keeps it an ordinary press. Turn it off
  for a game that has you hold the right button still.
* `pause_in_settings` pauses the program while the settings window is
  open (but for the Mixer and Stats pages): `true` (the default) or
  `false`, to let it run on behind the window. In a LAN room it always
  runs on.
* `version_check` asks rust-dos.com at startup whether a newer version
  is out (`https://rust-dos.com/version.json`); one that is shows in a
  notification over the picture and in a line under the box above the
  first DOS prompt. When the answer is slow to come, the line names the
  version the last check found, and the notification follows once the
  answer is in. `true` (the default) or `false`. Deterministic mode and
  the debug server never check.
* `almanac_auto` asks almanac.rust-dos.com for the settings players
  found best for a program started from the prompt, by its file's name,
  size and checksums, and starts it with them; your own settings come
  back once it has ended. A moderator's blessed settings win over the
  ones most reports chose. The program waits ten seconds at most for the
  answer, and Esc ends the wait. Only the machine's settings (CPU,
  memory, sound cards and the like) come from it, never drives,
  commands or paths. A game launched from its profile and
  deterministic mode never ask. `true` or `false` (the default).
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
* `idle_skip=true` (the default) lets the emulator skip the passes of a
  program's busy-wait loops up to the next timer event, and sleep
  instead of running them: a program polling the keyboard through the
  BIOS, waiting for the vertical retrace, for the timer's tick count to
  change, or for a time it reads from the PIT's count, as Windows 98's
  QueryPerformanceCounter does. A loop is skipped only after the
  emulator has seen one of its passes come back with the registers and
  the memory it wrote as they were, or as the same function of the PIT's
  count it read, and with nothing else to tell the passes apart (it
  reads no device but the video card's status, whose bits it looks at
  only through a mask, and the PIT's count), so programs see the same as
  without it. `false` runs every pass.
* `idle_hint=true` lets Windows' own idle calls wait for the next timer
  event, as a halted CPU does: Windows 3.1's idle loop in 386 enhanced
  mode (its VMPOLL device calling out, and the kernel's idle call), which
  reads the timer in every pass, so `idle_skip` can't skip it. Unlike
  `idle_skip` this isn't exact: the loop runs fewer passes than on a real
  machine, which Windows doesn't notice, but timing-sensitive programs
  might. Off by default.
* `hard_disk_speed` and `floppy_disk_speed` slow the disks down to those
  of the time; see [Disk speed and noises](#disk-speed-and-noises).

### `[sound]`

* `sbtype` is the Sound Blaster: `sb16` (the default), `sb16vibra`,
  `awe32`, `sbpro2`, `sb2` or `none`. `sbbase` (hex), `irq`, `dma` and
  `hdma` (the SB16's and AWE32's 16-bit channel) set its resources; the
  defaults are 220, 7, 1 and 5. The `BLASTER` environment variable follows
  them.
* `sb16vibra` is an SB16 as Creative's ViBRA16C (CTL0070, DSP 4.13), an
  ISA Plug and Play card, as DOSBox-X's `sbtype=sb16vibra` is. The BIOS
  leaves it configured at the resources above, so DOS programs find it as
  they find an SB16. Windows 95 finds it as a Plug and Play device
  ("Creative ViBRA16C PnP", with its audio device CTL0001 and its game
  port CTL7001) and gives it the resources it picks: the card's ports,
  IRQ and DMA channels move there. The MPU-401 stays at 330h, the OPL3 at
  388h and the game port at 201h whatever Windows gives those parts.
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
  in `ULTRASND.INI` in `ultradir`), `soundfont`, `gus`, `mt32`, `sc55`,
  `host`, or `none`. The built-in patches play even without the Ultrasound
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
* `sc55` plays a Roland Sound Canvas: the SC-55, SC-55mkII, SC-55ST,
  SC-155 (mkII), CM-300/SCC-1, SCB-55 or RLP-3237, built into rust-dos as
  a port of [Nuked-SC55](https://github.com/jcmoyer/Nuked-SC55), which
  emulates the module's chips and runs Roland's own firmware. The
  firmware and sounds are Roland's ROMs, which don't come with rust-dos.
  rust-dos finds them by their contents, whatever the files are called,
  in subfolders and in zip archives too: in `sc55roms`, or without it in
  `sc55-roms` in its configuration directory and in DOSBox Staging's
  `soundcanvas-roms`. The settings window's Sound page can download them
  from the Internet Archive once you agree to it (*Download the Sound
  Canvas ROMs...*), checks every file against the known dumps, and puts
  them in `sc55-roms`. `sc55model` picks the module: `auto` (the default:
  the SC-55 v1.21, which most games were written for, if its ROMs are
  there, else the SC-55mkII, else any other found), a model (`mk1`, `mk2`,
  `st`, `sc155`, `sc155mk2`, `cm300`, `scb55`, `rlp3237`) or a version of
  one (`mk1-v1.21`, `mk2-v1.01`, ...). The module needs a moment when it
  is switched on and after a GS reset (the mkII 1.6 s), as the real one
  does. Text a game puts on the display appears over the picture.
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
* `pit2_gate_at_start=true` starts PIT channel 2, the PC speaker's
  timer, with its gate (port 61h bit 0) high and its count running in
  mode 3 from FFFFh, with the speaker silent. The default, `false`, starts
  it with the gate low and the count stopped, as on a real PC, whose BIOS
  clears port 61h bits 0 and 1 after the POST beep, and as in DOSBox and
  86Box. Bao Xiao San Guo Zhi (SANGO.EXE) needs `true`: it reads its
  random numbers from the channel without setting it up or raising the
  gate, so with the gate low every roll is the same. DOSBox-X's
  `pcspeaker clock gate enable at startup` is the same setting and
  imports as this one. Put it in the game's
  [profile](#game-profiles) to have it for that game only. A change takes
  effect at the DOS prompt.
* `hard_disk_noise` and `floppy_disk_noise` add the drives' noises; see
  [Disk speed and noises](#disk-speed-and-noises).

### `[mixer]`

The volume of each sound source in percent, from 0 to 200: `speaker` (the
PC speaker and the prompt's beeps), `sb` (the Sound Blaster's digital
audio), `fm` (the FM synthesizer), `gus`, `midi`, `cdaudio`, `disknoise`,
`lptdac` (the Covox or Disney Sound Source), `tandy` (the Tandy's and
PCjr's sound chip), `awe32` (the AWE32's wavetable synthesizer),
`monitor` (the [monitor's effects](#emulator)), `modem` (the [modem's
speaker](#serial-and-modem-games)), `windows` (the built-in Windows's
sound: DirectSound and WinMM's wave audio), and `master` for all of them
together. At 100, the default, a source plays as loud as its card makes
it. The volumes apply on top of the Sound Blaster's own mixer, which
programs set.

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
* `blocksize` and `prebuffer` set how much sound waits for the host's
  sound device, and so how late it is heard: `blocksize` is the device's
  buffer in frames, `128`, `256`, `512` (the default, about 12 ms),
  `1024` or `2048`, and `prebuffer` the queue kept on top of it for a
  video frame that comes late, in milliseconds: `5`, `10`, `15`, `20`
  (the default), `25`, `30`, `40` or `50`. Smaller values bring the
  sound sooner; if it crackles, the host can't keep up with them. Only
  the rust-dos program has them: the libretro core and the browser leave
  the buffering to the frontend.
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
    are a joystick each, and with none nothing is plugged into the port
    (programs that find a joystick would ask to calibrate it).
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
  default), a port (`2323`, on all of this computer's addresses), or an
  address and a port (`127.0.0.1:2323` takes calls from this computer
  only, `[::1]:2323` the same over IPv6). A call rings the modem; `ATA`
  answers it, or `ATS0=1` answers the first ring.
* `modemtelnet` makes the modem's TCP calls speak telnet, for BBSes:
  `off` (the default: the characters as they are, as DOSBox's modem sends
  them) or `on`. Calls to the phonebook's numbers speak telnet anyway,
  unless their entry says `raw`.

### `[phonebook]`

Numbers of your own for the modem's phonebook (see [Serial and modem
games](#serial-and-modem-games)), one per line: the number, then the
board's host and port (23 unless given), then optionally `raw` for a
plain TCP call without telnet, and a name. Dashes in numbers are only for
reading: `ATDT 5550199` dials `555-0199` too. A number here comes before
rust-dos.com's and the built-in ones. The settings window doesn't change
this section; edit it in the file.

```ini
[phonebook]
555-0199 = bbs.example.org Example Board
555-0198 = 192.0.2.7:2323 raw My other board
```

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

### `[shell]`

The DOS prompt's line editor (see [the prompt](README.md#the-prompt)).

* `autosuggest` shows the rest of the line in dark grey, from the newest
  matching line in the history or else the first name Tab would complete:
  `true` (the default) or `false`.
* `colors` colors the prompt and the line typed at it: `true` (the
  default) or `false`.
* `save_history` keeps the history in `shell_history.txt` in rust-dos's
  configuration directory (the libretro core: in its save folder), so it is there after a
  restart: `true` (the default) or `false`. A line that begins with a
  space is never kept.
* `history_size` is how many lines are kept, 1 to 100000 (1000 by
  default).
* `prompt_color`, `command_color` (built-in commands), `executable_color`
  (programs found, drive letters), `unrecognized_color` (anything else
  where a command goes), `argument_color`, `flag_color` (`/X` switches)
  and `suggestion_color` are the colors, each one of `black`, `blue`,
  `green`, `cyan`, `red`, `magenta`, `brown`, `lightgray`, `darkgray`,
  `lightblue`, `lightgreen`, `lightcyan`, `lightred`, `lightmagenta`,
  `yellow`, `white`, or `default` for the screen's own. They are set only
  here; the settings window turns them on and off.

`PROMPT` may color itself with ANSI sequences after `$E`, as with clink:
`PROMPT $E[1;33m$P$E[0m$G` shows the directory in yellow. Codes 0
(reset), 1 and 22 (bright), 30 to 37 and 90 to 97 (text), 40 to 47 and
100 to 107 (background), and 39 and 49 (back to the default) work.

### `[vr]`

The picture on a screen in a 3D scene (see [3D scene and VR](#3d-scene-and-vr)).

* `mode`: `off` (the default), `desktop` (the scene in the window, with a
  camera to fly around) or `headset` (a VR headset through OpenXR, and the
  left eye's view in the window). It takes effect at the next start.
* `scene`: the scene, a glTF file (`.glb`, or `.gltf` with its files)
  exported from Blender, relative to the configuration file's folder.
  Empty for the built-in room. Changed in the settings window, it shows at
  once. The VR page's *Scene* lists the scenes downloaded and, when asked,
  those [rust-dos.com](https://rust-dos.com/vr/scenes.json) lists, which
  Enter downloads into `vr-scenes` in rust-dos's own directory.
* `controllers`: what a headset's controllers do: `both` (the default: the
  pointing hand is the mouse, the rest a gamepad), `pointer` or `gamepad`.
  See [VR headsets](#vr-headsets).
* `spatial_audio`: the sound comes from the screen's sides, or the scene's
  speakers, as the viewer turns and moves: `true` (the default) or `false`.
* `screen_fit`: how the picture fills the scene's screen: `auto` (the
  default: as the scene's `rustdos_screen_fit` property says, keeping the
  picture's shape without one), `fit` (keep the picture's shape, with black
  bars where the screen's shape differs) or `stretch` (cover the whole
  screen).
* `quality`: how much of the scene's lighting is worked out: `auto` (the
  default: `high`, or `medium` on a standalone headset's own graphics
  chip), `high` (soft shadows, the screen's light in twelve patches of the
  picture's colours, light bouncing around the room, and ambient occlusion
  darkening corners and gaps), `medium` (four patches, a little less soft)
  or `low` (hard shadows, the screen's light in one colour, no bounced light
  or ambient occlusion), for slow graphics chips. It takes effect at once.
* `ambient_occlusion`: the darkening of corners and gaps: `auto` (the
  default: as `quality` has it, but off on a standalone headset), `on` or
  `off`. It takes the graphics chip a pass of its own.
* `msaa`: how many samples each pixel of the scene takes, for smooth
  edges: `auto` (the default: 4, or 2 on a standalone headset), `off`, `2`
  or `4`.
* `screen_glow`: how brightly the screen lights the room, in percent of what
  the scene says, 0 to 400 (the default 100). 0 turns its light off.
* `scene_scale`: how big the scene looks in a headset, in percent, 50 to
  200 (the default 100). Above 100 the room is bigger and you smaller.
* `seat_right`, `seat_up`, `seat_forward`: where you sit in a headset from
  the scene's `spawn`, in cm, -100 to 100 (the default 0): to the right,
  higher and closer to the screen, as the spawn faces.
* `seat_turn`: how far you are turned to the left of the spawn's way, in
  degrees, -180 to 180 (the default 0).
* `resolution`: the headset's eye images, in percent of the size its
  runtime recommends, 30 to 150, or `auto` (the default): the recommended
  size, and on a standalone headset as much of it drawn, from half up, as
  the graphics chip has time for at the headset's rate (the runtime
  scales it up to fill the view). A number keeps the size it says. It takes
  effect at once.
* `foveation`: draws the edges of a headset's view coarser than where the
  eyes look, following the gaze: `auto` (the default: `medium` where Valve's
  foveation layer is installed, as on the Steam Frame), `off`, `low`,
  `medium` or `high`. It uses Valve's own layers
  (`VK_LAYER_VALVE_fdm_injection`), which reach OpenGL through Zink. It does
  nothing elsewhere. While it is on, `resolution=auto` draws all of the eye
  images. It takes effect at the next start.
* `keyboard`: shows the real keyboard (and the hands on it) through the
  scene with the headset's cameras, in a soft-edged window on the desk:
  `off` (the default), `auto` (below the screen, then where the fingers press
  keys while the hands are tracked) or `marked` (where it was marked with a
  controller from the settings window's VR page). It needs passthrough the
  program can use: OpenXR's alpha-blended environment (the Steam Frame
  running Rust-DOS itself, Varjo XR headsets) or `XR_FB_passthrough` (a
  Quest through Link). The places found and marked are kept in
  `vr-keyboard.txt` in Rust-DOS's folder. While it shows the real keyboard,
  the scene's keyboard (the one to type on with the controllers) is hidden.
  Turning it on or off starts the headset again.
* `refresh`: the headset's refresh rate in Hz, the runtime's nearest to
  it, or `auto` (the default: the runtime's own, or on a standalone headset
  the lowest from 72 Hz, for more time for each frame). Only where the
  runtime offers `XR_FB_display_refresh_rate`.
* `graphics`: how the headset's pictures get to the OpenXR runtime: `auto`
  (the default: OpenGL as the window's context allows, then Vulkan), `gl`
  (OpenGL through GLX, or WGL on Windows), `egl` (OpenGL through EGL,
  `XR_MNDX_egl_enable`, as Monado takes it) or `vulkan` (the pictures drawn
  with OpenGL and copied with Vulkan, for a runtime that takes Vulkan only).
  See [VR headsets](#vr-headsets). It takes effect at the next start.

The settings window's VR page has them all, and a **Center the view where
you sit now** button. Its Left and Right keys step the scale by 1%, the seat
by 1 cm and the turn by 1°, which take effect at once.

### `[vfx1]`

The Forte VFX1, a VR headset of 1995, for the games that support it (see
[The VFX1 headset](#the-vfx1-headset)): its card and driver in the
machine, and the headset on the desk of the 3D scene, to put on in a VR
headset.

* `enabled`: `true` puts the card in and the headset on the desk; `false`
  (the default) neither. With it, `VIPPORT` is set for games.
* `port`: the card's base port, which `VIPPORT` tells games: `260` (the
  default), `280`, `2A0` or `2C0`. The card answers at `300` too, where
  some games look for it.
* `stereo`: how each eye's picture is taken from the screen:
  * `auto` (the default): as the game tells the driver or the 3D BIOS
    (`stereo_bios`), or else, while the game reads the head tracker, VFX1
    pages on a 640x480 screen and lines on one of 400 lines; and the
    whole screen for both eyes otherwise (menus).
  * `pages`: the VFX1's own stereo, which most of its games draw: the
    left eye on the left half of a 640x480 screen and the right eye on
    the right, each page on every other line.
  * `lines`: even lines the left eye, odd lines the right, as games for
    the i-glasses! and the CyberMaxx draw (Quake's `lcd_x` too).
  * `side`: the left half of the screen the left eye.
  * `mono`: the whole screen for both eyes.
* `swap_eyes`: `true` swaps the eyes' pictures; `false` is the default.
* `view_size`: how wide the pictures look, in degrees, 20 to 110. The
  default, 36, is the VFX1's own; more fills more of the view.
* `steer`: `true` makes the head's movement between the game's frames
  larger as it draws more than 30 frames a second, for games that turn
  by it, as Descent turns the ship; `false` (the default) passes the head
  as it is.
* `stereo_bios`: `true` (the default) also answers games as the 3D BIOS
  of shutter glasses (Kasan's 3DMax, the LCDBIOS of SimulEyes and
  others: INT 10h AX=4ED0h and 4FD0h), so that the VFX1 shows what they
  draw for those, with `stereo=auto` as interlaced lines; `false` leaves
  them without one.

`enabled` and `port` take effect at the DOS prompt; the others at once.

### `[drives]`

Each line is `LETTER = PATH [more images] [floppy|hdd|cdrom] [-label NAME] [-ro] [-chs C,H,S] [-boot] [-overlay DIR]`,
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

A reboot runs them again, with `C:\AUTOEXEC.BAT`. Ctrl+Alt+Del reboots, as
does a program that resets the machine (through the keyboard controller,
port 92h or CF9h, INT 19h or a jump to the reset vector). Memory, the
devices and the resident programs start over, and the screen is cleared.

## Settings window

Press **Ctrl+F12**, type `DOSCONFIG` at the DOS prompt, or hold the right
mouse button still for 1.5 seconds (`mouse_menu_hold`) to open the
settings window over the running program. The program pauses while it is
open, except on the Mixer page, where it plays on so you hear the volumes
as you set them, and the Stats page, which shows it running; with
`pause_in_settings` off, or in a LAN room, it never pauses. Ctrl+F12 or
Esc closes it.

* **Drives:** mount a host directory, a zip or 7z archive, or a disk or
  CD image (Ins), change or
  swap the one a drive shows (Enter; this is how to change discs in the
  middle of a game), or unmount it (Del). **Browse...** picks directories
  and images, VHDs among them, from the host. **Create a disk image...**
  makes a new, empty floppy or hard disk image, as
  [MAKEIMG](#new-disk-images) does, and mounts it on the drive picked (a
  floppy on A: or B:, where free); a hard disk's **Format** makes it a raw
  image or a dynamic (growing) or fixed VHD. B (or
  the mount dialog's **Boot**) [boots](#booting-a-disk-image) the disk
  image of the selected drive now, and the dialog's **Auto-boot** has it
  boot whenever Rust-DOS starts, once saved (F2). Its **Write to** is a
  folder for the changes of a directory, an archive or a disk image
  (`<image>.rdelta` there), which leave it as it is
  ([`-overlay`](#mounting-drives)); such a drive is marked *ovl*. Its
  **IDE slot** puts a hard disk or CD-ROM drive on the IDE channel a
  booted system finds it on ([`-ide`](#booting-a-disk-image)); the first free one
  otherwise. Past the letters, its **Drive** offers the BIOS disk numbers
  0 to 3 (`[drives]` `2=win98.vhd`): an image only the BIOS has, for
  booting, without a DOS drive. They are listed after the lettered drives
  with their BIOS unit (*00h*, *01h*, *80h*, *81h*), and **Create a disk
  image...** can mount a new image on one of its kind.
* **Display:** the scale, fullscreen, fixed aspect ratio, variable
  refresh rate (VRR), the scaling filter, the CRT shader and the monochrome monitor.
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
but EXIT; what doesn't come across goes to the log. DOSBox Staging's
`cpu_cycles` and `cpu_cycles_protected` become `cycles` (a real-mode speed
with a faster protected mode is `auto` with that speed), `CONFIG -SET`
lines become the profile's settings, and `LOADFIX` runs the program it
names (programs load above 64 KB anyway). A value rust-dos can't have,
such as a Sound Blaster on IRQ 4, is left out for the default. A
configuration that mounts no C: gets its own folder as C:, and paths it
doesn't find from its folder are looked for in the folders up to four
levels above, as collections such as eXoDOS run DOSBox from their root.
[Packages](GAME-PACKAGES.md#packages-made-for-dosbox) made for DOSBox
are imported the same way. A GOG install whose launcher starts a Windows
program instead of DOSBox becomes a profile for the [built-in
Windows](#windows-games): its folder is C:, and the commands change to
the task's working folder and run the program by its DOS names, with
its arguments.

The settings window's **Games** page lists the profiles. Enter launches a
game at the DOS prompt: its settings apply on top of the configuration's,
its drives are mounted, and its commands run. When they have run and the
program they started has returned to the prompt, the settings and drives
(those its commands changed too) are back as they were, and a line above
the prompt says the game has ended. A program that quits at once (a
Windows that won't start, say) ends the game too, which is why its
drives are gone at the prompt after it. A game whose commands start no
program stays, with its drives, until another is launched. **Ins** makes
a profile from the settings as they are now: only those that differ from
the configuration file's, and the drives mounted since, go in it, with
the game's name, its directory and the command that starts it. While a
game plays, F2 saves the settings window's changes to its profile.
`--game NAME` launches a game at startup, by its file name (`keen4`) or
its name, and so does `?game=NAME` in the browser.

A profile with `overlay=true` in `[game]` keeps the game's files as they
were installed. New and imported profiles, and those of archives, have it. What the game
writes on its own drives (the host directories and disk images in its
`[drives]`, but not CD-ROMs or `-ro` drives) goes to `saves/<profile>/<drive letter>` beside
the `games` folder instead: saved games, setup changes, patches. Files the
game deletes are hidden, and listed in that folder's `.rust-dos-deleted`.
That folder holds everything the game changed, so it is the one to back up
or sync, and **R** on the Games page deletes it, taking the game back to
how it was installed. A drive with its own `-overlay` keeps it. In the
libretro core, `saves` is in the core's folder in the frontend's save
directory.

### Windows games

A Windows 95 or 98 game, or a Windows 3.1 program, runs in one of two
Windows. `windows=` in the profile's `[game]` picks which:

| | `windows=builtin` | `windows=image` |
|---|---|---|
| What runs the game | rust-dos's own Windows: the system DLLs are written into the emulator, and the game starts from the DOS prompt | A real Windows 95 or 98, installed on a disk image that the profile's `[autoexec]` boots |
| What you need besides the game | Nothing | A Windows CD, and the time to install Windows and its drivers on an image ([docs/win9x-howto.md](docs/win9x-howto.md)) |
| What it runs | Games that use what the built-in DLLs have: GDI, DirectDraw, DirectSound, DirectInput and DirectMusic of DirectX 3 to 6, Direct3D 5, 6 and 8, OpenGL and Glide 2 | What ran on that Windows with its drivers |
| State | Experimental: not every game runs yet | Depends on how well the emulated display, sound and 3D cards run their Windows drivers |

Start with `windows=builtin`. If the game doesn't run there, see
[when a game doesn't run](#when-a-game-doesnt-run), and please
[open an issue](https://github.com/dividebysandwich/rust-dos/issues)
that names the game, so the built-in Windows can learn to run it. Until
then, run it in an image.

`windows=builtin` turns [`win32`](#emulator) on while the game plays
and off when it ends. With `windows=image` the profile gets a warning
when its `[autoexec]` boots nothing (see
[an operating system](GAME-PACKAGES.md#an-operating-system)). A GOG
install whose launcher starts a Windows program gets a `windows=builtin`
profile when it is [imported](#game-profiles). The rest of this section
is about the built-in Windows.

#### A profile to start from

Once the game is [installed](#getting-the-game-installed), a profile
like this one runs it:

```ini
[game]
name=Enemy Infestation
windows=builtin

[emulator]
machine=svga_voodoo3
cpu=pentium_ii
memsize=64
win32_version=98se

[drives]
C=~/dos/ei
D=~/cd/EnemyInfestation.iso cdrom

[autoexec]
SET TEMP=C:\WINDOWS\TEMP
C:
CD \PROGRA~1\ENEMYI~1
EI.EXE
```

The settings in `[emulator]` are the ones a Windows game cares about.
Their defaults suit DOS games, so set them in every Windows game's
profile:

| Setting | Set it to | Why |
|---|---|---|
| `machine` | `svga_voodoo3` | Direct3D, OpenGL and Glide games draw on an emulated 3dfx card. The Voodoo3 has 2D and 3D on one card, so 3D shows in a window too, and with the Banshee it is the only card Direct3D 8 and OpenGL run on. `GLIDE2X.DLL` is there only with a 3dfx card. GDI and DirectDraw games run on it too |
| `cpu` | `pentium_ii` | The default 486 has no time stamp counter (RDTSC) or MMX, which many games of the time use: such a game stops with an invalid opcode. The Pentium II has both |
| `memsize` | `64`, or `128` for a 3D game from 1998 on | The default 16 MB is less than most Windows 98 games asked for. The game's heaps and its DirectDraw surfaces come out of it, and it is what the game is told the machine has |
| `win32_version` | `98se` | The setups and games tested with the built-in Windows were tested under Windows 98 SE. Under the default, `95`, a program that imports a function Windows 98 added doesn't start, and its message names this setting |
| `win32_depth` | `8` only for a game that says it needs 256 colours | The default desktop has the card's own depth (16 bits on a Voodoo3). A GDI game that draws with a palette needs `8`. DirectDraw games set their own mode and don't need it |
| `cycles` | leave it at `auto` | The game's timers follow the emulated clock, and `auto` finds the speed its frames need |

The `[autoexec]` lines run in the built-in DOS, so they use 8.3 names
(`PROGRA~1` for `Program Files`; `DIR` at the prompt shows them).
`SET TEMP` points the temporary folder at a writable drive: the built-in
DOS sets none, and without it a program that asks Windows for that
folder gets the current one, which is the read-only CD when a setup
starts from there.

#### Getting the game installed

A digital release, such as GOG's, is installed already: import it, or
[copy its folder](#copying-an-installed-game). Most games on CD have to
be installed before they run. Try these in order:

| Way | Use it when | Registry keys |
|---|---|---|
| [Run the setup](#running-the-setup-on-first-start) the first time the game starts | The setup is InstallShield 5, InstallShield 3 with `_INST32I.EX_` beside `SETUP.EXE`, or Wise | The setup writes them |
| [Unpack the installer](#unpacking-the-installer) with an `[install]` section | The setup doesn't run, and the CD has InstallShield cabinets (`data1.hdr`, or a `data1.cab` that holds the header) or an InstallShield 3 archive (`_SETUP.1`) | From a [`.reg` file](#registry-keys) |
| [Copy an installed game](#copying-an-installed-game) | Anything else, whatever the installer. GOG and other digital releases run this way | From a [`.reg` file](#registry-keys) |

#### Running the setup on first start

Put the setup in `[autoexec]` behind `IF NOT EXIST`, so it runs only
while the game's program isn't there yet. The lines after it start the
game once the setup has ended:

```ini
[game]
name=Enemy Infestation
windows=builtin

[emulator]
machine=svga_voodoo3
cpu=pentium_ii
memsize=64
win32_version=98se

[drives]
C=~/dos/ei
D=~/cd/EnemyInfestation.iso cdrom

[autoexec]
SET TEMP=C:\WINDOWS\TEMP
D:
IF NOT EXIST C:\PROGRA~1\ENEMYI~1\EI.EXE SETUP.EXE
C:
CD \PROGRA~1\ENEMYI~1
EI.EXE
```

The setup asks its questions as it would on Windows, and writes its
files, registry keys and shortcuts on the game's drives. With
`overlay=true` they go to the saves folder, so **R** on the Games page
takes the install away and the next start runs the setup again.

The settings of [the profile above](#a-profile-to-start-from) apply to
the setup as well as the game, and `SET TEMP` matters most here. The
setups listed in
[docs/native-windows.md](docs/native-windows.md#setup-programs-that-ran)
are known to run. For a game on more than one CD, list every image on
the drive's `[drives]` line; when the setup asks for the next disc,
Ctrl+F4 puts it in. Run `DIR` at the prompt after the setup to find the
8.3 names of the folders it made.

InstallShield's `SETUP.EXE` is a 16-bit launcher, which the built-in
Windows doesn't run. When `_INST32I.EX_` is beside it, the built-in
Windows unpacks the 32-bit engine from that archive into `TEMP` instead,
and starts it with the command line the launcher gives it on Windows 98
SE.

If the setup doesn't run, look in [the log](README.md#log-file):

- **`SETUP.EXE` says it needs Windows:** it is a 16-bit program the
  built-in Windows doesn't run. Either it isn't InstallShield's
  launcher, or `_INST32I.EX_` isn't beside it. Unpack the installer or
  copy an installed game instead.
- **A `[W32] ... is InstallShield's setup launcher: its engine runs`
  line, and then the engine ends without a window**, or with a message
  naming a function the built-in DLLs don't have: the engine can't run
  yet. Unpack the installer or copy an installed game instead.

#### Unpacking the installer

An `[install]` section unpacks the game from its installer's archives
onto its drives the first time it starts, without running the setup. It
reads InstallShield 5 and 6 cabinet sets (by their `data1.hdr`, or a
`data1.cab` that holds the header) and InstallShield 3 archives
(`_SETUP.1`, a `.Z` file):

```ini
[install]
from=D:\SETUP\DATA1.HDR
to=C:\ONI
group=Files (Program)
group=Files (Data)|GameDataFolder
```

| Key | What it does |
|---|---|
| `from=` | The cabinet set's header or the InstallShield 3 archive, by its DOS path. Each `from=` starts a step of its own, and the lines after it belong to that step |
| `to=` | The folder the files go in, with its drive |
| `group=NAME` | Unpack the cabinet's file group `NAME` (any case) into `to=`; `NAME\|FOLDER` into that folder under it. Without `group=` lines every file goes to its own path. An InstallShield 3 archive has no file groups, and a step that names one fails |
| `compression=` | `chunks` (the default) or `markers`, for version 0 sets whose compressed data ends each chunk with `00 00 FF FF` (Unshield's `-O`) |

The cabinet set doesn't say which folder each file group goes to: the
setup's script does. To find out:

1. List the set's file groups, and each file with the groups that hold
   it, with
   [`rust-dos unpack --list`](#unpacking-installshield-installers).
2. In an installation of the game, find a few files of each group. The
   folder under the install folder they are in is that group's
   `FOLDER`.

A file the set holds more than once at one path (one per language, say)
is unpacked only by a group that places it.

The section writes no registry keys and makes no shortcuts: put the
keys in a [`.reg` file](#registry-keys). A step that wrote every file
leaves `RUSTDOS.INS` in its folder and doesn't run again while that file
is there. One that didn't says why on the screen and runs again at the
next start. With `overlay=true` the files go to the game's saves folder,
so **R** on the Games page takes the install away and the next start
unpacks it again.

#### Copying an installed game

Install the game on Windows 98 (or take a digital release's folder),
copy its folder to the drive the profile mounts, and put its registry
keys in a [`.reg` file](#registry-keys). This works whatever the
installer.

#### Registry keys

A file `<profile>.reg` beside the profile (`games/oni.reg` for
`games/oni.conf`) holds the registry keys the game reads, as REGEDIT4
text. Each time the game starts, they are written into the built-in
Windows' registry, `C:\REGISTRY.REG`, over what is there. The game then
finds what its setup would have written. A [package](GAME-PACKAGES.md)
can't hold the `.reg` file: it goes beside the profile the package makes
in the `games` folder.

To get the keys:

- **From Windows 95 or 98:** in REGEDIT select the game's key, then
  Registry, Export Registry File.
- **From Windows 2000 or later:** export the same way, but save as
  "Win9x/NT4 Registration Files". The default format is Unicode text,
  which rust-dos reads as empty. On 64-bit Windows a 32-bit game's keys
  are under `HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node`: take
  `WOW6432Node\` out of their paths in the file.
- **From a setup that ran in the built-in Windows:** copy the game's
  sections out of `C:\REGISTRY.REG` (in the saves folder with
  `overlay=true`).

Then check the paths in the keys (the install folder, the CD's drive)
against the profile's `[drives]`, and change them where they differ.

#### The CD's drive letter and label

A game finds its CD by the drive letter its setup recorded, in its
registry keys or its own files, and often by the disc's label too.

- Keep the CD on the letter it had when the setup ran. With keys from
  another machine, change the letter in the `.reg` file to the
  profile's.
- A CD image carries the disc's label. A folder copied from the CD needs
  the label given with `-label` (`D=~/cd/ei cdrom -label EI`).

#### When a game doesn't run

The [log file](README.md#log-file) says what the built-in Windows did
last. Go through these in order:

1. **It stops at once with an invalid opcode:** set `cpu=pentium_ii`.
2. **Its message names `win32_version`:** set `win32_version=98se`.
3. **It says it needs 256 colours:** set `win32_depth=8`.
4. **It says there isn't enough memory:** raise `memsize`.
5. **It can't find its CD or its files:** check its
   [registry keys](#registry-keys) and
   [the CD's drive letter and label](#the-cds-drive-letter-and-label).
6. **It ends with a message naming a function, or a DLL it can't
   find:** the built-in Windows doesn't have that yet. The last `[W32]`
   lines of the log name it.

For anything else, and for the last case,
[open an issue](https://github.com/dividebysandwich/rust-dos/issues)
with the game's name, its profile and the last lines of the log, and
run the game in a [Windows image](docs/win9x-howto.md) until it is
fixed. Issues about games that start but look, sound or play wrong are
welcome too.

### Keys at the start

`input=` in `[game]` presses keys once the game's program has started:
characters typed as they are, keys in parentheses (`(ENTER)`, `(F1)`,
`(SPACE)`), a key held and let go (`(leftctrl:DOWN)c(leftctrl:UP)`), and
waits: `(WAIT:500)` for that many milliseconds of the machine's time,
`(WAITMODECHANGE)` until the game changes the video mode, and `(DELAY:15)`
for the time between keys (70 ms at first). The machine runs fast through
the waits. A key or a gamepad button the player presses stops the rest.

```ini
[game]
name=Liero
input=(WAIT:200)(ENTER)(WAITMODECHANGE)
```

### Gamepad mappings

A `[gamepad]` section makes the first gamepad press the PC's keys, the
mouse and the joystick for the game, as it was made to be played with
them: each of the pad's inputs (`up`, `down`, `left`, `right`, the face
buttons `a`, `b`, `x`, `y` by where they are, `l`, `r`, `l2`, `r2`, `l3`,
`r3`, `select`, `start`, and the sticks' directions `lstick_left` to
`rstick_down`) is up to four of the PC's keys (by name: `space`,
`leftctrl`, `f10`, `kp5`...), `mouse_move_up` (`_down`, `_left`, `_right`),
`mouse_left_click` (`_right`, `_middle`), `mouse_speed_up` (`_down`), or
`joy_up` (`_down`, `_left`, `_right`), `joy_button1` to `4`, `joy_2_up`
and the like for the second stick (`joy_hat_up`, too), joined by `+`, then
what it does in words:

```ini
[gamepad]
x=space Jump
start=leftctrl+f10 Open Menu
rstick_up=mouse_move_up
l=wheel
wheel_1=1 Fists
wheel_2=2 Pistol
padmousespeed=150
```

An input bound to `wheel` shows the action wheel while it is held: the
`wheel_1`, `wheel_2`... items in a ring over the picture, from the top
clockwise. The left stick (or the D-pad) points at one, and letting go of
the button presses it. `padmousespeed`, `mousespeed` and `mousexfactor`
are the mouse's speed by the pad, by any means, and across, in percent;
`mousewheelup` and `mousewheeldown` bind the mouse's wheel. While a game
with a mapping plays, the first pad is its, not the game port's joystick
(but for what it maps to `joy_` actions); in the libretro core, it is the
first port played as **Gamepad as keyboard**. **P** on the Games page
lists a game's mapping.

### Manuals and extras

A game's manual, code wheel, maps and reference cards can go with its
profile, as PDF documents or PNG, JPEG or GIF pictures, and show over the
game without leaving it: what the copy protection asks is looked up there.
**Ctrl+Shift+M** shows the running game's, and **M** on the Games page any
game's: a list of them, then the one picked over the whole picture, the
game paused. Opened again, they open on the manual that was open, at the
page it was left on. PgUp and PgDn turn the pages, Up and Down scroll, `+` and `-`
zoom (the whole page, its width, 150% and 200%), `[` and `]` scroll
sideways, and Esc goes back. In the window the page is drawn at the
window's own pixels, sharper than the game's picture; in the libretro core
it is drawn into the picture.

They are the profile's `manual=` lines in `[game]`, one a file, with a
title after `|` if it should be listed as something other than its name,
and then the files in the extras folder beside the profile,
`games/<profile>.extras`:

```ini
[game]
name=Pool of Radiance
manual=pool/Manual.pdf|Rule book
manual=~/scans/pool-codewheel.png|Code wheel
```

Relative paths are from the `games` folder, and a path can go into a zip
or 7z archive (`pool.zip/EXTRAS/Rules.pdf`). **Ins** in the list adds a
file to the profile. A game played from a [package](GAME-PACKAGES.md) has
the files in the package's `EXTRAS` folder too.
[GAME-PACKAGES.md](GAME-PACKAGES.md) is a guide to manuals and packages.

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

The site knows a game by the files of the zip (or `.dosz`)
it came in, so achievements are for games launched from their
[profiles](#game-profiles), and the profile says which version of the game
it is, in `[game]`:

```ini
[game]
name=Commander Keen 4
achievements=~/dos/archives/keen4.zip
```

`achievements` is the archive, relative to the `games` folder, or its hash
(32 hex digits). A zip dropped onto the window becomes a profile that
has its hash already; for other games, the Achievements page's
**Game's archive** row picks the archive and puts its hash in the profile.
Rust-DOS lays out the memory the MS-DOS sets read as they expect it:
the game's memory from 120h bytes below the
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
and a program or batch file is started from its folder. A game's
[package](GAME-PACKAGES.md), a zip or 7z archive (or `.dosz`)
or a folder with a `rust-dos.conf` at its root, becomes a game profile
with the package as C:, read where it is, its changes in the game's saves
(see [Game profiles](#game-profiles)), and is launched. The package's
`rust-dos.conf` gives the profile its settings, drives, manuals and
commands, its paths from the package's root; without commands, a single
program to start the game (setup and install programs aside) starts, else
the prompt is left on C:. Dropping it again launches the same profile, and
`--import` and the Games page's Import row take packages too.

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
access options are taken and ignored. A drive number instead of a
letter (`MOUNT 2 hdd.img`) mounts an image for the BIOS alone, as in
DOSBox (see [Booting a disk image](#booting-a-disk-image)). `MOUNT /?`
lists the options. Each drive keeps its own current directory, as in DOS.

A CD image always makes a read-only CD-ROM drive, labelled with the disc's
volume name unless `-label` says otherwise. It can be a CUE sheet (`.cue`,
GOG's `.ins`, GOG's `.bdn` for BloodNet, or a file of any other name that
is at most 64 KiB and reads as a sheet; with BINARY, MOTOROLA or WAVE
files, any number of tracks and gaps), a CD image in MAME's compressed CHD
format (`.chd`, as `chdman createcd` makes them; its data and audio
tracks, decompressed as they are
read), or a bare image of an ISO 9660 data track in 2048, 2336 or
2352-byte sectors (`.iso`, `.bin`, `.img`, GOG's `.gog`). Audio tracks can
be Ogg Vorbis, FLAC or MP3 files (FILE ... MP3, OGG or FLAC, or WAVE as
many sheets call them), and WAVE files at other rates: they are decoded to
CD audio as they play. Programs run from the image as from any other drive.

An image of whole 2352-byte sectors (a BIN, or a CHD) keeps each data
sector's EDC, and a sector whose EDC doesn't match its data reads as a
drive reads it: as an error (MEDIUM ERROR, unrecovered read error, on the
IDE drive; a CRC error through MSCDEX, and a data error when DOS reads a
file over it). Some copy protections master such sectors on purpose and
refuse to run when reading them succeeds, so an image as Redump makes it
passes their check. Sectors with an EDC of 0, which tools that turn an
ISO into a BIN leave, aren't checked.

The type of a drive is the word after the path in `[drives]`, or `-t` at
the prompt:

| Type | Behaves like |
|---|---|
| `hdd` or `dir` (default) | A fixed disk. BIOS unit 80h and up. |
| `floppy` or `fdd` | A removable 1.44 MB disk whose free space reflects its files. |
| `cdrom` or `iso` | A read-only drive that programs detect through MSCDEX (INT 2Fh AX=15xxh) and as a remote drive. From a CD image it is a whole disc: raw and cooked sector reads, the volume descriptors, the table of contents, and audio tracks that play through the Sound Blaster mixer's CD volume. From a directory, only its files are available. |

`-ro` makes any drive read-only.

A zip, 7z or `.dosz` archive is a drive with its files, read
from the archive where it is: a stored file as it is wanted, a compressed
one decompressed in memory the first time it is opened (a 7z file with
the others of its solid block). An archive whose files are all in one
folder has that folder as the drive's root, and a `.dosz` with a
`<parent>.parent` file in it has the parent archive's files under its own.
An archive of a disk or CD image (its one CUE sheet or image, and no
programs) mounts the image instead. A path into an archive
(`game.zip/CD/GAME.CUE`, `game.zip/CD`) mounts the image or folder there,
and a list of images can be in one archive. An archive can't be written: without
`-overlay` the drive is read-only, and with it the changes go to its
folder, a disk image's to a delta file there. A game
profile's archives always get one (see [Game profiles](#game-profiles)).

`-overlay DIR` leaves a host directory as it is: what DOS writes, creates,
renames or deletes on the drive goes to `DIR` (made if it isn't there),
and the drive shows the directory with those changes. A file is copied to
`DIR` when it is first written, and one deleted from the directory is
listed in `DIR/.rust-dos-deleted`. Deleting `DIR` takes the drive back to
the directory as it is. DOSBox's `MOUNT C DIR -t overlay` gives the
directory mounted as C: the overlay `DIR`, and an imported DOSBox
configuration keeps it. A booted system's [shared
disks](#booting-a-disk-image) write their changes to the overlay too.
`MOUNT` lists where each drive's changes go.

A floppy or hard disk image with `-overlay DIR` is left as it is too: what
is written to it goes to `DIR/<image name>.rdelta`, 64 KB at a time, and the
rest is read from the image. That makes one image the base of many
setups, a Windows 95 install with each game's changes in its own folder:

```
MOUNT 2 win95.img -overlay saves/carmageddon
MOUNT 2 win95.img -overlay saves/fury3
```

Deleting the `.rdelta` file takes the disk back to the image. The delta
file is made for its image: if the image changes after, the delta file is
refused. An image in an archive gets a delta file in the overlay folder in
the same way (one copied there whole by an older Rust-DOS is still used).
State files of a system booted from such a disk keep a copy of the whole
disk, as they do of any other.

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

86Box's 86F floppy images (`.86f`, found by their contents whatever their
name) mount like the others. They hold each track's FM or MFM bit cells,
which are decoded into the disk's 512-byte sectors, the geometry coming
from the tracks. A sector written is encoded back into its track in the
file, so the image stays an 86F image (in memory, as from an archive or the
web page, the changes stay with its sectors, and it downloads as an
`.img`). A sector whose data doesn't match its CRC reads with INT 13h's
CRC error (10h) and a missing one isn't found (04h), as a real drive
reports them; there is no floppy controller, so copy protections that
depend on its timing or on sectors of other sizes don't see them, and weak
bits read the same each time. The image's write-protect flag
write-protects the disk.

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

Booted systems have a floppy controller enabled by default for images
mounted as A: and B: (or disks 0 and 1). Their own floppy drivers can read
and write those images directly. Changing a disk with Ctrl+F4 or
`IMGMOUNT` signals a disk change to the guest system. The built-in DOS
continues to access floppy images through INT 13h.

The first start of a Windows installed without the floppy controller finds
it as new hardware (**Standard Floppy Disk Controller**) and may ask to
restart; after that, **System Properties** → **Performance** no longer lists
A: and B: as using "MS-DOS compatibility mode". If it doesn't find it, run
**Control Panel** → **Add New Hardware** and let it detect the hardware.

Set `floppy_controller=false` in `[emulator]` to disable the controller
and leave floppy access to the BIOS (INT 13h). The controller uses ports
3F0h–3F5h and 3F7h, IRQ 6 and DMA channel 2. Transfers without DMA are
not supported.

### Operating systems by name

Hard disk images of installed systems in the `os` folder of rust-dos's
configuration folder (`~/.config/rust-dos/os` on Linux, `os` beside the
executable in a portable install; the libretro core's `system/rust-dos/os`) can be named
without their path or extension: `IMGMOUNT C WIN98SE` mounts `win98se.img`
(or `.vhd`) where no file of that name is in the folder MOUNT starts from,
and so does `REMOUNT WIN98SE C`. `REMOUNT` alone lists them. In a game
profile, whose drives keep their changes apart (`overlay=true`), each game
gets the image's changes in its own delta file, and the image stays as it
was installed.

VHD images can be fixed, dynamic (growing as they are written) or
differencing: the changes to another VHD, its parent, which is looked for
by the name the image gives it, beside it and then in the `os` folder
(see [an operating system](GAME-PACKAGES.md#an-operating-system) for
packages that ship a game's install this way).
A `.vhd` mounts like any other hard disk image, and a zip or `.dosz` that
holds one mounts it. The settings window's Drives page makes new ones
(**Create a disk image...**, **Format**).

An archive (`.dosz`, `.zip`, `.7z`) or a folder in the `os` folder is a
system of files rather than a disk image, Windows 3.1 say, which runs on
rust-dos's DOS: `REMOUNT WIN311 C` mounts `win311.dosz` as C:, and a game
package's `os=win311` puts the system on C: and the game on D:. An image
of the same name comes before it. It can't be booted or mounted by
number.

`REMOUNT C D` moves a drive to another letter: the game on C: becomes D:, the shared disk of a system booted from an
image then mounted on C: (see below). C: is left empty in its place.

`SUBST E: C:\GAMES\KEEN` makes a folder a drive of its own, with what is
written there going where the folder's drive puts it (a game's changes
folder, for an archive). `SUBST C: X:\GAME` after `REMOUNT C X` makes a
game's folder C:, for a game packed in a folder. `SUBST E: /D` takes the drive away again (C: is left empty).
When a game ends, the drives it changed this way are as they were.

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
restarting (Ctrl+Alt+Del, or Windows' **Restart**) boots the disk again. A
system booted from a floppy starts from the first hard disk when the floppy
drive is empty by then, as a PC that boots from A: and then C: does.
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

A DOSBox-X configuration's `IMGMOUNT` and `BOOT` lines work as they are,
such as these, which mount the Windows image by number and a CD on the
secondary IDE channel:

```
imgmount 2 win95.img -size 512,63,16,520 -t hdd -fs none
imgmount D game.iso -t cdrom -fs iso -ide 2m
boot -l c
```

The S3 driver shows the desktop up to 1024x768 in 32 bits per pixel with
its accelerated drawing and hardware cursor; the Sound Blaster 16 plays
Windows' sounds and FM music; the PS/2 mouse and APM work (Shut Down turns
the machine off). MS-DOS Prompts run in a window or full screen
(Alt+Enter), and **Restart in MS-DOS mode** works. Windows keeps the
hardware it was installed on in its registry, so an image installed on
other hardware may look for devices this machine hasn't.

Windows 95, OSR2 included, is the system the booted machine was built and
tried with, and Windows 98 SE runs too (see below). Windows 98 First Edition
and ME have no recorded run, so try them on a copy of the image. The machine
has APM and no ACPI, and its Sound Blaster
is a legacy card that Windows finds at the resources [`[sound]`](#sound)
gives it (DOSBox-X's `sbtype=sb16vibra` is an ISA Plug and Play card).
`memsize` is capped by the CPU, at most 512 MB with `cpu=pentium_mmx` (see
[`[emulator]`](#emulator)).

### Windows 98

Windows 98 SE runs on the same machine, with a Pentium Pro or Pentium II
for its processor support:

```ini
[emulator]
machine=svga_s3
cpu=pentium_ii
memsize=128
```

An image installed in DOSBox-X (archive.org has such packs, with their
`dosbox.conf`) starts as it is, imported as a game or mounted by hand, and
one installed in another emulator, 86Box say, as well. Windows finds this
machine's IDE and floppy controllers as new hardware at its first start and
asks to restart; an image installed elsewhere also asks for the Windows 98
CD for its drivers (mount it as a CD-ROM drive, or, where Windows can't read
it yet, copy its `WIN98` folder to `C:\WIN98` from the built-in DOS and
give Windows that folder). After that, **System Properties** →
**Performance** says the system is configured for optimal performance: the
hard disk and the floppy drives in 32 bits, the Sound Blaster 16 playing,
the S3 driver up to 1024x768 in 32 bits per pixel, MS-DOS Prompts,
**Restart in MS-DOS mode**, Shut Down, and save states.

A few things an image may bring with it:

* **All drives in MS-DOS compatibility mode:** Windows switches its IDE
  driver off for good when it fails once, as it can in other emulators,
  with the value `NoIDE` in the registry key
  `HKEY_LOCAL_MACHINE\System\CurrentControlSet\Services\VxD\IOS`.
  Delete the value with `regedit` and restart.
* **The network card:** Windows keeps the NE2000's port and interrupt as
  the image was set up with. Give rust-dos the same ones (`nicbase`,
  `nicirq` in [`[network]`](#network)), or change the adapter's resources
  in Device Manager to rust-dos's, and Windows gets its address from the
  router and reaches the internet.

To install Windows 98 SE on a new disk, make the disk with `MAKEIMG` (a
partition formatted FAT32, so Setup needn't partition and format it) and
boot the Windows 98 boot floppy with the CD in the CD-ROM drive:

```
MAKEIMG win98.img -t hd -size 2000 -fat 32
```

```ini
[drives]
0=win98boot.img floppy
2=win98.img
D=win98se.iso cdrom

[autoexec]
boot -l A
```

Choose **Start Windows 98 Setup from CD-ROM**. When Setup offers to make a
startup disk, cancel it and unmount drive 0 (the settings window's Drives
page, or the debug server): its restarts then start from the hard disk.
The installation takes about a quarter of an hour at full speed, and ends
on the desktop with everything above working. An empty disk image works
too: partition it with the floppy's FDISK (with the partition active),
restart and format it with FORMAT, then run Setup from the CD (`OEMSETUP`
in `D:\WIN98` on an OEM CD). An OEM boot floppy lacks FORMAT.COM, which
`A:\EXTRACT /L A:\ A:\EBD.CAB FORMAT.COM` puts on the floppy.

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
  Tomb Raider Gold CD) runs with `voodoo=true` and `cpu=pentium`. Its
  saved games don't load in the PowerVR version, nor that version's in
  it (see the PowerVR card below).
  Games made for the Voodoo Rush and later boards load Glide from
  `GLIDE2X.OVL` (Tomb Raider's Rush patch, `3DPATCH\VOORUSH\TOMB.EXE`),
  and the one of 3dfx's Voodoo Graphics driver drives this card whatever
  board they were made for. It is 3dfx's and doesn't come with rust-dos:
  the settings window's Emulator page downloads it, after asking, from the
  driver (3.01.00) the Internet Archive keeps, into rust-dos's
  configuration directory, and Z: has it from then on, where they find it
  on the PATH (one beside the game comes first). A game that looks for it
  while it is missing says so on the screen. Such a game runs without
  rust-dos's DPMI host: the overlay is a DOS/4G DLL whose start-up only
  works with DOS/4GW as its own host, at level 0. Its accesses to the
  Pentium II's memory type range registers, which it marks for 3dfx's
  Windows driver to catch, do nothing.
* **Windows 95** [booted from a disk image](#booting-a-disk-image) finds
  the card as a "PCI Multimedia Video Device", which needs no driver:
  games bring `glide2x.dll`, or install 3dfx's Glide runtime (and with it
  `fxmemmap.vxd`). SubCulture's 3dfx version installs from its CD in the
  [CD-ROM drive](#booting-a-disk-image) and plays with its CD music, with
  `machine=svga_s3`, `memsize=64` and `cpu=pentium`.
* **Windows 98 SE** finds the card at its next start as a "3Dfx Voodoo"
  and installs its own driver for it from the Windows 98 CD, asking for the
  CD's `WIN98` folder (give it the drive letter the CD has in this
  machine), without a restart. The driver gives DirectDraw and Direct3D the
  Voodoo as a second device beside the 2D card, and Direct3D games that let
  you choose the 3D card draw on it: Expendable does with
  `DisplayDriver [1]` in its `config.txt`, with `machine=svga_s3virge`,
  `cpu=pentium_ii` and `memsize=128`. While a game draws on the card,
  `video.voodoo.output` in the debug server's `/api/status` is true.

The card swaps its buffers at the vertical retrace a game asks it to wait
for, and reports the swaps still waiting and its busy state as the real
one does, so games that pace themselves by it run at its 60 Hz. The
triangles are drawn on up to four threads of their own: rust-dos keeps
running the game meanwhile, and the picture is the same however many
there are. Save states and rewind keep the card with everything in its
memory.

### 3dfx Voodoo2

`voodoo_model=voodoo2` makes the card a Voodoo2 (device 0002h), with
`voodoo_memory=8` or `12`:

```ini
[emulator]
voodoo=true
voodoo_model=voodoo2
voodoo_memory=12
```

Besides what the Voodoo Graphics does, it has the Voodoo2's command FIFO
(the packets its drivers send through memory instead of register writes),
its triangle setup engine (vertices in, triangles drawn), its 2D blitter
(screen to screen, CPU to screen, rectangle fills), the eight-bit
palettes with alpha (texture format 6), a base address for each mipmap
level, the wider video fields of its resolutions up to 1024x768, fog
dithering and the clamping of iterated colours, depth and W (fbzColorPath
bit 28). The board is SLI-less: one card, as Glide sees a single Voodoo2.

**Windows 98 SE** needs 3dfx's Voodoo2 driver, which neither Windows 98
nor rust-dos comes with: the Windows 98 CD has drivers for the Voodoo
Graphics and the Banshee only. Unpack 3dfx's reference driver 3.03.00
(`rkv2dx6.exe`, which the Internet Archive keeps) into a folder Windows
can read, such as a [host folder](#host-folders-in-a-booted-system). At
its next start Windows finds the new PCI device and opens the Add New
Hardware wizard: choose to search for the best driver, tick
"Specify a location", give the folder that holds `voodoo2.inf`, and
restart Windows when it is done. Device Manager then lists a "Voodoo2 3D
Accelerator", and DirectDraw and Direct3D offer the card as "3dfxV2
(Voodoo2 DirectX 6 Driver)", which Direct3D games that let you choose the
3D card draw on.

#### OpenGL games under Windows

Windows' OpenGL never uses the Voodoo2 by itself, and a game that draws
with OpenGL gets Microsoft's software renderer instead: it runs, but at a
frame every few seconds, and the card draws nothing (the debug server's
`/api/status` shows `video.voodoo.triangles` at 0). A real Voodoo2 had
the same problem. `OPENGL32.DLL` asks the desktop's display driver which
OpenGL driver to load and looks the name up under
`HKLM\Software\Microsoft\Windows\CurrentVersion\OpenGLDrivers`. The
Voodoo2 is a 3D card beside the display adapter, not one itself: the
desktop runs on the S3 (or whichever card `machine` gives), whose driver
names no OpenGL driver. 3dfx's driver copies its OpenGL driver to
`C:\WINDOWS\SYSTEM\3DFXVGL.DLL` but can't make Windows pick it.

Glide games and Direct3D games don't go through `OPENGL32.DLL` and need
none of this. An OpenGL game reaches the card in one of three ways:

- Games that let you name the OpenGL driver, such as Quake III Arena
  (`r_glDriver 3dfxvgl`), load `3DFXVGL.DLL` themselves.
- For a game without such a setting (Oni, for one), copy `3DFXVGL.DLL`
  into the game's folder as `OPENGL32.DLL`. Windows loads a program's
  DLLs from its own folder before `WINDOWS\SYSTEM`, so only that game
  uses the card. This is what 3dfx's users did.
- Copying it over `C:\WINDOWS\SYSTEM\OPENGL32.DLL` sends every OpenGL
  program to the card, which only draws full screen: programs that draw
  OpenGL in a window no longer can.

The copy into the game's folder can be made by the game's configuration,
which leaves the disk image as it is. With the image mounted with
[`-overlay`](#mounting-drives), the built-in DOS copies the file before
the system boots, and the copy goes into the overlay's `.rdelta` file:

```ini
[drives]
# Windows 98 with 3dfx's Voodoo2 driver and the game installed. The image
# stays as it is: what is written to it goes into the folder overlays/oni
# (made if it isn't there), as overlays/oni/win98.img.rdelta.
C=~/images/win98.img -overlay overlays/oni -boot
D=~/cds/oni.cue cdrom

[autoexec]
# Runs in the built-in DOS before the image boots.
copy c:\windows\system\3dfxvgl.dll c:\progra~1\oni\opengl32.dll
```

The built-in DOS sees the disk's short names (`dir c:\progra~1` lists the
folders under `Program Files`). The copy runs at every start and
overwrites the one the last start made, so the setup needs nothing else,
and the image under it can stay the base of other games' setups.

DOS games that bring the Voodoo2's `GLIDE2X.OVL` or Glide 3 run as on the
Voodoo Graphics.

`voodoo_fps_cap` caps the frames a second games show on the card, for an
even pace without lowering `cycles`: a swap waits for the first multiple
of the cap's frame time (1/30 s at 30) at least a frame after the last
one, and the program waits with it, as at a HLT, while interrupts and
their handlers go on, so its clock, music and sound keep time. A frame
that comes later than its turn waits for the next one, so pick a rate the
game keeps up everywhere. With `vrr=true`, while the card shows its
picture and the cap is below its 60 Hz, the window refreshes at the cap's
frame times. It doesn't apply to VGA games, whose frames come at the
display's retraces they wait for.

`voodoo_gamma` sets Glide's gamma through the environment variables
`SST_RGAMMA`, `SST_GGAMMA` and `SST_BGAMMA`, all three to the value. They
are put into the guest's environment while the card is on and the guest
hasn't set them; a variable the guest sets, changes or removes is the
guest's from then on, and only the variables rust-dos put in are taken out
again when the setting goes off or changes. Changes take effect at the DOS
prompt. A save state keeps which variables rust-dos put in, and loading one
puts in the variables the settings want.

With `voodoo_renderer=opengl` the window shows the card's picture drawn
again with OpenGL, at `voodoo_scale` times the card's resolution: the same
triangles through the same pixel pipeline (textures, fog, blending, the
depth buffer and the gamma table), with sharper edges and textures, and
with the card's dithering unless `voodoo_dither=false` (see below).
`voodoo_msaa` smooths the triangles' edges with multisampling (as many
samples as the graphics card has, at most); antialiasing forced in the
graphics driver's control panel doesn't reach the framebuffer objects it
draws into. What games write into the frame buffer themselves (menus,
movies) stays at the card's resolution. `voodoo_anisotropy` sharpens
textures viewed at an angle, where the OpenGL driver supports
anisotropic filtering.
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
bigger picture. The libretro core draws it with the frontend's OpenGL (see
[its README](libretro/README.md#the-3dfx-voodoo-with-opengl)).

The card stores 16 bits a pixel (5-6-5), and where a game turns
dithering on (fbzMode bit 8, which Glide sets by default) it adds a 4x4
or 2x2 ordered dither to each colour before it drops the low bits: a
fine pattern of dots that hides most of the banding between its shades.
The OpenGL buffers have 8 bits a component. With `voodoo_dither=true`,
the default, the shader dithers each triangle's colour with the card's
own matrix and rounding, and fastfills fill with the card's dithered
colour, so at `voodoo_scale=1` an unblended triangle or a fastfill comes
out pixel for pixel as the software rasterizer draws it. The matrix
follows the card's pixels, so at 2x and above each dot of the pattern
covers a block of 2x2 (or 3x3, 4x4) pixels of the bigger picture, as the
card's picture looks enlarged. `voodoo_dither=false` keeps all 8 bits a
colour: a cleaner, smoother picture, without the dot pattern or the
banding the 16 bits leave. Games that don't dither look the same either
way. It changes at once, in the settings window (*3dfx dithering*) too,
from the next frame the game draws. When the OpenGL picture starts again
from the card's memory (a new `voodoo_scale` or `voodoo_msaa`, a loaded
state), it starts from the card's 16 bits, so with `voodoo_dither=false`
the pattern shows until the game draws over it.

Not all of the card's dithering is there. OpenGL blends after the shader,
so a blended triangle is dithered before the blend instead of after it: it
has the pattern, but its pixels aren't the card's, and fbzMode bit 19
(taking the dither back out of the colour the blend reads) does nothing.
Some games blend most of what they draw (Oni blends every triangle), so
their pictures have the card's pattern but not its exact pixels. The
texture units' dithered LOD isn't there either, as OpenGL picks the mipmap
levels. Frame buffer writes and the Voodoo2's blitter show what the card's
memory holds, dithered or not, whatever the setting. The Voodoo2's and
Voodoo3's video output filter, which partly smooths the pattern on the
card's own output, isn't modelled: with the setting on, the pattern shows
as the card writes it.

The dithering costs a few more instructions a pixel in the shader, and
nothing on the host's CPU. On an Intel UHD Graphics 770 at the default
2x, Oni (a Voodoo2 game that dithers every triangle) ran the same either
way: 9.7 frames a second in a level and 38 in the menu, with the same
CPU time and the same time in the 3dfx renderer's zones of the profile,
as the emulated CPU, not the graphics card, holds it back there. Drawing
nothing but dithered triangles, 200 screen-sized rectangles a frame at
2x, the graphics card took 7% longer with dithering on (1083 against
1010 ms a frame, about 0.3 ns more for each of the 246 million pixels):
the most it costs, where filling pixels is all a frame does.

## 3dfx Voodoo3 and Banshee

`machine=svga_voodoo3` makes the display adapter a 3dfx Voodoo3 3000
with 16 MB, and `machine=svga_banshee` a Voodoo Banshee:

```ini
[emulator]
machine=svga_voodoo3
cpu=pentium_iii
memsize=128
```

The Voodoo Graphics and the Voodoo2 are add-on cards next to the display
adapter, so Windows' own OpenGL and its desktop never use them. The
Voodoo3 and the Banshee are the display adapter themselves: a VGA, a 2D
engine, the Voodoo2's 3D engine and a video processor around one memory.
3dfx's driver for them runs the desktop, Direct3D on the primary display
and an OpenGL ICD (`3dfxOGL.dll`) that Windows' `opengl32.dll` hands GL
programs to. The card has no room for `voodoo=true` next to it.

On the PCI bus it is device 1 (vendor 121Ah, device 0005h, subsystem
003Ah, the Voodoo3 3000 PCI; the Banshee is device 0003h): its registers
at D0000000h, its memory at D2000000h and its I/O registers at D000h,
where the BIOS leaves them. The card's VGA is rust-dos's, and so is the
BIOS: DOS programs get the Super VGA's VESA modes, in their own memory,
and the card's extended modes start when its driver turns them on. The
2D engine does what the driver asks of it: fills, copies with all 256
raster operations and colour keys, stretched copies, images from the
CPU (one bit to 32 bits a pixel, YUV), lines, polylines and polygons,
on linear or tiled surfaces. The video processor shows the desktop in 8,
16, 24 or 32 bits through the colour lookup table, with the overlay and
the hardware cursor.

**Windows 98 SE** needs 3dfx's Voodoo3 driver (version 1.07.00 was
tried), which the Windows 98 CD doesn't have. Unpack its `driver9x`
folder where Windows can read it. At the next start Windows finds a
"Standard PCI Graphics Adapter (VGA)": choose to search for the best
driver, tick "Specify a location", give the folder that holds
`Voodoo3.inf`, and restart Windows when it is done. Windows then finds a
"Plug and Play Monitor": the card answers on its DDC lines with an EDID
for a CRT that takes up to 1600x1200 (DxDiag calls it "rust-dos CRT"),
and the VESA BIOS hands the same EDID to programs that ask through
4F15h. The card's interrupt, the vertical retrace one, is IRQ 11. The
desktop was tried at 640x480 and 1024x768 in High Color.

The 3D engine draws into the buffers the driver points it at, linear or
tiled, and a swap shows the buffer through the video processor. These
were tried on Windows 98 SE with the 1.07.00 driver:

* OpenGL through the ICD: the "3D Flying Objects" screen saver, and
  Oni (2001), which runs its menus and its first level. A game set up
  for a Voodoo2 may have 3dfx's MiniGL as `opengl32.dll` in its own
  folder; take that away, so the game gets Windows' `opengl32.dll` and
  the ICD.
* DxDiag's DirectDraw and Direct3D tests, windowed and full screen,
  with software and hardware rendering, from a 640x480 and a 1024x768
  desktop.
* Direct3D: Incoming (the GOG release, run without GOG's wrapper DLLs),
  its menus and the first level of its arcade game.

Direct3D needs DirectX 7, as the driver's readme says; Windows 98 SE
comes with 6.1. With 6.1 the driver takes the structure DirectDraw gives
it when a program starts Direct3D for the one DirectX 7 has, and faults:
the program's window stays white and the card draws nothing. Install
DirectX 7.0 (`dx70eng.exe`) or later first.

With `voodoo_renderer=opengl` the window's OpenGL draws the card's 3D
again at `voodoo_scale` times its resolution, as for the Voodoo Graphics
(see [3dfx Voodoo Graphics](#3dfx-voodoo-graphics)), and puts the picture
together as the card's video processor does: the desktop, the overlay
(the window or screen a game shows its 3D through, scaled and in YUV
too) and the cursor. A desktop or overlay that is a 16-bit buffer the 3D
draws into, laid out as the 3D draws it, comes from the bigger picture;
anything else (the Windows desktop, 8, 24 and 32-bit modes, YUV video)
comes from the card's memory at its own resolution, with the drawing
still queued for it done first. The software rasterizer keeps the
memory, so programs read back what they would on the card, and
screenshots and recordings show the software picture. The libretro core
draws a Banshee or Voodoo3 in software whatever `voodoo_renderer` says.

The card is a port of 86Box's Banshee and Voodoo3 emulation
(`vid_voodoo_banshee.c` and its 2D engine, by Sarah Walker, GPLv2, and
the I2C code in `i2c_gpio.c` and `i2c_eeprom.c` for the DDC), checked
against 3dfx's Voodoo3 specification.

## Rendition Vérité

`machine=svga_verite` makes the display adapter a Rendition Vérité V1000
with 4 MB, for the DOS games that came in Vérité versions, and for the
games Creative wrote for its 3D Blaster PCI, which is a Vérité:

```ini
[emulator]
machine=svga_verite
cpu=pentium
memsize=32
```

The Vérité is a VGA with a RISC processor that runs microcode the game
loads into its memory. The microcode takes drawing commands through the
FIFO at the card's ports, or by DMA from lists in the PC's memory, and
draws them with the card's pixel engine. Rust-DOS runs the RISC
instruction by instruction, with its pixel engine, as Rendition's V2200
Technical Specification describes the V1000's instruction set, so any
microcode a program brings runs: Rendition's Speedy3D, the Windows
drivers' 2D microcode, and the microcode of Creative's CGL modules.

| Setting | Values | What it does |
|---|---|---|
| `verite_risc` | `lle` (default), `hle` | `lle` runs the RISC; `hle` carries out Speedy3D's and the Windows 2D microcode's commands without it |
| `verite_mhz` | 1-1000 (25) | Million instructions a second the RISC runs: 25 as on the V1000 |
| `verite_rom` | a file | A dump of a Vérité board's 128 KB ROM for the RISC; without one, Rust-DOS's own microcode loader |
| `verite_irq` | 5, 7, 9, 10 (default), 11, 12, 15 | The card's interrupt line |

They take effect at the DOS prompt. `hle` is the older way: faster, but
only for Tomb Raider's and vQuake's kind of Speedy3D microcode and the
Windows desktop. With `lle` the RISC is as fast as the card's, so games
run at the frame rates they had on it.

Games built on Rendition's DOS library (RRedline, "Speedy3D") find the
card through the BIOS (INT 10h AX=158Dh describes the board), load
`spd3d.uc`, and start the processor through the BIOS. Rust-DOS's BIOS
has these functions; its VESA modes are the card's own, shown by its
CRTC, with the memory's byte order set for the PC to see it linear.

* **Tomb Raider**'s Vérité patch (`3DPATCH\RENDVRT` on the Tomb Raider
  Gold CD: `Tomb.exe` and `spd3d.uc`, copied into the game's folder, the
  executable renamed so the 2D one stays) runs with `cpu=pentium` and
  `memsize=32`.

* **vQuake** (Rendition's port of Quake; 1.07 beta 5 with its `spd3d.uc`,
  or 1.08 beta 1, copied into the Quake folder) runs with `dpmi=false`,
  `cpu=pentium` and `memsize=32`, and `-nocdaudio` unless the CD is
  mounted. It brings its own DPMI host, CWSDPMI, which it needs: its
  copy of Rendition's library finds the physical addresses for DMA by
  walking the processor's page tables, which the built-in host doesn't
  use.

### 3D Blaster PCI games (CGL)

Games for Creative's 3D Blaster PCI draw through Creative's Graphics
Library (CGL): a module beside the game, `VRITB170.CGL`, `VRITC170.CGL`,
`VRITE170.CGL` or `VRITI170.CGL`, runs on the PC and drives the card. It
starts the card on a microcode loader it copies from the board's ROM,
and sends that loader its own microcode. A dump of a real board's ROM
(`verite_rom`) can provide the loader; without one, the ROM Rust-DOS
gives the card has a loader of its own, which takes the microcode as the
board's would. Copy all four modules into the game's folder: each game
loads the one it was built for.

These ran on the module's own microcode, from the 3D Blaster PCI's bonus
CD and patches:

* **Battle Arena Toshinden** Special Edition (`TSD` on the bonus CD;
  install with its `SETUP`, choose 3D Blaster under Misc Options): menus
  and fights.
* **Rebel Moon** (`REBMOON` on the bonus CD; `REB3D` or `REBMOON
  -3DBLASTER`): menus, mission briefings and the game.
* **Flight Unlimited** (installed from `INST\INITIAL.EXE` on the bonus
  CD): the lobby in 3D and the menus.
* **Hi-Octane** 1.2 with its CGL executable: menus and races. Its menus
  read the keyboard once a frame: hold the keys a little.

Other games for the card (Actua Soccer, Screamer 2 and Rally, Euro 96,
Battle Race) haven't been tried.

### Windows 95 on the Vérité

Windows 95 ran on the Vérité with `verite_risc=hle` and Rendition's
V1000 reference driver (2.1 beta 2, `v1k_b2_1.zip`), installed with
**Have Disk** under Display Properties, Settings, Advanced Properties,
Change. The driver tests the card's processor and starts its 2D
microcode (`v10002d.uc`) through the card's debug registers, sets the
modes through the card's own CRTC, uses the Bt485 RAMDAC's palette and
hardware cursor, and draws through the microcode: fills, copies on and
between surfaces, one-bit images, text, brushes and clipping. The
desktop, windows, MS-DOS Prompts (windowed and full screen) and Paint
were tried at 800x600 in High Color and at 640x480 in 256 colours and in
True Color. With `verite_risc=lle` the 2D microcode itself runs; Windows
hasn't been tried that way yet, nor Direct3D and OpenGL, which load
Rendition's 3D microcode the same way.

The driver refuses True Color at 1024x768 at the refresh rate it picks
(85 Hz), as it would on the card: a Windows that ran in that mode on
another card shows a black screen after the driver is installed. Set
another mode before restarting, or in Safe Mode. Windows 95's own
Rendition driver, from the OSR2 CD, hasn't been tried.

## ATI 3D RAGE

`machine=svga_ati_3drage` makes the display adapter an ATI 3D RAGE
(mach64 GT) with 4 MB on the PCI bus; `svga_ati_3drage2` makes it a 3D
RAGE II (mach64 GT-B):

```ini
[emulator]
machine=svga_ati_3drage
cpu=pentium_ii
memsize=128
```

The card is a VGA with ATI's accelerator CRTC, hardware cursor, video
overlay, 2D draw engine and 3D engine. Its 16 MB aperture (BAR0) holds
the VESA linear frame buffer, and the registers in its top 2 KB; the
registers below 40h are also ports (256 of them at BAR1, or the
"sparse" ones from 2ECh with PCI configuration register 40h). Its BIOS
has ATI's signature and tables in the ROM and ATI's extended services
(INT 10h AH=A0h, and the far entry at C000:0064h that ATI's Windows
driver calls), whose mode tables list 640x480 to 1280x1024 and the low
resolutions 320x200, 320x240, 400x300 and 512x384. VESA modes are set
through the accelerator CRTC, as ATI's BIOS sets them.

The 2D engine draws fills, copies, one-bit images from the host or the
memory, patterns, lines, polygons and 24-bit pixels as ATI's drivers
use them. The 3D engine draws the trapezoids ATI's Direct3D HAL and
ATI3DCIF send: Gouraud shading, textures in all of the chip's formats
with mipmaps, bilinear filtering and perspective, the Z buffer, alpha
blending, fog and the texel colour key; the scaler stretches images
into rectangles. Everything is drawn at once, so the engine never
keeps the program waiting.

Windows 98 SE finds the card and installs its own driver for it (ATI
Rage/Rage II PCI, `ati_m64`, from the Windows CD): the desktop at up to
1024x768 in High Color, MS-DOS Prompts in a window and full screen, and
DxDiag's DirectDraw and Direct3D 7 tests pass on both chips. ATI's own
driver 4.35 (the mach64 driver CD's `WIN95` folder, with ATI3DCIF) runs
too, but its `MACXW4.INF` leaves the 3D RAGE (device 4754h) out of its
models list: add the line

```ini
%PCI\VEN_1002&DEV_4754_ENU.DeviceDesc%=MACXW4_PCI_English, PCI\VEN_1002&DEV_4754
```

under `[Mfg]` in a copy of the folder, and install it with **Have Disk**.
**Wipeout for 3D Rage** (the game ATI bundled with the card) runs with
it through ATI3DCIF. Windows sees the 3D RAGE II as new hardware after
a switch from the 3D RAGE, as it would after changing the card.

`RUST_DOS_MACH64_TRACE=<dir>` writes every register access and BIOS
call to `<dir>/trace.txt`.

## PowerVR PCX1 and PCX2

`powervr=pcx2` in `[emulator]` puts a PowerVR PCX2 in the machine, NEC's
3D chip on the Matrox m3D and VideoLogic Apocalypse 3Dx boards, for the
DOS games that came in PowerVR versions. `powervr=pcx1` puts in the chip
before it instead, the PCX1 of the VideoLogic Apocalypse 3D and NEC's
PC3D Engine:

```ini
[emulator]
powervr=pcx2
cpu=pentium
memsize=32
```

Unlike a 3dfx card, the PCX2 has no picture of its own. Games give it the
scene as planes and lists of the objects in each 32x32 tile, in the PC's
own memory, and their textures in its 4 MB; it works out which surface
each pixel shows (with hidden surfaces removed before anything is
textured, and translucent surfaces sorted by the card), textures and
shades the tile, and writes the finished pixels over the PCI bus into the
VGA card's VESA linear frame buffer. So the picture comes through the
display adapter: any `machine` with a VESA BIOS will do, screenshots and
recordings show it, and nothing else in the window changes.

The card is on the PCI bus as device 2 (vendor 1033h, device 0046h for
the PCX2, 002Ah for the PCX1), its registers at D4000000h (BAR0) and its
texture memory at D4400000h (BAR1), INTA# on IRQ 11, which it shares with
a Banshee or Voodoo3 as PCI cards do: the line stays up while either card
holds it. A render takes the time the chip's ISP and TSP would take (at
66 MHz on the PCX2, 60 on the PCX1), then sets its
end-of-render status (and interrupts, for games that unmask it); the
tiles are drawn on as many threads as the host has, and the picture is
the same however many there are. Save states and rewind keep the card
with its texture memory.

* **Tomb Raider**'s PowerVR patch (`3DPATCH\PWRVR\TOMBPCX2.EXE` on the Tomb
  Raider Gold CD, copied into the game's folder) runs with `cpu=pentium`
  and `memsize=32` (it wants 20 MB free), under its DOS/4GW. It finds a
  VESA 2.0 BIOS, so it doesn't run UniVBE. F1 and F2 change the
  resolution, F3 shows it and the frame rate. F8 switches between double
  and single buffering, going back to 640x480 as it does; single
  buffering can be 4 or 5 frames a second faster, which counts while the
  emulated card runs at 15 to 20 on a typical host. Keys this version has and the others
  don't (or use differently): F4 shows or hides the PowerVR logo (in the
  full game only), Z zooms the camera in and X out.
  Its saved games and the 3dfx version's (and the other versions') share
  the names `SAVEGAME.x` but don't work in each other: one way round the
  game loads to a black screen, the other way it quits with an error
  about GetHeight. The textures aren't harmed. [Issue #38](https://github.com/dividebysandwich/rust-dos/issues/38)
  has TOMBPCX2.EXE (v2.1e and v2.0e) modified to save as `SAVEPCX2.x`
  instead, so it lives peacefully beside the others in one folder, and
  the same for Tomb Raider: Unfinished Business's PowerVR version, which
  saves as `SVUBPX.x` instead of `SAVEUB.x`.
* The same patch's `TOMBPCX1.EXE` is the version for the PCX1, and runs
  the same way with `powervr=pcx1`. Each version tells you if the card is
  the other one.

The PCX1 does without some of what the PCX2 added. It has no bilinear
filtering: where a texture's two mip map levels meet, it blends a texel
of each instead, so textures look blockier than on a PCX2
(`powervr_filter=bilinear` smooths them anyway). Games also hand it their
scenes in fixed point rather than in floating point; the picture is the
same.

How the card draws follows Imagination's own driver sources and simulator
of the chip, which they released under the MIT licence
(github.com/powervr-graphics/PowerVR-Series1).

## Networking at a glance

Rust-DOS has two separate pieces of networking, and most people only ever
need the first one:

| You want to | You need | Setup |
|---|---|---|
| Play a DOS game in multiplayer (Doom, Duke Nukem 3D, Warcraft 2, Descent, C&C) | the **IPX driver** and a **room** | join a room, then pick the game's IPX option |
| Play a serial or modem game with one other player | a **room** | join a room, then pick the game's null modem or modem option |
| Browse the web, telnet, FTP or IRC from DOS, Windows 3.x or Windows 9x | the **NE2000 network card** | `ne2000=true`, plus a driver in the guest |
| Play a Windows 9x network game, or share files between two Windows guests | the **NE2000 network card** and a **room** | both of the above |

### The words

* **IPX** is the network protocol most DOS games used for multiplayer on
  1990s office networks. Games don't care whether there is a real network
  behind it, only that an IPX driver answers. The built-in DOS has one,
  so you don't load `IPX.COM`, `IPXODI` or anything else.
* A **room** is a virtual network cable between rust-dos instances.
  Everyone in a room is on the same network, as PCs plugged into one hub
  would be. Instances in different rooms don't see each other. A room has
  a name (`lobby` unless you choose one) and maybe a password.
* A **relay** is the program that holds the rooms and passes traffic
  between their members. Any rust-dos can be one (`LAN HOST`, or *Make
  room* in the settings window), and so can `rust-dos-relay` on a server.
  The public relay at `relay.rust-dos.com` is one that anyone can use.
  If you have used DOSBox, the relay does what an `IPXNET` server does,
  but the protocol is different: DOSBox and rust-dos can't play together.
* The **NE2000** is an emulated network card, a real piece of hardware
  that guest systems need a driver for. A system you boot from a disk
  image (Windows 95, Windows 98, a DOS with its own network setup) only
  sees a network through this card, never through the built-in IPX
  driver.
* The **router** is a small home router built into each rust-dos. It
  sits between the NE2000 and your computer's own internet connection.

### What the relay does and doesn't do

The relay only passes network packets (Ethernet frames) between the
members of a room, the way a network switch does. It doesn't hand out IP
addresses, it isn't a name server, and it doesn't connect anybody to the
internet. Your internet traffic never goes through it.

All of that is the job of the router in your own rust-dos:

```text
  guest system (Windows 95, DOS with mTCP, ...)
        |
    NE2000 card ─────┬────────────── room on a relay ─── other players' instances
        |            |               (LAN traffic only)
  IPX driver ────────┘
  (built-in DOS)
        |
  router in this rust-dos:  DHCP  10.0.2.15 for the guest
        |                   DNS   10.0.2.3, asks your computer's resolver
        |                   NAT   the guest's connections become your computer's
        v
  your computer's internet connection
```

* **DHCP:** the router gives the guest its address: 10.0.2.15, with the
  router at 10.0.2.2 and the name server at 10.0.2.3. Leave the guest's
  network settings on "obtain an address automatically" (DHCP or BOOTP).
  In a room, each instance's router hands out a different address
  (10.0.2.20 plus the member number), so guests in one room don't clash.
* **DNS:** the router's name server looks names up with your computer's
  own resolver, so whatever names your computer finds, the guest finds.
* **NAT:** the guest's TCP connections and UDP packets are made by
  rust-dos as ordinary connections of your computer. No administrator
  rights, network bridge or TAP device are needed. 10.0.2.2 stands for
  your computer itself: a web server on your computer's port 8080 is
  `http://10.0.2.2:8080/` in the guest.

What the router doesn't do:

* **No incoming connections from the internet:** nothing outside can
  connect to a server in the guest, and there are no port forwards.
  (Players in the same room can connect to each other's guests, since
  they are on the same LAN.)
* **No ping to the internet:** only the router's own addresses (10.0.2.2
  and 10.0.2.3) answer pings. A ping to a web site getting no reply
  doesn't mean the internet is down; try a browser or `HTGET` instead.
* **IPv4 only**, and name lookups for IPv4 addresses only, which is all
  programs of the time ask for.
* **Modern HTTPS doesn't work** in browsers of the time, which is a
  limit of the browsers, not of rust-dos. See [Network card](#network-card).

### Ports and firewalls

Rooms use UDP port 21213. To find rooms on your home network, rust-dos
broadcasts to that port, so a firewall on the computer that hosts the room
has to let UDP 21213 in. Joining the public relay only sends from your computer, so
it works behind a home router without any setup. Hosting a room for
players elsewhere on the internet (`LAN HOST`, then they `LAN JOIN` your
address) needs UDP 21213 forwarded to your computer in your router; the
public relay or `rust-dos-relay` on a server avoids that.

### Which systems can use what

| System | IPX games | Internet (TCP/IP) | LAN with other instances |
|---|---|---|---|
| Built-in DOS | built-in IPX driver | NE2000 + packet driver + mTCP or similar | IPX; TCP/IP over the NE2000 |
| Windows 3.1 started from the built-in DOS | DOS games in a DOS box: no IPX (see below) | NE2000 + packet driver + WINPKT + Trumpet Winsock | TCP/IP over the NE2000 |
| Windows for Workgroups 3.11 with its own network setup | its own IPX over the NE2000 | its own NE2000 driver and TCP/IP-32 | its own networking over the NE2000 |
| Windows 95/98 booted from an image | its IPX/SPX protocol over the NE2000 | its NE2000 driver and TCP/IP | its networking over the NE2000 |

The built-in IPX driver belongs to the built-in DOS: inside Windows, and
in systems booted from a disk image, games need that system's own IPX
over the NE2000. The steps for each system are under [Network
card](#network-card).

Tested so far: IPX games between instances (Doom, Heretic, Descent);
the internet from DOS with mTCP, from Windows 3.11 with Trumpet Winsock
and Netscape, and from Windows 95 with Internet Explorer. Not tested yet:
Windows 95's IPX against the built-in IPX driver, two Windows guests on
one LAN, and Windows for Workgroups' own TCP/IP-32. Reports are welcome.

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
* **The phonebook:** a number in it calls a bulletin board on the
  internet instead of the other player, by telnet: `ATDT 555-0101` dials
  Level 29. Rust-DOS knows a few boards that are still running; the
  settings window's *Modem phonebook* (Serial page) lists them, fetches
  more from rust-dos.com when you ask it to, and copies the command that
  dials one. Numbers of your own go in [`[phonebook]`](#phonebook).
* **A call sounds and takes as long as a real one:** the modem's speaker
  plays the dial tone, the number as touch tones (`ATDT`) or pulse clicks
  (`ATDP`), the other end ringing, and a real modem's handshake, about 25
  seconds in all; CONNECT comes when the handshake ends. A host on the
  internet dials a seven-digit number of its own. `ATM0` turns the speaker
  off, `ATM3` keeps it quiet while dialing, and `ATL0`..`ATL3` set its
  volume; the call takes as long either way. `modem` in
  [`[mixer]`](#mixer) is its volume in the mix. A key during the dialing
  or the handshake ends the call.
* The modem knows the usual commands: `ATZ`, `AT&F`, `ATE`, `ATV`, `ATQ`,
  `ATH`, `ATA`, `ATO`, `ATD`, `ATP`, `ATT`, `ATL`, `ATM`, `ATI`,
  `ATS0=`..`ATS15=` and `ATSn?`,
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
in fullscreen on a large screen. At exactly 2x or 3x with `aspect=none`
the scanlines are sharpest.

The shaders need OpenGL 3 (WebGL 2 in the browser). Without it, as with
`SDL_VIDEODRIVER=dummy`, rust-dos draws the picture with SDL's renderer as
before and says why at startup; the log names what draws the picture.
Screenshots and video recordings show the shader with `record_shader=true`
(see `capture_dir` in [`[emulator]`](#emulator)); animations and
debug-server screenshots always show the plain picture.

## 3D scene and VR

With `[vr] mode=desktop` (or `--vr-desktop`), the window shows the picture
on a screen in a 3D scene instead of filling it. With `mode=headset` (or
`--vr`), a VR headset shows the scene through OpenXR (SteamVR, Monado), and
the window shows the left eye's view. With no `scene`, the scene is a room
with the screen floating ahead and a small PC tower beside it, a dark floor
and a dark blue sky with an orange sunset.

The PC's lights are the machine's: the power light, the turbo light while
the CPU runs faster than 1000 instructions a millisecond (an XT's or a slow
AT's speed is below it), the hard disk light while a hard disk's files or
sectors are read or written, and the floppy drive's light while a floppy's
are.

With `spatial_audio` on, the left and right channels come from the
screen's left and right sides: turn your head (or the window's camera) and
the sound turns with the room; come closer and it gets louder. From where
the scene starts it sounds exactly as without the scene. It adds no delay
to the sound.

The picture goes on the screen through the [CRT shader](#crt-shaders), but
flat: the screen has its own shape. It is drawn at twice its size (1024 to
2048 pixels across) with smaller copies for the distance, so that scanlines
and masks don't shimmer as the view moves. Its light falls on what is in
front of it as a CRT's does in a dark room: the parts of the picture light
the room in their colours, a bright picture lights the keyboard and the
desk, a dark one leaves them dark, and things in front of the screen cast
soft shadows. 3dfx pictures drawn with OpenGL go on it too.

The scene's lights and the screen cast shadows, corners and gaps get less
of the light from all around (ambient occlusion), and the light bounces
around the room once or twice, the way a sunlit patch on a wall lights
the rest of it. The bounced light is worked out when the scene loads, the
first time in a second or so, and kept in `vr-cache` in the configuration
directory for the next start. The `quality` setting chooses how much of
this is done.

The keyboard and the mouse work as without the scene. The mouse points at
the screen through the camera: where the pointer is over the screen is
where the program's cursor goes, and off the screen it stays where it was.
A captured mouse (Ctrl+F10, Ctrl+Alt, a click) moves it as always.

While **Ctrl+Shift** is held, the mouse flies the window's camera instead,
and the machine sees neither it nor these keys:

| Input (with Ctrl+Shift held) | What it does |
|---|---|
| Moving the mouse | Look around |
| Dragging with the left button | Slide sideways and up and down |
| Dragging with the right button, or the wheel | Move ahead and back |
| Q / E (held) | Move down / up |
| Home | Back to where the scene starts; with a headset, center its view where you look |

### Scenes from Blender

[docs/vr.md](docs/vr.md) is the full guide to building rooms: the screen,
the speakers, the PC's lights, materials, exporting and checking a room.

A scene is a glTF 2.0 file. In Blender, File > Export > glTF 2.0:

* Format **glTF Binary (.glb)**, or glTF Separate with its files beside it.
* Include: tick **Custom Properties** if the screen is marked with one.
  Transform: **+Y Up** (the default). Mesh: **Apply Modifiers**.
* Lighting mode **Unitless** keeps lights about as bright as Blender's
  numbers; a scene's custom property `rustdos_exposure` scales all lit
  colours (1 by default).

The **screen** is the mesh whose object (or mesh) is named `screen`, or that
has the custom property `rustdos_screen` set to true or 1. Its UV map is
where the picture goes: unwrap it to fill 0 to 1 with the picture upright,
as an image texture would show it. Any shape works, flat or curved. The
picture keeps its proportions and is letterboxed in the screen's shape.

Optionally:

* An empty named `spawn` is where the eyes start, looking along the
  empty's front (Blender's +Y with no rotation). Without it, the first
  camera is, else a point 1.2 m above the origin looking along Blender's
  +Y.
* Lights (point, spot and sun, at most 8) light the scene; without any it
  is lit by the sunset. Materials take their base colour and emissive
  colour and textures (PNG or JPEG), alpha blend and clip, and double
  sidedness. An unlit material (Blender's "Background" shader exported as
  KHR_materials_unlit) shows its colour as it is, for baked lighting.
* The scene's custom property `rustdos_sky` set to false leaves out the
  sunset sky and its haze, for a closed room.
* Meshes named `led_power`, `led_turbo`, `led_hdd` and `led_floppy` (or
  with the custom property `rustdos_led` set to `power`, `turbo`, `hdd` or
  `floppy`) are the PC's lights: they glow in their material's emissive
  colour (or else its base colour) while lit, and not while dark.
* Empties named `speaker_left` and `speaker_right` are where the sound's
  channels come from, instead of the screen's sides.

A scene that can't be read, or has no screen, gives the test room
instead, and the log says why.

### VR headsets

The headset is driven through OpenXR, which needs the OpenXR loader: on
Linux, your distribution's `openxr` package (`libopenxr_loader.so.1`,
`libopenxr-loader1` on Debian and Ubuntu, which the `.deb` recommends), or
else the `libopenxr_loader.so.1` beside `rust-dos`, which the `.tar.gz` has; on
Windows, `openxr_loader.dll` beside `rust-dos.exe`, which the `.msi` and the
`.zip` have (built yourself, take it from the Khronos OpenXR SDK's
releases). The active runtime (SteamVR, Monado, ...) draws
with the window's OpenGL context, in one of three ways (`[vr] graphics`):

* OpenGL through GLX (X11) on Linux and WGL on Windows, which SteamVR
  takes. With `--vr` and a runtime that takes it, rust-dos opens its window
  through X11, under XWayland on a Wayland desktop.
* OpenGL through EGL (`XR_MNDX_egl_enable`), which Monado takes, under
  Wayland too, or under X11 told to make EGL contexts.
* Vulkan (`XR_KHR_vulkan_enable2`), for a runtime that takes Vulkan only:
  each eye is still drawn with OpenGL, into an image Vulkan shares
  (`GL_EXT_memory_object_fd`), then copied into the runtime's. OpenGL and
  Vulkan must use the same graphics card and driver.

`auto` asks the runtime as rust-dos starts and tries what it offers in
that order; if one fails, the next is tried. Without a loader, runtime or
headset, the scene is shown in the window and the log says why.

What happened goes to `vr.log` in rust-dos's own directory (the run before
is kept as `vr.log.1`): what the runtime offers and which way was taken,
the eye size, the session's states, the controllers the runtime reports
(their interaction profiles), and every 10 seconds how the frames keep up
with the headset (late frames, time spent drawing). `--vr-probe` writes
what the runtime and OpenGL offer to `vr-probe.log` and the console, makes
a session each way it can, and quits: the first thing to run on a new
headset. Hands tracked without controllers point and pinch to click where
the runtime has `XR_EXT_hand_interaction` (not on Windows).

Where OpenGL can draw both eyes in one pass (`GL_OVR_multiview2`, as Mesa
has it, Zink on a Steam Frame included), the scene is drawn once for both,
straight into the headset's images; elsewhere one eye at a time. On a
standalone headset's own graphics chip (Adreno, Mali and the like), the
`auto` settings go easier on it: medium lighting, no ambient occlusion,
2x multisampling, 72 Hz, and as much of each eye's image drawn as there is
time for (see `[vr] resolution`). `vr.log` says which, and every 10
seconds how long the graphics chip takes for a frame, by pass. A standalone
headset runs rust-dos on its own desktop or as a Steam shortcut; started
over SSH without one, rust-dos refuses to take the headset's display.

The eyes start where the scene's `spawn` is: the view is centered on your
head the first time the headset shows the scene. Ctrl+Shift+Home, the
controllers' menu button held for a second, or **Center the view where you
sit now** on the settings window's VR page centers it again where you are
and look. The `scene_scale`, `seat_*` settings fine-tune it from there.

The headset is drawn on a thread of its own, at the headset's rate (90 Hz
and the like), with the newest picture the machine finished: a long
moment of the machine's (a program loading, the recompiler at work)
doesn't make the headset miss frames, and the view follows your head
whatever the machine does. The window shows the left eye's view at the
window's own rate. The machine's own frame rate, 70 Hz for VGA, judders a
little against the headset's.

The controllers (Index, Touch, Vive, Windows Mixed Reality and any that
SteamVR maps to them; SteamVR's controller binding settings change them)
with `controllers=both`:

| Control | What it does |
|---|---|
| Trigger, on the hand last pulled with | Points a laser; where it meets the screen is the mouse, the trigger its left button |
| Grip, on the pointing hand | The mouse's right button |
| Thumbsticks (Vive: trackpads) | The joystick's two sticks |
| A / B (right), X / Y (left) | The joystick's buttons A, B, X, Y |
| The other hand's trigger and grip | The right trigger is button A, the right grip B, the left trigger X, the left grip Y |
| Menu (Index: left thumbstick click) | Opens and closes the settings window, which the laser clicks in; held a second, centers the view |

The controllers are the first gamepad, so a game's [gamepad
mapping](#gamepad-mappings) maps them too: the grips are its shoulder
buttons and the triggers its triggers. With `controllers=pointer` they only
point and click; with `gamepad` both hands are the gamepad, triggers and
grips included. Keyboard and mouse input go to the window, which has to
keep the focus.

### The VFX1 headset

With [`[vfx1]`](#vfx1) `enabled=true`, the machine has a Forte VFX1: the
VIP card at `VIPPORT` and the VFX1.COM driver's interface on INT 33h,
which games find without a driver of their own. Its head tracker and
CyberPuck are your headset and right controller. The VFX1 itself lies on
the desk of the 3D scene, beside the joystick.

In the headset, take the VFX1 with the grip button and bring it to your
face, whichever way round: it goes on as it comes to your eyes (or as
you let go of it near them). The room goes away, each eye sees the
picture the game draws for it, ahead of it wherever you turn, on black,
as the VFX1's two screens showed it, and your head is the game's head
tracker, level and ahead where you were as you put it on. To take it
off, grip next to your head and pull it away, then put it down or throw
it; it goes on again only once it has been away from your head. Should
it come to rest out of reach, on the floor, it is back on the desk.
While you wear it, the controllers are a gamepad and point at nothing;
the right controller is the CyberPuck as well, its tilt the puck's and
its trigger, grip and A button the puck's three buttons.

Start a game with its VFX1 switches:

| Game | Command | Notes |
|---|---|---|
| Descent (1.4 and later) | `DESCENT -vfx -vfxtrak` | Set `steer=true`. Enter re-centres the head, Shift+F7 swaps the eyes, Shift+F6 changes the detail |
| Descent 2 | `VR_type=1`, `VR_tracking=1` in DESCENT.CFG (its SETUP) | Set `steer=true`; see below for more pixels |
| Comanche 3 | `C3`; in flight, Video > Stereoscopic, then Alt+F12 | Interlaced 320x400, which `auto` shows as lines |
| Quake 1.08 | `VFX1QK QUAKE.EXE +vid_mode 7 +lcd_x 1` | VFX1QK from forte-vfx1-vr; `stereo=lines` |
| System Shock (CD) | `CDSHOCK`, VFX1 in its options | Full-screen view |
| MechWarrior 2 | VFX-1 under Cockpit Controls | Head tracking |
| Magic Carpet | `-vfx` | Head pitch and roll only |

[docs/vfx1.md](docs/vfx1.md) has how many pixels each eye gets in these
games, and how to get more. Descent 2 sets up its headset in DESCENT.CFG,
which its SETUP writes: `VR_type=1` is the VFX1, and draws 320x200 for each
eye, with the head tracker (`VR_tracking=1`). Its shutter glasses' setting
draws far more: with `VR_type=6` (LCDBIOS) and `VR_resolution=4` it draws
800x300 for each eye in an 800x600 mode (`3` is 640x240 in 640x480), which
the VFX1 shows through `stereo_bios`, though without its head tracker, which
Descent 2 reads only in its VFX1 setting.

Descent, Descent 2 and Comanche 3 were tried with rust-dos; the other games
are as the [forte-vfx1-vr](https://github.com/LeonG-ZA/forte-vfx1-vr)
project found them. Descent draws its heads-up display in the left eye only,
as it did on a VFX1. Without a headset, `RUST_DOS_VFX1_HEAD=yaw,pitch,roll`
(in degrees) holds the head turned, and `RUST_DOS_VFX1_DUMP=<folder>` writes
the eyes' pictures there as `left.png` and `right.png` once a second, to try
a game out; `RUST_DOS_VR_WEAR=1` starts with the VFX1 on.

## Command-line options

Options on the command line override the configuration file's settings for
that run; the settings window saves only what you change in it. `rust-dos
--help` lists them.

| Option | What it does |
|---|---|
| `-d, --dir DIR` | Make the host directory `DIR` drive C: (default: the configuration's C:, or the current directory) |
| `-c, --config FILE` | Use this configuration file (see [Configuration file](#configuration-file)) |
| `--no-config` | Read and create no configuration file |
| `--log FILE` | Write the [log](README.md#log-file) to `FILE` instead of `rust-dos.log` in the configuration directory; stop if it cannot be created |
| `--no-boot` | Start at the DOS prompt, without booting the disk image marked `-boot` in `[drives]` |
| `-s, --scale N` | The window scale factor, 1 to 16 |
| `--vrr` | Show frames at the machine's refresh rate, for a VRR display (`vrr`) |
| `--hidden` | Keep the window hidden; the picture is still drawn with OpenGL, `voodoo_renderer=opengl` included, for runs driven through the debug server (on a host without OpenGL 3, SDL's renderer and the software rasterizer draw it, as with a visible window) |
| `--vr` | Show the picture in a 3D scene in a VR headset ([`[vr]`](#vr) `mode=headset`) |
| `--vr-desktop` | Show the picture in a 3D scene in the window (`mode=desktop`) |
| `--vr-scene FILE` | The 3D scene, a glTF file from Blender (`scene`); on its own it means `--vr-desktop` |
| `--vr-graphics API` | How the headset's pictures get to OpenXR: `auto`, `gl`, `egl` or `vulkan` (`graphics`) |
| `--vr-resolution PERCENT` | The headset's eye images in percent of the recommended size, 30 to 150 (`resolution`) |
| `--vr-probe` | Write what the OpenXR runtime and OpenGL offer a headset to `vr-probe.log` and quit (with `--vr`, start after) |
| `--cycles N\|max\|auto` | The CPU speed (`cycles`) |
| `--core auto\|dynamic\|normal` | What runs the instructions (`core`) |
| `--game NAME` | Launch a [game profile](#game-profiles) at startup, by its file name or its name |
| `--import PATH` | Import a game set up for DOSBox as a game profile, and launch it |
| `--debug-server [ADDR]` | Start the [debug server](README.md#debug--remote-control-server) (default `127.0.0.1:8086`; port 0 takes a free port, printed on stdout) |
| `--trace-capacity N` | Entries in the debug server's instruction trace (default 1,000,000) |
| `--deterministic` | Run on emulated time alone, so that the same input gives the same run every time: a fixed speed, a clock that starts at `--start-time`, and the debug server's input delivered at emulated times. With `--debug-server` the machine starts paused (see [docs/llm-debugging.md](docs/llm-debugging.md#deterministic-runs)) |
| `--start-time "YYYY-MM-DD HH:MM:SS"` | The date and time the clock starts at with `--deterministic` (default 1995-04-11 12:34:56) |
| `--relay [PORT]` | Relay [LAN](#playing-over-a-lan) rooms on this UDP port (default 21213) instead of starting the emulator |
| `--relay-password TEXT` | One password for all the rooms of `--relay` (without it, whoever makes a room gives it one or none) |

### Running DOS tools from the host's shell

`rust-dos run COMMAND [ARGS...]` runs a DOS program, batch file or built-in
command without a window, as a tool of the host: what it prints through DOS
and the video BIOS goes to standard output (what it writes to handle 2 to
standard error), standard input is typed at its keyboard, and its exit code
is rust-dos's. A command that isn't found exits with 127, and a program that
still waits for keys once standard input has ended is stopped with 255.

```sh
rust-dos run LHA a DOCS.LZH README.TXT
echo y | rust-dos run -d ~/dos INSTALL.BAT
```

It reads no configuration file, and the machine has the default hardware.

| Option | What it does |
|---|---|
| `-d, --dir DIR` | The folder that is drive C: (default: the current directory) |
| `--cycles N` | Instructions per emulated millisecond (default 100000); the run doesn't wait for real time |
| `--mem MB` | The machine's memory, 1 to 256 MB (default 16) |
| `--screen` | Print the text screen at the end when nothing was printed through DOS, for programs that write to the screen's memory |
| `--win32` | Run Win32 console programs with the built-in Windows, as `win32=true` does |

### Unpacking InstallShield installers

`rust-dos unpack ARCHIVE` lists or extracts a Windows game's installer
archives on the host, without running its setup program: an
InstallShield 5 or 6 cabinet set by its header (`data1.hdr`, or a
`data1.cab` that holds the header; the other `dataN.cab` volumes are
found beside it) or an InstallShield 3 archive (`_SETUP.1`, a `.Z`
file). Each file is checked as it is expanded, by MD5 in version 6 sets
and by size in the others. A file the set holds more than one of at one
path is left out unless `--group` names a group that places it.

```sh
rust-dos unpack --list /cdrom/SETUP/DATA1.HDR
rust-dos unpack --to ~/games/oni --group "Files (Data)" /cdrom/DATA1.HDR
rust-dos unpack --old-compression --to out /cdrom/DATA1.CAB
```

| Option | What it does |
|---|---|
| `-l, --list` | List the file groups, the files with their sizes, entries and groups, and the entries that can't be read, with why |
| `-t, --to DIR` | The folder to extract into (default: the current directory) |
| `-g, --group NAME` | Extract only the files of this file group; it may be given more than once |
| `-O, --old-compression` | Read compressed files as raw deflate ending each chunk with `00 00 FF FF`, as some version 0 sets have them (Unshield's `-O`) |

It exits with 1 when the archive doesn't open, a group isn't in it, or a
file fails its check. A game profile's
[`[install]`](#unpacking-the-installer) section unpacks the same
archives from the game's drives.
