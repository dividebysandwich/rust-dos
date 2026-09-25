//! Picking a host directory or file in the settings window.

use std::fs;
use std::path::{Path, PathBuf};

/// The extensions of the disk and CD images a drive can show.
pub const IMAGES: &[&str] = &["cue", "iso", "bin", "img", "ima", "vfd", "flp", "dsk"];
pub const SOUNDFONTS: &[&str] = &["sf2"];
pub const MT32_ROMS: &[&str] = &["rom", "bin"];

pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

/// A listing of one host directory: its parent, subdirectories and the
/// files with the wanted extensions.
pub struct Browser {
    pub title: &'static str,
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    /// Whether the directory itself can be picked: then the first row is
    /// "Use this directory".
    pub pick_dirs: bool,
    extensions: &'static [&'static str],
    pub selected: usize,
}

/// A row of the listing.
pub enum Row<'a> {
    UseThisDirectory,
    Entry(&'a Entry),
}

impl Browser {
    /// A browser in `start`, or the nearest directory above it that can be
    /// listed.
    pub fn new(title: &'static str, start: &Path, pick_dirs: bool, extensions: &'static [&'static str]) -> Self {
        let mut browser =
            Self { title, dir: PathBuf::new(), entries: Vec::new(), pick_dirs, extensions, selected: 0 };
        for dir in start.ancestors() {
            if browser.load(dir).is_ok() {
                break;
            }
        }
        browser
    }

    /// List `dir`. On failure the browser stays where it was.
    fn load(&mut self, dir: &Path) -> Result<(), String> {
        let listing = fs::read_dir(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for entry in listing.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            // Follows symbolic links, unlike the entry's own file type.
            let path = entry.path();
            if path.is_dir() {
                dirs.push(Entry { name, is_dir: true });
            } else if self.wanted(&path) {
                files.push(Entry { name, is_dir: false });
            }
        }
        let by_name = |a: &Entry, b: &Entry| a.name.to_lowercase().cmp(&b.name.to_lowercase());
        dirs.sort_by(by_name);
        files.sort_by(by_name);
        self.entries.clear();
        if dir.parent().is_some() {
            self.entries.push(Entry { name: "..".to_string(), is_dir: true });
        }
        self.entries.extend(dirs);
        self.entries.extend(files);
        self.dir = dir.to_path_buf();
        self.selected = 0;
        Ok(())
    }

    fn wanted(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| self.extensions.iter().any(|w| w.eq_ignore_ascii_case(e)))
    }

    pub fn rows(&self) -> usize {
        self.pick_dirs as usize + self.entries.len()
    }

    pub fn row(&self, index: usize) -> Option<Row<'_>> {
        match index.checked_sub(self.pick_dirs as usize) {
            None => Some(Row::UseThisDirectory),
            Some(i) => self.entries.get(i).map(Row::Entry),
        }
    }

    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.rows().saturating_sub(1));
    }

    /// Enter on the selected row: open a directory, or pick this directory
    /// or a file.
    pub fn activate(&mut self) -> Result<Option<PathBuf>, String> {
        match self.row(self.selected) {
            None => Ok(None),
            Some(Row::UseThisDirectory) => Ok(Some(self.dir.clone())),
            Some(Row::Entry(entry)) if entry.name == ".." => self.parent().map(|()| None),
            Some(Row::Entry(entry)) if entry.is_dir => {
                let dir = self.dir.join(&entry.name);
                self.load(&dir).map(|()| None)
            }
            Some(Row::Entry(entry)) => Ok(Some(self.dir.join(&entry.name))),
        }
    }

    /// Go up a level, selecting the directory we came from.
    pub fn parent(&mut self) -> Result<(), String> {
        let Some(parent) = self.dir.parent().map(Path::to_path_buf) else { return Ok(()) };
        let from = self.dir.file_name().map(|n| n.to_string_lossy().into_owned());
        self.load(&parent)?;
        if let Some(from) = from
            && let Some(i) = self.entries.iter().position(|e| e.name == from)
        {
            self.selected = i + self.pick_dirs as usize;
        }
        Ok(())
    }

    /// Select the next entry after the selected one whose name starts with
    /// `c`.
    pub fn jump(&mut self, c: char) {
        let c = c.to_lowercase().next().unwrap_or(c);
        let rows = self.rows();
        for step in 1..=rows {
            let i = (self.selected + step) % rows;
            if let Some(Row::Entry(entry)) = self.row(i)
                && entry.name.to_lowercase().starts_with(c)
            {
                self.selected = i;
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::path::absolute(Path::new("target/test_config_ui").join(name)).unwrap();
        let _ = fs::remove_dir_all(&dir);
        for d in ["games/doom", "Images", ".hidden"] {
            fs::create_dir_all(dir.join(d)).unwrap();
        }
        for f in ["b.CUE", "a.iso", "notes.txt", "Images/x.bin"] {
            fs::write(dir.join(f), b"").unwrap();
        }
        dir
    }

    fn names(b: &Browser) -> Vec<&str> {
        b.entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn lists_directories_then_wanted_files() {
        let dir = scratch("list");
        let mut b = Browser::new("t", &dir, true, IMAGES);
        assert_eq!(names(&b), ["..", "games", "Images", "a.iso", "b.CUE"]);
        assert_eq!(b.rows(), 6);

        // Use this directory
        assert_eq!(b.activate(), Ok(Some(dir.clone())));
        // Into Images/ and back, landing on it.
        b.select(3);
        assert_eq!(b.activate(), Ok(None));
        assert_eq!(b.dir, dir.join("Images"));
        assert_eq!(names(&b), ["..", "x.bin"]);
        b.select(1);
        assert_eq!(b.activate(), Ok(None));
        assert_eq!(b.dir, dir);
        assert_eq!(b.selected, 3);

        b.jump('a');
        assert_eq!(b.activate(), Ok(Some(dir.join("a.iso"))));
    }

    #[test]
    fn starts_at_the_nearest_existing_directory() {
        let dir = scratch("start");
        let b = Browser::new("t", &dir.join("games/doom/missing/deeper"), false, IMAGES);
        assert_eq!(b.dir, dir.join("games/doom"));
        // Without directory picking the entries start at row 0.
        assert!(matches!(b.row(0), Some(Row::Entry(e)) if e.name == ".."));
    }
}
