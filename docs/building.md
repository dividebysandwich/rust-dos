# Building Rust-DOS

To play games, download Rust-DOS from the
[releases page](https://github.com/dividebysandwich/rust-dos/releases)
instead: the [README](../README.md#start-here) says how to start. This
page is for building it from source, and running its tests and
benchmarks.

## The program

Install Rust and SDL3's development files (`libsdl3-dev` and
`libasound2-dev` on Debian 13 and Ubuntu 25.04 or later, `sdl3` on Arch,
`SDL3-devel` on Fedora, `brew install sdl3` on macOS; older
distributions have none, and need SDL3
[built from source](https://wiki.libsdl.org/SDL3/README-cmake)), then:

```sh
cargo build --release
./target/release/rust-dos -d ~/dos
```

On Windows (MSVC), take SDL3's Visual C++ development package,
`SDL3-devel-<version>-VC.zip` from
[SDL's releases](https://github.com/libsdl-org/SDL/releases) (CI builds
with
[3.4.20](https://github.com/libsdl-org/SDL/releases/download/release-3.4.20/SDL3-devel-3.4.20-VC.zip)),
unpack it, and add its `lib\x64` to `LIB` so the linker finds `SDL3.lib`.
With 3.4.20 unpacked to `C:\`:

```powershell
$env:LIB = "C:\SDL3-3.4.20\lib\x64;$env:LIB"
cargo build --release
```

To run the program, put `SDL3.dll` from the same `lib\x64` next to
`rust-dos.exe` (in `target\release`) or on `PATH`. `cargo test` starts the
debug build in `target\debug` too, so put the DLL on `PATH` for the tests.

## Tests and benchmarks

`cargo test` runs the tests. CI runs them with
[cargo-nextest](https://nexte.st) (`cargo install cargo-nextest --locked`,
then `cargo nextest run`), which runs each test in a process of its own and
times it. The tests in `tests/` are seven programs, one for each folder,
and a test's name starts with its file's:
`cargo nextest run --test dos shell_tests::` runs those in
`tests/dos/shell_tests.rs`. A few tests host a LAN relay or look for one by
broadcast, on all of the computer's network interfaces; if your firewall
asks about them on every run, `cargo test-local` skips those few (with
nextest, add `--features skip-host-network-tests`).
`cargo bench` runs the benchmarks ([benchmarks.md](benchmarks.md)).

## The browser version

`web/` builds Rust-DOS for the browser as a WebAssembly module and a page
that runs it, in `web/www`. It needs the `wasm32-unknown-unknown` Rust target
and the `wasm-bindgen` program in the version `web/Cargo.lock` has (the build
script says which); `wasm-opt` is used if it is installed.

`cd web && cargo test --locked` tests the browser module. With Node.js
installed, `node --test web/tests/shader.test.mjs` tests the page's shader
uploads, overlays and fallback handling without opening a browser.

```sh
rustup target add wasm32-unknown-unknown
./web/build.sh
python3 -m http.server -d web/www
```

Then open http://localhost:8000/. `web/www` is the whole site, for any web
server that serves `.wasm` as `application/wasm`. The build also writes
gzipped copies of the module, `pkg/rust_dos_web_bg.wasm.gz` and
`pkg/rust_dos_web.js.gz`. The page downloads the `.wasm.gz`, about half
the size, and unpacks it itself, with the plain `.wasm` as the fallback,
so a server needs no compression set up. Using the page is described in
[REFERENCE.md](../REFERENCE.md#running-in-a-browser).

## The libretro core

`libretro/` builds Rust-DOS as a [libretro](https://www.libretro.com/)
core, with no SDL needed:

```sh
cd libretro && cargo build --release
retroarch -L target/release/librust_dos_libretro.so ~/games/keen.zip
```

[libretro/README.md](../libretro/README.md) has the details.
