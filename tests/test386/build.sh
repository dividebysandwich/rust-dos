#!/bin/sh
# Fetch and assemble test386.asm (github.com/barotto/test386.asm) into
# target/test386/ for the conformance harness in tests/test386.rs (see
# docs/cpu-tests.md). Needs git and nasm.
#
# The ROM is configured for the emulator: POST codes on port 190h, text
# on port E9h, the 128 KB ROM with the task switch tests, and the 80386's
# undefined behaviour tested.
#
# Usage: tests/test386/build.sh [destination]   (default: target/test386)
set -eu

REPO=https://github.com/barotto/test386.asm.git
DEST=${1:-target/test386}

if [ -d "$DEST/.git" ]; then
    git -C "$DEST" pull --depth 1 --ff-only
else
    mkdir -p "$(dirname "$DEST")"
    git clone --depth 1 "$REPO" "$DEST"
fi

CONF="$DEST/src/configuration.asm"
git -C "$DEST" checkout -- src
# NASM 3 rejects "mov [mem], word imm"; it wants the size on the memory
# operand.
sed -i -E 's/\bmov(\s+)\[([^]]*)\],\s*(byte|word|dword)\s+/mov\1\3 [\2], /' "$DEST"/src/*.asm "$DEST"/src/tests/*.asm
sed -i \
    -e 's/^POST_PORT equ .*/POST_PORT equ 0x190/' \
    -e 's/^OUT_PORT equ .*/OUT_PORT equ 0xE9/' \
    -e 's/^TEST_UNDEF equ .*/TEST_UNDEF equ 1/' \
    -e 's/^CPU_FAMILY equ .*/CPU_FAMILY equ 3/' \
    -e 's/^ROM128 equ .*/ROM128 equ 1/' \
    "$CONF"

(cd "$DEST" && nasm -i./src/ -f bin src/test386.asm -w-all -l test386.lst -o test386.bin)

echo "Built $DEST/test386.bin ($(wc -c < "$DEST/test386.bin") bytes)"
echo "Run: TEST386_DIR=$DEST cargo test --release --test test386 -- --ignored --nocapture"
