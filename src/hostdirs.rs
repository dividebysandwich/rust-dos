//! The host user's home directory and configuration directory. The `dirs`
//! crate finds them, but on Windows it asks the shell and frees the answer
//! with combase.dll's CoTaskMemFree, and Windows 7 has no combase.dll, so
//! rust-dos wouldn't start there. On Windows they come from the
//! environment instead, which Windows gives every program: the profile
//! (USERPROFILE) and the roaming application data (APPDATA), the folders
//! `dirs` finds.

use std::path::PathBuf;

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
