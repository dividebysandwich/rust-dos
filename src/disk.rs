use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use chrono::{DateTime, Local, Datelike, Timelike};

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
    // We assume 'C:' corresponds to the host current working directory
    current_drive: u8, 
}

impl DiskController {
    pub fn new() -> Self {
        Self {
            open_files: HashMap::new(),
            next_handle: FIRST_USER_HANDLE,
            current_drive: 2, // Default to C:
        }
    }

    // ========================================================================
    // FILE I/O OPERATIONS (Existing)
    // ========================================================================

    // INT 21h, AH=3Dh: Open File
    pub fn open_file(&mut self, filename: &str, mode: u8) -> Result<u16, u8> {
        let path = Path::new(filename);
        let mut options = OpenOptions::new();
        
        // DOS Mode (AL & 0x03): 0=Read, 1=Write, 2=Read/Write
        match mode & 0x03 {
            0 => { options.read(true); },
            1 => { options.write(true); },
            2 => { options.read(true).write(true); },
            _ => return Err(0x0C), // Error 0C: Invalid Access Code
        }

        match options.open(path) {
            Ok(f) => {
                let handle = self.next_handle;
                self.next_handle += 1;
                self.open_files.insert(handle, f);
                println!("[DISK] Opened '{}' as Handle {}", filename, handle);
                Ok(handle)
            }
            Err(_) => Err(0x02), // Error 02: File not found
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
                },
                Err(_) => Err(0x05), // Access Denied
            }
        } else {
            Err(0x06) // Invalid Handle
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
                Err(_) => Err(0x19), // Seek Error
            }
        } else {
            Err(0x06)
        }
    }

    // ========================================================================
    // FILESYSTEM METADATA & SEARCH (New Helpers)
    // ========================================================================

    // INT 21h, AH=19h: Get Current Default Drive
    #[allow(dead_code)]
    pub fn get_current_drive(&self) -> u8 {
        self.current_drive
    }

    // INT 21h, AH=36h: Get Disk Free Space
    // Input DL: 0=Default, 1=A, 2=B, 3=C, ...
    pub fn get_disk_free_space(&self, drive: u8) -> Result<(u16, u16, u16, u16), u16> {
        // DOS AH=36h uses 1-based indexing for explicit drives (1=A, 2=B, 3=C)
        // 0 means "Default Drive".
        // We simulate only Drive C, so we accept:
        //  0 (Default)
        //  3 (Explicit C:)
        //  2 (Sometimes used as 0-indexed C by internal APIs, safe to keep)
        
        if drive == 0 || drive == 3 || drive == 2 {
             // Fake 80MB drive safe for old apps
             // (8 sectors/cluster * 512 bytes * 20000 clusters = ~80 MB)
            Ok((8, 20000, 512, 20000)) 
        } else {
            Err(0x0F) // Invalid Drive
        }
    }

    // INT 21h, AH=43h: Get File Attributes
    // Returns: Attribute Byte (0x20 = Archive, 0x10 = Subdir, etc.)
    #[allow(dead_code)]
    pub fn get_file_attribute(&self, filename: &str) -> Result<u16, u8> {
        let path = Path::new(filename);
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

    // INT 21h, AH=47h: Get Current Directory
    // Returns the path string relative to root, e.g., "GAMES\DOOM"
    // For now, we assume root (""), so we return an empty string.
    fn to_short_name(filename: &str) -> (String, String) {
        let filename = filename.to_uppercase();
        
        // Split into Stem and Extension
        // If file starts with "." (e.g. .gitignore), treat whole thing as extension or invalid
        // Standard logic: rsplit_once finds the LAST dot.
        let (stem, ext) = match filename.rsplit_once('.') {
            Some((s, e)) => (s, e),
            None => (filename.as_str(), ""),
        };

        // Filter invalid chars from Stem (Keep A-Z, 0-9, _, -, etc)
        // DOS invalid chars: . " / \ [ ] : | < > + = ; , space
        let mut clean_stem: String = stem.chars()
            .filter(|c| c.is_ascii_alphanumeric() || "!@#$%^&()-_'{}`~".contains(*c))
            .collect();
        
        // Filter invalid chars from Ext
        let mut clean_ext: String = ext.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();

        // 1. Truncate Extension to 3 chars immediately
        if clean_ext.len() > 3 { clean_ext.truncate(3); }

        // 2. Truncate Stem to 8 chars (Base assumption, will shrink if collision)
        if clean_stem.len() > 8 { clean_stem.truncate(8); }

        // Edge case: If stem is empty (e.g. ".gitignore"), mapping is tricky.
        // Usually becomes "GITIGNOR" with empty ext, or "NONAME".
        if clean_stem.is_empty() {
            clean_stem = "NONAME".to_string();
        }

        (clean_stem, clean_ext)
    }

    /// Helper: Simple DOS wildcard matching (? and *)
    fn matches_pattern(filename: &str, pattern: &str) -> bool {
        // Simple implementation: * matches everything, ? matches any char
        // Real DOS parsing is complex, but for *.* this suffices.
        // We will assume the pattern is normalized to 8.3 (no dots) by the caller
        // OR we handle the dot separation. 
        
        // For simplicity in this step, let's implement a basic glob check.
        // If pattern is "*.*", return true.
        if pattern == "*.*" { return true; }
        
        // If pattern is complex, we need regex-like matching.
        // Let's do a basic check:
        // 1. Split filename and pattern by '.'
        let (f_name, f_ext) = filename.split_once('.').unwrap_or((filename, ""));
        let (p_name, p_ext) = pattern.split_once('.').unwrap_or((pattern, ""));

        let match_part = |f: &str, p: &str| -> bool {
            if p == "*" { return true; }
            let mut f_chars = f.chars();
            let mut p_chars = p.chars();
            loop {
                match (f_chars.next(), p_chars.next()) {
                    (None, None) => return true,
                    (Some(_), None) => return false, // Filename longer than pattern
                    (None, Some(pc)) => return pc == '*', // Pattern longer, ok if *
                    (Some(fc), Some(pc)) => {
                        if pc == '*' { return true; }
                        if pc != '?' && pc.to_ascii_uppercase() != fc.to_ascii_uppercase() { return false; }
                    }
                }
            }
        };

        match_part(f_name, p_name) && match_part(f_ext, p_ext)
    }

    // INT 21h, AH=4E/4F: Find First / Find Next
    // Added 'search_attr' parameter to handle Volume Labels
    pub fn find_directory_entry(&self, search_pattern: &str, search_index: usize, search_attr: u16) -> Result<DosDirEntry, u8> {
        
        // Handle Volume Label
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

        // Build Virtual File List (Stateless -> Stateful)
        let paths = fs::read_dir(".").map_err(|_| 0x03)?;
        
        // Key: "STEM.EXT", Value: Collision Count
        let mut generated_names: HashMap<String, usize> = HashMap::new();
        let mut valid_entries: Vec<DosDirEntry> = Vec::new();

        for entry in paths {
            if let Ok(entry) = entry {
                let original_name = entry.file_name().to_string_lossy().into_owned();

                // Skip dotfiles (Hidden in Unix, not standard in DOS unless explicitly mapped)
                if original_name.starts_with('.') { continue; }

                let metadata = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };

                let is_dir = metadata.is_dir();
                let mut file_attr = if is_dir { 0x10 } else { 0x20 };
                if metadata.permissions().readonly() { file_attr |= 0x01; }

                // DOS Logic: If file has Hidden/System/Dir attributes, 
                // but Search Attribute DOES NOT, skip the file.
                // (Volume Labels are handled separately above)
                let restricted_bits = 0x02 | 0x04 | 0x10; // Hidden, System, Dir
                if (file_attr & restricted_bits) & !search_attr != 0 {
                    // File is "more special" than what we asked for. Skip it.
                    continue;
                }

                // 8.3 Normalization
                let (stem, ext) = Self::to_short_name(&original_name);
                
                // Construct the "Base" 8.3 name to check for collisions
                // Note: We use the full dot format for the key, even if ext is empty, just for uniqueness checks.
                // But generally key = stem is enough if ext is empty.
                let base_key = if ext.is_empty() { stem.clone() } else { format!("{}.{}", stem, ext) };
                
                let count = *generated_names.get(&base_key).unwrap_or(&0);
                
                let final_name = if count == 0 {
                    // No collision yet. Use the normalized name as-is.
                    generated_names.insert(base_key, 1);
                    
                    if ext.is_empty() {
                        stem // No dot
                    } else {
                        format!("{}.{}", stem, ext)
                    }
                } else {
                    // Collision detected! Generate ~N name.
                    generated_names.insert(base_key.clone(), count + 1);
                    
                    // Generate Tilde Suffix
                    let suffix = format!("~{}", count); // e.g., "~1" or "~12"
                    
                    // We must ensure (Stem + Suffix).len() <= 8
                    let available_len = 8usize.saturating_sub(suffix.len());
                    
                    let short_stem = if stem.len() > available_len {
                        &stem[0..available_len]
                    } else {
                        &stem
                    };
                    
                    if ext.is_empty() {
                        format!("{}{}", short_stem, suffix) // "FILE~1" (No dot)
                    } else {
                        format!("{}{}.{}", short_stem, suffix, ext) // "FILE~1.TXT"
                    }
                };

                // Only add to list if it matches the requested DOS pattern
                if !Self::matches_pattern(&final_name, search_pattern) {
                    continue;
                }
                
                // Date/Time Conversion 
                let sys_time = metadata.modified().unwrap_or(std::time::SystemTime::now());
                let datetime: DateTime<Local> = sys_time.into();

                let dos_time = ((datetime.hour() as u16) << 11)
                             | ((datetime.minute() as u16) << 5)
                             | ((datetime.second() as u16) / 2);

                let year = datetime.year();
                let dos_date = if year < 1980 { 0x0021 } else {
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

        // Return Requested Index
        if search_index < valid_entries.len() {
            let entry = valid_entries.remove(search_index);
            // println!("[DISK] Index {}: {} ({})", search_index, entry.filename, if entry.is_dir { "DIR" } else { "FILE" });
            Ok(entry)
        } else {
            Err(0x12) // No More Files
        }
    }
}