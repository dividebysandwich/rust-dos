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

When a game doesn't run, the [log file](../README.md#log-file) says what
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
  Their targets, working folders and icons are the same. A shortcut to
  Notepad points to the CD's drive, as the setup finds no Notepad on C:.

### Disk space on a host folder

A hard disk that is a host folder has no size of its own, so its free
space is made up: between 1.75 and 3.75 GB through
`GetDiskFreeSpaceExA`, and at most just under 2 GB through
`GetDiskFreeSpaceA`, as Windows 95 and 98 give a real disk's space. It
drops as files are written. A setup that checks for the space it needs
finds enough. DOS's own answer for such a drive is 80 MB.

### The system folder

The built-in DLLs show as read-only files in `C:\WINDOWS\SYSTEM`, to
Windows programs and DOS programs alike, as on Windows 98. A game that
looks for a DLL there before it loads it finds the ones the built-in
Windows has. `GLIDE2X.DLL` and `GLIDE3X.DLL` are among them only with a
3dfx card (`voodoo=true`, or `machine=svga_voodoo3` or `svga_banshee`),
so a game that checks for Glide by its file sees none without one. A DLL
of the same name that a game copies to C: wins over the built-in one.

### Text and dialogs

The fonts are the VGA's ROM glyphs, scaled, with each character of the
dialog fonts as wide as in Windows 98's. Text still looks different, so
some lines wrap or are cut where Windows 98's aren't, and characters the
built-in fonts lack, such as `®`, show as `?`.

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
WSOCK32) has no network yet. TAPI32 is there as on a machine with no
modem installed: lineInitialize fails with LINEERR_NODEVICE, so a game
that imports it starts, and its modem play says no modem is there.
IMM32 is there as Windows 95 and 98 have it outside their Chinese,
Japanese and Korean editions, with no input methods: every function
returns 0 (ImmGetCompositionString -1), so a game that turns the IME off
for its window, such as Age of Empires, starts.

### Copy protection

A copy protection runs as it is and reads the disc image as it would
read the disc on Windows 98. An image that can't pass the check because
of how it was made, with its unreadable sectors read back as good ones,
fails it as on Windows 98. Dump the CD with its error-correction data
(a BIN or CHD made Redump-style).

### What isn't there

- **Software Direct3D.** Direct3D draws on the card's HAL alone: a
  Banshee or Voodoo3 as the display adapter, or a Voodoo Graphics or
  Voodoo2 add-on card in full screen. With another card a game finds no
  3D device.
- **DirectMusic's composition.** Segments, MIDI files, DLS instruments
  and bands play. Styles' patterns, chord and command tracks and the
  composer need the composition engine, which isn't there: they load and
  play nothing.
- **DirectInput's action maps** and device images (#378).
- **Functions not written yet.** A program that calls one stops with a
  message naming it. Please
  [open an issue](https://github.com/dividebysandwich/rust-dos/issues)
  that names the game.
