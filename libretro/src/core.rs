//! The machine as the frontend runs it: one frame of emulated time for
//! each `retro_run`, its picture and sound handed over, and the
//! frontend's keyboard, mouse and gamepads fed in.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rust_dos::audio;
use rust_dos::config_ui::{ConfigUi, Frontend, UiKey};
use rust_dos::cpu::Cpu;
use rust_dos::disk::{DRIVE_C, DriveKind, MountOptions};
use rust_dos::exec::{self, NoHook};
use rust_dos::hardware::{self, Hardware};
use rust_dos::joystick::{PAD_A, PAD_B, PAD_DOWN, PAD_LEFT, PAD_RIGHT, PAD_UP, PAD_X, PAD_Y, PadState};
use rust_dos::keyboard;
use rust_dos::keylayout::Layout;
use rust_dos::mount::MountSpec;
use rust_dos::stats::{FrameTimes, Stats};
use rust_dos::timer::{PIT_HZ, Pacer};
use rust_dos::video::{self, Frame};

use crate::content::{self, Base, Dirs};
use crate::ffi::*;
use crate::host::{self, Machine};
use crate::keys;
use crate::memmap::Published;
use crate::options::{self, Values};
use crate::{Callbacks, KeyEvent};

/// The frames the core hands over a second.
pub const FPS: f64 = 60.0;
/// The picture's largest size: the S3's 1600x1200.
pub const MAX_SIZE: (u32, u32) = (1600, 1200);
/// The settings window: no window of its own to scale, but the host's
/// files to mount.
const LIBRETRO: Frontend = Frontend { window: false, host_files: true };
/// Frames the text cursor shows, then doesn't.
const BLINK_FRAMES: u64 = 30;

/// The devices the gamepad ports can have.
pub const DEVICE_GAMEPORT: u32 = RETRO_DEVICE_JOYPAD;
pub const DEVICE_KEYS: u32 = retro_device_subclass(RETRO_DEVICE_JOYPAD, 0);

/// The keys a gamepad as keyboard presses, by button: the cursor keys,
/// Ctrl, Alt, Space and Shift (fire and jump in many games), Enter, Esc,
/// Page Up and Down, Tab and Backspace.
const PAD_KEYS: [(u32, u8, bool); 14] = [
    (RETRO_DEVICE_ID_JOYPAD_UP, 0x48, true),
    (RETRO_DEVICE_ID_JOYPAD_DOWN, 0x50, true),
    (RETRO_DEVICE_ID_JOYPAD_LEFT, 0x4B, true),
    (RETRO_DEVICE_ID_JOYPAD_RIGHT, 0x4D, true),
    (RETRO_DEVICE_ID_JOYPAD_B, 0x1D, false),
    (RETRO_DEVICE_ID_JOYPAD_A, 0x38, false),
    (RETRO_DEVICE_ID_JOYPAD_Y, 0x39, false),
    (RETRO_DEVICE_ID_JOYPAD_X, 0x2A, false),
    (RETRO_DEVICE_ID_JOYPAD_START, 0x1C, false),
    (RETRO_DEVICE_ID_JOYPAD_SELECT, 0x01, false),
    (RETRO_DEVICE_ID_JOYPAD_L, 0x49, true),
    (RETRO_DEVICE_ID_JOYPAD_R, 0x51, true),
    (RETRO_DEVICE_ID_JOYPAD_L2, 0x0F, false),
    (RETRO_DEVICE_ID_JOYPAD_R2, 0x0E, false),
];

/// Where the frontend's disk control is: the drive whose disks it changes,
/// and the disk it picked while the drive was open.
#[derive(Debug, Default)]
pub struct DiskControl {
    pub drive: Option<u8>,
    pub ejected: bool,
    pub pending: Option<usize>,
    /// Places `add_image_index` made for images yet to come.
    pub placeholders: usize,
}

pub struct Core {
    pub ui: ConfigUi,
    pub m: Machine,
    /// The content, and the options it was started with, for a reset.
    pub content: Option<PathBuf>,
    pub disk: DiskControl,
    pub memmap: Published,
    /// The save states' buffer size, once asked for.
    pub state_size: usize,
    stats: Stats,
    last_frame: Option<(Instant, FrameTimes)>,
    /// The time the frame spent in the frontend, handing over its sound
    /// and picture: with audio sync or vsync, it waits there.
    frontend: std::time::Duration,
    /// The video card's picture, rendered where it changed, and with the
    /// cursors and the settings window on top, as handed over.
    picture: Frame,
    screen: Frame,
    xrgb: Vec<u32>,
    /// The size and aspect the frontend was told, and the largest size.
    geometry: Option<retro_game_geometry>,
    max_size: (u32, u32),
    /// Emulated time at the end of the frame running, in PIT ticks, and
    /// the sixtieths of a tick left over.
    target_ticks: u64,
    tick_rem: u64,
    frames: u64,
    cursor_visible: bool,
    /// Keys the machine has down, by the frontend's key code.
    held: HashMap<u32, (u8, bool)>,
    /// The devices in the gamepad ports, their buttons last frame, and
    /// the keys gamepads as keyboards hold.
    pub ports: [u32; 2],
    pads: [u16; 2],
    pad_keys: [u32; 2],
    mouse_buttons: u8,
    pointer_down: bool,
    exit_sent: bool,
}

/// A directory the frontend names, if it does.
fn directory(cb: &Callbacks, cmd: u32) -> Option<PathBuf> {
    let mut dir: *const std::ffi::c_char = std::ptr::null();
    // SAFETY: the directory calls take a `const char **`.
    let ok = unsafe { cb.env(cmd, &mut dir as *mut _ as *mut std::ffi::c_void) };
    (ok && !dir.is_null()).then(|| {
        let dir = unsafe { std::ffi::CStr::from_ptr(dir) };
        PathBuf::from(dir.to_string_lossy().into_owned())
    })
}

impl Core {
    /// The machine set up for `content` (None: the prompt), with the core
    /// options `options`, the disk `initial_image` of a list in its drive.
    pub fn new(
        content: Option<PathBuf>,
        cb: &Callbacks,
        options: Values,
        initial_image: Option<(usize, PathBuf)>,
    ) -> Result<Box<Core>, String> {
        let fallback = content.as_deref().and_then(Path::parent).map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let system = directory(cb, RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY).unwrap_or_else(|| fallback.clone());
        let save = directory(cb, RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY).unwrap_or_else(|| system.clone());
        let dirs = Dirs::new(system, save);
        if let Err(e) = fs::create_dir_all(dirs.drive_c()) {
            cb.log(RETRO_LOG_WARN, &format!("{}: {}", dirs.drive_c().display(), e));
        }
        let base = Base::load(&dirs);
        let plan = content::plan(content.as_deref(), &dirs, options.boot())?;
        let (settings, mut warnings) = content::settings(&base, &options, plan.overlay.as_ref());
        let home = content::home();
        let memsize = match &plan.profile {
            Some(p) => rust_dos::games::prepare(&p.id, &settings, &p.text, &p.dir, home.as_deref())?.settings.memsize,
            None => settings.memsize,
        };

        let mut cpu = Box::new(Cpu::with_memory(plan.c_root.clone(), memsize));
        let log = *cb;
        cpu.bus.log_hook = Some(Box::new(move |line: &str| {
            let level = if line.contains("Warning") { RETRO_LOG_WARN } else { RETRO_LOG_DEBUG };
            log.log(level, line);
        }));
        warnings.extend(hardware::configure(&mut cpu, &settings, Layout::us()));

        // rust-dos.conf's drives, those of the configuration beside the
        // content, then the content's own.
        let content_c = plan.c_root != dirs.drive_c() || plan.mounts.iter().any(|s| s.drive == DRIVE_C);
        let mut mounts: Vec<(MountSpec, bool)> = base
            .config()
            .drives
            .into_iter()
            .filter(|s| !(s.drive == DRIVE_C && content_c))
            .map(|s| {
                let replace = s.drive == DRIVE_C;
                (s, replace)
            })
            .collect();
        if let Some(overlay) = &plan.overlay {
            mounts.extend(content::overlay_drives(overlay).into_iter().map(|s| (s, true)));
        }
        mounts.extend(plan.mounts.iter().cloned().map(|s| (s, true)));
        for (spec, replace) in mounts {
            if let Err(e) = cpu.bus.mount_drive(spec.drive, &spec.path, spec.opts.clone(), replace) {
                warnings.push(format!("drive {}: {}: {}", rust_dos::disk::drive_key(spec.drive), spec.path.display(), e));
            }
        }
        cpu.bus.config_dir = Some(base.dir().to_path_buf());
        for note in &plan.notes {
            cpu.bus.log_string(&format!("[CONFIG] {}", note));
        }
        for warning in &warnings {
            cpu.bus.log_string(&format!("[CONFIG] Warning: {}", warning));
        }
        cpu.bus.set_cycles_per_ms(settings.cycles.initial_cycles());

        let saved_drives = host::mounted_drives(&cpu);
        let blank = Frame::new(video::SCREEN_WIDTH, video::SCREEN_HEIGHT);
        let mut core = Box::new(Core {
            ui: ConfigUi::for_frontend(LIBRETRO),
            m: Machine {
                pacer: Pacer::new(settings.cycles, Instant::now()),
                hardware: Hardware::of(&settings),
                saved: settings.clone(),
                settings,
                cpu,
                dirs,
                base,
                options,
                overlay: plan.overlay.clone(),
                profile: plan.profile.clone(),
                game: None,
                saved_drives,
                notices: Vec::new(),
            },
            content,
            disk: DiskControl { drive: plan.disk_drive, ..DiskControl::default() },
            memmap: Published::default(),
            state_size: 0,
            stats: Stats::new(),
            last_frame: None,
            frontend: std::time::Duration::ZERO,
            picture: blank.clone(),
            screen: blank,
            xrgb: Vec::new(),
            geometry: None,
            max_size: MAX_SIZE,
            target_ticks: 0,
            tick_rem: 0,
            frames: 0,
            cursor_visible: true,
            held: HashMap::new(),
            ports: [DEVICE_GAMEPORT; 2],
            pads: [0; 2],
            pad_keys: [0; 2],
            mouse_buttons: 0,
            pointer_down: false,
            exit_sent: false,
        });
        core.boot(&plan);
        if let (Some(drive), Some((index, path))) = (core.disk.drive, initial_image) {
            let matches = core.m.cpu.bus.disk.images(drive).and_then(|(list, _)| list.get(index).cloned());
            if matches.is_some_and(|p| fs::canonicalize(&path).ok() == Some(p)) {
                let _ = core.m.cpu.bus.select_image(drive, index);
            }
        }
        Ok(core)
    }

    /// Start DOS: the shell, the box with what the machine has, then
    /// rust-dos.conf's `[autoexec]`, C:\AUTOEXEC.BAT and what the content
    /// asks for.
    fn boot(&mut self, plan: &content::Plan) {
        let m = &mut self.m;
        m.cpu.load_shell();
        let content = match &self.content {
            Some(path) => path.file_name().map_or(path.display().to_string(), |n| n.to_string_lossy().into_owned()),
            None => "none".to_string(),
        };
        let mut notes = vec![format!("Content: {}", content)];
        if m.base.file.is_file() {
            notes.push(format!("Config file: {}", rust_dos::mount::display_host_path(&m.base.file)));
        }
        video::print_banner(&mut m.cpu, &notes);
        m.cpu.queue_batch_lines(&m.base.config().autoexec);
        m.cpu.queue_batch_file("C:\\AUTOEXEC.BAT");
        match &plan.profile {
            Some(profile) => {
                if let Err(e) = m.start_game(&profile.id, &profile.text, &profile.dir) {
                    m.warn(&e);
                    m.notices.push(e);
                }
            }
            None => m.cpu.queue_batch_lines(&plan.commands),
        }
        self.target_ticks = m.cpu.bus.clock.now_ticks();
    }

    /// Run one frame: the input, a sixtieth of a second of emulated time,
    /// then the sound and the picture for the frontend.
    pub fn run_frame(&mut self, cb: &Callbacks) {
        let frame_start = Instant::now();
        if let Some((start, times)) = self.last_frame {
            self.stats.record(&self.m.cpu.bus, FrameTimes { wall: frame_start - start, ..times });
        }
        if options::updated(cb.env) {
            let values = options::read(cb.env);
            if values != self.m.options {
                self.m.options_changed(values);
            }
        }
        self.poll_input(cb);

        // Emulated time is counted in instructions (see timer.rs); each
        // frame runs to the next sixtieth of a second. The machine waits
        // while the settings window is open, but for its Mixer page.
        let waiting = self.ui.pauses_machine();
        let m = &mut self.m;
        m.cpu.bus.apply_freezes();
        let batch_end = if m.cpu.bus.exit_requested || waiting {
            self.target_ticks = m.cpu.bus.clock.now_ticks();
            self.tick_rem = 0;
            m.cpu.bus.clock.icount
        } else {
            self.tick_rem += PIT_HZ;
            self.target_ticks += self.tick_rem / FPS as u64;
            self.tick_rem %= FPS as u64;
            m.cpu.bus.clock.icount_at(self.target_ticks)
        };
        m.cpu.bus.start_batch(batch_end);
        let clock = &m.cpu.bus.clock;
        let (icount, stalled, idle) = (clock.icount, clock.stalled, clock.idle);
        let batch_start = Instant::now();
        exec::run_batch(&mut m.cpu, &mut NoHook, false);
        let exec_time = batch_start.elapsed();
        let clock = &m.cpu.bus.clock;
        let halted = clock.idle - idle;
        let executed = clock.icount - icount - halted - (clock.stalled - stalled);

        self.after_batch(cb);

        // The sound of the frame; silence while the machine waits, for the
        // frontend to go on at its pace.
        let m = &mut self.m;
        let mut samples = audio::pump_audio(&mut m.cpu.bus, waiting);
        if waiting {
            samples = vec![0; (rust_dos::opl::RATE as f64 / FPS) as usize * 2];
        } else if m.cpu.bus.mixer.muted {
            samples.fill(0);
        }
        let audio_start = Instant::now();
        cb.audio(&samples);
        let audio_wait = audio_start.elapsed();
        self.frontend = audio_wait;

        m.cpu.bus.flush_log();
        let render_start = Instant::now();
        self.render(cb);
        let render = render_start.elapsed();
        let m = &mut self.m;
        self.memmap.refresh(&mut m.cpu.bus, cb.env);
        for notice in m.notices.drain(..) {
            cb.message(&notice);
        }

        // The frontend's waits are neither the machine's work nor time
        // taken from it: the frame's work is what's left.
        let busy = frame_start.elapsed().saturating_sub(self.frontend);
        let overhead = busy.saturating_sub(exec_time);
        if let Some(cycles) = m.pacer.end_frame(&m.cpu.bus.clock, executed, exec_time, overhead) {
            m.cpu.bus.set_cycles_per_ms(cycles);
        }
        let render = render.saturating_sub(self.frontend - audio_wait);
        let times = FrameTimes { wall: busy, busy, render, executed, halted, ..FrameTimes::default() };
        self.last_frame = Some((frame_start, times));
    }

    /// What the machine asked for while it ran: the settings window, a
    /// changed mixer, a game ended, changed hardware, and turning off.
    fn after_batch(&mut self, cb: &Callbacks) {
        let m = &mut self.m;
        // DOSCONFIG asks for the settings window.
        if std::mem::take(&mut m.cpu.bus.config_ui_requested) && !self.ui.is_open() {
            self.toggle_settings();
        }
        let m = &mut self.m;
        // MIXER changed the mixer: the settings have it, to show and save.
        if std::mem::take(&mut m.cpu.bus.mixer_changed) {
            m.settings.mixer = m.cpu.bus.mixer.settings();
            self.ui.sync_mixer(m.settings.mixer);
        }
        // A launched game that has ended: the settings from before it come
        // back.
        if !self.ui.is_open()
            && let Some(ended) = m.game.take_if(|g| g.done(&m.cpu))
        {
            let name = ended.name.clone();
            m.end_game(ended);
            m.notices.push(format!("{} has ended", name));
        }
        if let Some(notice) = self.ui.take_notice() {
            m.notices.push(notice);
        }
        // Processor and sound changes wait for the running program to end.
        if !self.ui.is_open() && m.cpu.shell_idle() && m.hardware.differs(&m.settings) {
            let settings = m.settings.clone();
            for warning in m.hardware.apply(&mut m.cpu, &settings) {
                m.warn(&warning);
            }
        }
        if self.ui.is_open() {
            self.ui.poll(&mut self.m);
        }
        // EXIT at the prompt turns the machine off, and the frontend closes
        // the content.
        if self.m.cpu.bus.exit_requested && !self.exit_sent {
            self.exit_sent = true;
            // SAFETY: SHUTDOWN takes no data.
            unsafe { cb.env(RETRO_ENVIRONMENT_SHUTDOWN, std::ptr::null_mut()) };
        }
    }

    /// Render the video card's picture where it changed, put the cursors
    /// and the settings window on top, and hand it to the frontend.
    fn render(&mut self, cb: &Callbacks) {
        self.frames += 1;
        let bus = &mut self.m.cpu.bus;
        if self.frames.is_multiple_of(BLINK_FRAMES) {
            self.cursor_visible = !self.cursor_visible;
            // Blinking characters keep the cursor's time.
            bus.vga.set_blink(self.cursor_visible);
        }
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
        self.screen.clone_from(&self.picture);
        video::overlay::draw_cursors(&mut self.screen, bus, self.cursor_visible);
        video::mono::apply(&mut self.screen, self.m.settings.monochrome);
        if self.ui.is_open() {
            self.ui.set_mixer_status(bus.mixer.muted, bus.mixer.take_peaks());
        }
        if self.ui.is_open() || self.ui.overlay_shown() {
            self.ui.set_stats(self.stats.view());
        }
        self.ui.draw(&mut self.screen);
        self.ui.draw_overlay(&mut self.screen);

        self.xrgb.clear();
        self.xrgb.extend(
            self.screen.rgb.as_chunks::<3>().0.iter().map(|&[r, g, b]| u32::from_be_bytes([0, r, g, b])),
        );
        self.set_geometry(cb);
        let (w, h) = (self.screen.width, self.screen.height);
        let video_start = Instant::now();
        cb.video(&self.xrgb, w, h);
        self.frontend += video_start.elapsed();
    }

    /// Tell the frontend the picture's size and shape when they change.
    fn set_geometry(&mut self, cb: &Callbacks) {
        let geometry = self.geometry_now();
        if self.geometry == Some(geometry) {
            return;
        }
        self.geometry = Some(geometry);
        if geometry.base_width > self.max_size.0 || geometry.base_height > self.max_size.1 {
            self.max_size = (self.max_size.0.max(geometry.base_width), self.max_size.1.max(geometry.base_height));
            let geometry = self.geometry_now();
            let mut av = retro_system_av_info {
                geometry,
                timing: retro_system_timing { fps: FPS, sample_rate: rust_dos::opl::RATE as f64 },
            };
            // SAFETY: SET_SYSTEM_AV_INFO takes a retro_system_av_info.
            unsafe { cb.env(crate::ffi::RETRO_ENVIRONMENT_SET_SYSTEM_AV_INFO, &mut av as *mut _ as *mut std::ffi::c_void) };
            return;
        }
        let mut geometry = geometry;
        // SAFETY: SET_GEOMETRY takes a retro_game_geometry.
        unsafe { cb.env(RETRO_ENVIRONMENT_SET_GEOMETRY, &mut geometry as *mut _ as *mut std::ffi::c_void) };
    }

    /// The picture's size and shape now: 4:3 as a CRT shows it with
    /// `aspect`, else square pixels.
    pub fn geometry_now(&self) -> retro_game_geometry {
        let (w, h) = (self.screen.width.max(1), self.screen.height.max(1));
        let aspect_ratio = if self.m.settings.aspect { 4.0 / 3.0 } else { w as f32 / h as f32 };
        retro_game_geometry { base_width: w, base_height: h, max_width: self.max_size.0, max_height: self.max_size.1, aspect_ratio }
    }

    /// Emulated time goes on from where it is: after a state was loaded.
    pub fn rebase(&mut self) {
        self.target_ticks = self.m.cpu.bus.clock.now_ticks();
        self.tick_rem = 0;
        self.m.pacer.rebase(&self.m.cpu.bus.clock, Instant::now());
        self.m.cpu.bus.vga.mark_dirty_full();
        self.memmap.forget();
        self.release_input();
    }

    // ------------------------------------------------------------------
    // The settings window
    // ------------------------------------------------------------------

    /// Open or close the settings window (Ctrl+F12, DOSCONFIG, L3+R3).
    pub fn toggle_settings(&mut self) {
        if self.ui.is_open() {
            self.ui.close();
        } else {
            // The keys held stay held for nobody.
            self.release_input();
            let current = self.m.settings.clone();
            let file = self.m.config_path();
            self.ui.open(&current, Some(file), &self.m);
        }
    }

    /// Put the next disk in the drives mounted from lists of them.
    pub fn swap_images(&mut self) {
        let messages = self.m.cpu.bus.swap_images();
        if messages.is_empty() {
            self.m.notices.push("No drive has more than one disk".to_string());
        }
        self.m.notices.extend(messages);
    }

    // ------------------------------------------------------------------
    // Input
    // ------------------------------------------------------------------

    fn poll_input(&mut self, cb: &Callbacks) {
        cb.input_poll();
        for event in crate::take_keys() {
            self.key(event);
        }
        self.pads(cb);
        self.mouse(cb);
    }

    /// A key went down or up on the frontend's keyboard.
    pub fn key(&mut self, e: KeyEvent) {
        let ctrl = e.modifiers & RETROKMOD_CTRL != 0;
        let shift = e.modifiers & RETROKMOD_SHIFT != 0;
        if e.down && ctrl && e.keycode == key::F12 {
            if shift {
                if let Some(message) = self.ui.overlay_key() {
                    self.m.notices.push(message.to_string());
                }
            } else {
                self.toggle_settings();
            }
            return;
        }
        if e.down && ctrl && e.keycode == key::F4 {
            self.swap_images();
            return;
        }
        if self.ui.is_open() {
            let character = char::from_u32(e.character).filter(|_| e.character != 0);
            if e.down
                && let Some(key) = keys::ui_key(e.keycode, character, ctrl, shift)
            {
                self.ui.key(key, &mut self.m);
            }
            return;
        }
        let bus = &mut self.m.cpu.bus;
        if e.down {
            if let Some((scan, extended)) = keys::pc_scan(e.keycode) {
                keyboard::key_event(bus, scan, extended, true, None);
                self.held.insert(e.keycode, (scan, extended));
            }
        } else if let Some((scan, extended)) = self.held.remove(&e.keycode) {
            keyboard::key_event(bus, scan, extended, false, None);
        }
    }

    /// Let go of every key and button, so no game is left with Ctrl or a
    /// fire button held down.
    pub fn release_input(&mut self) {
        let bus = &mut self.m.cpu.bus;
        for (_, (scan, extended)) in self.held.drain() {
            keyboard::key_event(bus, scan, extended, false, None);
        }
        for port in 0..2 {
            for (i, &(_, scan, extended)) in PAD_KEYS.iter().enumerate() {
                if self.pad_keys[port] & (1 << i) != 0 {
                    keyboard::key_event(bus, scan, extended, false, None);
                }
            }
            self.pad_keys[port] = 0;
        }
        for button in 0..3 {
            if bus.mouse.buttons & (1 << button) != 0 {
                bus.mouse.button_up(button);
            }
        }
        self.mouse_buttons = 0;
    }

    /// The gamepads: the game port's joysticks, or keys, and the settings
    /// window's keys while it is open.
    fn pads(&mut self, cb: &Callbacks) {
        const L3_R3: u16 = 1 << RETRO_DEVICE_ID_JOYPAD_L3 | 1 << RETRO_DEVICE_ID_JOYPAD_R3;
        for port in 0..2 {
            let device = self.ports[port];
            let buttons = (0..16).filter(|&id| cb.input(port as u32, RETRO_DEVICE_JOYPAD, 0, id) != 0).fold(0u16, |b, id| b | 1 << id);
            let before = std::mem::replace(&mut self.pads[port], buttons);
            let pressed = buttons & !before;
            let analog = |index, id| cb.input(port as u32, RETRO_DEVICE_ANALOG, index, id) as f32 / 32768.0;
            let axes = [
                analog(RETRO_DEVICE_INDEX_ANALOG_LEFT, RETRO_DEVICE_ID_ANALOG_X),
                analog(RETRO_DEVICE_INDEX_ANALOG_LEFT, RETRO_DEVICE_ID_ANALOG_Y),
                analog(RETRO_DEVICE_INDEX_ANALOG_RIGHT, RETRO_DEVICE_ID_ANALOG_X),
                analog(RETRO_DEVICE_INDEX_ANALOG_RIGHT, RETRO_DEVICE_ID_ANALOG_Y),
            ];
            if port == 0 && buttons & L3_R3 == L3_R3 && pressed & L3_R3 != 0 {
                self.toggle_settings();
                continue;
            }
            if self.ui.is_open() {
                if port == 0 {
                    self.pad_ui(pressed);
                }
                if device == DEVICE_GAMEPORT {
                    self.m.cpu.bus.joystick.set_pad(port, Some(PadState::default()));
                }
                continue;
            }
            match device {
                DEVICE_GAMEPORT => {
                    let map = [
                        (RETRO_DEVICE_ID_JOYPAD_B, PAD_A),
                        (RETRO_DEVICE_ID_JOYPAD_A, PAD_B),
                        (RETRO_DEVICE_ID_JOYPAD_Y, PAD_X),
                        (RETRO_DEVICE_ID_JOYPAD_X, PAD_Y),
                        (RETRO_DEVICE_ID_JOYPAD_UP, PAD_UP),
                        (RETRO_DEVICE_ID_JOYPAD_DOWN, PAD_DOWN),
                        (RETRO_DEVICE_ID_JOYPAD_LEFT, PAD_LEFT),
                        (RETRO_DEVICE_ID_JOYPAD_RIGHT, PAD_RIGHT),
                    ];
                    let pad_buttons = map.iter().filter(|(id, _)| buttons & (1 << id) != 0).fold(0, |b, (_, bit)| b | bit);
                    self.m.cpu.bus.joystick.set_pad(port, Some(PadState { axes, buttons: pad_buttons }));
                }
                DEVICE_KEYS => {
                    self.m.cpu.bus.joystick.set_pad(port, None);
                    // The left stick is the cursor keys too.
                    let mut buttons = buttons;
                    for (axis, minus, plus) in [
                        (axes[0], RETRO_DEVICE_ID_JOYPAD_LEFT, RETRO_DEVICE_ID_JOYPAD_RIGHT),
                        (axes[1], RETRO_DEVICE_ID_JOYPAD_UP, RETRO_DEVICE_ID_JOYPAD_DOWN),
                    ] {
                        if axis < -0.5 {
                            buttons |= 1 << minus;
                        } else if axis > 0.5 {
                            buttons |= 1 << plus;
                        }
                    }
                    let analog_mouse = port == 0 && self.m.options.analog_mouse();
                    let keys = PAD_KEYS.iter().enumerate().fold(0u32, |held, (i, &(id, _, _))| {
                        let taken = analog_mouse && matches!(id, RETRO_DEVICE_ID_JOYPAD_L2 | RETRO_DEVICE_ID_JOYPAD_R2);
                        if buttons & (1 << id) != 0 && !taken { held | 1 << i } else { held }
                    });
                    let before = std::mem::replace(&mut self.pad_keys[port], keys);
                    for (i, &(_, scan, extended)) in PAD_KEYS.iter().enumerate() {
                        let (was, is) = (before & (1 << i) != 0, keys & (1 << i) != 0);
                        if was != is {
                            keyboard::key_event(&mut self.m.cpu.bus, scan, extended, is, None);
                        }
                    }
                }
                _ => self.m.cpu.bus.joystick.set_pad(port, None),
            }
        }
    }

    /// The gamepad's buttons in the settings window: the D-pad moves, A
    /// picks, B goes back, L and R go through the pages.
    fn pad_ui(&mut self, pressed: u16) {
        let map = [
            (RETRO_DEVICE_ID_JOYPAD_UP, UiKey::Up),
            (RETRO_DEVICE_ID_JOYPAD_DOWN, UiKey::Down),
            (RETRO_DEVICE_ID_JOYPAD_LEFT, UiKey::Left),
            (RETRO_DEVICE_ID_JOYPAD_RIGHT, UiKey::Right),
            (RETRO_DEVICE_ID_JOYPAD_A, UiKey::Enter),
            (RETRO_DEVICE_ID_JOYPAD_START, UiKey::Enter),
            (RETRO_DEVICE_ID_JOYPAD_B, UiKey::Esc),
            (RETRO_DEVICE_ID_JOYPAD_L, UiKey::BackTab),
            (RETRO_DEVICE_ID_JOYPAD_R, UiKey::Tab),
            (RETRO_DEVICE_ID_JOYPAD_X, UiKey::Save),
        ];
        for (id, key) in map {
            if pressed & (1 << id) != 0 && self.ui.is_open() {
                self.ui.key(key, &mut self.m);
            }
        }
    }

    /// The mouse, and the right stick as one with `analog_mouse`; in the
    /// settings window, the pointer.
    fn mouse(&mut self, cb: &Callbacks) {
        let speed = self.m.options.mouse_speed();
        let mouse = |id| cb.input(0, RETRO_DEVICE_MOUSE, 0, id);
        let mut dx = mouse(RETRO_DEVICE_ID_MOUSE_X) as f64 * speed;
        let mut dy = mouse(RETRO_DEVICE_ID_MOUSE_Y) as f64 * speed;
        let mut buttons = [RETRO_DEVICE_ID_MOUSE_LEFT, RETRO_DEVICE_ID_MOUSE_RIGHT, RETRO_DEVICE_ID_MOUSE_MIDDLE]
            .iter()
            .enumerate()
            .filter(|&(_, &id)| mouse(id) != 0)
            .fold(0u8, |b, (i, _)| b | 1 << i);
        if self.m.options.analog_mouse() && self.ports[0] != RETRO_DEVICE_NONE {
            let stick = |id| cb.input(0, RETRO_DEVICE_ANALOG, RETRO_DEVICE_INDEX_ANALOG_RIGHT, id) as f64 / 32768.0;
            let (x, y) = (stick(RETRO_DEVICE_ID_ANALOG_X), stick(RETRO_DEVICE_ID_ANALOG_Y));
            // A dead zone, then up to 8 pixels a frame.
            let curve = |v: f64| if v.abs() < 0.15 { 0.0 } else { v * v.abs() * 8.0 };
            dx += curve(x) * speed;
            dy += curve(y) * speed;
            let pad = self.pads[0];
            if pad & (1 << RETRO_DEVICE_ID_JOYPAD_R2) != 0 {
                buttons |= 1;
            }
            if pad & (1 << RETRO_DEVICE_ID_JOYPAD_L2) != 0 {
                buttons |= 2;
            }
        }
        let before = std::mem::replace(&mut self.mouse_buttons, buttons);

        if self.ui.is_open() {
            let pointer = |id| cb.input(0, RETRO_DEVICE_POINTER, 0, id) as i32;
            let pressed = pointer(RETRO_DEVICE_ID_POINTER_PRESSED) != 0 || buttons & 1 != 0;
            let was = std::mem::replace(&mut self.pointer_down, pressed);
            if pressed && !was {
                let (w, h) = (self.screen.width as i32, self.screen.height as i32);
                let x = (pointer(RETRO_DEVICE_ID_POINTER_X) + 0x7FFF) * w / 0xFFFE;
                let y = (pointer(RETRO_DEVICE_ID_POINTER_Y) + 0x7FFF) * h / 0xFFFE;
                self.ui.click(x, y, &mut self.m);
            }
            for (id, notches) in [(RETRO_DEVICE_ID_MOUSE_WHEELUP, 1), (RETRO_DEVICE_ID_MOUSE_WHEELDOWN, -1)] {
                if mouse(id) != 0 && self.ui.is_open() {
                    self.ui.wheel(notches, &mut self.m);
                }
            }
            return;
        }
        let bus = &mut self.m.cpu.bus;
        if dx != 0.0 || dy != 0.0 {
            let (dx, dy) = video::overlay::frame_motion_to_mouse(bus, &self.screen, (dx, dy));
            bus.mouse.move_by(dx, dy);
        }
        for button in 0..3 {
            let (was, is) = (before & (1 << button) != 0, buttons & (1 << button) != 0);
            if is && !was {
                bus.mouse.button_down(button);
            } else if was && !is {
                bus.mouse.button_up(button);
            }
        }
    }

    // ------------------------------------------------------------------
    // Disk control
    // ------------------------------------------------------------------

    /// The images of the disk control's drive, and which is in.
    pub fn disk_images(&self) -> (Vec<PathBuf>, usize) {
        self.disk
            .drive
            .and_then(|d| self.m.cpu.bus.disk.images(d))
            .map_or((Vec::new(), 0), |(list, i)| (list.to_vec(), i))
    }

    /// Add the image `path` to the disk control's drive, or, without one,
    /// put it in the drive it suits: A: for a floppy, the CD-ROM drive (or
    /// D:) for a CD.
    pub fn add_disk_image(&mut self, path: &Path) -> Result<(), String> {
        if let Some(drive) = self.disk.drive {
            return self.m.cpu.bus.disk.add_image(drive, path);
        }
        let kind = rust_dos::diskimage::detect(path, DriveKind::HardDisk)?;
        let (drive, kind) = match kind {
            rust_dos::diskimage::ImageKind::Floppy => (0, DriveKind::Floppy),
            rust_dos::diskimage::ImageKind::Cd => {
                let cd = self.m.cpu.bus.disk.drives_of_kind(DriveKind::CdRom).first().copied();
                (cd.unwrap_or(3), DriveKind::CdRom)
            }
            rust_dos::diskimage::ImageKind::HardDisk => return Err("Hard disk images don't change".to_string()),
        };
        let opts = MountOptions { kind, ..MountOptions::default() };
        self.m.cpu.bus.mount_drive(drive, path, opts, true)?;
        self.disk.drive = Some(drive);
        Ok(())
    }
}
