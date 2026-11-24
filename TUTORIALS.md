# Tutorials: setting up a game

Each tutorial here sets up one common case, with a whole configuration
ready to copy and change. The paths are examples: put your own in their
place. [CONFIGURATION.md](CONFIGURATION.md) has every setting the
tutorials use, and the links after each one go to the part that explains
it.

Most tutorials make a game profile: a file such as `games/doom.conf` in the
`games` folder beside your `rust-dos.conf` (`%APPDATA%\rust-dos\games` on
Windows, `~/.config/rust-dos/games` on Linux,
`~/Library/Application Support/rust-dos/games` on macOS; make the folder
if it isn't there). A profile holds only what differs from your
`rust-dos.conf`, and its settings and drives are put back as they were
when the game ends. Start one with `rust-dos --game doom`, or from the
Games page of the settings window (Ctrl+F12).

Paths in a profile are relative to the `games` folder, `~` is your home
folder, and a path with spaces goes in quotes. On Windows, either kind
of slash works: `C=D:\Games\Doom` or `C="D:/My Games/Doom"`.

* [Which tutorial](#which-tutorial)
* DOS games: [from a folder](#a-dos-game-from-a-folder),
  [in a profile](#a-dos-game-in-a-profile),
  [from a zip](#a-dos-game-from-a-zip-or-7z-archive),
  [from GOG or DOSBox](#a-game-set-up-for-dosbox-or-gog),
  [from a CD image](#a-dos-game-on-a-cd-image),
  [from floppy images](#a-dos-game-on-floppy-images),
  [with General MIDI or MT-32 music](#better-music-general-midi-mt-32-sound-canvas),
  [with a 3dfx card](#a-dos-game-with-a-3dfx-card)
* Windows games:
  [installed from a CD image](#a-windows-95-or-98-game-installed-from-its-cd-image),
  [already installed](#an-installed-windows-game-gog-or-a-copied-folder),
  [from 2000 to 2002, on the ToadForce](#a-demanding-windows-game-from-2000-to-2002-toadforce),
  [on a Windows 98 image](#a-windows-game-on-a-windows-98-image-with-its-cd-image)
  and [on a Voodoo2 in Windows 98](#a-windows-98-game-on-a-voodoo2)

## Which tutorial

| The game | Tutorial | What you need besides the game |
|---|---|---|
| A DOS game in a folder | [From a folder](#a-dos-game-from-a-folder), then [in a profile](#a-dos-game-in-a-profile) | Nothing |
| A DOS game in a zip or 7z | [From a zip](#a-dos-game-from-a-zip-or-7z-archive) | Nothing |
| A GOG DOS game, or one set up for DOSBox | [From GOG or DOSBox](#a-game-set-up-for-dosbox-or-gog) | Nothing |
| A DOS game on CD | [From a CD image](#a-dos-game-on-a-cd-image) | An image of the CD (CUE/BIN, ISO, CHD...) |
| A DOS game with a 3dfx version | [With a 3dfx card](#a-dos-game-with-a-3dfx-card) | Nothing, or 3dfx's `GLIDE2X.OVL`, which the settings window downloads |
| A Windows 95 or 98 game on CD | [Installed from a CD image](#a-windows-95-or-98-game-installed-from-its-cd-image) | An image of the CD |
| A Windows game already installed (GOG, or copied from a PC) | [Already installed](#an-installed-windows-game-gog-or-a-copied-folder) | Its registry keys, for some games |
| A Windows game of 2000 to 2002 that a Voodoo3 doesn't run | [On the ToadForce](#a-demanding-windows-game-from-2000-to-2002-toadforce) | Nothing |
| A Windows game the built-in Windows doesn't run yet | [On a Windows 98 image](#a-windows-game-on-a-windows-98-image-with-its-cd-image) | A Windows 98 SE CD, and an image with Windows installed on it |

DOS games run on Rust-DOS's own DOS. Windows games run on its built-in
Windows first: it needs no copy of Windows, and most of the Windows
tutorials use it. A game it doesn't run yet runs on a real Windows 98
installed on a disk image, which takes a Windows CD and an hour or so
of setting up ([win9x-howto.md](docs/win9x-howto.md)).

Before you write settings by hand, check whether
[almanac.rust-dos.com](https://almanac.rust-dos.com) has the game: the
settings window's *Fetch settings from Almanac* puts other players'
settings in place ([compatibility.md](docs/compatibility.md#the-almanac-first)).
[compatibility.md](docs/compatibility.md) lists what particular games need
beyond the tutorials here.

## A DOS game from a folder

The quickest way to try a game. Start Rust-DOS with the game's folder as
drive C:

```sh
rust-dos -d ~/games/doom
```

and type the game's program at the prompt (`DOOM`). Or start Rust-DOS
with no options and drop the folder onto its window: it is mounted on
the first free drive from D:, which the message names, so type `D:`
first. `DIR` lists the files, and the program to start is usually the
`.EXE` or `.BAT` named after the game.

Many games need their setup run once (`SETUP` or `INSTALL`) to choose
the sound card. The defaults that setup programs offer work: a Sound
Blaster 16 at port 220, IRQ 7, DMA 1 (high DMA 5), and the Gravis
Ultrasound at 240.

Leave `cycles` at `auto`, the default. It finds the speed the game
needs. Set a number only for a game that runs too fast, and Ctrl+F11 and
Ctrl+Shift+F11 to find it.

## A DOS game in a profile

A profile starts the game with one command, and keeps what it writes
(saved games, the setup's choices) apart from the game's folder:

```ini
# games/doom.conf
[game]
name=Doom
overlay=true

[drives]
C=~/games/doom

[autoexec]
C:
DOOM
```

```sh
rust-dos --game doom
```

`overlay=true` writes the game's changes to `saves/doom` beside the
`games` folder instead of its folder; **R** on the Games page deletes
them. When the game quits, Rust-DOS is back at its prompt with the
drives as they were.

The settings window makes the same file: start the game by hand, press
Ctrl+F12, go to the Games page and press **Ins**. It writes the settings
you changed since starting, the drives you mounted and the command that
started the game.

See: [game profiles](CONFIGURATION.md#game-profiles),
[`[drives]`](CONFIGURATION.md#drives).

## A DOS game from a zip or 7z archive

Drop the archive onto the window, or:

```sh
rust-dos --import ~/games/keen.zip
```

The archive becomes drive C: and a profile on the Games page, read
where it is: it is never unpacked or changed, and what the game writes
goes to its saves folder. If the archive holds one program (setup and
install programs aside), it starts; otherwise the prompt waits on C:.

To give the game settings and start commands, put a `rust-dos.conf` at
the archive's root:

```ini
# rust-dos.conf inside keen.zip
[game]
name=Commander Keen 4

[emulator]
cycles=10000

[autoexec]
C:
KEEN4E
```

Paths in it are relative to the archive's root, so a CD image inside the
archive is `D=CD/GAME.CUE`.

See: [game packages](GAME-PACKAGES.md#game-packages).

## A game set up for DOSBox or GOG

A GOG DOS game, a folder with a `dosbox.conf`, or an eXoDOS game:

```sh
rust-dos --import "C:\GOG Games\Ultima Underworld"
```

or drop the folder (or the `.conf` file) onto the window. Rust-DOS reads
the DOSBox configuration, makes a profile with its CPU, memory, video
and sound settings, its drives (CD images too) and its commands, and
starts the game. From then on it is on the Games page. What doesn't
come across is listed in the [log](REFERENCE.md#log-file).

A GOG game that is a Windows program gets a profile for the built-in
Windows instead
([an installed Windows game](#an-installed-windows-game-gog-or-a-copied-folder)).

See: [game profiles](CONFIGURATION.md#game-profiles).

## A DOS game on a CD image

Mount the image as a CD-ROM drive, and install the game to C: once:

```ini
# games/descent2.conf
[game]
name=Descent II
overlay=true

[drives]
C=~/games/descent2
D=~/cd/descent2.cue cdrom

[autoexec]
D:
IF NOT EXIST C:\DESCENT2\DESCENT2.EXE INSTALL
C:
CD \DESCENT2
DESCENT2
```

`~/games/descent2` starts as an empty folder. The first start runs the
installer from the CD; the `IF NOT EXIST` line skips it on later starts.
Install to `C:\DESCENT2` so the line finds the game. CD audio tracks
play from the image.

Images that work: a CUE sheet with its BIN files (or with WAV, MP3, OGG
or FLAC tracks), an ISO, a CHD, a CloneCD `.ccd`, an Alcohol 120% `.mds`,
a BlindWrite `.b5t` or `.b6t`, and GOG's `.ins` and `.gog`. For a game on
several CDs, list them all on the line (`D=~/cd/disc1.cue ~/cd/disc2.cue
cdrom`) and press Ctrl+F4 to change discs. A copy-protected disc may need
an image that keeps more than an ISO does
([copy-protection.md](docs/copy-protection.md)).

See: [CD images](CONFIGURATION.md#cd-images).

## A DOS game on floppy images

```ini
# games/monkey.conf
[game]
name=The Secret of Monkey Island
overlay=true

[drives]
C=~/games/monkey
A=~/floppies/monkey1.img ~/floppies/monkey2.img ~/floppies/monkey3.img ~/floppies/monkey4.img

[autoexec]
A:
INSTALL
```

The images go in A: one after the other: Ctrl+F4 puts the next one in
when the installer asks for it. After the install, change the
`[autoexec]` to start the game from C:.

See: [mounting drives](CONFIGURATION.md#mounting-drives),
[disk images](CONFIGURATION.md#disk-images).

## Better music: General MIDI, MT-32, Sound Canvas

Choose *General MIDI* or *Roland MPU-401* in the game's setup, at port
330. With nothing more set, the music plays on the Gravis Ultrasound
patches built into Rust-DOS. For another sound, add one of these to the
profile or to `rust-dos.conf`, and choose *Roland MT-32* in the setup
for the MT-32:

```ini
[sound]
# A General MIDI SoundFont of your choice
soundfont=~/soundfonts/GeneralUser.sf2
```

```ini
[sound]
# A Roland Sound Canvas SC-55: the settings window's Sound page downloads
# its ROMs, after asking
midisynth=sc55
```

```ini
[sound]
# A Roland MT-32, for Sierra and LucasArts games from 1988 to 1992:
# the ROMs go in mt32-roms in Rust-DOS's configuration folder
midisynth=mt32
```

The MT-32 needs munt's library: it comes with the Windows download, and
is the `libmt32emu2` package on Debian and Ubuntu, `munt` on Arch and
`brew install mt32emu` on macOS.

See: [`[sound]`](CONFIGURATION.md#sound).

## A DOS game with a 3dfx card

DOS games with a 3dfx version draw through Glide on a Voodoo Graphics.
Tomb Raider with the 3dfx patch from the Tomb Raider Gold CD
(`3DPATCH\3DFX\TOMB.EXE`, copied into the game's folder):

```ini
# games/tombraider-3dfx.conf
[game]
name=Tomb Raider (3dfx)
overlay=true

[emulator]
voodoo=true
cpu=pentium
# Draw the card's picture again with OpenGL at twice its resolution;
# leave these out to draw it as the card did
voodoo_renderer=opengl
voodoo_scale=2

[drives]
C=~/games/tombraid
D=~/cd/tombraider.cue cdrom

[autoexec]
C:
CD \TOMBRAID
TOMB
```

For a game made for the Voodoo2 (one that brings the Voodoo2's
`GLIDE2X.OVL`, or Glide 3), add `voodoo_model=voodoo2`. A game that says
it can't find `GLIDE2X.OVL` wants 3dfx's DOS overlay, which doesn't come
with Rust-DOS: open the settings window (Ctrl+F12), and on the Emulator
page choose *Download Glide's DOS overlay...*.

`voodoo_renderer=opengl` needs OpenGL 3 on your computer. `voodoo_scale`
goes up to 4; `voodoo_msaa=4` smooths the edges.

See: [3dfx.md](docs/3dfx.md), [the 3dfx OpenGL
renderer](CONFIGURATION.md#the-3dfx-opengl-renderer).

## A Windows 95 or 98 game installed from its CD image

The built-in Windows runs the game's own setup the first time, from the
CD image, and the game after it:

```ini
# games/ei.conf
[game]
name=Enemy Infestation
windows=builtin
overlay=true

[emulator]
machine=svga_voodoo3
cpu=pentium_ii
win32_version=98se

[drives]
C=~/games/ei
D=~/cd/EnemyInfestation.iso cdrom

[autoexec]
SET TEMP=C:\WINDOWS\TEMP
D:
IF NOT EXIST C:\PROGRA~1\ENEMYI~1\EI.EXE SETUP.EXE
C:
CD \PROGRA~1\ENEMYI~1
EI.EXE
```

`~/games/ei` starts as an empty folder: the setup installs into it. The
`[emulator]` lines are the ones every Windows game wants: a Voodoo3 for
Direct3D, OpenGL and Glide, a Pentium II for the instructions games of
the time use, and Windows 98 SE. Add `memsize=128` for a 3D game from
1998 on.

The `[autoexec]` lines run at the DOS prompt, so they use 8.3 names
(`PROGRA~1` for `Program Files`). After the first install, `DIR C:\` at
the prompt shows the names of the folders the setup made: put them in
the `IF NOT EXIST` line and the last two lines. A setup that doesn't run
on the built-in Windows can be unpacked instead, or the game copied from
a PC where it is installed
([getting the game installed](CONFIGURATION.md#getting-the-game-installed)).

If the game doesn't start, the
[log](REFERENCE.md#log-file) says why, and [when a game doesn't
run](CONFIGURATION.md#when-a-game-doesnt-run) goes through the usual
causes. A game that wants 256 colours takes `win32_depth=8`; one that
wants an 800x600 desktop takes `win32_desktop=800x600`.

See: [Windows games](CONFIGURATION.md#windows-games),
[builtin-windows.md](docs/builtin-windows.md).

## An installed Windows game (GOG, or a copied folder)

A GOG Windows game:

```sh
rust-dos --import "C:\GOG Games\Some Game"
```

makes a `windows=builtin` profile that runs the program GOG's launcher
starts, with the game's folder as C:. The import writes no `[emulator]`
section: open the profile in a text editor (it is in the `games` folder)
and add the one from the tutorial above (`machine=svga_voodoo3`,
`cpu=pentium_ii`, `win32_version=98se`). Take GOG's wrapper DLLs
(`ddraw.dll`, `aqrit.cfg`, dgVoodoo's files and the like) out of the
game's folder when it has them: they replace parts of Windows that
Rust-DOS has its own of. [compatibility.md](docs/compatibility.md) has
what particular GOG games need besides.

A game copied from a PC where it is installed:

```ini
# games/rom2.conf
[game]
name=Rage of Mages II
windows=builtin
overlay=true

[emulator]
machine=svga_voodoo3
cpu=pentium_ii
win32_version=98se

[drives]
C=~/games/rom2-c

[autoexec]
C:
CD \ROM2
ALLODS2.EXE -1024
```

Here the game's folder was copied to `~/games/rom2-c/ROM2`, so the game
sees it as `C:\ROM2`. Games that read their folders or their CD's drive
letter from the registry need those keys in `games/rom2.reg`, as
REGEDIT4 text, beside the profile:

```
REGEDIT4

[HKEY_LOCAL_MACHINE\Software\Rage of Mages 2]
"INSTALLDIR"="C:\\ROM2"
"CD"="C:\\ROM2"
```

See: [copying an installed game](CONFIGURATION.md#copying-an-installed-game),
[registry keys](CONFIGURATION.md#registry-keys).

## A demanding Windows game from 2000 to 2002 (ToadForce)

Games from 2000 on that ask for more than a Voodoo3 has (32-bit
textures, compressed textures, hardware transform and lighting, Direct3D
8) run on the ToadForce, a 3D card Rust-DOS defines for them. Zanzarah:
The Hidden Portal, installed from its CD image:

```ini
# games/zanzarah.conf
[game]
name=Zanzarah
windows=builtin
overlay=true

[emulator]
machine=svga_toadforce
win32_version=98se

[drives]
C=~/games/zanzarah
D=~/cd/zanzarah.iso cdrom

[autoexec]
SET TEMP=C:\WINDOWS\TEMP
D:
IF NOT EXIST C:\PROGRA~1\ZANZARAH\SYSTEM\ZANZARAH.EXE SETUP.EXE
C:
CD \PROGRA~1\ZANZARAH\SYSTEM
ZANZARAH.EXE
```

This is the tutorial for [a game installed from its CD
image](#a-windows-95-or-98-game-installed-from-its-cd-image) with
another card: `machine=svga_toadforce`. With it the machine gets a
Pentium III and 128 MB of memory unless `cpu` and `memsize` say
otherwise. Your graphics card draws the ToadForce's 3D through OpenGL
3.2 where it has it, and the processor draws it elsewhere; on the
processor, choose a lower resolution in the game.

The ToadForce has no Glide (take `svga_voodoo3` for Glide games), no
pixel shaders, and no driver for a booted Windows 95 or 98.

See: [ToadForce](CONFIGURATION.md#toadforce), and
[compatibility.md](docs/compatibility.md#windows-games-on-the-built-in-windows)
for what particular games need on it.

## A Windows game on a Windows 98 image, with its CD image

For a game the built-in Windows doesn't run yet. This needs a hard disk
image with Windows 98 SE and 3dfx's Voodoo3 driver installed:
[win9x-howto.md](docs/win9x-howto.md) goes through making one, once.
The game then gets a profile that boots the image with the game's CD in
the drive, here Glover:

```ini
# games/glover-win98.conf
[game]
name=Glover (Windows 98)
windows=image
overlay=true

[emulator]
machine=svga_voodoo3
cpu=pentium_iii
memsize=128

[sound]
sbtype=sb16

[drives]
C=~/images/win98se.img
D=~/cd/glover.iso cdrom

[autoexec]
BOOT -l C
```

With `overlay=true` the image stays as it is: what Windows writes,
the game's install included, goes to a file in the game's saves folder,
so every game gets a copy of Windows of its own over one image. Install
the game from the CD in Windows as you would on a PC, and end each
session with Start, Shut Down.

Give every game that boots the image the same `[emulator]`, `[sound]`
and network settings the image was installed with: Windows asks for
drivers again when it finds other hardware. A few games need a
particular driver version: Glover draws right only with the Voodoo3
driver 4.11.01.1151 ([compatibility.md](docs/compatibility.md) names
the others).

See: [booting a disk image](CONFIGURATION.md#booting-a-disk-image),
[Windows 98](CONFIGURATION.md#windows-98),
[3dfx Voodoo3 and Banshee](CONFIGURATION.md#3dfx-voodoo3-and-banshee).

## A Windows 98 game on a Voodoo2

A few games of 1997 to 1999 run only on an add-on 3dfx card: they bring
a Glide made for the Voodoo Graphics or Voodoo2, or draw wrongly on a
Voodoo3 with every driver ([compatibility.md](docs/compatibility.md) names
them). The Voodoo2 sits beside an S3 that runs the desktop:

```ini
# games/td4-voodoo2.conf
[game]
name=Test Drive 4 (Voodoo2)
windows=image
overlay=true

[emulator]
machine=svga_s3
cpu=pentium_ii
memsize=128
voodoo=true
voodoo_model=voodoo2
voodoo_memory=12

[drives]
C=~/images/win98se-voodoo2.img
D=~/cd/td4.cue cdrom

[autoexec]
BOOT -l C
```

The image needs 3dfx's Voodoo2 driver 3.03.00, installed as [3dfx
Voodoo2](CONFIGURATION.md#3dfx-voodoo2) says. Keep it an image of its
own: Windows installed on a Voodoo3 machine finds other hardware here.
Windows' OpenGL doesn't use an add-on card by itself, so an OpenGL game
needs one more step ([OpenGL games under
Windows](CONFIGURATION.md#opengl-games-under-windows)).

See: [3dfx.md](docs/3dfx.md).
