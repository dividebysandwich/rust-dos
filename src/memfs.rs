//! Read-only directory trees held in memory, for the drives that have no
//! host directory behind them: Z: and the built-in Ultrasound drive.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

/// The contents of a file: built into the program, or made at startup.
pub type Bytes = Cow<'static, [u8]>;

/// Paths are upper case, relative to the root and without a leading
/// backslash ("ULTRASND\MIDI\ACPIANO.PAT"); the root itself is "".
#[derive(Clone, Debug, Default)]
pub struct MemFs {
    files: BTreeMap<String, Bytes>,
    /// Every directory but the root.
    dirs: BTreeSet<String>,
}

impl MemFs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a file, and the directories on its way.
    pub fn insert(&mut self, path: &str, data: impl Into<Bytes>) {
        let path = path.trim_start_matches('\\').to_ascii_uppercase();
        for (i, _) in path.match_indices('\\') {
            self.dirs.insert(path[..i].to_string());
        }
        self.files.insert(path, data.into());
    }

    pub fn file(&self, path: &str) -> Option<&Bytes> {
        self.files.get(path)
    }

    pub fn is_dir(&self, path: &str) -> bool {
        path.is_empty() || self.dirs.contains(path)
    }

    /// What directory `dir` holds, by name: each file with its contents,
    /// each directory with None.
    pub fn list(&self, dir: &str) -> Vec<(&str, Option<&Bytes>)> {
        /// The name of `path` if it is directly in the directory `prefix`.
        fn child<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
            path.strip_prefix(prefix).filter(|name| !name.contains('\\'))
        }
        let prefix = if dir.is_empty() { String::new() } else { format!("{}\\", dir) };
        let dirs = self.dirs.range(prefix.clone()..).take_while(|d| d.starts_with(&prefix));
        let files = self.files.range(prefix.clone()..).take_while(|(f, _)| f.starts_with(&prefix));
        let mut entries: Vec<(&str, Option<&Bytes>)> = dirs
            .filter_map(|d| child(d, &prefix).map(|name| (name, None)))
            .chain(files.filter_map(|(f, data)| child(f, &prefix).map(|name| (name, Some(data)))))
            .collect();
        entries.sort_by_key(|&(name, _)| name);
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_make_their_directories() {
        let mut fs = MemFs::new();
        fs.insert("ULTRASND\\MIDI\\acpiano.pat", &b"piano"[..]);
        fs.insert("\\ULTRASND\\ULTRASND.INI", b"ini".to_vec());
        fs.insert("COMMAND.COM", &b""[..]);

        assert!(fs.is_dir("") && fs.is_dir("ULTRASND") && fs.is_dir("ULTRASND\\MIDI"));
        assert!(!fs.is_dir("ULTRASND\\ULTRASND.INI") && !fs.is_dir("MIDI"));
        assert_eq!(fs.file("ULTRASND\\MIDI\\ACPIANO.PAT").map(|d| &d[..]), Some(&b"piano"[..]));
        assert!(fs.file("ULTRASND\\MIDI").is_none());

        let names = |dir| fs.list(dir).iter().map(|&(n, d)| (n.to_string(), d.is_some())).collect::<Vec<_>>();
        assert_eq!(names(""), [("COMMAND.COM".to_string(), true), ("ULTRASND".to_string(), false)]);
        assert_eq!(names("ULTRASND"), [("MIDI".to_string(), false), ("ULTRASND.INI".to_string(), true)]);
        assert_eq!(names("ULTRASND\\MIDI"), [("ACPIANO.PAT".to_string(), true)]);
        assert!(names("NOWHERE").is_empty());
    }
}
