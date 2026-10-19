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

`runtime/runtime.c` is rust-dos's own: the C runtime the kernel32
tests link in place of MSVCRT.DLL, so that they test KERNEL32 alone. It
has the startup code and the parts of the C library that the framework
and the tests use, written on KERNEL32 alone.

The msvcrt tests test the built-in MSVCRT.DLL, so they are linked as
MinGW links any program: its startup code (`crt2.o`) and `-lmsvcrt`,
without `runtime.c`. They are built with two more options:

- `-fno-printf-return-value`, so that GCC calls sprintf for what it
  returns instead of counting it by C99's rules, which MSVCRT's
  formats (`%I64d`, `%lld`) don't follow;
- `-D__NO_ISOCEXT`, so that MinGW's headers keep MSVCRT's strtod
  instead of putting MinGW's own in its place.

`crt2.o` uses CMOVcc, so these programs run on a Pentium Pro; the
kernel32 ones on a 486.

## Running them

They need MinGW's 32-bit compiler, `i686-w64-mingw32-gcc`, or the one
`CONFORMANCE_CC` names. `expected.txt` was made with Ubuntu 22.04's
(`gcc-mingw-w64-i686-win32`: GCC 10.3, mingw-w64 8.0), as CI runs them,
since another version can build them into code that gives other
results.

```sh
CONFORMANCE_CC=i686-w64-mingw32-gcc-win32 cargo test --test conformance_tests
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

## The results

`expected.txt` has each test file's results: its summary line, the
places (file and line) of the checks that failed and of the tests
skipped, or the message the program ended with before its summary,
such as a call of a function the built-in DLL doesn't have. The test
fails when the results differ, better or worse, so the change shows in
the file's diff. When it is right, write the results anew:

```sh
CONFORMANCE_BLESS=1 cargo test --test conformance_tests
```

The msvcrt results that aren't clean, and why:

- `environ.c:51` and `file.c:1021`, `file.c:1023`: `system` and the
  `_spawn` functions only fail with ENOENT, as the built-in Windows
  can't start a command interpreter. `system(NULL)` still returns 1,
  as Windows 95's does, since there is one there.
- `dir` and `file` end where they call KERNEL32's file functions
  (`GetCurrentDirectoryA`, `CreateFileA`), which the built-in KERNEL32
  doesn't have yet. The built-in MSVCRT's own file functions use DOS.

## Adding tests

Copy a test file from the same Wine tree (`dlls/<dll>/tests/<file>.c`)
into `wine/<dll>/` once the built-in DLL has the functions it calls
directly: a call of one it lacks ends the program there. Functions it
finds with GetProcAddress may be missing, as on Windows 95; the tests
skip what needs them. Then write the results anew and read the diff of
`expected.txt`: every check that fails there is a difference from
Windows to fix or to explain.
