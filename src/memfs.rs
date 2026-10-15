//! Read-only directory trees held in memory, for the drives that have no
//! host directory behind them: Z:, the built-in Ultrasound drive, and CD
//! images, whose trees point at the files' sectors on the disc.

use std::borrow::Cow;
use std::collections::BTreeMap;

/// The contents of a file: built into the program, or made at startup.
pub type Bytes = Cow<'static, [u8]>;

/// A file of the tree.
#[derive(Clone, Debug)]
pub enum Node {
    /// Contents held in memory.
    Bytes(Bytes),
    /// A file on the CD image of the drive.
    Extent(crate::cdrom::Extent),
}

impl Node {
    /// Length in bytes.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> u64 {
        match self {
            Node::Bytes(data) => data.len() as u64,
            Node::Extent(extent) => extent.size as u64,
        }
    }
}

/// Paths are upper case, relative to the root and without a leading
/// backslash ("ULTRASND\MIDI\ACPIANO.PAT"); the root itself is "".
#[derive(Clone, Debug, Default)]
pub struct MemFs {
    files: BTreeMap<String, Node>,
    /// Every directory but the root, with its record on the CD if it has
    /// one (its date and hidden flag); the others show the drive's
    /// default date.
    dirs: BTreeMap<String, Option<crate::cdrom::Extent>>,
    /// The root directory's record on the CD.
    root: Option<crate::cdrom::Extent>,
}

impl MemFs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a file, and the directories on its way.
    pub fn insert(&mut self, path: &str, data: impl Into<Bytes>) {
        self.insert_node(path, Node::Bytes(data.into()));
    }

    /// Add a file of either kind, and the directories on its way.
    pub fn insert_node(&mut self, path: &str, node: Node) {
        let path = path.trim_start_matches('\\').to_ascii_uppercase();
        self.insert_parents(&path);
        self.files.insert(path, node);
    }

    /// Add a directory, which may stay empty.
    pub fn insert_dir(&mut self, path: &str) {
        self.insert_dir_with(path, None);
    }

    /// Add a directory with its record on the CD, which gives it the date
    /// and hidden flag it shows in listings. "" sets the root's record.
    pub fn insert_dir_record(&mut self, path: &str, record: crate::cdrom::Extent) {
        self.insert_dir_with(path, Some(record));
    }

    fn insert_dir_with(&mut self, path: &str, record: Option<crate::cdrom::Extent>) {
        let path = path.trim_start_matches('\\').to_ascii_uppercase();
        self.insert_parents(&path);
        if path.is_empty() {
            self.root = record.or(self.root);
        } else {
            let known = self.dirs.entry(path).or_default();
            *known = record.or(*known);
        }
    }

    /// The CD record `insert_dir_record` gave the directory at `path` ("" for
    /// the root).
    pub fn dir_record(&self, path: &str) -> Option<crate::cdrom::Extent> {
        if path.is_empty() {
            self.root
        } else {
            self.dirs.get(path).copied().flatten()
        }
    }

    fn insert_parents(&mut self, path: &str) {
        for (i, _) in path.match_indices('\\') {
            self.dirs.entry(path[..i].to_string()).or_default();
        }
    }

    pub fn file(&self, path: &str) -> Option<&Node> {
        self.files.get(path)
    }

    pub fn is_dir(&self, path: &str) -> bool {
        path.is_empty() || self.dirs.contains_key(path)
    }

    /// What directory `dir` holds, by name: each file with its contents,
    /// each directory with None.
    pub fn list(&self, dir: &str) -> Vec<(&str, Option<&Node>)> {
        /// The name of `path` if it is directly in the directory `prefix`.
        fn child<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
            path.strip_prefix(prefix).filter(|name| !name.contains('\\'))
        }
        let prefix = if dir.is_empty() { String::new() } else { format!("{}\\", dir) };
        let dirs = self.dirs.range(prefix.clone()..).map(|(d, _)| d).take_while(|d| d.starts_with(&prefix));
        let files = self.files.range(prefix.clone()..).take_while(|(f, _)| f.starts_with(&prefix));
        let mut entries: Vec<(&str, Option<&Node>)> = dirs
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
        assert!(matches!(fs.file("ULTRASND\\MIDI\\ACPIANO.PAT"), Some(Node::Bytes(d)) if &d[..] == b"piano"));
        assert!(fs.file("ULTRASND\\MIDI").is_none());

        let names = |dir| fs.list(dir).iter().map(|&(n, d)| (n.to_string(), d.is_some())).collect::<Vec<_>>();
        assert_eq!(names(""), [("COMMAND.COM".to_string(), true), ("ULTRASND".to_string(), false)]);
        assert_eq!(names("ULTRASND"), [("MIDI".to_string(), false), ("ULTRASND.INI".to_string(), true)]);
        assert_eq!(names("ULTRASND\\MIDI"), [("ACPIANO.PAT".to_string(), true)]);
        assert!(names("NOWHERE").is_empty());
    }

    #[test]
    fn directories_keep_their_records() {
        let record = |time, hidden| crate::cdrom::Extent { lba: 20, size: 2048, time, date: 0x1F5F, hidden };
        let mut fs = MemFs::new();
        fs.insert_dir_record("", record(0x5000, false));
        fs.insert_dir_record("\\data", record(0x6000, false));
        fs.insert_dir_record("DATA\\LEVELS", record(0x6001, true));
        // Adding what is inside a directory with a record keeps the record.
        fs.insert("DATA\\LEVELS\\ONE.LVL", &b""[..]);
        fs.insert_dir("DATA");
        fs.insert("SOUND\\BOOM.VOC", &b""[..]);

        assert_eq!(fs.dir_record(""), Some(record(0x5000, false)));
        assert_eq!(fs.dir_record("DATA"), Some(record(0x6000, false)));
        assert_eq!(fs.dir_record("DATA\\LEVELS"), Some(record(0x6001, true)));
        assert_eq!(fs.dir_record("SOUND"), None);
        assert_eq!(fs.dir_record("DATA\\LEVELS\\ONE.LVL"), None);
        assert!(fs.is_dir("DATA") && fs.is_dir("SOUND"));
        assert_eq!(MemFs::new().dir_record(""), None);
    }
}
