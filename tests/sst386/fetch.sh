#!/bin/sh
# Fetch the SingleStepTests/80386 real-mode suite into target/sst386/ for
# the conformance harness in tests/sst386.rs (see docs/cpu-tests.md).
#
# A shallow, sparse, blob-less clone: only the v1_ex_real_mode directory and
# the top-level files (revocation_list.txt, 80386.csv, README.md) are
# downloaded. Re-running the script updates an existing checkout.
#
# Usage: tests/sst386/fetch.sh [destination]   (default: target/sst386)
set -eu

REPO=https://github.com/SingleStepTests/80386.git
DEST=${1:-target/sst386}

if [ -d "$DEST/.git" ]; then
    git -C "$DEST" pull --depth 1 --ff-only
else
    mkdir -p "$(dirname "$DEST")"
    git clone --depth 1 --filter=blob:none --sparse "$REPO" "$DEST"
fi
git -C "$DEST" sparse-checkout set v1_ex_real_mode

echo "Tests in $DEST/v1_ex_real_mode: $(ls "$DEST/v1_ex_real_mode" | wc -l) files"
echo "Run: SST386_DIR=$DEST/v1_ex_real_mode cargo test --release --test sst386 -- --ignored --nocapture"
