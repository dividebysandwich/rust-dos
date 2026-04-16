use clap::Parser;
use iced_x86::{Decoder, DecoderOptions, Mnemonic};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::mouse::MouseButton;
use sdl2::pixels::PixelFormatEnum;
use std::io::Write;
use std::time::Duration;

use crate::audio::pump_audio;
use crate::command::CommandDispatcher;
use crate::cpu::{Cpu, CpuFlags, CpuState};
use crate::recorder::ScreenRecorder;
use crate::video::VideoMode;

mod audio;
mod bus;
mod command;
mod cpu;
mod disk;
mod f80;
mod instructions;
mod interrupts;
mod keyboard;
mod mcb;
mod mouse;
mod recorder;
mod shell;
mod video;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value_t = 1)]
    scale: u32,

    /// Root directory for Drive C:
    #[arg(short, long, default_value = ".")]
    dir: String,
}

fn main() -> Result<(), String> {
    let args = Args::parse();
    let mut debug_mode = false;

    let mut cursor_visible = true;
    let mut last_blink = std::time::Instant::now();
    let blink_interval = Duration::from_millis(500);

    // Initialize Recorder
    // TODO: Make configurable
    let mut recorder = ScreenRecorder::new(video::SCREEN_WIDTH, video::SCREEN_HEIGHT, 15);

    // SDL2 Setup
    let sdl_context = sdl2::init()?;
    let video_subsystem = sdl_context.video()?;
    let audio_subsystem = sdl_context.audio()?;
    let desired_spec = sdl2::audio::AudioSpecDesired {
        freq: Some(44100),
        channels: Some(1), // Mono is fine for beeps
        samples: None,     // Default buffer size
    };
    let audio_device = audio_subsystem
        .open_queue::<i16, _>(None, &desired_spec)
        .map_err(|e| e.to_string())?;
    audio_device.resume();

    let window = video_subsystem
        .window(
            "Rust DOS Emulator",
            video::SCREEN_WIDTH * args.scale,
            video::SCREEN_HEIGHT * args.scale,
        )
        .position_centered()
        .build()
        .map_err(|e| e.to_string())?;

    let mut canvas = window.into_canvas().build().map_err(|e| e.to_string())?;
    let texture_creator = canvas.texture_creator();
    // Texture is always 640x400 RGB
    let mut texture = texture_creator
        .create_texture_streaming(
            PixelFormatEnum::RGB24,
            video::SCREEN_WIDTH,
            video::SCREEN_HEIGHT,
        )
        .map_err(|e| e.to_string())?;

    let root_path = std::path::PathBuf::from(&args.dir);
    let mut cpu = Cpu::new(root_path);
    cpu.bus.audio_device = Some(audio_device);
    let mut event_pump = sdl_context.event_pump()?;

    // Load Shell Code into Memory
    cpu.load_shell();

    // Main Loop
    'running: loop {
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

                    // Debug Toggle (F12 reserved for Emulator)
                    if keycode == Keycode::F12 {
                        debug_mode = !debug_mode;
                        cpu.bus.log_string(&format!(
                            "[DEBUG] Tracing: {}",
                            if debug_mode { "ON" } else { "OFF" }
                        ));
                        continue;
                    }

                    // Map Key to PC Scancode/ASCII. High byte = scancode,
                    // low byte = ASCII. Keep pushing to the INT 16h buffer
                    // for BIOS-based input, and ALSO latch the raw scan code
                    // at port 0x60 + raise IRQ1 so games that poll the port
                    // or install a custom INT 09h ISR see the event.
                    if let Some(code) = keyboard::map_sdl_to_pc(keycode, keymod) {
                        cpu.bus.keyboard_buffer.push_back(code);
                        cpu.bus.last_scan_code = (code >> 8) as u8;
                        cpu.bus.irq1_pending = true;
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
                    if let Some(code) = keyboard::map_sdl_to_pc(keycode, keymod) {
                        let sc = (code >> 8) as u8;
                        if sc != 0 {
                            cpu.bus.last_scan_code = sc | 0x80;
                            cpu.bus.irq1_pending = true;
                        }
                    }
                }

                Event::MouseMotion { x, y, .. } => {
                    let (vx, vy) = host_to_virtual_mouse(&cpu, x, y, args.scale);
                    cpu.bus.mouse.set_position(vx, vy);
                }

                Event::MouseButtonDown { mouse_btn, x, y, .. } => {
                    let (vx, vy) = host_to_virtual_mouse(&cpu, x, y, args.scale);
                    cpu.bus.mouse.set_position(vx, vy);
                    if let Some(btn) = sdl_button_to_index(mouse_btn) {
                        cpu.bus.mouse.button_down(btn);
                    }
                }

                Event::MouseButtonUp { mouse_btn, x, y, .. } => {
                    let (vx, vy) = host_to_virtual_mouse(&cpu, x, y, args.scale);
                    cpu.bus.mouse.set_position(vx, vy);
                    if let Some(btn) = sdl_button_to_index(mouse_btn) {
                        cpu.bus.mouse.button_up(btn);
                    }
                }

                _ => {}
            }
        }

        // DEBUG: every ~30k-instruction batch, sample the current CS:IP so we
        // can tell which code region a program is spinning in when the screen
        // goes unresponsive. Logged ~once per second (SDL caps us at 60 fps).
        {
            use std::sync::atomic::{AtomicU32, Ordering};
            static FRAME_SAMPLE: AtomicU32 = AtomicU32::new(0);
            let n = FRAME_SAMPLE.fetch_add(1, Ordering::Relaxed);
            if n % 120 == 0 && cpu.cs < 0xF000 {
                let phys_code = cpu.get_physical_addr(cpu.cs, cpu.ip);
                let mut code_bytes = String::new();
                for i in 0..16 {
                    code_bytes.push_str(&format!("{:02X} ", cpu.bus.read_8(phys_code + i)));
                }
                // Also dump 12 bytes from DS:SI (likely palette-source buffer
                // in programs stuck in a fade loop) and 16 bytes from
                // BDA tick counter so we can see whether time is progressing.
                let phys_dssi = cpu.get_physical_addr(cpu.ds, cpu.si);
                let mut ds_si = String::new();
                for i in 0..12 {
                    ds_si.push_str(&format!("{:02X} ", cpu.bus.read_8(phys_dssi + i)));
                }
                let ticks_lo = cpu.bus.read_16(0x046C);
                let ticks_hi = cpu.bus.read_16(0x046E);
                cpu.bus.log_string(&format!(
                    "[SAMPLE] CS:IP={:04X}:{:04X} DS={:04X} SS={:04X} AX={:04X} BX={:04X} CX={:04X} DX={:04X} SI={:04X} BP={:04X} SP={:04X} ticks={:04X}{:04X}",
                    cpu.cs, cpu.ip, cpu.ds, cpu.ss, cpu.ax, cpu.bx, cpu.cx, cpu.dx, cpu.si, cpu.bp, cpu.sp,
                    ticks_hi, ticks_lo
                ));
                cpu.bus.log_string(&format!(
                    "         code@CS:IP:  {}", code_bytes.trim()
                ));
                cpu.bus.log_string(&format!(
                    "         data@DS:SI:  {}", ds_si.trim()
                ));
            }
        }

        // Execute instructions
        for _ in 0..30_000 {
            // Inject INT 08h (system timer IRQ0) at the BIOS standard ~18.2 Hz.
            // Without this, the BDA tick counter at 0x046C never advances and
            // programs that use INT 21h AH=2Ch or directly poll the BDA tick
            // count to gate fades, animations, or delays wait forever.
            // (Cpu::step has an equivalent injection for the test harness, but
            //  the real emulator loop dispatches instructions inline and so
            //  needs its own copy.)
            if cpu.get_cpu_flag(CpuFlags::IF) {
                // IRQ 0 (timer) — gated by PIC IMR bit 0.
                if (cpu.bus.pic_mask & 0x01) == 0 {
                    let now = cpu.bus.start_time.elapsed().as_millis();
                    if now.wrapping_sub(cpu.last_timer_tick) >= 55 {
                        cpu.last_timer_tick = now;
                        let ivt = 0x08usize * 4;
                        let handler_ip = cpu.bus.read_16(ivt);
                        let handler_cs = cpu.bus.read_16(ivt + 2);
                        if handler_cs != 0 || handler_ip != 0 {
                            cpu.push(cpu.get_cpu_flags().bits());
                            cpu.push(cpu.cs);
                            cpu.push(cpu.ip);
                            cpu.cs = handler_cs;
                            cpu.ip = handler_ip;
                            cpu.set_cpu_flag(CpuFlags::IF, false);
                            cpu.set_cpu_flag(CpuFlags::TF, false);
                            continue;
                        }
                    }
                }

                // IRQ 1 (keyboard) — gated by PIC IMR bit 1. Fired once per
                // key event so custom INT 09h ISRs see the scan code.
                if cpu.bus.irq1_pending && (cpu.bus.pic_mask & 0x02) == 0 {
                    cpu.bus.irq1_pending = false;
                    let ivt = 0x09usize * 4;
                    let handler_ip = cpu.bus.read_16(ivt);
                    let handler_cs = cpu.bus.read_16(ivt + 2);
                    if handler_cs != 0 || handler_ip != 0 {
                        cpu.push(cpu.get_cpu_flags().bits());
                        cpu.push(cpu.cs);
                        cpu.push(cpu.ip);
                        cpu.cs = handler_cs;
                        cpu.ip = handler_ip;
                        cpu.set_cpu_flag(CpuFlags::IF, false);
                        cpu.set_cpu_flag(CpuFlags::TF, false);
                        continue;
                    }
                }
            }

            let prev_ip = cpu.ip;

            // --- HANDLE PENDING COMMANDS (Outside Interrupts) ---
            if let Some(cmd) = cpu.pending_command.take() {
                // We have a command from the shell!
                cpu.bus
                    .log_string(&format!("[MAIN] Processing Command: {}", cmd));

                let (command, args) = match cmd.split_once(' ') {
                    Some((c, a)) => (c, a.trim()),
                    None => (cmd.as_str(), ""),
                };

                let dispatcher = CommandDispatcher::new();

                // Dispatch logic
                if dispatcher.dispatch(&mut cpu, command, args) {
                    // Built-in command executed. CPU continues shell loop.
                } else {
                    // Load Program
                    let filename = command.to_string();
                    let loaded = if !filename.contains('.') {
                        cpu.load_executable(&format!("{}.com", command), None)
                            || cpu.load_executable(&format!("{}.exe", command), None)
                    } else {
                        cpu.load_executable(&filename, None)
                    };

                    if !loaded {
                        crate::video::print_string(&mut cpu, "Bad command or file name.\r\n");
                    }
                    // If loaded, load_executable() reset CS:IP.
                    // The CPU will naturally start executing the new program next cycle.
                }

                // Skip the rest of this cycle to ensure clean state
                continue;
            }

            // --- HANDLE STATE CHANGES ---
            if cpu.state == CpuState::RebootShell {
                cpu.load_shell();
                cpu.state = CpuState::Running;

                //TODO: Replace this hack with a proper fix
                //Add a newline to make sure the prompt starts on a new line.
                let col = cpu.bus.read_8(0x0450);
                if col != 0 {
                    video::print_string(&mut cpu, "\r\n");
                }

                break; // Break inner loop to refresh SDL
            }

            // Handle "IP = 0" as an explicit exit (Standard COM behavior)
            // If the program jumps to the start of the segment, it wants to exit.
            if cpu.ip == 0x0000 && cpu.cs == 0x1000 {
                cpu.bus
                    .log_string("[DOS] Program jumped to offset 0000h. Exiting to Shell.");
                // Flush log on exit so we don't lose tail data
                let _ = cpu.bus.log_file.as_mut().unwrap().flush();
                cpu.load_shell();
                cpu.state = CpuState::Running;
                shell::show_prompt(&mut cpu);
                break;
            }

            // Current instruction
            let phys_ip = cpu.get_physical_addr(cpu.cs, cpu.ip);
            // Look ahead one instruction
            let b0 = cpu.bus.read_8(phys_ip);
            let b1 = cpu.bus.read_8(cpu.get_physical_addr(cpu.cs, cpu.ip + 1));
            let bytes = &cpu.bus.ram[phys_ip..];

            // If we are about to execute 00 00, stop immediately.
            if bytes.len() >= 2 && bytes[0] == 0x00 && bytes[1] == 0x00 {
                // panic!(
                //     "[CRITICAL] CPU hit 00 00 (Empty RAM) at {:04X}:{:04X}",
                //     cpu.cs, cpu.ip
                // );
            }

            // Check for "BOP" (BIOS Operation) -> FE 38 XX
            if b0 == 0xFE && b1 == 0x38 {
                let vector = cpu.bus.read_8(cpu.get_physical_addr(cpu.cs, cpu.ip + 2));

                // Run the HLE handler directly
                crate::interrupts::handle_hle(&mut cpu, vector);

                // Do not call real IRET, just simulate it
                cpu.ip = cpu.pop();
                cpu.cs = cpu.pop();

                let hle_cf = cpu.get_cpu_flag(CpuFlags::CF);
                let hle_zf = cpu.get_cpu_flag(CpuFlags::ZF);
                let flags_to_restore = CpuFlags::from_bits_truncate(cpu.pop());

                cpu.set_cpu_flags(flags_to_restore);
                cpu.set_cpu_flag(CpuFlags::DF, false);
                cpu.set_cpu_flag(CpuFlags::CF, hle_cf);
                cpu.set_cpu_flag(CpuFlags::ZF, hle_zf);

                continue; // Done for this cycle
            }

            let mut decoder = Decoder::with_ip(16, bytes, cpu.ip as u64, DecoderOptions::NONE);
            let instr = decoder.decode();

            if debug_mode || cpu.debug_qb_print {
                // Filter out the 'Wait for Key' interrupt loop to save disk space
                if !((instr.mnemonic() == Mnemonic::Int && instr.immediate8() == 0x16)
                    || (instr.mnemonic() == Mnemonic::Jmp && instr.near_branch16() == 0x10E))
                {
                    // Skip BIOS area noise
                    if cpu.cs < 0xF000 {
                        // Format the instruction string manually since we can't capture stdout
                        // (Assuming you want the same format as print_debug_trace)
                        let instr_text = format!("{}", instr);
                        let log_line = format!(
                            "{:04X}:{:04X}  AX:{:04X} BX:{:04X} CX:{:04X} DX:{:04X} SP:{:04X}  {}",
                            cpu.cs,
                            cpu.ip,
                            cpu.get_reg16(iced_x86::Register::AX),
                            cpu.get_reg16(iced_x86::Register::BX),
                            cpu.get_reg16(iced_x86::Register::CX),
                            cpu.get_reg16(iced_x86::Register::DX),
                            cpu.sp,
                            instr_text
                        );

                        // Write to file, ignore errors to keep emulation fast
                        let _ = cpu.bus.log_string(&log_line);

                        if instr.mnemonic() == Mnemonic::Int {
                            let vector = instr.immediate8();
                            // Read IVT (Vector * 4) to find where this points
                            let ivt_addr = (vector as usize) * 4;
                            let target_cs = cpu.bus.read_16(ivt_addr + 2);
                            let target_ip = cpu.bus.read_16(ivt_addr);

                            if target_cs == 0xF000 {
                                let log = format!(
                                    "[CPU-DEBUG] Hooked INT {:02X} detected -> Points to F000:{:04X}",
                                    vector, target_ip
                                );
                                cpu.bus.log_string(&log);
                            }
                        }
                    }
                }
            }

            cpu.trace_qb_conversion(&instr);

            cpu.ip = instr.next_ip() as u16;

            // Check State
            if cpu.state == CpuState::RebootShell {
                cpu.load_shell(); // Reloads assembly into RAM, resets IP/SP
                cpu.state = CpuState::Running;
                shell::show_prompt(&mut cpu);
                break; // Break inner execution batch
            }

            // Yield if we are in a tight loop
            if cpu.ip == prev_ip {
                std::thread::yield_now();
            }

            // Make it so
            instructions::execute_instruction(&mut cpu, &instr);
        }

        // Update Audio
        pump_audio(&mut cpu.bus);

        // Update Cursor Blink
        if last_blink.elapsed() >= blink_interval {
            cursor_visible = !cursor_visible;
            last_blink = std::time::Instant::now();
        }

        // Render Frame
        // Note: We redraw every frame here for simplicity, even if VRAM isn't dirty
        texture.with_lock(None, |buffer: &mut [u8], _pitch: usize| {
            // Draw the base screen (text characters)
            video::render_screen(buffer, &cpu.bus);

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
                                let idx = (draw_y * video::SCREEN_WIDTH as usize + draw_x) * 3;
                                if idx + 2 < buffer.len() {
                                    // Draw Cursor (Invert or Solid Block)
                                    // Using a distinct color (e.g., pure white or slightly transparent look)
                                    // TODO: Check if simple overwrite is good enough
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
            // screen pixels using the current video mode's dimensions.
            if cpu.bus.mouse.installed && cpu.bus.mouse.hide_counter <= 0 {
                let (mode_w, mode_h) = cpu.bus.video_mode.dimensions();
                let virt_w = if mode_w < 640 { 640 } else { mode_w };
                let virt_h = mode_h;
                let sx = (cpu.bus.mouse.x as i64 * video::SCREEN_WIDTH as i64 / virt_w as i64)
                    as i32;
                let sy = (cpu.bus.mouse.y as i64 * video::SCREEN_HEIGHT as i64 / virt_h as i64)
                    as i32;
                draw_default_mouse_cursor(buffer, sx, sy);
            }

            // Send Frame to Recorder before drawing recording indicator
            recorder.capture(buffer);

            // Draw Recording Indicator
            if recorder.is_active() {
                let radius = 5;
                let center_x = video::SCREEN_WIDTH as usize - 15;
                let center_y = 15;

                for y in (center_y - radius)..=(center_y + radius) {
                    for x in (center_x - radius)..=(center_x + radius) {
                        let dx = x as isize - center_x as isize;
                        let dy = y as isize - center_y as isize;
                        if dx * dx + dy * dy <= (radius * radius) as isize {
                            let idx = (y * video::SCREEN_WIDTH as usize + x) * 3;
                            if idx + 2 < buffer.len() {
                                buffer[idx] = 0xFF; // R
                                buffer[idx + 1] = 0x00; // G
                                buffer[idx + 2] = 0x00; // B
                            }
                        }
                    }
                }
            }
        })?;
        canvas.copy(&texture, None, None)?;
        canvas.present();

        std::thread::sleep(Duration::from_millis(16));
    }

    Ok(())
}

/// Convert host window coordinates (in pixels, at window `scale`) into the
/// driver's virtual coordinate system for the current video mode. Most DOS
/// mouse drivers use a fixed virtual X range of 0..639 regardless of the
/// actual horizontal resolution; Y follows the mode's pixel height.
fn host_to_virtual_mouse(cpu: &Cpu, host_x: i32, host_y: i32, scale: u32) -> (i32, i32) {
    let (mode_w, mode_h) = cpu.bus.video_mode.dimensions();
    let scale = scale.max(1) as i32;
    // Undo the window scale. The textured output is SCREEN_WIDTH x SCREEN_HEIGHT.
    let px = (host_x / scale).clamp(0, video::SCREEN_WIDTH as i32 - 1);
    let py = (host_y / scale).clamp(0, video::SCREEN_HEIGHT as i32 - 1);

    // Virtual X axis: 640 wide for 320-wide modes too (standard DOS convention).
    let virt_w = if mode_w < 640 { 640 } else { mode_w as i32 };
    let virt_h = mode_h as i32;

    let vx = (px as i64 * virt_w as i64 / video::SCREEN_WIDTH as i64) as i32;
    let vy = (py as i64 * virt_h as i64 / video::SCREEN_HEIGHT as i64) as i32;
    (vx.clamp(0, virt_w - 1), vy.clamp(0, virt_h - 1))
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

fn draw_default_mouse_cursor(buffer: &mut [u8], origin_x: i32, origin_y: i32) {
    for (row_idx, row) in CURSOR_ARROW.iter().enumerate() {
        for (col_idx, &cell) in row.iter().enumerate() {
            if cell == 0 {
                continue;
            }
            let x = origin_x + col_idx as i32;
            let y = origin_y + row_idx as i32;
            if x < 0
                || y < 0
                || x >= video::SCREEN_WIDTH as i32
                || y >= video::SCREEN_HEIGHT as i32
            {
                continue;
            }
            let idx = (y as usize * video::SCREEN_WIDTH as usize + x as usize) * 3;
            if idx + 2 >= buffer.len() {
                continue;
            }
            let (r, g, b) = if cell == 1 {
                (0xFF, 0xFF, 0xFF)
            } else {
                (0x00, 0x00, 0x00)
            };
            buffer[idx] = r;
            buffer[idx + 1] = g;
            buffer[idx + 2] = b;
        }
    }
}
