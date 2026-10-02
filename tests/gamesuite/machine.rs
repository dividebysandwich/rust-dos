//! A machine set up as rust-dos starts one from a configuration, run in
//! emulated time: the same input at the same emulated millisecond gives
//! the same pictures and sound on every run.

use rust_dos::config::{self, Settings};
use rust_dos::cpu::Cpu;
use rust_dos::exec::{self, NoHook, StopReason};
use rust_dos::keyboard::{self, PcKey};
use rust_dos::timer::CpuSpeed;
use rust_dos::video::{self, Frame};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

/// How often the sound is mixed, in emulated milliseconds.
const AUDIO_EVERY_MS: u64 = 10;

pub struct Machine {
    pub cpu: Cpu,
    /// Emulated milliseconds since the start.
    pub ms: u64,
    /// Its clock's lead over the start time.
    clock_ms: u64,
    cycles_per_ms: u64,
    /// The sound since `listen` started, while it is on.
    listening: Option<Vec<i16>>,
    /// Log lines that look like trouble: unhandled functions, unknown
    /// opcodes.
    pub notable: Rc<RefCell<Vec<String>>>,
    pub exited: bool,
    /// Pictures saved every so many milliseconds into a folder, to see
    /// what a game does when writing its script (`GAME_SUITE_SNAP`).
    pub snap: Option<(std::path::PathBuf, u64)>,
}

/// The date and time the machines start at; their real-time clock runs on
/// from it with emulated time, the same on every run.
fn start_time() -> chrono::NaiveDateTime {
    chrono::NaiveDate::from_ymd_opt(1995, 4, 11).unwrap().and_hms_opt(12, 34, 56).unwrap()
}

/// Set this thread's host clock to `ms` emulated milliseconds after the
/// start.
fn set_clock(ms: u64) {
    rust_dos::hosttime::fix(Some(start_time() + chrono::TimeDelta::milliseconds(ms as i64)));
}

/// How much later than the first machine's a second one's clock is:
/// programs that tell machines apart by the time (DOOM's SERSETUP) would
/// take two at the same time for one.
pub const PARTNER_CLOCK_MS: u64 = 4_987_654;

/// Log lines worth reporting, though they don't fail a scenario.
fn notable(line: &str) -> bool {
    ["Unhandled", "Unsupported", "Unknown", "Invalid opcode", "Triple fault", "shutdown"]
        .iter()
        .any(|word| line.contains(word))
}

impl Machine {
    /// A machine with C: at `c_dir` and the settings, drives and
    /// `[autoexec]` of the configuration `ini`. The speed must be fixed:
    /// `auto` and `max` follow the host's clock.
    pub fn new(c_dir: &Path, ini: &str, log: Option<&Path>) -> Result<Machine, String> {
        Machine::with_clock(c_dir, ini, log, 0)
    }

    /// A machine whose clock starts `clock_ms` after the start time.
    pub fn with_clock(c_dir: &Path, ini: &str, log: Option<&Path>, clock_ms: u64) -> Result<Machine, String> {
        set_clock(clock_ms);
        let config = config::parse(ini, c_dir, None);
        if !config.warnings.is_empty() {
            return Err(format!("configuration: {}", config.warnings.join("; ")));
        }
        let settings = Settings::from_config(&config);
        let CpuSpeed::Fixed(cycles) = settings.cycles else {
            return Err(format!("the speed must be fixed, not {:?}", settings.cycles));
        };
        let mut cpu = Cpu::with_memory(c_dir.to_path_buf(), settings.memsize);
        if let Some(path) = log {
            cpu.bus.log_file = rust_dos::log::LogFile::create(path).ok();
        }
        let notable_lines = Rc::new(RefCell::new(Vec::new()));
        let sink = notable_lines.clone();
        cpu.bus.log_hook = Some(Box::new(move |line: &str| {
            if notable(line) {
                let mut lines = sink.borrow_mut();
                if lines.len() < 200 && !lines.iter().any(|l| l == line) {
                    lines.push(line.to_string());
                }
            }
        }));
        let warnings = rust_dos::hardware::configure(&mut cpu, &settings, rust_dos::keylayout::Layout::us());
        if !warnings.is_empty() {
            return Err(format!("hardware: {}", warnings.join("; ")));
        }
        for spec in config.drives.iter().filter(|s| s.drive != rust_dos::disk::DRIVE_C) {
            cpu.bus.mount_drive(spec.drive, &spec.path, spec.opts.clone(), true)?;
        }
        cpu.bus.set_cycles_per_ms(cycles);
        cpu.load_shell();
        cpu.queue_batch_lines(&config.autoexec);
        Ok(Machine {
            cpu,
            ms: 0,
            clock_ms,
            cycles_per_ms: cycles as u64,
            listening: None,
            notable: notable_lines,
            exited: false,
            snap: None,
        })
    }

    pub fn set_cycles(&mut self, cycles: u32) {
        self.cpu.bus.set_cycles_per_ms(cycles);
        self.cycles_per_ms = cycles as u64;
    }

    /// Run one millisecond of emulated time.
    pub fn step_ms(&mut self) {
        if self.exited {
            return;
        }
        // Machines run side by side on one thread, which has one clock.
        set_clock(self.clock_ms + self.ms);
        let end = self.cpu.bus.clock.icount + self.cycles_per_ms;
        self.cpu.bus.start_batch(end);
        // A program that ends stops the batch early; the rest of the
        // millisecond runs the shell.
        while self.cpu.bus.clock.icount < end {
            match exec::run_batch(&mut self.cpu, &mut NoHook, false) {
                StopReason::Exit => {
                    self.exited = true;
                    break;
                }
                StopReason::BatchEnd => break,
                _ => self.cpu.bus.start_batch(end),
            }
        }
        // The display latches its Start Address at each retrace, as the
        // window's frames have it done.
        self.cpu.bus.sync_display();
        self.ms += 1;
        if self.ms.is_multiple_of(AUDIO_EVERY_MS) {
            self.mix();
        }
        if let Some((dir, every)) = self.snap.clone()
            && self.ms.is_multiple_of(every)
        {
            let path = dir.join(format!("snap-{:07}", self.ms));
            match self.screen_text() {
                Some(text) => drop(std::fs::write(path.with_extension("txt"), text)),
                None => drop(rust_dos::capture::png::save(&self.picture(), &path.with_extension("png"))),
            }
        }
    }

    /// Mix the sound up to now, keeping it while listening.
    fn mix(&mut self) {
        self.cpu.bus.audio_catch_up();
        let samples = self.cpu.bus.audio_out.drain(..);
        match &mut self.listening {
            Some(kept) => kept.extend(samples),
            None => drop(samples),
        }
    }

    /// Keep the sound from now on, interleaved stereo at 44.1 kHz.
    pub fn start_listening(&mut self) {
        self.mix();
        self.listening = Some(Vec::new());
    }

    /// The sound kept since `start_listening`.
    pub fn stop_listening(&mut self) -> Vec<i16> {
        self.mix();
        self.listening.take().unwrap_or_default()
    }

    pub fn key_down(&mut self, key: PcKey, ascii: u8) {
        keyboard::apply_key(&mut self.cpu.bus, key, ascii, true);
    }

    pub fn key_up(&mut self, key: PcKey) {
        keyboard::apply_key(&mut self.cpu.bus, key, 0, false);
    }

    /// The picture as the window would show it.
    pub fn picture(&mut self) -> Frame {
        self.cpu.bus.sync_display();
        let (width, height) = video::frame_size(&self.cpu.bus);
        let mut frame = Frame::new(width, height);
        self.cpu.bus.vga.mark_dirty_full();
        video::render_screen(&mut frame, &self.cpu.bus);
        frame
    }

    /// The text on a text-mode screen, a line per row; None in graphics
    /// modes.
    pub fn screen_text(&self) -> Option<String> {
        let g = video::text::geometry(&self.cpu.bus)?;
        let vram = self.cpu.bus.display_mem();
        let lines: Vec<String> = (0..g.rows)
            .map(|r| {
                let line: String = (0..g.cols)
                    .map(|c| video::CP437[vram[(g.start + r * g.row_bytes + c * 2) & g.wrap] as usize])
                    .collect();
                line.trim_end().to_string()
            })
            .collect();
        Some(lines.join("\n"))
    }

    /// The BIOS's video mode number (40:49h).
    pub fn bios_mode(&self) -> u8 {
        self.cpu.bus.peek_8(0x0449)
    }

    pub fn shell_idle(&self) -> bool {
        self.cpu.shell_idle()
    }
}

pub fn hash_frame(frame: &Frame) -> String {
    let mut h = Sha256::new();
    h.update(frame.width.to_le_bytes());
    h.update(frame.height.to_le_bytes());
    h.update(&frame.rgb);
    hex(&h.finalize()[..12])
}

pub fn hash_samples(samples: &[i16]) -> String {
    let mut h = Sha256::new();
    for s in samples {
        h.update(s.to_le_bytes());
    }
    hex(&h.finalize()[..12])
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// How many distinct colours the picture has.
pub fn colours(frame: &Frame) -> usize {
    let mut seen = std::collections::HashSet::new();
    for px in frame.rgb.as_chunks::<3>().0 {
        seen.insert((px[0], px[1], px[2]));
        if seen.len() > 64 {
            break;
        }
    }
    seen.len()
}

pub fn peak(samples: &[i16]) -> i32 {
    samples.iter().map(|s| (*s as i32).abs()).max().unwrap_or(0)
}

/// The machines a scenario plays on, run side by side a millisecond at a
/// time: one, or two linked by a network or a serial cable. Input and
/// what is looked at go to the one in focus.
pub struct Rig {
    pub machines: Vec<Machine>,
    pub focus: usize,
}

impl Rig {
    pub fn new(machines: Vec<Machine>) -> Rig {
        Rig { machines, focus: 0 }
    }

    /// The machine in focus.
    pub fn m(&mut self) -> &mut Machine {
        &mut self.machines[self.focus]
    }

    /// Emulated milliseconds since the start.
    pub fn ms(&self) -> u64 {
        self.machines[0].ms
    }

    /// Whether the machine in focus was turned off (EXIT).
    pub fn exited(&self) -> bool {
        self.machines[self.focus].exited
    }

    /// Run `ms` milliseconds of emulated time on every machine.
    pub fn run(&mut self, ms: u64) {
        for _ in 0..ms {
            for m in &mut self.machines {
                m.step_ms();
            }
        }
    }

    /// Press a key: down, held for `hold` ms, up, and `gap` ms to take it
    /// in.
    pub fn press(&mut self, key: PcKey, ascii: u8, hold: u64, gap: u64) {
        self.m().key_down(key, ascii);
        self.run(hold);
        self.m().key_up(key);
        self.run(gap);
    }

    /// Type text: each character's key, with Shift where it needs it.
    pub fn type_text(&mut self, text: &str) -> Result<(), String> {
        let shift = keyboard::lookup("shift").unwrap();
        for c in text.chars() {
            let (key, shifted) = keyboard::char_to_key(c).ok_or_else(|| format!("no key types {:?}", c))?;
            if shifted {
                self.m().key_down(shift, 0);
                self.run(30);
            }
            let ascii = if shifted { key.shifted } else { key.ascii };
            self.press(key, ascii, 30, 30);
            if shifted {
                self.m().key_up(shift);
                self.run(30);
            }
        }
        Ok(())
    }

    /// The sound of the next `ms` milliseconds on the machine in focus.
    pub fn listen(&mut self, ms: u64) -> Vec<i16> {
        self.m().start_listening();
        self.run(ms);
        self.m().stop_listening()
    }
}
