# 3dfx cards

Rust-DOS emulates four of 3dfx's cards. Two are 3D cards that sit beside
the display adapter and take over the monitor while a game draws on them,
as the real boards did through their pass-through cable: the Voodoo
Graphics and the Voodoo2. The other two are display adapters with 2D and
3D on one card: the Voodoo3 3000 and the Voodoo Banshee. Rust-DOS draws
each card's picture in software, or again with OpenGL at up to four times
the card's resolution.

This page says which card a game needs and how to set it up. The
reference is in CONFIGURATION.md: [3dfx Voodoo
Graphics](../CONFIGURATION.md#3dfx-voodoo-graphics), [3dfx
Voodoo2](../CONFIGURATION.md#3dfx-voodoo2) and [3dfx Voodoo3 and
Banshee](../CONFIGURATION.md#3dfx-voodoo3-and-banshee).

## Which card

| The game | Card | In `[emulator]` |
|---|---|---|
| A DOS game with a 3dfx or Glide version | Voodoo Graphics | `voodoo=true`, `cpu=pentium` |
| A DOS game that brings the Voodoo2's `GLIDE2X.OVL` or Glide 3 | Voodoo2 | `voodoo=true`, `voodoo_model=voodoo2`, `cpu=pentium` |
| A Windows game on the [built-in Windows](builtin-windows.md) | Voodoo3 | `machine=svga_voodoo3` |
| A Windows game on a booted Windows 95 or 98 image | Voodoo3, with 3dfx's Voodoo3 driver installed in the image | `machine=svga_voodoo3` |
| A Windows 95 or 98 game that brings its own Glide for the Voodoo Graphics or Voodoo2 | Voodoo Graphics or Voodoo2 beside an S3 | `voodoo=true` (and `voodoo_model=voodoo2`), `machine=svga_s3` or `svga_s3virge` |

Start with the Voodoo3 for Windows games. Its driver brings Glide 2 and
3, Direct3D and OpenGL, and the built-in Windows has all three on it.
Take an add-on card when a game only runs on one: it brings a Glide
library made for that board, or it is one of the games
[compatibility.md](compatibility.md) names with an add-on card.

`machine=svga_voodoo3` replaces the display adapter, so it leaves no
room for `voodoo=true`; with both set, the machine has the Voodoo3
alone. The Banshee (`machine=svga_banshee`) has one texture unit to the
Voodoo3's two, and Windows 98's CD has a driver for it.

## DOS games

1. Set `voodoo=true` and `cpu=pentium` in the game's
   [profile](../CONFIGURATION.md#game-profiles) or `rust-dos.conf`, and
   `voodoo_model=voodoo2` for a game made for the Voodoo2.
2. Choose the game's 3dfx or Glide version in its setup, or start its
   3dfx program.
3. If the game says it can't find `GLIDE2X.OVL`, it was made for the
   Voodoo Rush or a later board and expects 3dfx's DOS overlay, which
   doesn't come with rust-dos. Open the settings window (Ctrl+F12), go to
   the Emulator page and choose *Download Glide's DOS overlay...* (shown
   while the card is on and the overlay isn't there yet). It downloads
   3dfx's Voodoo Graphics driver 3.01.00 from the Internet Archive, after
   asking, and puts its `GLIDE2X.OVL` on Z:, which is on the PATH. A copy
   beside the game comes first.

The overlay only loads with DOS/4GW as its own DPMI host. Rust-DOS
recognises a program that loads it and lets the DOS/4GW the game brings
run in place of rust-dos's DPMI host, with no setting to change.

## Windows on a booted image

[win9x-howto.md](win9x-howto.md) has the whole setup of a Windows 95 or
98 image. The 3dfx part of it:

- **Voodoo3 or Banshee:** install 3dfx's Voodoo3 driver in the image.
  The [reference driver
  1.07.00](https://archive.org/details/3dfx-voodoo-3-reference-driver-1.07.00)
  is the one tried; [the Voodoo3
  driver](win9x-howto.md#the-voodoo3-driver) has the steps. Its
  Direct3D needs DirectX 7 or later. Glover needs the earlier
  4.11.01.1151 from 3dfx's [Voodoo3 driver CD
  1.2](https://archive.org/details/voodoo3v12) (see
  [compatibility.md](compatibility.md)).
- **Voodoo2 on Windows 98 SE:** install 3dfx's reference driver 3.03.00
  ([`rkv2dx6.exe`](https://archive.org/details/rkv2dx6)) as [3dfx
  Voodoo2](../CONFIGURATION.md#3dfx-voodoo2) says. Windows' OpenGL
  doesn't use an add-on card by itself: [OpenGL games under
  Windows](../CONFIGURATION.md#opengl-games-under-windows) says how a
  game reaches it.
- **Voodoo Graphics on Windows 98 SE:** Windows installs its own driver
  from the Windows 98 CD at the next start.
- **Voodoo Graphics on Windows 95:** no driver. Games bring
  `glide2x.dll`, or install 3dfx's Glide runtime.

## The built-in Windows

The built-in Windows needs no 3dfx driver: its own `GLIDE2X.DLL`,
`GLIDE3X.DLL`, Direct3D and `OPENGL32.DLL` draw on the emulated card.
Set `machine=svga_voodoo3` with the other settings [Windows
games](../CONFIGURATION.md#windows-games) lists. What each card runs
there:

| | Voodoo3, Banshee | Voodoo2 | Voodoo Graphics |
|---|---|---|---|
| Glide 2 and 3 | Yes | Yes | Yes |
| Direct3D before DirectX 8 | Yes, in a window too | Full screen | Full screen |
| Direct3D 8 | Yes | No | No |
| OpenGL | Yes, in a window too | Full screen | No |

`GLIDE2X.DLL` and `GLIDE3X.DLL` are in the system folder only with a
3dfx card, so a game that checks for Glide by its file finds none
without one. Without a 3dfx card, Direct3D finds no 3D device: there is
no software Direct3D.

## Settings

These go in `[emulator]`. The renderer settings work for every 3dfx
card; the add-on card's settings do nothing on a Voodoo3 or Banshee,
which always has 16 MB.

| Setting | Values (default first) | What it does |
|---|---|---|
| `voodoo` | `false`, `true` | Adds the Voodoo Graphics or Voodoo2 add-on card |
| `voodoo_model` | `voodoo1`, `voodoo2` | The add-on card's chip |
| `voodoo_memory` | `12`, `8`, `4` | The add-on card's memory in MB: 12 is two texture units with 4 MB each, 8 a Voodoo2 with 2 MB each (it picks the Voodoo2 when `voodoo_model` isn't set), 4 a Voodoo Graphics with one texture unit (a Voodoo2 takes it as 8) |
| `voodoo_renderer` | `software`, `opengl` | What draws the card's picture. `opengl` draws it again at a higher resolution, and needs OpenGL 3 on the host |
| `voodoo_scale` | `2`, 1 to 4 | How many times the card's resolution OpenGL draws at |
| `voodoo_msaa` | `off`, `2`, `4`, `8` | OpenGL's antialiasing |
| `voodoo_anisotropy` | `off`, `2`, `4`, `8`, `16` | OpenGL's filtering of textures at an angle |
| `voodoo_dither` | `true`, `false` | Whether OpenGL dithers to 16 bits as the card does; `false` is smoother |
| `voodoo_readback` | `gpu`, `exact` | What a game reads back from the card: OpenGL's picture, or the card's exact pixels drawn in software too |
| `voodoo_texture_sampling` | `default`, `unfiltered` | The game's texture filtering, or nearest-neighbour |
| `voodoo_fps_cap` | `off`, 10 to 240 | The most frames a second a game shows on the card |
| `voodoo_gamma` | `off`, 0.1 to 2 | Glide's gamma, through `SST_RGAMMA`, `SST_GGAMMA` and `SST_BGAMMA` (add-on card only) |

`voodoo`, `voodoo_model`, `voodoo_memory`, `voodoo_gamma` and `machine`
take effect at the DOS prompt, the others at once. The settings
window's Emulator page has them as *Video card*, *3dfx Voodoo
Graphics*, *3dfx card* (the chip and memory together), *3dfx drawn by*
and the *3dfx* settings after it. They show only while *3dfx Voodoo
Graphics* is on; on a Voodoo3 or Banshee, set the renderer in the
configuration file
([#821](https://github.com/dividebysandwich/rust-dos/issues/821)).

The Voodoo2 is a single board: SLI, two Voodoo2s drawing alternate
lines, isn't emulated, so games see one card. The libretro core draws a
Banshee or Voodoo3 in software whatever `voodoo_renderer` says.

[The 3dfx OpenGL renderer](../CONFIGURATION.md#the-3dfx-opengl-renderer)
explains the OpenGL settings, and [3dfx frame rate
cap](../CONFIGURATION.md#3dfx-frame-rate-cap) and [3dfx
gamma](../CONFIGURATION.md#3dfx-gamma) the other two.
