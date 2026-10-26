# Win16 conformance tests

Wine's conformance tests ported to Windows 3.1 programs, for the
built-in Windows's Win16 layer. `tests/win16_conformance_tests.rs`
builds each with OpenWatcom (`wcl`, large model, `-bt=windows`) and
runs it under the built-in Windows as Windows 95 would run it. Its
results must be those in `expected.txt`.

## Where they come from

Wine has no Win16 tests for most of these functions, so each file here
is a port of the Win32 test of the same functions, from Wine 1.2.3 (tag
`wine-1.2.3`), the tree `tests/conformance/` comes from:

- `kernel/`: `alloc.c`, `atom.c`, `file.c`, `module.c`, `profile.c`
  from `dlls/kernel32/tests/`, and `dpmi.c`, written here for the INT
  31h services Wine's `dlls/krnl386.exe16/int31.c` gives 16-bit
  programs;
- `user/`: `class.c`, `dialog.c`, `menu.c`, `msg.c`, `text.c`, `win.c`,
  `wsprintf.c` from `dlls/user32/tests/`;
- `gdi/`: `bitmap.c`, `brush.c`, `clipping.c`, `dc.c`, `font.c`,
  `gdiobj.c`, `mapping.c`, `palette.c`, `pen.c` from
  `dlls/gdi32/tests/`.

The comment at the top of each file says which of the Win32 tests it
keeps and what it leaves out: functions Windows 3.1 doesn't have
(threads, SetLastError, the Unicode functions, Windows 95's extended
styles), and checks whose results depend on NT. Where Wine accepts both
Windows 9x's and NT's answers, the 9x branch is the one kept.

`runtime/test.h` is a Win16 version of Wine's `include/wine/test.h`,
which writes the results to DOS's standard output in the form Wine's
tests write them.

## Running them

They need OpenWatcom, found through `WATCOM` (its folder) and `wcl` on
the path, or the compiler `WIN16_CC` names. Without it the test passes
with a note, unless `CONFORMANCE_REQUIRED` is set, as it is in CI,
where `.github/workflows/conformance.yml` installs the snapshot
`expected.txt` was made with.

```sh
cargo test --release --test win16_conformance_tests
CONFORMANCE_ONLY=user/win cargo test --release --test win16_conformance_tests -- --nocapture
CONFORMANCE_BLESS=1 cargo test --release --test win16_conformance_tests
```

## License

The tests are under the LGPL 2.1 or later, as Wine's are, with their
copyright notices kept, which rust-dos's GPL-2.0-or-later allows.
