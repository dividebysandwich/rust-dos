# Windows 95 and 98 system images

Before you make an image, try the built-in Windows. rust-dos has
experimental built-in support for Windows 3.1, 95 and 98 programs: with
`win32=true` in `[emulator]`, or `windows=builtin` in a game package's
`[game]`, a Windows game runs from the DOS prompt on system DLLs written
into the emulator, with no Windows image to install. `win32_version`
picks Windows 95, 98 or 98 SE, and Windows 3.1 programs run under each
of them. The settings are under `win32` in
[CONFIGURATION.md](../CONFIGURATION.md#emulator), and "Windows games"
there says how to install a game for it. Not every game runs this way
yet: if one doesn't start, or something in it looks, sounds or behaves
wrong, please
[open an issue](https://github.com/dividebysandwich/rust-dos/issues)
that names the game, its settings and what happened. Make your own
image, as the rest of this page describes, for the games the built-in
Windows can't run yet.

This is how to make a Windows 95 or 98 hard disk image that games can
name as their operating system, and how to use it: as a desktop of its
own, with games that run from a folder, and with games that have to be
installed. The example throughout is a Voodoo3 machine, so that Glide,
OpenGL and Direct3D games all work, but the steps are the same for any
display adapter rust-dos has.

The pieces are described in more detail in
[CONFIGURATION.md](../CONFIGURATION.md) ("Booting a disk image",
"Operating systems by name", "3dfx Voodoo3 and Banshee") and
[GAME-PACKAGES.md](../GAME-PACKAGES.md) ("An operating system", "The
automount folder").

## How it fits together

* The installed system is one VHD in the OS images folder
  (`~/.config/rust-dos/os` on Linux, `os` beside the executable in a
  portable install, `system/rust-dos/os` for the libretro core), say
  `win98se_v3.vhd`.
* A game names it with `os=win98se_v3` in its `rust-dos.conf`. The image
  is never written to: what Windows writes while a game runs goes to a
  delta file in that game's saves folder, so every game has its own copy
  of the system and the image stays as it was installed.
* A game that runs from its folder is the system's D:. A game that has
  to be installed ships its install as `automount/c.vhd`, a differencing
  VHD over the system's image that holds only the sectors the install
  changed.

## Pick the machine first

Windows binds drivers to the hardware it finds. A game that boots the
image on other hardware (another display adapter, another sound card,
the network card at other resources) gets the "New Hardware Found"
wizard, and maybe a driver it can't find. So decide the machine once,
install the system on it, and give every game the same settings. For a
Voodoo3 machine:

```ini
[emulator]
machine=svga_voodoo3
cpu=pentium_iii
memsize=128
cycles=max

[sound]
sbtype=sb16
sbbase=220
irq=7
dma=1
hdma=5

[network]
ne2000=true
nicbase=300
nicirq=3
```

Write every one of these out in the games' configurations, even where
it is rust-dos's default: a game's settings go over the player's own
`rust-dos.conf`, which may have `memsize=256` or `machine=svga_s3`.
Settings that Windows doesn't see (the scale, shaders, the Voodoo's
renderer, `fpu`, MIDI) can stay the player's.

The NE2000's `nicbase` and `nicirq` must be what the image's driver was
set up with; see [the network card](#the-network-card).

## Working on an image

Do the setup on a raw image (`.img`) or a VHD that is a copy, and boot it
from a configuration of its own, where the drive is written in place:

```ini
[emulator]
...the machine above...

[drives]
C=build.img -boot
D=setup cdrom -label SETUP
```

`setup` is a host folder with the drivers and tools, which Windows sees
as a CD. On a disk whose Windows has only C:, the CD is D:; on one that
was set up in another emulator with more drives, it can be E: or later
(look in My Computer). A host folder on a hard drive letter
(`E=folder`) is a second hard disk instead, which Windows calls D:, and
whose changes go back to the folder when Windows shuts down. It is the
easier way to bring in files Windows has to write next to, such as a
game that makes its configuration on its first run.

Always end a session with Start > Shut Down. The log says "the system
turned the power off" when Windows has finished. A machine closed
during Windows' start can boot into safe mode the next time; start again
from the copy you made before.

Keep a copy of the image after each step that worked. On a copy-on-write
file system, `cp --reflink=always` makes one in no time and no space.

## Windows 98 SE

### Starting point

Either install Windows 98 SE from its CD onto a new disk (see "Booting a
disk image" in CONFIGURATION.md, which also has the steps for a disk made
with `MAKEIMG`), or start from an image installed elsewhere. An image
made in another emulator usually needs two things:

* Windows turns its IDE driver off for good when it fails once, as it can
  in other emulators, and then sees no CD-ROM drive. Delete the value
  `NoIDE` in `HKEY_LOCAL_MACHINE\System\CurrentControlSet\Services\VxD\IOS`
  with `regedit` and restart.
* Its network card is set up at the other emulator's resources; see
  [the network card](#the-network-card).

### The Voodoo3 driver

Windows 98 SE doesn't have a Voodoo3 driver. 3dfx's 1.07.00 is known to
work: put its `driver9x` folder (the one with `Voodoo3.inf`) on the setup
CD. At the first start with `machine=svga_voodoo3`, Windows finds a
"Standard PCI Graphics Adapter (VGA)":

1. **Next**, then "Search for the best driver", **Next**.
2. Untick "Microsoft Windows Update", tick "Specify a location" and type
   the folder, `D:\V3` say. **Next**: Windows shows "3dfx Voodoo3".
   If it shows `MSDISP.INF` instead, it didn't find the INF there: check
   the drive letter.
3. Keep your own files when it asks about older versions
   (`msvcrt.dll`), **Finish**, and restart.
4. After the restart Windows finds a "Plug and Play Monitor" and installs
   it from its own files. The desktop comes up at 1024x768.

The driver brings Glide 2 and 3 (`glide2x.dll`, `glide3x.dll`), the
OpenGL ICD (`3dfxOGL.dll`) and Direct3D.

### DirectX

The Voodoo3's Direct3D needs DirectX 7 or later; 98 SE comes with 6.1.
Install DirectX 7.0 (`dx70eng.exe`, which unpacks with 7-Zip; run its
`DXSETUP.EXE`) or later. Check first: an image that has had other
software installed may already have 9.0c, and DirectX setup says so and
stops.

### ScanDisk at start

When rust-dos is closed while Windows runs, which players will do,
Windows starts the next time with ScanDisk. Turn that off in `C:\MSDOS.SYS`:

```
[Options]
...
AutoScan=0
```

The file is read-only, hidden and system (`attrib -r -s -h C:\MSDOS.SYS`
before editing it, `attrib +r +s +h` after). It must stay longer than
1024 bytes, which its `;xxx` lines are for.

### Windows 98 notes

* A DOS program run from Start > Run opens in a window. Alt+Enter makes
  it full screen and back.
* Explorer copies a folder from another drive (Ctrl+C, then Ctrl+V in
  `C:\`), and `xcopy D:\GAME C:\GAME\ /S /E` does it in a DOS box.

## Windows 95

Use OSR2 (4.00.950B or C, "4.00.1111" in its files): 3dfx's Voodoo3
driver and DirectX 7 support it. The steps are those of Windows 98 with
these differences:

* **The display driver:** OSR2's "Update Device Driver Wizard" first
  searches the drives and offers "Standard PCI Graphics Adapter (VGA)".
  Choose **Other Locations...**, give the folder with `Voodoo3.inf`, and
  **Finish**. When it asks for the disk "3dfx Voodoo3 Windows 9x AGP/PCI
  Install disk", give the same folder. Restart twice when asked.
* **DirectX 7** has to be installed: Windows 95 comes with DirectX 2 or
  3, which Direct3D games and the driver's Direct3D don't work with.
* **Windows' own files:** adding a network card or protocol asks for
  the Windows 95 disks. Put the OSR2 cabinets (`WIN95_02.CAB` to
  `WIN95_22.CAB`, `PRECOPY*.CAB`, `MINI.CAB`, from the CD's `WIN95`
  folder or the floppies) in a folder on the setup CD and give that
  folder when it asks.
* **ScanDisk at start:** OSR2 reads `AutoScan=0` too, but its
  `MSDOS.SYS` doesn't have the line. Add it under `[Options]`.
* **The network:** see below. Remove "Client for Microsoft Networks"
  and "Client for NetWare Networks" if games are all the image is for:
  without a client, Windows 95 doesn't ask for a network logon at each
  start.

## The network card

rust-dos's NE2000 has a router of its own: Windows gets its address by
DHCP and reaches the internet, and instances in one room are on one LAN
for multiplayer games (see "Networking" in CONFIGURATION.md).

* **An image that has an NE2000 driver** keeps the resources it was set up
  with. Look them up in Device Manager (Network adapters, NE2000
  Compatible, Resources) and give rust-dos the same `nicbase` and
  `nicirq`. Setting them in Device Manager to match rust-dos instead
  doesn't always take: on one Windows 98 image the new interrupt didn't
  stick, and the card went quiet.
* **Adding one:** Control Panel > Network > Add > Adapter >
  Novell/Anthem > NE2000 Compatible. In its Properties > Resources, set
  the I/O range to 300-31F (the default, 200-21F, is the joystick's) and
  the interrupt to rust-dos's. Add the protocol Microsoft > TCP/IP; it
  gets its address automatically by default. IPX/SPX, which Windows adds
  too, is what older network games use.
* **Check:** Start > Run > `winipcfg`, choose the NE2000 in the list:
  the address should be 10.0.2.15 with the gateway 10.0.2.2. An address
  of 169.254.x.x means DHCP got no answer: the resources don't match.

An image has COM2 at interrupt 3 too, so an NE2000 at interrupt 3 gives
COM2 a yellow mark in Device Manager. That does no harm unless something
needs COM2.

## Making the VHD

When the image is set up and shut down, make it a dynamic VHD, which
only stores what is in use:

```sh
qemu-img convert -O vpc -o subformat=dynamic,force_size=on build.img win98se_v3.vhd
```

Clusters that once held deleted files (Windows' setup leaves a lot) are
copied too. Zero the free space first to leave them out: in Windows,
before the last shutdown, a tool that writes zeros over free space does
it, or, on the host, anything that reads the FAT and zeroes the clusters
it marks free. That took the Windows 98 image above from 4 GB to 866 MB.

Put the VHD in the OS images folder. `REMOUNT` at the rust-dos prompt
lists the systems it finds there.

Treat the file as final from then on. Games' deltas and differencing
images only fit the image they were made over: when it changes, rust-dos
sets a game's delta aside (`.rdelta.stale-<date>`) and refuses a `c.vhd`
made over the old one. VirtualBox's `VBoxManage` may rewrite the
parent's footer the first time it makes a differencing image over it,
so make one before you copy the system image anywhere else, and use
that copy from then on.

## Using the image

### A Windows desktop

A folder with nothing but a `rust-dos.conf` is a package whose game is
the system itself:

```ini
[game]
name=Windows 98 SE
os=win98se_v3

...the machine above...
```

Drop the folder onto the rust-dos window, or `rust-dos --import
"Windows 98"`. It becomes a game on the Games page that boots the
desktop, and whatever you install or change goes to its saves folder (R
on the Games page puts it back as it was). Make one such folder for each
separate "PC" you want.

### A game that runs from its folder

Many games run from their folder without an installer, Quake II's and
the Unreal Tournament demo's for example. A package with the game's
files and a `rust-dos.conf` naming the system is enough: Windows sees the
package as D:, and the game starts from there (My Computer, D:). What the
game writes goes to the saves folder.

### A game that has to be installed

This makes a package that boots straight into the game:

1. **Make a differencing VHD** over the system's image:

   ```sh
   VBoxManage createmedium disk --filename c.vhd --diffparent ~/.config/rust-dos/os/win98se_v3.vhd --format VHD
   ```

   Use a VirtualBox settings folder of its own
   (`VBOX_USER_HOME=/tmp/vbox`) so the media aren't added to your own
   VirtualBox. Windows Disk Management and Hyper-V make differencing
   VHDs too.

2. **Boot it in place**, with the installer or the game's files on a host
   folder:

   ```ini
   [emulator]
   ...the machine above...

   [drives]
   C=c.vhd -boot
   E=stage
   ```

   rust-dos finds the parent by the name `c.vhd` records, in the OS
   images folder.

3. **Install the game** into C: (or copy its folder from D:). Run it once
   and set it up (renderer, resolution, sound), so that it doesn't ask
   on the player's first start. Some games can be set up beforehand in
   their configuration files instead; the Unreal Tournament demo, for
   example, with `GameRenderDevice=GlideDrv.GlideRenderDevice` and
   `FirstRun=348` in `System\UnrealTournament.ini`.

4. **Make it start with Windows:** Start > Run:

   ```
   rundll32 appwiz.cpl,NewLinkHere C:\WINDOWS\Start Menu\Programs\StartUp\
   ```

   opens the Create Shortcut wizard for the StartUp folder. Give the
   game's program with its options (`C:\QUAKE2\QUAKE2.EXE +set vid_ref
   gl`), **Next**, a name, **Finish**. The shortcut starts the program in
   its own folder. Restart Windows to see the game start.

5. **Quit the game and shut Windows down**, then pack:

   ```
   Quake II/
       rust-dos.conf      [game] name=..., os=win98se_v3, the machine
       automount/
           c.vhd
   ```

   ```sh
   zip -r -0 "Quake II.zip" "Quake II"
   ```

   With `c.vhd` in the automount folder, the package itself is no drive:
   Windows has C: and the CD drive, as on the machine it was installed
   on.

**The size of `c.vhd`.** A VHD stores what was written in blocks, 2 MB
each in the ones VirtualBox makes, and Windows writes a little all over
the disk at every start. Quake II's 53 MB install made a 160 MB
`c.vhd`, most of it blocks with a few sectors in them. The VHD format
allows smaller blocks: the same sectors rewritten into a VHD with 16 KB
blocks (same parent, same sector bitmaps) took 59 MB. Don't compress it
in the zip to save space either: rust-dos unpacks a compressed file into
memory when the game starts.

### Game notes

* **OpenGL games** get the Voodoo3's ICD through Windows'
  `opengl32.dll`. A game that brings 3dfx's MiniGL as `opengl32.dll` in
  its own folder (for a Voodoo1 or 2) should have it taken away; Quake
  II's `3dfxgl.dll` is only used when chosen in its video menu.
* **GLQuake with the shareware data** can't read the files it caches in
  `id1\glquake`: shareware Quake reads no files from subfolders outside
  its pak. So it rebuilds its palette table at every start, behind a
  progress window that takes the focus from the full-screen game, which
  then stays minimised on the taskbar (Alt+Tab brings it back). Adding
  a pak with the table doesn't help either: shareware Quake refuses
  "modified games". The registered data has neither problem.
* **Glide games** use the Voodoo3's `glide2x.dll` or `glide3x.dll`.
  Choose the Glide renderer in the game.
