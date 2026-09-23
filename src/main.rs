use clap::Parser;
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::mouse::MouseButton;
use sdl2::pixels::PixelFormatEnum;
use std::time::Duration;

use crate::audio::pump_audio;
use crate::cpu::Cpu;
use crate::recorder::ScreenRecorder;
use crate::video::VideoMode;

mod debug;

// The emulator itself is the library crate; only the debug server is
// private to the binary. These re-exports let the binary's modules refer to
// the library modules as `crate::...`.
use rust_dos::{
    audio, config, cpu, disk, exec, keyboard, mount, recorder, shell, timer, video,
};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Window scale factor [default: 1, or the config file's scale]
    #[arg(short, long)]
    scale: Option<u32>,

    /// Root directory for Drive C: [default: the config file's C:, or "."]
    #[arg(short, long)]
    dir: Option<String>,

    /// Configuration file to use instead of ./rust-dos.conf or the
    /// per-user default
    #[arg(short, long, value_name = "FILE", conflicts_with = "no_config")]
    config: Option<std::path::PathBuf>,

    /// Don't read or create any configuration file
    #[arg(long)]
    no_config: bool,

    /// Start the HTTP/WebSocket debug server (local-only, unauthenticated).
    /// Optionally takes the listen address.
    #[arg(long, value_name = "ADDR", num_args = 0..=1, default_missing_value = "127.0.0.1:8086")]
    debug_server: Option<std::net::SocketAddr>,

    /// Instruction trace ring buffer capacity (entries, ~64 bytes each).
    /// Allocated only once tracing is enabled via the debug server.
    #[arg(long, default_value_t = 1_000_000)]
    trace_capacity: usize,

    /// Emulated CPU speed in instructions per millisecond, or "max" for as
    /// fast as the host keeps up with [default: max, or the config file's
    /// cycles]
    #[arg(long, value_name = "N|max", value_parser = timer::CpuSpeed::parse)]
    cycles: Option<timer::CpuSpeed>,
}

fn main() -> Result<(), String> {
    let args = Args::parse();
    let config = load_config(&args)?;
    let scale = args.scale.or(config.scale).unwrap_or(1);

    let mut cursor_visible = true;
    let mut last_blink = std::time::Instant::now();
    let blink_interval = Duration::from_millis(500);

    // Initialize Recorder
    // TODO: Make configurable
    let mut recorder = ScreenRecorder::new(15);

    // SDL2 Setup
    let sdl_context = sdl2::init()?;
    let video_subsystem = sdl_context.video()?;
    let audio_subsystem = sdl_context.audio()?;
    let desired_spec = sdl2::audio::AudioSpecDesired {
        freq: Some(44100),
        channels: Some(2),
        samples: None,     // Default buffer size
    };
    let audio_device = audio_subsystem
        .open_queue::<i16, _>(None, &desired_spec)
        .map_err(|e| e.to_string())?;
    audio_device.resume();

    let window = video_subsystem
        .window(
            "Rust DOS Emulator",
            video::SCREEN_WIDTH * scale,
            video::SCREEN_HEIGHT * scale,
        )
        .position_centered()
        .build()
        .map_err(|e| e.to_string())?;

    let mut canvas = window.into_canvas().build().map_err(|e| e.to_string())?;
    let texture_creator = canvas.texture_creator();
    // The texture has the size of the picture, which follows the video
    // mode; the renderer scales it to the window, keeping its proportions.
    let mut texture = texture_creator
        .create_texture_streaming(
            PixelFormatEnum::RGB24,
            video::SCREEN_WIDTH,
            video::SCREEN_HEIGHT,
        )
        .map_err(|e| e.to_string())?;
    canvas
        .set_logical_size(video::SCREEN_WIDTH, video::SCREEN_HEIGHT)
        .map_err(|e| e.to_string())?;

    let mut cpu = create_cpu(&args, &config);
    if let Some(model) = config.cpu {
        cpu.model = model;
    }
    apply_sound_config(&mut cpu, &config.sound);
    cpu.bus.audio_device = Some(audio_device);
    let mut dbg = match args.debug_server {
        Some(addr) => debug::DebugHub::start(&mut cpu, addr, args.trace_capacity)?,
        None => debug::DebugHub::disabled(),
    };
    let mut event_pump = sdl_context.event_pump()?;

    // Load Shell Code into Memory
    cpu.load_shell();

    // Startup commands: the config's [autoexec] lines, then AUTOEXEC.BAT
    // from the C: root if there is one, like the startup sequence a real PC
    // would run. Each line runs as if typed at the prompt.
    cpu.queue_batch_lines(&config.autoexec);
    cpu.queue_batch_file("C:\\AUTOEXEC.BAT");

    // Cached render target. We re-render the full VGA surface only when
    // `cpu.bus.vga.dirty` is set — everything else (cursor blink, mouse
    // cursor, recording indicator) is overlaid on top of this buffer each
    // frame. For a program that isn't actively touching VRAM, the per-SDL-
    // frame cost drops from "640×400×3 zero fill + per-pixel palette/planar
    // lookup" to "one memcpy of the cached buffer + a tiny overlay pass".
    let mut cached_frame = video::Frame::new(video::SCREEN_WIDTH, video::SCREEN_HEIGHT);
    // The cached render with the cursors on top, as the screen shows it.
    let mut screen = cached_frame.clone();

    // Paces emulated time against the wall clock, one frame at a time.
    let speed = args.cycles.or(config.cycles).unwrap_or(timer::CpuSpeed::Max);
    cpu.bus.set_cycles_per_ms(speed.initial_cycles());
    let mut pacer = timer::Pacer::new(speed, std::time::Instant::now());

    // Main Loop
    'running: loop {
        let frame_start = std::time::Instant::now();
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'running,
                Event::KeyDown {
                    keycode: Some(keycode),
                    keymod,
                    ..
                } => {
                    // Update BDA Shift Flags (0x0417)
                    // This lets INT 16h AH=02 report modifier state correctly
                    let mut flags = cpu.bus.read_8(0x0417);
                    match keycode {
                        Keycode::RShift => flags |= 0x01,
                        Keycode::LShift => flags |= 0x02,
                        Keycode::LCtrl | Keycode::RCtrl => flags |= 0x04,
                        Keycode::LAlt | Keycode::RAlt => flags |= 0x08,
                        Keycode::CapsLock => flags ^= 0x40, // Toggle on press
                        _ => {}
                    }
                    cpu.bus.write_8(0x0417, flags);

                    // Recorder Toggle
                    if keycode == Keycode::PrintScreen {
                        recorder.toggle();
                        continue;
                    }

                    // Map Key to PC Scancode/ASCII. High byte = scancode,
                    // low byte = ASCII. Keep pushing to the INT 16h buffer
                    // for BIOS-based input, and ALSO latch the raw scan code
                    // at port 0x60 + raise IRQ1 so games that poll the port
                    // or install a custom INT 09h ISR see the event.
                    let extended = keyboard::is_extended(keycode);
                    if let Some(scan) = keyboard::modifier_scan(keycode) {
                        keyboard::deliver_scan_only(&mut cpu.bus, scan, extended);
                    } else if let Some(code) = keyboard::map_sdl_to_pc(keycode, keymod) {
                        keyboard::deliver_key_down(&mut cpu.bus, code, extended);
                    }
                }
                Event::KeyUp {
                    keycode: Some(keycode),
                    keymod,
                    ..
                } => {
                    // Update BDA Shift Flags (Clear bits)
                    let mut flags = cpu.bus.read_8(0x0417);
                    match keycode {
                        Keycode::RShift => flags &= !0x01,
                        Keycode::LShift => flags &= !0x02,
                        Keycode::LCtrl | Keycode::RCtrl => flags &= !0x04,
                        Keycode::LAlt | Keycode::RAlt => flags &= !0x08,
                        _ => {}
                    }
                    cpu.bus.write_8(0x0417, flags);

                    // Deliver release scan code (scancode | 0x80) to port 0x60
                    // and fire IRQ1. Games that track held keys (arrow-key
                    // movement, etc.) need these to know when the key stops
                    // being pressed.
                    let extended = keyboard::is_extended(keycode);
                    if let Some(scan) = keyboard::modifier_scan(keycode) {
                        keyboard::deliver_key_up(&mut cpu.bus, scan, extended);
                    } else if let Some(code) = keyboard::map_sdl_to_pc(keycode, keymod) {
                        keyboard::deliver_key_up(&mut cpu.bus, (code >> 8) as u8, extended);
                    }
                }

                Event::MouseMotion { x, y, .. } => {
                    let (vx, vy) = host_to_virtual_mouse(&cpu, &cached_frame, x, y);
                    cpu.bus.mouse.set_position(vx, vy);
                }

                Event::MouseButtonDown { mouse_btn, x, y, .. } => {
                    let (vx, vy) = host_to_virtual_mouse(&cpu, &cached_frame, x, y);
                    cpu.bus.mouse.set_position(vx, vy);
                    if let Some(btn) = sdl_button_to_index(mouse_btn) {
                        cpu.bus.mouse.button_down(btn);
                    }
                }

                Event::MouseButtonUp { mouse_btn, x, y, .. } => {
                    let (vx, vy) = host_to_virtual_mouse(&cpu, &cached_frame, x, y);
                    cpu.bus.mouse.set_position(vx, vy);
                    if let Some(btn) = sdl_button_to_index(mouse_btn) {
                        cpu.bus.mouse.button_up(btn);
                    }
                }

                _ => {}
            }
        }

        // Remote debug requests and queued remote input.
        dbg.poll(&mut cpu);

        // Run the emulated machine up to the wall clock. Emulated time is
        // counted in instructions (see timer.rs), so timer interrupts land on
        // the right instructions however the work is batched between frames.
        let batch_start = std::time::Instant::now();
        let batch_end = if dbg.paused {
            cpu.bus.clock.icount
        } else {
            pacer.batch_end(&cpu.bus.clock, batch_start)
        };
        cpu.bus.start_batch(batch_end);
        let batch_icount = cpu.bus.clock.icount;
        let batch_stalled = cpu.bus.clock.stalled;
        let batch_idle = cpu.bus.clock.idle;

        // Per-instruction debug hook (breakpoints / stepping / tracing) is
        // only consulted when something actually needs it.
        let dbg_hot = dbg.begin_batch(&cpu);
        exec::run_batch(&mut cpu, &mut dbg, dbg_hot);
        dbg.end_batch(&cpu);

        let exec_time = batch_start.elapsed();
        let stalled = cpu.bus.clock.stalled - batch_stalled;
        let idle = cpu.bus.clock.idle - batch_idle;
        let executed = cpu.bus.clock.icount - batch_icount - idle - stalled;
        dbg.record_batch(
            executed,
            exec_time,
            cpu.decode_cache.hits,
            cpu.decode_cache.misses,
        );

        // Update Audio
        pump_audio(&mut cpu.bus);
        cpu.bus.flush_log();

        // Update Cursor Blink
        if last_blink.elapsed() >= blink_interval {
            cursor_visible = !cursor_visible;
            last_blink = std::time::Instant::now();
        }

        // Render Frame. The expensive part (the pixel fill driven by
        // palette/planar lookups inside `render_screen`) only happens when the
        // VGA state changed since last frame. On "clean" frames we reuse
        // `cached_frame` and just overlay the cursor/mouse/recording pip.
        // The CRTC picks up the Start Address the program flipped to at the
        // vertical retraces that passed, whether or not it polled port 3DAh.
        cpu.bus.sync_display();
        let (width, height) = video::frame_size(&cpu.bus);
        if cached_frame.resize(width, height) {
            cpu.bus.vga.mark_dirty_full();
            texture = texture_creator
                .create_texture_streaming(PixelFormatEnum::RGB24, width, height)
                .map_err(|e| e.to_string())?;
            canvas.set_logical_size(width, height).map_err(|e| e.to_string())?;
            fit_window(&video_subsystem, canvas.window_mut(), width, height, scale);
        }
        if cpu.bus.vga.dirty {
            video::render_screen(&mut cached_frame, &cpu.bus);
            cpu.bus.vga.clear_dirty();
        }

        // Start from the cached render; the overlays go on top.
        screen.clone_from(&cached_frame);
        let buffer = &mut screen.rgb[..];
        let frame_w = width as usize;

        // Draw the Cursor (Overlay)
        // Only draw the hardware cursor in Text Modes!
        let current_mode = cpu.bus.video_mode;
        let is_text_mode = matches!(
            current_mode,
            VideoMode::Text80x25
                | VideoMode::Text80x25Color
                | VideoMode::Text40x25
                | VideoMode::Text40x25Color
        );
        if is_text_mode {
            // Read Cursor Position from BDA
            let cursor_col = cpu.bus.read_8(0x0450) as usize;
            let cursor_row = cpu.bus.read_8(0x0451) as usize;

            // Read Cursor Shape from BDA
            let cursor_shape = cpu.bus.read_16(0x0460);
            let start_scan = (cursor_shape >> 8) as u8;
            let end_scan = (cursor_shape & 0xFF) as u8;

            // Bit 5 of Start Scanline indicates "Invisible" in VGA hardware
            let is_hidden = (start_scan & 0x20) != 0;

            // Determine Cell Width based on Mode
            // 40-col modes have 16px wide characters (scaled 2x)
            let (cell_width, max_cols) = match current_mode {
                VideoMode::Text40x25 | VideoMode::Text40x25Color => (16, 40),
                _ => (8, 80),
            };
            // Cell height and visible rows come from BDA so 80x43 / 80x50
            // modes draw the cursor at the correct Y when programs like
            // Norton Commander load the 8x8 font.
            let cell_height = cpu.bus.read_16(0x0485) as usize;
            let cell_height = if cell_height == 0 { 16 } else { cell_height };
            let total_rows = cpu.bus.read_8(0x0484) as usize + 1;

            if cursor_visible
                && !is_hidden
                && cursor_col < max_cols
                && cursor_row < total_rows
            {
                // Calculate screen coordinates
                let start_x = cursor_col * cell_width;
                let start_y = cursor_row * cell_height;

                // Clamp scanlines to the active cell height - 1.
                let max_scan = cell_height.saturating_sub(1) as u8;
                let scan_start = (start_scan & 0x1F).min(max_scan) as usize;
                let scan_end = end_scan.min(max_scan) as usize;

                if scan_start <= scan_end {
                    for y_off in scan_start..=scan_end {
                        for x_off in 0..cell_width {
                            let draw_x = start_x + x_off;
                            let draw_y = start_y + y_off;

                            // Safety Check
                            let idx = (draw_y * frame_w + draw_x) * 3;
                            if idx + 2 < buffer.len() {
                                buffer[idx] = 0xDD;
                                buffer[idx + 1] = 0xDD;
                                buffer[idx + 2] = 0xDD;
                            }
                        }
                    }
                }
            }
        }

        // Draw Mouse Cursor (software overlay) when visible and installed.
        // The driver stores the cursor in virtual coords; map those to
        // screen pixels over the same virtual extent the host pointer spans.
        if cpu.bus.mouse.installed && cpu.bus.mouse.hide_counter <= 0 {
            let (virt_w, virt_h) = cpu.bus.mouse.virtual_extent(cpu.bus.video_mode);
            let sx = (cpu.bus.mouse.x as i64 * width as i64 / virt_w as i64) as i32;
            let sy = (cpu.bus.mouse.y as i64 * height as i64 / virt_h as i64) as i32;
            draw_default_mouse_cursor(&mut screen, sx, sy);
        }

        // Send Frame to Recorder / debug clients before drawing the
        // recording indicator
        recorder.capture(&screen);
        dbg.capture_frame(&screen);

        // Draw Recording Indicator
        if recorder.is_active() {
            let radius = 5;
            let center_x = frame_w - 15;
            let center_y = 15;

            for y in (center_y - radius)..=(center_y + radius) {
                for x in (center_x - radius)..=(center_x + radius) {
                    let dx = x as isize - center_x as isize;
                    let dy = y as isize - center_y as isize;
                    if dx * dx + dy * dy <= (radius * radius) as isize {
                        let idx = (y * frame_w + x) * 3;
                        if idx + 2 < screen.rgb.len() {
                            screen.rgb[idx] = 0xFF; // R
                            screen.rgb[idx + 1] = 0x00; // G
                            screen.rgb[idx + 2] = 0x00; // B
                        }
                    }
                }
            }
        }
        texture
            .update(None, &screen.rgb, frame_w * 3)
            .map_err(|e| e.to_string())?;
        canvas.clear();
        canvas.copy(&texture, None, None)?;
        canvas.present();

        let overhead = frame_start.elapsed().saturating_sub(exec_time);
        if let Some(cycles) = pacer.end_frame(&cpu.bus.clock, executed, exec_time, overhead) {
            cpu.bus.set_cycles_per_ms(cycles);
        }
        pacer.wait_for_next_frame();
    }

    Ok(())
}

/// Install the configured sound hardware and the drive with the built-in
/// Ultrasound software, advertise them in the BLASTER, ULTRASND and
/// ULTRADIR environment variables, and give the MPU-401 its synthesizer.
fn apply_sound_config(cpu: &mut cpu::Cpu, sound: &config::SoundConfig) {
    use config::MidiSynth;

    cpu.bus.configure_sound(sound.card(), sound.opl3);
    match &sound.card() {
        Some(sb) => cpu.set_env("BLASTER", &sb.blaster()),
        None => cpu.set_env("BLASTER", ""),
    }
    let gus = sound.ultrasound();
    if let Err(e) = cpu.bus.mount_ultrasnd(gus.as_ref().and_then(|g| g.drive)) {
        config_warning(cpu, &format!("gusdrive: {}", e));
    }
    match &gus {
        Some(g) => {
            cpu.set_env("ULTRASND", &g.ultrasnd());
            cpu.set_env("ULTRADIR", &g.ultradir());
        }
        None => {
            cpu.set_env("ULTRASND", "");
            cpu.set_env("ULTRADIR", "");
        }
    }
    cpu.bus.configure_gus(gus);

    let soundfont = match sound.midisynth {
        MidiSynth::SoundFont => true,
        MidiSynth::Auto => sound.soundfont.is_some(),
        MidiSynth::Gus | MidiSynth::None => false,
    };
    if soundfont {
        match &sound.soundfont {
            Some(path) => match cpu.bus.mpu.load_soundfont(path) {
                Ok(()) => cpu
                    .bus
                    .log_string(&format!("[CONFIG] General MIDI with SoundFont {}", path.display())),
                Err(e) => config_warning(cpu, &format!("soundfont: {}", e)),
            },
            None => config_warning(cpu, "midisynth=soundfont needs a soundfont setting"),
        }
    } else if sound.midisynth != MidiSynth::None {
        use rust_dos::gus::patch::PatchBank;
        // The built-in patches whether or not their drive is there.
        let bank = if sound.gus.builtin() {
            Ok((PatchBank::builtin(), "built into rust-dos".to_string()))
        } else {
            PatchBank::from_dos_dir(&cpu.bus.disk, &sound.gus.ultradir()).map(|(bank, dir)| (bank, format!("in {}", dir)))
        };
        match bank {
            Ok((bank, place)) => {
                cpu.bus
                    .log_string(&format!("[CONFIG] General MIDI with the Ultrasound patches {}", place));
                cpu.bus.mpu.load_gus_patches(bank);
            }
            Err(e) if sound.midisynth == MidiSynth::Gus => {
                config_warning(cpu, &format!("midisynth=gus: {}", e))
            }
            Err(e) => cpu.bus.log_string(&format!(
                "[CONFIG] No General MIDI synthesizer (no soundfont, and {})",
                e
            )),
        }
    }
}

/// A configuration problem: shown on the terminal and written to the log.
fn config_warning(cpu: &mut Cpu, msg: &str) {
    eprintln!("[CONFIG] Warning: {}", msg);
    cpu.bus.log_string(&format!("[CONFIG] Warning: {}", msg));
}

/// Find and parse the configuration file (see config.rs for the lookup
/// order). Problems in the file are reported but never stop the emulator;
/// only a missing `--config` file does. The file used goes to the log once
/// there is one (see `create_cpu`).
fn load_config(args: &Args) -> Result<config::Config, String> {
    if args.no_config {
        return Ok(config::Config::default());
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let config = config::load(
        args.config.as_deref(),
        &cwd,
        config::default_path(),
        dirs::home_dir().as_deref(),
    )?;
    if config.created
        && let Some(path) = &config.source
    {
        eprintln!("[CONFIG] Created default configuration file {}", path.display());
    }
    for warning in &config.warnings {
        eprintln!("[CONFIG] Warning: {}", warning);
    }
    Ok(config)
}

/// Build the CPU with drive C: from `-d`, the config file or the working
/// directory (in that order), then mount the config's other drives. Opens
/// the log file.
fn create_cpu(args: &Args, config: &config::Config) -> Cpu {
    use crate::disk::{DRIVE_C, drive_letter};

    let mut warnings = config.warnings.clone();
    let mut warn = |msg: String| {
        eprintln!("[CONFIG] Warning: {}", msg);
        warnings.push(msg);
    };

    let config_c = config.drive(DRIVE_C);
    let c_spec = match (&args.dir, config_c) {
        (Some(_), Some(_)) => {
            warn("-d/--dir overrides drive C: from the config file".to_string());
            None
        }
        (None, Some(spec)) if !spec.path.is_dir() => {
            warn(format!(
                "C: {} is not a directory, using the current directory",
                spec.path.display()
            ));
            None
        }
        (_, spec) => spec,
    };
    let root_path = match (&args.dir, c_spec) {
        (Some(dir), _) => std::path::PathBuf::from(dir),
        (None, Some(spec)) => spec.path.clone(),
        (None, None) => std::path::PathBuf::from("."),
    };

    let memory_mb = config.memsize.unwrap_or(rust_dos::bus::DEFAULT_MEMORY_MB);
    let mut cpu = Cpu::with_memory(root_path.clone(), memory_mb);
    cpu.bus.log_file = open_log_file();
    if let Some(spec) = c_spec {
        // Remount C: to apply the config's drive type, label and -ro
        if let Err(e) = cpu.bus.mount_drive(DRIVE_C, &root_path, spec.opts.clone(), true) {
            warn(format!("cannot set up drive C: {}", e));
        }
    }
    for spec in config.drives.iter().filter(|s| s.drive != DRIVE_C) {
        if let Err(e) = cpu.bus.mount_drive(spec.drive, &spec.path, spec.opts.clone(), false) {
            warn(format!("cannot mount {}: {}", drive_letter(spec.drive), e));
        }
    }

    if let Some(path) = &config.source {
        let action = if config.created { "Created default" } else { "Using" };
        cpu.bus
            .log_string(&format!("[CONFIG] {} configuration file {}", action, path.display()));
    }
    for warning in &warnings {
        cpu.bus.log_string(&format!("[CONFIG] Warning: {}", warning));
    }
    cpu
}

/// Create the log file in the per-user configuration directory, replacing
/// the previous run's. The emulator runs without one if that fails.
fn open_log_file() -> Option<rust_dos::log::LogFile> {
    let path = rust_dos::log::default_path()?;
    match rust_dos::log::LogFile::create(&path) {
        Ok(log) => Some(log),
        Err(e) => {
            eprintln!("Warning: cannot create the log file {}: {}", path.display(), e);
            None
        }
    }
}

/// Convert mouse coordinates in the picture's pixels (SDL scales window
/// coordinates to the logical size of the renderer, the picture's size)
/// into the driver's virtual coordinate system (see
/// `MouseState::virtual_extent`).
fn host_to_virtual_mouse(cpu: &Cpu, frame: &video::Frame, x: i32, y: i32) -> (i32, i32) {
    let (w, h) = (frame.width as i32, frame.height as i32);
    let px = x.clamp(0, w - 1);
    let py = y.clamp(0, h - 1);

    let (virt_w, virt_h) = cpu.bus.mouse.virtual_extent(cpu.bus.video_mode);

    let vx = (px as i64 * virt_w as i64 / w as i64) as i32;
    let vy = (py as i64 * virt_h as i64 / h as i64) as i32;
    (vx.clamp(0, virt_w - 1), vy.clamp(0, virt_h - 1))
}

/// Make the window `scale` times the picture's size, or the largest whole
/// multiple of it that fits the desktop.
fn fit_window(video: &sdl2::VideoSubsystem, window: &mut sdl2::video::Window, width: u32, height: u32, scale: u32) {
    let bounds = window.display_index().and_then(|display| video.display_usable_bounds(display));
    let mut scale = scale.max(1);
    if let Ok(bounds) = bounds {
        while scale > 1 && (width * scale > bounds.width() || height * scale > bounds.height()) {
            scale -= 1;
        }
    }
    let _ = window.set_size(width * scale, height * scale);
}

fn sdl_button_to_index(button: MouseButton) -> Option<usize> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Right => Some(1),
        MouseButton::Middle => Some(2),
        _ => None,
    }
}

/// Classic Microsoft-style arrow cursor as a 16x16 bitmap. 1 = white pixel,
/// 2 = black outline, 0 = transparent. Hotspot is (0,0).
const CURSOR_ARROW: [[u8; 16]; 16] = [
    [2,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
    [2,2,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
    [2,1,2,0,0,0,0,0,0,0,0,0,0,0,0,0],
    [2,1,1,2,0,0,0,0,0,0,0,0,0,0,0,0],
    [2,1,1,1,2,0,0,0,0,0,0,0,0,0,0,0],
    [2,1,1,1,1,2,0,0,0,0,0,0,0,0,0,0],
    [2,1,1,1,1,1,2,0,0,0,0,0,0,0,0,0],
    [2,1,1,1,1,1,1,2,0,0,0,0,0,0,0,0],
    [2,1,1,1,1,1,1,1,2,0,0,0,0,0,0,0],
    [2,1,1,1,1,1,2,2,2,2,0,0,0,0,0,0],
    [2,1,1,2,1,1,2,0,0,0,0,0,0,0,0,0],
    [2,1,2,0,2,1,1,2,0,0,0,0,0,0,0,0],
    [2,2,0,0,2,1,1,2,0,0,0,0,0,0,0,0],
    [0,0,0,0,0,2,1,1,2,0,0,0,0,0,0,0],
    [0,0,0,0,0,2,1,1,2,0,0,0,0,0,0,0],
    [0,0,0,0,0,0,2,2,2,0,0,0,0,0,0,0],
];

fn draw_default_mouse_cursor(frame: &mut video::Frame, origin_x: i32, origin_y: i32) {
    let (w, h) = (frame.width as i32, frame.height as i32);
    for (row_idx, row) in CURSOR_ARROW.iter().enumerate() {
        for (col_idx, &cell) in row.iter().enumerate() {
            if cell == 0 {
                continue;
            }
            let x = origin_x + col_idx as i32;
            let y = origin_y + row_idx as i32;
            if x < 0 || y < 0 || x >= w || y >= h {
                continue;
            }
            let idx = (y * w + x) as usize * 3;
            let (r, g, b) = if cell == 1 {
                (0xFF, 0xFF, 0xFF)
            } else {
                (0x00, 0x00, 0x00)
            };
            frame.rgb[idx] = r;
            frame.rgb[idx + 1] = g;
            frame.rgb[idx + 2] = b;
        }
    }
}
