# Conformance tests

Wine's conformance tests for the DLLs of the built-in Windows, which
`tests/conformance_tests.rs` builds with MinGW and runs under it. Each
test checks what a Windows function returns against what Windows
returned when Wine's developers ran it, accepting the Windows 9x answers
where they differ from NT's.

## Where they come from

`wine/` holds files from Wine 1.2.3 (tag `wine-1.2.3`, commit
`9dc5af175a5537b1f98af34cdcbdc22639b36d53`), unchanged but for the
change listed below:

- `include/wine/test.h`, the test framework;
- `kernel32/`: `alloc.c`, `codepage.c`, `directory.c`, `drive.c`,
  `environ.c`, `fiber.c`, `file.c`, `format_msg.c`, `heap.c`,
  `loader.c`, `locale.c`, `module.c`, `path.c`, `profile.c`, `sync.c`,
  `thread.c`, `time.c`, `timer.c`, `version.c`, `virtual.c` from
  `dlls/kernel32/tests/`;
- `advapi32/`: `registry.c` from `dlls/advapi32/tests/`;
- `user32/`: `class.c`, `dce.c`, `dialog.c`, `input.c`, `msg.c`,
  `resource.c`, `static.c`, `text.c`, `uitools.c`, `win.c`,
  `wsprintf.c`, and the resources they share, `resource.rc` with
  `test_mono.bmp`, from `dlls/user32/tests/`;
- `gdi32/`: `bitmap.c`, `brush.c`, `clipping.c`, `dc.c`, `gdiobj.c`,
  `mapping.c`, `palette.c`, `pen.c` from `dlls/gdi32/tests/`;
- `dinput/`: `device.c`, `joystick.c`, `keyboard.c`, `mouse.c` from
  `dlls/dinput/tests/`;
- `msvcrt/`: `data.c`, `dir.c`, `environ.c`, `file.c`, `heap.c`,
  `printf.c`, `scanf.c`, `signal.c`, `string.c`, `time.c` from
  `dlls/msvcrt/tests/`. `misc.c` is left out: it includes Wine's own
  `msvcrt.h`, which isn't among these files.
- `ddraw/`: `d3d.c`, `ddrawmodes.c`, `dsurface.c`, `overlay.c`,
  `refcount.c`, `visual.c` from `dlls/ddraw/tests/`. `d3d.c` and
  `visual.c` test Direct3D, and run with a Voodoo3 as the display
  adapter, whose HAL is Direct3D's device.
- `dsound/`: `capture.c`, `ds3d.c`, `ds3d8.c`, `dsound.c`, `dsound8.c`,
  `propset.c` and the header they share, `dsound_test.h`, from
  `dlls/dsound/tests/`.
- `winmm/`: `capture.c`, `mci.c`, `midi.c`, `mixer.c`, `mmio.c`,
  `timer.c`, `wave.c` and the header they share, `winmm_test.h`, from
  `dlls/winmm/tests/`. `generated.c` is left out: it checks the
  structures' sizes and fields, and MinGW's headers have some of the
  MCI structures' WORD fields as UINTs, so it doesn't build.
- `dmloader/`: `loader.c` from `dlls/dmloader/tests/`; `dmime/`:
  `performance.c` from `dlls/dmime/tests/` of Wine 1.4.1 (tag
  `wine-1.4.1`); `dmusic/`: `dmusic.c` from `dlls/dmusic/tests/` of
  Wine 1.6.2 (tag `wine-1.6.2`). 1.2.3 has only DirectMusic's
  `loader.c`, so the other two come from the first release that has
  them.

`heap.c` has the values of the `FLG_HEAP_*` and `FLG_POOL_ENABLE_TAGGING`
flags defined after its includes, from Wine's `winternl.h`, as MinGW's
lacks them.

Of the 1.x releases compared (1.0.1, 1.2.3, 1.4.1, 1.6.2, 1.8.7), 1.2.3's
kernel32 tests mention Windows 9x the most: later releases took many of
the 9x answers out once Wine stopped testing on 9x. Any change made to a
file is listed here.

They are under the LGPL, version 2.1 or later (`wine/COPYING.LIB`), with
their copyright notices. rust-dos is GPL-2.0-or-later, which LGPL code
may be used under.

`runtime/compat.h` is rust-dos's own: the few names the tests take from
Wine's headers that MinGW's lack (`__ms_va_list` and its macros,
`LOCALE_RETURN_GENITIVE_NAMES`, `LPMIDIEVENT`). Each test file is built
with it included first, so the files stay unchanged.

`runtime/runtime.c` is rust-dos's own: the C runtime the kernel32,
advapi32, user32, gdi32, dinput and winmm tests link in place of
MSVCRT.DLL, so that they test the DLLs alone. It has the startup code
and the parts of the C library that the framework and the tests use,
written on KERNEL32 alone. It also has `dump_region`, which `win.c`
calls and Wine has in `msg.c`: `win.c` is built with
`-Ddump_region=conformance_dump_region` to call that one. And it has
`sqrt`, which winmm's `timer.c` takes, and `sin` and `fabs`, for the
tones the dsound and winmm tests make.

`runtime/winmm.c` is rust-dos's own too: `mmsys_error`, the names of
the multimedia errors, which winmm's tests other than `wave.c` and
`capture.c` take from `wave.c`, as Wine builds them into one program.
Each of those is linked with it.

The user32 and gdi32 tests link the built-in USER32.DLL and GDI32.DLL.
Each user32 program is linked with `resource.rc`, compiled with
MinGW's `i686-w64-mingw32-windres`, or the one `CONFORMANCE_WINDRES`
names.

The dinput tests link the built-in DINPUT.DLL. They run with the mouse
as a joystick on the game port, so that `joystick.c`, which tests each
joystick DirectInput finds, has one to test.

The dsound tests call each other's helpers (`dsound.c` plays its
buffers with `ds3d.c`'s `test_buffer`), and winmm's `capture.c` calls
`wave.c`'s, so each of those DLLs' files is built into one program,
named after the DLL, as Wine builds them; it runs once for each file,
with the file's name as its argument. The winmm program is built with
`STATIC_KSDATAFORMAT_SUBTYPE_PCM` and `_IEEE_FLOAT` defined, as
`capture.c` declares those GUIDs of `mmreg.h` again. The dsound
programs link MinGW's `libksuser` for the GUIDs of `ksmedia.h`.

`wave.c` plays about 64 seconds of sound, in the formats WinMM takes,
and makes each buffer before it plays it: it runs for 1.3 billion
instructions, about 10 seconds of the suite, two thirds of the batches
a program may run for.

The msvcrt tests test the built-in MSVCRT.DLL, so they are linked as
MinGW links any program: its startup code (`crt2.o`) and `-lmsvcrt`,
without `runtime.c`. They are built with two more options:

- `-fno-printf-return-value`, so that GCC calls sprintf for what it
  returns instead of counting it by C99's rules, which MSVCRT's
  formats (`%I64d`, `%lld`) don't follow;
- `-D__NO_ISOCEXT`, so that MinGW's headers keep MSVCRT's strtod
  instead of putting MinGW's own in its place.

`crt2.o` uses CMOVcc, so these programs run on a Pentium Pro; the
others on a 486.

All of them run as Windows 98 SE (`win32_version = 98se`), so the
tests find the functions Windows 98 adds (SendInput, TrackMouseEvent,
GetAncestor, the monitors, GetLongPathNameA); under Windows 95 they
skip what needs them.

## Running them

They need MinGW's 32-bit compiler, `i686-w64-mingw32-gcc`, or the one
`CONFORMANCE_CC` names. `expected.txt` was made with Ubuntu 22.04's
(`gcc-mingw-w64-i686-win32`: GCC 10.3, mingw-w64 8.0), as CI runs them,
since another version can build them into code that gives other
results.

```sh
CONFORMANCE_CC=i686-w64-mingw32-gcc-win32 cargo test --release --test conformance_tests
```

Without a compiler the test says so and passes, so `cargo test` works
without MinGW. CI's `Conformance` workflow sets `CONFORMANCE_REQUIRED`,
which makes a missing compiler a failure.

On Windows, a WSL Ubuntu 22.04's compiler works through a wrapper such
as `mingw-gcc.cmd`, named by `CONFORMANCE_CC`. Its paths are relative,
so they are the same in WSL:

```bat
@wsl.exe -d Ubuntu -- i686-w64-mingw32-gcc-win32 %*
```

Each test file becomes `target/conformance/<dll>/<NAME>.EXE`, its output
`<file>.log` beside it.

A debug build runs them many times slower than a release build, which
CI uses. To work on some of them, `CONFORMANCE_ONLY` names those to run
(a DLL, or `user32/static`, or a comma-separated list), and prints
their results without comparing them. `CONFORMANCE_LOG=1` writes the
emulator's log, with every call a program makes, to
`target/conformance/<dll>/<file>.trace`.

```sh
CONFORMANCE_ONLY=user32/dce cargo test --release --test conformance_tests -- --nocapture
```

## The results

`expected.txt` has each test file's results: its summary line, the
places (file and line) of the checks that failed and of the tests
skipped, or the message the program ended with before its summary,
such as a call of a function the built-in DLL doesn't have. The test
fails when the results differ, better or worse, so the change shows in
the file's diff. When it is right, write the results anew:

```sh
CONFORMANCE_BLESS=1 cargo test --release --test conformance_tests
```

The results that aren't clean, and why:

- kernel32 `file.c:350`: FindFirstFileA gives "testfile.xxx" back as
  "TESTFILE.XXX": without long file names, which the built-in Windows
  doesn't have yet, DOS keeps names in upper case.
- kernel32 `module.c:259`, `270`, `282`: LoadLibraryExA of an empty
  file that another handle has open for writing fails with
  ERROR_BAD_FORMAT; Windows 9x's loader can't open it, and says
  ERROR_FILE_NOT_FOUND. `367` and `370`: COMCTL32.DLL isn't there to
  load.
- msvcrt `environ.c:51`: `system` can't start a command interpreter,
  which is a DOS program. `system(NULL)` still returns 1, as Windows
  95's does, since there is one there.
- msvcrt `file.c:1339`: `_pipe` fails with EMFILE, as KERNEL32 has no
  anonymous pipes yet.
- gdi32 `bitmap.c:2527`, a StretchBlt with both rectangles given
  backwards, and `2755`, a StretchDIBits that shrinks, give other
  pixels than Windows did. Wine marks both `todo_wine`.
- gdi32 `dc.c:453`: DeleteDC of a class DC leaves it there, as on
  Windows 9x, which the test allows only through `broken()`.
- user32 `msg.c` and `win.c` compare the messages a window gets with
  what Windows 2000 and XP sent, and many of their sequences fail: the
  built-in USER32 doesn't match every message of those yet. Some of the
  differences are Windows 95's own: it has no WM_UNINITMENUPOPUP,
  WM_MENUCOMMAND or TrackMouseEvent, and keeps a BS_USERBUTTON a user
  button where XP makes it a push button.
- user32 `win.c:4252` and `4264`: the clip box of a CS_PARENTDC
  window's DC, which Wine marks `todo_wine` too.
- dsound `dsound.c:1087`: DSOUND.DLL is still loaded after the last
  object goes, as the built-in DLLs never unload.
- dsound `propset.c:215`: OLE32's class factory gives E_POINTER for a
  CreateInstance without a place to put the object, where the test
  wants DSERR_INVALIDPARAM, as DirectSound's own factory gives.
- dsound `dsound.c:741`, skipped 48 times: WAVEFORMATEXTENSIBLE primary
  formats, which Windows 95's DirectSound doesn't take either.
  `ds3d8.c` and `dsound8.c` skip all they test, as DirectSoundCreate8
  is DirectX 8's.
- ddraw `dsurface.c:1725`, `2199`, `3344`: tests of surfaces the
  display has no hardware for skip, as on a card of Windows 95's time:
  cube maps, DXT textures in video memory, and the R3G3B2, L8 and A8L8
  formats.
- ddraw `overlay.c:60` and `220`: there are no overlays, so the test
  skips all of it.
- ddraw `d3d.c:377`: the Voodoo3 has no hardware transform and
  lighting, and says so as 3dfx's driver did (dwMaxActiveLights of
  -1), so the light tests skip. `d3d.c:2078`: it has no cube maps.
  `d3d.c:891` fails and `d3d.c:3532` skips: there is no RGB device, the
  software rasterizer, which the Direct3D 1 tests need and the test
  expects among the devices.
- ddraw `visual.c` draws in a 32-bit mode with a 24-bit depth buffer
  and reads the pixels back as 32-bit ones. The Voodoo3's 3D engine
  draws 16-bit 565 pixels and 16-bit depth only, so the HAL draws into
  16-bit shadows of the surfaces and widens what it drew into them when
  the program can see them; a Clear writes its exact color. Its 15
  failures are checks of drawn pixels that want more than 16 bits:
  `922`, `936`, `1376`, `1388`, `1509`, `1515`, `1625`, `1631`, `1637`,
  `1757` and `1895` blend or modulate to a value 5 or 6 bits can't come
  within the test's tolerance of (0x84 where it wants 0x80 to 0x82;
  `1376` and `1388` also take the texture's 8-bit alpha as 4-bit), and
  `2911` to `2915` want colors within 1 of 0x002B7F and 0xF9E814.
  `943` wants destination alpha, which a 565 shadow doesn't have.
  `2680` skips: there are no cube maps.

- winmm `mci.c:438` skips its recording tests, as there is no wave
  input device, and `515`, `634` and `870` skip the plays of the file
  those tests would have saved.
- winmm `mmio.c:77`, `244` and `497` skip: they read `msrle.avi`, a
  file of Wine's tests built from a resource, which isn't among these
  files.

- dmloader `loader.c`, dmime `performance.c`, dmusic `dmusic.c`: all
  skip, as on Windows 98 SE. The first two ask for DirectX 8's
  IDirectMusicLoader8 and IDirectMusicPerformance8, which DirectX 6.1
  doesn't have; `dmusic.c` makes DirectMusic from the multithreaded
  apartment, where Windows 98 SE gives its apartment-threaded classes
  only as IUnknown (REGDB_E_IIDNOTREG for IDirectMusic).

## Adding tests

Copy a test file from the same Wine tree (`dlls/<dll>/tests/<file>.c`)
into `wine/<dll>/` once the built-in DLL has the functions it calls
directly: a call of one it lacks ends the program there. Functions it
finds with GetProcAddress may be missing, as on Windows 95; the tests
skip what needs them. Then write the results anew and read the diff of
`expected.txt`: every check that fails there is a difference from
Windows to fix or to explain.
