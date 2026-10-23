//! InstallShield's installer archives (crates/installshield) as rust-dos
//! uses them: `rust-dos unpack`, which lists an archive or extracts it to
//! a host folder, and the placement of its files by file group, which a
//! game profile's install (install.rs) shares.
//!
//! An archive is a cabinet set, opened from its header (`data1.hdr`, or a
//! `data1.cab` that starts with one), or an InstallShield 3 archive
//! (`_SETUP.1`, a `.Z`). Which one a file is comes from its first bytes.

use rust_dos_installshield::{
    Archive, Cabinet, Compression, Folder, Kind, Limits, Member, MembershipKind, OutsideStatus,
    PathConflict, Skipped,
};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// An opened installer archive of either kind.
pub enum Source {
    Cabinet(Box<Cabinet>),
    Archive(Box<Archive>),
}

impl Source {
    /// The archive at `path` on the host.
    pub fn open_path(path: &Path, compression: Compression) -> Result<Source, String> {
        let shown = |e: &dyn std::fmt::Display| format!("{}: {}", path.display(), e);
        match rust_dos_installshield::sniff(path).map_err(|e| shown(&e))? {
            Some(Kind::Cabinet) => Cabinet::open_path(path, compression, &Limits::default())
                .map(|c| Source::Cabinet(Box::new(c)))
                .map_err(|e| shown(&e)),
            Some(Kind::Archive) => Archive::open_path(path, &Limits::default())
                .map(|a| Source::Archive(Box::new(a)))
                .map_err(|e| shown(&e)),
            None => Err(shown(
                &"not an InstallShield cabinet header or InstallShield 3 archive",
            )),
        }
    }

    /// The archive `name` (relative, `/` between parts) in `folder`.
    pub fn open_in(
        folder: Box<dyn Folder>,
        name: &str,
        compression: Compression,
    ) -> Result<Source, String> {
        let mut start = [0u8; 4];
        match folder
            .lookup(name)
            .map_err(|e| format!("{}: {}", name, e))?
        {
            rust_dos_installshield::Lookup::Found(mut stream, _) => {
                stream
                    .read_exact(&mut start)
                    .map_err(|e| format!("{}: {}", name, e))?;
            }
            _ => return Err(format!("{}: not found", name)),
        }
        match rust_dos_installshield::kind_of(&start) {
            Some(Kind::Cabinet) => Cabinet::open(folder, name, compression, &Limits::default())
                .map(|c| Source::Cabinet(Box::new(c)))
                .map_err(|e| format!("{}: {}", name, e)),
            Some(Kind::Archive) => {
                let stream = match folder
                    .lookup(name)
                    .map_err(|e| format!("{}: {}", name, e))?
                {
                    rust_dos_installshield::Lookup::Found(stream, _) => stream,
                    _ => return Err(format!("{}: not found", name)),
                };
                Archive::open(stream, &Limits::default())
                    .map(|a| Source::Archive(Box::new(a)))
                    .map_err(|e| format!("{}: {}", name, e))
            }
            None => Err(format!(
                "{}: not an InstallShield cabinet header or InstallShield 3 archive",
                name
            )),
        }
    }

    pub fn members(&self) -> &[Member] {
        match self {
            Source::Cabinet(c) => c.members(),
            Source::Archive(a) => a.members(),
        }
    }

    pub fn skipped(&self) -> &[Skipped] {
        match self {
            Source::Cabinet(c) => c.skipped(),
            Source::Archive(a) => a.skipped(),
        }
    }

    pub fn conflicts(&self) -> &[PathConflict] {
        match self {
            Source::Cabinet(c) => c.conflicts(),
            Source::Archive(a) => a.conflicts(),
        }
    }

    /// The file entry `index` holds, expanded and checked.
    pub fn read_entry(&self, index: usize) -> Result<Vec<u8>, String> {
        match self {
            Source::Cabinet(c) => c.read_entry(index),
            Source::Archive(a) => a.read_entry(index),
        }
        .map_err(|e| e.to_string())
    }

    /// The file entry `index` holds, written to `out` as it is expanded.
    pub fn copy_entry(&self, index: usize, out: &mut dyn std::io::Write) -> Result<u64, String> {
        match self {
            Source::Cabinet(c) => c.copy_entry(index, out),
            Source::Archive(a) => a.copy_entry(index, out),
        }
        .map_err(|e| e.to_string())
    }

    /// Every file it reads, with its entries: the listed members, the
    /// files at paths that hold different files, and the files a cabinet
    /// keeps outside that were found beside its header.
    fn files(&self) -> Vec<&Member> {
        let mut files: Vec<&Member> = self.members().iter().collect();
        files.extend(self.conflicts().iter().flat_map(|c| c.files.iter()));
        if let Source::Cabinet(c) = self {
            files.extend(
                c.outside_files()
                    .iter()
                    .filter(|o| o.status == OutsideStatus::Available)
                    .map(|o| &o.member),
            );
        }
        files
    }
}

/// Where one file goes: the entry that reads it, and its path under the
/// install folder with `/` between the parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub index: usize,
    pub path: String,
    pub size: u64,
}

/// Where the files of `source` go. Without `groups`, each file goes to
/// its own path (its directory and name), and a path that holds different
/// files is left out. With them, (group name, folder) pairs, a file goes
/// to the folder of each named group whose range holds one of its entries,
/// at that entry's directory and name; files of no named group are left
/// out. Two different files at one place leave the place out. What is
/// left out that way comes back as warnings.
pub fn placements(source: &Source, groups: &[(String, String)]) -> (Vec<Placement>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut wanted: Vec<Placement> = Vec::new();
    if groups.is_empty() {
        for m in source.members() {
            wanted.push(Placement {
                index: m.entry.index,
                path: m.path.clone(),
                size: m.size,
            });
        }
        for c in source.conflicts() {
            warnings.push(format!(
                "{} holds {} different files and is left out (a file group places each)",
                c.path,
                c.files.len()
            ));
        }
    } else {
        let Source::Cabinet(cabinet) = source else {
            warnings.push(
                "an InstallShield 3 archive has no file groups: nothing is placed".to_string(),
            );
            return (Vec::new(), warnings);
        };
        let names: Vec<Option<String>> = cabinet
            .file_groups()
            .iter()
            .map(|g| g.name.as_ref().map(|n| n.to_lowercase()))
            .collect();
        for (name, _) in groups {
            if !names
                .iter()
                .any(|n| n.as_deref() == Some(name.to_lowercase().as_str()))
            {
                warnings.push(format!("the cabinet has no file group '{}'", name));
            }
        }
        if let Some(problem) = cabinet.file_group_problem() {
            warnings.push(format!("the file groups don't read whole: {}", problem));
        }
        for m in source.files() {
            for e in std::iter::once(&m.entry).chain(&m.shared_by) {
                if e.groups.kind == MembershipKind::Undetermined && !e.groups.malformed.is_empty() {
                    warnings.push(format!("{}: a malformed file group may hold it", m.path));
                }
                for &g in &e.groups.groups {
                    let Some(group) = names.get(g).cloned().flatten() else {
                        continue;
                    };
                    for (_, folder) in groups.iter().filter(|(n, _)| n.to_lowercase() == group) {
                        let own = if e.directory.is_empty() {
                            e.name.clone()
                        } else {
                            format!("{}/{}", e.directory, e.name)
                        };
                        let folder = folder.replace('\\', "/");
                        let folder = folder.trim_matches('/');
                        let path = if folder.is_empty() {
                            own
                        } else {
                            format!("{}/{}", folder, own)
                        };
                        wanted.push(Placement {
                            index: m.entry.index,
                            path,
                            size: m.size,
                        });
                    }
                }
            }
        }
    }
    // Two different files at one place: neither goes there. One file
    // placed twice there (from two groups) goes once.
    let mut by_place: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, p) in wanted.iter().enumerate() {
        by_place.entry(p.path.to_lowercase()).or_default().push(i);
    }
    let mut keep = vec![false; wanted.len()];
    let mut seen = std::collections::HashSet::new();
    for (i, p) in wanted.iter().enumerate() {
        let at = &by_place[&p.path.to_lowercase()];
        let files: std::collections::HashSet<usize> = at.iter().map(|&j| wanted[j].index).collect();
        if files.len() > 1 {
            if seen.insert(p.path.to_lowercase()) {
                warnings.push(format!(
                    "{} would get {} different files and is left out",
                    p.path,
                    files.len()
                ));
            }
            continue;
        }
        keep[i] = at[0] == i;
    }
    let mut placed: Vec<Placement> = wanted
        .into_iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(p, _)| p)
        .collect();
    placed.sort_by_key(|p| p.path.to_lowercase());
    (placed, warnings)
}

/// What `rust-dos unpack` was given.
pub struct UnpackOptions {
    pub archive: PathBuf,
    /// List instead of extracting.
    pub list: bool,
    /// The folder to extract into.
    pub to: PathBuf,
    /// The file groups to extract, each to the folder as it is.
    pub groups: Vec<String>,
    pub compression: Compression,
}

/// `rust-dos unpack`: the exit code.
pub fn run(options: UnpackOptions) -> i32 {
    let source = match Source::open_path(&options.archive, options.compression) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("rust-dos: {}", e);
            return 1;
        }
    };
    if options.list {
        print!("{}", listing(&source));
        return 0;
    }
    if let Some(missing) = missing_group(&source, &options.groups) {
        eprintln!("rust-dos: {}", missing);
        return 1;
    }
    let groups: Vec<(String, String)> = options
        .groups
        .iter()
        .map(|g| (g.clone(), String::new()))
        .collect();
    let (placed, warnings) = placements(&source, &groups);
    for w in &warnings {
        eprintln!("rust-dos: {}", w);
    }
    let mut failed = 0;
    for p in &placed {
        if let Err(e) = extract_one(&source, p, &options.to) {
            eprintln!("rust-dos: {}: {}", p.path, e);
            failed += 1;
        }
    }
    println!(
        "{} files extracted to {}{}",
        placed.len() - failed,
        options.to.display(),
        if failed > 0 {
            format!(", {} failed", failed)
        } else {
            String::new()
        }
    );
    if failed > 0
        && matches!(source, Source::Cabinet(_))
        && options.compression == Compression::Chunks
    {
        eprintln!("rust-dos: if every file fails, try again with --old-compression");
    }
    if failed > 0 { 1 } else { 0 }
}

/// Why `groups` can't be extracted from `source`: an InstallShield 3
/// archive has no file groups, and a cabinet may not list one of them.
pub fn missing_group(source: &Source, groups: &[String]) -> Option<String> {
    if groups.is_empty() {
        return None;
    }
    let Source::Cabinet(cabinet) = source else {
        return Some("an InstallShield 3 archive has no file groups".to_string());
    };
    let unknown = groups.iter().find(|name| {
        let name = name.to_lowercase();
        !cabinet
            .file_groups()
            .iter()
            .any(|g| g.name.as_ref().is_some_and(|n| n.to_lowercase() == name))
    })?;
    Some(format!("the cabinet has no file group '{}'", unknown))
}

/// Write the file `p` places under `root`, through a temporary file that
/// takes its name once it has been read and checked whole.
fn extract_one(source: &Source, p: &Placement, root: &Path) -> Result<(), String> {
    let target = p
        .path
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part));
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let partial = target.with_file_name(format!(
        "{}.partial",
        target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    let result = (|| {
        let file = std::fs::File::create(&partial).map_err(|e| e.to_string())?;
        let mut out = std::io::BufWriter::new(file);
        source.copy_entry(p.index, &mut out)?;
        std::io::Write::flush(&mut out).map_err(|e| e.to_string())?;
        drop(out);
        std::fs::rename(&partial, &target).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

/// What `--list` prints: the members with their sizes and groups, then
/// what isn't listed and why.
pub fn listing(source: &Source) -> String {
    let mut out = String::new();
    let groups: Vec<String> = match source {
        Source::Cabinet(c) => {
            out.push_str(&format!(
                "InstallShield cabinet, major version {}, {} compression\n",
                c.major_version(),
                match c.compression() {
                    Compression::Chunks => "chunks",
                    Compression::Markers => "markers",
                }
            ));
            for g in c.file_groups() {
                out.push_str(&format!(
                    "group {}: {} (files {} to {}){}\n",
                    g.index,
                    g.name.as_deref().unwrap_or("?"),
                    g.first_file.map_or("?".to_string(), |v| v.to_string()),
                    g.last_file.map_or("?".to_string(), |v| v.to_string()),
                    g.problem
                        .as_ref()
                        .map_or(String::new(), |p| format!(": {}", p))
                ));
            }
            if let Some(p) = c.file_group_problem() {
                out.push_str(&format!("groups: {}\n", p));
            }
            c.file_groups()
                .iter()
                .map(|g| g.name.clone().unwrap_or_default())
                .collect()
        }
        Source::Archive(_) => {
            out.push_str("InstallShield 3 archive\n");
            Vec::new()
        }
    };
    let group_names = |m: &Member| -> String {
        let names: Vec<&str> = m
            .entry
            .groups
            .groups
            .iter()
            .filter_map(|&g| groups.get(g).map(String::as_str))
            .collect();
        if names.is_empty() {
            String::new()
        } else {
            format!("  [{}]", names.join(", "))
        }
    };
    for m in source.members() {
        out.push_str(&format!(
            "{:>12}  {:>6}  {}{}\n",
            m.size,
            m.entry.index,
            m.path,
            group_names(m)
        ));
    }
    for c in source.conflicts() {
        for f in &c.files {
            out.push_str(&format!(
                "{:>12}  {:>6}  {} (one of {} files at this path){}\n",
                f.size,
                f.entry.index,
                f.path,
                c.files.len(),
                group_names(f)
            ));
        }
    }
    if let Source::Cabinet(c) = source {
        for o in c.outside_files() {
            out.push_str(&format!(
                "{:>12}  {:>6}  {} (outside the cabinet: {:?})\n",
                o.member.size, o.member.entry.index, o.member.path, o.status
            ));
        }
    }
    for s in source.skipped() {
        out.push_str(&format!(
            "skipped {} ({}): {}{}\n",
            s.index,
            s.kind.name(),
            s.path.as_deref().unwrap_or("?"),
            format_args!(": {}", s.reason)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_dos_installshield::Lookup;
    use rust_dos_installshield::write::{
        self, ArchiveFile, CabinetFile, CabinetGroup, CabinetOptions,
    };
    use std::collections::HashMap;
    use std::io::Cursor;

    struct MemFolder(HashMap<String, Vec<u8>>);

    impl Folder for MemFolder {
        fn lookup(&self, path: &str) -> std::io::Result<Lookup> {
            Ok(
                match self.0.iter().find(|(k, _)| k.eq_ignore_ascii_case(path)) {
                    Some((_, data)) => {
                        Lookup::Found(Box::new(Cursor::new(data.clone())), data.len() as u64)
                    }
                    None => Lookup::Missing,
                },
            )
        }
    }

    /// A version 5 set: two languages' copies of one path, each in its
    /// own group, and a shared file in a third group.
    fn set() -> Source {
        let files = [
            CabinetFile::new("", "readme.txt", b"english"),
            CabinetFile::new("", "readme.txt", b"deutsch"),
            CabinetFile::new("data", "game.dat", b"shared data"),
        ];
        let options = CabinetOptions {
            groups: vec![
                CabinetGroup::new("English", 0, 0),
                CabinetGroup::new("German", 1, 1),
                CabinetGroup::new("Data", 2, 2),
            ],
            ..CabinetOptions::default()
        };
        let folder = MemFolder(write::cabinet(5, &files, &options).into_iter().collect());
        Source::open_in(Box::new(folder), "data1.hdr", Compression::Chunks).unwrap()
    }

    fn paths(placed: &[Placement]) -> Vec<&str> {
        placed.iter().map(|p| p.path.as_str()).collect()
    }

    #[test]
    fn without_groups_a_shared_path_is_left_out() {
        let source = set();
        let (placed, warnings) = placements(&source, &[]);
        assert_eq!(paths(&placed), ["data/game.dat"]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("readme.txt holds 2 different files"));
    }

    #[test]
    fn groups_go_to_their_folders() {
        let source = set();
        let groups = [
            ("english".to_string(), "".to_string()),
            ("Data".to_string(), "\\GAME\\".to_string()),
        ];
        let (placed, warnings) = placements(&source, &groups);
        assert!(warnings.is_empty(), "{:?}", warnings);
        assert_eq!(paths(&placed), ["GAME/data/game.dat", "readme.txt"]);
        assert_eq!(source.read_entry(placed[1].index).unwrap(), b"english");
    }

    #[test]
    fn two_groups_into_one_folder_leave_the_clash_out() {
        let source = set();
        let groups = [
            ("English".to_string(), "".to_string()),
            ("German".to_string(), "".to_string()),
            ("Data".to_string(), "".to_string()),
        ];
        let (placed, warnings) = placements(&source, &groups);
        assert_eq!(paths(&placed), ["data/game.dat"]);
        assert_eq!(
            warnings,
            ["readme.txt would get 2 different files and is left out"]
        );
    }

    #[test]
    fn missing_groups_are_named() {
        let source = set();
        assert_eq!(missing_group(&source, &[]), None);
        assert_eq!(missing_group(&source, &["GERMAN".to_string()]), None);
        assert_eq!(
            missing_group(&source, &["French".to_string()]).as_deref(),
            Some("the cabinet has no file group 'French'")
        );
    }

    #[test]
    fn listing_shows_groups_and_conflicts() {
        let text = listing(&set());
        assert!(
            text.starts_with("InstallShield cabinet, major version 5, chunks compression\n"),
            "{}",
            text
        );
        assert!(
            text.contains("group 1: German (files 1 to 1)\n"),
            "{}",
            text
        );
        assert!(text.contains("2  data/game.dat  [Data]\n"), "{}", text);
        assert!(
            text.contains("readme.txt (one of 2 files at this path)  [German]\n"),
            "{}",
            text
        );
    }

    #[test]
    fn an_archive_lists_and_places_by_path() {
        let data = write::archive(
            &[ArchiveFile::new("SUB", "A.TXT", b"archive member")],
            0,
            1,
            1,
        );
        let folder = MemFolder(HashMap::from([("_SETUP.1".to_string(), data)]));
        let source = Source::open_in(Box::new(folder), "_setup.1", Compression::Chunks).unwrap();
        assert!(listing(&source).starts_with("InstallShield 3 archive\n"));
        let (placed, warnings) = placements(&source, &[]);
        assert!(warnings.is_empty());
        assert_eq!(paths(&placed), ["SUB/A.TXT"]);
        assert_eq!(
            source.read_entry(placed[0].index).unwrap(),
            b"archive member"
        );
        assert!(missing_group(&source, &["x".to_string()]).is_some());
    }
}
