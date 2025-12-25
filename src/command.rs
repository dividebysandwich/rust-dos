use crate::video::print_string;
use crate::cpu::Cpu;

// The "DIR" Implementation
pub fn run_dir_command(cpu: &mut Cpu) {
    print_string(cpu, " Directory of C:\\\r\n\r\n");

    // List files in current directory
    if let Ok(entries) = std::fs::read_dir(".") {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if let Some(name) = path.file_name() {
                    let name_str = name.to_string_lossy();
                    // Format nicely
                    let line = format!("{} \r\n", name_str);
                    print_string(cpu, &line);
                }
            }
        }
    }
}

pub fn run_ver_command(cpu: &mut Cpu) {
    let version = env!("CARGO_PKG_VERSION");
    print_string(cpu, format!("Rust-DOS v{}\r\n", version).as_str());
}

pub fn run_type_command(cpu: &mut Cpu, filename: &str) {
    let target_lower = filename.to_lowercase();
    let mut found_path = None;

    // Search for the file (Case-Insensitive)
    if let Ok(entries) = std::fs::read_dir(".") {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                println!("Checking file: {:?}", path);
                if let Some(name) = path.file_name() {
                    let name_str = name.to_string_lossy();
                    println!("Filename: {}", name_str);
                    if name_str.to_lowercase() == target_lower {
                        found_path = Some(path);
                        break;
                    }
                }
            }
        }
    }

    // Open the file if found
    match found_path {
        Some(path) => {
            match std::fs::read_to_string(path) {
                Ok(contents) => {
                    // Normalize Newlines (\n -> \r\n)
                    let dos_format = contents.replace("\n", "\r\n").replace("\r\r\n", "\r\n");
                    print_string(cpu, &dos_format);
                    print_string(cpu, "\r\n");
                }
                Err(_) => {
                    print_string(cpu, "Error reading file\r\n");
                }
            }
        }
        None => {
            print_string(cpu, "File not found\r\n");
        }
    }
}
