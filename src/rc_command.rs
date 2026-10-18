//! RC: Rat Commander (`crates/rc`), the file manager in the style of
//! Norton Commander, run from the shell.
//!
//! It runs as one of the shell's waits (`ShellWait::Rc`), as EDIT does:
//! the shell's code reads the keys and hands each one over (`key`), and
//! on every tick between them RC looks at the mouse and the clock
//! (`tick`). Here it gets the machine's files through `CpuHost`, and its
//! screen is copied into the text screen's memory.
//!
//! To run a command line (one typed, a program chosen, F4's EDIT) RC
//! ends its wait with the line for the shell to run, and stays in
//! `cpu.rc_resume`; when the shell would show its prompt again, `resume`
//! brings it back with the screen the command left.

use rust_dos_rc::{
    Commander, Drive, DriveKind, Entry, Host, Input, Mouse, Outcome, Screen, UserScreen,
};

use crate::command::ShellCommand;
use crate::cpu::Cpu;
use crate::shell::{ShellWait, enter_wait, video_call as video};

/// RC: the file manager.
pub struct RcCommand;

impl ShellCommand for RcCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        if args.contains("/?") {
            crate::video::print_string(
                cpu,
                "Rat Commander: a file manager in the style of the Norton Commander.\r\n\r\n\
                 RC\r\n\r\n\
                 F1 lists the keys, F9 opens the menus and F10 quits.\r\n",
            );
            return;
        }
        if cpu.stdin_redirect.is_some() {
            crate::video::print_string(cpu, "RC needs the keyboard\r\n");
            return;
        }
        let saved = Saved::take(cpu);
        let (cols, rows) = size(cpu);
        let user = user_screen(cpu, cols, rows, rows.saturating_sub(2));
        let rc = Commander::new(&mut CpuHost(cpu), cols, rows, user);
        let wait = Box::new(RcWait {
            rc,
            saved,
            owner: cpu.secondary.is_some().then_some(cpu.current_psp),
            batch_after: cpu.batch.is_active(),
        });
        show(cpu, wait.rc.screen());
        enter_wait(cpu, ShellWait::Rc(wait));
    }
}

/// RC waiting for keys: the commander, and what is put back when it
/// leaves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RcWait {
    rc: Commander,
    saved: Saved,
    /// The secondary COMMAND.COM it runs in (its PSP), None for the
    /// shell's own prompt: it comes back only to the one it left.
    owner: Option<u16>,
    /// Batch lines were left to run after RC when it started: they wait
    /// for it, as for any command of a batch file.
    batch_after: bool,
}

/// The cursor's shape and the mouse as they were before RC.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Saved {
    shape: u16,
    mouse_installed: bool,
    mouse_hidden: i32,
}

crate::state_fields!(Saved {
    shape,
    mouse_installed,
    mouse_hidden
});
crate::state_fields!(RcWait { rc, saved, owner, batch_after });

impl Saved {
    /// What RC changes, kept; a text mode if the screen was in graphics,
    /// and the mouse in text cells.
    fn take(cpu: &mut Cpu) -> Saved {
        if crate::video::text::geometry(&cpu.bus).is_none() {
            crate::interrupts::int10::set_mode(cpu, 0x03);
        }
        let (cols, rows) = size(cpu);
        let saved = Saved {
            shape: cpu.bus.read_16(0x0460),
            mouse_installed: cpu.bus.mouse.installed,
            mouse_hidden: cpu.bus.mouse.hide_counter,
        };
        let mouse = &mut cpu.bus.mouse;
        mouse.reset(cols as i32 * 8, rows as i32 * 8);
        mouse.hide_counter = 0;
        saved
    }

    fn restore(&self, cpu: &mut Cpu) {
        video(cpu, 0x0100, 0, self.shape, 0);
        let mouse = &mut cpu.bus.mouse;
        mouse.installed = self.mouse_installed;
        mouse.hide_counter = self.mouse_hidden;
    }
}

/// A key while RC waits: RC to wait on with, or None once it left.
pub fn key(cpu: &mut Cpu, wait: Box<RcWait>, key: u16) -> Option<Box<RcWait>> {
    step(cpu, wait, Some(key))
}

/// A tick while RC waits: the mouse, the clock, an operation going on.
/// Whether RC goes on.
pub fn tick(cpu: &mut Cpu) -> bool {
    let Some(ShellWait::Rc(wait)) = cpu.shell_wait.take() else {
        return true;
    };
    // Batch lines came to run while RC waits (a game launched from the
    // settings window, the debugger's commands): RC steps aside for them
    // and comes back after, as the prompt gives up a line being typed.
    if cpu.batch.is_active() && !wait.batch_after && wait.owner.is_none() {
        leave(cpu, &wait);
        if cpu.rc_resume.is_none() {
            cpu.rc_resume = Some(*wait);
        }
        return false;
    }
    match step(cpu, wait, None) {
        Some(wait) => {
            cpu.shell_wait = Some(ShellWait::Rc(wait));
            true
        }
        None => false,
    }
}

fn step(cpu: &mut Cpu, mut wait: Box<RcWait>, key: Option<u16>) -> Option<Box<RcWait>> {
    let (cols, rows) = size(cpu);
    let input = Input {
        key,
        shift: cpu.bus.read_8(0x0417),
        mouse: mouse(cpu),
        cols,
        rows,
    };
    let outcome = wait.rc.step(&mut CpuHost(cpu), &input);
    match outcome {
        Outcome::Stay { redraw } => {
            if redraw {
                show(cpu, wait.rc.screen());
            }
            Some(wait)
        }
        Outcome::Quit => {
            leave(cpu, &wait);
            None
        }
        Outcome::Run { line, echo } => {
            leave(cpu, &wait);
            if echo {
                crate::shell::show_prompt(cpu);
                crate::shell::teletype(cpu, &crate::dosstr::to_bytes(&format!("{}\r\n", line)));
            }
            cpu.shell_history.push(&line);
            cpu.pending_command = Some(line);
            // An RC started from a program RC ran doesn't come back: the
            // one waiting does.
            if cpu.rc_resume.is_none() {
                cpu.rc_resume = Some(*wait);
            }
            None
        }
    }
}

/// When the shell is about to show its prompt (`owner` None) or a
/// secondary COMMAND.COM its own (`owner` its PSP): RC back, if it left
/// from there to run a command line. Whether it is.
pub fn resume(cpu: &mut Cpu, owner: Option<u16>) -> bool {
    if cpu.rc_resume.as_ref().is_none_or(|w| w.owner != owner) {
        return false;
    }
    let Some(wait) = cpu.rc_resume.take() else {
        return false;
    };
    let mut wait = Box::new(wait);
    wait.batch_after = false;
    wait.saved = Saved::take(cpu);
    let (cols, rows) = size(cpu);
    let line = wait.rc.command_row().min(rows - 1);
    let user = user_screen(cpu, cols, rows, line);
    wait.rc.resume(&mut CpuHost(cpu), cols, rows, user);
    show(cpu, wait.rc.screen());
    // A secondary COMMAND.COM waits in its own code, which asks for the
    // keys once it sees the wait.
    if owner.is_some() {
        cpu.shell_wait = Some(ShellWait::Rc(wait));
    } else {
        enter_wait(cpu, ShellWait::Rc(wait));
    }
    true
}

/// The DOS screen back as RC keeps it, the cursor on the command line's
/// row, the cursor's shape and the mouse as they were.
fn leave(cpu: &mut Cpu, wait: &RcWait) {
    let user = wait.rc.user_screen();
    let (cols, rows) = size(cpu);
    let base = text_base(cpu);
    for r in 0..rows.min(user.rows) {
        for c in 0..cols.min(user.cols) {
            let (ch, attr) = user.get(r, c);
            let at = base + (r * cols + c) * 2;
            cpu.bus.write_8(at, ch);
            cpu.bus.write_8(at + 1, attr);
        }
    }
    let page = cpu.bus.read_8(0x0462) as u16;
    let row = wait.rc.command_row() as u16;
    video(cpu, 0x0200, page << 8, 0, row << 8);
    wait.saved.restore(cpu);
}

/// The screen into the text screen's memory, and the cursor.
fn show(cpu: &mut Cpu, s: &Screen) {
    let (cols, rows) = size(cpu);
    let base = text_base(cpu);
    for r in 0..rows.min(s.rows) {
        for c in 0..cols.min(s.cols) {
            let (ch, attr) = s.get(r, c);
            let at = base + (r * cols + c) * 2;
            cpu.bus.write_8(at, ch);
            cpu.bus.write_8(at + 1, attr);
        }
    }
    let page = cpu.bus.read_8(0x0462) as u16;
    match s.cursor {
        Some((c, r)) => {
            video(cpu, 0x0200, page << 8, 0, ((r as u16) << 8) | c as u16);
            video(cpu, 0x0100, 0, 0x0607, 0);
        }
        None => video(cpu, 0x0100, 0, 0x2000, 0),
    }
}

/// The DOS screen as it is, its cursor's row brought to where RC's
/// command line is.
fn user_screen(cpu: &mut Cpu, cols: usize, rows: usize, line: usize) -> UserScreen {
    let base = text_base(cpu);
    let cells = (0..cols * rows)
        .map(|i| {
            (
                cpu.bus.read_8(base + i * 2),
                cpu.bus.read_8(base + i * 2 + 1),
            )
        })
        .collect();
    let page = cpu.bus.read_8(0x0462) as usize;
    let cursor_row = cpu.bus.read_8(0x0451 + page * 2) as usize;
    UserScreen::new(cols, rows, cells, cursor_row, line)
}

/// Where the text screen's page shows in memory.
fn text_base(cpu: &Cpu) -> usize {
    cpu.bus.vga.text_window().0 + cpu.bus.read_16(0x044E) as usize
}

fn size(cpu: &Cpu) -> (usize, usize) {
    (cpu.bus.text_cols().max(40), cpu.bus.text_rows().max(10))
}

/// The mouse in text cells.
fn mouse(cpu: &Cpu) -> Mouse {
    let m = &cpu.bus.mouse;
    let cell = |x: i32| (x / 8).max(0) as usize;
    Mouse {
        col: cell(m.x),
        row: cell(m.y),
        buttons: m.buttons,
        presses: [m.press_count[0], m.press_count[1]],
        pressed_at: [
            (cell(m.press_x[0]), cell(m.press_y[0])),
            (cell(m.press_x[1]), cell(m.press_y[1])),
        ],
    }
}

/// The machine's files and state, as RC asks for them.
struct CpuHost<'a>(&'a mut Cpu);

fn letter(drive: u8) -> char {
    (b'A' + drive) as char
}

impl Host for CpuHost<'_> {
    fn drives(&self) -> Vec<Drive> {
        use crate::disk::DriveKind as Kind;
        self.0
            .bus
            .disk
            .mounted_drives()
            .into_iter()
            .map(|d| Drive {
                drive: d.drive,
                kind: match d.kind {
                    Kind::Floppy => DriveKind::Floppy,
                    Kind::CdRom => DriveKind::Cdrom,
                    Kind::HardDisk => DriveKind::Hard,
                    Kind::Virtual => DriveKind::Network,
                },
                writable: !d.read_only,
            })
            .collect()
    }

    fn current_drive(&self) -> u8 {
        self.0.bus.disk.get_current_drive()
    }

    fn current_dir(&self, drive: u8) -> Option<String> {
        let dir = self.0.bus.disk.get_current_directory_of(drive)?;
        Some(format!(
            "{}:\\{}",
            letter(drive),
            dir.trim_start_matches('\\')
        ))
    }

    fn set_current(&mut self, dir: &str) {
        let disk = &mut self.0.bus.disk;
        let drive = (dir.as_bytes()[0].to_ascii_uppercase()).wrapping_sub(b'A');
        disk.set_current_drive(drive);
        disk.set_current_directory(dir);
    }

    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, u8> {
        let spec = if dir.ends_with('\\') {
            format!("{}*.*", dir)
        } else {
            format!("{}\\*.*", dir)
        };
        let list = self.0.bus.disk.list_directory(&spec, 0x16)?;
        Ok(list
            .into_iter()
            .filter(|e| e.filename != "." && e.filename != "..")
            .map(|e| Entry {
                name: e.filename,
                size: e.size as u64,
                attr: e.attr | if e.is_dir { 0x10 } else { 0 },
                time: e.dos_time,
                date: e.dos_date,
            })
            .collect())
    }

    fn read(&mut self, path: &str) -> Result<Vec<u8>, u8> {
        let data = self
            .0
            .bus
            .disk
            .file_data(path)
            .ok_or(0x02u8)?
            .read()
            .map_err(|_| 0x05u8)?;
        crate::file_commands::disk_activity(self.0, path, data.len(), false);
        Ok(data.into_owned())
    }

    fn write(&mut self, path: &str, data: &[u8], stamp: (u16, u16)) -> Result<(), u8> {
        self.0.bus.disk.write_whole_file(path, data, Some(stamp))?;
        crate::file_commands::disk_activity(self.0, path, data.len(), true);
        Ok(())
    }

    fn delete(&mut self, path: &str) -> Result<(), u8> {
        self.0.bus.disk.delete_file(path)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), u8> {
        self.0.bus.disk.rename_file(from, to)
    }

    fn mkdir(&mut self, path: &str) -> Result<(), u8> {
        self.0.bus.disk.create_directory(path)
    }

    fn rmdir(&mut self, path: &str) -> Result<(), u8> {
        self.0.bus.disk.remove_directory(path)
    }

    fn attributes(&mut self, path: &str) -> Result<u8, u8> {
        self.0.bus.disk.get_file_attribute(path).map(|a| a as u8)
    }

    fn set_attributes(&mut self, path: &str, attr: u8) -> Result<(), u8> {
        self.0.bus.disk.set_file_attribute(path, attr as u16)
    }

    fn space(&self, drive: u8) -> Option<(u64, u64)> {
        let (spc, free, bps, total) = self.0.bus.disk.get_disk_free_space32(drive + 1).ok()?;
        let cluster = spc as u64 * bps as u64;
        Some((total as u64 * cluster, free as u64 * cluster))
    }

    fn label(&self, drive: u8) -> Option<String> {
        self.0.bus.disk.volume_label(drive)
    }

    fn memory(&mut self) -> (u64, u64) {
        let (total, free) = crate::mem_command::conventional(&mut self.0.bus);
        (total as u64, free as u64)
    }

    fn prompt(&self) -> Vec<u8> {
        crate::shell::render_prompt(self.0)
    }

    fn history(&self) -> Vec<String> {
        self.0.shell_history.entries().to_vec()
    }

    fn now_ms(&self) -> u64 {
        self.0.bus.clock.now_ns() / 1_000_000
    }

    fn clock(&self) -> (u8, u8, u8) {
        use chrono::Timelike;
        let now = self.0.bus.cmos.now();
        (now.hour() as u8, now.minute() as u8, now.second() as u8)
    }
}
