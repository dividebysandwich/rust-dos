//! A drive's write overlay: its files as they are below (a folder, or an
//! archive) with the changes made to them, which go to a folder of their
//! own, the upper one. What is below is never written: a file written to
//! is copied up first, and a file deleted below is hidden, by its name in
//! the upper folder's `.rust-dos-deleted`. Deleting the upper folder
//! takes the drive back to what is below.
//!
//! An `Overlay` is a `hostfs` layer (`hostfs::add_layer`): the drive's
//! root is the layer's `layerN:/`, and the drives' code reaches the files
//! through `hostfs` as it does a folder's. Paths in it are taken as
//! `hostfs::layer_path` has them, `A/B.TXT`; "" is the root.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::hostfs::{self, Backend, File, Handle, Meta, Mode, OpenOptions};

/// The upper folder's list of the names deleted below, one path a line.
pub const DELETED: &str = ".rust-dos-deleted";

/// What is below an overlay: read, never written.
pub trait Lower {
    /// "" is the root, which is a folder.
    fn metadata(&self, path: &str) -> io::Result<Meta>;
    /// The names in a folder, and which are folders.
    fn read_dir(&self, path: &str) -> io::Result<Vec<(OsString, bool)>>;
    fn open(&self, path: &str) -> io::Result<Box<dyn Handle>>;
}

/// `path` (`A/B`) under the host folder `root`.
fn join(root: &Path, path: &str) -> PathBuf {
    let mut joined = root.to_path_buf();
    joined.extend(path.split('/').filter(|c| !c.is_empty()));
    joined
}

/// The folder `A/B` is in, "" for one at the root.
fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn child(path: &str, name: &str) -> String {
    if path.is_empty() { name.to_string() } else { format!("{}/{}", path, name) }
}

fn not_found() -> io::Error {
    io::Error::from(io::ErrorKind::NotFound)
}

/// A host folder, below.
pub struct Folder(pub PathBuf);

impl Lower for Folder {
    fn metadata(&self, path: &str) -> io::Result<Meta> {
        hostfs::metadata(join(&self.0, path))
    }

    fn read_dir(&self, path: &str) -> io::Result<Vec<(OsString, bool)>> {
        Ok(hostfs::read_dir(join(&self.0, path))?.into_iter().map(|e| (e.name, e.is_dir)).collect())
    }

    fn open(&self, path: &str) -> io::Result<Box<dyn Handle>> {
        Ok(Box::new(File::open(join(&self.0, path))?))
    }
}

pub struct Overlay {
    lower: Box<dyn Lower>,
    /// Where the changes go; None keeps the drive as it is below, and
    /// refuses writes.
    upper: Option<PathBuf>,
    /// The paths below that were deleted, and everything in them.
    deleted: RefCell<BTreeSet<String>>,
}

impl Overlay {
    /// The overlay of `lower` with the changes in `upper`, which is made
    /// if it isn't there.
    pub fn new(lower: Box<dyn Lower>, upper: Option<PathBuf>) -> io::Result<Overlay> {
        let mut deleted = BTreeSet::new();
        if let Some(dir) = &upper {
            hostfs::create_dir_all(dir)?;
            if let Ok(text) = hostfs::read_to_string(dir.join(DELETED)) {
                deleted.extend(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string));
            }
        }
        Ok(Overlay { lower, upper, deleted: RefCell::new(deleted) })
    }

    fn upper_path(&self, path: &str) -> Option<PathBuf> {
        self.upper.as_ref().map(|dir| join(dir, path))
    }

    /// The upper folder's path for `path`, to write: an error when there
    /// is no upper folder.
    fn writable(&self, path: &str) -> io::Result<PathBuf> {
        self.upper_path(path)
            .ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "the drive is read-only"))
    }

    /// Whether `path` below is hidden: it, or a folder it's in, was deleted.
    fn hidden(&self, path: &str) -> bool {
        let deleted = self.deleted.borrow();
        let mut at = path;
        loop {
            if deleted.contains(at) {
                return true;
            }
            if at.is_empty() {
                return false;
            }
            at = parent(at);
        }
    }

    fn lower_meta(&self, path: &str) -> Option<Meta> {
        if self.hidden(path) { None } else { self.lower.metadata(path).ok() }
    }

    fn upper_meta(&self, path: &str) -> Option<Meta> {
        self.upper_path(path).and_then(|p| hostfs::metadata(p).ok())
    }

    fn meta(&self, path: &str) -> io::Result<Meta> {
        self.upper_meta(path).or_else(|| self.lower_meta(path)).ok_or_else(not_found)
    }

    fn is_dir(&self, path: &str) -> bool {
        self.meta(path).is_ok_and(|m| m.is_dir)
    }

    /// Write the deleted names to the upper folder.
    fn save_deleted(&self) -> io::Result<()> {
        let file = self.writable(DELETED)?;
        let deleted = self.deleted.borrow();
        if deleted.is_empty() {
            return match hostfs::remove_file(&file) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        let text: String = deleted.iter().map(|p| format!("{}\n", p)).collect();
        let partial = file.with_extension("partial");
        hostfs::write(&partial, text)?;
        hostfs::rename(&partial, &file)
    }

    fn hide(&self, path: &str) -> io::Result<()> {
        if self.lower.metadata(path).is_ok() && self.deleted.borrow_mut().insert(path.to_string()) {
            self.save_deleted()?;
        }
        Ok(())
    }

    /// `path`, made again after it was deleted: what was below it stays
    /// hidden.
    fn unhide(&self, path: &str) -> io::Result<()> {
        if !self.deleted.borrow_mut().remove(path) {
            return Ok(());
        }
        if self.lower.metadata(path).is_ok_and(|m| m.is_dir) {
            let names = self.lower.read_dir(path).unwrap_or_default();
            let mut deleted = self.deleted.borrow_mut();
            deleted.extend(names.iter().map(|(name, _)| child(path, &name.to_string_lossy())));
        }
        self.save_deleted()
    }

    /// The folder `path` in the upper folder, with the folders it's in.
    fn upper_dir(&self, path: &str) -> io::Result<()> {
        hostfs::create_dir_all(self.writable(path)?)
    }

    /// `path`, a file below, copied up with its date.
    fn copy_up(&self, path: &str, target: &Path) -> io::Result<()> {
        let mut source = self.lower.open(path)?;
        let mut out = File::create(target)?;
        io::copy(&mut source, &mut out)?;
        out.flush()?;
        if let Some(time) = self.lower.metadata(path).ok().and_then(|m| m.modified) {
            let _ = out.set_modified(time);
        }
        Ok(())
    }

    /// The names in the folder `path`, from above and below; a name above
    /// hides the one below in any case.
    fn names(&self, path: &str) -> io::Result<Vec<(OsString, bool)>> {
        if !self.is_dir(path) {
            return Err(not_found());
        }
        let mut names: Vec<(OsString, bool)> = Vec::new();
        if self.lower_meta(path).is_some_and(|m| m.is_dir) {
            let deleted = self.deleted.borrow();
            names.extend(
                self.lower
                    .read_dir(path)?
                    .into_iter()
                    .filter(|(name, _)| !deleted.contains(&child(path, &name.to_string_lossy()))),
            );
        }
        if let Some(dir) = self.upper_path(path).filter(|p| hostfs::is_dir(p)) {
            for entry in hostfs::read_dir(dir)? {
                if path.is_empty() && entry.name == DELETED {
                    continue;
                }
                let upper = entry.name.to_string_lossy().to_lowercase();
                names.retain(|(name, _)| name.to_string_lossy().to_lowercase() != upper);
                names.push((entry.name, entry.is_dir));
            }
        }
        Ok(names)
    }

    /// `from`, a file or a folder and what's in it, copied to `to` above.
    fn copy_tree(&self, from: &str, to: &str) -> io::Result<()> {
        let target = self.writable(to)?;
        if self.is_dir(from) {
            hostfs::create_dir_all(&target)?;
            self.unhide(to)?;
            for (name, _) in self.names(from)? {
                let name = name.to_string_lossy();
                self.copy_tree(&child(from, &name), &child(to, &name))?;
            }
            return Ok(());
        }
        match self.upper_path(from).filter(|p| hostfs::is_file(p)) {
            Some(source) => {
                hostfs::copy(&source, &target)?;
            }
            None => self.copy_up(from, &target)?,
        }
        self.unhide(to)
    }

    /// `path`, a file or a folder and what's in it, deleted.
    fn remove_tree(&self, path: &str) -> io::Result<()> {
        if self.is_dir(path) {
            for (name, _) in self.names(path)? {
                self.remove_tree(&child(path, &name.to_string_lossy()))?;
            }
            self.remove_dir_at(path)
        } else {
            self.remove_file_at(path)
        }
    }

    fn remove_file_at(&self, path: &str) -> io::Result<()> {
        if self.meta(path)?.is_dir {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "a folder"));
        }
        let upper = self.writable(path)?;
        if hostfs::is_file(&upper) {
            hostfs::remove_file(&upper)?;
        }
        self.hide(path)
    }

    fn remove_dir_at(&self, path: &str) -> io::Result<()> {
        if !self.meta(path)?.is_dir || path.is_empty() {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "not a folder"));
        }
        if !self.names(path)?.is_empty() {
            return Err(io::Error::new(io::ErrorKind::DirectoryNotEmpty, "the folder isn't empty"));
        }
        let upper = self.writable(path)?;
        if hostfs::is_dir(&upper) {
            hostfs::remove_dir(&upper)?;
        }
        self.hide(path)
    }
}

impl Backend for Overlay {
    fn open(&self, path: &Path, mode: Mode) -> io::Result<Box<dyn Handle>> {
        let path = hostfs::layer_path(path);
        if !(mode.write || mode.create || mode.truncate) {
            if let Some(upper) = self.upper_path(&path).filter(|p| hostfs::is_file(p)) {
                return Ok(Box::new(File::open(upper)?));
            }
            return match self.lower_meta(&path) {
                Some(m) if !m.is_dir => self.lower.open(&path),
                _ => Err(not_found()),
            };
        }
        let upper = self.writable(&path)?;
        match self.meta(&path) {
            Ok(m) if m.is_dir => return Err(io::Error::new(io::ErrorKind::PermissionDenied, "a folder")),
            Err(_) if !mode.create => return Err(not_found()),
            _ => {}
        }
        if path.is_empty() || !self.is_dir(parent(&path)) {
            return Err(not_found());
        }
        self.upper_dir(parent(&path))?;
        if !hostfs::exists(&upper) && !mode.truncate && self.lower_meta(&path).is_some() {
            self.copy_up(&path, &upper)?;
        }
        self.unhide(&path)?;
        let file = OpenOptions::new()
            .read(mode.read)
            .write(mode.write)
            .create(mode.create)
            .truncate(mode.truncate)
            .open(upper)?;
        Ok(Box::new(file))
    }

    fn metadata(&self, path: &Path) -> io::Result<Meta> {
        self.meta(&hostfs::layer_path(path))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<(OsString, bool)>> {
        self.names(&hostfs::layer_path(path))
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        let path = hostfs::layer_path(path);
        if self.meta(&path).is_ok() {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        if !self.is_dir(parent(&path)) {
            return Err(not_found());
        }
        hostfs::create_dir_all(self.writable(&path)?)?;
        self.unhide(&path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.remove_file_at(&hostfs::layer_path(path))
    }

    fn remove_dir(&self, path: &Path) -> io::Result<()> {
        self.remove_dir_at(&hostfs::layer_path(path))
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let (from, to) = (hostfs::layer_path(from), hostfs::layer_path(to));
        self.meta(&from)?;
        if self.meta(&to).is_ok() {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        if from.is_empty() || to.is_empty() || !self.is_dir(parent(&to)) {
            return Err(not_found());
        }
        let (source, target) = (self.writable(&from)?, self.writable(&to)?);
        self.upper_dir(parent(&to))?;
        // Nothing of it below: the upper folder's own rename.
        if self.lower_meta(&from).is_none() {
            hostfs::rename(&source, &target)?;
            return self.unhide(&to);
        }
        self.copy_tree(&from, &to)?;
        self.remove_tree(&from)
    }

    fn set_readonly(&self, path: &Path, readonly: bool) -> io::Result<()> {
        let path = hostfs::layer_path(path);
        let meta = self.meta(&path)?;
        let upper = self.writable(&path)?;
        if !hostfs::exists(&upper) {
            if meta.is_dir {
                self.upper_dir(&path)?;
            } else {
                self.upper_dir(parent(&path))?;
                self.copy_up(&path, &upper)?;
            }
        }
        hostfs::set_readonly(&upper, readonly)
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-overlay").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("lower/SAVES")).unwrap();
        std::fs::write(dir.join("lower/GAME.EXE"), b"game").unwrap();
        std::fs::write(dir.join("lower/SAVES/SLOT1.SAV"), b"one").unwrap();
        dir
    }

    fn layer(dir: &Path) -> hostfs::Layer {
        let overlay = Overlay::new(Box::new(Folder(dir.join("lower"))), Some(dir.join("upper"))).unwrap();
        hostfs::add_layer(Arc::new(overlay))
    }

    fn names(path: PathBuf) -> Vec<String> {
        let mut names: Vec<String> =
            hostfs::read_dir(path).unwrap().into_iter().map(|e| e.name.to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn writes_go_above_and_leave_what_is_below() {
        let dir = scratch("writes");
        let layer = layer(&dir);
        let root = layer.root().to_path_buf();
        let mut file = OpenOptions::new().read(true).write(true).open(root.join("SAVES/SLOT1.SAV")).unwrap();
        file.write_all(b"ONE").unwrap();
        drop(file);
        hostfs::write(root.join("SAVES/SLOT2.SAV"), b"two").unwrap();
        assert_eq!(hostfs::read(root.join("SAVES/SLOT1.SAV")).unwrap(), b"ONE");
        assert_eq!(std::fs::read(dir.join("lower/SAVES/SLOT1.SAV")).unwrap(), b"one");
        assert_eq!(std::fs::read(dir.join("upper/SAVES/SLOT2.SAV")).unwrap(), b"two");
        assert!(!dir.join("lower/SAVES/SLOT2.SAV").exists());
        assert_eq!(names(root.join("SAVES")), ["SLOT1.SAV", "SLOT2.SAV"]);
        assert_eq!(names(root.clone()), ["GAME.EXE", "SAVES"]);
        assert_eq!(hostfs::metadata(root.join("GAME.EXE")).unwrap().len, 4);
    }

    #[test]
    fn deleted_files_stay_deleted_until_made_again() {
        let dir = scratch("deletes");
        {
            let layer = layer(&dir);
            hostfs::remove_file(layer.root().join("GAME.EXE")).unwrap();
            assert!(!hostfs::exists(layer.root().join("GAME.EXE")));
            assert!(dir.join("lower/GAME.EXE").exists());
        }
        // Again, from the upper folder's list.
        let layer = layer(&dir);
        let root = layer.root().to_path_buf();
        assert_eq!(names(root.clone()), ["SAVES"]);
        hostfs::write(root.join("GAME.EXE"), b"new").unwrap();
        assert_eq!(hostfs::read(root.join("GAME.EXE")).unwrap(), b"new");
        assert!(!dir.join("upper").join(DELETED).exists());
    }

    #[test]
    fn a_folder_made_again_is_empty() {
        let dir = scratch("folders");
        let layer = layer(&dir);
        let root = layer.root().to_path_buf();
        assert!(hostfs::remove_dir(root.join("SAVES")).is_err(), "not empty");
        hostfs::remove_file(root.join("SAVES/SLOT1.SAV")).unwrap();
        hostfs::remove_dir(root.join("SAVES")).unwrap();
        assert!(!hostfs::is_dir(root.join("SAVES")));
        hostfs::create_dir(root.join("SAVES")).unwrap();
        assert!(names(root.join("SAVES")).is_empty());
        assert!(!hostfs::exists(root.join("SAVES/SLOT1.SAV")));
    }

    #[test]
    fn renames_copy_up() {
        let dir = scratch("renames");
        let layer = layer(&dir);
        let root = layer.root().to_path_buf();
        hostfs::rename(root.join("SAVES"), root.join("OLD")).unwrap();
        assert_eq!(names(root.clone()), ["GAME.EXE", "OLD"]);
        assert_eq!(hostfs::read(root.join("OLD/SLOT1.SAV")).unwrap(), b"one");
        assert!(dir.join("lower/SAVES/SLOT1.SAV").exists());
        hostfs::write(root.join("NEW.TXT"), b"x").unwrap();
        hostfs::rename(root.join("NEW.TXT"), root.join("OLD/NEW.TXT")).unwrap();
        assert_eq!(names(root.join("OLD")), ["NEW.TXT", "SLOT1.SAV"]);
    }

    #[test]
    fn without_an_upper_folder_nothing_is_written() {
        let dir = scratch("readonly");
        let overlay = Overlay::new(Box::new(Folder(dir.join("lower"))), None).unwrap();
        let layer = hostfs::add_layer(Arc::new(overlay));
        assert_eq!(hostfs::read(layer.root().join("GAME.EXE")).unwrap(), b"game");
        assert!(hostfs::write(layer.root().join("NEW.TXT"), b"x").is_err());
        assert!(hostfs::remove_file(layer.root().join("GAME.EXE")).is_err());
    }

    #[test]
    fn names_joined_to_the_parent_of_a_file_at_the_root_reach_the_layer() {
        // On Windows the parent of `layerN:/GAME.EXE` is `layerN:`, and a
        // name joined to it is `layerN:\SAVES`.
        let dir = scratch("joined");
        let layer = layer(&dir);
        let parent = layer.root().join("GAME.EXE").parent().unwrap().to_path_buf();
        assert_eq!(hostfs::read(parent.join("GAME.EXE")).unwrap(), b"game");
        assert!(hostfs::is_dir(parent.join("SAVES")));
        assert_eq!(names(parent.clone()), ["GAME.EXE", "SAVES"]);
        let backslashed = PathBuf::from(format!("{:?}:\\SAVES\\SLOT1.SAV", layer));
        assert_eq!(hostfs::read(&backslashed).unwrap(), b"one");
        hostfs::write(PathBuf::from(format!("{:?}:\\NEW.TXT", layer)), b"x").unwrap();
        assert_eq!(std::fs::read(dir.join("upper/NEW.TXT")).unwrap(), b"x");
    }
}
