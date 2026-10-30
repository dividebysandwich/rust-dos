//! Sets the configuration flags of the target: the dynamic recompiler's
//! hosts and OpenXR's, records the git commit the build is made from, and
//! links in the Windows program's icon, manifest and version and the
//! exports that put it on a laptop's discrete graphics chip.

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
        println!("cargo::rerun-if-changed=packaging/windows/rust-dos.manifest");
        let out_dir = std::env::var("OUT_DIR").unwrap();
        version_resource(Path::new(&out_dir));
        embed_resource::compile_for("packaging/windows/rust-dos.rc", ["rust-dos"], embed_resource::ParamsIncludeDirs([&out_dir]))
            .manifest_required()
            .unwrap();
        // The discrete graphics chip on a laptop with two: the drivers
        // look for these exports in the .exe (see src/main.rs).
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            for name in ["NvOptimusEnablement", "AmdPowerXpressRequestHighPerformance"] {
                println!("cargo::rustc-link-arg-bin=rust-dos=/EXPORT:{name},DATA");
            }
        }
    }
}

/// Writes version.rc into `out_dir` for packaging/windows/rust-dos.rc to
/// include: the version and names that Explorer shows on the Details tab of
/// rust-dos.exe's properties, from Cargo.toml.
fn version_resource(out_dir: &Path) {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    let numbers = format!(
        "{},{},{},0",
        var("CARGO_PKG_VERSION_MAJOR"),
        var("CARGO_PKG_VERSION_MINOR"),
        var("CARGO_PKG_VERSION_PATCH")
    );
    let version = var("CARGO_PKG_VERSION");
    // Quotes in a resource string are doubled.
    let text = |s: &str| s.replace('"', "\"\"");
    let authors = text(&var("CARGO_PKG_AUTHORS").replace(':', ", "));
    let strings = [
        ("CompanyName", authors.clone()),
        ("FileDescription", "Rust-DOS".to_string()),
        ("FileVersion", text(&version)),
        ("InternalName", "rust-dos".to_string()),
        ("LegalCopyright", format!("{}, GPL-2.0-or-later", authors)),
        ("OriginalFilename", "rust-dos.exe".to_string()),
        ("ProductName", "Rust-DOS".to_string()),
        ("ProductVersion", text(&version)),
        ("Comments", text(&var("CARGO_PKG_DESCRIPTION"))),
    ];
    // An application (FILETYPE 1) for 32-bit Windows (FILEOS 0x40004).
    let mut rc = format!(
        "1 VERSIONINFO\nFILEVERSION {numbers}\nPRODUCTVERSION {numbers}\nFILEFLAGSMASK 0x3F\nFILEFLAGS 0\nFILEOS 0x40004\nFILETYPE 1\nFILESUBTYPE 0\nBEGIN\n  BLOCK \"StringFileInfo\"\n  BEGIN\n    BLOCK \"040904B0\"\n    BEGIN\n"
    );
    for (key, value) in strings {
        rc += &format!("      VALUE \"{}\", \"{}\"\n", key, value);
    }
    // US English, Unicode.
    rc += "    END\n  END\n  BLOCK \"VarFileInfo\"\n  BEGIN\n    VALUE \"Translation\", 0x409, 1200\n  END\nEND\n";
    std::fs::write(out_dir.join("version.rc"), rc).unwrap();
}

/// Sets RUST_DOS_GIT_COMMIT (the full hash of HEAD), RUST_DOS_GIT_COMMIT_SHORT,
/// RUST_DOS_GIT_DIRTY ("true" when a tracked file differs from HEAD) and
/// RUST_DOS_GIT_DESCRIBE ("1a2b3c4" or "1a2b3c4-dirty") for
/// src/build_info.rs. The first three are empty and the last "unknown"
/// when the source is not a git checkout of its own (a crate tarball, a
/// copy, a directory inside some other repository) or git can't be run.
///
/// The script reruns when HEAD moves (a commit, a checkout, a reset), when
/// the index changes, and when a file the program is compiled from changes:
/// src/, crates/, the files src/ includes (assets/, rust-dos.conf.example),
/// the Windows resource and the manifests. A change there rebuilds the
/// crate anyway, so watching it costs no extra link, and it keeps the dirty
/// flag right after an edit that was not staged.
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
    for path in [
        "src",
        "crates",
        "build.rs",
        "assets",
        "rust-dos.conf.example",
        "packaging/windows",
        "Cargo.toml",
        "Cargo.lock",
    ] {
        // Only those that exist: cargo reruns the script at every build for
        // a path that doesn't, and a packaged crate has no crates/.
        if dir.join(path).exists() {
            println!("cargo::rerun-if-changed={}", path);
        }
    }
    for var in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
        println!("cargo::rerun-if-env-changed={}", var);
    }
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
    /// HEAD, HEAD's reflog, the branch's ref, the packed refs and the index
    /// (those that exist: cargo reruns the script at every build for a path
    /// that doesn't).
    watch: Vec<PathBuf>,
}

fn git_state(dir: &Path) -> Option<GitState> {
    // `--no-optional-locks`: `git status` would otherwise refresh and
    // rewrite the index, which is watched, so the next build would rerun
    // the script, and it would take index.lock from under a git command
    // the user runs at the same time.
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git").arg("--no-optional-locks").arg("-C").arg(dir).args(args).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    // Only this crate's own checkout: a tarball unpacked inside another
    // repository would otherwise report that repository's commit.
    let top = git(&["rev-parse", "--show-toplevel"])?;
    if !same_dir(Path::new(&top), dir) {
        return None;
    }
    let commit = git(&["rev-parse", "--verify", "HEAD"])?;
    // 40 digits for SHA-1, 64 in a SHA-256 repository.
    if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let short = git(&["rev-parse", "--short", "HEAD"]).filter(|s| commit.starts_with(s.as_str()))?;
    let dirty = !git(&["status", "--porcelain", "--untracked-files=no"])?.is_empty();
    // In a worktree HEAD and the index are in its own git directory, the
    // refs in the common one; `--git-path` knows which. A relative answer
    // is relative to `dir`. logs/HEAD gets a line at every move of HEAD,
    // also when the branch's ref is only in packed-refs and the move
    // creates its file.
    let branch = git(&["symbolic-ref", "-q", "HEAD"]);
    let mut args = vec!["rev-parse"];
    for name in ["HEAD", "logs/HEAD", "index", "packed-refs"].into_iter().chain(branch.as_deref()) {
        args.extend(["--git-path", name]);
    }
    let watch = git(&args)
        .map(|paths| paths.lines().map(|p| dir.join(p)).filter(|p| p.exists()).collect())
        .unwrap_or_default();
    Some(GitState { commit, short, dirty, watch })
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}
