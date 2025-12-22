use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write}; // Added Seek and SeekFrom
use std::path::Path;

// DOS defines standard handles: 0=Stdin, 1=Stdout, 2=Stderr, 3=Aux, 4=Printer
// We start assigning new file handles at 5.
pub const FIRST_USER_HANDLE: u16 = 5;

pub struct DiskController {
    // Map DOS Handle (u16) -> Rust File Object
    open_files: HashMap<u16, File>,
    next_handle: u16,
}

impl DiskController {
    pub fn new() -> Self {
        Self {
            open_files: HashMap::new(),
            next_handle: FIRST_USER_HANDLE,
        }
    }

    // INT 21h, AH=3Dh: Open File
    // Returns: Result<Handle, ErrorCode>
    pub fn open_file(&mut self, filename: &str, mode: u8) -> Result<u16, u8> {
        let path = Path::new(filename);

        // DOS Mode (AL & 0x03): 0=Read, 1=Write, 2=Read/Write
        // IMPORTANT: 3Dh does NOT create files. That is 3Ch.
        // We must return Error 02 (File Not Found) if it doesn't exist.
        
        let mut options = OpenOptions::new();
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
        if self.open_files.remove(&handle).is_some() {
            // println!("[DISK] Closed Handle {}", handle);
            true
        } else {
            false
        }
    }

    // INT 21h, AH=3Fh: Read from File
    // Changed signature to return Vec<u8> to simplify the CPU logic
    pub fn read_file(&mut self, handle: u16, count: usize) -> Result<Vec<u8>, u16> {
        if let Some(file) = self.open_files.get_mut(&handle) {
            let mut buffer = vec![0u8; count]; // Allocate buffer
            match file.read(&mut buffer) {
                Ok(bytes_read) => {
                    // Truncate vector to the actual number of bytes read (handle EOF)
                    buffer.truncate(bytes_read);
                    Ok(buffer)
                },
                Err(_) => Err(0x05), // Error 05: Access Denied
            }
        } else {
            Err(0x06) // Error 06: Invalid Handle
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

    // INT 21h, AH=42h: LSEEK (Move File Pointer)
    // Returns: New absolute position in file
    pub fn seek_file(&mut self, handle: u16, offset: i64, origin: u8) -> Result<u64, u16> {
        if let Some(file) = self.open_files.get_mut(&handle) {
            let seek_from = match origin {
                0 => SeekFrom::Start(offset as u64), // Offset from Beginning
                1 => SeekFrom::Current(offset),      // Offset from Current Position
                2 => SeekFrom::End(offset),          // Offset from End of File
                _ => return Err(0x01),               // Error 01: Invalid Function
            };

            match file.seek(seek_from) {
                Ok(new_pos) => Ok(new_pos),
                Err(_) => Err(0x19), // Error 19: Seek Error
            }
        } else {
            Err(0x06) // Error 06: Invalid Handle
        }
    }
}