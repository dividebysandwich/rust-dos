//! Free space, attributes, directory creation and removal, and directory search.

use super::*;

impl DiskController {
    // ========================================================================
    // FILESYSTEM METADATA & SEARCH
    // ========================================================================

    /// Allocation geometry of `drive` (0-based) for INT 21h AH=1Bh/1Ch:
    /// (sectors per cluster, bytes per sector, total clusters), as the old
    /// functions can tell it (`old_space`).
    pub fn drive_geometry(&self, drive: u8) -> Option<(u16, u16, u16)> {
        self.layout(drive).map(|l| {
            let (spc, _, bps, total) =
                old_space(l.sectors_per_cluster as u32, 0, l.bytes_per_sector as u32, l.clusters);
            (spc, bps, total)
        })
    }

    /// Where `drive`'s FAT, directory and data are: a disk image's own, or
    /// a plausible one for the others (see `DriveKind::layout`).
    pub fn layout(&self, drive: u8) -> Option<FatLayout> {
        let d = self.drive(drive)?;
        Some(d.fat().map_or_else(|| d.kind.layout(), |volume| volume.layout()))
    }

    /// The media descriptor byte of `drive` (0 if it isn't mounted).
    pub fn media_descriptor(&self, drive: u8) -> u8 {
        self.layout(drive).map_or(0, |l| l.media)
    }

    /// The volume serial number of a drive mounted from a disk image.
    pub fn volume_serial(&self, drive: u8) -> Option<u32> {
        self.fat_volume(drive)?.serial()
    }

    /// INT 21h AH=36h: (sectors per cluster, free clusters, bytes per
    /// sector, total clusters) of `drive` (0 the current one, 1 A:), as
    /// the old function can tell them (`old_space`).
    pub fn get_disk_free_space(&self, drive: u8) -> Result<(u16, u16, u16, u16), u16> {
        let (spc, free, bps, total) = self.get_disk_free_space32(drive)?;
        Ok(old_space(spc, free, bps, total))
    }

    /// The free space of `drive` (0 the current one, 1 A:) as AX=7303h
    /// tells it: (sectors per cluster, free clusters, bytes per sector,
    /// total clusters), whatever their size.
    pub fn get_disk_free_space32(&self, drive: u8) -> Result<(u32, u32, u32, u32), u16> {
        let target_drive = if drive == 0 {
            self.current_drive
        } else {
            drive - 1
        };

        let d = self.drive(target_drive).ok_or(0x0Fu16)?; // Invalid Drive
        if let Some(volume) = d.fat() {
            let layout = volume.layout();
            let free = volume.free_clusters().min(layout.clusters);
            return Ok((layout.sectors_per_cluster as u32, free, layout.bytes_per_sector as u32, layout.clusters));
        }
        let (spc, bps, total) = d.kind.geometry();
        let free = match d.kind {
            // Floppies report real usage so programs can tell whether a save
            // will fit; hard disks keep reporting an empty fake 80 MB.
            DriveKind::Floppy => {
                let cluster_bytes = spc as u64 * bps as u64;
                let used = d.host_root().map_or(0, |root| Self::used_clusters(root, cluster_bytes, total as u64));
                total - used.min(total as u64) as u16
            }
            DriveKind::HardDisk => total,
            DriveKind::CdRom => 0,
            DriveKind::Virtual => 1000,
        };
        Ok((spc as u32, free as u32, bps as u32, total as u32))
    }

    /// Clusters occupied by the tree under `root` (one per directory plus
    /// each file rounded up). Stops counting once `cap` is reached so that
    /// mounting a huge directory as a floppy stays cheap. Symlinks are not
    /// followed.
    fn used_clusters(root: &Path, cluster_bytes: u64, cap: u64) -> u64 {
        let mut used = 0u64;
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            let Ok(read_dir) = hostfs::read_dir(&dir) else {
                continue;
            };
            for entry in read_dir {
                if entry.name.to_string_lossy().starts_with('.') {
                    continue;
                }
                if entry.is_dir {
                    used += 1;
                    pending.push(entry.path);
                } else if let Some(meta) = entry.metadata().ok().filter(|m| m.is_file()) {
                    used += meta.len.div_ceil(cluster_bytes);
                }
                if used >= cap {
                    return cap;
                }
            }
        }
        used
    }

    // INT 21h, AH=43h: Get File Attributes
    // Returns: Attribute Byte (0x20 = Archive, 0x10 = Subdir, etc.)
    #[allow(dead_code)]
    pub fn get_file_attribute(&self, filename: &str) -> Result<u16, u8> {
        if let Some((_, drive, path)) = self.locate_in_memory(filename) {
            let files = drive.tree().ok_or(0x03u8)?;
            return match files.file(&path) {
                Some(node) => Ok(Self::tree_attr(Some(node), None) as u16),
                None if files.is_dir(&path) => Ok(Self::tree_attr(None, files.dir_record(&path)) as u16),
                None => Err(0x02),
            };
        }
        if let Some(found) = self.find_fat(filename) {
            return found.map(|e| e.attr as u16);
        }
        let (drive, path) = self.locate(filename).ok_or(0x03)?;
        if !hostfs::exists(&path) {
            return Err(0x02); // File Not Found
        }

        let mut attr: u16 = 0;
        if hostfs::is_dir(&path) {
            attr |= 0x10; // Directory
        } else {
            attr |= 0x20; // Archive (standard file)
        }
        // Reflect host read-only state into DOS R/O bit. On Unix, read-only means
        // no user-write permission. On Windows, the readonly flag. Everything
        // on read-only media is R/O too.
        let host_ro = hostfs::metadata(&path).is_ok_and(|m| m.readonly);
        if host_ro || !self.is_writable(drive) {
            attr |= 0x01;
        }
        Ok(attr)
    }

    /// DOS AH=43h AL=01: Set file attributes. We honor just the R/O bit because
    /// the host filesystem generally doesn't have direct analogs for DOS's
    /// Hidden/System bits. Directory and Volume Label bits cannot be set via
    /// this call on real DOS either.
    pub fn set_file_attribute(&self, filename: &str, attr: u16) -> Result<(), u8> {
        if let Some((drive, volume, parts)) = self.locate_fat(filename) {
            let parts = refs(&parts);
            volume.find(&parts)?;
            self.check_writable(drive)?;
            return volume.set_attr(&parts, attr as u8);
        }
        let (drive, path) = self.locate(filename).ok_or(0x03)?;
        if !hostfs::exists(&path) {
            return Err(0x02);
        }
        self.check_writable(drive)?;
        if let Ok(meta) = hostfs::metadata(&path) {
            let want_ro = (attr & 0x01) != 0;
            if meta.readonly != want_ro {
                self.host_dirs.touched(&path);
                // Ignore permission-set errors on systems where it's not supported.
                let _ = hostfs::set_readonly(&path, want_ro);
            }
        }
        Ok(())
    }

    /// DOS AH=39h: Create a directory at the given DOS path.
    pub fn create_directory(&self, path: &str) -> Result<(), u8> {
        let normalized = path.replace('/', "\\");
        let (drive, rest) = self.split_drive(&normalized).ok_or(0x03)?;
        self.check_writable(drive)?;
        if let Some((_, volume, parts)) = self.locate_fat(path) {
            if parts.is_empty() {
                return Err(0x05);
            }
            return volume.mkdir(&refs(&parts));
        }

        // resolve_path walks any existing leaf, but MKDIR needs to create a new
        // leaf — so resolve the parent, then append the final component.
        let (parent_dos, leaf) = match rest.rsplit_once('\\') {
            Some(("", l)) => ("\\", l),
            Some((p, l)) => (p, l),
            None => (".", rest), // current directory of that drive
        };
        if leaf.is_empty() {
            return Err(0x03); // Path not found / invalid
        }
        let parent_path = self.resolve_on(drive, parent_dos).ok_or(0x03)?;
        if !hostfs::is_dir(&parent_path) {
            return Err(0x03);
        }
        // Case-insensitive: MKDIR "Foo" should collide with existing "FOO".
        if let Some(existing) = self.find_existing_child(&parent_path, leaf) {
            let full = parent_path.join(existing);
            if hostfs::exists(&full) {
                return Err(0x05); // Access denied / already exists
            }
        }
        let target = parent_path.join(leaf.to_uppercase());
        let created = hostfs::create_dir(&target);
        self.host_dirs.changed();
        self.host_dirs.touched(&target);
        created.map_err(|_| 0x05)
    }

    /// DOS AH=3Ah: Remove an empty directory.
    pub fn remove_directory(&self, path: &str) -> Result<(), u8> {
        let normalized = path.replace('/', "\\");
        let (drive_num, rest) = self.split_drive(&normalized).ok_or(0x03)?;
        if let Some((_, volume, parts)) = self.locate_fat(path) {
            let parts = refs(&parts);
            if parts.is_empty() || !volume.find(&parts).is_ok_and(|e| e.is_dir()) {
                return Err(0x03);
            }
            self.check_writable(drive_num)?;
            if self.drive(drive_num).is_some_and(|d| canonical_path(&parts) == d.current_dir) {
                return Err(0x10);
            }
            return volume.rmdir(&parts);
        }
        let (host_path, dos_form) = self.resolve_names_on(drive_num, rest).ok_or(0x03)?;
        if !hostfs::is_dir(&host_path) {
            return Err(0x03);
        }
        self.check_writable(drive_num)?;
        // DOS error 0x10 = "attempt to remove current directory".
        if self.drive(drive_num).is_some_and(|d| dos_form.eq_ignore_ascii_case(&d.current_dir)) {
            return Err(0x10);
        }
        let removed = hostfs::remove_dir(&host_path);
        self.host_dirs.changed();
        self.host_dirs.touched(&host_path);
        removed.map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => 0x03,
            _ => 0x05, // Access denied (e.g. not empty)
        })
    }

    /// Case-insensitive lookup of a child by name in a host directory.
    pub(super) fn find_existing_child(&self, parent: &Path, name: &str) -> Option<std::ffi::OsString> {
        let upper = name.to_uppercase();
        if let Ok(entries) = hostfs::read_dir(parent) {
            for e in entries {
                let fname = e.file_name();
                if fname.to_string_lossy().to_uppercase() == upper {
                    return Some(fname);
                }
            }
        }
        None
    }

    /// Helper: Simple DOS wildcard matching (? and *)
    fn matches_pattern(filename: &str, pattern: &str) -> bool {
        if pattern == "*.*" {
            return true;
        }

        // Split filename and pattern by '.'
        let (f_name, f_ext) = filename.split_once('.').unwrap_or((filename, ""));
        let (p_name, p_ext) = pattern.split_once('.').unwrap_or((pattern, ""));
        // DOS packs the pattern into a name of 8 and an extension of 3,
        // padded with spaces, so what goes past them is dropped and
        // trailing spaces are padding: Comanche looks for "LH66.RLE    ".
        let field = |part: &'_ str, len: usize| {
            let end = part.char_indices().nth(len).map_or(part.len(), |(i, _)| i);
            part[..end].trim_end_matches(' ').to_string()
        };
        let (p_name, p_ext) = (field(p_name, 8), field(p_ext, 3));

        let match_part = |f: &str, p: &str| -> bool {
            if p == "*" {
                return true;
            }
            let mut f_chars = f.chars();
            let mut p_chars = p.chars();
            loop {
                match (f_chars.next(), p_chars.next()) {
                    (None, None) => return true,
                    (Some(_), None) => return false, // Filename longer than pattern
                    (None, Some(pc)) => {
                        if pc == '*' {
                            return true;
                        }
                        if pc == '?' {
                            continue;
                        } // Treat ? as match for "empty" (padding)
                        return false;
                    }
                    (Some(fc), Some(pc)) => {
                        if pc == '*' {
                            return true;
                        }
                        if pc == '?' {
                            continue;
                        }
                        if pc.to_ascii_uppercase() != fc.to_ascii_uppercase() {
                            return false;
                        }
                    }
                }
            }
        };

        match_part(f_name, &p_name) && match_part(f_ext, &p_ext)
    }

    /// The attributes of an entry of a tree held in memory (None for a
    /// directory, with its record on the CD if it has one): read-only, and
    /// hidden where the CD says so.
    fn tree_attr(node: Option<&Node>, dir_record: Option<crate::cdrom::Extent>) -> u8 {
        match node {
            None if dir_record.is_some_and(|r| r.hidden) => 0x13,
            None => 0x11,
            Some(Node::Extent(extent)) if extent.hidden => 0x23,
            Some(_) => 0x21,
        }
    }

    /// A volume label as FindFirst reports it: labels longer than 8
    /// characters get a dot after the 8th, like a filename.
    fn label_entry(label: &str) -> DosDirEntry {
        let filename = if label.len() > 8 {
            format!("{}.{}", &label[..8], &label[8..])
        } else {
            label.to_string()
        };
        DosDirEntry {
            filename,
            size: 0,
            is_dir: false,
            is_readonly: false,
            dos_time: 0x0000,
            dos_date: 0x5021,
            attr: 0x08,
        }
    }

    /// The ".." and "." entries of a directory below the root that match
    /// `pattern`, with the DOS time and date of each: the parent's for "..",
    /// the directory's own for ".".
    fn dot_entries(pattern: &str, times: [(u16, u16); 2]) -> impl Iterator<Item = DosDirEntry> + '_ {
        ["..", "."]
            .into_iter()
            .zip(times)
            .filter(|(dot, _)| Self::matches_pattern(dot, pattern))
            .map(|(dot, (dos_time, dos_date))| DosDirEntry {
                filename: dot.to_string(),
                size: 0,
                is_dir: true,
                is_readonly: false,
                dos_time,
                dos_date,
                attr: 0x10,
            })
    }

    // INT 21h, AH=4E/4F: Find First / Find Next
    // search_spec contains the path AND the pattern e.g. "C:\GAMES\*.EXE" or "*.EXE"
    /// Entry `search_index` of the listing of `search_spec`. FindNext
    /// passes the same spec with the next index each call; on a host drive
    /// the listing FindFirst (index 0) made is kept for it, as long as the
    /// folder doesn't change (`HostDirCache`), rather than listing the
    /// folder again for each entry.
    pub fn find_directory_entry(
        &self,
        search_spec: &str,
        search_index: usize,
        search_attr: u16,
    ) -> Result<DosDirEntry, u8> {
        let Some(key) = self.search_key(search_spec, search_attr) else {
            return self.list_directory(search_spec, search_attr)?.into_iter().nth(search_index).ok_or(0x12);
        };
        let entries = self.host_dirs.search(key, search_index == 0, || self.list_directory(search_spec, search_attr))?;
        entries.get(search_index).cloned().ok_or(0x12)
    }

    /// What a search of a folder on a host drive lists (see
    /// `list_directory`); None for other drives and volume label searches.
    fn search_key(&self, search_spec: &str, search_attr: u16) -> Option<SearchKey> {
        if search_attr & 0x08 != 0 {
            return None;
        }
        let normalized = search_spec.replace('/', "\\");
        let (drive_num, rest) = self.split_drive(&normalized)?;
        let drive = self.drive(drive_num)?;
        if drive.fat().is_some() || drive.tree().is_some() {
            return None;
        }
        let (parent_dir, pattern) = match rest.rfind('\\') {
            Some(idx) => rest.split_at(idx + 1),
            None => ("", rest),
        };
        let dir = self.resolve_on(drive_num, if parent_dir.is_empty() { "." } else { parent_dir })?;
        // FindNext gets the pattern back from the DTA ("RGN*.GFF" comes
        // back as "RGN?????.GFF"), so both are keyed in that form.
        let pattern = crate::interrupts::utils::pattern_to_fcb(pattern);
        let root = drive.host_root() == Some(dir.as_path());
        // FindNext gets the attributes back from the DTA's one byte.
        Some(SearchKey { dir, pattern, attr: search_attr & 0xFF, writable: drive.writable(), root })
    }

    /// The files a path names, with wildcards in its last part or not,
    /// each with its fully qualified DOS path, as DEL, COPY, REN, FOR and
    /// IF EXIST take them: directories, hidden and system files are left
    /// out.
    pub fn matching_files(&self, spec: &str) -> Result<Vec<(String, DosDirEntry)>, u8> {
        let entries = self.list_directory(spec, 0)?;
        let dir = self.qualify_directory(spec).ok_or(0x03u8)?;
        let dir = dir.trim_end_matches('\\');
        Ok(entries
            .into_iter()
            .filter(|e| !e.is_dir)
            .map(|e| (format!("{}\\{}", dir, e.filename), e))
            .collect())
    }

    /// All entries matching a search spec, in the order FindFirst/FindNext
    /// return them. Directory listings (DIR) use this directly.
    pub fn list_directory(
        &self,
        search_spec: &str,
        search_attr: u16,
    ) -> Result<Vec<DosDirEntry>, u8> {
        let normalized = search_spec.replace('/', "\\");
        let (drive_num, rest) = self.split_drive(&normalized).ok_or(0x03)?;
        let drive = self.drive(drive_num).ok_or(0x03)?;

        // Handle Volume Label request
        if (search_attr & 0x08) != 0 {
            return Ok(vec![Self::label_entry(&drive.label)]);
        }

        // Split Spec into Directory and Pattern
        let (parent_dir, pattern) = match rest.rfind('\\') {
            Some(idx) => rest.split_at(idx + 1),
            None => ("", rest),
        };
        let search_dir_str = if parent_dir.is_empty() {
            "."
        } else {
            parent_dir
        };

        // Search attribute bits an entry needs to be listed.
        let restricted_bits = 0x02 | 0x04 | 0x10;
        let mut valid_entries: Vec<DosDirEntry> = Vec::new();

        if let Some(volume) = drive.fat() {
            let dir = Self::logical_components(drive, search_dir_str);
            for entry in volume.list(&dir).map_err(|_| 0x03u8)? {
                let attr = entry.attr;
                if (attr as u16 & restricted_bits) & !search_attr != 0 || !Self::matches_pattern(&entry.name, pattern) {
                    continue;
                }
                valid_entries.push(DosDirEntry {
                    is_dir: entry.is_dir(),
                    is_readonly: attr & fat::ATTR_READ_ONLY != 0,
                    filename: entry.name,
                    size: entry.size,
                    dos_time: entry.time,
                    dos_date: entry.date,
                    attr,
                });
            }
            return Ok(valid_entries);
        }

        if let Some(files) = drive.tree() {
            let dir = Self::memory_path(drive, search_dir_str);
            if !files.is_dir(&dir) {
                return Err(0x03);
            }
            // A directory's time and date: its record's on a CD, the
            // drive's default elsewhere.
            let dir_time = |path: &str| {
                files.dir_record(path).map_or((MEMORY_TIME, MEMORY_DATE), |r| (r.time, r.date))
            };
            if !dir.is_empty() {
                let parent = dir.rsplit_once('\\').map_or("", |(parent, _)| parent);
                valid_entries.extend(Self::dot_entries(pattern, [dir_time(parent), dir_time(&dir)]));
            }
            for (name, node) in files.list(&dir) {
                let dir_record = match node {
                    None if dir.is_empty() => files.dir_record(name),
                    None => files.dir_record(&format!("{}\\{}", dir, name)),
                    Some(_) => None,
                };
                let attr = Self::tree_attr(node, dir_record);
                if (attr as u16 & restricted_bits) & !search_attr != 0 || !Self::matches_pattern(name, pattern) {
                    continue;
                }
                let (dos_time, dos_date) = match (node, dir_record) {
                    (Some(Node::Extent(extent)), _) => (extent.time, extent.date),
                    (None, Some(record)) => (record.time, record.date),
                    _ => (MEMORY_TIME, MEMORY_DATE),
                };
                valid_entries.push(DosDirEntry {
                    filename: name.to_string(),
                    size: node.map_or(0, |n| n.len() as u32),
                    is_dir: node.is_none(),
                    is_readonly: true,
                    dos_time,
                    dos_date,
                    attr,
                });
            }
            return Ok(valid_entries);
        }

        // Host Filesystem Listing
        let host_dir = self.resolve_on(drive_num, search_dir_str).ok_or(0x03)?;

        if !hostfs::is_dir(&host_dir) {
            return Err(0x03);
        }
        let is_host_root = drive.host_root() == Some(host_dir.as_path());
        let media_ro = !drive.writable();

        if !is_host_root {
            valid_entries.extend(Self::dot_entries(pattern, [(0, 0); 2]));
        }

        let (mut entries, kept) = self.host_entries(&host_dir);
        // The folder may have changed without its time moving
        // (`HostDirCache`). A wildcard search lists what is there now: it
        // stats every match anyway, so the read adds little. A search for
        // one name reads again only when nothing kept matches.
        let wild = pattern.contains(['*', '?']);
        if kept && (wild || !entries.iter().any(|(_, name)| Self::matches_pattern(name, pattern))) {
            entries = self.host_dirs.reread(&host_dir, || Self::read_host_entries(&host_dir));
        }
        for (original_name, final_name) in entries.iter() {
            // The name decides first: a stat per entry is what a listing
            // costs, and a search for one name needs only its own.
            if !Self::matches_pattern(final_name, pattern) {
                continue;
            }
            let Ok(metadata) = hostfs::metadata(host_dir.join(original_name)) else {
                continue;
            };

            let is_dir = metadata.is_dir;
            let is_readonly = media_ro || metadata.readonly;
            let mut file_attr: u8 = if is_dir { 0x10 } else { 0x20 };
            if is_readonly {
                file_attr |= 0x01;
            }

            if (file_attr as u16 & restricted_bits) & !search_attr != 0 {
                continue;
            }

            let (dos_time, dos_date) = system_time_to_dos(metadata.modified.unwrap_or(std::time::SystemTime::now()));

            valid_entries.push(DosDirEntry {
                filename: final_name.clone(),
                size: metadata.len as u32,
                is_dir,
                is_readonly,
                dos_time,
                dos_date,
                attr: file_attr,
            });
        }

        Ok(valid_entries)
    }
}
