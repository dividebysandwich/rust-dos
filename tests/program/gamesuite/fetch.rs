//! The games' archives: downloaded once into the cache, checked against
//! their SHA-256, and unpacked.

use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug)]
pub struct Archive {
    /// The name it is kept under in the cache.
    pub file: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
}

/// Where downloads and installed games are kept: `GAME_SUITE_CACHE`, or
/// `~/.cache/rust-dos/game-suite`.
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("GAME_SUITE_CACHE") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("target"));
    home.join(".cache/rust-dos/game-suite")
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    Ok(super::machine::hex(&Sha256::digest(&bytes)))
}

/// The archive in the cache, downloaded if it isn't there yet.
pub fn ensure(archive: &Archive) -> Result<PathBuf, String> {
    let dir = cache_dir().join("dl");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(archive.file);
    if path.is_file() && sha256_file(&path)? == archive.sha256 {
        return Ok(path);
    }
    let partial = dir.join(format!("{}.part", archive.file));
    eprintln!("[game-suite] downloading {}", archive.url);
    let status = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "--max-time", "600", "-o"])
        .arg(&partial)
        .arg(archive.url)
        .status()
        .map_err(|e| format!("can't run curl: {}", e))?;
    if !status.success() {
        let _ = fs::remove_file(&partial);
        return Err(format!("downloading {} failed ({})", archive.url, status));
    }
    let got = sha256_file(&partial)?;
    if got != archive.sha256 {
        let _ = fs::remove_file(&partial);
        return Err(format!("{}: SHA-256 {} where {} was expected", archive.url, got, archive.sha256));
    }
    fs::rename(&partial, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Unpack the zip `zip` into `dest`, leaving out `strip` at the start of
/// the names (a folder the files are in).
pub fn unzip(zip: &Path, dest: &Path, strip: &str) -> Result<(), String> {
    use rust_dos::archive::zip;
    let mut file = fs::File::open(zip).map_err(|e| e.to_string())?;
    for entry in zip::central_directory(&mut file)? {
        let name = entry.name();
        let Some(rest) = name.strip_prefix(strip) else { continue };
        let rest = rest.trim_start_matches('/');
        if rest.is_empty() || rest.split('/').any(|p| p == "..") {
            continue;
        }
        let out = dest.join(rest);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let bytes = zip::read(&mut file, &entry)?;
        fs::write(&out, bytes).map_err(|e| format!("{}: {}", out.display(), e))?;
    }
    Ok(())
}

/// Copy a folder's tree.
pub fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(from).map_err(|e| format!("{}: {}", from.display(), e))?.flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target).map_err(|e| format!("{}: {}", target.display(), e))?;
        }
    }
    Ok(())
}

/// Write a file, by a DOS path relative to `dir`, replacing one there in
/// any case.
pub fn write_nocase(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let mut at = dir.to_path_buf();
    let parts: Vec<&str> = name.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
    for (i, part) in parts.iter().enumerate() {
        let existing = fs::read_dir(&at)
            .ok()
            .and_then(|entries| entries.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part)));
        at = match existing {
            Some(e) => e.path(),
            None => at.join(part),
        };
        if i + 1 < parts.len() {
            fs::create_dir_all(&at).map_err(|e| e.to_string())?;
        }
    }
    fs::write(&at, bytes).map_err(|e| format!("{}: {}", at.display(), e))
}
