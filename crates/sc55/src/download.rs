//! Fetching a set of Sound Canvas ROMs from the Internet Archive, which
//! the settings window does only once the user has agreed to it: the
//! firmware is Roland's, and rust-dos doesn't come with it. Only files
//! whose hashes are a set's ROMs are kept, so a changed archive can't
//! slip anything else in.

use std::path::{Path, PathBuf};

use super::rom::{self, Romset};

/// The archive in the item with `romset`'s ROMs.
pub fn archive_name(romset: &Romset) -> Option<&'static str> {
    Some(match romset.name {
        "mk1-v1.00" => "sc55_1.00.zip",
        "mk1-v1.10" => "sc55_1.10.zip",
        "mk1-v1.20" => "sc55_1.20.zip",
        "mk1-v1.21" => "sc55_1.21.zip",
        "mk1-v2.00" => "sc55_2.00.zip",
        "mk2-v1.01" => "sc55mk2_1.01.zip",
        "sc155-rev1" => "sc155_1.00(rev1).zip",
        "cm300-v1.30" => "scc1a_1.30.zip",
        _ => return None,
    })
}

/// The set a download for `model` fetches: the first it accepts that the
/// archive has.
pub fn romset_for(model: &str) -> Option<&'static Romset> {
    rom::candidates(model).into_iter().find(|r| archive_name(r).is_some())
}

/// Download `romset` into `dir`/<set name>, each ROM checked against its
/// hash and written to a temporary file first, so a failed download
/// leaves nothing behind. Returns the folder. It blocks: run it on a
/// thread of its own.
#[cfg(all(feature = "download", not(target_arch = "wasm32")))]
pub fn download(romset: &'static Romset, dir: &Path) -> Result<PathBuf, String> {
    /// The Internet Archive item with a zip of each set.
    const ITEM: &str = "https://archive.org/download/roland-sc-55-series-roms/";
    /// The biggest archive taken.
    const LIMIT: u64 = 16 << 20;
    let name = archive_name(romset).ok_or_else(|| format!("the Internet Archive has no {}", romset.display_name()))?;
    let url = format!("{}{}", ITEM, name.replace('(', "%28").replace(')', "%29"));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .user_agent(format!("rust-dos/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let zip = agent
        .get(&url)
        .call()
        .and_then(|mut response| response.body_mut().with_config().limit(LIMIT).read_to_vec())
        .map_err(|e| format!("download failed: {}", e))?;
    install(romset, &zip, dir)
}

#[cfg(not(all(feature = "download", not(target_arch = "wasm32"))))]
pub fn download(_romset: &'static Romset, _dir: &Path) -> Result<PathBuf, String> {
    Err("downloads are only available in the rust-dos program".to_string())
}

/// Put the ROMs of `romset` in the archive `zip` into `dir`/<set name>.
pub fn install(romset: &'static Romset, zip: &[u8], dir: &Path) -> Result<PathBuf, String> {
    let files = rom::roms_in_zip(zip)?;
    let mut wanted = Vec::new();
    for (location, hash) in romset.roms {
        let (file, bytes) = files
            .iter()
            .find(|(_, bytes)| rom::sha256_hex(bytes) == *hash)
            .ok_or_else(|| format!("the download has no {:?} ROM of {}", location, romset.display_name()))?;
        wanted.push((file.clone(), bytes));
    }
    let dest = dir.join(romset.name);
    std::fs::create_dir_all(&dest).map_err(|e| format!("{}: {}", dest.display(), e))?;
    for (file, bytes) in wanted {
        let path = dest.join(&file);
        let temp = path.with_extension("part");
        std::fs::write(&temp, bytes).map_err(|e| format!("{}: {}", temp.display(), e))?;
        std::fs::rename(&temp, &path).map_err(|e| format!("{}: {}", path.display(), e))?;
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_fetch_sets_the_archive_has() {
        assert_eq!(romset_for("auto").unwrap().name, "mk1-v1.21");
        assert_eq!(romset_for("mk2").unwrap().name, "mk2-v1.01");
        assert_eq!(romset_for("cm300").unwrap().name, "cm300-v1.30");
        assert!(romset_for("st").is_none());
    }

    #[test]
    fn archives_without_the_roms_are_refused() {
        let zip = rust_dos_zip::tests::zip(&[("rom1.bin", &[1; 0x8000], true)]);
        let dir = std::env::temp_dir().join(format!("rust-dos-sc55-install-{}", std::process::id()));
        let set = Romset::by_name("mk2-v1.01").unwrap();
        assert!(install(set, &zip, &dir).is_err());
        assert!(!dir.join(set.name).exists());
    }

    /// Every set the table names comes whole out of the Internet Archive.
    /// Opt-in, as it fetches about 25 MB: RUST_DOS_SC55_DOWNLOAD=<dir>.
    #[test]
    #[cfg(all(feature = "download", not(target_arch = "wasm32")))]
    fn every_archive_has_its_set() {
        let Some(dir) = std::env::var_os("RUST_DOS_SC55_DOWNLOAD") else { return };
        let dir = PathBuf::from(dir);
        for romset in rom::ROMSETS.iter().filter(|r| archive_name(r).is_some()) {
            let dest = download(romset, &dir).unwrap_or_else(|e| panic!("{}: {}", romset.name, e));
            let found = rom::scan(&[dest], romset.name);
            assert_eq!(found.first().map(|f| f.romset.name), Some(romset.name));
        }
    }
}
