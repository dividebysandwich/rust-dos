use chrono::{DateTime, Datelike, Local, Timelike};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

// DOS defines standard handles: 0=Stdin, 1=Stdout, 2=Stderr, 3=Aux, 4=Printer
pub const FIRST_USER_HANDLE: u16 = 5;

/// Helper struct to transfer directory search results back to the CPU
#[allow(dead_code)]
pub struct DosDirEntry {
    pub filename: String,
    pub size: u32,
    pub is_dir: bool,
    pub is_readonly: bool,
    pub dos_time: u16,
    pub dos_date: u16,
}

pub struct DiskController {
    // Map DOS Handle (u16) -> Rust File Object
    open_files: HashMap<u16, File>,
    next_handle: u16,

    // File System State
    root_path: PathBuf,                      // The host directory acting as C:\
    current_dir: String,                     // The current DOS directory (e.g., "GAMES\DOOM")
    current_drive: u8,                       // 0=A, ... 2=C, ... 25=Z
    virtual_files: HashMap<String, Vec<u8>>, // In-memory files for Z: drive
}

impl DiskController {
    pub fn new(root_path: PathBuf) -> Self {
        // Ensure root path exists
        if !root_path.exists() {
            println!(
                "[DISK] Warning: Root path {:?} does not exist. Creating it.",
                root_path
            );
            let _ = fs::create_dir_all(&root_path);
        }

        let canonical = fs::canonicalize(&root_path).unwrap_or(root_path);

        let mut virtual_files = HashMap::new();
        // Create a dummy COMMAND.COM on Z:
        virtual_files.insert("COMMAND.COM".to_string(), vec![0x90; 5000]);

        Self {
            open_files: HashMap::new(),
            next_handle: FIRST_USER_HANDLE,
            root_path: canonical,
            current_dir: String::new(),
            current_drive: 2, // Default to C:
            virtual_files,
        }
    }

    pub fn set_current_drive(&mut self, drive: u8) -> u8 {
        // Only allow switching to C (2) or Z (25) for now
        // Return the number of logical drives (26)
        if drive == 2 || drive == 25 {
            self.current_drive = drive;
        }
        26
    }

    pub fn get_current_drive(&self) -> u8 {
        self.current_drive
    }

    /// Resolves a DOS path (e.g., "GAMES\DOOM.EXE" or "..\FILE.TXT")
    /// to a Host Path, ensuring it stays within `root_path`.
    /// Handles case-insensitivity and short filenames (8.3).
    pub fn resolve_path(&self, dos_path: &str) -> Option<PathBuf> {
        // 1. Normalize Separators and Uppercase
        let path_str = dos_path.replace('/', "\\");

        // 2. Handle Drive Letter (Strip "C:")
        let mut drive = self.current_drive;
        let mut clean_path_str = path_str.clone();

        // 2. Handle Drive Letter (e.g. "C:...")
        if path_str.len() >= 2 && &path_str[1..2] == ":" {
            let drive_char = path_str.chars().next().unwrap().to_ascii_uppercase();
            if drive_char >= 'A' && drive_char <= 'Z' {
                drive = (drive_char as u8) - b'A';
                clean_path_str = path_str[2..].to_string();
            } else {
                return None; // Invalid drive
            }
        }

        // If trying to access Z:, ensure it is a virtual path
        if drive == 25 {
            // Z: drive (Virtual)
            // We return a special "PathBuf" which won't likely exist on host,
            // but we can check `virtual_files` later?
            // Actually, `resolve_path` returns `PathBuf` which is then used by `fs::` calls.
            // This is a problem for virtual files.
            // However, our `open_file` logic can check for Z: BEFORE calling `fs::open`.
            // But `resolve_path` is also used for directory listing.

            // For now, let's map Z: to a non-existent host path so standard FS calls fail,
            // but we can recognize it.
            // Or better: `resolve_path` is designed to return a Host Path.
            // If it's a virtual file, we can't return a Host Path.
            // Refactoring `resolve_path` to return an Enum would be big.
            // Let's rely on callers checking drive/path logic.

            // Wait, if I return None, `open_file` errors "Path not found".
            // If I return a dummy path, `fs::open` errors "File not found".

            // Let's modify `open_file` and `find_directory_entry` to check for Z: usage explicitly.
            // Here, we just return None for Z: to indicate "Not on Host Disk C".
            // BUT, `current_drive` matters.
            return None;
        }

        if drive != 2 {
            // We only support C: for actual Disk I/O currently.
            return None;
        }

        let clean_path = &clean_path_str;

        let is_absolute = clean_path.starts_with('\\');

        // Build a list of logical components to traverse
        let mut components: Vec<&str> = Vec::new();

        if !is_absolute && !self.current_dir.is_empty() {
            for part in self.current_dir.split('\\') {
                if !part.is_empty() {
                    components.push(part);
                }
            }
        }

        for part in clean_path.split('\\') {
            if part == "." || part.is_empty() {
                continue;
            }
            if part == ".." {
                components.pop();
            } else {
                components.push(part);
            }
        }

        // Traverse and Resolve to Host Paths
        let mut full_path = self.root_path.clone();

        for part in components {
            // Security Check: If we pop below root, it's invalid?
            // ".." handling above prevents growing stack incorrectly,
            // but we must ensure we don't traverse out.
            // Since we rebuild from root, ".." popping from vector works.

            let actual_name = self.find_host_child(&full_path, part);
            full_path.push(actual_name);
        }

        // Final Security Check
        if full_path.starts_with(&self.root_path) {
            Some(full_path)
        } else {
            None
        }
    }

    // Helper to check if a file exists on Z:
    pub fn is_virtual_file(&self, filename: &str) -> bool {
        // Check if explicit Z:
        let upper = filename.to_ascii_uppercase();
        if upper.starts_with("Z:") {
            let name = &upper[2..];
            // Remove leading slash if any
            let name = name.trim_start_matches('\\').trim_start_matches('/');
            return self.virtual_files.contains_key(name);
        }

        // If current drive is Z:
        if self.current_drive == 25 {
            let name = upper.trim_start_matches('\\').trim_start_matches('/');
            return self.virtual_files.contains_key(name);
        }

        false
    }

    // Helper to get virtual file size
    pub fn get_virtual_file_size(&self, filename: &str) -> u32 {
        let upper = filename.to_ascii_uppercase();
        // Simplistic stripping
        let name = if upper.starts_with("Z:") {
            &upper[2..]
        } else {
            &upper
        };
        let name = name.trim_start_matches('\\').trim_start_matches('/');

        if let Some(data) = self.virtual_files.get(name) {
            return data.len() as u32;
        }
        0
    }

    /// Helper to find a child in a directory matching DOS semantics
    /// (Case-Insensitive OR Short Filename match)
    fn find_host_child(&self, dir: &Path, target: &str) -> String {
        // Read directory and sort for deterministic short names
        let mut entries: Vec<String> = Vec::new();
        if let Ok(read_dir) = fs::read_dir(dir) {
            for entry in read_dir.flatten() {
                entries.push(entry.file_name().to_string_lossy().to_string());
            }
        }
        entries.sort(); // Ensure ~1 order is consistent

        let target_upper = target.to_ascii_uppercase();
        let mut generated_counts: HashMap<String, usize> = HashMap::new();

        for name in entries {
            if name.starts_with('.') {
                continue;
            }

            // 1. Exact/Case-Insensitive Match
            if name.eq_ignore_ascii_case(target) {
                return name;
            }

            // 2. Short Name Match
            // Generate Short Name for this entry
            let (stem, ext) = Self::to_short_name(&name);
            let base_key = if ext.is_empty() {
                stem.clone()
            } else {
                format!("{}.{}", stem, ext)
            };

            let count = *generated_counts.get(&base_key).unwrap_or(&0);
            let final_short_name = if count == 0 {
                generated_counts.insert(base_key, 1);
                if ext.is_empty() {
                    stem
                } else {
                    format!("{}.{}", stem, ext)
                }
            } else {
                generated_counts.insert(base_key, count + 1);
                let suffix = format!("~{}", count);
                let available_len = 8usize.saturating_sub(suffix.len());
                let short_stem = if stem.len() > available_len {
                    &stem[0..available_len]
                } else {
                    &stem
                };

                if ext.is_empty() {
                    format!("{}{}", short_stem, suffix)
                } else {
                    format!("{}{}.{}", short_stem, suffix, ext)
                }
            };

            if final_short_name == target_upper {
                return name; // Found the host file corresponding to the short name
            }
        }

        // Not found? Return target as uppercase (default for creation)
        target.to_ascii_uppercase()
    }

    // ========================================================================
    // DIR OPERATIONS
    // ========================================================================

    pub fn set_current_directory(&mut self, path: &str) -> bool {
        // Resolve the new path to check existence
        if let Some(host_path) = self.resolve_path(path) {
            if host_path.exists() && host_path.is_dir() {
                // Update self.current_dir
                // We need to store the DOS representation (relative to root)

                // One way: strip root_path from host_path
                if let Ok(suffix) = host_path.strip_prefix(&self.root_path) {
                    self.current_dir = suffix.to_string_lossy().replace('/', "\\");
                    return true;
                }
            }
        }
        false
    }

    pub fn get_current_directory(&self) -> String {
        self.current_dir.to_ascii_uppercase()
    }

    // ========================================================================
    // FILE I/O OPERATIONS
    // ========================================================================

    // INT 21h, AH=3Dh: Open File
    pub fn open_file(&mut self, filename: &str, mode: u8) -> Result<u16, u8> {
        // Handle Virtual Z: files
        if self.is_virtual_file(filename) {
            // For now, we don't support actually reading/seeking virtual files with standard file handles
            // nicely. We will just return a dummy handle and special case read/seek if needed?
            // OR: We return an error if we don't support it, but since we just want EXEC to work,
            // we might not need `open_file` to succeed for COMMAND.COM unless NC tries to read it.
            // NC *does* check if COMMAND.COM exists.

            // Let's create a temporary file or use a special handle range?
            // Using a special handle range is cleaner.
            let handle = 0xAA00 + (self.next_handle % 100);
            self.next_handle += 1;
            // storing nothing in `open_files` means read/write fails, which is fine for now
            // or we could store a special marker.
            // For this specific task (EXEC), NC just needs to know it exists or "load" it (which uses EXEC).
            // EXEC loading handles file reading itself usually via `load_executable` in `cpu.rs`
            // (which uses `open_file`? No, `load_executable` uses `disk.read_file` maybe?
            // `cpu.load_executable` uses `disk.open_file` -> `read_file` flow usually).
            // We'll see. If `open_file` succeeds, that's step 1.
            return Ok(handle);
        }

        let path = self.resolve_path(filename).ok_or(0x03)?; // Path not found

        let mut options = OpenOptions::new();
        match mode & 0x03 {
            0 => {
                options.read(true);
            }
            1 => {
                options.write(true).create(true).truncate(false);
            } // logic tweak for safety
            2 => {
                options.read(true).write(true).create(true);
            }
            _ => return Err(0x0C),
        }

        match options.open(path) {
            Ok(f) => {
                let handle = self.next_handle;
                self.next_handle += 1;
                self.open_files.insert(handle, f);
                // println!("[DISK] Opened '{}' as Handle {}", filename, handle);
                Ok(handle)
            }
            Err(_) => Err(0x02),
        }
    }

    // INT 21h, AH=3Eh: Close File
    pub fn close_file(&mut self, handle: u16) -> bool {
        self.open_files.remove(&handle).is_some()
    }

    // INT 21h, AH=3Fh: Read from File
    pub fn read_file(&mut self, handle: u16, count: usize) -> Result<Vec<u8>, u16> {
        if let Some(file) = self.open_files.get_mut(&handle) {
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
        if let Some(file) = self.open_files.get_mut(&handle) {
            match file.write(data) {
                Ok(bytes_written) => Ok(bytes_written as u16),
                Err(_) => Err(0x05),
            }
        } else {
            Err(0x06)
        }
    }

    // INT 21h, AH=42h: Seek
    pub fn seek_file(&mut self, handle: u16, offset: i64, origin: u8) -> Result<u64, u16> {
        if let Some(file) = self.open_files.get_mut(&handle) {
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

    // ========================================================================
    // FILESYSTEM METADATA & SEARCH
    // ========================================================================

    // INT 21h, AH=36h: Get Disk Free Space
    // Input DL: 0=Default, 1=A, 2=B, 3=C, ...
    pub fn get_disk_free_space(&self, drive: u8) -> Result<(u16, u16, u16, u16), u16> {
        let target_drive = if drive == 0 {
            self.current_drive
        } else {
            drive - 1
        };

        if target_drive == 2 {
            // C: drive (Fake 80MB)
            Ok((8, 20000, 512, 20000))
        } else if target_drive == 25 {
            // Z: drive (Virtual, read-only, small)
            Ok((1, 1000, 512, 2000))
        } else {
            Err(0x0F) // Invalid Drive
        }
    }

    // INT 21h, AH=43h: Get File Attributes
    // Returns: Attribute Byte (0x20 = Archive, 0x10 = Subdir, etc.)
    #[allow(dead_code)]
    pub fn get_file_attribute(&self, filename: &str) -> Result<u16, u8> {
        let path = self.resolve_path(filename).ok_or(0x03)?;
        if path.exists() {
            if path.is_dir() {
                Ok(0x10) // Directory
            } else {
                Ok(0x20) // Archive (Standard File)
            }
        } else {
            Err(0x02) // File Not Found
        }
    }

    // Returns the path string relative to root, e.g., "GAMES\DOOM"
    fn to_short_name(filename: &str) -> (String, String) {
        let filename = filename.to_uppercase();

        let (stem, ext) = match filename.rsplit_once('.') {
            Some((s, e)) => (s, e),
            None => (filename.as_str(), ""),
        };

        // Filter invalid chars
        let mut clean_stem: String = stem
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || "!@#$%^&()-_'{}`~".contains(*c))
            .collect();

        let mut clean_ext: String = ext.chars().filter(|c| c.is_ascii_alphanumeric()).collect();

        if clean_ext.len() > 3 {
            clean_ext.truncate(3);
        }
        if clean_stem.len() > 8 {
            clean_stem.truncate(8);
        }

        if clean_stem.is_empty() {
            clean_stem = "NONAME".to_string();
        }

        (clean_stem, clean_ext)
    }

    /// Helper: Simple DOS wildcard matching (? and *)
    fn matches_pattern(filename: &str, pattern: &str) -> bool {
        if pattern == "*.*" {
            return true;
        }

        // Split filename and pattern by '.'
        let (f_name, f_ext) = filename.split_once('.').unwrap_or((filename, ""));
        let (p_name, p_ext) = pattern.split_once('.').unwrap_or((pattern, ""));

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

        match_part(f_name, p_name) && match_part(f_ext, p_ext)
    }

    // INT 21h, AH=4E/4F: Find First / Find Next
    // search_spec contains the path AND the pattern e.g. "C:\GAMES\*.EXE" or "*.EXE"
    pub fn find_directory_entry(
        &self,
        search_spec: &str,
        search_index: usize,
        search_attr: u16,
    ) -> Result<DosDirEntry, u8> {
        // Handle Volume Label request
        if (search_attr & 0x08) != 0 {
            if search_index == 0 {
                return Ok(DosDirEntry {
                    filename: "RUSTDOS".to_string(),
                    size: 0,
                    is_dir: false,
                    is_readonly: false,
                    dos_time: 0x0000,
                    dos_date: 0x5021,
                });
            } else {
                return Err(0x12);
            }
        }

        // Split Spec into Directory and Pattern manually
        let (parent_dir, pattern) =
            if let Some(idx) = search_spec.rfind(|c| c == '\\' || c == '/' || c == ':') {
                let (dir, pat) = search_spec.split_at(idx + 1);
                (dir, pat)
            } else {
                ("", search_spec)
            };

        let search_dir_str = if parent_dir.is_empty() {
            "."
        } else {
            parent_dir
        };

        // Z: Drive Detection
        let is_z_drive =
            self.current_drive == 25 || search_spec.to_ascii_uppercase().starts_with("Z:");

        let mut valid_entries: Vec<DosDirEntry> = Vec::new();

        if is_z_drive {
            // Virtual Z: Drive Listing
            // Currently only populating valid_entries with virtual files that match pattern
            for (fname, data) in &self.virtual_files {
                if Self::matches_pattern(fname, &pattern) {
                    valid_entries.push(DosDirEntry {
                        filename: fname.clone(),
                        size: data.len() as u32,
                        is_dir: false,
                        is_readonly: true,
                        dos_time: 0x0000,
                        dos_date: 0x5021,
                    });
                }
            }
        } else {
            // Host Filesystem Listing
            let host_dir = self.resolve_path(search_dir_str).ok_or(0x03)?;

            let read_dir = fs::read_dir(&host_dir).map_err(|_| 0x03)?;
            let mut all_entries: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();
            all_entries.sort_by_key(|dir_entry| dir_entry.file_name());

            let mut generated_names: HashMap<String, usize> = HashMap::new();

            let is_host_root = host_dir == self.root_path;

            if !is_host_root {
                if Self::matches_pattern("..", &pattern) {
                    valid_entries.push(DosDirEntry {
                        filename: "..".to_string(),
                        size: 0,
                        is_dir: true,
                        is_readonly: false,
                        dos_time: 0,
                        dos_date: 0,
                    });
                }
                if Self::matches_pattern(".", &pattern) {
                    valid_entries.push(DosDirEntry {
                        filename: ".".to_string(),
                        size: 0,
                        is_dir: true,
                        is_readonly: false,
                        dos_time: 0,
                        dos_date: 0,
                    });
                }
            }

            for entry in all_entries {
                let original_name = entry.file_name().to_string_lossy().into_owned();

                if original_name.starts_with('.') {
                    continue;
                }

                let metadata = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };

                let is_dir = metadata.is_dir();
                let mut file_attr = if is_dir { 0x10 } else { 0x20 };
                if metadata.permissions().readonly() {
                    file_attr |= 0x01;
                }

                let restricted_bits = 0x02 | 0x04 | 0x10;
                if (file_attr & restricted_bits) & !search_attr != 0 {
                    continue;
                }

                let (stem, ext) = Self::to_short_name(&original_name);
                let base_key = if ext.is_empty() {
                    stem.clone()
                } else {
                    format!("{}.{}", stem, ext)
                };

                let count = *generated_names.get(&base_key).unwrap_or(&0);

                let final_name = if count == 0 {
                    generated_names.insert(base_key, 1);
                    if ext.is_empty() {
                        stem
                    } else {
                        format!("{}.{}", stem, ext)
                    }
                } else {
                    generated_names.insert(base_key.clone(), count + 1);
                    let suffix = format!("~{}", count);
                    let available_len = 8usize.saturating_sub(suffix.len());
                    let short_stem = if stem.len() > available_len {
                        &stem[0..available_len]
                    } else {
                        &stem
                    };

                    if ext.is_empty() {
                        format!("{}{}", short_stem, suffix)
                    } else {
                        format!("{}{}.{}", short_stem, suffix, ext)
                    }
                };

                if !Self::matches_pattern(&final_name, &pattern) {
                    continue;
                }

                let sys_time = metadata.modified().unwrap_or(std::time::SystemTime::now());
                let datetime: DateTime<Local> = sys_time.into();
                let dos_time = ((datetime.hour() as u16) << 11)
                    | ((datetime.minute() as u16) << 5)
                    | ((datetime.second() as u16) / 2);
                let year = datetime.year();
                let dos_date = if year < 1980 {
                    0x0021
                } else {
                    (((year - 1980) as u16) << 9)
                        | ((datetime.month() as u16) << 5)
                        | (datetime.day() as u16)
                };

                valid_entries.push(DosDirEntry {
                    filename: final_name,
                    size: metadata.len() as u32,
                    is_dir: metadata.is_dir(),
                    is_readonly: metadata.permissions().readonly(),
                    dos_time,
                    dos_date,
                });
            }
        }

        if search_index < valid_entries.len() {
            Ok(valid_entries.remove(search_index))
        } else {
            Err(0x12)
        }
    }
}
