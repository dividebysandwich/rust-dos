<center><img width="467" height="173" alt="image" src="https://github.com/user-attachments/assets/eea40654-bd61-4251-b377-13c28ee5724f" /></center>
<br/>
<hr/>
<br/>
<center><img width="640" height="400" alt="image" src="https://rust-dos.com/assets/images/rust-dos-descent-tour.gif" /></center>

## Introduction

Rust-DOS is an emulator for PC games, from the early DOS days to the
Windows 95 and 98 years.

* **DOS games** run on its built-in DOS. No copy of MS-DOS is needed.
* **Windows 95 and 98 games** run one of two ways:
  * on its built-in Windows, with no Windows image needed
    ([Windows games](CONFIGURATION.md#windows-games)). This is
    experimental and under active development: some games already run
    well, and more are tested and fixed all the time.
  * in a real copy of Windows, booted from a disk image
    ([docs/win9x-howto.md](docs/win9x-howto.md)).

Rust-DOS as a whole is a work in progress, and some games and programs
may not run yet.

*Rust-DOS is looking for contributors!*

## Start here

Download Rust-DOS for Windows, macOS, Linux, the browser, or RetroArch
on a desktop or on Android, from the
[releases page](https://github.com/dividebysandwich/rust-dos/releases); its
notes say which file to take. Then pick what you want to do:

| I want to... | Go to |
|---|---|
| Try a DOS game I have in a folder | [First DOS game](#first-dos-game) below |
| Play on an Android phone or tablet, or in RetroArch | [Android and RetroArch](#android-and-retroarch) below |
| Play a DOS game from a zip, a GOG install or a DOSBox setup | [Tutorial: from a zip](TUTORIALS.md#a-dos-game-from-a-zip-or-7z-archive), [from GOG or DOSBox](TUTORIALS.md#a-game-set-up-for-dosbox-or-gog) |
| Play a DOS game from its CD or floppy images | [Tutorial: CD image](TUTORIALS.md#a-dos-game-on-a-cd-image), [floppy images](TUTORIALS.md#a-dos-game-on-floppy-images) |
| Play a DOS game's 3dfx version | [Tutorial: a 3dfx card](TUTORIALS.md#a-dos-game-with-a-3dfx-card) |
| Install and play a Windows 95 or 98 game from its CD | [Tutorial: installed from a CD image](TUTORIALS.md#a-windows-95-or-98-game-installed-from-its-cd-image) |
| Play a Windows game I already have installed (GOG, or copied from a PC) | [Tutorial: an installed Windows game](TUTORIALS.md#an-installed-windows-game-gog-or-a-copied-folder) |
| Play a demanding Windows 3D game made for a GeForce-class card | [Tutorial: the ToadForce](TUTORIALS.md#a-demanding-windows-3d-game-toadforce) |
| Run a game in a real Windows 98 | [Tutorial: a Windows 98 image](TUTORIALS.md#a-windows-game-on-a-windows-98-image-with-its-cd-image), then [docs/win9x-howto.md](docs/win9x-howto.md) |
| Find out what a particular game needs | [almanac.rust-dos.com](https://almanac.rust-dos.com), then [docs/compatibility.md](docs/compatibility.md) |
| Look up a setting, a command or a key | [Documentation](#documentation) below |
| Build Rust-DOS from source, for what main has that no release has yet | [docs/building.md](docs/building.md) |

### The Linux AppImage

On a Linux desktop the `.AppImage` is the simplest download: one file,
for glibc 2.35 or later (Ubuntu 22.04, Debian 12 and newer), that uses
the system's SDL3 or its own where the system has none. Take the
`x86_64` one for PCs and the `aarch64` one for ARM machines. Make it
executable, then use the file's path wherever a command in this README
starts with `rust-dos`. The options stay the same:

```sh
chmod +x rust-dos-*-x86_64.AppImage
./rust-dos-*-x86_64.AppImage -d ~/games/doom/game
```

It mounts itself with FUSE, and where there is none, as in most
containers, it stops with an error. Pass `--appimage-extract-and-run`
before the other options, or set `APPIMAGE_EXTRACT_AND_RUN=1`, and it
unpacks itself to a temporary directory and runs from there.

Its settings, game profiles, saves, log and downloaded ROMs are kept in
`~/.config/rust-dos/`, so replacing the AppImage with a newer one keeps
them. To keep all of that in one folder instead, use the `.tar.gz` as a
portable install, described below.

### Android and RetroArch

On Android, Rust-DOS runs as a libretro core in
[RetroArch](https://www.retroarch.com/). Take the
`-libretro-android-<abi>.zip` from the releases page (`arm64-v8a` on most
phones and tablets), copy `rust_dos_libretro_android.so` to the device and
install it in RetroArch with *Load Core → Install or Restore a Core*. The
same core runs in RetroArch on Windows, macOS and Linux. It starts games
from folders, zips, CD and floppy images and game profiles, and its
settings are RetroArch's core options. With the core option *3dfx
renderer* set to OpenGL and RetroArch's `gl` or `glcore` video driver
(`gl` is OpenGL ES 3 on Android), it draws the 3dfx card with OpenGL;
otherwise the software renderer draws it.
[libretro/README.md](libretro/README.md) has the details: which content it
takes, its options, the keyboard and gamepad, and save states.

### First DOS game

Start Rust-DOS with the game's folder as drive C:

```sh
rust-dos -d ~/games/doom/game
```

and type the game's program at the prompt, `DOOM` here. `DIR` lists the
files, and the program to start is usually the `.EXE` or `.BAT` named
after the game. Or start Rust-DOS on its own and drop the folder onto
its window: it becomes the first free drive from D:, which the message
names, so type `D:` before the program. A zip of the game or a GOG
install dropped there becomes a game and starts. Leave the speed at its
default, `auto`: it finds the speed the game needs.

To start the game with one command next time, with its saves kept apart
from its files, give it a [game
profile](TUTORIALS.md#a-dos-game-in-a-profile). `rust-dos --help` lists
the command-line options, and
[CONFIGURATION.md](CONFIGURATION.md#command-line-options) describes
them, with `rust-dos run` for DOS tools and `rust-dos unpack` for
InstallShield archives.

### The keys you need first

| Key | What it does |
|---|---|
| Ctrl+F12 | The settings window: every setting, the drives, your games and the save states |
| Alt+Enter | Fullscreen, and back |
| Ctrl+F10 | Capture the mouse for the game, and let it go |
| Ctrl+F1, Ctrl+F2 | Save the machine to a save state, and load it |
| Ctrl+F4 | The next disc or floppy, in a drive mounted from a list of images |
| Ctrl+F11, Ctrl+Shift+F11 | Slow the machine down, and speed it up |
| Ctrl+F5 | A screenshot |

[REFERENCE.md](REFERENCE.md#keyboard-shortcuts) lists the others, and
[hotkey profiles](REFERENCE.md#hotkey-profiles) that give them the keys
of PCSX2, DuckStation, RetroArch, DOSBox and other emulators.

### Your settings

Rust-DOS keeps its settings in `rust-dos.conf`, in `%APPDATA%\rust-dos\`
on Windows, `~/.config/rust-dos/` on Linux and
`~/Library/Application Support/rust-dos/` on macOS. A `rust-dos.conf`
beside the program makes a portable install, which keeps everything in
the program's folder instead ([where Rust-DOS
looks](CONFIGURATION.md#configuration-file)). The settings window
(Ctrl+F12) changes them while a game runs, and F2 there saves them to
the file. A game's own settings go in a game profile beside it, so they
don't change the others'. The [tutorials](TUTORIALS.md) have
whole configurations to copy.

### When a game doesn't run

1. Look the game up in the almanac: the settings window's Emulator
   page, *Fetch settings from Almanac*, puts other players' settings in
   place.
2. Read [docs/compatibility.md](docs/compatibility.md) for what the game
   needs beyond settings.
3. Read the end of the [log file](REFERENCE.md#log-file), `rust-dos.log`
   in the folder above (the program's, in a portable install). For a
   Windows game, [when a game doesn't
   run](CONFIGURATION.md#when-a-game-doesnt-run) goes through the usual
   causes.
4. [Open an issue](https://github.com/dividebysandwich/rust-dos/issues)
   with the game's name, your settings and the log.

Bugs of the games themselves are welcome too: a crash, a hang or a
glitch that happened on real PCs as well. If you have a savegame or a
[save state](REFERENCE.md#save-states) from just before one, attach it
to an issue (zipped) with the steps that make it happen. We aim to fix
as many of these bugs as we can, in any game Rust-DOS runs, as [game
fixes](CONFIGURATION.md#game-fixes).

## Documentation

**Guides**, to get something done:

* [TUTORIALS.md](TUTORIALS.md): whole configurations for common cases,
  from a DOS game in a folder to a Windows game on a Windows 98 image
* [docs/win9x-howto.md](docs/win9x-howto.md): making a Windows 95 or 98
  disk image, and running games in it
* [docs/3dfx.md](docs/3dfx.md): which 3dfx card a game needs, and how to
  set it up
* [docs/builtin-windows.md](docs/builtin-windows.md): using the built-in
  Windows, and how it differs from Windows 98
* [GAME-PACKAGES.md](GAME-PACKAGES.md): putting a game, its CD, settings
  and manuals in one zip
* [docs/compatibility.md](docs/compatibility.md): what particular games
  need
* [libretro/README.md](libretro/README.md): the libretro core, for
  RetroArch on Android and on the desktop
* [docs/copy-protection.md](docs/copy-protection.md): which image of a
  copy-protected disc a game needs

**Reference**, the complete description:

* [CONFIGURATION.md](CONFIGURATION.md): every setting, game profiles,
  drives and disk images, booting, the emulated cards, networking and the
  command-line options
* [REFERENCE.md](REFERENCE.md): the DOS prompt and its commands, the
  keyboard shortcuts, save states, the log file, the browser and libretro
  versions, and the debug server
* [docs/README.md](docs/README.md): every page in `docs/`, the
  developers' included

## Features

* **CPU:** 386, 486, Pentium, Pentium MMX, AMD K6-2 (with 3DNow!), Pentium Pro, Pentium II and Pentium III (with SSE) with FPU, protected mode, paging and
  virtual-8086 mode for DOS extenders such as DOS/4GW (Descent, Heretic),
  and a dynamic recompiler for x86-64 and ARM64 hosts (see
  [docs/dynrec.md](docs/dynrec.md))
* **DOS built in:** no DOS or BIOS images needed. XMS, EMS, a DPMI host
  for DOS extenders, upper memory (`LOADHIGH`), a mouse driver, keyboard
  layouts, and a [DOS prompt](REFERENCE.md#the-dos-prompt) with batch
  files and a `COMMAND.COM` programs can shell out to
* **Windows built in (experimental):** Windows 95 and 98 games without
  a Windows image (`win32=true`, or `windows=builtin` in a
  [game profile](CONFIGURATION.md#windows-games)). Rust-DOS has its own
  KERNEL32, USER32 and GDI32, DirectX 3 to 6 (with Direct3D and Glide
  on an emulated 3dfx card), and WinMM's wave audio, MIDI and CD audio.
  It runs InstallShield and Wise setups. It runs fewer games than
  booting Windows: a program that calls a function it doesn't have yet
  stops with a message naming it
  ([docs/builtin-windows.md](docs/builtin-windows.md))
* **Graphics:** text, CGA with composite artifact colours, EGA, VGA with
  Mode X, VESA VBE 2.0 up to 1024x768, a Tseng ET4000 with HiColor, an S3
  Trio64 with its 2D accelerator, an [S3
  ViRGE](CONFIGURATION.md#direct3d-on-an-s3-virge) whose 3D engine draws
  Direct3D in Windows 95, Hercules, the Tandy 1000's and IBM PCjr's
  16-colour modes, a [3dfx Voodoo
  Graphics](CONFIGURATION.md#3dfx-voodoo-graphics) or
  [Voodoo2](CONFIGURATION.md#3dfx-voodoo2) for Glide games in DOS and
  Windows 95 and Direct3D and OpenGL games in Windows 98, drawn by Rust-DOS
  or again by OpenGL at up to four times its resolution, a [3dfx Voodoo3 or
  Banshee](CONFIGURATION.md#3dfx-voodoo3-and-banshee) as the display adapter
  itself, which runs Windows' desktop, Direct3D and OpenGL with 3dfx's
  driver, an [ATI 3D RAGE or 3D RAGE II](CONFIGURATION.md#ati-3d-rage) whose
  2D and 3D engines run ATI's Windows drivers, Direct3D and ATI3DCIF games,
  and a [PowerVR PCX1 or PCX2](CONFIGURATION.md#powervr-pcx1-and-pcx2) and a
  [Rendition Vérité](CONFIGURATION.md#rendition-vérité), its RISC emulated,
  for DOS games' PowerVR and Vérité versions and the 3D Blaster PCI's games.
  [docs/3dfx.md](docs/3dfx.md) says which 3dfx card a game needs and how to
  set it up
* **Sound:** Sound Blaster 16, Pro 2 and 2.0 with OPL3 FM music, Gravis
  Ultrasound with a built-in patch set, General MIDI through a SoundFont,
  the Ultrasound patches, a Roland MT-32 (via munt), a Roland Sound Canvas
  SC-55 (a port of Nuked-SC55) or the host's MIDI ports, Covox and Disney Sound Source, the Tandy/PCjr sound chip, and CD
  audio
* **Drives:** host directories as floppy, hard disk and CD-ROM drives, CD
  images (CUE with BIN, WAV, MP3, OGG or FLAC tracks, ISO, CHD, BlindWrite,
  CloneCD, Alcohol 120%) and FAT12, FAT16 and FAT32 disk images (raw, VHD,
  and 86Box's 86F floppies), with DOSBox Staging's `MOUNT` (and `IMGMOUNT`),
  and emulated disk speeds and noises.
  [docs/copy-protection.md](docs/copy-protection.md) says which image of a
  copy-protected disc a game needs
* **[Booting disk images](CONFIGURATION.md#booting-a-disk-image)**
  (`BOOT`): MS-DOS, [Windows 95](CONFIGURATION.md#windows-95) and other systems of their own, with a Plug
  and Play BIOS, APM, a PCI bus and IDE hard disks and CD-ROM drive (for
  Windows' 32-bit disk access), from the prompt, the settings window or
  [at every start](CONFIGURATION.md#booting-at-startup); your folders
  [reach the booted system](CONFIGURATION.md#host-folders-in-a-booted-system)
  as hard disks whose changes come back, or as CDs
* **Display:** CRT shaders (scanlines, aperture grille, curved shadow mask)
  and monochrome monitors (white, amber, green)
* **Settings window** (Ctrl+F12) that changes most settings without a
  restart, the display adapter included, and saves them to the configuration
  file
* **Game profiles** with settings and start commands of their own, and
  import of GOG and DOSBox games; a game's files stay as installed, its
  saves and changes in a folder of their own, and a reset takes it back
* **Manuals and extras:** a game's PDF manual, code wheel and maps shown
  over the picture (Ctrl+Shift+M), for its copy protection, and game
  packages: one zip with the game, its CD, settings and manuals
  ([GAME-PACKAGES.md](GAME-PACKAGES.md))
* **[Save states](REFERENCE.md#save-states)** (nine slots per game) and
  rewind
* **Cheats:** find a game's values in memory, and change or freeze them
* **[RetroAchievements](CONFIGURATION.md#retroachievements):**
  achievements, leaderboards and rich presence, hardcore mode included
* **Capture:** screenshots, and sound, video and GIF recordings
* **Joysticks** from game controllers, or the mouse as one
* **[Networking](CONFIGURATION.md#playing-over-a-lan):** IPX games between
  rust-dos instances on a LAN or over the internet (`LAN HOST`, `LAN
  JOIN`, a room browser in the settings window for rooms on the LAN or on
  a public relay, and a relay for servers of your own), and an NE2000
  network card behind a router to the internet, for DOS packet drivers,
  Windows 3.x with Trumpet Winsock and Windows 95
* **[Serial ports](CONFIGURATION.md#serial-and-modem-games):** a serial
  mouse, and null modem and modem games between the first two players in
  a LAN room; the Hayes modem also dials hosts over TCP and takes calls
* **[Printer](CONFIGURATION.md#printer):** an Epson ESC/P 2 printer on
  LPT1 that prints to PDF documents, PNG pictures, the host's printer or
  a file of what the program sent
* Runs in a **[web browser](REFERENCE.md#running-in-a-browser)** as
  WebAssembly
* A web-based
  **[debugger](REFERENCE.md#debug--remote-control-server)** and HTTP API
* **VR support**: Model your childhood bedroom in Blender and play your favorite
  DOS games in an OpenXR VR headset or in 2D mode  ([more](CONFIGURATION.md#3d-scene-and-vr))

<img width="1280" height="720" alt="image" src="https://github.com/user-attachments/assets/e9330c8a-ba96-4568-a0d9-183aad3a38ef" />

## Why

I wanted to learn more about the nuances of DOS emulation. Also, there's only one other DOS emulator written in Rust, and that one hasn't seen any development in 5 years and was using lots of unsafe{} code blocks.

## Building from source

Build Rust-DOS yourself to get the fixes and features on main before
they are released. [docs/building.md](docs/building.md) says how to
build it, its browser version and its libretro core, and how to run the
tests.

## Acknowledgements

Rust-DOS is standing on the shoulders of giants. This would not have been possible 
without the incredible work of these open-source projects:

* **[Dosbox](https://www.dosbox.com/)** - The original that paved the way and laid the foundations. A lot of
knowledge was preserved in it that would otherwise be forgotten now.
* **[Dosbox Staging](https://github.com/dosbox-staging/)** - The modern
continuation where a lot of care and attention was given to staying authentic to
the era and provide a stable emulator used by GOG and others. Staging was and is a
major source of foundational knowledge and advanced concepts such as dynamic
recompilation.
* **[Dosbox-X](https://github.com/joncampbell123/dosbox-x)** - Many innovations
and new features were spearheaded here: More processors, Win9X, a built-in UI. This
project did a lot of the hard work Rust-DOS now benefits from.

## Contributing, LLM Usage, Licensing

Both local and hosted LLMs (usually advertised as "Generative AI") were used in 
the development of this software. Contributions written using LLMs are ok 
provided the following rules are observed:

* **Read and review** generated code. You should be able to answer questions 
about your contribution.
* **Document and comment** non-trivial parts of the code.
* **Test** your contribution using actual games and programs.
* Don't use LLMs for trivial things like changing a constant. This is slow, 
  wasteful and runs the risk of unnecessary modifications elsewhere.
* Use modern, sufficiently sized models with sufficient context size. Running 
  small or outdated models or limiting them to small contexts results in low 
  quality code and damage to existing functionality.
* Usage of locally-hosted LLMs is encouraged, but not required.
* Please keep commits as vendor-neutral as possible, i.e. put skills into the 
  ``docs`` directory and reference them.

Rust-DOS is licensed under the GNU General Public License, version 2 or later
(see [LICENSE](LICENSE)). Code it takes from other projects and the crates it
is built with keep their own licences, whose notices are in
[THIRD-PARTY-NOTICES](THIRD-PARTY-NOTICES); every release package carries it.
