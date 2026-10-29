//! Opening, reading, writing and closing files, and the SFT entries behind handles.

use super::*;

impl DiskController {
    // ========================================================================
    // FILE I/O OPERATIONS
    // ========================================================================

    pub(super) fn check_writable(&self, drive: u8) -> Result<(), u8> {
        match self.drive(drive) {
            None => Err(0x03),
            Some(d) if !d.writable() => Err(0x05), // Access denied
            Some(_) => Ok(()),
        }
    }

    /// A file just opened as `filename` with access `mode`, with the one
    /// reference of whoever opened it.
    pub(super) fn opened(&mut self, data: OpenData, drive: u8, owner: u16, key: u64, filename: &str, mode: u8) -> OpenFile {
        let path = self.qualify_path(filename).unwrap_or_else(|| filename.to_ascii_uppercase());
        OpenFile { data, drive, owner, key, path, mode, refs: 1, fcb: false, empty_on_use: false, raw: false }
    }

    /// The lowest unused entry of the file table.
    pub(super) fn free_handle(&self) -> Result<u16, u8> {
        (FIRST_FILE..FILES)
            .find(|h| !self.open_files.contains_key(h))
            .ok_or(0x04) // Too many open files
    }

    /// Put an open file in the table at `sft`.
    pub(super) fn insert(&mut self, sft: u16, file: OpenFile) {
        self.open_files.insert(sft, file);
        self.mark_dirty(sft);
    }

    pub(super) fn mark_dirty(&mut self, sft: u16) {
        self.sft_dirty |= 1u128.checked_shl(sft as u32).unwrap_or(0);
    }

    fn mark_moved(&mut self, sft: u16) {
        self.position_dirty |= 1u128.checked_shl(sft as u32).unwrap_or(0);
    }

    /// The entries whose copy in DOS memory is out of date since the last
    /// call, and those of them whose position only.
    pub fn take_dirty(&mut self) -> (u128, u128) {
        let whole = std::mem::take(&mut self.sft_dirty);
        (whole, std::mem::take(&mut self.position_dirty) & !whole)
    }

    /// The character device `filename` names, EMMXXXX0 among them while
    /// there is EMS.
    pub fn device(&self, filename: &str) -> Option<CharDevice> {
        let device = char_device(filename);
        if device.is_some() || !self.emm_device {
            return device;
        }
        let last = filename.rsplit(['\\', '/', ':']).next()?;
        let stem = last.split('.').next()?.trim();
        stem.eq_ignore_ascii_case("EMMXXXX0").then_some(CharDevice::Emm)
    }

    // INT 21h, AH=3Dh: Open File. `owner` is the PSP of the calling process.
    pub fn open_file(&mut self, filename: &str, mode: u8, owner: u16) -> Result<u16, u8> {
        self.open_or_create(filename, mode, owner, false)
    }

    /// Open `filename` with access `mode`; `create` makes a missing file,
    /// as the create calls do. Opening alone never creates one: programs
    /// probe for files by opening them read/write.
    pub(super) fn open_or_create(&mut self, filename: &str, mode: u8, owner: u16, create: bool) -> Result<u16, u8> {
        // Devices open by name, whatever the directory; there's no file to
        // create.
        if let Some(device) = self.device(filename) {
            let handle = self.free_handle()?;
            let file = self.opened(OpenData::Device(device), self.current_drive, owner, 0, filename, mode);
            self.insert(handle, file);
            return Ok(handle);
        }
        // DOS empties the file at the create call, so opening it again while
        // the create's handle is still unused must find it empty.
        self.settle_named(filename);
        if let Some(data) = self.system_file(filename) {
            if create {
                return Err(0x05);
            }
            return self.open_system_file(filename, data, mode, owner);
        }
        // A system directory the drive hasn't is no file to open (05h), and
        // a file in it that isn't a system file is not found (02h), as in
        // a directory that is there, on a disk image as on the host.
        if self.is_system_dir(filename) {
            return Err(0x05);
        }
        if !create && self.in_system_dir(filename) {
            return Err(if mode & 0x03 == 3 { 0x0C } else { 0x02 });
        }
        // Files held in memory are read-only: read/write opens are
        // downgraded as on a CD-ROM.
        if let Some((drive, d, path)) = self.locate_in_memory(filename) {
            let files = d.tree().ok_or(0x03u8)?;
            let data = match files.file(&path).and_then(|node| FileData::of(node, d.image())) {
                Some(FileData::Memory(data)) => OpenData::Memory(data, Rc::new(Cell::new(0))),
                Some(FileData::Image(image, extent)) => OpenData::Image(image, extent, Rc::new(Cell::new(0))),
                _ if files.is_dir(&path) => return Err(0x05),
                _ => {
                    let parent = path.rsplit_once('\\').map_or("", |(p, _)| p);
                    return Err(if files.is_dir(parent) { 0x02 } else { 0x03 });
                }
            };
            match mode & 0x03 {
                0 | 2 => {}
                1 => return Err(0x05),
                _ => return Err(0x0C),
            }
            let handle = self.free_handle()?;
            let key = self.file_key(filename);
            let file = self.opened(data, drive, owner, key, filename, mode);
            self.insert(handle, file);
            return Ok(handle);
        }

        if let Some((drive, volume, parts)) = self.locate_fat(filename) {
            let writable = self.is_writable(drive);
            let access = mode & 0x03;
            match access {
                3 => return Err(0x0C),
                1 if !writable => return Err(0x05),
                _ => {}
            }
            let parts = refs(&parts);
            let handle = self.free_handle()?;
            let entry = match volume.find(&parts) {
                Ok(entry) if entry.is_dir() => return Err(0x05),
                Ok(entry) => entry,
                Err(0x02) if create && writable => volume.create(&parts, 0)?,
                Err(0x02) if create => return Err(0x05),
                Err(e) => return Err(e),
            };
            // Read/write opens on write-protected disks are downgraded, as
            // on CD-ROMs; read-only files can't be written.
            let write = access != 0 && writable;
            if write && entry.attr & fat::ATTR_READ_ONLY != 0 {
                return Err(0x05);
            }
            let at = entry.at.ok_or(0x05u8)?;
            let data = OpenData::Fat { volume, at, pos: Rc::new(Cell::new(0)), write };
            let key = self.file_key(filename);
            let file = self.opened(data, drive, owner, key, filename, mode);
            self.insert(handle, file);
            return Ok(handle);
        }

        let (drive, path, missing) = self.locate_found(filename).ok_or(0x03)?; // Path not found
        // A directory (or a bare "D:") isn't a file: access denied. A name
        // its folder's entries lack names nothing there (`locate_found`):
        // programs poll for files that aren't there, and a stat or an
        // open of a missing name costs the host more than the lookup.
        let existing = missing.is_none().then(|| hostfs::metadata(&path).ok()).flatten();
        if existing.as_ref().is_some_and(|m| m.is_dir) {
            return Err(0x05);
        }
        let writable = self.is_writable(drive);

        let mut options = OpenOptions::new();
        match mode & 0x03 {
            0 => {
                options.read(true);
            }
            1 => {
                if !writable {
                    return Err(0x05);
                }
                options.write(true).create(create).truncate(false);
            }
            2 => {
                // Read/write opens on read-only media (CD-ROM) are quietly
                // downgraded to read-only, as MSCDEX does; lots of CD games
                // open their data files R/W without ever writing.
                if writable {
                    options.read(true).write(true).create(create);
                } else {
                    options.read(true);
                }
            }
            _ => return Err(0x0C),
        }

        let handle = self.free_handle()?;
        let opened = match missing {
            Some(_) if !create => Err(std::io::ErrorKind::NotFound.into()),
            _ => options.open(&path),
        };
        // Opening a file that is there already adds no name to its folder.
        if create && existing.is_none() {
            self.host_dirs.changed();
            self.host_dirs.touched(&path);
        }
        match opened {
            Ok(file) => {
                let key = self.file_key(filename);
                let written = (writable && mode & 0x03 != 0).then_some(path);
                let file = self.opened(OpenData::Host(file, written), drive, owner, key, filename, mode);
                self.insert(handle, file);
                Ok(handle)
            }
            // Missing: file not found (02h) when its folder is there, path
            // not found (03h) when a folder on the way isn't.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(if path.parent().is_some_and(hostfs::is_dir) { 0x02 } else { 0x03 })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => Err(0x03),
            // A name Windows refuses (a wildcard in it) is a file that
            // isn't there, as on other hosts.
            Err(e) if e.kind() == std::io::ErrorKind::InvalidFilename => Err(0x02),
            // A file that is there but can't be opened this way (read-only,
            // in use by another program) is access denied, as DOS has it.
            Err(_) => Err(0x05),
        }
    }

    /// INT 21h, AH=3Ch: Create File. Opens read/write, creating the file
    /// if missing. A file that was there is emptied as DOS does, but only
    /// when the program first reads, writes, seeks, resizes, stamps or
    /// closes the handle, or opens, deletes or renames the file by name:
    /// the old bytes stay on disk until then, so a crash between the
    /// create and the write that follows doesn't lose a save.
    pub fn create_file(&mut self, filename: &str, owner: u16) -> Result<u16, u8> {
        if self.device(filename).is_some() {
            return self.open_file(filename, 0x02, owner);
        }
        let normalized = filename.replace('/', "\\");
        let (drive, _) = self.split_drive(&normalized).ok_or(0x03)?;
        self.check_writable(drive)?;
        self.host_dirs.creating();
        // A system file or directory is there already.
        if self.is_system_entry(filename) {
            return Err(0x05);
        }
        self.make_system_dirs(filename);
        let sft = self.open_or_create(filename, 0x02, owner, true)?;
        if let Some(open) = self.open_files.get_mut(&sft) {
            open.empty_on_use = match &open.data {
                OpenData::Host(file, _) => file.len().is_ok_and(|len| len > 0),
                OpenData::Fat { volume, at, write: true, .. } => volume.reload(*at).is_ok_and(|e| e.size > 0),
                _ => false,
            };
        }
        Ok(sft)
    }

    /// Close the file `sft` that was just opened for a program that got no
    /// handle for it: a file a create call opened keeps its bytes, as DOS
    /// fails such a create before it touches the file.
    pub fn close_unused(&mut self, sft: u16) -> bool {
        if let Some(open) = self.open_files.get_mut(&sft) {
            open.empty_on_use = false;
        }
        self.close_file(sft)
    }

    /// Empty the file `sft` names if a create call left that for its first
    /// use, through whichever handle: DOS emptied it for all of them.
    fn settle(&mut self, sft: u16) {
        if self.open_files.values().any(|f| f.empty_on_use)
            && let Some(key) = self.open_files.get(&sft).map(|f| f.key)
        {
            self.settle_where(|_, f| f.key == key);
        }
    }

    /// Empty the file `sft` if a create call left that for its first use.
    fn settle_one(&mut self, sft: u16) {
        if let Some(open) = self.open_files.get_mut(&sft)
            && open.empty_on_use
        {
            open.empty_on_use = false;
            empty_created(open);
            // A search's listing has the file's size.
            if let OpenData::Host(_, Some(path)) = &open.data {
                self.host_dirs.touched(path);
            }
            self.mark_dirty(sft);
        }
    }

    /// Empty every file a create call left for its first use that
    /// `which` picks.
    pub(super) fn settle_where(&mut self, which: impl Fn(u16, &OpenFile) -> bool) {
        let pending: Vec<u16> =
            self.open_files.iter().filter(|&(&sft, f)| f.empty_on_use && which(sft, f)).map(|(&sft, _)| sft).collect();
        for sft in pending {
            self.settle_one(sft);
        }
    }

    /// Empty the file `filename` names if a create call left that for its
    /// first use: before it is opened again, deleted or renamed, which DOS
    /// does to the file the create already emptied.
    fn settle_named(&mut self, filename: &str) {
        if self.open_files.values().any(|f| f.empty_on_use) {
            let key = self.file_key(filename);
            self.settle_where(|_, f| f.key == key);
        }
    }

    /// INT 21h, AH=5Bh: create a file that must not exist yet.
    pub fn create_new_file(&mut self, filename: &str, owner: u16) -> Result<u16, u8> {
        self.host_dirs.creating();
        if self.device(filename).is_none() && self.exists(filename) {
            return Err(0x50); // File exists
        }
        self.create_file(filename, owner)
    }

    /// INT 21h, AH=5Ah: create a file with a unique name in `directory`
    /// (which ends in a backslash). Returns the handle and the name.
    pub fn create_temp_file(&mut self, directory: &str, owner: u16) -> Result<(u16, String), u8> {
        let stamp = crate::hosttime::now().timestamp_subsec_nanos();
        for i in 0..1000u32 {
            let name = format!("{}{:08X}", directory, stamp.wrapping_add(i) & 0x0FFF_FFFF);
            if let Ok(handle) = self.create_new_file(&name, owner) {
                return Ok((handle, name));
            }
        }
        Err(0x05)
    }

    /// Write a whole file at once, as COPY does: `data` at `dos_path`,
    /// replacing a file that is there, dated `stamp` (the packed DOS time
    /// and date) or now.
    pub fn write_whole_file(&mut self, dos_path: &str, data: &[u8], stamp: Option<(u16, u16)>) -> Result<(), u8> {
        self.host_dirs.creating();
        // Before the file is replaced: a FAT image's entry may move.
        self.settle_named(dos_path);
        if self.is_system_entry(dos_path) {
            return Err(0x05);
        }
        self.make_system_dirs(dos_path);
        if let Some((drive, volume, parts)) = self.locate_fat(dos_path) {
            self.check_writable(drive)?;
            let (time, date) = stamp.unwrap_or_else(fat::dos_now);
            return volume.put_file(&refs(&parts), data, time, date);
        }
        if self.locate_in_memory(dos_path).is_some() {
            return Err(0x05);
        }
        let (drive, path) = self.locate(dos_path).ok_or(0x03)?;
        self.check_writable(drive)?;
        if hostfs::is_dir(&path) {
            return Err(0x05);
        }
        if !path.parent().is_some_and(hostfs::is_dir) {
            return Err(0x03);
        }
        // A read-only file isn't replaced, even where the host would let
        // us (root on Unix writes files without write permission).
        if hostfs::metadata(&path).is_ok_and(|m| m.readonly) {
            return Err(0x05);
        }
        let written = hostfs::write(&path, data);
        self.host_dirs.changed();
        self.host_dirs.touched(&path);
        written.map_err(|_| 0x05)?;
        if let Some(when) = stamp.and_then(|(time, date)| dos_to_system_time(time, date)) {
            let _ = OpenOptions::new().write(true).open(&path).and_then(|f| f.set_modified(when));
        }
        Ok(())
    }

    /// INT 21h, AH=41h: delete a file.
    pub fn delete_file(&mut self, filename: &str) -> Result<(), u8> {
        // A disk image's entry must not be emptied after it is freed.
        self.settle_named(filename);
        if self.system_file(filename).is_some() {
            return Err(0x05);
        }
        // A system directory the drive hasn't, or a file in it, is not
        // found (02h), as a directory or a missing file is on the host.
        if self.is_system_dir(filename) || self.in_system_dir(filename) {
            return Err(0x02);
        }
        if let Some((drive, volume, parts)) = self.locate_fat(filename) {
            let parts = refs(&parts);
            let entry = volume.find(&parts)?;
            if entry.is_dir() {
                return Err(0x02);
            }
            self.check_writable(drive)?;
            if entry.attr & fat::ATTR_READ_ONLY != 0 {
                return Err(0x05);
            }
            return volume.remove(&parts);
        }
        let (drive, path) = self.locate(filename).ok_or(0x03)?;
        if !hostfs::is_file(&path) {
            return Err(0x02); // File not found
        }
        self.check_writable(drive)?;
        let removed = hostfs::remove_file(&path);
        self.host_dirs.changed();
        self.host_dirs.touched(&path);
        removed.map_err(|_| 0x05)
    }

    /// INT 21h, AH=56h: rename or move a file within a drive.
    pub fn rename_file(&mut self, from: &str, to: &str) -> Result<(), u8> {
        // A disk image's entry may move: empty the file while it is where
        // the handle has it.
        self.settle_named(from);
        // A system file is read-only, and a system directory has them.
        if self.is_system_entry(from) || self.is_system_entry(to) {
            return Err(0x05);
        }
        // A file in a system directory the drive hasn't is not found
        // (02h), on a disk image as on the host.
        if self.in_system_dir(from) {
            return Err(0x02);
        }
        self.make_system_dirs(to);
        if let Some((drive, volume, from_parts)) = self.locate_fat(from) {
            let from_parts = refs(&from_parts);
            if from_parts.is_empty() {
                return Err(0x05);
            }
            volume.find(&from_parts)?;
            self.check_writable(drive)?;
            let normalized = to.replace('/', "\\");
            let (to_drive, rest) = self.split_drive(&normalized).ok_or(0x03)?;
            if to_drive != drive {
                return Err(0x11); // Not same device
            }
            let to_parts = Self::logical_components(self.drive(drive).ok_or(0x03u8)?, rest);
            if to_parts.is_empty() {
                return Err(0x03);
            }
            return volume.rename(&from_parts, &to_parts);
        }
        let (drive, source) = self.locate(from).ok_or(0x03)?;
        if !hostfs::exists(&source) {
            return Err(0x02);
        }
        self.check_writable(drive)?;
        let normalized = to.replace('/', "\\");
        let (to_drive, rest) = self.split_drive(&normalized).ok_or(0x03)?;
        if to_drive != drive {
            return Err(0x11); // Not same device
        }
        let (parent_dos, leaf) = match rest.rsplit_once('\\') {
            Some(("", l)) => ("\\", l),
            Some((p, l)) => (p, l),
            None => (".", rest),
        };
        let parent = self.resolve_on(to_drive, parent_dos).ok_or(0x03)?;
        if leaf.is_empty() || !hostfs::is_dir(&parent) {
            return Err(0x03);
        }
        if self.find_existing_child(&parent, leaf).is_some() {
            return Err(0x05); // Destination exists
        }
        let target = parent.join(leaf.to_uppercase());
        let renamed = hostfs::rename(&source, &target);
        self.host_dirs.changed();
        self.host_dirs.touched(&source);
        self.host_dirs.touched(&target);
        renamed.map_err(|_| 0x05)
    }

    /// INT 21h, AX=5700h: the DOS time and date of a file's last change.
    pub fn file_time(&self, handle: u16) -> Result<(u16, u16), u8> {
        let open = self.open_files.get(&handle).ok_or(0x06)?;
        let modified = match &open.data {
            OpenData::Host(f, _) => f.modified(),
            OpenData::Memory(..) => return Ok((MEMORY_TIME, MEMORY_DATE)),
            OpenData::Image(_, extent, _) => return Ok((extent.time, extent.date)),
            OpenData::Fat { volume, at, .. } => {
                let entry = volume.reload(*at)?;
                return Ok((entry.time, entry.date));
            }
            OpenData::Device(_) => None,
        };
        let t: DateTime<Local> = modified.map_or_else(crate::hosttime::now, DateTime::from);
        let time = (t.hour() << 11 | t.minute() << 5 | (t.second() / 2)) as u16;
        let year = (t.year().max(1980) - 1980) as u32;
        let date = (year << 9 | t.month() << 5 | t.day()) as u16;
        Ok((time, date))
    }

    /// INT 21h AX=5701h: set the time and date of an open file. Only
    /// files on disk images keep them; host files keep their own.
    pub fn set_file_time(&mut self, handle: u16, time: u16, date: u16) -> Result<(), u8> {
        // Emptying a disk-image file at its first use stamps it with the
        // current time, which would replace the one set here.
        self.settle(handle);
        match &self.open_files.get(&handle).ok_or(0x06u8)?.data {
            OpenData::Fat { volume, at, .. } => volume.set_time(*at, time, date),
            _ => Ok(()),
        }
    }

    /// True if the file table entry `sft` is an open file.
    pub fn is_open(&self, sft: u16) -> bool {
        self.open_files.contains_key(&sft)
    }

    /// One more handle (or FCB) refers to the open file `sft`.
    pub fn add_ref(&mut self, sft: u16) -> bool {
        let Some(open) = self.open_files.get_mut(&sft) else { return false };
        open.refs = open.refs.saturating_add(1);
        self.mark_dirty(sft);
        true
    }

    /// A handle (or FCB) that referred to the open file `sft` is closed
    /// (INT 21h AH=3Eh): the file closes with the last of them, but the
    /// standard devices stay. False if it isn't open.
    pub fn close_file(&mut self, sft: u16) -> bool {
        self.settle(sft);
        let Some(open) = self.open_files.get_mut(&sft) else { return false };
        open.refs = open.refs.saturating_sub(1);
        if open.refs == 0 && sft >= FIRST_FILE {
            // The host may set a written file's time when it is closed.
            if let Some(OpenFile { data: OpenData::Host(_, Some(path)), .. }) = self.open_files.remove(&sft) {
                self.host_dirs.touched(&path);
            }
        }
        self.mark_dirty(sft);
        true
    }

    /// Whether a process started now gets a handle for the open file
    /// `sft`: unless it was opened with the no-inherit bit (80h).
    pub fn inheritable(&self, sft: u16) -> bool {
        self.open_files.get(&sft).is_some_and(|f| f.mode & 0x80 == 0 && !f.fcb)
    }

    /// The access mode the open file `sft` was opened with (INT 21h
    /// AH=3Dh's AL).
    pub fn open_mode(&self, sft: u16) -> Option<u8> {
        self.open_files.get(&sft).map(|f| f.mode)
    }

    /// The open file `sft` is an FCB's.
    pub fn set_fcb(&mut self, sft: u16) {
        if let Some(open) = self.open_files.get_mut(&sft) {
            open.fcb = true;
            self.mark_dirty(sft);
        }
    }

    /// Close the files a terminating process opened for FCBs, which no
    /// handle refers to.
    pub fn close_fcb_files(&mut self, owner: u16) {
        let gone: Vec<u16> =
            self.open_files.iter().filter(|(_, f)| f.fcb && f.owner == owner).map(|(&sft, _)| sft).collect();
        for sft in gone {
            self.open_files.remove(&sft);
            self.mark_dirty(sft);
        }
    }

    /// Close every open file, for when the shell is reloaded. A file a
    /// create call left for its first use keeps its bytes: the program
    /// was aborted or the machine reset before it wrote its save.
    pub fn close_all_files(&mut self) {
        self.open_files.clear();
        self.open_standard_devices();
    }

    // INT 21h, AH=3Fh: Read from File
    pub fn read_file(&mut self, handle: u16, count: usize) -> Result<Vec<u8>, u16> {
        self.settle(handle);
        self.mark_moved(handle);
        if let Some(open) = self.open_files.get_mut(&handle) {
            let file = match &mut open.data {
                OpenData::Host(file, _) => file,
                OpenData::Memory(data, pos) => {
                    let start = (pos.get() as usize).min(data.len());
                    let end = start.saturating_add(count).min(data.len());
                    pos.set(pos.get() + (end - start) as u64);
                    return Ok(data[start..end].to_vec());
                }
                OpenData::Image(image, extent, pos) => {
                    let mut buffer = vec![0u8; count];
                    // A sector a drive can't read is a data (CRC) error,
                    // device error 4 plus 19, as MSCDEX reports it; any
                    // other failure is a read fault.
                    let n = image
                        .read_extent(extent, pos.get(), &mut buffer)
                        .map_err(|e| {
                            if crate::cdrom::image::is_unreadable(&e) {
                                0x17u16
                            } else {
                                0x1E
                            }
                        })?;
                    buffer.truncate(n);
                    pos.set(pos.get() + n as u64);
                    return Ok(buffer);
                }
                OpenData::Fat { volume, at, pos, .. } => {
                    let entry = volume.reload(*at)?;
                    let mut buffer = vec![0u8; count];
                    let n = volume.read(&entry, pos.get(), &mut buffer)?;
                    buffer.truncate(n);
                    pos.set(pos.get() + n as u64);
                    return Ok(buffer);
                }
                OpenData::Device(_) => return Ok(Vec::new()),
            };
            let mut buffer = vec![0u8; count];
            match file.read(&mut buffer) {
                Ok(bytes_read) => {
                    buffer.truncate(bytes_read);
                    Ok(buffer)
                }
                Err(_) => Err(0x05),
            }
        } else {
            Err(0x06)
        }
    }

    // INT 21h, AH=40h: Write to File
    pub fn write_file(&mut self, handle: u16, data: &[u8]) -> Result<u16, u8> {
        self.settle(handle);
        self.mark_dirty(handle);
        if let Some(open) = self.open_files.get_mut(&handle) {
            let file = match &mut open.data {
                OpenData::Host(file, path) => {
                    // A search's listing has the file's size and date.
                    if let Some(path) = path {
                        self.host_dirs.touched(path);
                    }
                    file
                }
                OpenData::Memory(..) | OpenData::Image(..) => return Err(0x05),
                OpenData::Fat { write: false, .. } => return Err(0x05),
                OpenData::Fat { volume, at, pos, .. } => {
                    let n = volume.write(*at, pos.get(), data)?;
                    pos.set(pos.get() + n as u64);
                    return Ok(n as u16);
                }
                OpenData::Device(_) => return Ok(data.len() as u16),
            };
            match file.write(data) {
                Ok(bytes_written) => Ok(bytes_written as u16),
                Err(_) => Err(0x05),
            }
        } else {
            Err(0x06)
        }
    }

    /// Seek in a file of `len` bytes that isn't a host file: past the end
    /// is fine, before the start is not.
    fn seek_in(len: u64, pos: &Cell<u64>, offset: i64, origin: u8) -> Result<u64, u16> {
        let base = match origin {
            0 => 0,
            1 => pos.get() as i64,
            2 => len as i64,
            _ => return Err(0x01),
        };
        let at = base.checked_add(offset).filter(|&at| at >= 0).ok_or(0x19u16)? as u64;
        pos.set(at);
        Ok(at)
    }

    // INT 21h, AH=42h: Seek
    pub fn seek_file(&mut self, handle: u16, offset: i64, origin: u8) -> Result<u64, u16> {
        self.settle(handle);
        self.mark_moved(handle);
        if let Some(open) = self.open_files.get_mut(&handle) {
            let file = match &mut open.data {
                OpenData::Host(file, _) => file,
                OpenData::Memory(data, pos) => return Self::seek_in(data.len() as u64, pos, offset, origin),
                OpenData::Image(_, extent, pos) => return Self::seek_in(extent.size as u64, pos, offset, origin),
                OpenData::Fat { volume, at, pos, .. } => {
                    let size = volume.reload(*at)?.size;
                    return Self::seek_in(size as u64, pos, offset, origin);
                }
                OpenData::Device(_) => return Ok(0),
            };
            let seek_from = match origin {
                0 => SeekFrom::Start(offset as u64),
                1 => SeekFrom::Current(offset),
                2 => SeekFrom::End(offset),
                _ => return Err(0x01),
            };
            match file.seek(seek_from) {
                Ok(new_pos) => Ok(new_pos),
                Err(_) => Err(0x19),
            }
        } else {
            Err(0x06)
        }
    }

    /// Make an open file `size` bytes long, cutting it or adding zeros
    /// (INT 21h AH=28h with no records, AH=40h with none to write). Host
    /// files and files on disk images opened for writing can be cut; the
    /// others grow with zeros written at their end, and get 27h when the
    /// disk fills up first. Files that can't be written get 05h. The
    /// handle's position stays where it was.
    pub fn set_file_size(&mut self, handle: u16, size: u64) -> Result<(), u8> {
        self.settle(handle);
        let open = self.open_files.get(&handle).ok_or(0x06u8)?;
        match &open.data {
            OpenData::Host(file, path) => {
                if let Some(path) = path {
                    self.host_dirs.touched(path);
                }
                let result = file.set_len(size).map_err(|_| 0x05);
                self.mark_dirty(handle);
                return result;
            }
            OpenData::Memory(..) | OpenData::Image(..) | OpenData::Fat { write: false, .. } => return Err(0x05),
            OpenData::Fat { volume, at, .. } if size < volume.reload(*at)?.size as u64 => {
                let result = volume.truncate(*at, size);
                self.mark_dirty(handle);
                return result;
            }
            OpenData::Fat { .. } | OpenData::Device(_) => {}
        }
        let was = self.position(handle).unwrap_or(0);
        let mut end = self.seek_file(handle, 0, 2).map_err(|_| 0x05u8)?;
        // A piece at a time, so a position far past the end doesn't take a
        // buffer that big. A full disk stops it short with 27h.
        let zeros = vec![0u8; size.saturating_sub(end).min(0x8000) as usize];
        let mut result = if end > size { Err(0x05) } else { Ok(()) };
        while result.is_ok() && end < size {
            let n = (size - end).min(zeros.len() as u64) as usize;
            result = match self.write_file(handle, &zeros[..n]) {
                Ok(written) if written as usize == n => Ok(()),
                Ok(_) => Err(0x27),
                Err(code) => Err(code),
            };
            end += n as u64;
        }
        let _ = self.seek_file(handle, was as i64, 0);
        result
    }
}
