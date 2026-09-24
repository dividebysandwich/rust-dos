//! rust-dos in the browser: the emulator as a WebAssembly module, which the
//! page in `www/` runs one animation frame at a time the way the rust-dos
//! program's main loop runs its window. The page brings the keyboard, the
//! mouse, the sound and the drives: C: is a hard disk image held in
//! memory, which the page keeps in the browser's storage.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rust_dos::audio::{self, AudioOutput};
use rust_dos::config::{self, Filter, Settings};
use rust_dos::cpu::Cpu;
use rust_dos::disk::{self, DRIVE_C, DriveKind, MountOptions};
use rust_dos::diskimage::{self, DiskImage, MemoryImage};
use rust_dos::exec::{self, NoHook};
use rust_dos::keyboard::{self, MOD_ALT, MOD_CTRL, MOD_LSHIFT, MOD_RSHIFT, PcKey};
use rust_dos::timer::Pacer;
use rust_dos::video::{self, Frame};
use wasm_bindgen::prelude::*;
use web_time::{Duration, Instant};

/// C:'s image, as the drive list and downloads name it.
const C_IMAGE: &str = "C.IMG";
/// How often the text cursor blinks.
const BLINK: Duration = Duration::from_millis(500);
/// The Caps Lock key's scan code, and its bit at 40:17h.
const CAPS_LOCK_SCAN: u8 = 0x3A;
const CAPS_LOCK_ON: u8 = 0x40;
/// The key between left Shift and Z on 102-key keyboards, which
/// `keyboard::KEYS` doesn't have: \ and | on the US layout.
const INTL_BACKSLASH: PcKey = PcKey { scan: 0x56, ascii: b'\\', shifted: b'|', modifier: 0, extended: false };

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(line: &str);
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

/// The sound on its way to the page, which plays it with Web Audio.
#[derive(Default)]
struct Sound {
    /// Frames the page has waiting to be played.
    queued: usize,
    /// Samples (44.1 kHz stereo, interleaved) for the page to take.
    out: Vec<i16>,
}

/// The page's Web Audio output as the emulator's sound device.
struct PageAudio(Rc<RefCell<Sound>>);

impl AudioOutput for PageAudio {
    fn queued_frames(&self) -> usize {
        let sound = self.0.borrow();
        sound.queued + sound.out.len() / 2
    }

    fn queue(&mut self, samples: &[i16]) -> Result<(), String> {
        self.0.borrow_mut().out.extend_from_slice(samples);
        Ok(())
    }
}

/// The emulated PC, as the page sees it.
#[wasm_bindgen]
pub struct Machine {
    cpu: Cpu,
    settings: Settings,
    autoexec: Vec<String>,
    warnings: Vec<String>,
    pacer: Pacer,
    sound: Rc<RefCell<Sound>>,
    /// The video card's picture, rendered where it changed.
    picture: Frame,
    /// The picture with the cursors on top, as last shown and as it is
    /// being put together.
    screen: Frame,
    next: Frame,
    /// `screen` as the canvas takes it: RGBA.
    rgba: Vec<u8>,
    cursor_visible: bool,
    last_blink: Instant,
    /// Keys pressed on the machine and not released yet, by
    /// `KeyboardEvent.code`.
    held: HashMap<String, PcKey>,
    /// The disk image `begin_image` started, for `mount_image`.
    staged: Option<MemoryImage>,
}

#[wasm_bindgen]
impl Machine {
    /// A machine set up as the configuration file text `config` (in
    /// rust-dos.conf's format) has it. There are no host directories for
    /// `[drives]` to mount: the page mounts C: (`format_c`, `mount_image`)
    /// before `boot`.
    #[wasm_bindgen(constructor)]
    pub fn new(config: &str) -> Machine {
        let config = config::parse(config, Path::new("/"), None);
        let settings = Settings::from_config(&config);
        let mut warnings = config.warnings.clone();
        if !config.drives.is_empty() {
            warnings.push("[drives]: there are no host directories in the browser; add files to C: instead".into());
        }
        let mut cpu = Cpu::with_memory(PathBuf::from("/"), settings.memsize);
        cpu.model = settings.cpu;
        cpu.bus.set_disk_settings(settings.disk);
        warnings.extend(rust_dos::sound::apply_config(&mut cpu, &settings.sound, None));
        for warning in &warnings {
            cpu.bus.log_string(&format!("[CONFIG] Warning: {}", warning));
        }
        let sound = Rc::new(RefCell::new(Sound::default()));
        cpu.bus.audio_device = Some(Box::new(PageAudio(sound.clone())));
        cpu.bus.set_cycles_per_ms(settings.cycles.initial_cycles());
        let blank = Frame::new(video::SCREEN_WIDTH, video::SCREEN_HEIGHT);
        Machine {
            pacer: Pacer::new(settings.cycles, Instant::now()),
            cpu,
            settings,
            autoexec: config.autoexec,
            warnings,
            sound,
            picture: blank.clone(),
            screen: blank.clone(),
            next: blank,
            rgba: Vec::new(),
            cursor_visible: true,
            last_blink: Instant::now(),
            held: HashMap::new(),
            staged: None,
        }
    }

    /// Problems with the configuration.
    pub fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }

    /// Whether to stretch the picture to 4:3 (`aspect`).
    pub fn aspect(&self) -> bool {
        self.settings.aspect
    }

    /// Whether to scale the picture up smoothly (`filter=linear`).
    pub fn smooth(&self) -> bool {
        self.settings.filter == Filter::Linear
    }

    /// Send the emulator's log to the browser console.
    pub fn log_to_console(&mut self) {
        self.cpu.bus.log_hook = Some(Box::new(log));
    }

    /// Start DOS: the shell, with a box above its first prompt that has
    /// `notes` under the emulator's name, then the configuration's
    /// `[autoexec]` lines, C:\AUTOEXEC.BAT and the command lines
    /// `commands`.
    pub fn boot(&mut self, notes: Vec<String>, commands: Vec<String>) {
        self.cpu.load_shell();
        print_banner(&mut self.cpu, &notes);
        self.cpu.queue_batch_lines(&self.autoexec);
        self.cpu.queue_batch_file("C:\\AUTOEXEC.BAT");
        self.cpu.queue_batch_lines(&commands);
        self.pacer = Pacer::new(self.settings.cycles, Instant::now());
    }

    /// Type `line` at the DOS prompt, once the shell is back there.
    pub fn run_command(&mut self, line: &str) {
        self.cpu.queue_batch_lines([line]);
    }

    /// Run the machine up to the wall clock, then render its screen and
    /// hand over its sound (`take_sound`). `queued` is the sound the page
    /// has waiting to be played, in stereo frames at 44.1 kHz. Returns
    /// whether the screen changed.
    pub fn run_frame(&mut self, queued: u32) -> bool {
        let frame_start = Instant::now();
        self.sound.borrow_mut().queued = queued as usize;

        // Emulated time is counted in instructions (see timer.rs), so
        // however long the page's frames are, timer interrupts land on the
        // right instructions.
        let cpu = &mut self.cpu;
        let batch_start = Instant::now();
        let batch_end = if cpu.bus.exit_requested {
            cpu.bus.clock.icount
        } else {
            self.pacer.batch_end(&cpu.bus.clock, batch_start)
        };
        cpu.bus.start_batch(batch_end);
        let (icount, stalled, idle) = (cpu.bus.clock.icount, cpu.bus.clock.stalled, cpu.bus.clock.idle);
        exec::run_batch(cpu, &mut NoHook, false);
        let exec_time = batch_start.elapsed();
        let clock = &cpu.bus.clock;
        let executed = clock.icount - icount - (clock.idle - idle) - (clock.stalled - stalled);

        audio::pump_audio(&mut cpu.bus);
        cpu.bus.flush_log();
        let changed = self.render();

        let overhead = frame_start.elapsed().saturating_sub(exec_time);
        if let Some(cycles) = self.pacer.end_frame(&self.cpu.bus.clock, executed, exec_time, overhead) {
            self.cpu.bus.set_cycles_per_ms(cycles);
        }
        changed
    }

    /// The sound rendered since the last call, 44.1 kHz stereo: the left
    /// channel's samples, then the right's.
    pub fn take_sound(&mut self) -> Vec<f32> {
        let mut sound = self.sound.borrow_mut();
        let frames = sound.out.len() / 2;
        let mut planar = vec![0.0; frames * 2];
        for (i, [left, right]) in sound.out.as_chunks::<2>().0.iter().enumerate() {
            planar[i] = *left as f32 / 32768.0;
            planar[frames + i] = *right as f32 / 32768.0;
        }
        sound.out.clear();
        planar
    }

    /// The screen's size in pixels.
    pub fn screen_width(&self) -> u32 {
        self.screen.width
    }

    pub fn screen_height(&self) -> u32 {
        self.screen.height
    }

    /// Where the screen's pixels are in the module's memory, RGBA, as the
    /// canvas's `ImageData` takes them.
    pub fn screen_pixels(&self) -> *const u8 {
        self.rgba.as_ptr()
    }

    /// Emulated instructions per millisecond, as the speed setting or the
    /// host's speed makes them.
    pub fn cycles(&self) -> u32 {
        self.cpu.bus.clock.cycles_per_ms()
    }

    /// The drives read or written since the last call: bit n for drive n.
    pub fn take_drive_activity(&mut self) -> u32 {
        std::mem::take(&mut self.cpu.bus.drives_active)
    }

    /// Whether EXIT turned the machine off.
    pub fn exit_requested(&self) -> bool {
        self.cpu.bus.exit_requested
    }

    /// Whether DOSCONFIG asked for the settings since the last call.
    pub fn take_settings_request(&mut self) -> bool {
        std::mem::take(&mut self.cpu.bus.config_ui_requested)
    }

    // ------------------------------------------------------------------
    // Keyboard and mouse
    // ------------------------------------------------------------------

    /// A key went down on the page, as its `KeyboardEvent` has it: `code`
    /// is the key, `key` what it types with the keyboard layout, and
    /// `alt_graph` whether AltGr is held. Returns whether the machine took
    /// the key, which the page then keeps from the browser.
    pub fn key_down(&mut self, code: &str, key: &str, alt_graph: bool) -> bool {
        let bus = &mut self.cpu.bus;
        if code == "CapsLock" {
            if !self.held.contains_key(code) {
                keyboard::deliver_scan_only(bus, CAPS_LOCK_SCAN, false);
                let flags = bus.read_8(0x0417);
                bus.write_8(0x0417, flags ^ CAPS_LOCK_ON);
                let caps = PcKey { scan: CAPS_LOCK_SCAN, ascii: 0, shifted: 0, modifier: 0, extended: false };
                self.held.insert(code.to_string(), caps);
            }
            return true;
        }
        // AltGr is there to type characters with, not as Alt.
        if key == "AltGraph" {
            return false;
        }
        let Some(pc) = pc_key(code) else {
            return false;
        };
        if pc.modifier != 0 {
            if !self.held.contains_key(code) {
                keyboard::apply_key(bus, pc, 0, true);
                self.held.insert(code.to_string(), pc);
            }
            return true;
        }
        // Held keys repeat, as keyboards do.
        let flags = bus.read_8(0x0417);
        let ascii = typed_char(pc, code, key, flags);
        if alt_graph && ascii != 0 {
            // The character AltGr typed, not an Alt or Ctrl combination.
            bus.write_8(0x0417, flags & !(MOD_CTRL | MOD_ALT));
            keyboard::apply_key(bus, pc, ascii, true);
            bus.write_8(0x0417, flags);
        } else {
            keyboard::apply_key(bus, pc, ascii, true);
        }
        self.held.insert(code.to_string(), pc);
        true
    }

    /// A key went up on the page. Only keys the machine saw go down come
    /// up for it.
    pub fn key_up(&mut self, code: &str) -> bool {
        let Some(pc) = self.held.remove(code) else {
            return false;
        };
        if pc.scan == CAPS_LOCK_SCAN {
            keyboard::deliver_key_up(&mut self.cpu.bus, pc.scan, false);
        } else {
            keyboard::apply_key(&mut self.cpu.bus, pc, 0, false);
        }
        true
    }

    /// Let go of every key and mouse button, as the page loses the
    /// keyboard, so no game is left with Ctrl or a fire button held down.
    pub fn release_input(&mut self) {
        let codes: Vec<String> = self.held.keys().cloned().collect();
        for code in codes {
            self.key_up(&code);
        }
        let flags = self.cpu.bus.read_8(0x0417);
        self.cpu.bus.write_8(0x0417, flags & !(MOD_LSHIFT | MOD_RSHIFT | MOD_CTRL | MOD_ALT));
        for button in 0..3 {
            if self.cpu.bus.mouse.buttons & (1 << button) != 0 {
                self.cpu.bus.mouse.button_up(button);
            }
        }
    }

    /// Whether a program has the mouse driver in use, for the page to
    /// capture the mouse.
    pub fn mouse_installed(&self) -> bool {
        self.cpu.bus.mouse.installed
    }

    /// The mouse moved to (`x`, `y`) on the screen, in its pixels.
    pub fn mouse_move(&mut self, x: f64, y: f64) {
        let (vx, vy) = video::overlay::frame_to_mouse(&self.cpu.bus, &self.screen, (x as i32, y as i32));
        self.cpu.bus.mouse.set_position(vx, vy);
    }

    /// A mouse button (`MouseEvent.button`: 0 left, 1 middle, 2 right)
    /// went down or up.
    pub fn mouse_button(&mut self, button: u8, down: bool) {
        let index = match button {
            0 => 0,
            2 => 1,
            1 => 2,
            _ => return,
        };
        if down {
            self.cpu.bus.mouse.button_down(index);
        } else {
            self.cpu.bus.mouse.button_up(index);
        }
    }

    // ------------------------------------------------------------------
    // Drives
    // ------------------------------------------------------------------

    /// Make C: a new, empty hard disk of `megabytes` MB, in place of what
    /// was there.
    pub fn format_c(&mut self, megabytes: u32) -> Result<(), JsError> {
        let disk = DiskImage::blank_hard_disk(C_IMAGE, (megabytes as u64) << 20, Some(disk::DEFAULT_LABEL))
            .map_err(|e| JsError::new(&e))?;
        self.cpu.bus.mount_disk_image(DRIVE_C, disk, MountOptions::default()).map_err(|e| JsError::new(&e))
    }

    /// Start a disk or CD image of `size` bytes, zeros until `write_image`
    /// fills them in, for `mount_image`.
    pub fn begin_image(&mut self, size: f64) {
        self.staged = Some(MemoryImage::new(size as u64));
    }

    /// Fill in bytes of the image `begin_image` started, from `offset` on.
    pub fn write_image(&mut self, offset: f64, data: &[u8]) -> Result<(), JsError> {
        let image = self.staged.as_mut().ok_or_else(|| JsError::new("No image started"))?;
        match image.write_at(offset as u64, data) {
            true => Ok(()),
            false => Err(JsError::new("Past the end of the image")),
        }
    }

    /// Mount the image `begin_image` started as `drive` (0 is A:), in place
    /// of what is there. `name` is its file name, which with the contents
    /// tells what kind of image it is unless `kind` ("floppy", "hdd" or
    /// "cdrom") says.
    pub fn mount_image(&mut self, drive: u8, name: &str, kind: &str) -> Result<(), JsError> {
        let data = self.staged.take().ok_or_else(|| JsError::new("No image started"))?;
        let kind = match kind {
            "floppy" => DriveKind::Floppy,
            "cdrom" => DriveKind::CdRom,
            _ => DriveKind::HardDisk,
        };
        let opts = MountOptions { kind, ..MountOptions::default() };
        self.cpu.bus.mount_memory_image(drive, name, data, opts).map_err(|e| JsError::new(&e))
    }

    /// Take the disk out of `drive`.
    pub fn unmount(&mut self, drive: u8) -> Result<(), JsError> {
        self.cpu.bus.unmount_drive(drive).map_err(|e| JsError::new(&e))
    }

    /// The mounted drives, as JSON: for each its letter, type (as
    /// `DriveKind::name` has it), label, image file name, size in bytes and
    /// whether it is read-only.
    pub fn drives(&self) -> String {
        let disk = &self.cpu.bus.disk;
        let drives: Vec<_> = disk
            .mounted_drives()
            .into_iter()
            .map(|info| {
                let size = disk.bios_image(info.drive).map_or(0, |image| image.sectors() * diskimage::SECTOR_SIZE as u64);
                serde_json::json!({
                    "drive": info.drive,
                    "letter": info.letter().to_string(),
                    "kind": info.kind.name(),
                    "label": info.label,
                    "image": info.image.map(|path| path.display().to_string()),
                    "size": size,
                    "read_only": info.read_only,
                })
            })
            .collect();
        serde_json::Value::from(drives).to_string()
    }

    /// Whether an image of `bytes` bytes has the size of a floppy disk's,
    /// which is what makes it one.
    pub fn is_floppy_size(bytes: f64) -> bool {
        diskimage::floppy_geometry(bytes as u64).is_some()
    }

    /// Bytes in each piece of an image, as `written_chunks` and
    /// `image_chunk` count them.
    pub fn chunk_size() -> u32 {
        diskimage::CHUNK as u32
    }

    /// The pieces of `drive`'s image written since the last call, by
    /// number, for the page to save.
    pub fn written_chunks(&self, drive: u8) -> Vec<u32> {
        self.cpu.bus.disk.bios_image(drive).map_or_else(Vec::new, |image| {
            image.take_written().into_iter().map(|i| i as u32).collect()
        })
    }

    /// The size in bytes of `drive`'s image, if it is a disk image held in
    /// memory (else 0).
    pub fn image_size(&self, drive: u8) -> f64 {
        let image = self.cpu.bus.disk.bios_image(drive);
        image.and_then(|image| image.memory().map(|memory| memory.len() as f64)).unwrap_or(0.0)
    }

    /// The piece `index` of `drive`'s image, or nothing if it is zeros.
    pub fn image_chunk(&self, drive: u8, index: u32) -> Option<Vec<u8>> {
        let image = self.cpu.bus.disk.bios_image(drive)?;
        let memory = image.memory()?;
        memory.chunk(index as usize).map(<[u8]>::to_vec)
    }

    /// The DOS paths on C: of the files at the relative paths `paths`
    /// ("Keen 4/KEEN4E.EXE"): each name in a directory made an 8.3 name
    /// with the others there, as host directories' long names are (see
    /// `disk::short_names`), so the same files always get the same names.
    pub fn dos_paths(paths: Vec<String>) -> Vec<String> {
        let split = |path: &str| -> Vec<String> {
            path.split(['/', '\\']).filter(|p| !p.is_empty() && *p != "." && *p != "..").map(str::to_string).collect()
        };
        // Each directory's entries, by the directory's path.
        let mut dirs: BTreeMap<Vec<String>, Vec<String>> = BTreeMap::new();
        for path in &paths {
            let parts = split(path);
            for depth in 0..parts.len() {
                let entries = dirs.entry(parts[..depth].to_vec()).or_default();
                if !entries.contains(&parts[depth]) {
                    entries.push(parts[depth].clone());
                }
            }
        }
        let mut short: HashMap<(Vec<String>, String), String> = HashMap::new();
        for (dir, mut entries) in dirs {
            entries.sort_by_key(|name| name.to_ascii_uppercase());
            for (name, dos) in entries.iter().zip(disk::short_names(&entries)) {
                short.insert((dir.clone(), name.clone()), dos);
            }
        }
        paths
            .iter()
            .map(|path| {
                let parts = split(path);
                (0..parts.len())
                    .map(|depth| short[&(parts[..depth].to_vec(), parts[depth].clone())].as_str())
                    .collect::<Vec<_>>()
                    .join("\\")
            })
            .collect()
    }

    /// Put a file on C: at the DOS path `path` (see `dos_paths`), dated
    /// `time` and `date` as DOS dates files, making the directories on its
    /// way and replacing a file that is there.
    pub fn put_file(&mut self, path: &str, data: &[u8], time: u16, date: u16) -> Result<(), JsError> {
        let volume = self.cpu.bus.disk.fat_volume(DRIVE_C).ok_or_else(|| JsError::new("C: is not a disk image"))?;
        let parts: Vec<&str> = path.split('\\').filter(|p| !p.is_empty()).collect();
        volume.put_file(&parts, data, time, date).map_err(|code| {
            JsError::new(&match code {
                0x27 => format!("C: is full: no room for {}", path),
                0x03 => format!("C:\\{}: not a name DOS takes", path),
                _ => format!("C:\\{}: can't be written (DOS error {:02X}h)", path, code),
            })
        })
    }
}

impl Machine {
    /// Render the video card's picture where it changed and put the
    /// cursors on top. Returns whether the screen changed.
    fn render(&mut self) -> bool {
        if self.last_blink.elapsed() >= BLINK {
            self.cursor_visible = !self.cursor_visible;
            self.last_blink = Instant::now();
        }
        let bus = &mut self.cpu.bus;
        // The CRTC picks up the Start Address the program flipped to at the
        // vertical retraces that passed.
        bus.sync_display();
        let (width, height) = video::frame_size(bus);
        if self.picture.resize(width, height) {
            bus.vga.mark_dirty_full();
        }
        if bus.vga.dirty {
            video::render_screen(&mut self.picture, bus);
            bus.vga.clear_dirty();
        }
        self.next.clone_from(&self.picture);
        video::overlay::draw_cursors(&mut self.next, bus, self.cursor_visible);
        let same = (self.next.width, self.next.height) == (self.screen.width, self.screen.height)
            && self.next.rgb == self.screen.rgb;
        if same && !self.rgba.is_empty() {
            return false;
        }
        std::mem::swap(&mut self.next, &mut self.screen);
        self.rgba.clear();
        self.rgba.extend(self.screen.rgb.as_chunks::<3>().0.iter().flat_map(|&[r, g, b]| [r, g, b, 0xFF]));
        true
    }
}

/// The key of `keyboard::KEYS` that a `KeyboardEvent.code` names.
fn pc_key(code: &str) -> Option<PcKey> {
    let name = match code {
        "Minus" => "minus",
        "Equal" => "equals",
        "BracketLeft" => "leftbracket",
        "BracketRight" => "rightbracket",
        "Backslash" => "backslash",
        "IntlBackslash" => return Some(INTL_BACKSLASH),
        "Semicolon" => "semicolon",
        "Quote" => "quote",
        "Comma" => "comma",
        "Period" => "period",
        "Slash" => "slash",
        "Backquote" => "backquote",
        "Space" => "space",
        "Enter" => "enter",
        "Backspace" => "backspace",
        "Tab" => "tab",
        "Escape" => "escape",
        "ArrowUp" => "up",
        "ArrowDown" => "down",
        "ArrowLeft" => "left",
        "ArrowRight" => "right",
        "Home" => "home",
        "End" => "end",
        "PageUp" => "pageup",
        "PageDown" => "pagedown",
        "Insert" => "insert",
        "Delete" => "delete",
        "NumpadDecimal" => "kpperiod",
        "NumpadAdd" => "kpplus",
        "NumpadSubtract" => "kpminus",
        "NumpadMultiply" => "kpmultiply",
        "NumpadDivide" => "kpdivide",
        "NumpadEnter" => "kpenter",
        "ShiftLeft" => "lshift",
        "ShiftRight" => "rshift",
        "ControlLeft" => "lctrl",
        "ControlRight" => "rctrl",
        "AltLeft" => "lalt",
        "AltRight" => "ralt",
        // KeyA-KeyZ, Digit0-Digit9, Numpad0-Numpad9 and F1-F12.
        _ => {
            let name = match code.strip_prefix("Key").or_else(|| code.strip_prefix("Digit")) {
                Some(name) => name.to_string(),
                None => match code.strip_prefix("Numpad") {
                    Some(digit) => format!("kp{}", digit),
                    None => code.to_string(),
                },
            };
            return keyboard::lookup(&name);
        }
    };
    keyboard::lookup(name)
}

/// The character a key types: what the keyboard layout makes of it
/// (`key`), if code page 437 has it, or else the US layout's.
fn typed_char(pc: PcKey, code: &str, key: &str, flags: u8) -> u8 {
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => {
            if let Some(byte) = cp437(c) {
                return byte;
            }
        }
        // The keypad with Num Lock off: cursor keys, which type nothing.
        _ if code.starts_with("Numpad") && code != "NumpadEnter" => return 0,
        _ => {}
    }
    if flags & (MOD_LSHIFT | MOD_RSHIFT) != 0 { pc.shifted } else { pc.ascii }
}

/// The code page 437 character that `c` is, if there is one: the one the
/// VGA font draws for it.
fn cp437(c: char) -> Option<u8> {
    if (' '..='~').contains(&c) {
        return Some(c as u8);
    }
    video::CP437[0x80..].iter().position(|&x| x == c).map(|i| (0x80 + i) as u8)
}

/// The box above the first DOS prompt: the emulator and its version, then
/// `notes`, then the way to the settings.
fn print_banner(cpu: &mut Cpu, notes: &[String]) {
    // Bright cyan, white and yellow on blue, as the rust-dos program has it.
    const FRAME: u8 = 0x1B;
    const TEXT: u8 = 0x1F;
    const HIGHLIGHT: u8 = 0x1E;
    // The widest text inside the frame: a line of 80 would wrap.
    const MAX_WIDTH: usize = 74;

    let title = format!("Rust-DOS v{}", env!("CARGO_PKG_VERSION"));
    let description = " - An x86 DOS emulator written in Rust";
    let mut lines: Vec<Vec<(String, u8)>> = vec![vec![(title, HIGHLIGHT), (description.to_string(), TEXT)], vec![]];
    lines.extend(notes.iter().map(|note| vec![(note.chars().take(MAX_WIDTH).collect(), TEXT)]));
    lines.push(vec![
        ("Press ".to_string(), TEXT),
        ("Ctrl+F12".to_string(), HIGHLIGHT),
        (" or type ".to_string(), TEXT),
        ("DOSCONFIG".to_string(), HIGHLIGHT),
        (" to open the settings.".to_string(), TEXT),
    ]);
    let len = |line: &[(String, u8)]| line.iter().map(|(text, _)| text.chars().count()).sum::<usize>();
    let width = lines.iter().map(|line| len(line)).max().unwrap_or(0);

    fn put(cpu: &mut Cpu, text: &str, attr: u8) {
        let cells: Vec<u8> = text.chars().map(|c| cp437(c).unwrap_or(b'?')).collect();
        video::print_cp437(cpu, &cells, attr);
    }
    put(cpu, &format!("╔{}╗", "═".repeat(width + 2)), FRAME);
    video::print_string(cpu, "\r\n");
    for line in &lines {
        put(cpu, "║ ", FRAME);
        for (text, attr) in line {
            put(cpu, text, *attr);
        }
        put(cpu, &" ".repeat(width - len(line)), TEXT);
        put(cpu, " ║", FRAME);
        video::print_string(cpu, "\r\n");
    }
    put(cpu, &format!("╚{}╝", "═".repeat(width + 2)), FRAME);
    video::print_string(cpu, "\r\n\r\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_by_code() {
        assert_eq!(pc_key("KeyA").map(|k| k.scan), Some(0x1E));
        assert_eq!(pc_key("Digit1").map(|k| k.scan), Some(0x02));
        assert_eq!(pc_key("Numpad7").map(|k| (k.scan, k.extended)), Some((0x47, false)));
        assert_eq!(pc_key("F12").map(|k| k.scan), Some(0x86));
        assert_eq!(pc_key("ArrowLeft").map(|k| (k.scan, k.extended)), Some((0x4B, true)));
        assert_eq!(pc_key("ControlRight").map(|k| (k.modifier, k.extended)), Some((MOD_CTRL, true)));
        assert!(pc_key("MetaLeft").is_none() && pc_key("Fn").is_none());

        let a = pc_key("KeyA").unwrap();
        assert_eq!(typed_char(a, "KeyA", "a", 0), b'a');
        assert_eq!(typed_char(a, "KeyA", "A", MOD_LSHIFT), b'A');
        // A German layout's ö, and a dead key, which types nothing yet.
        let semicolon = pc_key("Semicolon").unwrap();
        assert_eq!(typed_char(semicolon, "Semicolon", "ö", 0), 0x94);
        assert_eq!(typed_char(pc_key("Space").unwrap(), "Space", " ", 0), b' ');
        assert_eq!((cp437('═'), cp437('~'), cp437('€')), (Some(0xCD), Some(b'~'), None));
        assert_eq!(typed_char(semicolon, "Semicolon", "Dead", MOD_LSHIFT), b':');
        let kp8 = pc_key("Numpad8").unwrap();
        assert_eq!(typed_char(kp8, "Numpad8", "8", 0), b'8');
        assert_eq!(typed_char(kp8, "Numpad8", "ArrowUp", 0), 0);
    }

    #[test]
    fn long_names_become_dos_names() {
        let paths = ["Keen 4/KEEN4E.EXE", "Keen 4/Readme first.txt", "Keen 4/Readme second.txt", "autoexec.bat"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            Machine::dos_paths(paths),
            ["KEEN4~1\\KEEN4E.EXE", "KEEN4~1\\README~1.TXT", "KEEN4~1\\README~2.TXT", "AUTOEXEC.BAT"]
        );
    }
}
