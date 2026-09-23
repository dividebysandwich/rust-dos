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

mod adlib;
mod audio;
mod bus;
mod command;
mod config;
mod cpu;
mod debug;
mod disk;
mod f80;
mod instr_cache;
mod instructions;
mod interrupts;
mod keyboard;
mod mcb;
mod mount;
mod mouse;
mod recorder;
mod sb;
mod shell;
mod timer;
mod video;

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
            video::SCREEN_WIDTH * scale,
            video::SCREEN_HEIGHT * scale,
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

    let mut cpu = create_cpu(&args, &config);
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

    // Decoded-instruction cache. 64K direct-mapped slots (~3.5 MB) — comfortably
    // large for any DOS program's hot working set, and small enough to fit in
    // modern L2/L3. See `instr_cache.rs` for the SMC-invalidation scheme.
    let mut instr_cache = instr_cache::InstrCache::new(16);

    // Cached render target. We re-render the full VGA surface only when
    // `cpu.bus.vga.dirty` is set — everything else (cursor blink, mouse
    // cursor, recording indicator) is overlaid on top of this buffer each
    // frame. For a program that isn't actively touching VRAM, the per-SDL-
    // frame cost drops from "640×400×3 zero fill + per-pixel palette/planar
    // lookup" to "one memcpy of the cached buffer + a tiny overlay pass".
    let mut cached_frame: Vec<u8> =
        vec![0u8; (video::SCREEN_WIDTH * video::SCREEN_HEIGHT * 3) as usize];

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
                        keyboard::deliver_key_down(&mut cpu.bus, code);
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
                        keyboard::deliver_key_up(&mut cpu.bus, (code >> 8) as u8);
                    }
                }

                Event::MouseMotion { x, y, .. } => {
                    let (vx, vy) = host_to_virtual_mouse(&cpu, x, y, scale);
                    cpu.bus.mouse.set_position(vx, vy);
                }

                Event::MouseButtonDown { mouse_btn, x, y, .. } => {
                    let (vx, vy) = host_to_virtual_mouse(&cpu, x, y, scale);
                    cpu.bus.mouse.set_position(vx, vy);
                    if let Some(btn) = sdl_button_to_index(mouse_btn) {
                        cpu.bus.mouse.button_down(btn);
                    }
                }

                Event::MouseButtonUp { mouse_btn, x, y, .. } => {
                    let (vx, vy) = host_to_virtual_mouse(&cpu, x, y, scale);
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
        // Instructions skipped while the CPU waited for an interrupt.
        let mut idle_instructions = 0u64;

        // Hoist the iced_x86 decoder OUT of the inner loop. Creating a fresh
        // `Decoder::with_ip` every instruction did not allocate heap, but it
        // re-initialized a ~256-byte struct for every decode. Reusing a
        // single decoder and calling `decode_out` straight into the
        // instruction cache slot avoids that and any stack copies.
        //
        // SAFETY: we build an overlapping read-only view of `cpu.bus.ram`
        // that outlives the subsequent mutable borrows (execute_instruction
        // can write to ram via bus.write_8). This is sound in our setup
        // because:
        //   1. `cpu.bus.ram` is a `Vec<u8>` of fixed 1 MiB capacity that is
        //      never resized after `Bus::new()`, so the pointer stays valid.
        //   2. Emulation is single-threaded, so no concurrent access occurs.
        //   3. Reads go through the decoder's slice; writes go through
        //      `bus.write_8`. The operations are strictly sequential inside
        //      this loop, never overlapping, and `u8` has no alignment or
        //      niche requirements that would be violated by aliasing.
        //   4. Self-modifying code (LZEXE decompressors, etc.) works because
        //      the decoder reads the ram bytes *at decode time* — writes
        //      performed on prior iterations are visible on the next decode.
        let (ram_ptr, ram_len) = (cpu.bus.ram.as_ptr(), cpu.bus.ram.len());
        let ram_slice: &'static [u8] = unsafe { std::slice::from_raw_parts(ram_ptr, ram_len) };
        let mut decoder = Decoder::with_ip(16, ram_slice, 0, DecoderOptions::NONE);

        // Per-instruction debug hook (breakpoints / stepping / tracing) is
        // only consulted when something actually needs it.
        let dbg_hot = dbg.begin_batch(&cpu);

        loop {
            // Timer events, and the end of the batch.
            if cpu.bus.clock.icount >= cpu.bus.clock.deadline {
                if cpu.bus.clock.icount >= cpu.bus.clock.batch_end() {
                    break;
                }
                cpu.bus.service_timers();
            }

            // Deliver pending hardware IRQs at the start of each instruction
            // as a CPU would, but only when the program has IF=1 (interrupts
            // enabled) and the PIC lets the line through: not masked in the
            // IMR and not blocked by an interrupt still in service.
            if cpu.get_cpu_flag(CpuFlags::IF) {
                // IRQ 0 (timer), IRQ 1 (keyboard) and IRQ 5 (Sound Blaster),
                // highest priority first. The Sound Blaster ISR acks IRQ 5 by
                // reading port 0x22E, which clears irq_pending via
                // Bus::io_read.
                if let Some(line) = cpu.bus.pic_pending_irq() {
                    let ivt = (0x08 + line as usize) * 4;
                    let handler_ip = cpu.bus.read_16(ivt);
                    let handler_cs = cpu.bus.read_16(ivt + 2);
                    if handler_cs != 0 || handler_ip != 0 {
                        cpu.bus.pic_acknowledge(line);
                        cpu.push(cpu.get_cpu_flags().bits());
                        cpu.push(cpu.cs);
                        cpu.push(cpu.ip);
                        cpu.cs = handler_cs;
                        cpu.ip = handler_ip;
                        cpu.set_cpu_flag(CpuFlags::IF, false);
                        cpu.set_cpu_flag(CpuFlags::TF, false);
                    } else {
                        // No handler installed — drop the IRQ rather than
                        // spinning on it.
                        cpu.bus.pic_drop(line);
                    }
                    continue;
                }

                // Mouse event handler installed with INT 33h AX=000C.
                if mouse::deliver_callback(&mut cpu) {
                    continue;
                }
            }

            // --- INJECT NEXT BATCH LINE ---
            // If we're idle in shell-land (no child program on the stack and CS
            // still in the shell segment) and there are batch lines pending,
            // pop one and feed it through the same path the shell uses for a
            // typed command. We echo the line at a synthesized prompt so the
            // user sees what's running, MS-DOS style. A leading '@' suppresses
            // the echo.
            if cpu.cs == 0
                && cpu.pending_command.is_none()
                && !cpu.batch_queue.is_empty()
                && cpu.process_stack.is_empty()
            {
                let raw = cpu.batch_queue.pop_front().unwrap();
                let (line, line_echo) = if let Some(stripped) = raw.strip_prefix('@') {
                    (stripped.trim().to_string(), false)
                } else {
                    (raw, true)
                };
                if !line.is_empty() {
                    // Per-line @ suppresses the echo for that line only;
                    // ECHO OFF (cpu.batch_echo) suppresses all subsequent lines.
                    if line_echo && cpu.batch_echo {
                        shell::show_prompt(&mut cpu);
                        crate::video::print_string(&mut cpu, &format!("{}\r\n", line));
                    }
                    cpu.pending_command = Some(line);
                }
            }

            // --- HANDLE PENDING COMMANDS (Outside Interrupts) ---
            // (Checked before taking it: `take` would store None back on
            // every instruction.)
            if cpu.pending_command.is_some() {
                let cmd = cpu.pending_command.take().unwrap();
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
                    // Load Program. With no extension we probe .com, .exe,
                    // then .bat — matching DOS's COMMAND.COM precedence.
                    let filename = command.to_string();
                    let lower = filename.to_lowercase();
                    let loaded = if lower.ends_with(".bat") {
                        cpu.queue_batch_file(&filename)
                    } else if !filename.contains('.') {
                        cpu.load_executable(&format!("{}.com", command), None)
                            || cpu.load_executable(&format!("{}.exe", command), None)
                            || cpu.queue_batch_file(&format!("{}.bat", command))
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
            if cpu.ip == 0x0000 && cpu.cs == cpu.transient_segment() {
                cpu.bus
                    .log_string("[DOS] Program jumped to offset 0000h. Exiting to Shell.");
                // Flush log on exit so we don't lose tail data
                let _ = cpu.bus.log_file.as_mut().unwrap().flush();
                cpu.load_shell();
                cpu.state = CpuState::Running;
                // Skip the courtesy prompt while a batch is still feeding
                // commands — the batch injector prints its own prompt+echo
                // for each line, so a stray "C:\>" here would double up.
                if cpu.batch_queue.is_empty() {
                    shell::show_prompt(&mut cpu);
                }
                break;
            }

            // Current instruction. Direct indexing into ram — no VGA range
            // check needed for code fetch, code segments are always below
            // 0xA0000 in our loaded programs.
            let phys_ip = cpu.get_physical_addr(cpu.cs, cpu.ip);

            // Tripwire: arriving in the IVT / BIOS data area with an
            // application context (DS not 0, not the shell at CS=0) almost
            // always means a corrupted FAR pointer landed us here.
            if cpu.cs == 0 && cpu.ip < 0x100 && cpu.ds != 0 && cpu.ds != cpu.transient_segment() {
                cpu.bus.log_string(&format!(
                    "[TRIPWIRE] Entered IVT region CS:IP={:04X}:{:04X} DS={:04X} ES={:04X} SS:SP={:04X}:{:04X} AX={:04X} BX={:04X} CX={:04X} DX={:04X}",
                    cpu.cs, cpu.ip, cpu.ds, cpu.es, cpu.ss, cpu.sp, cpu.ax, cpu.bx, cpu.cx, cpu.dx
                ));
                // Dump 64 bytes of stack so we can see remaining return
                // addresses (anything the RETF left unconsumed).
                let ss_base = (cpu.ss as usize) * 16;
                let mut sbytes = String::new();
                for i in 0..64 {
                    let a = ss_base + cpu.sp as usize + i;
                    if a < cpu.bus.ram.len() {
                        sbytes.push_str(&format!("{:02X} ", cpu.bus.ram[a]));
                    }
                }
                cpu.bus.log_string(&format!(
                    "[TRIPWIRE] stack@SS:SP ({:04X}:{:04X}): {}",
                    cpu.ss, cpu.sp, sbytes.trim()
                ));
                let _ = cpu.bus.log_file.as_mut().unwrap().flush();
                cpu.state = CpuState::RebootShell;
                break;
            }

            if dbg_hot && dbg.before_exec(&cpu, phys_ip, ram_slice) {
                break;
            }
            dbg.icount += 1;

            let b0 = ram_slice[phys_ip];
            let b1 = ram_slice[phys_ip + 1];

            // Check for "BOP" (BIOS Operation) -> FE 38 XX
            if b0 == 0xFE && b1 == 0x38 {
                let vector = ram_slice[phys_ip + 2];

                // Run the HLE handler directly, then simulate its IRET
                crate::interrupts::handle_hle(&mut cpu, vector);
                crate::interrupts::return_from_hle(&mut cpu, vector);

                cpu.bus.clock.icount += 1;
                if cpu.idle {
                    // A BIOS service is waiting for input: skip ahead to the
                    // next timer event instead of spinning on the retry.
                    cpu.idle = false;
                    idle_instructions += cpu.bus.clock.skip_to_deadline();
                }
                continue; // Done for this cycle
            }

            // Decode via the decoded-instruction cache. On a hit (the common
            // case inside hot loops) we skip iced's decode path entirely and
            // use the stored Instruction in place. On a miss the reused
            // decoder fills the cache slot.
            let page_gen = cpu.bus.page_gen[(phys_ip >> 12) & 0xFF];
            let ip = cpu.ip;
            let instr = instr_cache.get_or_decode(phys_ip, cpu.cs, ip, page_gen, |slot| {
                decoder.set_position(phys_ip).unwrap();
                decoder.set_ip(ip as u64);
                decoder.decode_out(slot);
            });

            if debug_mode {
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

            cpu.ip = instr.next_ip() as u16;

            // Check State
            if cpu.state == CpuState::RebootShell {
                cpu.load_shell(); // Reloads assembly into RAM, resets IP/SP
                cpu.state = CpuState::Running;
                if cpu.batch_queue.is_empty() {
                    shell::show_prompt(&mut cpu);
                }
                break; // Break inner execution batch
            }

            // Make it so
            instructions::execute_instruction(&mut cpu, instr);
            cpu.bus.clock.icount += 1;

            if cpu.state == CpuState::Halted {
                // HLT: nothing runs until the next interrupt.
                cpu.state = CpuState::Running;
                idle_instructions += cpu.bus.clock.skip_to_deadline();
            }
        }

        dbg.end_batch(&cpu);
        let exec_time = batch_start.elapsed();
        let stalled = cpu.bus.clock.stalled - batch_stalled;
        let executed = cpu.bus.clock.icount - batch_icount - idle_instructions - stalled;
        dbg.record_batch(executed, exec_time, instr_cache.hits, instr_cache.misses);

        // Update Audio
        pump_audio(&mut cpu.bus);

        // Update Cursor Blink
        if last_blink.elapsed() >= blink_interval {
            cursor_visible = !cursor_visible;
            last_blink = std::time::Instant::now();
        }

        // Render Frame. The expensive part (the 640×400×3 pixel fill driven by
        // palette/planar lookups inside `render_screen`) only happens when the
        // VGA state changed since last frame. On "clean" frames we reuse
        // `cached_frame` and just overlay the cursor/mouse/recording pip.
        // Each frame is a vertical retrace: the CRTC picks up the Start
        // Address the program flipped to, whether or not it polled port 3DAh.
        cpu.bus.vga.latch_start_address();
        if cpu.bus.vga.dirty {
            video::render_screen(&mut cached_frame, &cpu.bus);
            cpu.bus.vga.clear_dirty();
        }

        texture.with_lock(None, |buffer: &mut [u8], _pitch: usize| {
            // Copy the cached render into the texture. This is a single
            // ~768 KiB memcpy — cheap on any modern machine — and leaves us a
            // clean canvas for the per-frame overlays (cursor/mouse/recorder
            // indicator) without re-running the VGA renderer.
            buffer.copy_from_slice(&cached_frame);

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
            // screen pixels over the same virtual extent the host pointer spans.
            if cpu.bus.mouse.installed && cpu.bus.mouse.hide_counter <= 0 {
                let (virt_w, virt_h) = cpu.bus.mouse.virtual_extent(cpu.bus.video_mode);
                let sx =
                    (cpu.bus.mouse.x as i64 * video::SCREEN_WIDTH as i64 / virt_w as i64) as i32;
                let sy =
                    (cpu.bus.mouse.y as i64 * video::SCREEN_HEIGHT as i64 / virt_h as i64) as i32;
                draw_default_mouse_cursor(buffer, sx, sy);
            }

            // Send Frame to Recorder / debug clients before drawing the
            // recording indicator
            recorder.capture(buffer);
            dbg.capture_frame(buffer);

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

        let overhead = frame_start.elapsed().saturating_sub(exec_time);
        if let Some(cycles) = pacer.end_frame(&cpu.bus.clock, executed, exec_time, overhead) {
            cpu.bus.set_cycles_per_ms(cycles);
        }
        pacer.wait_for_next_frame();
    }

    Ok(())
}

/// Find and parse the configuration file (see config.rs for the lookup
/// order). Problems in the file are reported but never stop the emulator;
/// only a missing `--config` file does.
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
    if let Some(path) = &config.source {
        let action = if config.created { "Created default" } else { "Using" };
        eprintln!("[CONFIG] {} configuration file {}", action, path.display());
    }
    for warning in &config.warnings {
        eprintln!("[CONFIG] Warning: {}", warning);
    }
    Ok(config)
}

/// Build the CPU with drive C: from `-d`, the config file or the working
/// directory (in that order), then mount the config's other drives.
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

    let mut cpu = Cpu::new(root_path.clone());
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

    for warning in &warnings {
        cpu.bus.log_string(&format!("[CONFIG] Warning: {}", warning));
    }
    cpu
}

/// Convert host window coordinates (in pixels, at window `scale`) into the
/// driver's virtual coordinate system (see `MouseState::virtual_extent`).
fn host_to_virtual_mouse(cpu: &Cpu, host_x: i32, host_y: i32, scale: u32) -> (i32, i32) {
    let scale = scale.max(1) as i32;
    // Undo the window scale. The textured output is SCREEN_WIDTH x SCREEN_HEIGHT.
    let px = (host_x / scale).clamp(0, video::SCREEN_WIDTH as i32 - 1);
    let py = (host_y / scale).clamp(0, video::SCREEN_HEIGHT as i32 - 1);

    let (virt_w, virt_h) = cpu.bus.mouse.virtual_extent(cpu.bus.video_mode);

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
