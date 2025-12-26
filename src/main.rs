use iced_x86::{Decoder, DecoderOptions, Mnemonic};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;
use std::time::Duration;
use std::io::Write;

use crate::audio::pump_audio;
use crate::cpu::{Cpu, CpuState};
use crate::command::CommandDispatcher;

mod audio;
mod bus;
mod command;
mod cpu;
mod instructions;
mod disk;
mod interrupts;
mod shell;
mod video;

fn main() -> Result<(), String> {
    let mut debug_mode = true;

    let mut cursor_visible = true;
    let mut last_blink = std::time::Instant::now();
    let blink_interval = Duration::from_millis(500);

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
                    ..
                } => {
                    // Convert SDL Keycode to ASCII
                    // This is a VERY simplified mapping
                    let ascii = match keycode {
                        Keycode::F1 => {
                            debug_mode = !debug_mode;
                            cpu.bus.log_string(&format!("[DEBUG] Tracing: {}", if debug_mode { "ON" } else { "OFF" }));
                            0
                        }
                        Keycode::A => b'a',
                        Keycode::B => b'b',
                        Keycode::C => b'c',
                        Keycode::D => b'd',
                        Keycode::E => b'e',
                        Keycode::F => b'f',
                        Keycode::G => b'g',
                        Keycode::H => b'h',
                        Keycode::I => b'i',
                        Keycode::J => b'j',
                        Keycode::K => b'k',
                        Keycode::L => b'l',
                        Keycode::M => b'm',
                        Keycode::N => b'n',
                        Keycode::O => b'o',
                        Keycode::P => b'p',
                        Keycode::Q => b'q',
                        Keycode::R => b'r',
                        Keycode::S => b's',
                        Keycode::T => b't',
                        Keycode::U => b'u',
                        Keycode::V => b'v',
                        Keycode::W => b'w',
                        Keycode::X => b'x',
                        Keycode::Y => b'y',
                        Keycode::Z => b'z',
                        Keycode::Space => b' ',
                        Keycode::Return => 0x0D,
                        Keycode::Backspace => 0x08,
                        Keycode::Period => b'.',
                        Keycode::Kp0 | Keycode::Num0 => b'0',
                        Keycode::Kp1 | Keycode::Num1 => b'1',
                        Keycode::Kp2 | Keycode::Num2 => b'2',
                        Keycode::Kp3 | Keycode::Num3 => b'3',
                        Keycode::Kp4 | Keycode::Num4 => b'4',
                        Keycode::Kp5 | Keycode::Num5 => b'5',
                        Keycode::Kp6 | Keycode::Num6 => b'6',
                        Keycode::Kp7 | Keycode::Num7 => b'7',
                        Keycode::Kp8 | Keycode::Num8 => b'8',
                        Keycode::Kp9 | Keycode::Num9 => b'9',
                        _ => 0,
                    };

                    if ascii != 0 {
                        // Push to CPU Keyboard Buffer (Scan=0 for simplicity)
                        cpu.bus.keyboard_buffer.push_back(ascii as u16);
                    }
                }
                _ => {}
            }
        }

        // Execute instructions
        for _ in 0..30_000 {


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
        
                let old_ip = cpu.ip;
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
            // TODO: Make this work across video modes
            
            // Read Cursor Position from BDA
            let cursor_col = cpu.bus.read_8(0x0450) as usize;
            let cursor_row = cpu.bus.read_8(0x0451) as usize;
            
            // Read Cursor Shape from BDA
            let cursor_shape = cpu.bus.read_16(0x0460);
            let start_scan = (cursor_shape >> 8) as u8;
            let end_scan = (cursor_shape & 0xFF) as u8;

            // Bit 5 of Start Scanline indicates "Invisible" in VGA hardware
            let is_hidden = (start_scan & 0x20) != 0;

            if cursor_visible && !is_hidden && cursor_col < 80 && cursor_row < 25 {
                let cell_width = 8;
                let cell_height = 16;
                
                // Calculate screen coordinates
                let start_x = cursor_col * cell_width;
                let start_y = cursor_row * cell_height;

                // Sanitize scanlines to prevent buffer overflows
                let scan_start = (start_scan & 0x1F).min(15) as usize; // Mask out visibility bit
                let scan_end = end_scan.min(15) as usize;

                // Draw the cursor lines
                // Note: DOS cursors can wrap (start > end), effectively drawing two blocks.
                // For simplicity, we implement the standard range logic here.
                if scan_start <= scan_end {
                    for y_off in scan_start..=scan_end {
                        for x_off in 0..cell_width {
                            let draw_x = start_x + x_off;
                            let draw_y = start_y + y_off;

                            // Calculate buffer index (RGB24 = 3 bytes per pixel)
                            let offset = (draw_y * (video::SCREEN_WIDTH as usize) + draw_x) * 3;
                            
                            if offset + 2 < buffer.len() {
                                // Draw Cursor Color (Usually bright light gray or white)
                                // We use 0xDD to allow the text behind to remain slightly distinct 
                                // if we implemented XOR, but for a solid block, we just overwrite.
                                buffer[offset] = 0xDD;     // R
                                buffer[offset + 1] = 0xDD; // G
                                buffer[offset + 2] = 0xDD; // B
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
