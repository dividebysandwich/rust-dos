//! Glide's DOS overlay, GLIDE2X.OVL: the Glide 2 library as a DOS/4G DLL,
//! which games load from their folder or the PATH instead of linking
//! Glide in, such as Tomb Raider's Voodoo Rush version. The one for the
//! Voodoo Graphics drives the emulated card whatever board the game was
//! made for. It is 3dfx's and doesn't come with rust-dos: the settings
//! window fetches it, once the user agrees, from 3dfx's last Voodoo
//! Graphics driver (3.01.00) as the Internet Archive keeps it, and it is
//! then on Z:, which is on the PATH.

use std::io::Cursor;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "GLIDE2X.OVL";

/// The overlay of 3dfx's Voodoo Graphics driver 3.01.00 (Glide 2.46).
const SHA256: &str = "9dadbeb79ddaa23d6125e0a84a4f2aafaec74bf1c2c18a07f5847931d2d42b43";

/// Where the download comes from, for the question.
pub const SOURCE: &str = "archive.org/details/voodoo1-30100_20251011_1715";

/// The driver: a zip of its self-extracting archive (a zip too).
#[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
const URL: &str = "https://archive.org/download/voodoo1-30100_20251011_1715/voodoo1-30100.zip";

/// The biggest archive taken, and looked into.
const LIMIT: u64 = 16 << 20;

/// Where the download is kept, and found.
pub fn path() -> Option<PathBuf> {
    crate::config::user_dir().map(|dir| dir.join(FILE_NAME))
}

/// The overlay, if it is there.
pub fn find() -> Option<PathBuf> {
    path().filter(|p| p.is_file())
}

/// Put the overlay on Z: if it is there and Z: hasn't it yet.
pub fn provide(bus: &mut crate::bus::Bus) {
    let z = format!("Z:\\{}", FILE_NAME);
    if bus.disk.exists(&z) {
        return;
    }
    let Some(path) = find() else { return };
    match std::fs::read(&path) {
        Ok(bytes) => {
            bus.disk.add_virtual_file(FILE_NAME, bytes);
            bus.log_string(&format!("[3DFX] {} is on Z:", FILE_NAME));
        }
        Err(e) => bus.log_string(&format!("[3DFX] {}: {}", path.display(), e)),
    }
}

/// A program looked for `path` and didn't find it: if it is the overlay
/// and Z: hasn't it either, tell the user where to get it, or to put the
/// 3dfx card in, once for the program.
pub fn looked_for(cpu: &mut crate::cpu::Cpu, path: &str) {
    let name = path.rsplit(['\\', '/', ':']).next().unwrap_or(path);
    if !name.eq_ignore_ascii_case(FILE_NAME) || cpu.bus.disk.exists(&format!("Z:\\{}", FILE_NAME)) {
        return;
    }
    let psp = cpu.current_psp;
    if std::mem::replace(&mut cpu.bus.glide_hint, Some(psp)) == Some(psp) {
        return;
    }
    let (title, detail) = if cpu.bus.voodoo.is_none() {
        ("This game wants a 3dfx card", "Turn on 3dfx Voodoo on the settings' Emulator page (Ctrl+F12)")
    } else {
        ("This game needs 3dfx's GLIDE2X.OVL", "Download it on the settings' Emulator page (Ctrl+F12)")
    };
    cpu.bus.log_string(&format!("[3DFX] {}: {}", title, detail));
    cpu.bus.notices.push((title.to_string(), detail.to_string()));
}

/// The overlay in `zip`, looked for in the archives in it too (the driver
/// is a self-extracting archive in a zip), by its hash.
pub fn extract(zip: &[u8]) -> Result<Vec<u8>, String> {
    find_in(zip, 2).ok_or_else(|| format!("the download has no {} of the Voodoo Graphics driver", FILE_NAME))
}

fn find_in(zip: &[u8], depth: u32) -> Option<Vec<u8>> {
    let mut cursor = Cursor::new(zip);
    let entries = crate::archive::zip::central_directory(&mut cursor).ok()?;
    for entry in entries.iter().filter(|e| !e.is_dir() && e.size <= LIMIT) {
        let name = entry.name().to_ascii_lowercase();
        if name.ends_with(".ovl") {
            let bytes = crate::archive::zip::read(&mut cursor, entry).ok()?;
            if crate::sc55::rom::sha256_hex(&bytes) == SHA256 {
                return Some(bytes);
            }
        } else if depth > 0 && (name.ends_with(".zip") || name.ends_with(".exe")) {
            let bytes = crate::archive::zip::read(&mut cursor, entry).ok()?;
            if let Some(found) = find_in(&bytes, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

/// Download the driver and keep its overlay at `dest`, checked against
/// its hash and written to a temporary file first, so a failed download
/// leaves nothing behind. It blocks: run it on a thread of its own.
#[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
pub fn download(dest: &Path) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .user_agent(format!("rust-dos/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let zip = agent
        .get(URL)
        .call()
        .and_then(|mut response| response.body_mut().with_config().limit(LIMIT).read_to_vec())
        .map_err(|e| format!("download failed: {}", e))?;
    let overlay = extract(&zip)?;
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    }
    let temp = dest.with_extension("part");
    std::fs::write(&temp, &overlay).map_err(|e| format!("{}: {}", temp.display(), e))?;
    std::fs::rename(&temp, dest).map_err(|e| format!("{}: {}", dest.display(), e))
}

#[cfg(not(all(feature = "sdl", not(target_arch = "wasm32"))))]
pub fn download(_dest: &Path) -> Result<(), String> {
    Err("downloads are only available in the rust-dos program".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The driver's zip, when a copy is at hand (RUST_DOS_GLIDE_ZIP).
    #[test]
    fn the_overlay_comes_out_of_the_drivers_archive() {
        let Some(zip) = std::env::var_os("RUST_DOS_GLIDE_ZIP").and_then(|p| std::fs::read(p).ok()) else { return };
        let overlay = extract(&zip).unwrap();
        assert_eq!(overlay.len(), 195_815);
    }

    #[test]
    fn other_archives_have_none() {
        assert!(extract(b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0").is_err());
        assert!(extract(b"not a zip").is_err());
    }
}
