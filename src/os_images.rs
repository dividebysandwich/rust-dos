//! Operating systems installed on hard disk images, by name: `IMGMOUNT C
//! WIN98SE` mounts `win98se.img` from the OS images folders. A game's
//! profile puts
//! the image's changes in its own delta file (`games::overlay_drives`), so
//! one install serves every game unchanged.

use crate::hostfs;
use std::path::PathBuf;

/// The images' extensions, in the order a name is looked for.
const EXTENSIONS: [&str; 3] = ["img", "vhd", "ima"];

static SEARCH_DIRS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Look for images in `dir` too, before the usual places.
pub fn add_search_dir(dir: PathBuf) {
    let mut dirs = SEARCH_DIRS.lock().unwrap_or_else(|e| e.into_inner());
    if !dirs.contains(&dir) {
        dirs.push(dir);
    }
}

/// The folders the images are in: the frontend's, then `os` in rust-dos's
/// own folder.
pub fn dirs() -> Vec<PathBuf> {
    let mut dirs = SEARCH_DIRS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    dirs.extend(crate::config::user_dir().map(|d| d.join("os")));
    dirs
}

/// The image called `name`, with or without its extension; a path is no
/// name.
pub fn find(name: &str) -> Option<PathBuf> {
    find_in(&dirs(), name)
}

fn find_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\', ':', '*', '?']) {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    let images = list_in(dirs);
    let named = |ext: Option<&str>| {
        images.iter().find(|(stem, path)| {
            let file_ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
            match ext {
                Some(ext) => *stem == lower && file_ext.as_deref() == Some(ext),
                None => path.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name)),
            }
        })
    };
    named(None).or_else(|| EXTENSIONS.iter().find_map(|ext| named(Some(ext)))).map(|(_, path)| path.clone())
}

/// The images there are, by name in lower case, the first folder's where
/// two have one.
pub fn list() -> Vec<(String, PathBuf)> {
    list_in(&dirs())
}

fn list_in(dirs: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut images: Vec<(String, PathBuf)> = Vec::new();
    for dir in dirs {
        let mut found: Vec<(String, PathBuf)> = hostfs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter(|e| !e.is_dir)
            .filter(|e| e.path.extension().is_some_and(|x| EXTENSIONS.iter().any(|ext| x.eq_ignore_ascii_case(ext))))
            .filter_map(|e| Some((e.path.file_stem()?.to_string_lossy().to_ascii_lowercase(), e.path)))
            .collect();
        found.sort();
        images.extend(found);
    }
    images
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_found_by_name() {
        let root = PathBuf::from("target/test_os_images");
        let _ = std::fs::remove_dir_all(&root);
        let (own, other) = (root.join("os"), root.join("more"));
        std::fs::create_dir_all(&own).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(own.join("Win98SE.img"), b"").unwrap();
        std::fs::write(other.join("win98se.vhd"), b"").unwrap();
        std::fs::write(other.join("Win95.VHD"), b"").unwrap();
        std::fs::write(other.join("notes.txt"), b"").unwrap();
        let dirs = [own.clone(), other.clone()];
        assert_eq!(find_in(&dirs, "win98se"), Some(own.join("Win98SE.img")), "the first folder's, .img first");
        assert_eq!(find_in(&dirs, "WIN98SE.VHD"), Some(other.join("win98se.vhd")));
        assert_eq!(find_in(&dirs, "win95"), Some(other.join("Win95.VHD")));
        assert_eq!(find_in(&dirs, "notes"), None);
        assert_eq!(find_in(&dirs, "dos/win95"), None, "a path isn't a name");
        let names: Vec<String> = list_in(&dirs).into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["win98se", "win95", "win98se"]);
    }
}
