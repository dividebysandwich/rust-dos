#!/bin/sh
# Build Rust-DOS for the browser: the emulator as WebAssembly in web/www/pkg,
# next to the page that runs it. web/www is then the whole site; serve it
# with any web server, e.g.
#
#   ./web/build.sh && python3 -m http.server -d web/www
#
# Needs the wasm32-unknown-unknown target (rustup target add
# wasm32-unknown-unknown) and the wasm-bindgen program in the version
# web/Cargo.lock has. Runs wasm-opt from binaryen, if it is installed, to
# make the module smaller and faster.
set -eu
cd "$(dirname "$0")"

version=$(sed -n '/^name = "wasm-bindgen"$/{n;s/^version = "\(.*\)"$/\1/p;}' Cargo.lock)
installed=$(wasm-bindgen --version 2>/dev/null | cut -d' ' -f2 || true)
if [ "$installed" != "$version" ]; then
    echo "error: this needs wasm-bindgen $version, found ${installed:-none}." >&2
    echo "Install it with: cargo install wasm-bindgen-cli --version $version --locked" >&2
    exit 1
fi

cargo build --release --locked --target wasm32-unknown-unknown
wasm-bindgen --target web --no-typescript --out-dir www/pkg \
    target/wasm32-unknown-unknown/release/rust_dos_web.wasm
if command -v wasm-opt >/dev/null 2>&1; then
    wasm-opt -O3 www/pkg/rust_dos_web_bg.wasm -o www/pkg/rust_dos_web_bg.wasm
else
    echo "wasm-opt not found: the module is left as the compiler made it." >&2
fi
echo "Built web/www. Serve it with, for example: python3 -m http.server -d web/www"
