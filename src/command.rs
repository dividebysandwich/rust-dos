use std::fs;
use chrono::{DateTime, Local};
use std::collections::HashMap;
use crate::cpu::Cpu;
use crate::video::print_string;

pub trait ShellCommand {
    /// `args` contains everything after the command name (e.g., "FILE.TXT" for "TYPE FILE.TXT")
    fn execute(&self, cpu: &mut Cpu, args: &str);
}

pub struct CommandDispatcher {
    registry: HashMap<String, Box<dyn ShellCommand>>,
}

impl CommandDispatcher {
    pub fn new() -> Self {
        let mut dispatcher = Self {
            registry: HashMap::new(),
        };

        // Register core commands
        dispatcher.register("DIR", Box::new(DirCommand));
        dispatcher.register("VER", Box::new(VerCommand));
        dispatcher.register("VERSION", Box::new(VerCommand)); // Alias
        dispatcher.register("TYPE", Box::new(TypeCommand));
        dispatcher.register("CLS", Box::new(ClsCommand));
        dispatcher.register("EXIT", Box::new(ExitCommand));

        dispatcher
    }

    /// Registers a new command dynamically
    pub fn register(&mut self, name: &str, command: Box<dyn ShellCommand>) {
        self.registry.insert(name.to_uppercase(), command);
    }

    /// Returns true if the command was found and executed, false otherwise.
    pub fn dispatch(&self, cpu: &mut Cpu, command: &str, args: &str) -> bool {
        if let Some(cmd) = self.registry.get(&command.to_uppercase()) {
            cmd.execute(cpu, args);
            true
        } else {
            false
        }
    }
}

// --- Command Implementations ---

struct DirCommand;
impl ShellCommand for DirCommand {
    fn execute(&self, cpu: &mut Cpu, _args: &str) {
        print_string(cpu, " Volume in drive C has no label.\r\n");
        print_string(cpu, " Directory of C:\\\r\n\r\n");

        let mut file_count = 0;
        let mut dir_count = 0;
        let mut total_bytes = 0;

        if let Ok(entries) = fs::read_dir(".") {
            for entry in entries.flatten() {
                let path = entry.path();
                let metadata = path.metadata().ok();
                
                // Get Date/Time
                // DOS uses local time. We convert SystemTime -> DateTime<Local>
                let timestamp: DateTime<Local> = metadata
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .map(|t| t.into())
                    .unwrap_or_else(|| Local::now());

                let date_str = timestamp.format("%m/%d/%Y  %I:%M %p").to_string();

                // Get File Size or <DIR> tag
                let size_str = if path.is_dir() {
                    dir_count += 1;
                    "<DIR>     ".to_string() // Padding for alignment
                } else {
                    file_count += 1;
                    let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
                    total_bytes += size;
                    // Format size with simple commas (optional, but looks "real")
                    format_size(size)
                };

                // Get Filename
                let name_str = path
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default();

                // Print Line: DATE  TIME  <DIR>|SIZE  NAME
                // {:<22} left-aligns the date, {:>14} right-aligns size
                let line = format!(
                    "{}  {:>14} {}\r\n", 
                    date_str, size_str, name_str
                );
                print_string(cpu, &line);
            }
        }

        // Summary Footer
        print_string(cpu, &format!(
            "{:>16} File(s) {:>14} bytes\r\n", 
            file_count, format_size(total_bytes)
        ));
        print_string(cpu, &format!(
            "{:>16} Dir(s)  {:>14} bytes free\r\n", 
            dir_count, "0" // We don't really track free space on host yet
        )); 
    }
}

/// Format u64 as string with commas (e.g. 1,024)
fn format_size(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let mut count = 0;
    for c in s.chars().rev() {
        if count > 0 && count % 3 == 0 {
            result.push(',');
        }
        result.push(c);
        count += 1;
    }
    result.chars().rev().collect()
}

struct VerCommand;
impl ShellCommand for VerCommand {
    fn execute(&self, cpu: &mut Cpu, _args: &str) {
        let version = env!("CARGO_PKG_VERSION");
        print_string(cpu, &format!("Rust-DOS v{}\r\n", version));
    }
}

struct TypeCommand;
impl ShellCommand for TypeCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        if args.trim().is_empty() {
            print_string(cpu, "Required parameter missing\r\n");
            return;
        }

        let target_lower = args.trim().to_lowercase();
        let mut found_path = None;

        // Case-insensitive search
        if let Ok(entries) = std::fs::read_dir(".") {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name() {
                    if name.to_string_lossy().to_lowercase() == target_lower {
                        found_path = Some(path);
                        break;
                    }
                }
            }
        }

        match found_path {
            Some(path) => match std::fs::read_to_string(path) {
                Ok(contents) => {
                    // DOS formatting: \n -> \r\n
                    let dos_text = contents.replace('\n', "\r\n").replace("\r\r\n", "\r\n");
                    print_string(cpu, &dos_text);
                    print_string(cpu, "\r\n");
                }
                Err(_) => print_string(cpu, "Error reading file\r\n"),
            },
            None => print_string(cpu, "File not found\r\n"),
        }
    }
}

struct ClsCommand;
impl ShellCommand for ClsCommand {
    fn execute(&self, cpu: &mut Cpu, _args: &str) {
        // Direct VRAM clear (0xB8000) to avoid circular dependency on int10.rs
        // Writes Space (0x20) with Gray-on-Black (0x07)
        for i in (0..4000).step_by(2) {
            cpu.bus.write_8(0xB8000 + i, 0x20);
            cpu.bus.write_8(0xB8000 + i + 1, 0x07);
        }
        // Reset Cursor (BDA 0x0450)
        cpu.bus.write_16(0x0450, 0x0000); 
    }
}

struct ExitCommand;
impl ShellCommand for ExitCommand {
    fn execute(&self, cpu: &mut Cpu, _args: &str) {
        cpu.bus.log_string("[SHELL] Exiting Emulator via command...");
        std::process::exit(0);
    }
}