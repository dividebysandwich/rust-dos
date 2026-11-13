# Vendored crates

## oxideav-indeo

`oxideav-indeo/` is the source of the
[oxideav-indeo](https://github.com/OxideAV/oxideav-indeo) crate at
revision `7fa59e6b6ea951602d428a8fbb74e71723249517`, the head of its
[pull request 8](https://github.com/OxideAV/oxideav-indeo/pull/8). The
files are that revision's, byte for byte: its `Cargo.toml`, `LICENSE`,
`README.md`, `CHANGELOG.md`, `src/`, `benches/` and `examples/`. Its
`tests/` and `fuzz/` folders, which the crate's own packages leave out
too, and its CI files aren't copied. Don't edit the copy: a fix goes
upstream.

The pull request replaces the crate's Indeo 5 decoder with one that
follows Intel's driver, which movies with a four-band luma plane need
(issue 985). The latest release, 0.0.4, doesn't have it, and a git
dependency on the head of an open pull request stops building when the
branch is rewritten, so the tree carries the source until a release
does.

The crate is under the MIT licence (`oxideav-indeo/LICENSE`, copyright
Karpelès Lab Inc.), which allows the copy as long as that file stays
with it.

The root `Cargo.toml` names the folder as a path dependency and keeps it
out of the workspace, so `cargo test --workspace`, clippy and rustfmt
leave it alone. The libretro and web manifests get the crate through
their dependency on `rust-dos`, and the code names it `oxideav_indeo`
as it would the registry's. A `[patch.crates-io]` entry would have to be
repeated in the libretro and web manifests, each a workspace root, and
would leave a dependency line that reads as release 0.0.4, whose Indeo 5
decoder is a different one.

### Going back to the registry's

When the crate is released with the pull request (0.0.5 is expected),
as [issue 1008](https://github.com/dividebysandwich/rust-dos/issues/1008)
tracks:

1. Record the pictures the vendored decoder gives for an Indeo 5 movie:

   ```
   RUST_DOS_AVI=movie.avi cargo test --lib vfw::codec -- --ignored --nocapture
   ```

2. In the root `Cargo.toml`, replace
   `oxideav-indeo = { path = "vendor/oxideav-indeo" }` with
   `oxideav-indeo = "=0.0.5"`, and shorten the comment above it.
3. In the same file, delete `exclude = ["vendor/oxideav-indeo"]` under
   `[workspace]`.
4. `git rm -r vendor`
5. Update the lock files:

   ```
   cargo update -p oxideav-indeo
   cargo update --manifest-path libretro/Cargo.toml -p oxideav-indeo
   cargo update --manifest-path web/Cargo.toml -p oxideav-indeo
   ```

6. Run `cargo test --lib vfw::` and the command of step 1 again, which
   must print the same line.
