//! The VR scenes rust-dos.com lists, and those downloaded from it: the
//! settings window's scene list asks for the list (`CATALOG_URL`) when the
//! user does, and downloads a scene's files into a folder of its own in
//! rust-dos's directory (`vr-scenes/<id>/`), each checked against its size
//! and hash, with what the list said of it beside them (`scene.json`).
//! Downloaded scenes are listed from there, without asking anyone.
//!
//! The list is a JSON file in the website's repository:
//!
//! ```json
//! { "format": 1, "scenes": [ { "id": "bedroom", "name": "90s bedroom",
//!   "version": "2026-10-06", "author": "...", "description": "...",
//!   "license": "...", "homepage": "https://...", "requires": "1.4.0",
//!   "scene": "bedroom.glb",
//!   "files": [ { "path": "bedroom.glb", "url": "https://...",
//!                "size": 15716500, "sha256": "..." } ] } ] }
//! ```
//!
//! `scene` is the file among `files` that rust-dos loads; the others are
//! what it needs (a .gltf's buffers and textures) or goes with it (its
//! credits). A scene that doesn't make sense is left out of the list.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The list of scenes, on the website.
pub const CATALOG_URL: &str = "https://rust-dos.com/vr/scenes.json";

/// The list's format this rust-dos reads.
const FORMAT: u32 = 1;

/// The biggest list taken, a file and a scene.
#[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
const CATALOG_LIMIT: u64 = 1 << 20;
const FILE_LIMIT: u64 = 1 << 30;
const SCENE_LIMIT: u64 = 2 << 30;
const MAX_FILES: usize = 256;

/// What the list said of a downloaded scene, beside its files.
const INFO: &str = "scene.json";

/// A scene as the list gives it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SceneInfo {
    /// Its folder's name: lowercase letters, digits, - and _.
    pub id: String,
    pub name: String,
    /// Whatever tells its releases apart: a different one is an update.
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub homepage: String,
    /// The oldest rust-dos that shows it, if any.
    #[serde(default)]
    pub requires: String,
    /// The file rust-dos loads, among `files`.
    pub scene: String,
    pub files: Vec<SceneFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SceneFile {
    /// Where it goes in the scene's folder, with / between folders.
    pub path: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Deserialize)]
struct Catalog {
    format: u32,
    scenes: Vec<serde_json::Value>,
}

impl SceneInfo {
    /// Whether it makes sense: a folder name, files that stay in it and
    /// come over HTTPS with their sizes and hashes, and a .glb or .gltf
    /// among them to load.
    fn check(&self) -> Result<(), String> {
        let id_ok = (1..=64).contains(&self.id.len())
            && self.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
        if !id_ok {
            return Err(format!("the scene id '{}' isn't a folder name", self.id));
        }
        if self.name.trim().is_empty() {
            return Err(format!("scene '{}' has no name", self.id));
        }
        if self.files.is_empty() || self.files.len() > MAX_FILES {
            return Err(format!("scene '{}' has {} files", self.id, self.files.len()));
        }
        let mut total = 0u64;
        for file in &self.files {
            if !safe_path(&file.path) {
                return Err(format!("scene '{}': the path '{}' isn't one", self.id, file.path));
            }
            if !file.url.starts_with("https://") {
                return Err(format!("scene '{}': {} doesn't come over HTTPS", self.id, file.path));
            }
            if file.size > FILE_LIMIT {
                return Err(format!("scene '{}': {} is too big", self.id, file.path));
            }
            if file.sha256.len() != 64 || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!("scene '{}': {} has no SHA-256", self.id, file.path));
            }
            total += file.size;
        }
        if total > SCENE_LIMIT {
            return Err(format!("scene '{}' is too big", self.id));
        }
        let loads = self.scene.to_ascii_lowercase();
        if !(loads.ends_with(".glb") || loads.ends_with(".gltf")) || !self.files.iter().any(|f| f.path == self.scene) {
            return Err(format!("scene '{}' has no .glb or .gltf file '{}'", self.id, self.scene));
        }
        Ok(())
    }

    /// How many bytes its files are.
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// The newer rust-dos it needs, if this one is too old for it.
    pub fn needs(&self) -> Option<&str> {
        let wanted = parse_version(&self.requires)?;
        (wanted > parse_version(env!("CARGO_PKG_VERSION"))?).then_some(self.requires.as_str())
    }
}

/// A path that stays in the scene's folder: names of letters, digits,
/// . - _ and spaces, none of them . or .. or hidden, and not the file the
/// list's entry is kept in.
fn safe_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    path.len() <= 200
        && parts.len() <= 4
        && path != INFO
        && parts.iter().all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && !part.ends_with(' ')
                && part.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b' '))
        })
}

/// "1.4.0" as numbers, to compare.
fn parse_version(text: &str) -> Option<Vec<u32>> {
    let text = text.trim().trim_start_matches('v');
    (!text.is_empty()).then(|| text.split('.').map(|n| n.parse().ok()).collect::<Option<Vec<u32>>>())?
}

/// The scenes of the list in `text` that make sense, in its order.
pub fn parse_catalog(text: &str) -> Result<Vec<SceneInfo>, String> {
    let catalog: Catalog = serde_json::from_str(text).map_err(|e| format!("the list of scenes can't be read: {}", e))?;
    if catalog.format != FORMAT {
        return Err(format!("the list of scenes is in format {}, which needs a newer rust-dos", catalog.format));
    }
    let mut scenes: Vec<SceneInfo> = Vec::new();
    for value in catalog.scenes {
        let Ok(scene) = serde_json::from_value::<SceneInfo>(value) else { continue };
        if scene.check().is_ok() && !scenes.iter().any(|s| s.id == scene.id) {
            scenes.push(scene);
        }
    }
    Ok(scenes)
}

/// Where downloaded scenes are kept.
pub fn dir() -> Option<PathBuf> {
    crate::config::user_dir().map(|dir| dir.join("vr-scenes"))
}

/// A scene downloaded: what the list said of it, and the file to load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Downloaded {
    pub info: SceneInfo,
    pub path: PathBuf,
}

/// The scenes downloaded, by name.
pub fn downloaded() -> Vec<Downloaded> {
    dir().map(|dir| downloaded_in(&dir)).unwrap_or_default()
}

fn downloaded_in(root: &Path) -> Vec<Downloaded> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut scenes: Vec<Downloaded> = entries
        .flatten()
        .filter_map(|entry| {
            let folder = entry.path();
            let text = std::fs::read_to_string(folder.join(INFO)).ok()?;
            let info: SceneInfo = serde_json::from_str(&text).ok()?;
            info.check().ok()?;
            if entry.file_name().to_str() != Some(info.id.as_str()) {
                return None;
            }
            let path = folder.join(&info.scene);
            path.is_file().then_some(Downloaded { info, path })
        })
        .collect();
    scenes.sort_by_key(|s| s.info.name.to_lowercase());
    scenes
}

/// Delete the downloaded scene `id`.
pub fn delete(id: &str) -> Result<(), String> {
    let root = dir().ok_or("there is no directory for rust-dos's files")?;
    let folder = root.join(id);
    let id_ok = !id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if !id_ok || !folder.join(INFO).is_file() {
        return Err(format!("no scene '{}' is downloaded", id));
    }
    std::fs::remove_dir_all(&folder).map_err(|e| format!("{}: {}", folder.display(), e))
}

#[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
fn agent(timeout: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(timeout)))
        .user_agent(format!("rust-dos/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Ask the website for its list of scenes. It blocks: run it on a thread
/// of its own.
#[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
pub fn fetch_catalog() -> Result<Vec<SceneInfo>, String> {
    let text = agent(30)
        .get(CATALOG_URL)
        .call()
        .and_then(|mut response| response.body_mut().with_config().limit(CATALOG_LIMIT).read_to_string())
        .map_err(|e| format!("rust-dos.com didn't answer: {}", e))?;
    parse_catalog(&text)
}

#[cfg(not(all(feature = "sdl", not(target_arch = "wasm32"))))]
pub fn fetch_catalog() -> Result<Vec<SceneInfo>, String> {
    Err("downloads are only available in the rust-dos program".to_string())
}

/// Download `scene`'s files into its folder, adding the bytes that came
/// to `progress` as they come: the file to load. They go into a hidden
/// folder first, each checked against its size and hash, which then takes
/// the place of the scene's folder, so a failed download leaves the scene
/// as it was. It blocks: run it on a thread of its own.
#[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
pub fn download(scene: &SceneInfo, progress: &std::sync::atomic::AtomicU64) -> Result<PathBuf, String> {
    let root = dir().ok_or("there is no directory for rust-dos's files")?;
    download_into(&root, scene, progress)
}

/// `download`, into `root`'s folders.
#[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
fn download_into(root: &Path, scene: &SceneInfo, progress: &std::sync::atomic::AtomicU64) -> Result<PathBuf, String> {
    use std::io::Read;
    use std::sync::atomic::Ordering;
    scene.check()?;
    std::fs::create_dir_all(root).map_err(|e| format!("{}: {}", root.display(), e))?;
    let temp = root.join(format!(".{}.part", scene.id));
    let _ = std::fs::remove_dir_all(&temp);
    let fail = |e: String| {
        let _ = std::fs::remove_dir_all(&temp);
        e
    };
    let agent = agent(600);
    for file in &scene.files {
        let mut response = agent.get(&file.url).call().map_err(|e| fail(format!("{}: {}", file.path, e)))?;
        let mut reader = response.body_mut().with_config().limit(file.size + 1).reader();
        let mut bytes = Vec::with_capacity(file.size as usize);
        let mut chunk = vec![0; 1 << 16];
        loop {
            let n = reader.read(&mut chunk).map_err(|e| fail(format!("{}: {}", file.path, e)))?;
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..n]);
            progress.fetch_add(n as u64, Ordering::Relaxed);
        }
        if bytes.len() as u64 != file.size || crate::sc55::rom::sha256_hex(&bytes) != file.sha256.to_ascii_lowercase() {
            return Err(fail(format!("{} isn't the file the list names", file.path)));
        }
        let dest = temp.join(&file.path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| fail(format!("{}: {}", parent.display(), e)))?;
        }
        std::fs::write(&dest, &bytes).map_err(|e| fail(format!("{}: {}", dest.display(), e)))?;
    }
    let info = serde_json::to_string_pretty(scene).map_err(|e| fail(e.to_string()))?;
    std::fs::write(temp.join(INFO), info).map_err(|e| fail(format!("{}: {}", INFO, e)))?;
    let folder = root.join(&scene.id);
    if folder.exists() {
        std::fs::remove_dir_all(&folder).map_err(|e| fail(format!("{}: {}", folder.display(), e)))?;
    }
    std::fs::rename(&temp, &folder).map_err(|e| fail(format!("{}: {}", folder.display(), e)))?;
    Ok(folder.join(&scene.scene))
}

#[cfg(not(all(feature = "sdl", not(target_arch = "wasm32"))))]
pub fn download(_scene: &SceneInfo, _progress: &std::sync::atomic::AtomicU64) -> Result<PathBuf, String> {
    Err("downloads are only available in the rust-dos program".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "bb7ca6c0348d47c862857888c2024d4351df63b73eb5c94e04847c9c85bb7c16";

    fn scene(id: &str, path: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id, "name": "A room", "scene": path,
            "files": [{ "path": path, "url": "https://example.com/x", "size": 10, "sha256": HASH }],
        })
    }

    /// The website's list, when a copy is at hand (RUST_DOS_SCENES_JSON):
    /// every scene in it makes sense.
    #[test]
    fn the_sites_list_is_read_whole() {
        let Some(text) = std::env::var_os("RUST_DOS_SCENES_JSON").and_then(|p| std::fs::read_to_string(p).ok()) else {
            return;
        };
        let scenes = parse_catalog(&text).unwrap();
        let listed = serde_json::from_str::<serde_json::Value>(&text).unwrap()["scenes"].as_array().unwrap().len();
        assert_eq!(scenes.len(), listed);
        assert!(scenes.iter().all(|s| s.needs().is_none()), "{:?}", scenes);
    }

    #[test]
    fn a_list_is_read() {
        let mut list = serde_json::json!({ "format": 1, "scenes": [scene("bedroom", "bedroom.glb")] });
        list["scenes"][0]["files"].as_array_mut().unwrap().push(serde_json::json!({
            "path": "credits.md", "url": "https://example.com/c", "size": 5, "sha256": HASH,
        }));
        list["scenes"][0]["license"] = "CC0".into();
        let scenes = parse_catalog(&list.to_string()).unwrap();
        assert_eq!(scenes.len(), 1);
        assert_eq!((scenes[0].id.as_str(), scenes[0].scene.as_str(), scenes[0].license.as_str()), ("bedroom", "bedroom.glb", "CC0"));
        assert_eq!(scenes[0].size(), 15);
    }

    #[test]
    fn scenes_that_make_no_sense_are_left_out() {
        let list = serde_json::json!({ "format": 1, "scenes": [
            scene("good", "room.glb"),
            scene("../up", "room.glb"),
            scene("escape", "../room.glb"),
            scene("absolute", "/etc/room.glb"),
            scene("hidden", ".room.glb"),
            scene("info", "scene.json"),
            scene("model", "room.obj"),
            scene("good", "again.glb"),
            { "id": "no-files", "name": "x", "scene": "a.glb", "files": [] },
            "not even an object",
        ]});
        let scenes = parse_catalog(&list.to_string()).unwrap();
        assert_eq!(scenes.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["good"]);
        assert!(parse_catalog(r#"{"format": 2, "scenes": []}"#).is_err());
        assert!(parse_catalog("<html>").is_err());
    }

    #[test]
    fn files_stay_in_the_scenes_folder() {
        assert!(safe_path("room.glb") && safe_path("textures/wall 1.png"));
        for bad in ["", "..", "a/../b", "/a", "a//b", "C:/a", "a\\b", ".git/x", "a/b/c/d/e"] {
            assert!(!safe_path(bad), "{}", bad);
        }
    }

    #[test]
    fn a_scene_may_need_a_newer_rust_dos() {
        let mut info: SceneInfo = serde_json::from_value(scene("a", "a.glb")).unwrap();
        info.requires = "999.0".into();
        assert_eq!(info.needs(), Some("999.0"));
        info.requires = "1.0.0".into();
        assert_eq!(info.needs(), None);
        info.requires = String::new();
        assert_eq!(info.needs(), None);
    }

    #[test]
    fn downloaded_scenes_are_found_by_their_list_entry() {
        let root = std::env::temp_dir().join(format!("rust-dos-scenes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (folder, id) in [("b", "b"), ("a", "a"), ("wrong", "c")] {
            let dir = root.join(folder);
            std::fs::create_dir_all(&dir).unwrap();
            let mut info: SceneInfo = serde_json::from_value(scene(id, "room.glb")).unwrap();
            info.name = format!("Room {}", id.to_uppercase());
            std::fs::write(dir.join(INFO), serde_json::to_string(&info).unwrap()).unwrap();
            std::fs::write(dir.join("room.glb"), b"glTF").unwrap();
        }
        // Without its file, a scene isn't there.
        std::fs::create_dir_all(root.join("d")).unwrap();
        let info: SceneInfo = serde_json::from_value(scene("d", "room.glb")).unwrap();
        std::fs::write(root.join("d").join(INFO), serde_json::to_string(&info).unwrap()).unwrap();
        let found = downloaded_in(&root);
        assert_eq!(found.iter().map(|s| s.info.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(found[0].path, root.join("a").join("room.glb"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The scenes of a list (RUST_DOS_SCENES_ONLINE: the website's, or a
    /// copy of it), downloaded into a folder of the test's: every file
    /// comes and checks out, and the scenes are then found there.
    #[cfg(all(feature = "sdl", not(target_arch = "wasm32")))]
    #[test]
    fn the_sites_scenes_download() {
        let Some(list) = std::env::var_os("RUST_DOS_SCENES_ONLINE") else { return };
        let scenes = match std::fs::read_to_string(&list) {
            Ok(text) => parse_catalog(&text).unwrap(),
            Err(_) => fetch_catalog().unwrap(),
        };
        assert!(!scenes.is_empty());
        let root = std::env::temp_dir().join(format!("rust-dos-scenes-online-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for scene in &scenes {
            let progress = std::sync::atomic::AtomicU64::new(0);
            let path = download_into(&root, scene, &progress).unwrap();
            assert!(path.is_file(), "{}", path.display());
            assert_eq!(progress.into_inner(), scene.size());
        }
        let found = downloaded_in(&root);
        assert_eq!(found.len(), scenes.len());
        // A file that isn't what the list says leaves nothing behind.
        let mut wrong = scenes[0].clone();
        wrong.id = "wrong".into();
        wrong.files[0].sha256 = "0".repeat(64);
        assert!(download_into(&root, &wrong, &std::sync::atomic::AtomicU64::new(0)).is_err());
        assert!(!root.join("wrong").exists() && !root.join(".wrong.part").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
