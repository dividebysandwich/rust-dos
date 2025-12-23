use iced_x86::{Decoder, DecoderOptions, Mnemonic};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;
use std::time::Duration;
use std::io::Write;

use crate::audio::pump_audio;
use crate::bus::Bus;
use crate::cpu::{Cpu, CpuState};

mod audio;
mod bus;
mod command;
mod cpu;
mod cpu_instr;
mod disk;
mod interrupt;
mod video;

/// A Tiny "OS" written in Machine Code. Reads keys into a buffer at offset 0x0200
/// On Enter, calls INT 20h (Our Rust Shell). Handles backspace visually and in buffer
fn get_shell_code() -> Vec<u8> {
    vec![
        // ----------------------------------------------------
        // BOOTLOADER: Initialize Segments
        // ----------------------------------------------------
        0x31, 0xC0, // XOR AX, AX
        0x8E, 0xD8, // MOV DS, AX
        0x8E, 0xC0, // MOV ES, AX
        0x8E, 0xD0, // MOV SS, AX
        0xBC, 0x00, 0xFF, // MOV SP, 0xFF00
        // ----------------------------------------------------
        // SHELL: Setup
        // ----------------------------------------------------
        0xBE, 0x00, 0x02, // MOV SI, 0x0200 (Buffer Start)
        // ----------------------------------------------------
        // MAIN INPUT LOOP (Offset 0x10E in memory)
        // ----------------------------------------------------
        // Reset AH to 00 (Read Key Mode)
        0xB4, 0x00, // MOV AH, 00h
        0xCD, 0x16, // INT 16h (Wait for Key)
        // Check ENTER (0x0D)
        0x3C, 0x0D, // CMP AL, 0x0D
        0x74, 0x1A, // JE EXECUTE (+26 bytes)
        // Check BACKSPACE (0x08)
        0x3C, 0x08, // CMP AL, 0x08
        0x74, 0x09, // JE HANDLE_BACKSPACE (+9 bytes)
        // Normal Character
        0xB4, 0x0E, // MOV AH, 0Eh (Teletype Output)
        0xCD, 0x10, // INT 10h (Print Char)
        0x88, 0x04, // MOV [SI], AL (Store in Buffer)
        0x46, // INC SI (Advance Pointer)
        // JMP LOOP_START
        // -21 bytes (0xEB) ensures we hit MOV AH, 00
        0xEB, 0xEB,
        // ----------------------------------------------------
        // BACKSPACE HANDLER
        // ----------------------------------------------------
        // Check Boundary
        0x81, 0xFE, 0x00, 0x02, // CMP SI, 0x0200
        0x74, 0xE5, // JE LOOP_START (Empty buffer? Restart)
        // Perform Backspace
        0x4E, // DEC SI (Move Pointer Back)
        0xB4, 0x0E, // MOV AH, 0Eh
        0xCD, 0x10, // INT 10h (Print BS)
        // -34 bytes (0xDE) ensures we hit MOV AH, 00
        0xEB, 0xDE, // JMP LOOP_START
        // ----------------------------------------------------
        // EXECUTE COMMAND
        // ----------------------------------------------------
        0xC6, 0x04, 0x00, // MOV BYTE PTR [SI], 0
        0xBA, 0x00, 0x02, // MOV DX, 0x0200
        0xCD, 0x20, // INT 20h
        // ----------------------------------------------------
        // RESET
        // ----------------------------------------------------
        0xBE, 0x00, 0x02, // MOV SI, 0x0200
        0xEB, 0xD1, // JMP LOOP_START (-47 bytes)
    ]
}

fn main() -> Result<(), String> {
    let mut debug_mode = true;

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

    video::print_string(&mut cpu, "C:\\>");

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

            // Handle "IP = 0" as an explicit exit (Standard COM behavior)
            // If the program jumps to the start of the segment, it wants to exit.
            if cpu.ip == 0x0000 && cpu.cs == 0x1000 {
                cpu.bus.log_string("[DOS] Program jumped to offset 0000h. Exiting to Shell.");
                // Flush log on exit so we don't lose tail data
                let _ = cpu.bus.log_file.as_mut().unwrap().flush();
                cpu.load_shell(); 
                cpu.state = CpuState::Running;
                video::print_string(&mut cpu, "\r\nC:\\>");
                break;
            }

            let phys_ip = cpu.get_physical_addr(cpu.cs, cpu.ip);
            let bytes = &cpu.bus.ram[phys_ip..];

            // If we are about to execute 00 00, stop immediately.
            if bytes.len() >= 2 && bytes[0] == 0x00 && bytes[1] == 0x00 {
                // panic!(
                //     "[CRITICAL] CPU hit 00 00 (Empty RAM) at {:04X}:{:04X}",
                //     cpu.cs, cpu.ip
                // );
            }

            let mut decoder = Decoder::with_ip(16, bytes, cpu.ip as u64, DecoderOptions::NONE);
            let instr = decoder.decode();

            if debug_mode {
                // Filter out the 'Wait for Key' interrupt loop to save disk space
                if !((instr.mnemonic() == Mnemonic::Int && instr.immediate8() == 0x16) ||
                     (instr.mnemonic() == Mnemonic::Jmp && instr.near_branch16() == 0x10E)) 
                {
                    // Format the instruction string manually since we can't capture stdout
                    // (Assuming you want the same format as print_debug_trace)
                    let instr_text = format!("{}", instr);
                    let log_line = format!(
                        "{:04X}:{:04X}  AX:{:04X} BX:{:04X} CX:{:04X} DX:{:04X} SP:{:04X}  {}\n",
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

            cpu.ip = instr.next_ip() as u16;

            // Check State
            if cpu.state == CpuState::RebootShell {
                cpu.load_shell(); // Reloads assembly into RAM, resets IP/SP
                cpu.state = CpuState::Running;
                video::print_string(&mut cpu, "\r\nC:\\>");
                break; // Break inner execution batch
            }

            cpu_instr::execute_instruction(&mut cpu, &instr);
        }


        // Update Audio
        pump_audio(&mut cpu.bus);

        // Render Frame
        // Note: We redraw every frame here for simplicity, even if VRAM isn't dirty
        texture.with_lock(None, |buffer: &mut [u8], _: usize| {
            video::render_screen(buffer, &cpu.bus);
        })?;
        canvas.copy(&texture, None, None)?;
        canvas.present();

        std::thread::sleep(Duration::from_millis(16));
    }

    Ok(())
}
