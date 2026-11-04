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
the program file started at its prompt. That works for DOS games and
for games on the built-in Windows, which start from the same prompt.
The almanac carries the machine's settings only: the processor, speed,
memory, DPMI, the Windows version, the 3D cards and the sound cards.

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
  Flight Unlimited, Hi-Octane): `machine=svga_verite`, and copy the four
  `VRIT*170.CGL` modules into the game's folder
  ([3D Blaster PCI games](../CONFIGURATION.md#3d-blaster-pci-games-cgl)
  has each game's start command).
* **Bao Xiao San Guo Zhi** (`SANGO.EXE`): every random roll comes out
  the same unless timer 2's gate is high at start. Set
  `pit2_gate_at_start=true` in [`[sound]`](../CONFIGURATION.md#sound),
  in the game's profile.
* **BloodNet** (GOG): GOG's `.bdn` file mounts as a CUE sheet
  ([disk images](../CONFIGURATION.md#disk-images)).
* **Blues Brothers: Jukebox Adventure**: the game hangs about 75
  seconds into the title screen's music, on a real PC too. Rust-DOS
  fixes that only in Razor 1911's release (BLUES.EXE of 25096 bytes);
  with any other, press a key on the title screen before then
  ([game fixes](../CONFIGURATION.md#game-fixes)).
* **Comanche 3** on a VFX1: in flight, Video > Stereoscopic, then
  Alt+F12 for head tracking
  ([the VFX1 headset](../CONFIGURATION.md#the-vfx1-headset)).
* **Descent**: keep `cycles=auto`, the default. A high fixed `cycles`
  runs it faster than the display's refresh rate, and it plays badly
  there. On a VFX1
  headset, start it as `DESCENT -vfx -vfxtrak` with `steer=true` in
  [`[vfx1]`](../CONFIGURATION.md#vfx1); its heads-up display shows in
  the left eye only. [docs/vfx1.md](vfx1.md) has the resolutions.
* **Descent 2** on a VFX1: `VR_type=1` and `VR_tracking=1` in
  `DESCENT.CFG` (its SETUP writes them) and `steer=true`
  ([the VFX1 headset](../CONFIGURATION.md#the-vfx1-headset)).
  [docs/vfx1.md](vfx1.md) has a mode with more pixels that loses the
  head tracker.
* **Doom and Heretic** over a LAN or a serial line: both players join
  one room with `LAN HOST` or `LAN JOIN`, then start
  `IPXSETUP -nodes 2`, or `SERSETUP -com2`
  ([playing over a LAN](../CONFIGURATION.md#playing-over-a-lan),
  [serial and modem games](../CONFIGURATION.md#serial-and-modem-games)).
* **King's Quest, Ultima and other early CGA games**: their composite
  colours need `machine=cga`; `composite` and `composite_era` keep their
  defaults, `auto` and `old`
  ([`[emulator]`](../CONFIGURATION.md#emulator)).
* **Quake 1.08** on a VFX1:
  `VFX1QK QUAKE.EXE +vid_mode 7 +lcd_x 1` with `stereo=lines`, as the
  forte-vfx1-vr project found it; it hasn't been tried in rust-dos
  ([the VFX1 headset](../CONFIGURATION.md#the-vfx1-headset)).
* **Tomb Raider's 3D patches** (Gold CD, `3DPATCH`):
  * 3dfx: `voodoo=true`, `cpu=pentium`
    ([3dfx Voodoo Graphics](../CONFIGURATION.md#3dfx-voodoo-graphics)).
  * Voodoo Rush: `voodoo=true` and 3dfx's `GLIDE2X.OVL`, which the
    settings window downloads onto Z:.
  * Vérité: copy `Tomb.exe` and `spd3d.uc` into the game's folder,
    renaming the executable so the 2D one stays;
    `machine=svga_verite`, `cpu=pentium`, `memsize=32`
    ([Rendition Vérité](../CONFIGURATION.md#rendition-vérité)).
  * PowerVR: `powervr=pcx2`, `cpu=pentium`, `memsize=32`
    ([PowerVR PCX1 and PCX2](../CONFIGURATION.md#powervr-pcx1-and-pcx2)).
    Its saves don't load in the other versions, nor theirs in it.
* **vQuake**: `machine=svga_verite`, `dpmi=false` (it brings its own
  CWSDPMI), `cpu=pentium`, `memsize=32`, and `-nocdaudio` unless the CD
  is mounted ([Rendition Vérité](../CONFIGURATION.md#rendition-vérité)).

## Windows games on the built-in Windows

[Windows games](../CONFIGURATION.md#windows-games) and
[when a game doesn't run](../CONFIGURATION.md#when-a-game-doesnt-run)
cover the settings most games need: `cpu=pentium_ii` for an invalid
opcode, `win32_version=98se` when a message names the version, more
`memsize` when it runs out. `win32_depth` is in
[`[emulator]`](../CONFIGURATION.md#emulator).

* **Enemy Infestation**: the profile in
  [a profile to start from](../CONFIGURATION.md#a-profile-to-start-from)
  installs and runs it. For a CD copied to a folder, give the drive the
  label `EI`.
* **Grom**: two CDs; list both images on the drive line and press
  Ctrl+F4 when the setup asks for the second ([running the
  setup](../CONFIGURATION.md#running-the-setup-on-first-start)).
* **Iron Helix**: `win32_depth=8` (256 colours).
* **Moto Racer**: `win32_depth=16`; it checks the desktop's depth before
  it sets its own mode.
* **Wages of War**: `win32_depth=8`; it says it must run with 256
  colours and stops on the default desktop, which has more.
* **Xenocracy**: its hardware check looks for `GLIDE2X.DLL`, which is
  there only with a 3dfx card: `voodoo=true`, or
  `machine=svga_voodoo3` or `svga_banshee`
  ([docs/native-windows.md](native-windows.md)).
* **SafeDisc 2 games**: use a CD image that keeps the error-correction
  data (a BIN or CHD dumped Redump-style). An image whose unreadable
  sectors read back as good fails the check, as on Windows 98
  ([disk images](../CONFIGURATION.md#disk-images)).

## Windows games on a Windows 95 or 98 image

[docs/win9x-howto.md](win9x-howto.md) says how to make the image and
which drivers to install.

* **Expendable**: Windows 98 SE on a Voodoo Graphics card,
  `voodoo=true`, `machine=svga_s3virge`, `cpu=pentium_ii`,
  `memsize=128`, and `DisplayDriver [1]` in its `config.txt`
  ([3dfx Voodoo Graphics](../CONFIGURATION.md#3dfx-voodoo-graphics)).
* **Glover**: on a Voodoo3 (`machine=svga_voodoo3`), install the driver
  from 3dfx's Voodoo3 driver CD 1.2 (`3dfx32v3.dll` 4.11.01.1151), not
  the later 1.07.00. With 1.07.00 the picture flickers: about half the
  frames show only the background colour, and the LOADING text is
  missing. 4.11.01.1151 draws it correctly with DirectX 6.1 or 7. It
  was checked only with its files copied over 1.07.00's
  ([the Voodoo3 driver](win9x-howto.md#the-voodoo3-driver)). On the
  built-in Windows Glover needs neither.
* **GLQuake with the shareware data**: rebuilds its palette table at
  every start, behind a window that takes the focus, so the game stays
  minimised (Alt+Tab brings it back). The registered data doesn't
  ([game notes](win9x-howto.md#game-notes)).
* **Incoming** (GOG release): run it without GOG's wrapper DLLs. On a
  Voodoo3 with the 1.07.00 driver it needs DirectX 7
  ([3dfx Voodoo3 and
  Banshee](../CONFIGURATION.md#3dfx-voodoo3-and-banshee)).
* **OpenGL games for a Voodoo1 or 2** that bring 3dfx's MiniGL as
  `opengl32.dll` in their folder: on a Voodoo3, take it away so the game
  gets the card's OpenGL driver. Quake II's `3dfxgl.dll` is used only
  when chosen in its video menu ([game notes](win9x-howto.md#game-notes)).
* **Oni**: on a Voodoo3, take away a MiniGL `opengl32.dll` from its
  folder. On a Voodoo2, copy `3DFXVGL.DLL` into its folder as
  `OPENGL32.DLL`
  ([OpenGL games under
  Windows](../CONFIGURATION.md#opengl-games-under-windows)).
* **Quake III Arena** on a Voodoo2: `r_glDriver 3dfxvgl`
  ([OpenGL games under
  Windows](../CONFIGURATION.md#opengl-games-under-windows)).
* **SubCulture** (3dfx version): Windows 95, `voodoo=true`,
  `machine=svga_s3`, `memsize=64`, `cpu=pentium`
  ([3dfx Voodoo Graphics](../CONFIGURATION.md#3dfx-voodoo-graphics)).
* **Wipeout for 3D Rage**: `machine=svga_ati_3drage` and ATI's driver
  4.35 with one line added to its INF
  ([ATI 3D RAGE](../CONFIGURATION.md#ati-3d-rage)).
