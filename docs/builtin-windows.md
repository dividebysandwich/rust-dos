# The built-in Windows

Rust-DOS runs Windows 95 and 98 games, and Windows 3.1 programs, without
a copy of Windows. Its KERNEL32, USER32, GDI32, DirectX and the other
system DLLs are written into the emulator, as its DOS is, and a game
starts from the DOS prompt like a DOS game. No Windows files are needed.

It is experimental: it runs fewer games than a Windows 95 or 98 image
booted in the emulator ([win9x-howto.md](win9x-howto.md)). A program
that calls a function the built-in DLLs don't have yet stops with a
message naming it.

This page is about using it. The games known to run, and what each
needs, are in
[compatibility.md](compatibility.md#windows-games-on-the-built-in-windows).
How it is built is in
[builtin-windows-architecture.md](builtin-windows-architecture.md).

## Turning it on

- **At the prompt:** set [`win32=true`](../CONFIGURATION.md#emulator)
  and type the program's name, as for a DOS program. It runs in
  protected mode and comes back to the prompt when it ends. With `win32`
  off, the program's DOS stub runs instead and says it needs Windows.
- **From a game profile:** `windows=builtin` in the profile's `[game]`
  turns `win32` on while the game plays.
  [Windows games](../CONFIGURATION.md#windows-games) in CONFIGURATION.md
  has a profile to start from and the settings a Windows game needs:
  the machine (a Voodoo3 for 3D games), the processor, the memory,
  `win32_version` and `win32_depth`. A GOG install whose launcher starts
  a Windows program gets such a profile when it is imported.

The built-in Windows is Windows 95 unless `win32_version` says `98` or
`98se`. Under Windows 95 a program that imports a function Windows 98
added doesn't start, and its message names the setting. Most games and
setups were tested with `win32_version=98se`.

When a game doesn't run, the [log file](../REFERENCE.md#log-file) says what
the built-in Windows did last: its `[W32]` lines name each call, and the
last ones name a missing function or DLL.
[When a game doesn't run](../CONFIGURATION.md#when-a-game-doesnt-run)
goes through the usual causes.

## Installing games

A digital release, such as GOG's, is installed already. A game on CD
gets onto the hard disk in one of three ways, which
[Getting the game installed](../CONFIGURATION.md#getting-the-game-installed)
describes:

- **Run its setup** the first time the game starts. InstallShield 5,
  InstallShield 3 with `_INST32I.EX_` beside `SETUP.EXE`, and Wise
  setups run. InstallShield's `SETUP.EXE` is a 16-bit launcher that only
  starts the 32-bit engine; the built-in Windows unpacks that engine
  into `TEMP` and starts it itself, with the command line Windows 98
  SE's launcher gives it. The launcher's clean-up program is 16-bit and
  isn't run, so the engine's files stay in `TEMP`.
- **Unpack the installer** with an
  [`[install]`](../CONFIGURATION.md#unpacking-the-installer) section,
  without running the setup: InstallShield 5 and 6 cabinets and
  InstallShield 3 archives. The registry keys come from a `.reg` file
  beside the profile. On the host, `rust-dos unpack` lists or extracts
  the same archives
  ([Unpacking InstallShield installers](../CONFIGURATION.md#unpacking-installshield-installers)).
- **Copy an installed game** from Windows 98, with its registry keys in
  a `.reg` file. This works whatever the installer.

The registry is the file `C:\REGISTRY.REG`, REGEDIT4 text that can be
read and edited by hand. It is made the first time a program uses the
registry.

## What differs from Windows 98

### How an install differs

Setups of InstallShield 5, InstallShield 3's 32-bit engine and Wise were
compared with the same setup run on Windows 98 SE, file by file with
hashes, registry key by key and shortcut by shortcut. The files they
install and the registry keys they write are the same, apart from these:

- The setup's own logs differ: InstallShield's uninstall log (`.isu`) is
  a few dozen bytes larger, with the names of the engine's dialog
  variables in it, and Wise's `INSTALL.LOG` differs in the time, the
  CD's letter, a `Made Dir` line for `C:\Program Files` and the case of
  names.
- A file the setup writes the CD's letter into has the letter of the
  emulator's drive.
- InstallShield 3's engine also makes an empty `HKLM\SharedDLLs`: it
  creates SharedDLLs under the wrong key when it doesn't find it, which
  Windows 98, where it is always there, never shows.
- Names on a drive that is a host folder are in capitals, as DOS made
  them: the Start menu's folder is `START MENU`.
- Shortcuts are smaller: they have no relative path and no file times.
  Their targets, working folders and icons are the same.

### Disk space on a host folder

A hard disk that is a host folder has no size of its own, so its free
space is made up: between 1.75 and 3.75 GB through
`GetDiskFreeSpaceExA`, and at most just under 2 GB through
`GetDiskFreeSpaceA`, as Windows 95 and 98 give a real disk's space. It
drops as files are written. A setup that checks for the space it needs
finds enough. DOS's own answer for such a drive is 512 MB to 1 GB.

### The system folder

The built-in DLLs show as files in `C:\WINDOWS\SYSTEM`, to Windows
programs and DOS programs alike, as on Windows 98. A game that looks for
a DLL there before it loads it finds the ones the built-in Windows has.
`GLIDE2X.DLL` and `GLIDE3X.DLL` are among them only with a 3dfx card
(`voodoo=true`, or `machine=svga_voodoo3` or `svga_banshee`), so a game
that checks for Glide by its file sees none without one.

A file of the same name that a game copies to C: takes the shown file's
place, as a copy over Windows' own file does: setups that copy their own
`COMMDLG.DLL` or `VER.DLL` there succeed. So does a file moved to that
name, as InstallShield 6 puts its `MSVCRT.DLL` in place: it deletes the
old file and moves its own copy there. Deleting a shown file, or changing
its attributes, changes nothing.

`WING32.DLL`, WinG, is among them too. Games of 1995 that draw through
WinG installed it from their CDs; on the built-in Windows they find it
there without that, and a WinG that a game brings in its own folder
isn't loaded in its place. WinG's halftone brushes are solid brushes of
their colour, without WinG's dithering.

`MSVCRT20.DLL` and `MSVCRT40.DLL`, the C runtimes of Visual C++ 2 and
4 that Windows 95 and 98 have beside `MSVCRT.DLL`, are there too, for
programs and DLLs built with those compilers. Their functions are
`MSVCRT.DLL`'s, so the three share one heap and one set of open files,
where Windows' own keep one each.

A setup that installs an INF file through SETUPX copies its files with
Windows 98's version check: a file in place with a newer version stays.
The shown files have the versions of the built-in Windows' own, so a
package that brings an older `MSVCRT.DLL`, as the Windows Media Audio
runtime does, leaves the built-in one in place.

The 16-bit system modules are there too, as on Windows 95 and 98:
`KRNL386.EXE`, `USER.EXE`, `GDI.EXE`, `MMSYSTEM.DLL`, `COMMDLG.DLL` and
the others the built-in Windows has for Windows 3.1 programs. Each is an
NE file with the module's name, its exports and a version resource, so
a program that looks for `MMSYSTEM.DLL` by its file, as the Autodesk
Animator player `AAPLAY.DLL` does, finds it. Loading one by its name,
by its path in the system folder or as a copy of the file under another
name gives the built-in module.

### DOS programs started from Windows

A Windows program can start a DOS program, as setups do to run a DOS
installer. On Windows 98 the DOS program runs in a DOS box, full screen
or in a window, while the Windows programs go on. Here it runs full
screen, on the same drives, and the Windows programs wait until it
ends; then the desktop comes back.

Starting a PIF, as a Start menu icon of a DOS game does, runs the
program the PIF names, in its working folder (or the icon's) and with
its parameters. Its other settings (memory, window, close on exit)
aren't used yet.

### Notepad

`C:\WINDOWS\NOTEPAD.EXE` is there, as on Windows 98, and the
registry opens text files (`.txt`) with it. A setup that offers to show
its readme when it ends starts it, by its name or by opening the file,
and the readme opens in a window titled with its name
(`readme.txt - Notepad`), which stays open after the setup ends. The
DOS prompt comes back once it is closed too (Alt+F4 or its close
button). The text, in the system's fixed font, scrolls and can be
selected and copied. Notepad's menus aren't there, so nothing can be
saved, and a file that isn't there opens an empty window instead of
asking whether to make it.

### Registry Editor

`C:\WINDOWS\REGEDIT.EXE` is there too, for setups that import a
registration file with it, as Video for Windows 1.1's setup runs
`regedit.exe /s C:\WINDOWS\mplayer.reg`. Its command line works as on
Windows 98:

- `regedit file.reg` puts the file's keys into the registry and saves
  it at once. The file is in Windows 95's format (`REGEDIT4`, then
  `[key]` and `"name"="value"` lines) or in Windows 3.1's (`REGEDIT`,
  then `HKEY_CLASSES_ROOT\key = value` lines); any other file isn't
  imported. A message box then says that the file was imported, or why
  it wasn't. With `/s` there is no message box.
- `regedit /e file.reg` writes the whole registry to the file, in
  Windows 95's format, and `regedit /e file.reg HKEY_CLASSES_ROOT\key`
  that key with its subkeys. Nothing is written for a key that isn't
  there.

The editor's window isn't there: REGEDIT started without a file ends at
once. `/c` (replace the registry with a file) and `/d` (delete a key)
do nothing, and `/l:` and `/r:` are ignored.

### Registering a DLL

`C:\WINDOWS\SYSTEM\REGSVR32.EXE` registers a DLL, as Leadfoot's setup
runs `regsvr32.exe /s ir50_32.dll` for the Indeo 5 codec. Its command
line works as on Windows 98:

- `regsvr32 file.dll` loads the DLL and calls its DllRegisterServer,
  and `/u` its DllUnregisterServer. The DLL's code runs in REGSVR32's
  process, as on Windows.
- `/i` calls its DllInstall too, after the registration (or before the
  unregistration with `/u`), with the text after `/i:`; `/n` with `/i`
  calls DllInstall alone.
- A message box says whether it worked, and with `/s` there is none.
  The exit code is 0 when it worked, 3 when the DLL doesn't load, 4
  when the function isn't there and 5 when it fails.

### Text and dialogs

MS Sans Serif 8, the font of menus, dialogs and buttons, has glyphs drawn in
its style, with one-pixel strokes. The other fonts are the VGA's ROM glyphs,
scaled, with each character of the dialog fonts as wide as in Windows 98's.
Text still looks different, so some lines wrap or are cut where Windows 98's
aren't, and characters the built-in fonts lack, such as `®`, show as `?`.
Raster fonts a program adds from its own .FON or .FNT files with
`AddFontResource` are drawn with their own glyphs; its TrueType fonts get a
built-in face.

### Painting

Window painting matches Windows 98 SE's message for message in what
probes compared: shrinking, window regions, validating, frames,
BeginPaint and DefWindowProc. A program that counts paint messages can
see these differences:

- A child shown in a visible parent also invalidates the parent over
  the child on Windows, even with WS_CLIPCHILDREN. The built-in Windows
  doesn't, so the parent gets one WM_PAINT fewer, and PeekMessage
  filtered on the child finds the child's WM_PAINT, where on Windows
  the parent's comes first.
- SetWindowPos with a redraw sends WM_NCPAINT and WM_ERASEBKGND at once
  on Windows and leaves a shrunk window nothing to paint. The built-in
  Windows invalidates the whole window and paints it later.
- A CS_PARENTDC child of a parent with a pending invalidate reports the
  parent's WM_PAINT through a filtered PeekMessage, and gets its
  WM_NCPAINT and WM_ERASEBKGND at another time than on Windows.
- A child with a border gets its frame later, with one more WM_PAINT
  and WM_NCPAINT after UpdateWindow.

### Direct3D's devices

Direct3D lists the devices Windows 98 SE's DirectX 6.1 lists: the
software devices first, Ramp (through IDirect3D and IDirect3D2) and RGB,
then the HAL device where the card has one (a Banshee, a Voodoo3 or a
ToadForce as the display adapter, or a Voodoo Graphics or Voodoo2
add-on card in full screen), and Direct3D 7's T&L HAL device on a card
that transforms and lights. The software devices draw on the processor
into render targets and depth buffers in system or video memory, with
any card. They differ from Windows 98's in a few ways:

- The Ramp device reports its mono color model but lights, shades,
  blends and takes textures as the RGB device does, and its description
  says so. Windows' Ramp device lights in shades of one color through
  each material's ramp. It takes no 8-bit palettized render target.
- There is no MMX device.
- They draw with one texture stage, don't dither, and have no stencil
  buffer.

### Screen savers

A screen saver starts after WIN.INI's `[windows]` `ScreenSaveTimeOut`
seconds with no key or mouse input, when `ScreenSaveActive=1`. The saver
is SYSTEM.INI's `[boot] SCRNSAVE.EXE`, run with `/s`. Programs can read
and change both settings through SystemParametersInfo. A `.SCR` can also
be run from the prompt by its name, for example `saver.scr /s`.

### Network play

DirectPlay games find each other through the instance's LAN room, over
both the IPX and the TCP/IP connection
([DirectPlay games on the built-in Windows](../CONFIGURATION.md#directplay-games-on-the-built-in-windows)).
DirectPlay's protocol here is rust-dos's own, so a game on a Windows 95
or 98 image in another instance doesn't see these sessions. Lobby
programs, such as the Zone's, aren't there, and WinSock (WS2_32 and
WSOCK32) has no network yet. DirectPlay 8 (DPNET.DLL), which DirectX 8
games use, has no network either: none of its connections can start, so
such a game finds no connection for network play and plays alone. TAPI32
is there as on a machine with no modem installed: lineInitialize fails
with LINEERR_NODEVICE, so a game that imports it starts, and its modem
play says no modem is there. IMM32 is there as Windows 95 and 98 have it
outside their Chinese, Japanese and Korean editions, with no input
methods: every function returns 0, but ImmGetCompositionString, which
returns -1, and ImmGetVirtualKey, which returns VK_PROCESSKEY, so a game
that turns the IME off for its window, such as Age of Empires, starts.

### Copy protection

A copy protection runs as it is and reads the disc image as it would
read the disc on Windows 98, through WNASPI32 and the CD-ROM drive. An
image that doesn't keep what the protection checks fails it as it would
on Windows 98: an ISO fails SafeDisc's check, and an image without DPM
data fails SecuROM 4.8's. [copy-protection.md](copy-protection.md) says
which image each protection needs and how to make it.

### What isn't there

- **DirectMusic's composition.** Segments, MIDI files, DLS instruments
  and bands play. Styles' patterns, chord and command tracks and the
  composer need the composition engine, which isn't there: they load and
  play nothing.
- **DirectInput's action maps** and device images (#378).
- **GLU's quadrics, NURBS and tessellator.** GLU32.DLL scales images,
  builds mipmaps, sets up matrices and gives its texts. gluSphere,
  gluNurbsCurve, gluTessVertex and the other drawing functions do
  nothing, and gluNewQuadric, gluNewNurbsRenderer and gluNewTess return
  NULL (#741).
- **OLE's dialogs, fonts and pictures.** OLEDLG.DLL and OLEPRO32.DLL
  are there, so a program linked with MFC's OLE support starts, but of
  their functions only OleTranslateColor, DllCanUnloadNow,
  DllRegisterServer and DllUnregisterServer are written. Calling another
  ends the program with a message naming it.
- **SETUPX's device installer.** The 16-bit SETUPX.DLL carries out an
  INF file's install section for the IExpress packages of Internet
  Explorer's time, such as the Windows Media Audio runtime: its files,
  registry values and INI lines. Its device installer and GenInstallEx,
  which DirectX's own setups call, aren't there.
- **Functions not written yet.** A program that calls one stops with a
  message naming it. Please
  [open an issue](https://github.com/dividebysandwich/rust-dos/issues)
  that names the game.
