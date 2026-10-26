# Built-in Windows 95: a plan

rust-dos runs DOS programs without a copy of DOS: its DOS is written in
Rust and is part of the emulator. Windows 95 and 98 games run today only
by booting a real Windows image (see `win9x-howto.md`), which the user has
to own, install and configure: a VHD, the right drivers for the emulated
video card, and a per-game delta. This document plans a built-in Win32
layer that runs Windows 95 games the way the built-in DOS runs DOS games:
type the game's name at the prompt, or start it from a game profile, and
it runs. No Windows files are involved.

The scope is games from about 1995 to 1999: Win32 programs that use
DirectX 3 to 6 (DirectDraw, Direct3D immediate mode, DirectSound,
DirectInput), WinMM and Glide. Windows 98 comes after Windows 95; the API
the games of the two use is nearly the same.

The built-in Windows subsystem was designed and implemented by
[kibertoad](https://github.com/kibertoad).

## Prior art

| Project | License | Approach | What it gives us |
|---|---|---|---|
| [Boxedwine](https://github.com/danoon2/Boxedwine) | GPL-2.0-only | x86 emulator plus Linux syscall emulation, running stock 32-bit Wine on Tiny Core Linux | Proof that the emulate-and-run-Wine path works for late-90s games; its x86 tests |
| [Wine](https://gitlab.winehq.org/wine/wine) | LGPL-2.1+ | Win32 reimplemented on a Unix host; since Wine 8 every DLL builds as PE and reaches Unix through an internal syscall table | API semantics, and code we may port (LGPL-2.1 section 3 allows relicensing a copy to GPL-2.0-or-later) |
| [ReactOS](https://github.com/reactos/reactos) | GPL-2.0 | An NT 5.2 operating system, user mode mostly from Wine | Nothing beyond Wine for us; it is NT, not 9x |
| [retrowin32](https://github.com/evmar/retrowin32) | Apache-2.0 | x86 interpreter plus Win32 DLLs written in Rust | The closest design to this plan (see below); ideas only, since Apache-2.0 code can't go into a GPL-2.0 work |
| [HX DOS Extender](https://github.com/Baron-von-Riedesel/HX) | Freeware, no license file | Win32 PE programs on DOS plus a DPMI host; DLLs written in assembly, including a software DirectDraw and DirectSound, no Direct3D | Proof that a Win32 layer on top of DOS runs games (Descent, Quake 2); its documented behaviour; it already runs on rust-dos's DPMI host |
| [winevdm](https://github.com/otya128/winevdm) | GPL-2.0 | Win16 on Win64 with MAME's CPU and Wine's 16-bit DLLs | Scope of a Win16 layer, for 16-bit setup programs |
| [cnc-ddraw](https://github.com/FunkyFr3sh/cnc-ddraw) | MIT | DirectDraw reimplemented on GDI, OpenGL or D3D9 | DirectDraw semantics for 2D games, in code we may reuse |
| [DDrawCompat](https://github.com/narzoul/DDrawCompat) | 0BSD | DirectDraw and D3D 1-7 fixes on modern Windows | The same, for the parts games depend on that the docs don't say |
| [RetroTick](https://github.com/lqs/retrotick) | CC0 | Browser emulator with reimplemented Win16/Win32 APIs | Small, but reusable without conditions |
| [Cxbx-Reloaded](https://github.com/Cxbx-Reloaded/Cxbx-Reloaded) | GPL-2.0 | Xbox kernel and Direct3D 8 reimplemented | Precedent for HLE of one fixed DirectX version |

"RetroOS": no Win32-compatible system by that name turned up. The nearest
hit, [RetrOS-32](https://github.com/joexbayer/RetrOS-32), is a hobby i386
OS with its own GUI and no PE or Win32 support.

What retrowin32 learned matters most, since it is the same problem in the
same language:

- It first called Rust from fake addresses in the import table. That broke
  `LoadLibrary` (programs read the returned module's headers), data
  exports (vtables, CRT variables) and the debugger. It now builds real PE
  DLLs whose exports are small stubs that trap into the emulator.
- Calls from Rust back into the program (window procedures, enumeration
  callbacks, timer callbacks) are its hardest part. It uses Rust `async`
  for them, which its author describes as possibly not fully correct.
- Guest threads are CPU contexts switched on one host thread.
- Its DirectDraw keeps surfaces in guest memory and converts them for the
  host after `Unlock`.

## Three ways to do it

1. **Emulate an OS that runs Wine** (Boxedwine). It is the most
   compatible, and for rust-dos the most foreign: a Linux system call layer
   with `mmap`, signals and threads, a 100 MB or larger Linux and Wine
   userland, and Wine's Direct3D needing host OpenGL or Vulkan from inside
   the guest. None of it would use the emulated hardware, so save states,
   the web build and libretro would each need their own answers. It is
   also NT behaviour, not 9x.
2. **Run Wine's PE DLLs as guest code** and implement Wine's internal
   syscall table (`ntdll`, `win32u` and the `__wine_unix_call` libraries)
   in Rust. The license fits, but the interface is Wine-internal, changes
   between releases, and still leaves a display driver and wined3d (OpenGL
   or Vulkan, or 2D only) to provide.
3. **Write the Win32 DLLs in Rust** (retrowin32, HX): a PE loader, stub
   DLLs that trap into the emulator, and the APIs games call implemented
   in Rust on top of the emulated machine.

**Recommendation: 3.** It is what rust-dos already does for DOS, and the
emulator has most of what it needs:

- **The CPU** runs 32-bit protected mode with paging, and the dynamic
  recompiler engages on exactly this kind of code (flat 32-bit programs).
- **Service traps** (`FE 38..3B`, `src/exec.rs:1291`) already let guest
  code call Rust. The DPMI host (`src/dpmi/`) is a working example of a
  protected-mode environment built by Rust code: its GDT, IDT and TSS live
  in XMS memory, its clients run at CPL 3, and it has a save state section.
- **The DOS layer** (`DiskController`, `hostfs`, archives, CD images,
  MSCDEX) gives kernel32 its drives, and stays underneath as real DOS did
  under Windows 95.
- **The emulated hardware** can carry the output. DirectDraw can draw into
  the SVGA card's linear frame buffer through VBE modes; Glide can drive
  the emulated Voodoo; DirectSound can feed the mixer; MIDI can go to the
  MPU-401 and its synthesizers. Then every frontend (SDL, libretro, web),
  save states, rewind, screenshots and recording keep working unchanged.

A game uses a few hundred Win32 functions, not the whole API. Expendable
(1999, Direct3D) imports 54 functions from KERNEL32, 10 from USER32, 4
registry functions, 2 from OLE32, 2 from DDRAW and `PropertySheetA`; its
DLLs add WinMM and some of GDI32. Implementing against trace logs of real
games keeps the work bounded.

Option 2 stays open for the DLLs at the edges, as an experiment after
phase 3 (see "Wine's DLLs as guest code").

## Architecture

### Where it runs

A Win32 program starts like a DOS one. When EXEC (the shell, `INT 21h`
4Bh, or `rust-dos run`) finds an MZ file whose `e_lfanew` points at a
`PE\0\0` header, and the layer is on, `Cpu::load_executable`
(`src/cpu/mod.rs:1856`) hands it to the Win32 loader instead of running
the DOS stub ("This program requires Microsoft Windows"). The DOS program
that started it waits, as COMMAND.COM did for a DOS box, and gets control
back when the last Win32 process ends.

New code goes in `src/win32/`, a sibling of `src/dpmi/`, with its state a
`Bus` field and a save state section of its own. The parts that need no
CPU or bus, such as the PE parser, the registry and GDI's rasterizer, can
be workspace crates (`crates/pe`, ...) with their own tests.

```text
src/win32/
  mod.rs        state, starting and ending a process
  pe.rs         or crates/pe: parsing, mapping, relocations, imports
  stubs.rs      building the system DLLs' images in guest memory
  dispatch.rs   the service trap, the function tables, the API trace
  mem.rs        page tables, VirtualAlloc, heaps
  process.rs    PEB, TEB/TIB, selectors, modules, handles
  thread.rs     guest threads and their scheduler
  seh.rs        CPU faults to structured exceptions
  glue.rs       guest-side code for calls back into the program
  kernel32/ user32/ gdi32/ advapi32/ winmm/ ole32/
  ddraw/ dsound/ dinput/ glide/ d3d/
```

### Machine state while a Win32 program runs

Rust builds the protected-mode environment itself, as the DPMI host does,
but with paging on:

- Flat code and data selectors at CPL 3 for the program; a per-thread
  selector in FS for the TIB (`fs:[0]` exception chain, `fs:[18h]` self
  pointer, stack bounds).
- Page tables laid out like Windows 95's: the program's private arena from
  4 MB to 2 GB, the shared arena from 2 GB to 3 GB. The system DLLs' stub
  images go where Windows 95 had them (KERNEL32 near `BFF70000h`), because
  games and copy protection check for a KERNEL32 above 2 GB to tell 9x
  from NT. Paging also lets `VirtualAlloc` reserve more than the machine
  has, as games do, and commit pages on demand. Physical pages come from
  extended memory, as the DPMI host's tables do.
- An IDT whose exception and IRQ gates lead to service traps in the ROM.
  Hardware interrupts (PIT, keyboard, PS/2 mouse) are acknowledged there
  and feed timers, input and thread switching.
- An I/O permission bitmap that lets the program reach the ports Windows
  95 left open. Some games read the joystick port or check the Sound
  Blaster themselves.

### Calling Rust from the program

The system DLLs are real PE images that Rust writes into guest memory
when the layer starts: DOS header, PE header, sections, an export
directory with names and ordinals, and a code section of stubs:

```text
GetTickCount:   FE 3E 41 02      ; service trap, function 0241h
                C3               ; ret        (cdecl, or no arguments)
Sleep:          FE 3E 42 02
                C2 04 00         ; ret 4      (stdcall, one argument)
```

The trap reads the arguments off the guest stack, runs the Rust function,
sets EAX (and EDX), and execution continues with the `RET`. This is a
fifth trap kind next to `FE 38..3B`, accepted in protected mode only while
the layer is active and only inside the stub images' code pages. The
places that know the trap kinds (`is_service_trap`, the `allowed` match in
`service_trap`, `idle::pure_service`, the profiler's zones and the
disassembler's `trap_name`) each learn the new one. `FE 3E` and `FE 3F`
are the free encodings that need no SIB byte or displacement in 32-bit
code, so the disassembler's lengths stay simple. Dynrec blocks already end
before invalid encodings.

Because the DLLs are real images, `LoadLibrary`, `GetProcAddress`,
`GetModuleHandle`, programs that walk export tables, data exports and the
debugger all work on them like on any DLL. DirectX objects are COM
objects whose vtables are data in the stub images, pointing at stubs.

Functions are declared in Rust tables, one line each: DLL, name, ordinal,
calling convention, argument count, handler. A missing function is bound
to a stub that logs `dll!name` with its arguments and ends the program
with a message saying which function it needed, so the trace of a game
that fails says what to write next. The trace of every call (like Wine's
`+relay`) goes to the log and to the debug server.

### Calling the program from Rust

Window procedures, `EnumDisplayModes` callbacks, `timeSetEvent` and
DirectSound notifications call into the program. A Rust function can't
keep its stack frame across that: the program may switch threads, and a
save state must be possible at any instruction. So no Rust frame lives
across a guest call. The loops that call back are written as guest code
(`glue.rs`, assembled with iced's `code_asm` as the tests' `pmrig` does)
that calls a trap for each step:

```text
DispatchMessageA:
    trap  msg_target        ; Rust: find the window procedure and args
    push  args...           ; glue pushes what Rust put in registers
    call  eax               ; the program's window procedure
    trap  msg_done          ; Rust: record the result
    ret   4
```

The state between steps is on the guest stack and in Rust tables of plain
data, which the save state covers.

### Threads

Guest threads are saved CPU contexts, switched on one host thread. A
thread that blocks (`WaitForSingleObject`, `GetMessage`, `Sleep`) is
switched out at once; others are preempted on the PIT interrupt at a
fixed time slice, because games spin on flags that another thread sets.
There are no host threads, so the web build works, and the dynrec's
interrupt points keep runs deterministic.

### Exceptions

A fault at CPL 3 (#GP, #PF on an uncommitted page, #DE, INT 3) reaches
Rust through the IDT. A page fault on a reserved page that should be
committed (stack growth, guard pages) is handled there. Others become an
`EXCEPTION_RECORD` and a `CONTEXT` on the thread's stack, and glue code
walks the `fs:[0]` chain, calls the handlers and unwinds, as
`KiUserExceptionDispatcher` does. MSVC's `_except_handler3` and C++
exceptions run as the program's own code on top of that.

### Files, registry and version

- Paths go to `DiskController`'s drives, so a game sees the same C: and
  D: as under the built-in DOS, CD images included (`GetDriveType` says
  CD-ROM, `GetVolumeInformation` gives the label the CD check wants).
- Long file names are needed from the start. The built-in DOS doesn't
  have them (`INT 21h` AX=71xxh returns 7100h, `int21.rs:2178`), so
  kernel32 works on host directories' names directly, and `fat.rs` needs
  to read and write LFN entries for images.
- The registry is a tree in Rust (`src/win32/registry.rs`), the
  system's, shared by the processes and kept in save states. It is seeded
  with the keys games read (`HKLM\Software\Microsoft\Windows\CurrentVersion`
  with VersionNumber 4.00.950, DirectX's version, the display settings,
  the user and computer names) and kept on drive C: as
  `C:\REGISTRY.REG`, a REGEDIT4 file that can be read and edited by hand.
  A program's first registry call reads the seed with the file's keys
  and values over it; changes are written back when a key is closed or
  flushed and when the last process ends. Without a writable C: the
  registry lasts until then. A game's profile brings its keys as a .reg
  file, which the same import puts in (`[-key]` and `"name"=-` delete).
- `GetVersion` and `GetVersionEx` say Windows 95 (4.0.950, platform
  1), or with `win32_version=98` Windows 98 (4.10.1998) and with
  `win32_version=98se` Windows 98 SE (4.10.2222, " A "), as do the
  registry's version values and MSVCRT's `_osver`. Under 98 the
  built-in DLLs also export the functions Windows 98 added that games
  use (`version.rs` lists them: USER32's monitor functions,
  `AnimateWindow`, `GetCursorInfo`, KERNEL32's `IsDebuggerPresent`,
  `InterlockedCompareExchange`, `GetFileAttributesExA`,
  `GetLongPathNameA` and others); under 95 a program that imports one
  doesn't load, as on Windows 95, and `GetProcAddress` doesn't find it.
  The setting is read when the first process starts, and kept in save
  states.

### Video

All output goes through the emulated video hardware:

- **GDI and windows**: one desktop surface in a VBE mode (640x480 at 8,
  16 or 32 bits per pixel, or the size the game asks for). Windows are
  drawn into it; the fonts are a bundled bitmap font under a license that
  lets us ship it. Most games have one top-level window, and HX gets far
  with that alone.
- **DirectDraw** in exclusive full-screen mode sets the VBE mode the game
  asks for. The primary surface is the linear frame buffer, so `Lock`
  returns a pointer into it and the game's own pixel loops write straight
  to the card. `Flip` changes the display start (VBE 4F07h), and palettes
  go to the DAC. Off-screen surfaces live in guest memory; `Blt` and
  `BltFast` (fills, color keys, stretching) are done in Rust. Windowed
  DirectDraw composes into the desktop surface.
- **Glide 2.x** (`glide2x.dll`) is written in Rust against the emulated
  Voodoo's registers, as 3dfx's library was against the card. The Voodoo
  emulation (`src/voodoo/`) does the rendering, and 3dfx's MiniGL drivers,
  which games of the time shipped, run on top of it.
- **Direct3D** (DX5 and DX6 immediate mode: execute buffers and
  `DrawPrimitive`) becomes a HAL on the same emulated hardware, the way
  3dfx's Direct3D driver was one. The target card is the Banshee or
  Voodoo3 (`svga_banshee`, `svga_voodoo3`): 2D and 3D on one card, so
  DirectDraw, GDI and Direct3D share one frame buffer, and Direct3D in a
  window works. On a Voodoo 1 or 2, 3D shows only in full screen,
  through the pass-through. The emulated Voodoo already replays its
  triangles in OpenGL (`voodoo_renderer=opengl`), and issue #164 removes
  the software rendering that still runs alongside the replay. A software
  rasterizer in Rust would have neither, and would be a second 3D
  pipeline to keep correct. The HAL can feed triangles to the Voodoo's
  setup code directly, instead of encoding them as register writes that
  the card then decodes. The card's limits (16-bit color and depth,
  256x256 textures) are the ones games of the time were written for;
  games that want 32-bit color or larger textures get the 16-bit modes
  and downscaled textures they got on a Voodoo3.

### Sound and input

- **DirectSound and `waveOut`**: buffers stay in guest memory; a mixer
  device on the bus reads them at the emulated rate and writes to
  `Bus::audio_out`, so it goes through the host mixer like the Sound
  Blaster. Play cursors and notifications come from emulated time.
- **`midiOut` and MCI**: MIDI goes to the MPU-401's synthesizers
  (FluidSynth, MT-32, SC-55); MCI CD audio to the CD audio player.
- **DirectMusic** (DirectX 6.1, `src/win32/dmusic/`): DMUSIC, DMIME,
  DMLOADER, DMSTYLE, DMBAND and DMCOMPOS, their classes made through
  OLE32. The loader reads segments (.sgt), MIDI files, DLS collections,
  bands, styles and chord maps from files, memory or OLE32's streams on
  global memory and caches them, and the objects load themselves from such
  streams (IPersistStream::Load); a program's own IStream isn't read. A
  performance plays segments' sequence, tempo, time signature, system
  exclusive and band tracks by the emulated clock on the process's
  multimedia thread, with notifications. Both ports it lists (the
  Microsoft Synthesizer and the MIDI mapper) are the MPU-401's
  synthesizer, which plays downloaded DLS instruments by their General
  MIDI patches. Styles' patterns, chord and command tracks and the
  composer need the composition engine, which isn't there: they load and
  play nothing.
- **Input**: from the emulated 8042 keyboard, the PS/2 mouse and the game
  port, so the frontends, the debug server's input injection and key
  remapping need no changes. DirectInput and window messages both read
  from them.

### Installing games

Most Windows 95 CD games have to be installed before they run. A setup
program decompresses files from the CD to the hard disk, writes registry
keys (the install folder, the CD's drive letter, settings), may install
DirectX, and adds Start menu shortcuts. Games read those keys when they
start and fail without them. Many of these setup programs are 16-bit or
start with a 16-bit stub, from the Windows 3.1 era: InstallShield 3 is
16-bit throughout, and InstallShield 5's `SETUP.EXE` is a 16-bit stub
that starts a 32-bit engine. Running them as they are needs a Win16
layer: an NE loader, 16-bit segments as LDT selectors, and KERNEL, USER
and GDI's 16-bit APIs thunked to the 32-bit ones, as winevdm does. That is
a second project about the size of phases 1 and 2 together.

Most installs can do without it:

1. **Installed folders.** GOG and other digital releases, and folders
   copied from a real Windows installation, need no setup. The game's
   profile carries the registry keys it reads (a `.reg` file next to it),
   so it starts as if setup had run. This works from phase 1 on.
2. **32-bit setup engines.** For a known 16-bit stub, run its 32-bit
   engine directly, as 64-bit Windows does for InstallShield 5. Wise and
   later InstallShield setups are 32-bit already. This needs the layer to
   be broad enough for setup programs (dialogs, common controls, shell
   links), so it comes in phase 5.
3. **Unpacking in Rust.** Read the installer's archives to copy files
   without running setup at all, with the registry keys from the profile
   as in 1. The readers follow those of the refurbished-dinosaurs
   toolkit (`RefurbishedDinosaurs.LegacyFormats`, MIT), which are C#, so
   they are ported rather than reused:
   - InstallShield 5 and 6 cabinets (`data1.hdr` and `dataN.cab`, major
     versions 0, 5 and 6), laid out as Unshield reads them. Compressed
     members are deflate chunks with 16-bit lengths or, in some version
     0 sets, raw deflate ending each chunk with `00 00 FF FF` (Unshield's
     `-O`). Nothing in the header tells the two apart, so the profile
     names the form and the reader never guesses it. Version 6 members
     are checked against the header's MD5, the others by size.
   - InstallShield 3 archives (`_SETUP.1`, `.Z` files inside
     self-extractors), whose members are compressed with PKWARE's DCL
     implode, decoded as zlib's `contrib/blast` describes the format.
   - The header is checked whole before any member is read: relative
     paths, data inside the volumes, and limits on counts and sizes.
     Entries the reader can't read (marked invalid, stored outside the
     cabinet, two different files at one path) are listed with the
     reason, and the rest of the set still opens; a file at a shared
     path is read by its index.
   - A cabinet places files by file group. The reader reports each
     member's groups, and the profile maps groups to folders under the
     install folder, recorded from a real install as its registry keys
     are.

   The readers are the `rust-dos-installshield` crate
   (`crates/installshield`), pure Rust without unsafe code, with deflate
   from flate2. `rust-dos unpack` lists or extracts a set or an archive
   on the host, by file group if asked, and a game profile's `[install]`
   section unpacks one from the game's drives onto them the first time
   the game starts (`src/install.rs`), with the profile's `windows=builtin`
   and `games/<id>.reg` doing the rest of what setup would. A GOG folder
   whose launcher starts a Windows program imports as such a profile.

With these, a Win16 layer isn't needed to start, and it comes last, in
phase 6, for the Windows 3.1 games and the 16-bit setup programs that 1
to 3 don't reach.

### Setup programs that ran

InstallShield's 16-bit `SETUP.EXE` (InstallShield 5, and InstallShield
3 releases that ship the 32-bit engine) only unpacks its engine and
starts it. An NE `SETUP.EXE` with `_INST32I.EX_` beside it is taken
for that launcher (`src/win32/launcher.rs`): the files of the archive
(a list of PKWARE DCL members, read by the installshield crate's
`EngineArchive`) go to `TEMP`, all but the 16-bit `BOOT16.EXE`, and
the engine starts with the command line Windows 98 SE's launcher gives
it, as a probe program read it from the running engine there:

```
C:\WINDOWS\TEMP\_INS0432._MP -fD:\SETUP.INS  -z1 -cx -xC:\WINDOWS\TEMP\
```

The launcher's `_ISDEL.EXE`, which deletes those files after setup, is
16-bit and isn't run, so they stay in `TEMP`.

Getting the setups to the end took these, each as Windows 98 SE does
it, checked with a probe program there:

- A write to a module's resources, which are read-only pages,
  succeeds: Windows 98 makes the page writable and the write goes
  through, though VirtualQuery says read-only and IsBadWritePtr says
  the page can't be written (InstallShield's engine draws into its own
  bitmap resource).
- WINSPOOL.DRV with EnumPortsA (FILE:, LPT1:, COM1:, COM2:).
- ADVAPI32's security descriptor functions return TRUE with error 120
  (InitializeSecurityDescriptor, SetSecurityDescriptorDacl and
  SetSecurityDescriptorOwner) or FALSE with it (InitializeAcl,
  AllocateAndInitializeSid and the token functions). InstallShield's
  engine writes no registry key when InitializeSecurityDescriptor
  fails.
- MPR.DLL with WNetGetConnectionA, which finds no drive redirected.
- GetTempPathA makes the folder it names when it isn't there, as a
  game's C: drive has no `C:\WINDOWS\TEMP` until something makes it.
- GDI32 keeps metafiles (SetMetaFileBitsEx) and plays them
  (PlayMetaFile): Wise's dialogs draw their pictures from them.

Each install was compared with the same setup run on Windows 98 SE,
file by file with hashes, registry key by key and shortcut by
shortcut:

- **Enemy Infestation** (InstallShield 5): the 9 game files are the
  same. `Ei.cfg` differs in the CD's letter (D: here, E: on the
  Windows 98 machine). The registry keys are the same. The uninstall
  log `Uninst.isu` is 3729 bytes against 3680, with the names of the
  engine's dialog variables (`ResultA`, `bOpt1A`) and `5.00.000` in
  it. The Read Me shortcut points to `D:\NOTEPAD.EXE`, as the setup
  finds no Notepad on C:. The game starts, plays its intro and shows
  its menu.
- **Wages of War** (InstallShield 3's 32-bit engine): 361 of 362 files
  are the same, all but the uninstall log `DeIsL1.isu` (22979 bytes
  against 22922). The registry keys are the same, and the setup also
  makes an empty `HKLM\SharedDLLs`: the engine creates SharedDLLs
  under the wrong key when it doesn't find it, which Windows 98, where
  it is always there, never shows. The game says it must run in
  256-colour mode and stops: the built-in Windows' desktop has 32 bits
  per pixel.
- **Grom** (Wise, two CDs, the minimum installation): 1931 of 1932
  files are the same, all but the install log `INSTALL.LOG`, which
  differs in the time, the CD's letter, a `Made Dir` line for
  `C:\Program Files` and the case of names. The registry keys are the
  same but for the time of the last update check and the case of the
  Start menu folder's path. The setup asks for the second CD halfway
  through, as on Windows 98. It warns that the disk is too small, as
  DOS reports 80 MB free on a drive that is a host folder; Ignore goes
  on. The game itself needs IMAGEHLP, AVIFIL32 and MSVFW32, which
  aren't there.

Some differences hold for every setup:

- Names on a drive that is a host folder are in capitals, as DOS made
  them: the Start menu's folder is `START MENU`.
- The CD's Joliet names aren't read, so files the setup copies by
  their long names from the CD get 8.3 ones (Enemy Infestation's
  `.url` shortcuts are `PLAYON~1.URL`).
- Shortcuts are smaller: they have no relative path and no file
  times. Their targets, working folders and icons are the same.
- Dialog text is laid out with other font metrics, so some lines wrap
  or are cut where Windows 98's aren't, and characters the built-in
  fonts lack show as `?` (Grom's "DirectX®").

### Timing

`GetTickCount`, `timeGetTime`, `QueryPerformanceCounter` (at the PIT's
1.193182 MHz, as Windows 95 reported) and RDTSC all come from the emulated
clock, so a game's frame pacing follows `cycles` and is deterministic.

## Phases

Each phase ends with programs that run, checked by tests.

**Phase 0: plumbing.**

- A PE parser (headers, sections, relocations, imports by name and by
  ordinal, forwarded exports, resources, TLS directory) with tests.
- The new service trap in the interpreter, the dynrec, the debugger and
  the profiler.
- Starting a process: page tables, selectors, TIB, stub images, the
  entry point, and `ExitProcess` back to DOS.
- Console output (`GetStdHandle`, `WriteFile`, `WriteConsoleA`) to the
  DOS console, so `rust-dos run --win32 hello.exe` prints in the
  terminal.
- `win32=true` under `[emulator]` turns it on.
- The save state section (`W32 `), and a `[W32]` log line for each call
  of a built-in function.
- Test programs are built in the tests with a small PE writer and
  `code_asm`, so no binaries are checked in.
- Wine's conformance tests (see Testing) with the kernel32 functions
  their framework and first files need: the environment, TLS, the
  process heap, GetProcAddress, GetVersionExA, drives, times and code
  page conversions.

**Phase 1: the runtime.**

- kernel32's core: heaps, virtual memory, files, directories, the
  environment and command line, modules, TLS, critical sections, events,
  mutexes, threads, time.
- Structured exceptions.
- advapi32's registry.
- Enough of `msvcrt.dll` for programs that link the CRT dynamically.
- A debug server endpoint for processes, modules, threads and handles
  (`GET /api/win32`, described in `docs/llm-debugging.md`).

Milestone: console programs built with MSVC 4 to 6, Watcom and Borland
run, as do HX's sample programs.

**Phase 2: windows and GDI.**

- user32: classes, windows, message queues, input messages from the
  emulated devices, timers, `MessageBox`, dialogs from resources with the
  basic controls.
- gdi32: device contexts, DIB sections, `BitBlt`, `StretchBlt`,
  `TextOut`, palettes, on the VBE desktop.

Milestone: simple windowed games and a game's own launcher dialog.

**Phase 3: 2D DirectX and sound.**

- DirectDraw 1 to 6 interfaces, full screen and windowed.
- DirectSound, WinMM audio, MIDI and CD audio.
- DirectInput.
- `CoCreateInstance` for the DirectX classes.

Milestone candidates: the freeware Win95 releases of Command & Conquer and
Red Alert, the Diablo demo, Quake 2's software renderer.

Fallout (1997, DirectX 3) runs on it from GOG's folder: its movies with
their sound in 640x480 at 8 bits a pixel, its menus, and the mouse
through DirectInput. Overlays, gamma ramps, sound capture and force
feedback answer as a machine without that hardware does.

**Phase 4: 3D.**

- Glide 2.x on the emulated Voodoo.
- Direct3D 5 and 6 on the same.

Milestone candidates: Quake 2 with 3dfx's MiniGL, the Unreal demo with
Glide, Expendable with Direct3D.

GLIDE2X.DLL drives the card's registers as 3dfx's library did, on a
Voodoo Graphics, a Voodoo2, a Banshee and a Voodoo3, and Direct3D's HAL
(Direct3D 1 to 7's interfaces in DDRAW.DLL) draws through the same code.
On a Banshee or Voodoo3 the desktop is 640x480 at 16 bits a pixel in the
card's own memory and modes, so DirectDraw's surfaces there are what the
3D engine renders into, in a window too. A Voodoo Graphics or Voodoo2
add-on card is a second DirectDraw device, as 3dfx's driver made it,
with the card's full-screen modes and buffers. Without a 3dfx card there
is no GLIDE2X.DLL, as on Windows without 3dfx's driver: `LoadLibrary`
doesn't find it (or finds the game's own copy), and a program that
imports it doesn't start. There is no software rasterizer, so the RGB
and Ramp devices aren't there. The 3D engine draws 16-bit color and
depth only, so on a Banshee or Voodoo3 a 24- or 32-bit render target or
depth buffer is drawn into a 16-bit shadow of it in the card's memory,
which the HAL loads from the surface before drawing and widens into it
when the program can see it (Lock, GetDC, Blt, Flip, EndScene): the
surface stays 32-bit, what is drawn has 16-bit precision, and a Clear
writes its exact color. On a Voodoo Graphics or Voodoo2 such targets
fail, as they did with 3dfx's driver.

Unreal Gold runs from GOG's folder on a Voodoo2 and a Voodoo3: its
levels with Glide, its intro and menus with Direct3D. Quake 2's 3.14
demo runs its demos through 3dfx's MiniGL on both, with the MiniGL it
came with on the Voodoo2 and MiniGL 1.49 on the Voodoo3, as on the real
cards. Incoming and Expendable run with Direct3D. SHELL32, COMCTL32,
COMDLG32, WSOCK32 and DPLAYX came in for them, answering as a machine
without a network does, with each common dialog cancelled.

**Phase 5: breadth.**

- 32-bit installers (InstallShield 5's 32-bit engine, Wise) and
  unpacking installer archives (see "Installing games").
- Windows 98 as a version setting, and DirectX 6.1's additions
  (DirectMusic).
- Game profiles that pick the layer.
- The Wine leaf DLL experiment from above.

The archive readers, `rust-dos unpack` and the profiles' `windows=`,
`[install]` and `.reg` are in "Installing games"; the setup programs
that ran to a finished install, and what differed from the same
install on Windows 98 SE, in "Setup programs that ran"; the Wine DLLs
that ran as guest code in "Wine's DLLs as guest code".

COMCTL32 is Windows 95's version 4.70, its functions exported at their
ordinals, for launchers and setup programs: image lists, DPAs and DSAs,
MRU lists, and the progress bar, up-down control, trackbar, status bar,
tooltips, toolbar, header, list view, tree view, tab control and
property sheets with wizards, ported from Wine 1.2.3. The animation,
hot key and ComboBoxEx classes draw their background and nothing else,
and the date and time picker, month calendar, rebar and IP address
controls aren't there. Functions of later versions (SetWindowSubclass,
the DPA stream functions) are left out, and the controls take their A
messages, the list view and toolbar some W ones too.

COMDLG32 has GetOpenFileNameA and GetSaveFileNameA, which list the
folders and files of the emulated drives: in the Explorer style, or in
the old style when a program asks for multiple selection, a hook or a
template without OFN_EXPLORER. The Explorer-style dialog's view is an
owner-drawn list box. A hook that asks the dialog for its IShellBrowser
gets one that gives the dialog's windows and fails the rest with
E_NOTIMPL, and the dialogs don't take OFN_ENABLESIZING. GetFileTitleA
and CommDlgExtendedError are there too. ChooseColorA, ChooseFontA,
PrintDlgA and PageSetupDlgA return FALSE with no error, which is what
they return when the user presses Cancel, so the colour, font and print
dialogs never show. The W functions fail with
ERROR_CALL_NOT_IMPLEMENTED, and FindTextA and ReplaceTextA make no
window.

For setup programs, SHELL32 has shell links (CLSID_ShellLink with
IShellLinkA and IPersistFile, reading and writing `.lnk` files as
Windows does), SHFileOperationA, the special folders laid out as
Windows 95 has them and made when first asked for, item id lists,
ShellExecuteA and ShellExecuteExA (programs, documents by the types
the registry gives their extensions, folders, URLs by their scheme's
key) and SHBrowseForFolderA as a dialog with a list of folders.
Program Manager's DDE server, which InstallShield 5 makes its program
groups through, is built in, as Explorer answered for it: a group is a
folder under the Start menu's Programs, an item a shell link in it.
DDEML in USER32 talks to that server alone, as no program can be a
DDE server or client of another yet. VERSION.DLL reads version
resources and installs files with VerFindFileA and VerInstallFileA,
and LZ32 and KERNEL32 expand files that COMPRESS.EXE packed. OLE32
creates objects from the class objects programs register, from the
in-process servers the registry names and from the built-in classes;
it keeps CoInitialize's rules for apartments, maps ProgIDs to CLSIDs
and back, and marshals an interface between threads of one process as
its pointer. OLEAUT32 has BSTRs, and ADVPACK the three functions setup
programs ask before they install (IsNTAdmin, NeedReboot and
NeedRebootInit). Structured storage (compound files) is left for a
program that needs it.

`win32_version` picks Windows 95, 98 or 98 SE, and DirectMusic is
there under each (see "Sound and input"). Its classes are
apartment-threaded, as on Windows 98 SE, where a thread of the
multithreaded apartment gets them only as IUnknown: Wine's DirectMusic
tests ask for them that way or for DirectX 8's interfaces, so on the
built-in Windows they skip, as they do on Windows 98 SE.

Wine's tests cover the DLLs this phase adds, but not the installers, the
archive readers or what Windows 98 does differently from 95, so those
are tested in other ways (see "Testing").

**Phase 6: Win16.**

Windows 3.1 games, and Windows 95 games' 16-bit setup programs, without
any Windows files. Today `os=win311` runs Windows 3.1 on the built-in
DOS, but needs Windows 3.1's own files.

- An NE loader: segments as LDT selectors, segment relocations, imports
  by ordinal, resources (the parser shared with PE's), self-loading
  applications.
- KERNEL, USER, GDI, MMSYSTEM, KEYBOARD, SOUND, SHELL, COMMDLG, TOOLHELP
  and WING as 16-bit stub modules, whose functions thunk to the 32-bit
  implementations, as Windows 95's own 16-bit modules did.
- Win16 tasks scheduled cooperatively under one lock (Win16Mutex), next
  to the preemptive Win32 threads.
- 16-bit DPMI calls (INT 31h) and DOS calls from Windows programs, which
  the existing DPMI host and DOS already have.

winevdm's scope (GPL-2.0, Wine's 16-bit DLLs) is the reference for what a
complete layer needs.

Some choices in the earlier phases keep this phase from having to rework
them:

- Window, GDI object and module handles are 16-bit values, as they were
  on Windows 95, so 16- and 32-bit code share the same windows and
  objects.
- Selectors come from an LDT the process owns from phase 0, with room
  for 16-bit segments.
- The service trap is accepted in 16-bit code segments too.
- Message queues and window procedures carry a flag for 16-bit
  procedures, so messages can be translated between the two sizes.

Phase 3 is where the layer starts running games people know, so phases 0
to 2 should stay as small as the phase 3 games allow: implement what their
traces call, not whole DLLs.

## Testing

- Unit tests per DLL area, with generated PE programs, run on both cores
  as the rest of the suite is (`RUST_DOS_CORE=dynamic`).
- Win32 games in the game suite (`docs/game-suite.md`), from freeware
  releases and demos, with golden screen hashes. Determinism makes the
  hashes stable.
- Comparisons with the same game on a booted Windows 95 image
  (`C:\sources\win98` has the setup for that), using the debug server to
  compare screens and the API trace to find where they part.
- Wine's conformance tests (`dlls/*/tests/*.c`), in
  `tests/conformance/`. They are Win32 programs that check the API's
  behaviour against what real Windows returned. These rules apply:
  - They are kept in the repository, copied from one pinned Wine tree,
    so CI fetches nothing and they change only in a commit. Any patch
    one needs is listed in the folder's README.
  - The tree is Wine 1.2.3. Wine tested on Windows 9x until about 2010,
    and its tests of that time accept 9x's results (`broken()`,
    `win_skip`); later trees expect NT's in places. DirectMusic's tests
    are the exception: 1.2.3 has only dmloader's `loader.c`, dmime's
    `performance.c` came in 1.4.1 and dmusic's `dmusic.c` in 1.6.2. They
    are copied from the release that first has them, and each check
    that fails is compared with the booted Windows 98 image before it
    is fixed or its result is written into `expected.txt`.
  - A test file is copied once the built-in DLL has the functions it
    calls directly, so the set grows with the DLLs: twenty of
    kernel32's files, advapi32's `registry`, ten of msvcrt's, eleven of
    user32's (`msg` and `win` among them), eight of gdi32's, and from
    phase 3 four of ddraw's, six of dsound's, four of dinput's and seven
    of winmm's (the folder's README lists them). Phase 4 adds ddraw's
    `d3d` and `visual`, which test Direct3D. Phase 5 adds the DLLs that
    setup programs use, all from the 1.2.3 tree: sixteen of comctl32's,
    comdlg32's `filedlg`, six of shell32's (`progman_dde` among them,
    for the program groups InstallShield 5 makes through DDE),
    version's `info` and `install`, lz32's `lzexpand_main`, and ole32's
    `compobj`, `hglobalstream` and `propvariant`; and DirectMusic's
    three, from the releases named above.
  - Since phase 5 they run as Windows 98 SE (`win32_version=98se`), so
    that the functions Windows 98 added are there for the tests that
    use them.
  - `tests/conformance_tests.rs` builds them with MinGW and runs each
    under the built-in Windows: all but msvcrt's as 486 code linked with
    a small C runtime of rust-dos's own on KERNEL32, so they test the
    built-in DLLs alone; msvcrt's linked with the built-in MSVCRT.DLL,
    on a Pentium Pro. No binaries are checked in.
  - Their results (each file's summary, the places of the checks that
    fail and the tests skipped, or how a program ended early) must be
    those in `expected.txt`. Any difference, better or worse, fails the
    test, so every change to them shows in that file's diff.
  - CI runs them in a workflow of their own with Ubuntu 22.04's MinGW,
    the one the expected results were made with: on changes to the
    built-in Windows, the PE crate, the processor and recompiler, DOS's
    file writes or the tests, and every night. Without MinGW the test
    passes with a note, so `cargo test` works on any machine.
  - They are under the LGPL 2.1 or later, kept with their notices,
    which rust-dos's GPL-2.0-or-later allows.
- The same test programs on the booted Windows 95 and 98 images give
  the expected output where Wine's tests don't say what 9x does: a test
  passes when the built-in layer prints what Windows printed. Wine's
  1.2.3 tests accept 95's, 98's and ME's answers alike, so they show
  that the Windows 98 setting breaks nothing, but not where 98 should
  answer differently from 95. Those differences come from the booted
  images.
- Installer archives: unit tests build InstallShield cabinets,
  InstallShield 3 archives and implode streams with a writer in the
  tests, as the PE tests build programs. A writer and a reader made
  from one reading of a format only show that the two agree, so the
  tests also decode streams that an independent decoder expanded, as
  the toolkit's tests do: the example in blast's format description,
  and a coded-literal stream pwexplode expanded to the same text. A
  game's own archive, compared entry by entry with what Unshield (or
  idecomp, for InstallShield 3) extracts, is the acceptance check. It
  runs outside the repository, as the game suite does. In phase 5 it
  covered, all files equal by SHA-1:
  - Oni (Europe): three version 5 sets with `.hdr` files (data1, _sys1,
    _user1), 122 files, among them paths that hold one file per language
    and are read by group.
  - Rent-A-Hero (Europe): version 5 with the header in `data1.cab` (129
    files) and its `_sys1.cab` (4).
  - WarGames (USA) v1.01: version 0 sets (`data1.cab`, 224 files, and
    `_sys1.cab`, 4), which need the marker-delimited form; the chunked
    one fails on them with a message that names the other.
  - Wages of War: InstallShield 3 `_SETUP.1` (622 files) and five
    `.Z` archives (61 files), paths equal too.
  - Railroad Tycoon II (USA): InstallShield 3 `_SETUP.1` (63 files).

  No version 6 set was found on the media at hand, so version 6 is
  tested by the writer's sets alone. Magic: The Gathering - Battlegrounds
  has a major version 7 header, which the reader refuses.
- Installs: a real installer, run under the built-in Windows or
  unpacked in Rust, from media kept outside the repository as the game
  suite's are. The check is what the install leaves behind, the files
  written with their hashes and the registry keys set, compared with an
  install of the same game on the booted Windows 98 image.

## Licensing

rust-dos is GPL-2.0-or-later.

- Wine's code (LGPL-2.1+) may be ported, with its copyright notices kept.
- cnc-ddraw (MIT) and DDrawCompat (0BSD) may be reused.
- Boxedwine (GPL-2.0-only) can't be: it would make the whole program
  GPL-2.0-only.
- retrowin32 (Apache-2.0) is incompatible with GPL-2.0, so we take its
  ideas and not its code, unless its author agrees to relicense.
- HX has no license file, so it serves as a reference and a test, not a
  source.
- The refurbished-dinosaurs toolkit's format readers (MIT) and Unshield
  (MIT) may be ported. Code from idecomp or pwexplode (GPL-3.0) would
  make the whole program GPL-3.0, which isn't ours to decide, so their
  code is not used. The readers implement the same formats from
  descriptions of them (the toolkit's decision records, blast's format
  description), and the two tools are run as programs outside the
  repository: for research into what an archive holds, and to check
  what the readers extract.
- Nothing from Microsoft is used or shipped: no DLLs, fonts or headers
  beyond what Wine's and MinGW's public headers document.

## Risks

- **Scope.** The Win32 API is large. Trace-driven work and early
  milestones with real games keep it to what games call.
- **Copy protection.** SafeDisc and SecuROM use VxDs and ring 0 tricks.
  They are out of scope; games' no-CD releases or GOG versions are the
  way around.
- **Direct hardware access.** Games that talk to VxDs (`DeviceIoControl`
  on `\\.\VWIN32` for DOS calls is common and can go to the built-in
  DOS; others are not) or need a vendor's driver.
- **Direct3D's spread.** Games use D3D in many ways; the Voodoo HAL covers
  what Voodoo cards could do, which most games of the period targeted.
- **Fonts and look.** Dialogs and text in games depend on Windows' fonts'
  metrics; a bundled font changes layout in places.

## Wine's DLLs as guest code

Wine's PE builds of DLLs at the edges of the API could run as guest code
on top of the Rust core, instead of being written again. The candidates
are the DLLs games use a little of but whose behaviour is large:

- `comctl32` and `comdlg32` for launchers and setup programs;
- `msacm32` with its ADPCM codecs, for compressed WAV files;
- `quartz` and `amstream` for DirectShow video, as late-90s games used for
  their cut scenes.

The core (kernel32, user32, gdi32, ddraw, dsound, dinput, winmm) stays in
Rust. Wine's versions of those sit on its NT kernel interface and on
wined3d, which is what this plan avoids.

The cost is what the Wine DLLs import:

- Most of them call `ntdll` (`Rtl*`, `Nt*`) and Wine's own exports
  (`__wine_dbg_*`).
- They implement the ANSI functions on top of the Unicode ones, so the
  Rust user32 and gdi32 would need the `W` functions as well, which Win9x
  games themselves rarely call.
- Shipping them means following the LGPL: the DLLs stay separate files,
  and their source is offered.

Which Wine version to use follows from that. The PE conversion was
complete in Wine 8.0, and in 8.10 the last direct calls between PE and
Unix code went through the syscall interface, so 8.10 is the oldest
version to consider. The latest release has the most fixes. Each release
also changes what its DLLs import, so the version is pinned and moved
only on purpose. Before committing to one, list the imports of each
candidate DLL in the latest release and in an older one, and take the
version whose import list the Rust core can cover with the least work.
Usually that will be the latest. Wine's code may also be ported into Rust
instead (LGPL-2.1 section 3 allows that for a GPL project), which avoids
the import problem for DLLs that are small enough.

### The experiment: imports of Wine 9.0 and 11.0

The DLLs came from WineHQ's Ubuntu packages of the two stable releases,
`wine-stable-i386` 9.0 (jammy) and 11.0 (noble), whose
`lib/wine/i386-windows` folders hold the PE builds. Each DLL's import
table was read and every import compared with the exports of the
built-in DLLs. "Missing" is an import no built-in DLL exports; "stub" is
one a built-in DLL exports as Windows 95 does, as a stub that fails
(mostly `W` functions of USER32 and KERNEL32).

| DLL            | 9.0: imports, missing, stub | 11.0: imports, missing, stub |
|----------------|-----------------------------|------------------------------|
| `msacm32`      | 67, 22, 9                   | 67, 22, 9                    |
| `imaadp32.acm` | 24, 14, 1                   | 24, 14, 1                    |
| `msadp32.acm`  | 24, 14, 1                   | 24, 14, 1                    |
| `comctl32`     | 378, 115, 37                | 307, 79, 36                  |
| `comdlg32`     | 297, 102, 46                | 285, 88, 46                  |
| `quartz`       | 215, 124, 22                | 181, 64, 22                  |
| `amstream`     | 52, 25, 5                   | 53, 8, 5                     |

Wine 11.0 is the one to use. The ACM DLLs miss the same functions in
both. Counted after the aliases described below (UCRT as MSVCRT,
KERNELBASE as KERNEL32), 11.0 misses fewer functions for `comctl32` (13
against 36), `comdlg32` (58 against 73) and `quartz` (63 against 94);
only `amstream` misses more (8 against 5), and it is the smallest.

Most of what the ACM DLLs miss is UCRT: Wine's DLLs import their C
runtime from `ucrtbase.dll`, whose functions are MSVCRT's but for four
(`__acrt_iob_func`, `__stdio_common_vsprintf`,
`__stdio_common_vswprintf`, `_assert`). The rest is `ntdll`'s
`_vsnprintf` and KERNEL32's `GetPrivateProfileSectionW`. None of these
DLLs imports Wine's own `__wine_dbg_*` exports: the ACM DLLs look them
up in `ntdll` with `GetProcAddress` and print nothing without them.

### How they load

The `win32_dlls` setting names a host folder, such as Wine's
`i386-windows`, whose DLLs a program may load (CONFIGURATION.md). The
loader (`src/win32/loader.rs`) looks there for a DLL that has no built-in
DLL and isn't on the search path, and reads it as if it were in
`C:\WINDOWS\SYSTEM`; `win32_dlls_replace` lets the folder's DLL replace
a built-in one of the same name. With the setting, `ucrtbase.dll` and
`api-ms-win-crt-*.dll` are the built-in MSVCRT, `kernelbase.dll` is
KERNEL32, and there is a built-in NTDLL.DLL at BFE40000h with
`_vsnprintf`; without it there is no NTDLL, as programs looking for
Windows NT expect.

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
is the process heap, so each TIB now points at a block of the process's
that has it.

### What ran

Wine 11.0's `msacm32.dll` with its IMA ADPCM codec, `imaadp32.acm`,
encodes 16-bit PCM to IMA ADPCM and decodes it back through
`acmStreamOpen` and `acmStreamConvert`, and the samples match a decoder
written from the format's definition (`tests/wine_dlls.rs`, which runs
when WINE_DLLS names the folder). MSACM32 finds the codec in SYSTEM.INI's
`[drivers32]` (`msacm.imaadpcm=imaadp32.acm`) and opens it with
OpenDriver, so WINMM's installable drivers are now real: OpenDriver loads
the DLL as LoadLibrary does, finds its alias in SYSTEM.INI, and sends its
`DriverProc` DRV_LOAD, DRV_ENABLE and DRV_OPEN; SendDriverMessage and
CloseDriver call it as Wine's `winmm` does.

What it needed besides the loader: the four UCRT functions, NTDLL's
`_vsnprintf`, `GetPrivateProfileSectionW`, the installable drivers, the
process heap at `fs:[30h]`, and ADVAPI32's `Reg*W` stubs returning
ERROR_CALL_NOT_IMPLEMENTED to guest code. Windows 95's stubs return 0
with that error set, as Wine's registry tests find, and programs keep
getting that; MSACM32 takes 0 as ERROR_SUCCESS and would enumerate the
registry with a key it never got. No call failed through a guest stub
on the way. The other stubs it met fail as on Windows 95 and MSACM32
copes: `GetModuleHandleW` (for `ntdll`'s debug functions),
`RegOpenKeyExW` and `RegCreateKeyW` (its driver cache, which it fills
from the drivers instead).

### What the others would take

The counts after this work, with the aliases and the functions above:

- `comctl32` 11.0: 13 missing, 36 stubs. The missing ones are mostly
  Vista functions (`ResolveDelayLoadedAPI`, `DelayLoadFailureHook`,
  `GdiAlphaBlend`, `GetMonitorInfoW`, `IsDialogMessageW`). The stubs are
  the larger work: its window classes are written in Unicode, so
  `CreateWindowExW`, `DefWindowProcW`, `SendMessageW` and the other `W`
  functions of USER32 and GDI32 would have to work for guest code, with
  windows whose procedures take Unicode messages. With the built-in
  COMCTL32 written in Rust, it is only worth it for controls that one
  doesn't have.
- `comdlg32` 11.0: 58 missing, 46 stubs. It needs `comctl32`'s image
  lists and up-down control, `shell32` and `shlwapi` for its file
  dialogs, activation contexts and resource functions in Unicode
  (`FindResourceW`, `EnumResourceNamesW`), `CreateDCW` for printing, and
  `ntdll`'s Unicode strings. It would come after `comctl32`.
- `quartz` 11.0: 63 missing, 22 stubs. It needs I/O completion ports,
  the thread pool and condition variables of KERNEL32,
  `DirectSoundCreate8`, OLEAUT32, RPCRT4 for its COM proxies, and
  `msvfw32` for its codecs, which are Wine DLLs in turn. Of the three it
  is the furthest off; `amstream` (8 missing: condition variables,
  `InitializeCriticalSectionEx`, `__iob_func`) sits on top of it.

## Decisions

1. **Direct3D**: on the emulated Banshee/Voodoo3, with its OpenGL replay
   (see "Video").
2. **Installers and Win16**: installed folders with registry keys from
   the profile first, then 32-bit setup engines and unpacking in Rust
   (see "Installing games"). Full Win16 is phase 6, with the earlier
   phases keeping 16-bit handles and selectors ready for it.
3. **Wine's DLLs**: an experiment after phase 3, limited to the DLLs at
   the edges, on a pinned version chosen by measuring its imports.
