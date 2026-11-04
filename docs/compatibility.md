# Game compatibility notes

Games that need a setting, a setup step or a particular driver to run
well in rust-dos, and the limits that are known. A game that isn't
listed here may run fine with the defaults; if one doesn't, please
[open an issue](https://github.com/dividebysandwich/rust-dos/issues)
that names the game, its settings and what happened.

Each entry says what to do and links to the section that explains the
setting. Notes for a game run on rust-dos's built-in Windows don't apply
to the same game on a Windows 95 or 98 image, and the other way round.

For DOS programs started from the prompt, `almanac_auto` (see
[`[emulator]`](../CONFIGURATION.md#emulator)) can fetch the settings
other players found best. It covers the machine's settings only, not
drivers, files or install steps, and it doesn't see a game that runs
inside a booted Windows image.

## DOS games

* **Blues Brothers: Jukebox Adventure**: the sound clicks and the game
  hangs about 75 seconds into the title music, on a real PC too. Two
  [game fixes](../CONFIGURATION.md#game-fixes), `jukebox-sb-mix` and
  `jukebox-mod-hang`, deal with both; the second was made for Razor
  1911's BLUES.EXE only.
* **Bao Xiao San Guo Zhi** (`SANGO.EXE`): every random roll comes out
  the same unless timer 2's gate is high at start. Set
  `pit2_gate_at_start=true` in [`[sound]`](../CONFIGURATION.md#sound),
  in the game's profile.
* **BloodNet** (GOG): GOG's `.bdn` file mounts as a CUE sheet
  ([disk images](../CONFIGURATION.md#disk-images)).
* **Descent**: plays badly at frame rates above the display's;
  `cycles=auto` doesn't speed it past the refresh rate. On a VFX1
  headset, start it as `DESCENT -vfx -vfxtrak` with `steer=true` in
  [`[vfx1]`](../CONFIGURATION.md#vfx1); its heads-up display shows in
  the left eye only. [docs/vfx1.md](vfx1.md) has the resolutions.
* **Descent 2** on a VFX1: `VR_type=1` and `VR_tracking=1` in
  `DESCENT.CFG` (its SETUP writes them) and `steer=true`.
  [docs/vfx1.md](vfx1.md) has a mode with more pixels that loses the
  head tracker.
* **Comanche 3** on a VFX1: in flight, Video > Stereoscopic, then
  Alt+F12 for head tracking ([`[vfx1]`](../CONFIGURATION.md#vfx1)).
* **Doom and Heretic** over a LAN or a serial line: `IPXSETUP -nodes 2`,
  or `SERSETUP -com2`
  ([playing over a LAN](../CONFIGURATION.md#playing-over-a-lan),
  [serial and modem games](../CONFIGURATION.md#serial-and-modem-games)).
* **F-117A, F-19, Gunship 2000, Covert Action** (MicroProse): DSWAP.EXE
  waits for the install disk and PLAYER.EXE loops forever without the
  copy protection's data; the
  [game fixes](../CONFIGURATION.md#game-fixes) `microprose-dswap` and
  `microprose-player` skip them.
* **King's Quest, Ultima and other early CGA games**: their composite
  colours need `machine=cga`, `composite=auto` or `on`, and
  `composite_era=old` ([`[emulator]`](../CONFIGURATION.md#emulator)).
* **Prehistorik 2**: the sound clicks at every note, on a real Sound
  Blaster too. The [game fix](../CONFIGURATION.md#game-fixes)
  `prehistorik2-sb-mix` mixes it around the centre.
* **Quake 1.08**: `fpu=fast` saves host processor time. On a VFX1:
  `VFX1QK QUAKE.EXE +vid_mode 7 +lcd_x 1` with `stereo=lines`.
* **Tomb Raider's 3D patches** (Gold CD, `3DPATCH`):
  * 3dfx: `voodoo=true`, `cpu=pentium`
    ([3dfx Voodoo Graphics](../CONFIGURATION.md#3dfx-voodoo-graphics)).
  * Voodoo Rush: needs 3dfx's `GLIDE2X.OVL`, which the settings window
    downloads onto Z:.
  * Vérité: copy `Tomb.exe` and `spd3d.uc` into the game's folder;
    `machine=svga_verite`, `cpu=pentium`, `memsize=32`
    ([Rendition Vérité](../CONFIGURATION.md#rendition-vérité)).
  * PowerVR: `powervr=pcx2`, `cpu=pentium`, `memsize=32`
    ([PowerVR PCX1 and PCX2](../CONFIGURATION.md#powervr-pcx1-and-pcx2)).
    Its saves don't load in the other versions, nor theirs in it.
* **vQuake**: `dpmi=false` (it brings its own CWSDPMI), `cpu=pentium`,
  `memsize=32`, and `-nocdaudio` unless the CD is mounted.
* **3D Blaster PCI games** (Battle Arena Toshinden SE, Rebel Moon,
  Flight Unlimited, Hi-Octane): copy the four `VRIT*170.CGL` modules
  into the game's folder
  ([3D Blaster PCI games](../CONFIGURATION.md#3d-blaster-pci-games-cgl)
  has each game's start command).

## Windows games on the built-in Windows

[Windows games](../CONFIGURATION.md#windows-games) and
[when a game doesn't run](../CONFIGURATION.md#when-a-game-doesnt-run)
cover the settings most games need: `cpu=pentium_ii` for an invalid
opcode, `win32_version=98se` when a message names the version, more
`memsize` when it runs out.

* **Enemy Infestation**: the profile in
  [a profile to start from](../CONFIGURATION.md#a-profile-to-start-from)
  installs and runs it. For a CD copied to a folder, give the drive the
  label `EI`.
* **Grom**: two CDs; list both images on the drive line and press
  Ctrl+F4 when the setup asks for the second.
* **Iron Helix**: `win32_depth=8` (256 colours).
* **Moto Racer**: `win32_depth=16`; it checks the desktop's depth before
  it sets its own mode.
* **Wages of War**: `win32_depth=8`; it stops on the default 32-bit
  desktop.
* **Xenocracy**: its hardware check looks for `GLIDE2X.DLL`, which is
  there only with a 3dfx card: `voodoo=true`, or
  `machine=svga_voodoo3` or `svga_banshee`.
* **SafeDisc 2 games**: use a CD image that keeps the error-correction
  data (a BIN or CHD dumped Redump-style). An image whose unreadable
  sectors read back as good fails the check, as on Windows 98.

## Windows games on a Windows 95 or 98 image

[docs/win9x-howto.md](win9x-howto.md) says how to make the image and
which drivers to install.

* **Expendable**: Windows 98 SE on a Voodoo Graphics card,
  `machine=svga_s3virge`, `cpu=pentium_ii`, `memsize=128`, and
  `DisplayDriver [1]` in its `config.txt`.
* **Glover**: on a Voodoo3 (`machine=svga_voodoo3`), install the driver
  from 3dfx's Voodoo3 driver CD 1.2 (`3dfx32v3.dll` 4.11.01.1151), not
  the later 1.07.00. With 1.07.00 about half the frames show only the
  background colour or a scene half drawn, and the LOADING text, the
  star wipe and the title screen are missing. 4.11.01.1151 draws it
  correctly with DirectX 6.1 or 7. This was checked only with that
  driver's files copied over 1.07.00's. On the built-in Windows Glover
  needs neither.
* **GLQuake with the shareware data**: rebuilds its palette table at
  every start, behind a window that takes the focus, so the game stays
  minimised (Alt+Tab brings it back). The registered data doesn't.
* **Incoming** (GOG release): run it without GOG's wrapper DLLs. On a
  Voodoo3 it needs DirectX 7.
* **OpenGL games for a Voodoo1 or 2** that bring 3dfx's MiniGL as
  `opengl32.dll` in their folder: on a Voodoo3, take it away so the game
  gets the card's OpenGL driver. Quake II's `3dfxgl.dll` is used only
  when chosen in its video menu.
* **Oni**: on a Voodoo3, take away a MiniGL `opengl32.dll` from its
  folder. On a Voodoo2, copy `3DFXVGL.DLL` into its folder as
  `OPENGL32.DLL`
  ([OpenGL games under Windows](../CONFIGURATION.md#opengl-games-under-windows)).
* **Quake III Arena** on a Voodoo2: `r_glDriver 3dfxvgl`.
* **SubCulture** (3dfx version): Windows 95, `voodoo=true`,
  `machine=svga_s3`, `memsize=64`, `cpu=pentium`.
* **Unreal Tournament demo**: to set it up before its first start, put
  `GameRenderDevice=GlideDrv.GlideRenderDevice` and `FirstRun=348` in
  `System\UnrealTournament.ini`.
* **Wipeout for 3D Rage**: `machine=svga_ati_3drage` and ATI's driver
  4.35 with one line added to its INF
  ([ATI 3D RAGE](../CONFIGURATION.md#ati-3d-rage)).
