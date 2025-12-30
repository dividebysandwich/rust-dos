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
    root_path: PathBuf,  // The host directory acting as C:\
    current_dir: String, // The current DOS directory (e.g., "GAMES\DOOM")
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

        Self {
            open_files: HashMap::new(),
            next_handle: FIRST_USER_HANDLE,
            root_path: canonical,
            current_dir: String::new(), // Root is empty string or "\"
        }
    }

    /// Resolves a DOS path (e.g., "GAMES\DOOM.EXE" or "..\FILE.TXT")
    /// to a Host Path, ensuring it stays within `root_path`.
    /// Handles case-insensitivity and short filenames (8.3).
    pub fn resolve_path(&self, dos_path: &str) -> Option<PathBuf> {
        // 1. Normalize Separators and Uppercase
        let path_str = dos_path.replace('/', "\\");

        // 2. Handle Drive Letter (Strip "C:")
        let clean_path = if path_str.len() >= 2 && &path_str[1..2] == ":" {
            if path_str.to_ascii_uppercase().starts_with("C:") {
                &path_str[2..]
            } else {
                return None;
            }
        } else {
            &path_str
        };

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
        if drive == 0 || drive == 3 || drive == 2 {
            // Fake 80MB drive
            Ok((8, 20000, 512, 20000))
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

        // Split Spec into Directory and Pattern manually (Path::new is platform specific)
        // Find the last separator (\, /, or :)
        let (parent_dir, pattern) =
            if let Some(idx) = search_spec.rfind(|c| c == '\\' || c == '/' || c == ':') {
                let (dir, pat) = search_spec.split_at(idx + 1);
                // If the separator was ':', keep it in the dir part?
                // e.g. "C:file" -> "C:", "file".
                // e.g. "C:\file" -> "C:\", "file"
                // Yes, split_at includes separator in the first part (index is exclusive? No)
                // split_at(idx+1): first part [0..idx+1], second [idx+1..]
                // So "C:\foo" (idx=2 for \), split_at(3) -> "C:\", "foo". Correct.
                (dir, pat)
            } else {
                ("", search_spec)
            };

        // If parent_dir is empty, it implies current directory
        let search_dir_str = if parent_dir.is_empty() {
            "."
        } else {
            parent_dir
        };

        let host_dir = self.resolve_path(search_dir_str).ok_or(0x03)?;

        let read_dir = fs::read_dir(&host_dir).map_err(|_| 0x03)?;
        let mut all_entries: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();
        all_entries.sort_by_key(|dir_entry| dir_entry.file_name());

        let mut generated_names: HashMap<String, usize> = HashMap::new();
        let mut valid_entries: Vec<DosDirEntry> = Vec::new();

        // Always add "." and ".." if we are not in root
        // Note: fs::read_dir does NOT return . and ..
        // We must synthesize them if we are in a subdir.
        // let is_root = self.current_dir.is_empty() && (parent_dir.is_empty() || parent_dir == "\\" || parent_dir == "C:\\");
        // Actually, detecting if host_dir is root is safer
        let is_host_root = host_dir == self.root_path;

        if !is_host_root {
            // ..
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
            // .
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

        if search_index < valid_entries.len() {
            Ok(valid_entries.remove(search_index))
        } else {
            Err(0x12)
        }
    }
}
