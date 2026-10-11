//! The power-on self test screen (`post`): before the built-in DOS starts,
//! and again at each reboot, a BIOS screen counts the memory and lists the
//! CPU, the drives and the sound cards, with "Press DEL to enter SETUP".
//! Del opens the settings as a BIOS setup (`Bus::setup_requested`), Esc
//! skips the memory count. It is the shell waiting (`ShellWait::Post`), so
//! the startup's batch lines wait for it to end, and DOS's box and prompt
//! come after it.

use crate::cpu::Cpu;
use crate::disk::DriveKind;
use crate::timer::PIT_HZ;

/// How the machine shows its POST (`post`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PostMode {
    /// None: DOS starts at once.
    #[default]
    Off,
    /// A moment, with the memory counted at once.
    Quick,
    /// The memory counted up as it is tested, a few seconds.
    Full,
}

impl PostMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::Quick, Self::Full];

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Quick => "quick",
            Self::Full => "full",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|m| m.name() == value)
    }

    /// As the settings window shows it.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Quick => "quick (1 s)",
            Self::Full => "full, with the memory test (3 s)",
        }
    }

    /// How long the memory count and the whole screen take, in PIT ticks.
    fn lengths(self) -> (u64, u64) {
        match self {
            Self::Off => (0, 0),
            Self::Quick => (0, PIT_HZ),
            Self::Full => (PIT_HZ * 8 / 5, PIT_HZ * 16 / 5),
        }
    }
}

/// The POST running.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Post {
    mode: PostMode,
    /// When it started, in PIT ticks; when the memory count is done; and
    /// when it ends.
    start: u64,
    counted: u64,
    end: u64,
    /// The memory to count to, in KB.
    memory_kb: u32,
    /// The memory count shown last.
    shown_kb: u32,
}

/// The BIOS's badge at the top right, in its box.
const BADGE: [&str; 4] = [
    "\u{2554}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2557}",
    "\u{2551} RUST-DOS \u{2551}",
    "\u{2551}  BIOS 86 \u{2551}",
    "\u{255A}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{255D}",
];

const NORMAL: u8 = 0x07;
const BRIGHT: u8 = 0x0F;
const BADGE_ATTR: u8 = 0x1E;
const OK: u8 = 0x0A;

impl crate::savestate::State for PostMode {
    fn save(&self, w: &mut crate::savestate::Writer) {
        (*self as u8).save(w);
    }
    fn load(&mut self, r: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        let mut n = 0u8;
        n.load(r)?;
        *self = Self::ALL.get(n as usize).copied().unwrap_or_default();
        Ok(())
    }
}

crate::state_fields!(Post { mode, start, counted, end, memory_kb, shown_kb });

/// Write `text` at (`col`, `row`) of the text screen in `attr`.
fn put(cpu: &mut Cpu, col: usize, row: usize, text: &str, attr: u8) {
    let (base, _, _) = cpu.bus.vga.text_window();
    for (i, c) in text.chars().enumerate().take(80usize.saturating_sub(col)) {
        let at = base + (row * 80 + col + i) * 2;
        cpu.bus.write_8(at, crate::keylayout::cp437(c).unwrap_or(b'?'));
        cpu.bus.write_8(at + 1, attr);
    }
}

fn clear(cpu: &mut Cpu) {
    let (base, _, _) = cpu.bus.vga.text_window();
    for i in (0..cpu.bus.text_rows() * 160).step_by(2) {
        cpu.bus.write_8(base + i, 0x20);
        cpu.bus.write_8(base + i + 1, NORMAL);
    }
}

fn set_cursor(cpu: &mut Cpu, col: usize, row: usize) {
    cpu.bus.cursor_x = col;
    cpu.bus.cursor_y = row;
    cpu.bus.write_8(0x0450, col as u8);
    cpu.bus.write_8(0x0451, row as u8);
}

/// What the screen lists under the memory: the drives and the sound
/// cards.
fn devices(cpu: &Cpu) -> Vec<String> {
    let bus = &cpu.bus;
    let mut lines: Vec<String> = bus
        .disk
        .mounted_drives()
        .into_iter()
        .filter(|d| d.kind != DriveKind::Virtual)
        .map(|d| {
            let kind = match d.kind {
                DriveKind::Floppy => "Floppy disk",
                DriveKind::HardDisk => "Hard disk",
                DriveKind::CdRom => "CD-ROM drive",
                DriveKind::Virtual => "",
            };
            let label = if d.label.is_empty() { String::new() } else { format!("  {}", d.label) };
            format!("Drive {}:    {}{}", (b'A' + d.drive) as char, kind, label)
        })
        .collect();
    if let Some(sb) = &bus.sb {
        lines.push(format!("Sound card:  Sound Blaster  {}", sb.config.blaster()));
    }
    if let Some(gus) = &bus.gus {
        lines.push(format!("Sound card:  Gravis Ultrasound at {:X}h", gus.config.base));
    }
    if bus.awe.is_some() {
        lines.push("Sound card:  AWE32 wavetable (EMU8000)".to_string());
    }
    lines
}

/// Start the POST: its screen, and the shell waiting for it.
pub fn begin(cpu: &mut Cpu, mode: PostMode) {
    let now = cpu.bus.clock.now_ticks();
    let (count, total) = mode.lengths();
    let memory_kb = (cpu.bus.ram().len() / 1024) as u32;
    let post = Post { mode, start: now, counted: now + count, end: now + total, memory_kb, shown_kb: u32::MAX };
    clear(cpu);
    put(cpu, 1, 0, &format!("Rust-DOS BIOS v{}", env!("CARGO_PKG_VERSION")), BRIGHT);
    put(cpu, 1, 1, "Copyright (C) 2026 the Rust-DOS authors", NORMAL);
    for (row, text) in BADGE.iter().enumerate() {
        put(cpu, 66, row, text, BADGE_ATTR);
    }
    let cpu_line = format!("{} CPU, {} cycles per millisecond", cpu.model.describe(), cpu.bus.clock.cycles_per_ms());
    put(cpu, 1, 4, &cpu_line, NORMAL);
    let rows = cpu.bus.text_rows();
    put(cpu, 1, rows - 1, "Press DEL to enter SETUP, ESC to skip the memory test", NORMAL);
    let mut post = Box::new(post);
    show(cpu, &mut post);
    crate::shell::enter_wait(cpu, crate::shell::ShellWait::Post(post));
}

/// Draw the count and, once it is done, the devices.
fn show(cpu: &mut Cpu, post: &mut Post) {
    let now = cpu.bus.clock.now_ticks();
    let counting = post.counted.saturating_sub(post.start).max(1);
    let done = now >= post.counted;
    let kb = if done {
        post.memory_kb
    } else {
        // In steps of 64 KB, as BIOSes counted.
        let part = now.saturating_sub(post.start) * post.memory_kb as u64 / counting;
        (part as u32) & !63
    };
    if kb == post.shown_kb {
        return;
    }
    post.shown_kb = kb;
    put(cpu, 1, 5, &format!("Memory Test : {:>8}K", kb), NORMAL);
    if done {
        put(cpu, 25, 5, "OK", OK);
        for (i, line) in devices(cpu).iter().enumerate() {
            put(cpu, 1, 7 + i, line, NORMAL);
        }
    }
    set_cursor(cpu, 0, 6);
}

/// The POST on a timer tick: on with the count; false once it is over and
/// DOS starts.
pub fn tick(cpu: &mut Cpu) -> bool {
    let Some(crate::shell::ShellWait::Post(mut post)) = cpu.shell_wait.take() else { return false };
    show(cpu, &mut post);
    if cpu.bus.clock.now_ticks() >= post.end {
        finish(cpu);
        return false;
    }
    cpu.shell_wait = Some(crate::shell::ShellWait::Post(post));
    true
}

/// A key during the POST: Del enters setup, Esc skips the count. The POST
/// if it goes on.
pub fn key(cpu: &mut Cpu, mut post: Box<Post>, key: u16) -> Option<Box<Post>> {
    let (scan, ascii) = ((key >> 8) as u8, key as u8);
    if scan == 0x53 && matches!(ascii, 0x00 | 0xE0 | 0x2E) {
        put(cpu, 1, cpu.bus.text_rows() - 1, &format!("{:<60}", "Entering SETUP..."), BRIGHT);
        cpu.bus.setup_requested = true;
        finish(cpu);
        return None;
    }
    if ascii == 0x1B && cpu.bus.clock.now_ticks() < post.counted {
        let now = cpu.bus.clock.now_ticks();
        let rest = post.end.saturating_sub(post.counted);
        (post.counted, post.end) = (now, now + rest);
        show(cpu, &mut post);
    }
    Some(post)
}

/// The POST is over: DOS's box, and the prompt after it.
fn finish(cpu: &mut Cpu) {
    clear(cpu);
    set_cursor(cpu, 0, 0);
    let (notes, update) = (cpu.startup.notes.clone(), cpu.startup.update.clone());
    crate::video::print_banner(cpu, &notes, update.as_deref());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse_back() {
        for mode in PostMode::ALL {
            assert_eq!(PostMode::parse(mode.name()), Some(mode));
        }
        assert_eq!(PostMode::parse(" FULL "), Some(PostMode::Full));
        assert_eq!(PostMode::parse("slow"), None);
    }

    #[test]
    fn the_badge_s_box_is_closed() {
        let widths: Vec<usize> = BADGE.iter().map(|row| row.chars().count()).collect();
        assert!(widths.iter().all(|&w| w == widths[0]), "{:?}", widths);
    }
}
