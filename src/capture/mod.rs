//! Captures of the machine: screenshots (PNG), the sound (WAV) and video
//! with sound (AVI), into the capture folder (`capture_dir`) under names
//! that say when they were made.

pub mod avi;
pub mod png;
pub mod wav;
pub mod zmbv;

use std::path::{Path, PathBuf};

/// A new file in `dir` for a capture of `kind` ("screenshot", "sound",
/// "video") with `extension`: rust-dos_<kind>_<date>_<time>.<extension>,
/// with -2, -3, ... when one of that second exists. Creates `dir`.
pub fn capture_path(dir: &Path, kind: &str, extension: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Can't create the capture folder {}: {}", dir.display(), e))?;
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    let name = format!("rust-dos_{}_{}", kind, stamp);
    let mut path = dir.join(format!("{}.{}", name, extension));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{}-{}.{}", name, n, extension));
        n += 1;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_new_files_in_the_folder() {
        let dir = std::env::temp_dir().join(format!("rust-dos-capture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = capture_path(&dir, "screenshot", "png").unwrap();
        assert!(dir.is_dir());
        let name = first.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("rust-dos_screenshot_") && name.ends_with(".png"), "{}", name);
        // Within the same second, the next ones are numbered.
        std::fs::write(&first, b"x").unwrap();
        let second = capture_path(&dir, "screenshot", "png").unwrap();
        assert!(second != first && !second.exists());
        std::fs::write(&second, b"x").unwrap();
        let third = capture_path(&dir, "screenshot", "png").unwrap();
        assert!(third != second && !third.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
