//! Sets the configuration flags of the target: the dynamic recompiler's
//! hosts and OpenXR's, records the git commit the build is made from, and
//! links in the Windows program's icon.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    git_commit();
    // The hosts the dynamic recompiler (src/dynrec) generates code for.
    println!("cargo::rustc-check-cfg=cfg(dynrec)");
    if matches!(std::env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("x86_64" | "aarch64")) {
        println!("cargo::rustc-cfg=dynrec");
    }
    // VR headsets through OpenXR, which the `vr` feature has where there
    // are OpenXR runtimes to talk to: Linux and Windows.
    println!("cargo::rustc-check-cfg=cfg(xr)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if std::env::var_os("CARGO_FEATURE_VR").is_some() && matches!(os.as_str(), "linux" | "windows") {
        println!("cargo::rustc-cfg=xr");
    }
    // The relay program uses nothing of SDL, which the library links. The
    // GNU linker leaves such a library out by itself; macOS's keeps it
    // unless told otherwise, and the relay would want SDL2 to start.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo::rustc-link-arg-bin=rust-dos-relay=-Wl,-dead_strip_dylibs");
    }
    // The program's icon in rust-dos.exe, for Explorer, shortcuts and the
    // taskbar; the window's own is set at run time (src/display.rs). Only
    // with `sdl`, which the program needs: the libretro core has neither.
    if os == "windows" && std::env::var_os("CARGO_FEATURE_SDL").is_some() {
        println!("cargo::rerun-if-changed=packaging/windows/rust-dos.ico");
        embed_resource::compile_for("packaging/windows/rust-dos.rc", ["rust-dos"], embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }
}

/// Sets RUST_DOS_GIT_COMMIT (the full hash of HEAD), RUST_DOS_GIT_COMMIT_SHORT,
/// RUST_DOS_GIT_DIRTY ("true" when a tracked file differs from HEAD) and
/// RUST_DOS_GIT_DESCRIBE ("1a2b3c4" or "1a2b3c4-dirty") for
/// src/build_info.rs. The first three are empty and the last "unknown" when the source is not a git checkout of its own (a
/// crate tarball, a copy, a directory inside some other repository) or
/// git can't be run.
///
/// The script reruns when HEAD moves (a commit, a checkout), when the index
/// changes, and when a file under src/ or crates/ changes. A change there
/// rebuilds the crate anyway, so watching it costs no extra link, and it
/// keeps the dirty flag right after an edit that was not staged.
fn git_commit() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let (commit, short, dirty) = match git_state(&dir) {
        Some(GitState { commit, short, dirty, watch }) => {
            for path in watch {
                println!("cargo::rerun-if-changed={}", path.display());
            }
            (commit, short, dirty.to_string())
        }
        None => Default::default(),
    };
    for path in ["src", "crates", "build.rs"] {
        println!("cargo::rerun-if-changed={}", path);
    }
    println!("cargo::rerun-if-env-changed=GIT_DIR");
    println!("cargo::rustc-env=RUST_DOS_GIT_COMMIT={}", commit);
    println!("cargo::rustc-env=RUST_DOS_GIT_COMMIT_SHORT={}", short);
    println!("cargo::rustc-env=RUST_DOS_GIT_DIRTY={}", dirty);
    let describe = match (short.as_str(), dirty.as_str()) {
        ("", _) => "unknown".to_string(),
        (short, "true") => format!("{}-dirty", short),
        (short, _) => short.to_string(),
    };
    println!("cargo::rustc-env=RUST_DOS_GIT_DESCRIBE={}", describe);
}

struct GitState {
    /// HEAD's full hash.
    commit: String,
    /// HEAD's hash as `git rev-parse --short` gives it.
    short: String,
    /// Whether a tracked file differs from HEAD.
    dirty: bool,
    /// The files whose change means any of the above may have changed:
    /// HEAD, the branch's ref, the packed refs and the index (those that
    /// exist: cargo reruns the script at every build for a path that
    /// doesn't).
    watch: Vec<PathBuf>,
}

fn git_state(dir: &Path) -> Option<GitState> {
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    // Only this crate's own checkout: a tarball unpacked inside another
    // repository would otherwise report that repository's commit.
    let top = git(&["rev-parse", "--show-toplevel"])?;
    if !same_dir(Path::new(&top), dir) {
        return None;
    }
    let commit = git(&["rev-parse", "--verify", "HEAD"])?;
    if commit.len() != 40 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let short = git(&["rev-parse", "--short", "HEAD"]).filter(|s| commit.starts_with(s.as_str()))?;
    let dirty = !git(&["status", "--porcelain", "--untracked-files=no"])?.is_empty();
    // In a worktree HEAD and the index are in its own git directory, the
    // refs in the common one; `--git-path` knows which. A relative answer
    // is relative to `dir`.
    let path = |p: &str| git(&["rev-parse", "--git-path", p]).map(|p| dir.join(p));
    let mut names = vec!["HEAD".to_string(), "index".to_string(), "packed-refs".to_string()];
    names.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    let watch = names.iter().filter_map(|p| path(p)).filter(|p| p.exists()).collect();
    Some(GitState { commit, short, dirty, watch })
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
