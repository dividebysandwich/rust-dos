use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::bus;

// DOS defines standard handles: 0=Stdin, 1=Stdout, 2=Stderr, 3=Aux, 4=Printer
pub const FIRST_USER_HANDLE: u16 = 5;

/// Helper struct to transfer directory search results back to the CPU
pub struct DosDirEntry {
    pub filename: String,
    pub size: u32,
    pub is_dir: bool,
    pub is_readonly: bool,
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
    pub fn get_current_directory(&self, _drive: u8) -> Result<String, u8> {
        // In a real emulator, map this to an internal "CWD" state variable.
        // For 'd.com', returning empty string (root) is sufficient.
        Ok(String::new()) 
    }

    // INT 21h, AH=4E/4F: Find First / Find Next
    // Added 'search_attr' parameter to handle Volume Labels
    pub fn find_directory_entry(&self, search_pattern: &str, search_index: usize, search_attr: u16) -> Result<DosDirEntry, u8> {
        
        // CHECK FOR VOLUME LABEL REQUEST (Bit 3)
        // DOS behavior: If Bit 3 is set, strictly look for volume label.
        if (search_attr & 0x08) != 0 {
            if search_index == 0 {
                return Ok(DosDirEntry {
                    filename: "RUSTDOS".to_string(), // Fake Volume Label
                    size: 0,
                    is_dir: false,
                    is_readonly: false,
                    // We must ensure the attribute has 0x08 set
                    // We return a special flag or handle it in interrupt.rs
                    // Here we just identify it.
                });
            } else {
                return Err(0x12); // No more files (only 1 label exists)
            }
        }

        println!("[DISK] Searching for Index {} in '.' (Pattern: {})", search_index, search_pattern);

        // NORMAL SEARCH (Host Filesystem)
        let paths = fs::read_dir(".").map_err(|_| 0x03)?; 
        
        let mut found_count = 0;

        for entry in paths {
            if let Ok(entry) = entry {
                let filename = entry.file_name().to_string_lossy().into_owned();

                println!("[DISK] Scanned: '{}' ... ", filename);

                // Skip hidden/dotfiles
                if filename.starts_with('.') {
                    println!("Skipped (Dotfile)");
                    continue; 
                }

                if found_count == search_index {
                    println!("MATCH! Returning this file.");
                    let metadata = entry.metadata().map_err(|_| 0x05)?;
                    
                    return Ok(DosDirEntry {
                        filename: filename.to_uppercase(),
                        size: metadata.len() as u32,
                        is_dir: metadata.is_dir(),
                        is_readonly: metadata.permissions().readonly(),
                    });
                }
                println!("Ignored (Index {} != {})", found_count, search_index);
                found_count += 1;
            }
        }

        println!("[DISK] End of Directory. Total files found: {}", found_count);
        Err(0x12) // No More Files
    }
}