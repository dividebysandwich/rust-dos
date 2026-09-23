use crate::cpu::Cpu;
use crate::disk::{DriveKind, drive_letter, parse_drive_prefix};
use crate::mount::{MOUNT_USAGE, MountCmd, display_host_path, parse_mount_command};
use crate::video::print_string;
use std::collections::HashMap;

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
        dispatcher.register("CD", Box::new(CdCommand));
        dispatcher.register("CHDIR", Box::new(CdCommand));
        dispatcher.register("ECHO", Box::new(EchoCommand));
        dispatcher.register("MOUNT", Box::new(MountCommand));
        dispatcher.register("SET", Box::new(SetCommand));
        dispatcher.register("PATH", Box::new(PathCommand));

        dispatcher
    }

    /// Registers a new command dynamically
    pub fn register(&mut self, name: &str, command: Box<dyn ShellCommand>) {
        self.registry.insert(name.to_uppercase(), command);
    }

    /// Returns true if the command was found and executed, false otherwise.
    pub fn dispatch(&self, cpu: &mut Cpu, command: &str, args: &str) -> bool {
        // "D:" switches the current drive
        if let (Some(drive), "") = parse_drive_prefix(command) {
            if cpu.bus.disk.is_mounted(drive) {
                cpu.bus.disk.set_current_drive(drive);
            } else {
                print_string(cpu, "Invalid drive specification\r\n");
            }
            return true;
        }
        // "PATH=C:\DOS" is PATH with its argument after the '='.
        if let Some((name, value)) = command.split_once('=') {
            if name.eq_ignore_ascii_case("PATH") {
                PathCommand.execute(cpu, &format!("{} {}", value, args));
                return true;
            }
        }
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
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        // DIR [d:][path][pattern]; switches like /W or /P are ignored.
        let arg = args
            .split_whitespace()
            .find(|a| !a.starts_with('/'))
            .unwrap_or("");
        let (drive_spec, rest) = parse_drive_prefix(arg);
        let drive = drive_spec.unwrap_or(cpu.bus.disk.get_current_drive());
        if !cpu.bus.disk.is_mounted(drive) {
            print_string(cpu, "Invalid drive specification\r\n");
            return;
        }

        // A bare drive or directory lists everything in it.
        let letter = drive_letter(drive);
        let is_dir = rest.is_empty()
            || rest.ends_with('\\')
            || cpu.bus.disk.is_directory(arg);
        let spec = if rest.is_empty() {
            format!("{}:*.*", letter)
        } else if is_dir {
            format!("{}:{}\\*.*", letter, rest.trim_end_matches('\\'))
        } else {
            format!("{}:{}", letter, rest)
        };

        let label = cpu.bus.disk.volume_label(drive).unwrap_or_default();
        if label.is_empty() {
            print_string(
                cpu,
                &format!(" Volume in drive {} has no label\r\n", letter),
            );
        } else {
            print_string(
                cpu,
                &format!(" Volume in drive {} is {}\r\n", letter, label),
            );
        }
        let directory = cpu.bus.disk.qualify_directory(&spec).unwrap_or_default();
        print_string(cpu, &format!(" Directory of {}\r\n\r\n", directory));

        // Directories + hidden + system
        let entries = match cpu.bus.disk.list_directory(&spec, 0x16) {
            Ok(entries) if !entries.is_empty() => entries,
            _ => {
                print_string(cpu, "File not found\r\n");
                return;
            }
        };

        let mut file_count = 0;
        let mut total_bytes = 0u64;
        for entry in &entries {
            let (stem, ext) = match entry.filename.split_once('.') {
                Some((s, e)) if !s.is_empty() => (s, e),
                _ => (entry.filename.as_str(), ""),
            };
            // MS-DOS puts <DIR> at the left of the size column
            let size_str = if entry.is_dir {
                format!("{:<14}", "<DIR>")
            } else {
                file_count += 1;
                total_bytes += entry.size as u64;
                format_size(entry.size as u64)
            };
            let timestamp = if entry.dos_date == 0 {
                String::new()
            } else {
                format_dos_timestamp(entry.dos_date, entry.dos_time)
            };
            let line = format!("{:<8} {:<3} {:>14} {}", stem, ext, size_str, timestamp);
            let line = format!("{}\r\n", line.trim_end());
            print_string(cpu, &line);
        }

        // Summary Footer
        let free_bytes = cpu
            .bus
            .disk
            .get_disk_free_space(drive + 1)
            .map(|(spc, free, bps, _)| spc as u64 * free as u64 * bps as u64)
            .unwrap_or(0);
        print_string(
            cpu,
            &format!(
                "{:>9} file(s) {:>14} bytes\r\n{:>24} bytes free\r\n",
                file_count,
                format_size(total_bytes),
                format_size(free_bytes)
            ),
        );
    }
}

/// "MM-DD-YY  HH:MMa" from packed DOS date and time words.
fn format_dos_timestamp(date: u16, time: u16) -> String {
    let (year, month, day) = (1980 + (date >> 9), (date >> 5) & 0x0F, date & 0x1F);
    let (hour, minute) = (time >> 11, (time >> 5) & 0x3F);
    let (hour12, suffix) = match hour {
        0 => (12, 'a'),
        1..=11 => (hour, 'a'),
        12 => (12, 'p'),
        _ => (hour - 12, 'p'),
    };
    format!(
        "{:02}-{:02}-{:02}  {:>2}:{:02}{}",
        month,
        day,
        year % 100,
        hour12,
        minute,
        suffix
    )
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
        let target = args.trim();
        if target.is_empty() {
            print_string(cpu, "Required parameter missing\r\n");
            return;
        }

        match cpu.bus.disk.file_data(target) {
            Some(file) => match file.read() {
                Ok(bytes) => {
                    // DOS formatting: \n -> \r\n
                    let contents = String::from_utf8_lossy(&bytes);
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
        cpu.bus
            .log_string("[SHELL] Exiting Emulator via command...");
        cpu.bus.flush_log();
        std::process::exit(0);
    }
}

struct SetCommand;
impl ShellCommand for SetCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let args = args.trim_start();
        match args.split_once('=') {
            Some((name, value)) => cpu.set_env(name.trim(), value),
            None if args.trim().is_empty() => {
                let lines: Vec<String> = cpu
                    .environment
                    .iter()
                    .map(|(name, value)| format!("{}={}\r\n", name, value))
                    .collect();
                for line in lines {
                    print_string(cpu, &line);
                }
            }
            None => print_string(cpu, "Syntax error\r\n"),
        }
    }
}

struct PathCommand;
impl ShellCommand for PathCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let path = args.trim().trim_start_matches('=').trim();
        match path {
            "" => {
                let text = match cpu.get_env("PATH") {
                    Some(p) => format!("PATH={}\r\n", p),
                    None => "No Path\r\n".to_string(),
                };
                print_string(cpu, &text);
            }
            // "PATH ;" clears the search path.
            ";" => cpu.set_env("PATH", ""),
            _ => cpu.set_env("PATH", &path.to_ascii_uppercase()),
        }
    }
}

struct EchoCommand;
impl ShellCommand for EchoCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            let state = if cpu.batch_echo { "on" } else { "off" };
            print_string(cpu, &format!("ECHO is {}\r\n", state));
            return;
        }
        match trimmed.to_ascii_uppercase().as_str() {
            "ON" => cpu.batch_echo = true,
            "OFF" => cpu.batch_echo = false,
            _ => print_string(cpu, &format!("{}\r\n", args)),
        }
    }
}

struct CdCommand;
impl ShellCommand for CdCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let path = args.trim();
        // "CD" and "CD D:" print the current directory of that drive
        let (drive_spec, rest) = parse_drive_prefix(path);
        if rest.is_empty() {
            let drive = drive_spec.unwrap_or(cpu.bus.disk.get_current_drive());
            match cpu.bus.disk.get_current_directory_of(drive) {
                Some(cwd) => print_string(cpu, &format!("{}:\\{}\r\n", drive_letter(drive), cwd)),
                None => print_string(cpu, "Invalid drive specification\r\n"),
            }
        } else if !cpu.bus.disk.set_current_directory(path) {
            print_string(cpu, "Invalid directory\r\n");
        }
    }
}

/// MOUNT                          list drives
/// MOUNT d path [type] [options]  mount a host directory
/// MOUNT -u d                     unmount
struct MountCommand;
impl ShellCommand for MountCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let cwd = std::env::current_dir().unwrap_or_default();
        let home = dirs::home_dir();
        match parse_mount_command(args, &cwd, home.as_deref()) {
            Ok(MountCmd::List) => {
                print_string(cpu, "Drive Type    Label       Host directory\r\n");
                for info in cpu.bus.disk.mounted_drives() {
                    let host = match &info.root {
                        Some(root) => display_host_path(root),
                        None => "(built-in)".to_string(),
                    };
                    let access = if info.read_only && info.kind != DriveKind::Virtual {
                        " (read-only)"
                    } else {
                        ""
                    };
                    let line = format!(
                        "{}:    {:<7} {:<11} {}{}\r\n",
                        info.letter(),
                        info.kind.name(),
                        info.label,
                        host,
                        access
                    );
                    print_string(cpu, &line);
                }
            }
            Ok(MountCmd::Mount(spec)) => {
                let kind = spec.opts.kind;
                match cpu
                    .bus
                    .mount_drive(spec.drive, &spec.path, spec.opts, false)
                {
                    Ok(root) => {
                        let msg = format!(
                            "Drive {}: is mounted as {} {}\r\n",
                            drive_letter(spec.drive),
                            kind.name(),
                            display_host_path(&root)
                        );
                        print_string(cpu, &msg);
                    }
                    Err(e) => print_string(cpu, &format!("{}\r\n", e)),
                }
            }
            Ok(MountCmd::Unmount(drive)) => match cpu.bus.unmount_drive(drive) {
                Ok(()) => print_string(
                    cpu,
                    &format!("Drive {}: has been unmounted\r\n", drive_letter(drive)),
                ),
                Err(e) => print_string(cpu, &format!("{}\r\n", e)),
            },
            Err(e) => {
                print_string(cpu, &format!("{}\r\n", e));
                print_string(cpu, MOUNT_USAGE);
            }
        }
    }
}
