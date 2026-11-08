# Game compatibility notes

## The almanac first

[almanac.rust-dos.com](https://almanac.rust-dos.com) collects players'
reports of the settings games run well with. Rust-DOS asks it in two
ways:

* In the settings window (**Ctrl+F12**), on the Emulator page, *Fetch
  settings from Almanac* lists the games it has for the program started
  last, or for a name you type, and Enter puts the settings of the one
  you pick in place. *Report settings to Almanac* tells it how a game
  played with the settings as they are, which helps the next player.
* With `almanac_auto=true` in
  [`[emulator]`](../CONFIGURATION.md#emulator), a program started from
  the prompt waits a moment while Rust-DOS asks, and starts with the
  settings the almanac recommends; yours come back when it ends. A game
  that has a [game profile](../CONFIGURATION.md#game-profiles) and is
  started from it runs with the profile's settings, and Rust-DOS doesn't
  ask the almanac for it.

Rust-DOS finds a game in the almanac by the name, size and checksums of
the program file started at its prompt; `almanac_auto` takes the
settings of a game only when the checksums match. That works for DOS
games and for games on the built-in Windows, which start from the same
prompt. The almanac carries the machine's settings only: the graphics
card, processor, speed, memory, DPMI, the DOS and Windows versions, the
3D cards, the sound cards, the joystick and the disks' speeds.

## What this page adds

What the almanac can't carry:

* files to copy or remove, drivers, command-line switches and install
  steps;
* settings outside its list, such as `win32_depth`;
* limits that are known;
* games on a Windows 95 or 98 disk image. There the game is started by
  the Windows inside the image, not at Rust-DOS's prompt, so Rust-DOS
  never sees which program runs and can't look it up.

Where an entry also names settings the almanac takes, they can be
fetched from it or reported to it like any other.

Each entry says what to do and links to the section that explains it.
Notes for a game on the built-in Windows don't apply to the same game
on a Windows image, and the other way round. A game that isn't listed
may run fine with the defaults or the almanac's settings; if one
doesn't, please
[open an issue](https://github.com/dividebysandwich/rust-dos/issues)
that names the game, its settings and what happened.

## DOS games

* **3D Blaster PCI games** (Battle Arena Toshinden SE, Rebel Moon,
  Flight Unlimited, Hi-Octane):
  [3D Blaster PCI games](../CONFIGURATION.md#3d-blaster-pci-games-cgl)
  has the modules to copy and each game's start command.
* **Bao Xiao San Guo Zhi** (`SANGO.EXE`): `pit2_gate_at_start=true` in
  its profile; [`[sound]`](../CONFIGURATION.md#sound) says why.
* **BloodNet** (GOG): GOG's `.bdn` file mounts as a CUE sheet
  ([CD images](../CONFIGURATION.md#cd-images)).
* **Descent**: keep `cycles=auto`, the default. A high fixed `cycles`
  runs it faster than the display's refresh rate, and it plays badly
  there.
* **Descent, Descent 2 and Comanche 3** on a VFX1 headset, and Quake
  1.08, which hasn't been tried: the table in
  [the VFX1 headset](../CONFIGURATION.md#the-vfx1-headset) has each
  one's start command and settings, and [docs/vfx1.md](vfx1.md) their
  resolutions.
* **Doom and Heretic** over a LAN or a serial line:
  [playing over a LAN](../CONFIGURATION.md#playing-over-a-lan) and
  [serial and modem games](../CONFIGURATION.md#serial-and-modem-games).
* **King's Quest, Ultima and other early CGA games**: their composite
  colours need `machine=cga`; `composite` and `composite_era` keep their
  defaults, `auto` and `old`
  ([`[emulator]`](../CONFIGURATION.md#emulator)).
* **Tomb Raider's 3D patches** (in `3DPATCH` on the Tomb Raider Gold
  CD), each copied into the game's folder:
  * 3dfx (`3DFX\TOMB.EXE`): `voodoo=true`, `cpu=pentium`
    ([3dfx Voodoo Graphics](../CONFIGURATION.md#3dfx-voodoo-graphics)).
  * Voodoo Rush (`VOORUSH\TOMB.EXE`): `voodoo=true` and 3dfx's
    `GLIDE2X.OVL`, which the settings window downloads onto Z:.
  * Vérité (`RENDVRT`): copy `Tomb.exe` and `spd3d.uc`, renaming the
    executable so the 2D one stays; `machine=svga_verite`,
    `cpu=pentium`, `memsize=32`
    ([Rendition Vérité](../CONFIGURATION.md#rendition-vérité)).
  * PowerVR (`PWRVR\TOMBPCX2.EXE`): `powervr=pcx2`, `cpu=pentium`,
    `memsize=32` (it wants 20 MB free); `TOMBPCX1.EXE` is the same with
    `powervr=pcx1`, and each tells you if the card is the other one
    ([PowerVR PCX1 and PCX2](../CONFIGURATION.md#powervr-pcx1-and-pcx2)).
    F1 and F2 change the resolution, F3 shows it and the frame rate, F4
    shows or hides the PowerVR logo (in the full game only), Z and X
    zoom the camera in and out. F8 switches to single buffering, which
    can be 4 or 5 frames a second faster, going back to 640x480 as it
    does.
  * The versions' saved games share the names `SAVEGAME.x` but don't
    load in each other: one way round the game loads to a black screen,
    the other way it quits with an error about GetHeight.
    [Issue #38](https://github.com/dividebysandwich/rust-dos/issues/38)
    has `TOMBPCX2.EXE` (v2.1e and v2.0e) changed to save as
    `SAVEPCX2.x`, and Unfinished Business's PowerVR version changed to
    save as `SVUBPX.x`, so they can share a folder with the others.
* **vQuake** (1.07 beta 5 with its `spd3d.uc`, or 1.08 beta 1, copied
  into the Quake folder): `machine=svga_verite`, `dpmi=false` (it needs
  the CWSDPMI it brings), `cpu=pentium`, `memsize=32`, and `-nocdaudio`
  unless the CD is mounted
  ([Rendition Vérité](../CONFIGURATION.md#rendition-vérité)).

## Windows games on the built-in Windows

[Windows games](../CONFIGURATION.md#windows-games) and
[when a game doesn't run](../CONFIGURATION.md#when-a-game-doesnt-run)
cover the settings most games need: `cpu=pentium_ii` for an invalid
opcode, `win32_version=98se` when a message names the version, more
`memsize` when it runs out. `win32_depth` is in
[`[emulator]`](../CONFIGURATION.md#emulator).

* **Enemy Infestation**: the profile in
  [a profile to start from](../CONFIGURATION.md#a-profile-to-start-from)
  installs it with its InstallShield 5 setup and runs it. For a CD
  copied to a folder, give the drive the label `EI` ([the CD's drive
  letter and label](../CONFIGURATION.md#the-cds-drive-letter-and-label)).
* **Grom**: its Wise setup runs (tested with the minimum installation).
  It is on two CDs; list both images on the drive line and press Ctrl+F4
  when the setup asks for the second ([running the
  setup](../CONFIGURATION.md#running-the-setup-on-first-start)).
* **Iron Helix**: `win32_depth=8` (256 colours).
* **Moto Racer**: `win32_depth=16`; it checks the desktop's depth before
  it sets its own mode. With `machine=svga_voodoo3` the GOG release's
  `game.EXE` draws the races on the emulated Voodoo3, correctly with
  either `voodoo_renderer`. *Play Link* races between instances in a [LAN
  room](../CONFIGURATION.md#directplay-games-on-the-built-in-windows)
  over the IPX connection, the only one the game takes; it lists the
  TCP/IP one but ignores it, on Windows too (tested with the demo).
* **Wages of War**: `win32_depth=8`; it says it must run with 256
  colours and stops on the default desktop, which has more. Its setup
  (InstallShield 3's 32-bit engine) runs, and its archives also unpack
  ([installer archives](#installer-archives)).
* **Xenocracy**: its hardware check looks for `GLIDE2X.DLL`, which is
  there only with a 3dfx card: `voodoo=true`, or
  `machine=svga_voodoo3` or `svga_banshee`
  ([the system folder](builtin-windows.md#the-system-folder)).
* **SafeDisc and SecuROM games**: use an image of the disc that keeps
  what the protection checks. An ISO fails SafeDisc's check, and an
  image without DPM data SecuROM 4.8's, as on Windows 98.
  [copy-protection.md](copy-protection.md) has the image each
  protection needs and the tools that make it.

### Installer archives

These games' InstallShield archives were unpacked by
[`rust-dos unpack`](../CONFIGURATION.md#unpacking-installshield-installers),
which reads them as a profile's
[`[install]` section](../CONFIGURATION.md#unpacking-the-installer) does,
with every file the same as Unshield's or idecomp's:

* **Oni** (Europe): three cabinet sets (`data1`, `_sys1`, `_user1`).
  Some paths hold one file per language; unpack those by file group.
* **Railroad Tycoon II** (USA): `_SETUP.1`.
* **Rent-A-Hero** (Europe): `data1.cab`, which holds the header, and
  `_sys1.cab`.
* **Wages of War**: `_SETUP.1` and five `.Z` archives.
* **WarGames** (USA) v1.01: `data1.cab` and `_sys1.cab`, with
  `compression=markers` (`--old-compression` for `rust-dos unpack`).
* **Magic: The Gathering - Battlegrounds**: its cabinets have a major
  version 7 header, which the reader refuses.

## Windows games on a Windows 95 or 98 image

[docs/win9x-howto.md](win9x-howto.md) says how to make the image and
which drivers to install, and [3dfx.md](3dfx.md) which 3dfx card a game
needs.

* **Anachronox**: mount disc 2 while you play. Its SafeDisc check reads
  disc 2, as its readme says, and with disc 1 it says it can't find the
  CD ([copy-protection.md](copy-protection.md) has the image it needs).
* **Expendable**: Windows 98 SE on a Voodoo Graphics card,
  `voodoo=true`, `machine=svga_s3virge`, `cpu=pentium_ii`,
  `memsize=128`, and `DisplayDriver [1]` in its `config.txt`
  ([3dfx Voodoo Graphics](../CONFIGURATION.md#3dfx-voodoo-graphics)).
* **Glover**: on a Voodoo3 (`machine=svga_voodoo3`), install the driver
  from 3dfx's Voodoo3 driver CD 1.2 (`3dfx32v3.dll` 4.11.01.1151), not
  the later 1.07.00. With 1.07.00 the picture flickers: about half the
  frames show only the background colour, and the LOADING text is
  missing. 4.11.01.1151 draws it correctly with DirectX 6.1 or 7. It
  was checked only with the driver files of the CD's driver folder
  (`3dfx32v3.dll`, `3dfx16v3.drv`, `3dfxv3.vxd` and the rest) copied over
  those of 1.07.00, installed first as
  [the Voodoo3 driver](win9x-howto.md#the-voodoo3-driver) says; the CD's
  own `Voodoo3.inf` hasn't been tried. On the built-in Windows Glover
  needs neither.
* **GLQuake with the shareware data**: shareware Quake reads no files
  from subfolders outside its pak, so GLQuake can't read what it caches
  in `id1\glquake` and rebuilds its palette table at every start. The
  progress window it shows takes the focus from the full-screen game,
  which stays minimised on the taskbar: Alt+Tab brings it back. A pak
  with the table doesn't help, since shareware Quake refuses "modified
  games". The registered data has neither problem.
* **Incoming** (GOG release): run it without GOG's wrapper DLLs. On a
  Voodoo3 with the 1.07.00 driver it needs DirectX 7
  ([3dfx Voodoo3 and
  Banshee](../CONFIGURATION.md#3dfx-voodoo3-and-banshee)).
* **Moto Racer** (GOG release, `game.EXE`): play it on [the built-in
  Windows](#windows-games-on-the-built-in-windows), which draws it
  correctly. On a Voodoo3 image, install the driver from 3dfx's Voodoo3
  driver CD 1.2 (`3dfx32v3.dll` 4.11.01.1151), copied over 1.07.00 as
  for Glover above. With 1.07.00 the bikes and riders are flat white and grey shapes without
  their textures, while the track and the scenery have theirs; 86Box
  with a Voodoo3 and the same driver draws them the same way. With
  4.11.01.1151, and on a Voodoo2 with 3dfx's Voodoo2 driver, the bikes
  and riders are textured.
* **Oni**: on a Voodoo3, take away a MiniGL `opengl32.dll` from its
  folder. On a Voodoo2, copy `3DFXVGL.DLL` into its folder as
  `OPENGL32.DLL` ([OpenGL games under
  Windows](../CONFIGURATION.md#opengl-games-under-windows)).
* **Quake III Arena** on a Voodoo2: `r_glDriver 3dfxvgl` ([OpenGL games
  under Windows](../CONFIGURATION.md#opengl-games-under-windows)).
* **SubCulture** (3dfx version): Windows 95, `voodoo=true`,
  `machine=svga_s3`, `memsize=64`, `cpu=pentium`. It installs from its
  CD in the image's CD-ROM drive and plays its CD music
  ([3dfx Voodoo Graphics](../CONFIGURATION.md#3dfx-voodoo-graphics)).
* **Wipeout for 3D Rage**: `machine=svga_ati_3drage` and ATI's driver
  4.35 with one line added to its INF
  ([ATI 3D RAGE](../CONFIGURATION.md#ati-3d-rage)).
