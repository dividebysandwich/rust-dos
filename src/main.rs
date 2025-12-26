use iced_x86::{Decoder, DecoderOptions, Mnemonic};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;
use std::time::Duration;
use std::io::Write;

use crate::audio::pump_audio;
use crate::cpu::{Cpu, CpuState};
use crate::command::CommandDispatcher;
use crate::recorder::ScreenRecorder;
use crate::video::VideoMode;

mod audio;
mod bus;
mod command;
mod cpu;
mod disk;
mod keyboard;
mod instructions;
mod interrupts;
mod recorder;
mod shell;
mod video;

fn main() -> Result<(), String> {
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
        .window("Rust DOS Emulator", video::SCREEN_WIDTH, video::SCREEN_HEIGHT)
        .position_centered()
        .build()
        .map_err(|e| e.to_string())?;

    let mut canvas = window.into_canvas().build().map_err(|e| e.to_string())?;
    let texture_creator = canvas.texture_creator();
    // Texture is always 640x400 RGB
    let mut texture = texture_creator
        .create_texture_streaming(PixelFormatEnum::RGB24, video::SCREEN_WIDTH, video::SCREEN_HEIGHT)
        .map_err(|e| e.to_string())?;

    let mut cpu = Cpu::new();
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
                        cpu.bus.log_string(&format!("[DEBUG] Tracing: {}", if debug_mode { "ON" } else { "OFF" }));
                        continue;
                    }

                    // Map Key to PC Scancode/ASCII
                    if let Some(code) = keyboard::map_sdl_to_pc(keycode, keymod) {
                        cpu.bus.keyboard_buffer.push_back(code);
                    }
                }
                // KeyUp only matters for modifiers                
                Event::KeyUp { 
                    keycode: Some(keycode), 
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
                }

                _ => {}
            }
        }

        // Execute instructions
        for _ in 0..30_000 {

            let prev_ip = cpu.ip;

            // --- HANDLE PENDING COMMANDS (Outside Interrupts) ---
            if let Some(cmd) = cpu.pending_command.take() {
                // We have a command from the shell!
                cpu.bus.log_string(&format!("[MAIN] Processing Command: {}", cmd));
                
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
                          cpu.load_executable(&format!("{}.com", command)) 
                          || cpu.load_executable(&format!("{}.exe", command))
                     } else {
                          cpu.load_executable(&filename)
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
                // No print needed here, shell handles prompt
                break; // Break inner loop to refresh SDL
            }

            // Handle "IP = 0" as an explicit exit (Standard COM behavior)
            // If the program jumps to the start of the segment, it wants to exit.
            if cpu.ip == 0x0000 && cpu.cs == 0x1000 {
                cpu.bus.log_string("[DOS] Program jumped to offset 0000h. Exiting to Shell.");
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

                // POP the flags to clear the stack, but ignore the value
                // We want to keep the Flags set by the Rust HLE Handler (like Carry Flag).
                let _popped_flags = cpu.pop();

                // Ensure reserved bits (1, 3, 5, 15) are set correctly, 
                // but preserve the Condition Codes (CF, ZF, etc) from the HLE handler.
                cpu.flags = (cpu.flags & 0x0FD5) | 0x0002;
        
                continue; // Done for this cycle
            }

            let mut decoder = Decoder::with_ip(16, bytes, cpu.ip as u64, DecoderOptions::NONE);
            let instr = decoder.decode();

            if debug_mode {
                // Filter out the 'Wait for Key' interrupt loop to save disk space
                if !((instr.mnemonic() == Mnemonic::Int && instr.immediate8() == 0x16) ||
                     (instr.mnemonic() == Mnemonic::Jmp && instr.near_branch16() == 0x10E))
                {
                    // Skip BIOS area noise
                    if cpu.cs < 0xF000 {
                        // Format the instruction string manually since we can't capture stdout
                        // (Assuming you want the same format as print_debug_trace)
                        let instr_text = format!("{}", instr);
                        let log_line = format!(
                            "{:04X}:{:04X}  AX:{:04X} BX:{:04X} CX:{:04X} DX:{:04X} SP:{:04X}  {}",
                            cpu.cs, cpu.ip,
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
                VideoMode::Text80x25 | VideoMode::Text80x25Color | 
                VideoMode::Text40x25 | VideoMode::Text40x25Color
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

                if cursor_visible && !is_hidden && cursor_col < max_cols && cursor_row < 25 {
                    let cell_height = 16;
            
                    // Calculate screen coordinates
                    let start_x = cursor_col * cell_width;
                    let start_y = cursor_row * cell_height;

                    // Clamp scanlines
                    let scan_start = (start_scan & 0x1F).min(15) as usize; 
                    let scan_end = end_scan.min(15) as usize;

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
                        if dx*dx + dy*dy <= (radius*radius) as isize {
                            let idx = (y * video::SCREEN_WIDTH as usize + x) * 3;
                            if idx + 2 < buffer.len() {
                                buffer[idx] = 0xFF;   // R
                                buffer[idx+1] = 0x00; // G
                                buffer[idx+2] = 0x00; // B
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
