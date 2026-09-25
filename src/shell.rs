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

/// Where the shell's code is loaded in its segment.
const ORIGIN: u16 = 0x0100;

/// Offsets in the shell's code (at SHELL_SEGMENT:0100h) that the emulator
/// sends it to.
#[derive(Clone, Copy, Debug)]
pub struct ShellLabels {
    /// The prompt is printed and a line read from here.
    pub prompt_start: u16,
    /// Where the prompt's INT 16h for the next key of the line returns.
    pub key_read: u16,
    /// The command trap returns here, to a JMP PROMPT_START: batch lines are
    /// dispatched before it.
    pub after_trap: u16,
    /// PAUSE and CHOICE wait for a key here.
    pub shell_wait: u16,
    /// The key they waited for is handed over here, in AX.
    pub key_ready: u16,
}

/// A tiny assembler for the shell's code: bytes, and jumps to labels
/// resolved at the end, so the code can change without offsets worked out
/// by hand.
struct Asm {
    code: Vec<u8>,
    labels: Vec<(&'static str, u16)>,
    /// (position of the operand, label, 1 for a short jump's displacement
    /// or 2 for an absolute word)
    fixups: Vec<(usize, &'static str, u8)>,
}

impl Asm {
    fn label(&mut self, name: &'static str) {
        self.labels.push((name, ORIGIN + self.code.len() as u16));
    }

    fn op(&mut self, bytes: &[u8]) {
        self.code.extend_from_slice(bytes);
    }

    /// A short jump (`opcode` rel8) to a label.
    fn jump(&mut self, opcode: u8, target: &'static str) {
        self.code.push(opcode);
        self.fixups.push((self.code.len(), target, 1));
        self.code.push(0);
    }

    /// An instruction ending in the address of a label as a word.
    fn address(&mut self, bytes: &[u8], target: &'static str) {
        self.code.extend_from_slice(bytes);
        self.fixups.push((self.code.len(), target, 2));
        self.code.extend_from_slice(&[0, 0]);
    }

    fn at(&self, name: &str) -> u16 {
        self.labels.iter().find(|(n, _)| *n == name).map(|&(_, at)| at).unwrap_or_else(|| panic!("no label {}", name))
    }

    fn finish(mut self) -> Vec<u8> {
        for &(pos, target, size) in &self.fixups {
            let target = self.at(target);
            if size == 1 {
                let next = ORIGIN as i32 + pos as i32 + 1;
                let disp = i8::try_from(target as i32 - next).expect("short jump out of range");
                self.code[pos] = disp as u8;
            } else {
                self.code[pos..pos + 2].copy_from_slice(&target.to_le_bytes());
            }
        }
        self.code
    }
}

/// A Tiny "OS" written in Machine Code, and its labels. Asks the emulator
/// for the prompt, reads keys into a buffer at offset 0x0200, and on Enter
/// hands the line to the Rust shell via the SHELL_COMMAND_BOP trap.
/// Handles backspace visually and in buffer, and hands the other control
/// keys and the extended keys (Esc, Tab, Up and Down) to `edit_key`.
fn assemble() -> (Vec<u8>, ShellLabels) {
    use crate::bios::{SERVICE_SHELL_KEY, SERVICE_SHELL_KEY_READY, SERVICE_SHELL_PROMPT, SERVICE_SHELL_TICK};
    let mut a = Asm { code: Vec::new(), labels: Vec::new(), fixups: Vec::new() };
    // We are loaded at SHELL_SEGMENT:0100 (see Cpu::load_shell). Set
    // DS=ES=SS=CS so buffers and stack live in the shell segment.
    a.op(&[0x8C, 0xC8]); // MOV AX, CS
    a.op(&[0x8E, 0xD8]); // MOV DS, AX
    a.op(&[0x8E, 0xC0]); // MOV ES, AX
    a.op(&[0x8E, 0xD0]); // MOV SS, AX
    a.op(&[0xBC, 0x00, 0x0F]); // MOV SP, 0x0F00 (SHELL_STACK)

    a.label("PROMPT_START");
    a.op(&[0xFE, 0x39, SERVICE_SHELL_PROMPT]); // `prompt`: print it, SI = 0200h

    // Read keys: Enter hands the line over, Backspace takes a character
    // back, control and extended keys (AL below 20h) go to `edit_key`.
    a.label("WAIT_KEY");
    a.op(&[0xB4, 0x00, 0xCD, 0x16]); // MOV AH, 00h; INT 16h
    a.label("KEY_READ");
    a.op(&[0x3C, 0x0D]); // CMP AL, 0Dh
    a.jump(0x74, "EXECUTE"); // JE
    a.op(&[0x3C, 0x08]); // CMP AL, 08h
    a.jump(0x74, "BACKSPACE"); // JE
    a.op(&[0x3C, 0x20]); // CMP AL, 20h
    a.jump(0x72, "EDIT_KEY"); // JB
    // A full buffer takes no more (MAX_LINE characters)
    a.op(&[0x81, 0xFE, 0x7F, 0x02]); // CMP SI, 027Fh
    a.jump(0x73, "WAIT_KEY"); // JAE
    a.op(&[0xB4, 0x0E, 0xCD, 0x10]); // MOV AH, 0Eh; INT 10h: echo it
    a.op(&[0x88, 0x04, 0x46]); // MOV [SI], AL; INC SI
    a.jump(0xEB, "WAIT_KEY");

    a.label("BACKSPACE");
    a.op(&[0x81, 0xFE, 0x00, 0x02]); // CMP SI, 0200h
    a.jump(0x74, "WAIT_KEY"); // JE: nothing to take back
    a.op(&[0x4E, 0xB4, 0x0E]); // DEC SI; MOV AH, 0Eh
    a.op(&[0xB0, 0x08, 0xCD, 0x10, 0xB0, 0x20, 0xCD, 0x10, 0xB0, 0x08, 0xCD, 0x10]); // BS, space, BS
    a.jump(0xEB, "WAIT_KEY");

    // Esc, Tab or the history replace the line (SI).
    a.label("EDIT_KEY");
    a.op(&[0xFE, 0x39, SERVICE_SHELL_KEY]);
    a.jump(0xEB, "WAIT_KEY");

    a.label("EXECUTE");
    a.op(&[0xC6, 0x04, 0x00]); // MOV BYTE PTR [SI], 0
    a.op(&[0xB4, 0x0E, 0xB0, 0x0D, 0xCD, 0x10, 0xB0, 0x0A, 0xCD, 0x10]); // CR LF
    a.op(&[0xBA, 0x00, 0x02]); // MOV DX, 0200h
    // The trap returns like an IRET, so build the frame an INT would:
    // FLAGS, CS, then the address of the JMP below.
    a.op(&[0x9C, 0x0E]); // PUSHF; PUSH CS
    a.address(&[0xB8], "AFTER_TRAP"); // MOV AX, AFTER_TRAP
    a.op(&[0x50]); // PUSH AX
    a.op(&[0xFE, 0x38, SHELL_COMMAND_BOP]); // Hand the line to the Rust shell
    a.label("AFTER_TRAP");
    a.jump(0xEB, "PROMPT_START");

    // PAUSE and CHOICE: wait for a key with the timers running (a CHOICE
    // with a timeout looks at the time on every tick), then hand it over.
    a.label("SHELL_WAIT");
    a.op(&[0xB4, 0x01, 0xCD, 0x16]); // MOV AH, 01h; INT 16h
    a.jump(0x75, "GOT_KEY"); // JNZ
    a.op(&[0xFE, 0x39, SERVICE_SHELL_TICK]); // may hand over the default
    a.op(&[0xF4]); // HLT until the next tick
    a.jump(0xEB, "SHELL_WAIT");
    a.label("GOT_KEY");
    a.op(&[0xB4, 0x00, 0xCD, 0x16]); // MOV AH, 00h; INT 16h
    a.label("KEY_READY");
    a.op(&[0xFE, 0x39, SERVICE_SHELL_KEY_READY]); // `key_ready`
    a.jump(0xEB, "PROMPT_START");

    let labels = ShellLabels {
        prompt_start: a.at("PROMPT_START"),
        key_read: a.at("KEY_READ"),
        after_trap: a.at("AFTER_TRAP"),
        shell_wait: a.at("SHELL_WAIT"),
        key_ready: a.at("KEY_READY"),
    };
    (a.finish(), labels)
}

static SHELL: std::sync::OnceLock<(Vec<u8>, ShellLabels)> = std::sync::OnceLock::new();

/// The shell's code, loaded at SHELL_SEGMENT:0100h.
pub fn get_shell_code() -> Vec<u8> {
    SHELL.get_or_init(assemble).0.clone()
}

/// Where the parts of the shell's code are.
pub fn labels() -> ShellLabels {
    SHELL.get_or_init(assemble).1
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

/// Print the prompt PROMPT sets (`render_prompt`), as the shell does
/// before a line is typed or a batch line runs. The registers stay as they
/// were.
pub fn show_prompt(cpu: &mut Cpu) {
    let text = render_prompt(cpu);
    teletype(cpu, &text);
}

/// The prompt as the PROMPT variable has it ($P$G if it isn't set), with
/// its codes put in: $P the current drive and directory, $N the drive,
/// $G > $L < $B | $Q = $A & $C ( $F ), $D the date, $T the time, $V the
/// version, $_ a new line, $E Escape, $H a backspace and $$ a dollar sign.
pub fn render_prompt(cpu: &Cpu) -> Vec<u8> {
    let spec = dosstr::to_bytes(cpu.get_env("PROMPT").unwrap_or("$P$G"));
    let now = crate::hosttime::now();
    let mut out = Vec::new();
    let mut codes = spec.iter();
    while let Some(&b) = codes.next() {
        if b != b'$' {
            out.push(b);
            continue;
        }
        let Some(code) = codes.next() else { break };
        match code.to_ascii_uppercase() {
            b'P' => out.extend(dosstr::to_bytes(prompt_string(&cpu.bus.disk).trim_end_matches('>'))),
            b'N' => out.push(drive_letter(cpu.bus.disk.get_current_drive()) as u8),
            b'G' => out.push(b'>'),
            b'L' => out.push(b'<'),
            b'B' => out.push(b'|'),
            b'Q' => out.push(b'='),
            b'A' => out.push(b'&'),
            b'C' => out.push(b'('),
            b'F' => out.push(b')'),
            b'$' => out.push(b'$'),
            b'_' => out.extend(b"\r\n"),
            b'E' => out.push(0x1B),
            b'H' => out.extend(b"\x08 \x08"),
            b'D' => out.extend(now.format("%a %m-%d-%Y").to_string().bytes()),
            b'T' => {
                use chrono::Timelike;
                let hundredths = now.nanosecond() / 10_000_000 % 100;
                out.extend(format!("{}.{:02}", now.format("%H:%M:%S"), hundredths).bytes());
            }
            b'V' => out.extend(format!("Rust-DOS Version {}", env!("CARGO_PKG_VERSION")).bytes()),
            _ => {}
        }
    }
    out
}

/// Print code page 437 text with INT 10h's teletype, at the cursor of the
/// page shown, as the shell's own code prints; the registers stay as they
/// were.
fn teletype(cpu: &mut Cpu, text: &[u8]) {
    let saved = (cpu.ax(), cpu.bx(), cpu.cx(), cpu.dx());
    let page = cpu.bus.read_8(0x0462) as u16;
    for &b in text {
        video_call(cpu, 0x0E00 | b as u16, page << 8, 0, 0);
    }
    let (ax, bx, cx, dx) = saved;
    cpu.set_ax(ax);
    cpu.set_reg16(iced_x86::Register::BX, bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
}

/// The cursor of the page shown: (column, row).
fn cursor(cpu: &Cpu) -> (u8, u8) {
    let page = cpu.bus.read_8(0x0462) as usize;
    (cpu.bus.read_8(0x0450 + page * 2), cpu.bus.read_8(0x0451 + page * 2))
}

/// The shell's code at PROMPT_START (SERVICE_SHELL_PROMPT): the prompt,
/// unless ECHO is off or DATE or TIME ask for a line, and the line
/// buffer emptied (SI).
pub fn prompt(cpu: &mut Cpu) {
    cpu.set_si(0x0200);
    // The batch files have run: ECHO is as it was before them.
    cpu.batch.settle();
    cpu.shell_prompt_at = None;
    if cpu.batch.echo && cpu.shell_wait.is_none() {
        cpu.shell_prompt_at = Some(cursor(cpu));
        show_prompt(cpu);
    }
}

/// What the shell waits for, outside of a typed line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellWait {
    /// PAUSE: any key.
    Pause,
    /// CHOICE: one of its keys.
    Choice(Choice),
}

/// A CHOICE waiting for a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// The keys it takes; the ERRORLEVEL is the position of the one
    /// pressed, from 1.
    pub keys: Vec<u8>,
    /// Whether 'y' and 'Y' are different keys (/S).
    pub case_sensitive: bool,
    /// The key it takes without one, and when (PIT ticks), for /T.
    pub timeout: Option<(u8, u64)>,
}

impl Choice {
    /// The position of `key` among the keys, if it is one of them.
    fn index(&self, key: u8) -> Option<usize> {
        self.keys.iter().position(|&k| if self.case_sensitive { k == key } else { k.eq_ignore_ascii_case(&key) })
    }
}

/// Have the shell wait for a key for PAUSE or CHOICE, from the command
/// that asks: its code goes to SHELL_WAIT, where the timers and interrupts
/// run on, and the key comes to `key_ready`. Batch lines wait until then.
pub fn enter_wait(cpu: &mut Cpu, wait: ShellWait) {
    use crate::cpu::{SHELL_SEGMENT, SHELL_STACK};
    cpu.set_cs(SHELL_SEGMENT);
    cpu.set_ds(SHELL_SEGMENT);
    cpu.set_es(SHELL_SEGMENT);
    cpu.set_ss(SHELL_SEGMENT);
    cpu.set_sp(SHELL_STACK);
    cpu.set_ip(labels().shell_wait);
    cpu.shell_wait = Some(wait);
}

/// The shell's code at KEY_READY (SERVICE_SHELL_KEY_READY), with the key
/// PAUSE or CHOICE waited for in AX. Ctrl+C ends the batch files; a key
/// CHOICE doesn't take beeps and waits again.
pub fn key_ready(cpu: &mut Cpu) {
    let key = cpu.ax() as u8;
    let Some(wait) = cpu.shell_wait.take() else { return };
    if key == 0x03 {
        video::print_string(cpu, "^C\r\n");
        cpu.batch.clear();
        return;
    }
    match wait {
        ShellWait::Pause => video::print_string(cpu, "\r\n"),
        ShellWait::Choice(choice) => match choice.index(key) {
            Some(i) => {
                video::print_string(cpu, &format!("{}\r\n", choice.keys[i] as char));
                cpu.errorlevel = i as u8 + 1;
            }
            None => {
                crate::audio::play_sdl_beep(&mut cpu.bus);
                cpu.shell_wait = Some(ShellWait::Choice(choice));
                cpu.set_ip(labels().shell_wait);
            }
        },
    }
}

/// The shell's code on every tick while PAUSE or CHOICE waits
/// (SERVICE_SHELL_TICK): once a CHOICE's time is up, its default key is
/// handed over as if pressed.
pub fn tick(cpu: &mut Cpu) {
    if let Some(ShellWait::Choice(Choice { timeout: Some((key, at)), .. })) = cpu.shell_wait
        && cpu.bus.clock.now_ticks() >= at
    {
        cpu.set_ax(key as u16);
        cpu.set_ip(labels().key_ready);
    }
}

/// While the shell waits in INT 16h for the next key of a line being
/// typed, batch lines came to run (the browser page's commands, a game
/// launched from the settings window): give up the line, take the prompt
/// back off the screen, and go on where they are dispatched, instead of
/// waiting for a key first. Called from INT 16h waiting for a key; false
/// if its caller isn't the shell at its prompt.
pub fn abandon_input(cpu: &mut Cpu) -> bool {
    use crate::cpu::SHELL_SEGMENT;
    if cpu.pe() || !cpu.batch.is_active() || cpu.shell_wait.is_some() || !cpu.process_stack.is_empty() {
        return false;
    }
    let frame = cpu.get_physical_addr(cpu.ss(), cpu.sp());
    if cpu.bus.read_16(frame + 2) != SHELL_SEGMENT || cpu.bus.read_16(frame) != labels().key_read {
        return false;
    }
    if let Some((col, row)) = cpu.shell_prompt_at.take() {
        let (end_col, end_row) = cursor(cpu);
        let cols = (cpu.bus.read_16(0x044A) as usize).max(1);
        let start = row as usize * cols + col as usize;
        let end = end_row as usize * cols + end_col as usize;
        let page = cpu.bus.read_8(0x0462) as u16;
        let at = |cpu: &mut Cpu| video_call(cpu, 0x0200, page << 8, 0, ((row as u16) << 8) | col as u16);
        at(cpu);
        for _ in start..end.max(start) {
            video_call(cpu, 0x0E20, page << 8, 0, 0);
        }
        at(cpu);
    }
    cpu.set_sp(cpu.sp().wrapping_add(6));
    cpu.set_cs(SHELL_SEGMENT);
    cpu.set_ip(labels().after_trap);
    true
}
