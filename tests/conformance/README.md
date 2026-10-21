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
`LOCALE_RETURN_GENITIVE_NAMES`). Each test file is built with it
included first, so the files stay unchanged.

`runtime/runtime.c` is rust-dos's own: the C runtime the kernel32,
advapi32, user32, gdi32 and dinput tests link in place of MSVCRT.DLL,
so that they test the DLLs alone. It has the startup code and the
parts of the C library that the framework and the tests use, written on
KERNEL32 alone. It also has `dump_region`, which `win.c` calls and Wine
has in `msg.c`: `win.c` is built with
`-Ddump_region=conformance_dump_region` to call that one.

The user32 and gdi32 tests link the built-in USER32.DLL and GDI32.DLL.
Each user32 program is linked with `resource.rc`, compiled with
MinGW's `i686-w64-mingw32-windres`, or the one `CONFORMANCE_WINDRES`
names.

The dinput tests link the built-in DINPUT.DLL. They run with the mouse
as a joystick on the game port, so that `joystick.c`, which tests each
joystick DirectInput finds, has one to test.

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

## Adding tests

Copy a test file from the same Wine tree (`dlls/<dll>/tests/<file>.c`)
into `wine/<dll>/` once the built-in DLL has the functions it calls
directly: a call of one it lacks ends the program there. Functions it
finds with GetProcAddress may be missing, as on Windows 95; the tests
skip what needs them. Then write the results anew and read the diff of
`expected.txt`: every check that fails there is a difference from
Windows to fix or to explain.
