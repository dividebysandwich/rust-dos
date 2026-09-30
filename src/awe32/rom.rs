//! The AWE32's 1 MB sample ROM (`awe32.raw`): 512K little-endian 16-bit
//! words holding the General MIDI sounds. It is Creative's, so rust-dos
//! doesn't ship it: it is looked for on disk, and can be downloaded from
//! the copy the libretro PCem core keeps, which is checked by its hash.
//! Without it the chip works and plays what programs load into its RAM;
//! only the ROM sounds are silent.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::ROM_WORDS;

pub const FILE_NAME: &str = "awe32.raw";

/// Where the download comes from, and the SHA-256 it has to have.
pub const DOWNLOAD_URL: &str = "https://raw.githubusercontent.com/libretro/libretro-pcem/master/awe32.raw";
const SHA256: &str = "4e143b94f758734f594ded78f4e5115635975c16fa37fabeae0baa4938ca710e";

const ROM_BYTES: usize = 2 * ROM_WORDS;

/// Directories a frontend adds to the search, such as a libretro
/// frontend's system directory.
static SEARCH_DIRS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Look for the ROM in `dir` too, before the usual places.
pub fn add_search_dir(dir: PathBuf) {
    let mut dirs = SEARCH_DIRS.lock().unwrap_or_else(|e| e.into_inner());
    if !dirs.contains(&dir) {
        dirs.push(dir);
    }
}

/// Where the ROM is looked for without an `awe32rom` setting: the
/// frontend's directories, rust-dos's own directory (where the download
/// goes), an AWE32ROM directory in it as DOSBox-X has it, and the working
/// directory.
pub fn default_paths() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> =
        SEARCH_DIRS.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|dir| dir.join(FILE_NAME)).collect();
    if let Some(user) = crate::config::user_dir() {
        paths.push(user.join(FILE_NAME));
        paths.push(user.join("AWE32ROM").join(FILE_NAME));
    }
    paths.push(PathBuf::from(FILE_NAME));
    paths.push(PathBuf::from("AWE32ROM").join(FILE_NAME));
    if cfg!(unix) {
        paths.push(PathBuf::from("/usr/share/awe32").join(FILE_NAME));
    }
    paths
}

/// Where the download is saved.
pub fn download_path() -> Option<PathBuf> {
    crate::config::user_dir().map(|dir| dir.join(FILE_NAME))
}

/// The ROM to use: the configured file (a directory is searched for
/// `awe32.raw`), else the first of the default places that has one.
pub fn find(configured: Option<&Path>) -> Option<PathBuf> {
    match configured {
        Some(path) if path.is_dir() => Some(path.join(FILE_NAME)).filter(|p| p.is_file()),
        Some(path) => Some(path.to_path_buf()).filter(|p| p.is_file()),
        None => default_paths().into_iter().find(|p| p.is_file()),
    }
}

/// The ROM's words from the bytes of a file: a 1 MB image, or the dump
/// AWE-DUMP makes, which is shifted by one word (and may lack the last).
pub fn parse(bytes: &[u8]) -> Result<Arc<[i16]>, String> {
    if bytes.len() != ROM_BYTES && bytes.len() != ROM_BYTES - 2 {
        return Err(format!("{} bytes, not the 1 MB of an AWE32 ROM", bytes.len()));
    }
    let mut words: Vec<i16> = bytes.as_chunks::<2>().0.iter().map(|&b| i16::from_le_bytes(b)).collect();
    words.resize(ROM_WORDS, 0);
    if words[3] == 0x314D && words[4] == 0x474D {
        words.remove(0);
        words.push(0);
    }
    Ok(words.into())
}

pub fn load(path: &Path) -> Result<Arc<[i16]>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    parse(&bytes).map_err(|e| format!("{}: {}", path.display(), e))
}

/// Whether `bytes` are the ROM the download should give.
pub fn verify(bytes: &[u8]) -> bool {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(bytes);
    let hex: String = hash.iter().map(|b| format!("{:02x}", b)).collect();
    bytes.len() == ROM_BYTES && hex == SHA256
}

/// Download the ROM to `dest`, checked against its hash, written to a
/// temporary file first so a failed download leaves nothing behind. It
/// blocks: run it on a thread of its own.
#[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
pub fn download(dest: &Path) -> Result<(), String> {
    use std::time::Duration;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .user_agent(format!("rust-dos/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let bytes = agent
        .get(DOWNLOAD_URL)
        .call()
        .and_then(|mut response| response.body_mut().with_config().limit(2 * ROM_BYTES as u64).read_to_vec())
        .map_err(|e| format!("download failed: {}", e))?;
    if !verify(&bytes) {
        return Err("the downloaded file is not the AWE32 ROM (its checksum differs)".to_string());
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    }
    let temp = dest.with_extension("part");
    std::fs::write(&temp, &bytes).map_err(|e| format!("{}: {}", temp.display(), e))?;
    std::fs::rename(&temp, dest).map_err(|e| format!("{}: {}", dest.display(), e))
}

#[cfg(not(all(feature = "sdl", not(target_arch = "wasm32"))))]
pub fn download(_dest: &Path) -> Result<(), String> {
    Err("downloads are only available in the rust-dos program".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_of_the_wrong_size_are_refused() {
        assert!(parse(&[0; 1000]).is_err());
        assert_eq!(parse(&vec![0; ROM_BYTES]).unwrap().len(), ROM_WORDS);
    }

    #[test]
    fn awe_dump_images_are_shifted_back() {
        let mut bytes = vec![0u8; ROM_BYTES];
        bytes[2..4].copy_from_slice(&0x1234u16.to_le_bytes());
        bytes[6..8].copy_from_slice(&0x314Du16.to_le_bytes());
        bytes[8..10].copy_from_slice(&0x474Du16.to_le_bytes());
        let rom = parse(&bytes).unwrap();
        assert_eq!(rom[0], 0x1234);
        assert_eq!(rom[2], 0x314D);
        assert_eq!(rom.len(), ROM_WORDS);
    }

    #[test]
    fn only_the_right_rom_verifies() {
        assert!(!verify(&vec![0; ROM_BYTES]));
    }
}
