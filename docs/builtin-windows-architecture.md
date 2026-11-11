# The built-in Windows: architecture

rust-dos runs DOS programs without a copy of DOS: its DOS is written in
Rust and is part of the emulator. The built-in Windows does the same for
Windows 95 and 98 programs (`src/win32/`) and Windows 3.1 programs
(`src/win16/`): their system DLLs are written in Rust, on top of the
built-in DOS and the emulated machine, and a program starts from the DOS
prompt. No Windows files are involved. The target is games from about
1995 to 1999: Win32 programs that use DirectX 3 to 6 (DirectDraw,
Direct3D immediate mode, DirectSound, DirectInput), WinMM and Glide, and
the setup programs that install them.

This page describes how the subsystem is built. How to use it, and what
a player sees differ from Windows 98, is in
[builtin-windows.md](builtin-windows.md); which games run is in
[compatibility.md](compatibility.md#windows-games-on-the-built-in-windows).

The built-in Windows subsystem was designed and implemented by
[kibertoad](https://github.com/kibertoad).

## The approach

The Win32 DLLs are written in Rust on the emulated machine, as the
built-in DOS is, with a PE loader and stub DLLs that trap into the
emulator. The other ways to run Win32 programs were to emulate a Linux
that runs Wine, as Boxedwine does, or to run Wine's PE DLLs as guest
code on Rust implementations of Wine's internal system call table. The
first brings a Linux system call layer and a large userland, and Wine's
Direct3D would need the host's OpenGL or Vulkan from inside the guest,
past the emulated hardware, so save states, the web build and libretro
would each need their own answers; it is also NT's behaviour, not 9x's.
The second ties the emulator to an interface Wine changes between
releases. Wine's DLLs still run as guest code at the edges of the API
(see "Wine's DLLs as guest code").

The built-in Windows builds on what the emulator has:

- **The CPU** runs 32-bit protected mode with paging, and the dynamic
  recompiler engages on flat 32-bit code.
- **Service traps** let guest code call Rust (`is_service_trap` and
  `service_trap` in `src/exec.rs`). The DPMI host (`src/dpmi/`) was the
  model: a protected-mode environment built by Rust code, its tables in
  extended memory, its clients at CPL 3, with a save state section.
- **The DOS layer** (`DiskController`, `hostfs`, archives, CD images,
  MSCDEX) gives KERNEL32 its drives, and stays underneath as real DOS
  did under Windows 95.
- **The emulated hardware** carries the output: DirectDraw and GDI draw
  on the SVGA card's or the 3dfx card's video memory, Glide and Direct3D
  on the emulated Voodoo, DirectSound and wave audio feed the mixer, and
  MIDI goes to the MPU-401's synthesizers. Every frontend (SDL,
  libretro, web), save states, rewind, screenshots and recording work
  with it as with DOS programs.

## Starting a program

When DOS loads an EXE, `Cpu::load_executable` (`src/cpu/mod.rs`) hands
it to `win32::prepare` (`src/win32/mod.rs`). With `win32` on, a PE file
doesn't run its DOS stub: the program starts at a trap in the ROM
(`START`), which makes its process (`loader.rs`). That takes a block of
extended memory for the process's pages and tables, maps the built-in
DLLs and the program where Windows 95 has them, and goes on in
protected mode at level 3 at the program's entry point (`system.rs`).
ExitProcess goes back to real mode and ends the DOS process with INT 21h
AH=4Ch, so the shell or the program that started it goes on. An NE file
starts as a Win16 task, or, if it is InstallShield's setup launcher, as
the 32-bit engine it would have started (`launcher.rs`). Under a
virtual-8086 monitor (EMM386, or a booted Windows' DOS box) the DOS stub
runs.

The state is a `Bus` field, `win32`, kept in save states in a section of
its own (`W32 `). The parts that need no CPU or bus are workspace crates
with their own tests: `crates/pe` parses PE files, `crates/ne` NE
files, and `crates/installshield` reads installer archives.

The main files of `src/win32/`:

```text
mod.rs        the state, starting a process, the ROM's traps
loader.rs     processes and DLLs: mapping, relocations, imports, TLS
stubs.rs      the built-in DLLs' images and their function tables
ordinals.rs   tests: the tables have Windows 98 SE's ordinals
trace.rs      the API trace
memory.rs     physical pages, page tables, the arenas
heap.rs       heaps
system.rs     descriptor tables, the TSS, interrupts and exceptions
realmode.rs   real-mode code run for a protected-mode program
thread.rs     guest threads and their scheduler
object.rs     kernel objects and handles
call.rs       calls from the system into the program
task.rs       USER32 and GDI32 functions that call into the program
seh.rs        structured exceptions
registry.rs   the registry
boot.rs       the session's start: WININIT.INI and RunOnce
dll_files.rs  the built-in DLLs and NOTEPAD.EXE as files in C:\WINDOWS
screen.rs     the display adapter as the built-in Windows drives it
kernel32/ user32/ gdi32/ advapi32.rs ole32/ comctl32/ comdlg32/ shell32/
ddraw/ dsound/ dinput/ dplayx/ dmusic/ glide/ d3d8/ opengl32/ winmm/ vfw/
```

## Machine state while a Win32 program runs

Rust builds the protected-mode environment itself, as the DPMI host
does, but with paging on (`system.rs`, `memory.rs`):

- Flat code and data selectors at CPL 3 for the program, and a
  per-thread selector in FS for the TIB (`fs:[0]` exception chain,
  `fs:[18h]` self pointer, stack bounds).
- Page tables laid out like Windows 95's: the program's private arena
  from 4 MB to 2 GB, the shared arena (system DLLs, memory-mapped files)
  from 2 GB to 3 GB, the system's above. The system DLLs' images are
  where Windows 95 has them (KERNEL32 at `BFF70000h`), because games and
  copy protections check for a KERNEL32 above 2 GB to tell 9x from NT.
  Paging lets `VirtualAlloc` reserve more than the machine has, as games
  do, and commit pages on demand. Physical pages come from extended
  memory, taken from the XMS pool when the first process starts.
- An IDT whose gates lead to service traps in the ROM. Hardware
  interrupts go to their real-mode handlers (the BIOS's, DOS's, resident
  programs') as under Windows 95, through `realmode.rs`, and the timer's
  drives thread switching.
- A TSS whose I/O permission bitmap leaves every port open, as Windows
  95 leaves those no driver of its own claims. Some games read the
  joystick port or check the Sound Blaster themselves.

## Calling Rust from the program

The system DLLs are real PE images that Rust writes into guest memory:
DOS header, PE header, sections, an export directory with names and
ordinals, and a code section of stubs (`stubs.rs`):

```text
GetTickCount:  FE 3E 04 00     service trap, function 0004h
               C3              ret       (no arguments, or cdecl)
Sleep:         FE 3E 05 00
               C2 04 00        ret 4     (stdcall, one argument)
```

The trap reads the arguments off the guest stack, runs the Rust
function, sets EAX (and EDX), and execution continues with the `RET`.
`FE 3E` is a trap kind of its own next to the DOS and DPMI ones
(`FE 38` to `FE 3B`); `win32::accepts` takes it only from the stubs of a
running process, and the system's traps only from the ROM. The places
that know the trap kinds each know this one: `is_service_trap`,
`service_trap` and `win32_trap` in `src/exec.rs`, the idle detector's
service checks, the profiler's `Win32` zone and the debugger's trap
names.

A trap leaves translated code for the interpreter on every call. A
function that programs call many times a frame and that Windows has as
a few instructions can instead be that x86 code (`Conv::Code`): it goes
in the code section after the stubs and vtables, its export points at
it, and its stub jumps to it, so the recompiler translates it with the
program's code. MSVCRT's `_ftol` is Windows 98 SE's: it sets the
control word to chop, stores the integer with `FISTP` and puts the
control word back. Its Rust function stays for states saved when it was
a trap, whose memory still has that stub. `_EH_prolog`, which Visual
C++ 6 code calls at the start of a function to make its C++ exception
frame, is code too: Wine's nine instructions, which push the frame on
the caller's stack and return with ESP and EBP moved, leaving the
registers and stack as Windows' ten do. It has been code since the DLL
had it, so no state holds a trap for it, and its Rust function is
`stubs::never_a_trap`, which ends the program as a call of an unknown
function does.

Because the DLLs are real images, `LoadLibrary`, `GetProcAddress`,
`GetModuleHandle`, programs that walk export tables, data exports and
the debugger all work on them like on any DLL. retrowin32, which solves
the same problem in Rust, found that fake addresses in the import table
break exactly these; its code is Apache-2.0 and isn't used, only that
finding. DirectX objects are COM objects whose vtables are data in the
stub images, pointing at stubs (`com.rs`).

Functions are declared in Rust tables, one entry each: name, ordinal,
argument count, calling convention, handler. The ordinal is the one the
export directory gives the function, and it is Windows 98 SE's, so a
program that imports a function by its ordinal gets the one it would on
Windows 98. The tests in `src/win32/ordinals.rs` hold the tables to
lists `tools/win98_ordinals.py` made from the DLLs on the Windows 98 SE
CD (DirectX 6.1's DDRAW, DSOUND and DINPUT among them). KERNEL32 also
has, under their ordinals, functions Windows 98 exports by ordinal only:
VxDCall0 to VxDCall8 (1 to 9) and LoadLibrary16, FreeLibrary16 and
GetProcAddress16 (35 to 37), which SafeDisc's loader imports that way.
A function Windows 98 doesn't have is numbered after Windows' last
ordinal, and a function the built-in DLL doesn't have keeps no ordinal
of Windows': an import of it by ordinal binds to the missing-function
stub below, as an import by name does.

A function's id, which its stub's trap carries, is its DLL's place in
`DLLS` and its own in the DLL's table, apart from its ordinal; saved
states keep the stubs in memory, so tables are only added to at their
ends. A missing function is bound to a stub that logs `dll!name` with
its arguments and ends the program with a message saying which function
it needed, so the log of a game that fails says what to write next. The
trace of every call (`trace.rs`, like Wine's `+relay`) goes to the log
and to the debug server.

## Calling the program from Rust

Window procedures, enumeration callbacks, `timeSetEvent`, DLL entry
points and exception handlers call into the program. A Rust function
can't keep its stack frame across that: the program may switch threads,
and a save state must be possible at any instruction. So no Rust frame
lives across a guest call.

Instead the call is a frame on the program's stack, its return address
the stub of `CallReturn`, and a `Call` in the thread's list that says
what runs when it returns: `then`, a function in `THENS`, with the data
it needs (`call.rs`). The USER32 and GDI32 functions that call into the
program as they go (CreateWindowExA sends messages one after another,
DispatchMessageA calls a window procedure, hooks and enumeration
functions call callbacks) are tasks: functions written as steps, each of
which does its part and says what comes next, a call into the program
and the step that goes on when it returns, or the value the function
returns (`task.rs`). The state between steps is on the guest stack and
in Rust tables of plain data, which the save state covers.

## Threads

Guest threads are saved CPU contexts, switched on one host thread
(`thread.rs`). A thread that blocks (`WaitForSingleObject`,
`GetMessage`, `Sleep`) is switched out at once; one that runs gives way
to another that is ready after a fixed slice, switched on the timer's
interrupt, because games spin on flags that another thread sets. There
are no host threads, so the web build works, and runs stay
deterministic.

## Exceptions

A fault at CPL 3 (#GP, #PF on an uncommitted page, #DE, INT 3) reaches
Rust through the IDT. A page fault on a reserved page that should be
committed (stack growth, guard pages) is handled there. Others, and
RaiseException, become an `EXCEPTION_RECORD` and a `CONTEXT` on the
thread's stack, and the handlers on the `fs:[0]` chain are called one
after the other as Windows' dispatcher calls them, through `call.rs`,
until one continues the program or none is left; then the filter of
SetUnhandledExceptionFilter gets its say (`seh.rs`). MSVC's
`_except_handler3` and C++ exceptions run as the program's own code on
top of that.

MSVCRT also has the C++ classes and run-time type information of Visual
C++ 6's runtime (`msvcrt/cxx.rs`): `exception`, `bad_cast`,
`bad_typeid` and `__non_rtti_object` with their exported vtables,
`type_info`, and `__RTDynamicCast`, `__RTtypeid` and `__RTCastToVoid`,
which walk the program's complete object locators and class
hierarchies. The exceptions they throw are raised as
`_CxxThrowException` raises them, with the object below the caller's
return address, so the program's catch blocks take them. The type
descriptors, hierarchies and throw information of the runtime's own
classes are in the DLL's data section. `type_info::name` and
`__unDName` undecorate names with a port of Wine's `undname.c` (LGPL
2.1 or later), with the spacing and the names Microsoft's undecorator
of Visual C++ 6 gives and gives up on (`msvcrt/undname.rs`, tested
against that DLL's results in `undname_vc6.txt`).

## Files, registry and version

- Paths go to `DiskController`'s drives, so a program sees the same C:
  and D: as under the built-in DOS, CD images included (`GetDriveType`
  says CD-ROM, `GetVolumeInformation` gives the label the CD check
  wants).
- The built-in DOS has no long file names (`INT 21h` AX=71xxh returns
  7100h, in `src/interrupts/int21.rs`), but its host drives keep the
  host's names, which are the long ones Windows 95 shows, and KERNEL32
  uses them (`kernel32/path.rs`). Names on disk images and CDs are 8.3
  alone.
- The registry is a tree in Rust (`registry.rs`), the system's, shared
  by the processes and kept in save states. It is seeded with the keys
  games read (`HKLM\Software\Microsoft\Windows\CurrentVersion` with
  VersionNumber and a ProductId, DirectX's version, the display
  settings, the user and computer names) and kept on drive C: as
  `C:\REGISTRY.REG`, a REGEDIT4 file. A program's first registry call
  reads the seed with the file's keys and values over it; changes are
  written back when a key is closed or flushed and when the last process
  ends. Without a writable C: the registry lasts until then. A game's
  profile brings its keys as a .reg file, which the same import puts in
  (`[-key]` and `"name"=-` delete).
- What a setup program leaves for the next boot runs at the start of a
  session, which is the built-in Windows' boot (`boot.rs`): first the
  `[rename]` section of `C:\WINDOWS\WININIT.INI`, whose `NUL=file` lines
  delete files and `new=old` lines put one file in another's place,
  after which the file is renamed to `WININIT.BAK`, as WININIT.EXE does;
  then the commands of HKLM's RunServicesOnce, HKLM's RunOnce and HKCU's
  RunOnce, each value deleted as its command starts. The commands run
  one at a time, each to the end of what it started, and the program DOS
  started comes after them, with its exit code for DOS. A restart
  (`ExitWindowsEx` with `EWX_REBOOT`) boots as soon as the session's
  last process has ended, and the DOS prompt comes back after the
  commands. Both kinds of work are files on drive C:, so work left when
  the emulator was closed runs at the next session.
- A hard disk that is a host folder has no size of its own, so its free
  space is made up: between 1.75 and 3.75 GB through
  `GetDiskFreeSpaceExA` (`kernel32/win98.rs`), and at most just under 2
  GB through `GetDiskFreeSpaceA` (`kernel32/disk.rs`), as Windows 95 and
  98 give a real disk's space. It drops as files are written. DOS's own
  answer for such a drive is 160 to 480 MB, too little for some setups
  that check the space they need.
- `GetVersion` and `GetVersionEx` say Windows 95 (4.0.950, platform 1),
  or with `win32_version=98` Windows 98 (4.10.1998) and with
  `win32_version=98se` Windows 98 SE (4.10.2222, " A "), as do the
  registry's version values and MSVCRT's `_osver`. Under 98 the built-in
  DLLs also export the functions Windows 98 added that games use
  (`version.rs` lists them: USER32's monitor functions, `AnimateWindow`,
  `GetCursorInfo`, KERNEL32's `IsDebuggerPresent`,
  `InterlockedCompareExchange`, `GetFileAttributesExA`,
  `GetLongPathNameA` and others); under 95 a program that imports one
  doesn't load, as on Windows 95, and `GetProcAddress` doesn't find it.
  The setting is read when the first process starts, and kept in save
  states.
- The built-in DLLs are files in the system directory,
  `C:\WINDOWS\SYSTEM`, as Windows has its DLLs there: programs and
  setups that look for a DLL by its file (`CreateFile` or `fopen` for
  reading, `OpenFile` with `OF_EXIST`, `GetFileAttributes`,
  `FindFirstFile`) find the ones `LoadLibrary` loads (`dll_files.rs`).
  Each file is the DLL's PE image: its headers, sections and export
  directory as it is mapped in a process, with the functions the version
  of Windows has, and the version resource `GetFileVersionInfo` gives
  for it. They are there while the built-in Windows is on, and follow
  the loader's rules: `GLIDE2X.DLL` and `GLIDE3X.DLL` with a 3dfx card
  alone (`voodoo=true`, or a Banshee or Voodoo3 as the display adapter),
  `MSIMG32.DLL` from Windows 98 on, and a DLL that `win32_dlls_replace`
  replaces with one of the folder's not at all.

  The files are read-only: they open for reading alone, show as
  read-only (FILE_ATTRIBUTE_READONLY and ARCHIVE), and opening one for
  writing, making, deleting or renaming one, copying a file over it or
  changing its attributes fails with ERROR_ACCESS_DENIED, as for a
  read-only file. A file of the same name on C: wins: a game that copies
  its own DLL there has it. `DiskController` shows them
  (`src/disk/system_files.rs`), under the drive's own files, so DOS sees
  them too: `DIR C:\WINDOWS\SYSTEM` and a DOS program's INT 21h list and
  open them, as in a DOS box of Windows 98. A save state opens a file
  that was open again by its path, and once loaded the files are of the
  version of Windows its programs run under. While C: has no
  `WINDOWS\SYSTEM` folder, it and `WINDOWS` show as directories with
  just the DLLs in them, and are made on C: when a program makes a file
  or directory in them; neither can be removed while it holds them.

  The Windows directory has `NOTEPAD.EXE` the same way, which setups run
  to show a readme (`user32/notepad.rs`). It is a PE image the emulator
  builds, a Windows program of its own whose code is a message loop:
  NotepadMain and NotepadWndProc, functions of USER32 that USER32
  doesn't export, read the file the command line names and make the
  window, a multi-line edit control with the file's text. Started by
  WinExec or CreateProcessA, it is a process of its own and stays open
  after the program that started it ends, as on Windows 98.
- The CD-ROM drive takes SCSI commands through WNASPI32.DLL, ASPI for
  Win32, as Windows 98 SE's does (`wnaspi32.rs`). Copy protections read
  their discs that way: SafeDisc 2 loads it to check the CD. On Windows
  98 the DLL hands each request block to APIX.VXD, and the IOS and the
  IDE port driver send it to the drive as an ATAPI packet. Here the
  drive is the ATAPI drive a booted Windows finds on the IDE channel
  (`ide::Atapi`), of its own, since the built-in Windows has no IDE
  channel: `Atapi::run_packet` runs each packet through its task file
  and data port at once, and its delays pass at once. So the disc
  answers as it does to a booted Windows 98, with the same data and the
  same sense data, MEDIUM ERROR for a sector whose EDC doesn't match.
  ASPI shows one host adapter. Its target 0 is the CD-ROM drive a booted
  system's IDE channel would show: the one it shows stays while it has a
  disc, and it moves to a disc in another CD-ROM drive only when its own
  is empty (1 when MOUNT's `-ide` puts it on a slave). SC_EXEC_SCSI_CMD
  and SC_RESET_DEV run on the drive at once. Without an event,
  SendASPI32Command returns SS_PENDING with the final status already in
  the block and the posting routine called, as when APIX's thread runs a
  command at once. With an event, it returns SS_PENDING with the block
  still empty, and the scheduler finishes the block and sets the event
  afterwards, as APIX's thread does. The registry has the drive's device
  keys (HKLM\Enum and Config Manager's nodes in HKEY_DYN_DATA), which
  SafeDisc 2 reads to find the drive letter of an ASPI target.

## Video

All output goes through the emulated video hardware (`screen.rs`):

- **GDI and windows**: one desktop in a mode of the display adapter:
  640x480 at 32 bits a pixel on a VESA card, and at 16 on a Banshee or
  Voodoo3, which use the card's own modes, set through its registers as
  3dfx's driver set them. Windows are drawn into it. The fonts are
  raster fonts drawn from the VGA's ROM glyphs, scaled, with the dialog
  faces' character widths taken from Windows 98's own raster fonts
  (`gdi32/font.rs`). With `win32_depth=8` the desktop has 8 bits a
  pixel, and GDI keeps a system palette in the DAC as Windows does on a
  256-colour display: palettes realized in the foreground or the
  background, `PC_RESERVED` entries for `AnimatePalette`,
  `SetSystemPaletteUse`, and `WM_PALETTECHANGED` and
  `WM_QUERYNEWPALETTE` for the windows (`gdi32/palette.rs`).
- **DirectDraw** (`ddraw/`) in exclusive full-screen mode sets the mode
  the game asks for. Surfaces in video memory are in the card's memory,
  which each process that uses DirectDraw has mapped whole, so `Lock`
  returns a pointer into it and the game's own pixel loops write
  straight to the card; surfaces in system memory are in the process's
  pages. `Flip` moves the display start, and palettes go to the DAC. In
  a window the primary surface is the desktop. A Voodoo Graphics or
  Voodoo2 add-on card is a second DirectDraw device (`ddraw/voodoo.rs`),
  as 3dfx's DirectX drivers made it, whose primary surface, back buffer
  and Z buffer are the card's buffers.
- **Glide 2.x** (`glide2x.dll`, `glide/`) is written in Rust against the
  emulated Voodoo's registers, as 3dfx's library was against the card.
  The Voodoo emulation (`src/voodoo/`) does the rendering, and 3dfx's
  MiniGL drivers, which games of the time shipped, run on top of it.
  `grSstControl` moves the monitor between the card and the 2D picture
  while the display stays open, as a game with DirectDraw menus and
  Glide races does between the two: a Voodoo Graphics or Voodoo2 turns
  its pass-through, and a Banshee or Voodoo3 gives the display back and
  later takes it again, with the textures where they were. **Glide 3**
  (`glide3x.dll`), which 3dfx's OpenGL drivers for the Voodoo2, Banshee
  and Voodoo3 use, is the same code with Glide 3's functions: vertices
  in the program's own layout, in arrays, strips and fans and in clip
  coordinates, log2 texture sizes, and `grGet` and `grQueryResolutions`
  in place of `grSstQueryHardware`.
- **Direct3D** (IDirect3D to IDirect3D7, `ddraw/d3d/`) has one device, the
  card's HAL, as 3dfx's Direct3D driver was: on a Banshee or Voodoo3 as the
  display adapter, so DirectDraw, GDI and Direct3D share one frame buffer
  and Direct3D in a window works, and on a Voodoo Graphics or Voodoo2 add-on
  card in full screen. Transform, lighting, clipping and fog are done on the
  processor (`pipeline.rs`), as Windows' Direct3D did them for the card, and
  the HAL draws the screen vertices through the Glide layer's triangle setup
  (`hal/voodoo.rs`). Direct3D has no rasterizer of its own: it would be a
  second 3D pipeline to keep correct, and the Voodoo emulation already
  replays its triangles in OpenGL. The card's limits (16-bit colour and
  depth, 256x256 textures) are the ones games of the time were written for.
  On a ToadForce (`docs/toadforce.md`) as the display adapter the same
  device draws through that card's HAL (`hal/toadforce.rs`), which sends the
  card its level 1 commands: 32-bit targets, 24-bit depth with stencil,
  textures up to 2048 pixels a side, compressed and bump formats.
  `hal/mod.rs` is the interface both are behind. D3D8.DLL (`d3d8/`) is
  DirectX 8.1's Direct3D on a Banshee or Voodoo3, and OPENGL32.DLL
  (`opengl32/`) is OpenGL 1.1 there and on a Voodoo2 in full screen.

Painting follows Windows 98 SE's messages, compared by probes there.
The differences that remain are listed in
[builtin-windows.md](builtin-windows.md#painting). What a change on the
screen uncovers (`invalidate_screen` in `user32/paint.rs`) stays cropped
to the window's shape, so a window gets no WM_PAINT or WM_NCPAINT for
what its region cuts off. No probe has compared that crop with Windows.

### DXT surfaces

The card has no compressed textures, so DXT1 to DXT5 surfaces are in
system memory, as DirectX 7 makes them on Windows 98 SE with a card
without them. A program decodes them into a 16-bit texture with `Blt`
or `BltFast`, as The Operative: No One Lives Forever does. A probe
program run on Windows 98 SE with DirectX 7.0a, and on Windows 11, gave
what `ddraw/dxt_blt.rs` and `video/dxt.rs` do:

- A DXT surface's width and height are multiples of 4, or creating it
  fails with DDERR_INVALIDPARAMS. Mipmap levels below 4 pixels are one
  block each, and `Lock` gives the size of the level as its pitch.
- The decoder follows S3TC, with DirectDraw's differences: DXT5's alpha
  values between the two ends run from the second to the first, DXT2
  to DXT5 have three colours and black when the first colour isn't
  greater, as DXT1 does, and DXT2's and DXT4's colours are clamped to
  their alpha.
- A blit to a 16, 24 or 32-bit RGB surface keeps the top bits of each
  channel, and a 32-bit destination gets the alpha in its top byte
  even without alpha bits. A stretch takes the source pixel at the
  middle of each destination pixel, in 16.16 fixed point.
- A blit to an 8-bit palette surface succeeds and draws nothing, a fill
  of a DXT surface fails with E_NOTIMPL, and so do a source colour key
  and DXT2 or DXT4 to a format with alpha.
- Windows compresses what is blitted to a DXT surface. Here only blocks
  of the same format are copied, between rectangles on block
  boundaries; other blits to a DXT surface fail with E_NOTIMPL.

## Movies

AVIFIL32.DLL reads AVI files, and MSVFW32.DLL's DrawDib functions
decompress and draw their frames (`vfw/`). AVIStreamGetFrame decompresses
a video stream's frames to a DIB of the program's choice, 24 bits a pixel
by default, from the last key frame on. The codecs are four of those
Windows 98 installs: Indeo Video 3 and 5, decoded by the `oxideav-indeo`
crate, Indeo Video Interactive 4, and Cinepak. Indeo 5 key frames match
FFmpeg's, but for frames that both decoders refuse as damaged, where the
picture before stays. Indeo 5's frames that depend on earlier ones have
not been compared with a reference. Cinepak's pictures match FFmpeg's
exactly, every tenth frame of Wages of War's two Cinepak movies.
ICInfo lists the four as installed `vidc` codecs, under the drivers
Windows 98 SE's SYSTEM.INI names, so a program that asks whether Indeo
4 is there before it plays a movie, as Age of Empires does, finds it.
ICOpen opens them to decompress, and ICClose closes the handle; a mode
that compresses or draws finds no codec. Rent-A-Hero opens Indeo 5
before it starts, to see that it is there.

## Sound and input

- **DirectSound and `waveOut`**: buffers stay in guest memory; a device
  reads them at the emulated rate and mixes them into the host's sound,
  through the host mixer like the Sound Blaster. Play cursors and
  notifications come from emulated time (`dsound/`, `winmm/`).
- **`midiOut` and MCI**: MIDI goes to the MPU-401's synthesizers
  (FluidSynth, MT-32, SC-55); MCI CD audio to the CD audio player.
- **DirectMusic** (DirectX 6.1, `dmusic/`): DMUSIC, DMIME, DMLOADER,
  DMSTYLE, DMBAND and DMCOMPOS, their classes made through OLE32. The
  loader reads segments (.sgt), MIDI files, DLS collections, bands,
  styles and chord maps from files, memory or OLE32's streams on global
  memory and caches them, and the objects load themselves from such
  streams (IPersistStream::Load); a program's own IStream isn't read. A
  performance plays segments' sequence, tempo, time signature, system
  exclusive and band tracks by the emulated clock on the process's
  multimedia thread, with notifications. Both ports it lists (the
  Microsoft Synthesizer and the MIDI mapper) are the MPU-401's
  synthesizer, which plays downloaded DLS instruments by their General
  MIDI patches. Styles' patterns, chord and command tracks and the
  composer need the composition engine, which isn't there: they load
  and play nothing.
- **Input**: from the emulated 8042 keyboard, the PS/2 mouse and the
  game port, so the frontends, the debug server's input injection and
  key remapping need no changes. DirectInput (`dinput/`) and window
  messages both read from them.

## Network play

DirectPlay (`dplayx/`) has IDirectPlay of DirectX 3 to IDirectPlay4A,
with the IPX and TCP/IP service providers. Both carry the session over
the instance's LAN room: DirectPlay's packets are IPX packets on a socket
of the built-in DOS's IPX driver, which the switch and the relay pass on
as they do a DOS game's. The host of a session answers the enumeration
requests every endpoint broadcasts, so every member of a room sees every
session hosted in it, and takes in the members, who then talk to each
other directly. Messages that must arrive (guaranteed ones, and every
change to the session) go over a link that sends them again until they
are acknowledged, in order; a member silent for 20 seconds is taken for
gone. The protocol is rust-dos's own. What the program sees follows
Windows 98 as Wine's DirectPlay tests recorded it: which players get
which system messages, in which order, and the sizes of the messages.
DPLAY.DLL, DirectX 3's, which some games import by ordinal, makes and
lists DPLAYX's objects and providers, as Wine's does. WS2_32 and WSOCK32
have no network yet.

## Setup programs

A game on CD is installed in one of three ways, which the player page
lists: its setup runs under the built-in Windows, its installer's
archives are unpacked in Rust, or an installed copy is used with its
registry keys from the profile's `.reg` file.

### InstallShield's launcher

InstallShield's 16-bit `SETUP.EXE` (InstallShield 5, and InstallShield 3
releases that ship the 32-bit engine) only unpacks its engine and starts
it. An NE `SETUP.EXE` with `_INST32I.EX_` beside it is taken for that
launcher (`launcher.rs`): the files of the archive (a list of PKWARE DCL
members, read by the installshield crate's `EngineArchive`) go to
`TEMP`, all but the 16-bit `BOOT16.EXE`, and the engine starts with the
command line Windows 98 SE's launcher gives it, as a probe program read
it from the running engine there:

```
C:\WINDOWS\TEMP\_INS0432._MP -fD:\SETUP.INS  -z1 -cx -xC:\WINDOWS\TEMP\
```

That is launcher 5.00.149's. Later ones (5.00.212 and 5.00.221) add the
setup's folder and its language, the `Default=` of `SETUP.LID`'s
`[Languages]`; one of them gave this, found in the memory of Windows 98
SE running it:

```
-fD:\SETUP.INS  -z1 -cx -xC:\WINDOWS\TEMP\ -x1D:\ -q10009
```

Their engine looks for the setup's `layout.bin` and cabinets in the
folder `-x1` names, or without it in `TEMP`, and ends at once when they
aren't there. Which arguments a launcher gives is in its table of them,
strings ending in a zero, so the built-in Windows gives `-x1` and `-q1`
when that table has them. The launcher's `_ISDEL.EXE`, which deletes
those files after setup, is 16-bit and isn't run, so they stay in
`TEMP`.

### What setups need

These behaviours exist for setup engines, each as Windows 98 SE does it,
checked with a probe program there:

- A write to a module's resources, which are read-only pages, succeeds:
  Windows 98 makes the page writable and the write goes through, though
  VirtualQuery says read-only and IsBadWritePtr says the page can't be
  written (InstallShield's engine draws into its own bitmap resource).
- WINSPOOL.DRV with EnumPortsA (FILE:, LPT1:, COM1:, COM2:).
- ADVAPI32's security descriptor functions return TRUE with error 120
  (InitializeSecurityDescriptor, SetSecurityDescriptorDacl and
  SetSecurityDescriptorOwner) or FALSE with it (InitializeAcl,
  AllocateAndInitializeSid and the token functions). InstallShield's
  engine writes no registry key when InitializeSecurityDescriptor fails.
- MPR.DLL with WNetGetConnectionA, which finds no drive redirected.
- GetTempPathA makes the folder it names when it isn't there, as a
  game's C: drive has no `C:\WINDOWS\TEMP` until something makes it.
- GDI32 keeps metafiles (SetMetaFileBitsEx) and plays them
  (PlayMetaFile): Wise's dialogs draw their pictures from them.

What an install by these setups differs in from one on Windows 98 is in
[builtin-windows.md](builtin-windows.md#how-an-install-differs).

### Unpacking in Rust

`crates/installshield` (`rust-dos-installshield`, pure Rust without
unsafe code, deflate from flate2) reads installer archives without
running the setup. Its readers follow those of the refurbished-dinosaurs
toolkit (`RefurbishedDinosaurs.LegacyFormats`, MIT), which are C#, so
they are ported rather than reused:

- InstallShield 5 and 6 cabinets (`data1.hdr` and `dataN.cab`, major
  versions 0, 5 and 6), laid out as Unshield reads them. Compressed
  members are deflate chunks with 16-bit lengths or, in some version 0
  sets, raw deflate ending each chunk with `00 00 FF FF` (Unshield's
  `-O`). Nothing in the header tells the two apart, so the profile names
  the form and the reader never guesses it. Version 6 members are
  checked against the header's MD5, the others by size. A major version
  7 header is refused.
- InstallShield 3 archives (`_SETUP.1`, `.Z` files inside
  self-extractors), whose members are compressed with PKWARE's DCL
  implode, decoded as zlib's `contrib/blast` describes the format.
- The header is checked whole before any member is read: relative paths,
  data inside the volumes, and limits on counts and sizes. Entries the
  reader can't read (marked invalid, stored outside the cabinet, two
  different files at one path) are listed with the reason, and the rest
  of the set still opens; a file at a shared path is read by its index.
- A cabinet places files by file group. The reader reports each member's
  groups, and the profile maps groups to folders under the install
  folder, recorded from a real install as its registry keys are.

`rust-dos unpack` (`src/unpack.rs`) lists or extracts a set or an archive
on the host, by file group if asked, and a game profile's `[install]`
section unpacks one from the game's drives onto them the first time the
game starts (`src/install.rs`), with the profile's `windows=builtin` and
`games/<id>.reg` doing the rest of what setup would.

## Windows 3.1 programs

Win16 programs (NE files) run as Windows 95 runs them (`src/win16/`).
Their tasks are threads of one Win32 process, the Win16 process
(KRNL386.EXE), which they share: its address space, its LDT, where their
segments' selectors are, and the global heap. The system's 16-bit
modules (KERNEL, USER, GDI and the others) are NE images the emulator
builds, each export a stub that traps into Rust, and most of them go on
to the 32-bit function that does the work (`thunk.rs`). Tasks take turns
as Windows 3.1's do, while the Win32 processes' threads go on around
them.

## Timing

`GetTickCount`, `timeGetTime`, `QueryPerformanceCounter` (at the PIT's
1.193182 MHz, as Windows 95 reported) and RDTSC all come from the
emulated clock, so a game's frame pacing follows `cycles` and is
deterministic.

## Wine's DLLs as guest code

Wine's PE builds of DLLs at the edges of the API can run as guest code
on top of the Rust core. The core (KERNEL32, USER32, GDI32, DirectX,
WinMM) stays in Rust: Wine's versions of those sit on its NT kernel
interface and on wined3d.

The `win32_dlls` setting names a host folder, such as Wine's
`i386-windows`, whose DLLs a program may load (CONFIGURATION.md). The
loader (`loader.rs`) looks there for a DLL that has no built-in DLL and
isn't on the search path, and reads it as if it were in
`C:\WINDOWS\SYSTEM`; `win32_dlls_replace` lets the folder's DLL replace
a built-in one of the same name. With the setting, `ucrtbase.dll` and
`api-ms-win-crt-*.dll` are the built-in MSVCRT, `kernelbase.dll` is
KERNEL32. The built-in NTDLL.DLL, at BFE40000h, is there with or
without the setting, as Windows 95 and 98 both have one with
`RtlUnwind` (Microsoft's D3DRM.DLL and DirectX 6.1a's ADVPACK.DLL
import it); it has `_vsnprintf` for Wine's DLLs too.

A DLL from the folder is guest code (the module flag `GUEST`). A
function it imports that no built-in DLL has gets the stub that ends a
program, but called from guest code the stub logs the name and fails the
call instead: it returns 0 with ERROR_CALL_NOT_IMPLEMENTED. For the stub
to pop a stdcall function's arguments, the loader reads their size from
the decorated names (`_Name@8`) in the folder's import libraries
(`lib<dll>.a`), which Wine installs beside its DLLs.

Wine's DLLs are built for the i686 and use CMOV, so they need
`cpu=pentium_pro` or a later processor. They inline GetProcessHeap as a
read of `fs:[30h]` and the field at 18h there. That is the process
database on Windows 95 and the PEB on Windows NT, and on both the field
is the process heap, so each TIB points at a block of the process's that
has it.

Wine 11.0's `msacm32.dll` with its IMA ADPCM codec, `imaadp32.acm`,
encodes 16-bit PCM to IMA ADPCM and decodes it back through
`acmStreamOpen` and `acmStreamConvert`, and the samples match a decoder
written from the format's definition (`tests/win32/wine_dlls.rs`, which
runs when WINE_DLLS names the folder). It takes
`win32_dlls_replace=msacm32`: the built-in MSACM32.DLL has Windows'
exports and only the decoders of MS ADPCM and IMA ADPCM. MSACM32 finds
the codec in SYSTEM.INI's `[drivers32]` (`msacm.imaadpcm=imaadp32.acm`)
and opens it with OpenDriver. WINMM's installable drivers are real for
that: OpenDriver loads the DLL as LoadLibrary does, finds its alias in
SYSTEM.INI, and sends its `DriverProc` DRV_LOAD, DRV_ENABLE and
DRV_OPEN; SendDriverMessage and CloseDriver call it as Wine's `winmm`
does. ADVAPI32's `Reg*W` stubs return ERROR_CALL_NOT_IMPLEMENTED to
guest code: Windows 95's return 0 with that error set, and MSACM32 takes
0 as ERROR_SUCCESS.

Wine's `comctl32`, `comdlg32` and `quartz` don't run this way: their
window classes and dialogs are written in Unicode, so they need USER32's
and GDI32's `W` functions, which the built-in DLLs have as stubs that
fail, as Windows 95's are, and more of NTDLL and KERNEL32. The built-in
COMCTL32 and COMDLG32 are written in Rust instead, partly ported from
Wine's.

## Testing

- Unit tests per DLL area, with generated PE programs, run on both cores
  as the rest of the suite is (`RUST_DOS_CORE=dynamic`).
- Win32 games in the game suite (`docs/game-suite.md`), with golden
  screen hashes. Determinism makes the hashes stable.
- Comparisons with the same program on a booted Windows 98 SE image,
  using the debug server to compare screens and the API trace to find
  where they part.
- Wine's conformance tests (`dlls/*/tests/*.c`), in `tests/conformance/`.
  They are Win32 programs that check the API's behaviour against what
  real Windows returned. These rules apply:
  - They are kept in the repository, copied from one pinned Wine tree,
    so CI fetches nothing and they change only in a commit. Any patch
    one needs is listed in the folder's README, which also lists the
    files.
  - The tree is Wine 1.2.3. Wine tested on Windows 9x until about 2010,
    and its tests of that time accept 9x's results (`broken()`,
    `win_skip`); later trees expect NT's in places. DirectMusic's tests
    are the exception: 1.2.3 has only dmloader's `loader.c`, dmime's
    `performance.c` came in 1.4.1 and dmusic's `dmusic.c` in 1.6.2. They
    are copied from the release that first has them, and each check that
    fails is compared with the booted Windows 98 image before it is
    fixed or its result is written into `expected.txt`.
  - A test file is copied once the built-in DLL has the functions it
    calls directly, so the set grows with the DLLs.
  - They run as Windows 98 SE (`win32_version=98se`), so that the
    functions Windows 98 added are there for the tests that use them.
  - `tests/win32/conformance_tests.rs` builds them with MinGW and runs
    each under the built-in Windows: all but msvcrt's as 486 code linked
    with a small C runtime of rust-dos's own on KERNEL32, so they test
    the built-in DLLs alone; msvcrt's linked with the built-in
    MSVCRT.DLL, on a Pentium Pro. No binaries are checked in.
  - Their results (each file's summary, the places of the checks that
    fail and the tests skipped, or how a program ended early) must be
    those in `expected.txt`. Any difference, better or worse, fails the
    test, so every change to them shows in that file's diff.
  - CI runs them in a workflow of their own with Ubuntu 22.04's MinGW,
    the one the expected results were made with: on changes to the
    built-in Windows, the PE crate, the processor and recompiler, DOS's
    file writes or the tests, and every night. Without MinGW the test
    passes with a note, so `cargo test` works on any machine.
- Wine's tests ported to Windows 3.1 programs, for Win16, in
  `tests/conformance16/` (its README lists them), each cut from the
  Win32 test of the same functions in the 1.2.3 tree to what Windows 3.1
  has, and a test of the DPMI services written here.
  `tests/win16/win16_conformance_tests.rs` builds them with OpenWatcom
  and checks their results against `expected.txt` as the Win32 tests
  are checked. CI runs them in the conformance workflow with a pinned
  OpenWatcom snapshot.
- The same test programs on the booted Windows 95 and 98 images give
  the expected output where Wine's tests don't say what 9x does: a test
  passes when the built-in layer prints what Windows printed. Wine's
  1.2.3 tests accept 95's, 98's and ME's answers alike, so they show
  that the Windows 98 setting breaks nothing, but not where 98 should
  answer differently from 95. Those differences come from the booted
  images.
- Installer archives: unit tests build InstallShield cabinets,
  InstallShield 3 archives and implode streams with a writer in the
  tests, as the PE tests build programs. A writer and a reader made from
  one reading of a format only show that the two agree, so the tests
  also decode streams that an independent decoder expanded, as the
  toolkit's tests do: the example in blast's format description, and a
  coded-literal stream pwexplode expanded to the same text. A game's own
  archive, compared entry by entry with what Unshield (or idecomp, for
  InstallShield 3) extracts, is the acceptance check. It runs outside
  the repository, as the game suite does. It covered these, all files
  equal by SHA-1 (the games are in
  [compatibility.md](compatibility.md#installer-archives)):
  - Version 5 sets with `.hdr` files (data1, _sys1, _user1), among them
    paths that hold one file per language and are read by group.
  - Version 5 sets with the header in `data1.cab`.
  - Version 0 sets, which need the marker-delimited form; the chunked
    one fails on them with a message that names the other.
  - InstallShield 3 `_SETUP.1` archives and `.Z` archives, paths equal
    too.

  No version 6 set was found on the media at hand, so version 6 is
  tested by the writer's sets alone.
- Installs: a real installer, run under the built-in Windows or unpacked
  in Rust, from media kept outside the repository as the game suite's
  are. The check is what the install leaves behind, the files written
  with their hashes and the registry keys set, compared with an install
  of the same game on the booted Windows 98 image.

## Licensing

rust-dos is GPL-2.0-or-later.

- Wine's code (LGPL-2.1+) is ported, with its copyright notices kept in
  the files it went into (Direct3D, D3D8, COMCTL32, COMDLG32 and
  others); LGPL-2.1 section 3 allows a copy under GPL-2.0-or-later. The
  conformance tests are kept under the LGPL with their notices.
- The refurbished-dinosaurs toolkit's format readers (MIT) and Unshield
  (MIT) are ported into `crates/installshield`. Code from idecomp or
  pwexplode (GPL-3.0) would make the whole program GPL-3.0, which isn't
  ours to decide, so their code is not used. The readers implement the
  same formats from descriptions of them (the toolkit's decision
  records, blast's format description), and the two tools are run as
  programs outside the repository: for research into what an archive
  holds, and to check what the readers extract.
- retrowin32 (Apache-2.0) is incompatible with GPL-2.0, and Boxedwine
  (GPL-2.0-only) would make the whole program GPL-2.0-only: neither's
  code is used. HX DOS Extender has no license file, so it serves as a
  reference and a test, not a source.
- Nothing from Microsoft is used or shipped: no DLLs, fonts or headers
  beyond what Wine's and MinGW's public headers document.

## Rules the code follows

1. **Output goes through the emulated hardware.** No DLL draws or plays
   on the host directly, so frontends, save states and recording need
   nothing of their own for Windows programs.
2. **Direct3D runs on the emulated card's HAL**, whichever card it is
   (see "Video"), without a rasterizer of its own.
3. **No Rust frame lives across a call into the program** (see "Calling
   the program from Rust"), so a save state is possible at any
   instruction.
4. **Behaviour follows Windows 98 SE**, as probes on a booted image and
   Wine's tests recorded it, under `win32_version=98se`, and Windows 95's
   under the default.
5. **Copy protections run unchanged.** The built-in Windows provides
   what Windows 98 and the machine provide (KERNEL32's ordinal-only
   exports, VxDCall, SCSI commands through WNASPI32), and a protection
   reads the disc image as on Windows 98. A protection is never patched,
   skipped or detected by name. An image that can't pass the check
   because of how it was made fails it, as on Windows 98.
6. **Installs come from the game's own installer** where it runs, and
   from unpacking its archives or an installed copy with registry keys
   from the profile where it doesn't (see "Setup programs").
7. **Wine's DLLs run only at the edges**, as guest code from a folder
   the user names, never in place of the core DLLs.
