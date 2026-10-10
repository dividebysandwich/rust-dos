//! The host directories of rust-dos: the user's home and configuration
//! directories, and rust-dos's own directory, where it keeps its log,
//! shell history, downloaded ROMs and the like.
//!
//! The `dirs` crate finds the home and configuration directories, but on
//! Windows it asks the shell and frees the answer with combase.dll's
//! CoTaskMemFree, and Windows 7 has no combase.dll, so rust-dos wouldn't
//! start there. On Windows they come from the
//! environment instead, which Windows gives every program: the profile
//! (USERPROFILE) and the roaming application data (APPDATA), the folders
//! `dirs` finds.

use std::path::{Path, PathBuf};

/// The name of rust-dos's configuration file.
pub const FILE_NAME: &str = "rust-dos.conf";

/// The user's home directory.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    return std::env::home_dir();
    #[cfg(not(windows))]
    return dirs::home_dir();
}

/// Where the user's programs keep their settings: `~/.config` on Linux,
/// `~/Library/Application Support` on macOS, and `%APPDATA%` on Windows.
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    return std::env::var_os("APPDATA").filter(|dir| !dir.is_empty()).map(PathBuf::from);
    #[cfg(not(windows))]
    return dirs::config_dir();
}

/// rust-dos's own directory, for its log, shell history, downloaded ROMs
/// and the like: the executable's directory in a portable install (one with
/// a `rust-dos.conf` beside the executable), else the per-user one, e.g.
/// `~/.config/rust-dos` on Linux.
pub fn user_dir() -> Option<PathBuf> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| user_dir_for(exe_dir().as_deref(), profile_dir())).clone()
}

/// The per-user directory, e.g. `~/.config/rust-dos` on Linux.
pub fn profile_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("rust-dos"))
}

/// `exe_dir` if it holds a `rust-dos.conf` (a portable install), else
/// `profile`.
pub fn user_dir_for(exe_dir: Option<&Path>, profile: Option<PathBuf>) -> Option<PathBuf> {
    match exe_dir {
        Some(dir) if dir.join(FILE_NAME).is_file() => Some(dir.to_path_buf()),
        _ => profile,
    }
}

/// The directory holding the rust-dos executable, where a
/// `rust-dos.conf` makes the install portable.
pub fn exe_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    exe.parent().map(Path::to_path_buf)
}
