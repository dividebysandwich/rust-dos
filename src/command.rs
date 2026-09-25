use crate::cpu::Cpu;
use crate::disk::{DRIVE_C, DriveKind, drive_letter, parse_drive_prefix};
use crate::mount::{
    IMGMOUNT_USAGE, MOUNT_USAGE, MountCmd, MountSpec, display_host_path, parse_imgmount_command, parse_mount_command,
};
use crate::video::{print_cp437, print_string};

pub trait ShellCommand {
    /// `args` contains everything after the command name (e.g., "FILE.TXT" for "TYPE FILE.TXT")
    fn execute(&self, cpu: &mut Cpu, args: &str);
}

/// The built-in commands, by name.
static COMMANDS: &[(&str, &(dyn ShellCommand + Sync))] = &[
    ("DIR", &DirCommand),
    ("LS", &LsCommand),
    ("VER", &VerCommand),
    ("VERSION", &VerCommand),
    ("TYPE", &TypeCommand),
    ("CLS", &ClsCommand),
    ("EXIT", &ExitCommand),
    ("CD", &CdCommand),
    ("CHDIR", &CdCommand),
    ("ECHO", &EchoCommand),
    ("REM", &RemCommand),
    ("GOTO", &GotoCommand),
    ("CALL", &CallCommand),
    ("SHIFT", &ShiftCommand),
    ("IF", &IfCommand),
    ("FOR", &ForCommand),
    ("PAUSE", &PauseCommand),
    ("CHOICE", &ChoiceCommand),
    ("PROMPT", &PromptCommand),
    ("COPY", &crate::file_commands::CopyCommand),
    ("DEL", &crate::file_commands::DelCommand),
    ("ERASE", &crate::file_commands::DelCommand),
    ("REN", &crate::file_commands::RenCommand),
    ("RENAME", &crate::file_commands::RenCommand),
    ("MD", &crate::file_commands::MdCommand),
    ("MKDIR", &crate::file_commands::MdCommand),
    ("RD", &crate::file_commands::RdCommand),
    ("RMDIR", &crate::file_commands::RdCommand),
    ("VOL", &crate::file_commands::VolCommand),
    ("KEYB", &KeybCommand),
    ("DATE", &crate::time_commands::DateCommand),
    ("TIME", &crate::time_commands::TimeCommand),
    ("MOUNT", &MountCommand),
    ("IMGMOUNT", &ImgMountCommand),
    ("SET", &SetCommand),
    ("PATH", &PathCommand),
    ("DOSCONFIG", &DosConfigCommand),
    ("LOADHIGH", &LoadHighCommand),
    ("LH", &LoadHighCommand),
    ("MIXER", &crate::mixer_command::MixerCommand),
];

/// The built-in command called `name`, in any case.
fn builtin(name: &str) -> Option<&'static (dyn ShellCommand + Sync)> {
    COMMANDS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|&(_, command)| command)
}

/// The command name at the start of a command line, and the rest of the
/// line after it, as COMMAND.COM splits them. The name ends at a space or
/// tab, or at one of `/ = , ;` (DIR/W, PATH=C:\DOS), which the rest begins
/// with. A built-in's name also ends at `.`, `\` or `:`, so CD.., CD\ and
/// ECHO. work, while GAME.EXE and C:\GAME stay a program's name.
pub fn split_command(line: &str) -> (&str, &str) {
    let line = line.trim_start_matches([' ', '\t', ',', ';', '=']);
    for (i, c) in line.char_indices() {
        match c {
            ' ' | '\t' | '/' | '=' | ',' | ';' => return (&line[..i], &line[i..]),
            '.' | '\\' | ':' if builtin(&line[..i]).is_some() => return (&line[..i], &line[i..]),
            _ => {}
        }
    }
    (line, "")
}

/// Runs the built-in commands.
#[derive(Default)]
pub struct CommandDispatcher;

impl CommandDispatcher {
    pub fn new() -> Self {
        Self
    }

    /// Run `command` if it is a built-in, with `args` as `split_command`
    /// left them. Returns true if the command was found and executed,
    /// false otherwise.
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
        let Some(cmd) = builtin(command) else {
            return false;
        };
        // ECHO prints its text as it is, spaces and all, which batch files
        // indent their menus with.
        let raw = command.eq_ignore_ascii_case("ECHO") || command.eq_ignore_ascii_case("REM");
        cmd.execute(cpu, if raw { args } else { args.trim() });
        true
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
        let Some((drive, spec)) = search_spec(cpu, arg) else {
            print_string(cpu, "Invalid drive specification\r\n");
            return;
        };
        let letter = drive_letter(drive);

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

/// The drive and search spec that DIR and LS list for a path: everything
/// in the directory it names (the current one if it names none), or the
/// files matching it. None if its drive isn't mounted.
fn search_spec(cpu: &Cpu, arg: &str) -> Option<(u8, String)> {
    let (drive_spec, rest) = parse_drive_prefix(arg);
    let drive = drive_spec.unwrap_or(cpu.bus.disk.get_current_drive());
    if !cpu.bus.disk.is_mounted(drive) {
        return None;
    }
    let letter = drive_letter(drive);
    let spec = if rest.is_empty() {
        format!("{}:*.*", letter)
    } else if rest.ends_with('\\') || cpu.bus.disk.is_directory(arg) {
        format!("{}:{}\\*.*", letter, rest.trim_end_matches('\\'))
    } else {
        format!("{}:{}", letter, rest)
    };
    Some((drive, spec))
}

const LS_USAGE: &str = "Lists the files and directories in wide format.\r\n\
\r\n\
LS [/A] [pattern|path ...]\r\n\
\r\n\
  pattern  file names, with the wildcards * and ?\r\n\
  path     a directory to list the contents of\r\n\
  /A       also list hidden and system files\r\n\
\r\n\
Directories are shown in blue, programs and batch files (*.COM, *.EXE,\r\n\
*.BAT) in green.\r\n";

/// LS [/A] [pattern|path ...]: the names in a directory in as many
/// columns as fit, as DOSBox Staging's LS lists them. Directories come
/// first, in blue capitals; the files follow in lower case, programs and
/// batch files in green.
struct LsCommand;
impl ShellCommand for LsCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let mut all = false;
        let mut patterns = Vec::new();
        for arg in args.split_whitespace() {
            match arg.to_ascii_lowercase().as_str() {
                "/?" | "-?" | "-h" | "--help" => {
                    print_string(cpu, LS_USAGE);
                    return;
                }
                "/a" | "-a" => all = true,
                _ if arg.starts_with('/') => {
                    print_string(cpu, &format!("Invalid switch - {}\r\n", arg));
                    return;
                }
                _ => patterns.push(arg),
            }
        }
        // Only the last part of a path can have wildcards.
        for pattern in &patterns {
            let wildcard = pattern.find(['*', '?']);
            if wildcard.is_some_and(|w| pattern.rfind(['\\', '/']).is_some_and(|sep| sep > w)) {
                print_string(cpu, &format!("Unhandled wildcard pattern - {}\r\n", pattern));
                return;
            }
        }
        if patterns.is_empty() {
            patterns.push("");
        }

        let search_attr = if all { 0x16 } else { 0x10 };
        let mut entries = Vec::new();
        for pattern in patterns {
            let Some((_, mut spec)) = search_spec(cpu, pattern) else {
                continue;
            };
            // "C*" is "C*.*", as in DOSBox.
            if !spec.rsplit(['\\', ':']).next().is_some_and(|name| name.contains('.')) {
                spec.push_str(".*");
            }
            if let Ok(found) = cpu.bus.disk.list_directory(&spec, search_attr) {
                entries.extend(found.into_iter().filter(|e| e.filename != "." && e.filename != ".."));
            }
        }
        if entries.is_empty() {
            print_string(cpu, "No files or subdirectories to display\r\n");
            return;
        }
        entries.sort_by_key(|e| (!e.is_dir, e.filename.to_ascii_uppercase()));
        // Patterns that overlap find some names twice.
        entries.dedup_by(|a, b| a.is_dir == b.is_dir && a.filename.eq_ignore_ascii_case(&b.filename));

        let widths: Vec<usize> = entries.iter().map(|e| e.filename.len() + LS_SEPARATION).collect();
        let columns = ls_columns(&widths, LS_SCREEN_COLS);
        for (i, entry) in entries.iter().enumerate() {
            let (name, attr) = if entry.is_dir {
                (entry.filename.to_ascii_uppercase(), 0x09)
            } else {
                let name = entry.filename.to_ascii_lowercase();
                let program = [".com", ".exe", ".bat"].iter().any(|ext| name.ends_with(ext));
                (name, if program { 0x0A } else { 0x07 })
            };
            print_cp437(cpu, name.as_bytes(), attr);
            let column = i % columns.len();
            if column + 1 == columns.len() || i + 1 == entries.len() {
                print_string(cpu, "\r\n");
            } else {
                print_string(cpu, &" ".repeat(columns[column] - name.len()));
            }
        }
    }
}

/// Spaces at least between LS's columns.
const LS_SEPARATION: usize = 2;
/// The shell's text mode is 80 columns wide.
const LS_SCREEN_COLS: usize = 80;

/// The widths of the columns LS fills row by row with names `widths` wide
/// (separation included): as many as fit in a line shorter than the
/// screen, so the line never wraps.
fn ls_columns(widths: &[usize], screen_cols: usize) -> Vec<usize> {
    let most = ((screen_cols - 1) / (LS_SEPARATION + 1)).min(widths.len());
    for count in (2..=most).rev() {
        let mut columns = vec![0; count];
        for (i, &width) in widths.iter().enumerate() {
            columns[i % count] = columns[i % count].max(width);
        }
        if columns.iter().sum::<usize>() < screen_cols {
            return columns;
        }
    }
    vec![widths.iter().copied().max().unwrap_or(0)]
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
                    print_cp437(cpu, &type_text(&bytes), 0x07);
                    if cpu.bus.cursor_x != 0 {
                        print_string(cpu, "\r\n");
                    }
                }
                Err(_) => print_string(cpu, "Error reading file\r\n"),
            },
            None => print_string(cpu, "File not found\r\n"),
        }
    }
}

/// A text file as TYPE shows it: up to its end of file mark (^Z), with its
/// lines ending in CR LF even where the file has LF alone.
fn type_text(bytes: &[u8]) -> Vec<u8> {
    let end = bytes.iter().position(|&b| b == 0x1A).unwrap_or(bytes.len());
    let mut text = Vec::with_capacity(end);
    for (i, &b) in bytes[..end].iter().enumerate() {
        if b == b'\n' && (i == 0 || bytes[i - 1] != b'\r') {
            text.push(b'\r');
        }
        text.push(b);
    }
    text
}

struct ClsCommand;
impl ShellCommand for ClsCommand {
    fn execute(&self, cpu: &mut Cpu, _args: &str) {
        // Direct VRAM clear to avoid circular dependency on int10.rs:
        // Space (0x20) with Gray-on-Black (0x07) on every cell of the text
        // screen, wherever the adapter keeps it (B8000h, B0000h).
        let (base, _, _) = cpu.bus.vga.text_window();
        for i in (0..cpu.bus.text_rows() * 160).step_by(2) {
            cpu.bus.write_8(base + i, 0x20);
            cpu.bus.write_8(base + i + 1, 0x07);
        }
        // Reset Cursor (BDA 0x0450)
        cpu.bus.write_16(0x0450, 0x0000);
    }
}

/// EXIT: back from a secondary COMMAND.COM to the program that started it,
/// or, at the top-level prompt, quit rust-dos.
struct ExitCommand;
impl ShellCommand for ExitCommand {
    fn execute(&self, cpu: &mut Cpu, _args: &str) {
        if let Some(dispatch) = cpu.secondary.as_mut() {
            dispatch.exit = true;
            return;
        }
        cpu.bus
            .log_string("[SHELL] Exiting Emulator via command...");
        cpu.bus.flush_log();
        cpu.bus.exit_requested = true;
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

/// DOSCONFIG: open the settings window, as Ctrl+F12 does.
struct DosConfigCommand;
impl ShellCommand for DosConfigCommand {
    fn execute(&self, cpu: &mut Cpu, _args: &str) {
        cpu.bus.config_ui_requested = true;
    }
}

/// LOADHIGH (LH): run a program in upper memory, where a TSR stays out of
/// conventional memory. DOS's /L and /S switches are taken and ignored.
struct LoadHighCommand;
impl ShellCommand for LoadHighCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let mut rest = args.trim();
        while rest.starts_with('/') {
            rest = rest.split_once(char::is_whitespace).map_or("", |(_, r)| r.trim_start());
        }
        let (program, program_args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        if program.is_empty() {
            print_string(cpu, "Required parameter missing\r\n");
            return;
        }
        if cpu.bus.umb.is_none() {
            print_string(cpu, "No upper memory (umb=false): loading the program low\r\n");
        }
        if !crate::exec::run_program(cpu, program, program_args.trim(), true) {
            print_string(cpu, "Bad command or file name.\r\n");
        }
    }
}

/// ECHO [ON|OFF|text]: turn the echo of batch lines on or off, show
/// whether it is, or print the text. ECHO. (or ECHO: and the like) prints
/// the text after the dot, an empty line if there is none.
struct EchoCommand;
impl ShellCommand for EchoCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        // The character after ECHO separates it from the text: a space, or
        // a dot and the like, with which ECHO. prints an empty line.
        if let Some(text) = args.strip_prefix(['.', ':', ',', ';', '=', '\\', '[', ']', '+', '(']) {
            print_string(cpu, &format!("{}\r\n", text));
            return;
        }
        let text = args.strip_prefix([' ', '\t']).unwrap_or(args);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            let state = if cpu.batch.echo { "on" } else { "off" };
            print_string(cpu, &format!("ECHO is {}\r\n", state));
            return;
        }
        match trimmed.to_ascii_uppercase().as_str() {
            "ON" => cpu.batch.echo = true,
            "OFF" => cpu.batch.echo = false,
            _ => print_string(cpu, &format!("{}\r\n", text)),
        }
    }
}

/// REM: a comment, which does nothing.
struct RemCommand;
impl ShellCommand for RemCommand {
    fn execute(&self, _cpu: &mut Cpu, _args: &str) {}
}

/// GOTO label: go on after the line `:label` in the batch file running,
/// or end it if it has none. At the prompt it does nothing.
struct GotoCommand;
impl ShellCommand for GotoCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        if !cpu.batch.running() {
            return;
        }
        let label = args.split_whitespace().next().unwrap_or("");
        if !cpu.batch.goto(label) {
            print_string(cpu, "Label not found\r\n");
            cpu.batch.end_file();
        }
    }
}

/// CALL command: run a batch file and come back to the one running, as
/// running it from a batch line without CALL never does.
struct CallCommand;
impl ShellCommand for CallCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        crate::exec::call(cpu, args);
    }
}

/// SHIFT: move the batch file's parameters down by one, %1 to %0 and so on.
struct ShiftCommand;
impl ShellCommand for ShiftCommand {
    fn execute(&self, cpu: &mut Cpu, _args: &str) {
        cpu.batch.shift();
    }
}

/// IF [NOT] ERRORLEVEL n command, IF [NOT] EXIST file command, IF [NOT]
/// string1==string2 command: run the command if the condition holds (or,
/// with NOT, doesn't). ERRORLEVEL n holds for an exit code of n or more,
/// and strings compare in their case.
struct IfCommand;
impl ShellCommand for IfCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let (not, rest) = match keyword(args, "NOT") {
            Some(rest) => (true, rest),
            None => (false, args),
        };
        let condition = if let Some(rest) = keyword(rest, "ERRORLEVEL") {
            let (number, command) = first_word(rest);
            number.parse::<u16>().ok().map(|n| (cpu.errorlevel as u16 >= n, command))
        } else if let Some(rest) = keyword(rest, "EXIST") {
            let (name, command) = first_word(rest);
            Some((file_exists(cpu, name), command))
        } else if let Some((left, right)) = rest.split_once("==") {
            let (right, command) = first_word(right);
            Some((left.trim() == right, command))
        } else {
            None
        };
        match condition {
            Some((holds, command)) if !command.trim().is_empty() => {
                if holds != not {
                    crate::exec::run_command_line(cpu, command);
                }
            }
            _ => print_string(cpu, "Syntax error\r\n"),
        }
    }
}

/// The rest of `text` after the word `word` (in any case) and the
/// whitespace after it, if it begins with them.
fn keyword<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let text = text.trim_start();
    let rest = text.get(word.len()..)?;
    (text[..word.len()].eq_ignore_ascii_case(word) && rest.starts_with([' ', '\t'])).then(|| rest.trim_start())
}

/// The first word of `text` and the rest after it.
fn first_word(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    text.split_once([' ', '\t']).unwrap_or((text, ""))
}

/// Whether IF EXIST finds `name`: a file, or one matching its wildcards.
/// `dir\NUL` exists when the directory does, which batch files test
/// directories with.
fn file_exists(cpu: &Cpu, name: &str) -> bool {
    let name = name.trim_matches('"');
    let upper = name.to_ascii_uppercase();
    if upper == "NUL" || upper.ends_with("\\NUL") || upper.ends_with(":NUL") {
        let dir = &name[..name.len() - 3];
        return match dir.trim_end_matches('\\') {
            "" => true,
            d if d.ends_with(':') => parse_drive_prefix(d).0.is_some_and(|drive| cpu.bus.disk.is_mounted(drive)),
            d => cpu.bus.disk.is_directory(d),
        };
    }
    if name.contains(['*', '?']) {
        cpu.bus.disk.matching_files(name).is_ok_and(|files| !files.is_empty())
    } else {
        cpu.bus.disk.is_file(name)
    }
}

/// PAUSE: wait for a key. Ctrl+C ends the batch files.
struct PauseCommand;
impl ShellCommand for PauseCommand {
    fn execute(&self, cpu: &mut Cpu, _args: &str) {
        print_string(cpu, "Press any key to continue . . .");
        crate::shell::enter_wait(cpu, crate::shell::ShellWait::Pause);
    }
}

/// CHOICE [/C[:]keys] [/N] [/S] [/T[:]c,nn] [text]: show the text and the
/// keys (Y and N without /C), as in "Continue [Y,N]?", and wait for one
/// of them; the ERRORLEVEL is its position among them, from 1. /N leaves
/// the keys out, /S tells upper from lower case, and /T takes the key c
/// once nn seconds have passed without one.
struct ChoiceCommand;
impl ShellCommand for ChoiceCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        match parse_choice(cpu, args) {
            Ok((choice, text, show_keys)) => {
                let mut prompt = text.to_string();
                if show_keys {
                    let keys: Vec<String> = choice.keys.iter().map(|&k| (k as char).to_string()).collect();
                    prompt.push_str(&format!("[{}]?", keys.join(",")));
                }
                print_string(cpu, &prompt);
                crate::shell::enter_wait(cpu, crate::shell::ShellWait::Choice(choice));
            }
            Err(e) => {
                print_string(cpu, &format!("{}\r\n", e));
                cpu.errorlevel = 255;
            }
        }
    }
}

/// CHOICE's switches: the keys to wait for, its text, and whether to show
/// the keys.
fn parse_choice<'a>(cpu: &Cpu, args: &'a str) -> Result<(crate::shell::Choice, &'a str, bool), String> {
    let mut keys = b"YN".to_vec();
    let (mut case_sensitive, mut show_keys, mut timeout) = (false, true, None);
    let mut rest = args.trim_start();
    while let Some(switch) = rest.strip_prefix('/') {
        let (word, after) = first_word(switch);
        rest = after.trim_start();
        let (letter, value) = word.split_at(word.chars().next().map_or(0, char::len_utf8));
        let value = value.strip_prefix(':').unwrap_or(value);
        match letter.to_ascii_uppercase().as_str() {
            "C" => keys = crate::dosstr::to_bytes(value),
            "N" => show_keys = false,
            "S" => case_sensitive = true,
            "T" => {
                let (key, seconds) = value.split_once(',').ok_or("Invalid switch - /T")?;
                let (key, seconds) = (key.bytes().next(), seconds.trim().parse::<u64>().ok());
                let (Some(key), Some(seconds)) = (key, seconds.filter(|&s| s <= 99)) else {
                    return Err("Invalid switch - /T".into());
                };
                timeout = Some((key, seconds));
            }
            _ => return Err(format!("Invalid switch - /{}", word)),
        }
    }
    if !case_sensitive {
        keys.make_ascii_uppercase();
    }
    if keys.is_empty() {
        return Err("Invalid switch - /C".into());
    }
    let mut choice = crate::shell::Choice { keys, case_sensitive, timeout: None };
    if let Some((key, seconds)) = timeout {
        let Some(i) = choice.keys.iter().position(|&k| if case_sensitive { k == key } else { k.eq_ignore_ascii_case(&key) }) else {
            return Err("Timeout default not in specified (or default) choices.".into());
        };
        let at = cpu.bus.clock.now_ticks() + seconds * crate::timer::PIT_HZ;
        choice.timeout = Some((choice.keys[i], at));
    }
    Ok((choice, rest, show_keys))
}

/// KEYB [code[,codepage]]: type in another keyboard layout (keylayout.rs),
/// or show which. The code page is taken and ignored: the text is code page
/// 437. An unknown code sets ERRORLEVEL 1.
struct KeybCommand;
impl ShellCommand for KeybCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let code = args.split([',', ' ', '\t']).next().unwrap_or("").trim();
        if code.is_empty() {
            let layout = cpu.bus.kbd.layout;
            print_string(cpu, &format!("Current keyboard code: {} ({})\r\n", layout.code.to_ascii_uppercase(), layout.name));
            return;
        }
        match crate::keylayout::Layout::by_code(code) {
            Some(layout) => {
                cpu.bus.kbd.layout = layout;
                cpu.errorlevel = 0;
            }
            None => {
                print_string(cpu, "Invalid keyboard code specified\r\n");
                cpu.errorlevel = 1;
            }
        }
    }
}

/// PROMPT [text]: set the prompt, with $ codes (see `shell::render_prompt`);
/// without text, back to $P$G.
struct PromptCommand;
impl ShellCommand for PromptCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        cpu.set_env("PROMPT", args);
    }
}

/// FOR %v IN (set) DO command (%%v in a batch file): run the command once
/// for every member of the set, with the member in place of %v. Members
/// with wildcards stand for the files they match.
struct ForCommand;
impl ShellCommand for ForCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        match for_lines(cpu, args) {
            Some(lines) => cpu.batch.push_for(lines),
            None => print_string(cpu, "Syntax error\r\n"),
        }
    }
}

/// The command lines a FOR line runs, None if it isn't one.
fn for_lines(cpu: &Cpu, args: &str) -> Option<Vec<String>> {
    let rest = args.trim_start().strip_prefix('%')?;
    let var = rest.chars().next().filter(|c| !c.is_whitespace() && *c != '%')?;
    let variable = format!("%{}", var);
    let rest = keyword(&rest[var.len_utf8()..], "IN")?;
    let rest = rest.strip_prefix('(')?;
    let (set, rest) = rest.split_once(')')?;
    let command = keyword(rest, "DO")?;
    if command.trim().is_empty() {
        return None;
    }
    let mut members = Vec::new();
    for member in set.split([' ', '\t', ',', ';']).filter(|m| !m.is_empty()) {
        if member.contains(['*', '?']) {
            // The files it matches, in the directory it names.
            let dir = member.rfind(['\\', ':']).map_or("", |i| &member[..=i]);
            if let Ok(files) = cpu.bus.disk.matching_files(member) {
                members.extend(files.into_iter().map(|(_, e)| format!("{}{}", dir, e.filename)));
            }
        } else {
            members.push(member.to_string());
        }
    }
    Some(members.iter().map(|m| command.replace(&variable, m)).collect())
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
/// MOUNT d path [type] [options]  mount a host directory or a disk or CD image
/// MOUNT -u d                     unmount
struct MountCommand;
impl ShellCommand for MountCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let cwd = std::env::current_dir().unwrap_or_default();
        let home = dirs::home_dir();
        match parse_mount_command(args, &cwd, home.as_deref()) {
            Ok(MountCmd::List) => {
                print_string(cpu, "Drive Type    Label       Host path\r\n");
                for info in cpu.bus.disk.mounted_drives() {
                    let host = match info.root.as_ref().or(info.image.as_ref()) {
                        Some(path) => display_host_path(path),
                        None => "(built-in)".to_string(),
                    };
                    let access = if info.read_only && info.kind != DriveKind::Virtual {
                        " (read-only)"
                    } else {
                        ""
                    };
                    let line = format!(
                        "{}:    {:<7} {:<11} {}{}{}\r\n",
                        info.letter(),
                        info.kind.name(),
                        info.label,
                        host,
                        disk_number(info.image_index, info.images.len()),
                        access
                    );
                    print_string(cpu, &line);
                }
            }
            Ok(MountCmd::Mount(spec)) => mount(cpu, spec),
            Ok(MountCmd::Unmount(drive)) => unmount(cpu, drive),
            Err(e) => {
                print_string(cpu, &format!("{}\r\n", e));
                print_string(cpu, MOUNT_USAGE);
            }
        }
    }
}

/// IMGMOUNT d image [image ...] [options]   mount disk or CD images
/// IMGMOUNT -u d                            unmount
///
/// DOSBox's command, for the batch files made for it. The image is found
/// by its DOS path first.
struct ImgMountCommand;
impl ShellCommand for ImgMountCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let cwd = std::env::current_dir().unwrap_or_default();
        let home = dirs::home_dir();
        let disk = &cpu.bus.disk;
        let locate = |path: &str| disk.resolve_path(path).filter(|p| p.is_file());
        match parse_imgmount_command(args, &locate, &cwd, home.as_deref()) {
            Ok(MountCmd::Mount(spec)) => mount(cpu, spec),
            Ok(MountCmd::Unmount(drive)) => unmount(cpu, drive),
            Ok(MountCmd::List) => {}
            Err(e) => {
                print_string(cpu, &format!("{}\r\n", e));
                print_string(cpu, IMGMOUNT_USAGE);
            }
        }
    }
}

/// " (disk 2 of 3)" for a drive mounted from a list of images.
fn disk_number(index: usize, count: usize) -> String {
    if count > 1 { format!(" (disk {} of {})", index + 1, count) } else { String::new() }
}

fn mount(cpu: &mut Cpu, spec: MountSpec) {
    // C: is always there; a disk image can take the host directory's place.
    let replace = spec.drive == DRIVE_C && spec.path.is_file();
    match cpu.bus.mount_drive(spec.drive, &spec.path, spec.opts, replace) {
        Ok(path) => {
            let kind = cpu.bus.disk.drive_kind(spec.drive).map_or("", DriveKind::name);
            let count = cpu.bus.disk.drive_info(spec.drive).map_or(0, |info| info.images.len());
            let msg = format!(
                "Drive {}: is mounted as {} {}{}\r\n",
                drive_letter(spec.drive),
                kind,
                display_host_path(&path),
                disk_number(0, count)
            );
            print_string(cpu, &msg);
        }
        Err(e) => print_string(cpu, &format!("{}\r\n", e)),
    }
}

fn unmount(cpu: &mut Cpu, drive: u8) {
    match cpu.bus.unmount_drive(drive) {
        Ok(()) => print_string(cpu, &format!("Drive {}: has been unmounted\r\n", drive_letter(drive))),
        Err(e) => print_string(cpu, &format!("{}\r\n", e)),
    }
}
