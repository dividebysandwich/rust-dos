use crate::cpu::Cpu;
use crate::disk::{DiskController, drive_letter};
use crate::dosstr;
use crate::interrupts::utils::read_asciiz_bytes;
use crate::video;

/// Private BOP vector the shell uses to hand a typed line to the emulator.
/// It is only ever executed inline from the shell code (never via the IVT),
/// so it can't collide with a real INT FFh or with the INT 2Fh multiplexer.
pub const SHELL_COMMAND_BOP: u8 = 0xFF;

/// The longest line the prompt takes: its buffer at 0200h ends where the
/// current directory's at 0300h begins.
pub const MAX_LINE: usize = 0x7F;

/// A Tiny "OS" written in Machine Code. Reads keys into a buffer at offset 0x0200
/// On Enter, hands the line to the Rust shell via the SHELL_COMMAND_BOP trap.
/// Handles backspace visually and in buffer, and hands the other control
/// keys and the extended keys (Esc, Tab, Up and Down) to `edit_key`.
#[rustfmt::skip] // keep one instruction per line
pub fn get_shell_code() -> Vec<u8> {
    vec![
        // ----------------------------------------------------
        // BOOTLOADER: Initialize Segments
        // ----------------------------------------------------
        // We are loaded at SHELL_SEGMENT:0100 (see Cpu::load_shell).
        // Set DS=ES=SS=CS so buffers and stack live in the shell segment.
        0x8C, 0xC8, // MOV AX, CS (Copy CS to AX)
        0x8E, 0xD8, // MOV DS, AX
        0x8E, 0xC0, // MOV ES, AX
        0x8E, 0xD0, // MOV SS, AX
        0xBC, 0x00, 0x0F, // MOV SP, 0x0F00 (SHELL_STACK)
        // ----------------------------------------------------
        // SHELL LOOP START
        // ----------------------------------------------------
        // Label: PROMPT_START (0x010B)
        // 1. Print the current drive and ":\"
        0xB4, 0x19, 0xCD, 0x21, // MOV AH, 19h, INT 21h (AL = drive, 0=A)
        0x04, 0x41, // ADD AL, 'A'
        0xB4, 0x0E, 0xCD, 0x10, // MOV AH, 0Eh, INT 10h
        0xB0, 0x3A, 0xCD, 0x10, // MOV AL, ':', INT 10h
        0xB0, 0x5C, 0xCD, 0x10, // MOV AL, '\', INT 10h
        // 2. Get Current Directory (INT 21h, AH=47h)
        // Returns null-terminated string at DS:SI (we use 0x0300 as buffer)
        // DL = Drive (0=Default/C)
        0xB4, 0x47, // MOV AH, 47h
        0xB2, 0x00, // MOV DL, 0
        0xBE, 0x00, 0x03, // MOV SI, 0x0300
        0xCD, 0x21, // INT 21h
        // 3. Print CWD Loop
        0xBE, 0x00, 0x03, // MOV SI, 0x0300 (Reset SI to start of buffer)
        // Label: PRINT_LOOP
        0xAC, // LODSB (AL = [SI], SI++)
        0x08, 0xC0, // OR AL, AL
        0x74, 0x06, // JZ PRINT_DONE (+6 bytes to '>')
        0xB4, 0x0E, // MOV AH, 0Eh
        0xCD, 0x10, // INT 10h
        0xEB, 0xF5, // JMP PRINT_LOOP (-11 bytes)
        // Label: PRINT_DONE
        // 4. Print ">"
        0xB4, 0x0E, // MOV AH, 0Eh (SafeGuard: ensure AH is 0E)
        0xB0, 0x3E, 0xCD, 0x10, // MOV AL, '>', INT 10h
        // Reset Buffer Pointer
        0xBE, 0x00, 0x02, // MOV SI, 0x0200 (Buffer Start)
        // ----------------------------------------------------
        // INPUT LOOP (Wait for keys)
        // ----------------------------------------------------
        // Label: WAIT_KEY (0x013D)
        0xB4, 0x00, // MOV AH, 00h
        0xCD, 0x16, // INT 16h (Wait for Key)
        // Check ENTER (0x0D)
        0x3C, 0x0D, // CMP AL, 0x0D
        0x74, 0x37, // JE EXECUTE (0x017C)
        // Check BACKSPACE (0x08)
        0x3C, 0x08, // CMP AL, 0x08
        0x74, 0x17, // JE HANDLE_BACKSPACE (0x0160)
        // Control keys (Esc, Tab, Ctrl+letters) and extended keys (AL 00h,
        // or E0h for the grey ones) aren't typed
        0x3C, 0x20, // CMP AL, 0x20
        0x72, 0x2A, // JB EDIT_KEY (0x0177)
        0x3C, 0xE0, // CMP AL, 0xE0
        0x74, 0x26, // JE EDIT_KEY (0x0177)
        // A full buffer takes no more (MAX_LINE characters)
        0x81, 0xFE, 0x7F, 0x02, // CMP SI, 0x027F
        0x73, 0xE6, // JAE WAIT_KEY
        // Normal Character
        0xB4, 0x0E, // MOV AH, 0Eh (Teletype Output)
        0xCD, 0x10, // INT 10h (Print Char)
        0x88, 0x04, // MOV [SI], AL (Store in Buffer)
        0x46, // INC SI (Advance Pointer)
        0xEB, 0xDD, // JMP WAIT_KEY
        // ----------------------------------------------------
        // BACKSPACE HANDLER (0x0160)
        // ----------------------------------------------------
        // Check Boundary (Start of Buffer)
        0x81, 0xFE, 0x00, 0x02, // CMP SI, 0x0200
        0x74, 0xD7, // JE WAIT_KEY (If empty, just wait)
        // Perform Visual Backspace
        0x4E, // DEC SI (Move Pointer Back)
        0xB4, 0x0E, // MOV AH, 0Eh
        0xB0, 0x08, 0xCD, 0x10, // Print Backspace
        0xB0, 0x20, 0xCD, 0x10, // Print Space
        0xB0, 0x08, 0xCD, 0x10, // Print Backspace
        0xEB, 0xC6, // JMP WAIT_KEY
        // ----------------------------------------------------
        // EDIT KEYS (0x0177): Esc, Tab or the history replaces the line (SI)
        // ----------------------------------------------------
        0xFE, 0x39, crate::bios::SERVICE_SHELL_KEY, // edit_key
        0xEB, 0xC1, // JMP WAIT_KEY
        // ----------------------------------------------------
        // EXECUTE COMMAND (0x017C)
        // ----------------------------------------------------
        0xC6, 0x04, 0x00, // MOV BYTE PTR [SI], 0 (Null Terminate)
        // Print Newline
        0xB4, 0x0E, 0xB0, 0x0D, 0xCD, 0x10, // CR
        0xB0, 0x0A, 0xCD, 0x10, // LF
        0xBA, 0x00, 0x02, // MOV DX, 0x0200
        // The trap returns like an IRET, so build the frame an INT would:
        // FLAGS, CS, then the address of the JMP below.
        0x9C, // PUSHF
        0x0E, // PUSH CS
        0xB8, 0x95, 0x01, // MOV AX, 0x0195
        0x50, // PUSH AX
        0xFE, 0x38, SHELL_COMMAND_BOP, // Hand the line to the Rust shell
        // ----------------------------------------------------
        // RESET LOOP
        // ----------------------------------------------------
        0xE9, 0x73, 0xFF, // 0x0195: JMP PROMPT_START (-141 bytes, a near jump)
    ]
}

/// The lines typed at the prompt, oldest first, for Up and Down.
#[derive(Clone, Debug, Default)]
pub struct ShellHistory {
    entries: Vec<String>,
    /// The entry Up and Down are at: `entries.len()` is the new line
    /// below the newest.
    pos: usize,
}

impl ShellHistory {
    /// The lines kept.
    const MAX: usize = 100;

    /// A line was entered: keep it, unless it is empty or the one before
    /// again, and start again from below the newest.
    pub fn push(&mut self, line: &str) {
        if !line.is_empty() && self.entries.last().map(String::as_str) != Some(line) {
            self.entries.push(line.to_string());
            if self.entries.len() > Self::MAX {
                self.entries.remove(0);
            }
        }
        self.pos = self.entries.len();
    }

    /// Up: the entry before, if there is one.
    pub fn older(&mut self) -> Option<&str> {
        self.pos = self.pos.checked_sub(1)?;
        self.entries.get(self.pos).map(String::as_str)
    }

    /// Down: the entry after, or the empty new line after the newest; None
    /// when already there.
    pub fn newer(&mut self) -> Option<&str> {
        if self.pos >= self.entries.len() {
            return None;
        }
        self.pos += 1;
        Some(self.entries.get(self.pos).map_or("", String::as_str))
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }
}

/// Where Tab left the line at the prompt.
#[derive(Clone, Debug)]
pub struct Completion {
    /// The line as Tab left it: Tab again on it goes on to the next name,
    /// on any other line it starts over.
    line: Vec<u8>,
    /// Where the name being completed begins in it.
    start: usize,
    /// The names that fit, and the one in the line.
    names: Vec<String>,
    index: usize,
}

/// A control or extended key at the prompt (the shell code's EDIT_KEY, AL
/// its character, 00h or E0h for an extended key, AH its scan code): Esc
/// blanks the line being typed, Tab and Shift+Tab complete the name being
/// typed in it (`complete`), and Up and Down put the line before or after
/// in the command history in its place, on the screen and in the buffer at
/// DS:0200h, and leave SI after it. The other keys do nothing.
pub fn edit_key(cpu: &mut Cpu) {
    let line = match (cpu.get_al(), cpu.get_ah()) {
        (0x09, _) => complete(cpu, true),
        (0x00, 0x0F) => complete(cpu, false),
        (al, ah) => match (al, ah) {
            (0x1B, _) => Some(""),
            (0x00 | 0xE0, 0x48) => cpu.shell_history.older(),
            (0x00 | 0xE0, 0x50) => cpu.shell_history.newer(),
            _ => None,
        }
        .map(dosstr::to_bytes),
    };
    let Some(mut line) = line else { return };
    line.truncate(MAX_LINE);
    let saved = (cpu.ax(), cpu.bx(), cpu.cx(), cpu.dx());

    // Back to where the line began, over as many cells as it had
    // characters (it may have wrapped onto the next row).
    let typed = (cpu.si() as usize).saturating_sub(0x0200).min(MAX_LINE);
    let page = cpu.bus.read_8(0x0462);
    let cols = (cpu.bus.read_16(0x044A) as usize).max(1);
    let (col, row) = (cpu.bus.read_8(0x0450 + page as usize * 2), cpu.bus.read_8(0x0451 + page as usize * 2));
    let start = (row as usize * cols + col as usize).saturating_sub(typed);
    let set_cursor = |cpu: &mut Cpu| video_call(cpu, 0x0200, (page as u16) << 8, 0, ((start / cols) as u16) << 8 | (start % cols) as u16);
    // Blank it, and write the new line from its start.
    set_cursor(cpu);
    for _ in 0..typed {
        video_call(cpu, 0x0E20, (page as u16) << 8, 0, 0);
    }
    set_cursor(cpu);
    for &b in &line {
        video_call(cpu, 0x0E00 | b as u16, (page as u16) << 8, 0, 0);
    }

    let buffer = cpu.get_physical_addr(cpu.ds(), 0x0200);
    for (i, &b) in line.iter().enumerate() {
        cpu.bus.write_8(buffer + i, b);
    }
    cpu.set_si(0x0200 + line.len() as u16);
    let (ax, bx, cx, dx) = saved;
    cpu.set_ax(ax);
    cpu.set_reg16(iced_x86::Register::BX, bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
}

/// The line being typed at the prompt: the buffer at DS:0200h up to SI.
fn typed_line(cpu: &Cpu) -> Vec<u8> {
    let buffer = cpu.get_physical_addr(cpu.ds(), 0x0200);
    let len = (cpu.si() as usize).saturating_sub(0x0200).min(MAX_LINE);
    (0..len).map(|i| cpu.bus.read_8(buffer + i)).collect()
}

/// Tab (`forward`) or Shift+Tab at the prompt, as in DOSBox: the line with
/// the name being typed, the last word after its last '\', '/' or ':',
/// completed to the first (Shift+Tab: last) file or directory beginning
/// with it. Tab again goes on to the next one, Shift+Tab back to the one
/// before. None when no name fits.
fn complete(cpu: &mut Cpu, forward: bool) -> Option<Vec<u8>> {
    let typed = typed_line(cpu);
    let mut completion = match cpu.shell_completion.take() {
        Some(mut completion) if completion.line == typed => {
            let count = completion.names.len();
            completion.index = (if forward { completion.index + 1 } else { completion.index + count - 1 }) % count;
            completion
        }
        _ => {
            let word = typed.iter().rposition(|&b| b == b' ').map_or(0, |i| i + 1);
            let start = typed[word..].iter().rposition(|&b| matches!(b, b'\\' | b'/' | b':')).map_or(word, |i| word + i + 1);
            let names = completion_names(cpu, &typed, word);
            let index = if forward { 0 } else { names.len().checked_sub(1)? };
            Completion { line: Vec::new(), start, names, index }
        }
    };
    let mut line = typed[..completion.start].to_vec();
    line.extend(dosstr::to_bytes(completion.names.get(completion.index)?));
    line.truncate(MAX_LINE);
    completion.line = line.clone();
    cpu.shell_completion = Some(completion);
    Some(line)
}

/// The names Tab goes through for the word at `word` in the line `typed`:
/// the files and directories beginning with it, the programs (.BAT, .COM,
/// .EXE) first, then the others, each in order of name. After CD only the
/// directories.
fn completion_names(cpu: &Cpu, typed: &[u8], word: usize) -> Vec<String> {
    let command = typed.split(|&b| b == b' ').find(|w| !w.is_empty()).unwrap_or_default();
    let cd = word > 0 && (command.eq_ignore_ascii_case(b"CD") || command.eq_ignore_ascii_case(b"CHDIR"));
    // "GA" looks for "GA*.*", "GAME.E" for "GAME.E*".
    let name = dosstr::from_bytes(&typed[word..]);
    let mask = if name.rsplit(['\\', '/', ':']).next().is_some_and(|n| n.contains('.')) {
        format!("{}*", name)
    } else {
        format!("{}*.*", name)
    };
    let Ok(entries) = cpu.bus.disk.list_directory(&mask, 0x16) else {
        return Vec::new();
    };
    let mut names: Vec<(bool, String)> = entries
        .into_iter()
        .filter(|e| e.filename != "." && e.filename != ".." && (e.is_dir || !cd))
        .map(|e| {
            let upper = e.filename.to_ascii_uppercase();
            let program = !e.is_dir && [".BAT", ".COM", ".EXE"].iter().any(|ext| upper.ends_with(ext));
            (!program, e.filename)
        })
        .collect();
    names.sort_by_cached_key(|(other, name)| (*other, name.to_ascii_uppercase()));
    names.into_iter().map(|(_, name)| name).collect()
}

/// INT 10h with these registers.
fn video_call(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16, dx: u16) {
    cpu.set_ax(ax);
    cpu.set_reg16(iced_x86::Register::BX, bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
    crate::interrupts::int10::handle(cpu);
}

/// SHELL_COMMAND_BOP handler: queue the ASCIIZ line at DS:DX for the main
/// loop to dispatch.
pub fn handle_command_bop(cpu: &mut Cpu) {
    // Safety: Clear buffer so we don't repeat commands
    cpu.bus.keyboard_buffer.clear();
    cpu.con_pending_scan = None;

    // Read Command from DS:DX (set by the shell code): the typed
    // characters, code page 437 ones included, without control characters.
    let phys_addr = cpu.get_physical_addr(cpu.ds(), cpu.dx());
    let mut clean = Vec::new();
    for b in read_asciiz_bytes(&cpu.bus, phys_addr) {
        match b {
            0x08 => {
                clean.pop();
            }
            0x20..=0x7E | 0x80..=0xFF => clean.push(b),
            _ => {}
        }
    }
    let clean_cmd = dosstr::from_bytes(&clean);
    cpu.shell_history.push(&clean_cmd);
    cpu.shell_completion = None;

    // Queue for Main Loop
    if !clean_cmd.is_empty() {
        cpu.pending_command = Some(clean_cmd);
    }
}

/// The DOS prompt for the current drive and directory, e.g. "D:\GAMES>".
pub fn prompt_string(disk: &DiskController) -> String {
    format!(
        "{}:\\{}>",
        drive_letter(disk.get_current_drive()),
        disk.get_current_directory()
    )
}

pub fn show_prompt(cpu: &mut Cpu) {
    // let col = cpu.bus.read_8(0x0450);
    // if col != 0 {
    //     video::print_string(cpu, "\r\n");
    // }

    let prompt = prompt_string(&cpu.bus.disk);
    video::print_string(cpu, &prompt);
}
