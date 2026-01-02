use chrono::{Local, Timelike};
use iced_x86::Register;

use super::utils::{pattern_to_fcb, read_asciiz_string, read_dta_template};
use crate::audio::play_sdl_beep;
use crate::cpu::{Cpu, CpuFlags, CpuState};
use crate::video::print_char;

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    match ah {
        // AH = 0Eh: Select Default Drive
        0x0E => {
            let drive = cpu.get_dl();
            // set_current_drive returns total logical drives (26)
            let logical_drives = cpu.bus.disk.set_current_drive(drive);
            cpu.set_reg8(Register::AL, logical_drives);
        }

        // AH = 00h: Terminate Program (Legacy Method)
        0x00 => {
            cpu.bus
                .log_string("[DOS] Program Terminated (Legacy INT 20h/21h AH=00).");

            if cpu.restore_process_context() {
                cpu.bus
                    .log_string("[DOS] AH=00: Returning to Parent Process");
            } else {
                cpu.state = CpuState::RebootShell;
            }
        }

        // AH=11h (Find First FCB) / AH=12h (Find Next FCB)
        0x11 | 0x12 => {
            let dta_current = cpu.bus.dta_segment;
            let dta_off = cpu.bus.dta_offset;
            let dta_phys = cpu.get_physical_addr(dta_current, dta_off);

            let fcb_addr = cpu.get_physical_addr(cpu.ds, cpu.dx);

            let (index, pattern) = if ah == 0x11 {
                let p = read_dta_template(&cpu.bus, fcb_addr); // Reusing helper
                (0, p)
            } else {
                // Read index from FCB reserved area (Offset 0x0C)
                let idx = cpu.bus.read_16(fcb_addr + 0x0C) as usize;

                let p = read_dta_template(&cpu.bus, fcb_addr);
                (idx, p)
            };

            cpu.bus.log_string(&format!(
                "[DOS] FCB Find{:02X}: Pattern='{}' Index={}",
                ah, pattern, index
            ));

            let search_attr = 0x10; // Directory + Archive + ReadOnly (Implicit for FCB?)

            match cpu
                .bus
                .disk
                .find_directory_entry(&pattern, index, search_attr)
            {
                Ok(entry) => {
                    // Success: AL=00
                    cpu.set_reg8(Register::AL, 0x00);

                    // Write Result to DTA (Not DS:DX? Or implicitly DTA?)
                    // "The DTA is filled with..."
                    // Ensure we write to DTA, not back to DS:DX (unless they are same).

                    cpu.bus.write_8(dta_phys + 0, 1); // Drive A: (Simulated) or 0? 
                    // Valid drive for C: is 3? No, FCB: 0=Default, 1=A, 3=C.
                    // Let's write 0 (Default) or 3.
                    cpu.bus.write_8(dta_phys + 0, 3);

                    // Write Filename to DTA+1 (11 bytes)
                    let fcb_bytes = pattern_to_fcb(&entry.filename);
                    for i in 0..11 {
                        cpu.bus.write_8(dta_phys + 1 + i, fcb_bytes[i]);
                    }

                    // Store Index for Next Call at Input FCB Reserved Area (Offset 0x0C)
                    // This allows FindNext to know where to resume, even if DTA != Input FCB
                    cpu.bus.write_16(fcb_addr + 0x0C, (index + 1) as u16);

                    // Fill other stats?
                    // FCB: 16h=Time, 14h=Date, 10h=Size
                    cpu.bus.write_16(dta_phys + 0x16, entry.dos_time);
                    cpu.bus.write_16(dta_phys + 0x14, entry.dos_date);
                    cpu.bus.write_32(dta_phys + 0x10, entry.size);
                }
                Err(_) => {
                    // Failure: AL=FFh
                    cpu.set_reg8(Register::AL, 0xFF);
                }
            }
        }

        // AH = 02h: Output Character (DL = Char)
        0x02 => {
            let char_byte = cpu.get_dl();
            if char_byte == 0x07 {
                play_sdl_beep(&mut cpu.bus);
            } else {
                print_char(&mut cpu.bus, char_byte);
            }
            cpu.set_reg8(Register::AL, char_byte);
        }

        // AH = 06h: Direct Console I/O
        0x06 => {
            let dl = cpu.get_reg8(Register::DL);

            if dl == 0xFF {
                // --- INPUT (Non-Blocking) ---
                // Check if a key is in the buffer
                if let Some(key_code) = cpu.bus.keyboard_buffer.pop_front() {
                    // Key Available: Return ASCII and Clear Zero Flag
                    let ascii = (key_code & 0xFF) as u8;

                    // Handle Extended Keys (First byte is 0x00) logic if necessary,
                    // but for now we just return the low byte.
                    cpu.set_reg8(Register::AL, ascii);
                    cpu.set_zflag(false);
                } else {
                    // No Key: Return 0 and Set Zero Flag
                    cpu.set_reg8(Register::AL, 0x00);
                    cpu.set_zflag(true);
                }
            } else {
                // --- OUTPUT ---
                // Write character in DL to screen
                if dl == 0x07 {
                    play_sdl_beep(&mut cpu.bus);
                } else {
                    print_char(&mut cpu.bus, dl);
                }
                // AL is officially undefined on output, but we leave it alone.
            }
        }

        // AH = 07h: Direct Console Input Without Echo
        0x07 => {
            if let Some(key_code) = cpu.bus.keyboard_buffer.pop_front() {
                let ascii = (key_code & 0xFF) as u8;
                cpu.set_reg8(Register::AL, ascii);
            } else {
                // Retry logic
                // Calculate Physical Address of the Stack Pointer (SS:SP)
                let sp = cpu.sp;
                let ss = cpu.ss;
                let phys_sp = (ss as usize * 16) + sp as usize; // TODO: Check phys addr calc

                let saved_ip = cpu.bus.read_16(phys_sp & 0xFFFFF);

                // Substract 4 and make the CPU re-execute the trap instruction after returning.
                cpu.bus
                    .write_16(phys_sp & 0xFFFFF, saved_ip.wrapping_sub(4));
            }
        }

        // AH = 09h: Print String (Ends in '$')
        0x09 => {
            let mut offset = cpu.dx;
            loop {
                let char_byte = cpu.bus.read_8(cpu.get_physical_addr(cpu.ds, offset));
                if char_byte == b'$' {
                    break;
                }

                if char_byte == 0x07 {
                    play_sdl_beep(&mut cpu.bus);
                } else {
                    print_char(&mut cpu.bus, char_byte);
                }
                offset += 1;
            }
        }

        // AH = 0Ch: Clear Keyboard Buffer and Invoke Keyboard Function
        // AL = Function to execute after clearing (1, 6, 7, 8, 0xA)
        0x0C => {
            let next_fn = cpu.get_al();

            cpu.bus.keyboard_buffer.clear();

            match next_fn {
                0x01 | 0x06 | 0x07 | 0x08 | 0x0A => {
                    // Set AH to the next function and recurse
                    cpu.set_reg8(Register::AH, next_fn);
                    handle(cpu);
                }
                _ => {
                    // If AL is 0 or invalid, just return after clearing
                    cpu.set_reg8(Register::AL, 0);
                }
            }
        }

        // AH=19h: Get Current Default Drive
        0x19 => {
            // Return Default Drive (0=A, 1=B, 2=C)
            let drive = cpu.bus.disk.get_current_drive();
            cpu.set_reg8(Register::AL, drive);
        }

        // AH=1Ah: Set Disk Transfer Area (DTA) Address
        0x1A => {
            let ds = cpu.ds;
            let dx = cpu.get_reg16(Register::DX);
            cpu.bus.dta_segment = ds;
            cpu.bus.dta_offset = dx;
            cpu.bus
                .log_string(&format!("[DOS] Set DTA to {:04X}:{:04X}", ds, dx));
        }

        // AH = 29h: Parse Filename
        0x29 => {
            // DS:SI = Pointer to string
            // ES:DI = Pointer to FCB
            // AL = Bit mask (0x01=Leading separators, 0x02=Drive ID, 0x04=Ext, 0x08=Name)
            // For now we just do a basic implementation that reads the string and writes FCB

            let si = cpu.get_reg16(Register::SI);
            let str_addr = cpu.get_physical_addr(cpu.ds, si);
            let raw_str = read_asciiz_string(&cpu.bus, str_addr);

            // Extract first token (space separated)
            let token = raw_str.split_whitespace().next().unwrap_or("");

            if token.is_empty() {
                cpu.set_reg8(Register::AL, 0xFF); // Invalid
            } else {
                let fcb = pattern_to_fcb(token);
                let di = cpu.get_reg16(Register::DI);
                let fcb_addr = cpu.get_physical_addr(cpu.es, di);

                // Drive byte (0 = default) - Simplified
                // If token string starts with "C:", "A:", etc we could parse it.
                // pattern_to_fcb just handles name.ext.

                // Check drive
                let drive = if token.len() > 1 && token.chars().nth(1) == Some(':') {
                    let d = token.chars().next().unwrap().to_ascii_uppercase();
                    if d >= 'A' && d <= 'Z' {
                        (d as u8) - b'A' + 1
                    } else {
                        0
                    }
                } else {
                    0 // No change / Default
                };

                cpu.bus.write_8(fcb_addr, drive);

                for i in 0..11 {
                    cpu.bus.write_8(fcb_addr + 1 + i, fcb[i]);
                }

                cpu.set_reg8(Register::AL, 0x01); // No wildcards? Or 00? 
                // AL=1 if wildcard
                if token.contains('*') || token.contains('?') {
                    cpu.set_reg8(Register::AL, 0x01);
                } else {
                    cpu.set_reg8(Register::AL, 0x00);
                }

                let new_si = si.wrapping_add(token.len() as u16);
                cpu.set_reg16(Register::SI, new_si);
            }
        }

        // AH = 25h: Set Interrupt Vector
        0x25 => {
            let int_num = cpu.get_al() as usize;
            let new_off = cpu.dx;
            let new_seg = cpu.ds;
            let phys_addr = int_num * 4;

            cpu.bus.write_8(phys_addr, (new_off & 0xFF) as u8);
            cpu.bus.write_8(phys_addr + 1, (new_off >> 8) as u8);
            cpu.bus.write_8(phys_addr + 2, (new_seg & 0xFF) as u8);
            cpu.bus.write_8(phys_addr + 3, (new_seg >> 8) as u8);

            cpu.bus.log_string(&format!(
                "[DOS] Hooked Interrupt {:02X} to {:04X}:{:04X}",
                int_num, new_seg, new_off
            ));
        }

        // AH = 4Bh: Load and Execute Program (EXEC)
        0x4B => {
            let mode = cpu.get_al();
            let name_addr = cpu.get_physical_addr(cpu.ds, cpu.dx);
            let filename = read_asciiz_string(&cpu.bus, name_addr);

            cpu.bus.log_string(&format!(
                "[DOS] EXEC AH=4B Name='{}' AL={:02X}",
                filename, mode
            ));

            if mode == 0x00 {
                // Load and Execute
                // ES:BX points to Parameter Block
                // Offset 00: Segment of environment (word)
                // Offset 02: Pointer to command line (dword) -> Write to PSP 80h
                // Offset 06: Pointer to FCB 1 (dword) -> Write to PSP 5Ch
                // Offset 0A: Pointer to FCB 2 (dword) -> Write to PSP 6Ch

                let param_block = cpu.bx; // Offset in ES
                let param_seg = cpu.es;
                let param_phys = cpu.get_physical_addr(param_seg, param_block);

                // Read Environment Segment
                let env_seg = cpu.bus.read_16(param_phys);

                // Read Command Line Pointer
                let cmd_off = cpu.bus.read_16(param_phys + 2);
                let cmd_seg = cpu.bus.read_16(param_phys + 4);

                cpu.bus.log_string(&format!(
                    "[DEBUG] EXEC ParamBlock: Env={:04X} Cmd={:04X}:{:04X}",
                    env_seg, cmd_seg, cmd_off
                ));

                // Read Enviroment Block
                // If EnvSeg is 0, we should inherit from parent (which means reading *current* PSP's env).
                // If non-zero, read until double-null.

                let actual_env_seg = if env_seg == 0 {
                    // Inherit from current PSP
                    // PSP Offset 0x2C contains the env segment
                    let current_psp = cpu.current_psp;
                    if current_psp == 0 {
                        // Startup case: No parent. Use 0.
                        0
                    } else {
                        let env_ptr = cpu.get_physical_addr(current_psp, 0x2C);
                        cpu.bus.read_16(env_ptr)
                    }
                } else {
                    env_seg
                };

                let mut env_block = Vec::new();

                if actual_env_seg != 0 {
                    let mut env_phys = cpu.get_physical_addr(actual_env_seg, 0);
                    loop {
                        let b = cpu.bus.read_8(env_phys);
                        env_block.push(b);
                        env_phys += 1;

                        // Check for Double Null termination
                        if env_block.len() >= 2 {
                            let last = env_block[env_block.len() - 1];
                            let prev = env_block[env_block.len() - 2];
                            if last == 0 && prev == 0 {
                                break;
                            }
                        }
                        // Safety cap
                        if env_block.len() > 32768 {
                            break;
                        }
                    }
                } else {
                    // Start from scratch if 0 (Top level process)
                    let default_env = b"PATH=C:\\\0COMSPEC=COMMAND.COM\0\0";
                    for &b in default_env {
                        env_block.push(b);
                    }
                }

                // Read Command Line Content BEFORE we nuke RAM
                let cmd_phys = cpu.get_physical_addr(cmd_seg, cmd_off);
                let mut cmd_tail = Vec::new();

                // Command tail format: [LEN][String...][CR]
                let len = cpu.bus.read_8(cmd_phys);
                for i in 0..len {
                    cmd_tail.push(cpu.bus.read_8(cmd_phys + 1 + i as usize));
                }

                let cmd_str = String::from_utf8_lossy(&cmd_tail);
                cpu.bus
                    .log_string(&format!("[DEBUG] EXEC CmdLine: '{}'", cmd_str));

                // Log Env Block content (first 64 bytes)
                let mut env_preview = String::new();
                for i in 0..std::cmp::min(env_block.len(), 128) {
                    let b = env_block[i];
                    if b >= 32 && b <= 126 {
                        env_preview.push(b as char);
                    } else if b == 0 {
                        env_preview.push_str("\\0");
                    } else {
                        env_preview.push('.');
                    }
                }
                cpu.bus
                    .log_string(&format!("[DEBUG] EXEC Env Content: {}", env_preview));

                // DOS 3.0+ Program Name Appending
                // After the double-null (00 00), we append:
                // 1. A word count (0x0001)
                // 2. The full path of the executable program (AsciiZ)
                // This allows the program to find its own directory (e.g., to load nc.mnu).
                if env_block.len() < 2
                    || env_block[env_block.len() - 1] != 0
                    || env_block[env_block.len() - 2] != 0
                {
                    env_block.push(0);
                    if env_block.len() == 1 || env_block[env_block.len() - 2] != 0 {
                        env_block.push(0);
                    }
                }

                // Append Word Count 0x0001 (Little Endian: 01 00)
                env_block.push(0x01);
                env_block.push(0x00);

                // Append Filename (FullPath)
                // Using `filename` from function arg which logs show is fully qualified (C:\NC3\NCMAIN.EXE)
                for b in filename.bytes() {
                    env_block.push(b);
                }
                env_block.push(0x00); // Null terminator for filename

                // DEBUG LOG: Verify what we appended
                let mut dbg_tail = String::new();
                // Last 50 bytes or so
                let start_chk = if env_block.len() > 60 {
                    env_block.len() - 60
                } else {
                    0
                };
                for i in start_chk..env_block.len() {
                    let b = env_block[i];
                    if b >= 32 && b <= 126 {
                        dbg_tail.push(b as char);
                    } else if b == 0 {
                        dbg_tail.push_str("\\0");
                    } else {
                        dbg_tail.push_str(&format!("\\x{:02X}", b));
                    }
                }
                cpu.bus
                    .log_string(&format!("[DEBUG] Env Tail: {}", dbg_tail));

                // Check for COMMAND.COM interception
                let upper_name = filename.to_ascii_uppercase();
                let (target_filename, target_cmd_tail_bytes) =
                    if upper_name.ends_with("COMMAND.COM") {
                        // Check for /C (execute string)
                        // cmd_str is derived from cmd_tail (which includes length byte at 0? No, cmd_tail is vec of bytes)
                        // In previous code:
                        // let len = cpu.bus.read_8(cmd_phys);
                        // for i in 0..len { cmd_tail.push(...) }
                        // So cmd_tail is just the string bytes (no len, no CR).

                        let full_cmd = String::from_utf8_lossy(&cmd_tail).to_string();
                        let trimmed = full_cmd.trim_start();

                        if trimmed.to_ascii_uppercase().starts_with("/C") {
                            // Extract program and args
                            // Format: /C program args
                            let after_c = trimmed[2..].trim_start();
                            // Get program name (up to first space)
                            let mut parts = after_c.splitn(2, char::is_whitespace);
                            let prog = parts.next().unwrap_or("");
                            let args = parts.next().unwrap_or("");

                            // Construct new command tail for the target program
                            // Standard DOS command tail: [LEN] [SPACE] [ARGS] [CR]

                            let mut new_tail = Vec::new();

                            if !args.is_empty() {
                                new_tail.push(b' ');
                                for b in args.bytes() {
                                    new_tail.push(b);
                                }
                            }
                            // Note: CR is added later by the write logic (cpu.bus.write_8(..., 0x0D))

                            cpu.bus.log_string(&format!(
                                "[DOS] Intercepted COMMAND.COM /C. Target='{}', Args='{}'",
                                prog, args
                            ));

                            (prog.to_string(), new_tail)
                        } else {
                            // Not /C? Just run COMMAND.COM (which is dummy)
                            // It will load the dummy Z:\COMMAND.COM which is NOPs.
                            (filename.clone(), cmd_tail.clone())
                        }
                    } else {
                        (filename.clone(), cmd_tail.clone())
                    };

                // Allocate memory for new process using heap_pointer
                // Align to next paragraph if needed (heap_pointer is already paragraph)
                let load_segment = cpu.heap_pointer;

                if cpu.load_executable(&target_filename, Some(load_segment)) {
                    let psp_phys = cpu.get_physical_addr(load_segment, 0);

                    // Write Environment Block
                    let env_seg = cpu.heap_pointer;
                    let env_paras = (env_block.len() + 15) / 16;
                    // Increment heap
                    cpu.heap_pointer += env_paras as u16 + 1; // +1 safety

                    let env_phys_dest = cpu.get_physical_addr(env_seg, 0);
                    for (i, &b) in env_block.iter().enumerate() {
                        cpu.bus.write_8(env_phys_dest + i, b);
                    }

                    // Now load program at *new* heap pointer
                    let new_env_seg = 0x0C00;
                    let new_env_phys = cpu.get_physical_addr(new_env_seg, 0);
                    for (i, &b) in env_block.iter().enumerate() {
                        cpu.bus.write_8(new_env_phys + i, b);
                    }

                    // Update PSP offset 0x2C (Environment Segment)
                    cpu.bus.write_16(psp_phys + 0x2C, new_env_seg);

                    // Update PSP offset 0x16 (Parent PSP Segment)
                    let parent_psp = cpu.current_psp;
                    cpu.bus.write_16(psp_phys + 0x16, parent_psp);

                    // Write Command Tail to 80h
                    // target_cmd_tail_bytes does NOT include CR, logic below adds it.
                    cpu.bus
                        .write_8(psp_phys + 0x80, target_cmd_tail_bytes.len() as u8);
                    for (i, &b) in target_cmd_tail_bytes.iter().enumerate() {
                        cpu.bus.write_8(psp_phys + 0x81 + i, b);
                    }
                    // Ensure CR at end
                    cpu.bus
                        .write_8(psp_phys + 0x81 + target_cmd_tail_bytes.len(), 0x0D);

                    // We do NOT set CF=0 because we don't return to the caller yet!
                    // The caller is suspended.
                } else {
                    // Fail
                    cpu.restore_process_context(); // Restore parent immediately
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                    cpu.set_reg16(Register::AX, 0x02); // File not found
                }
            } else {
                cpu.bus.log_string("[DOS] EXEC Unsupported Mode");
                cpu.set_cpu_flag(CpuFlags::CF, true);
                cpu.set_reg16(Register::AX, 0x01); // Invalid function
            }
        }

        // AH = 2Ch: Get System Time
        // Returns: CH=Hour, CL=Minute, DH=Second, DL=1/100s
        0x2C => {
            let now = Local::now();

            let hour = now.hour() as u8;
            let minute = now.minute() as u8;
            let second = now.second() as u8;
            // chrono stores nanoseconds. 10,000,000 nanos = 1/100th second.
            let hundredths = (now.nanosecond() / 10_000_000) as u8;

            cpu.set_reg8(Register::CH, hour);
            cpu.set_reg8(Register::CL, minute);
            cpu.set_reg8(Register::DH, second);
            cpu.set_reg8(Register::DL, hundredths);
        }

        // AH=2Fh: Get DTA Address
        0x2F => {
            cpu.es = cpu.bus.dta_segment;
            cpu.set_reg16(Register::BX, cpu.bus.dta_offset);
        }

        // AH = 30h: Get DOS Version
        0x30 => {
            cpu.set_reg8(Register::AL, 5); // Major: 5
            cpu.set_reg8(Register::AH, 0); // Minor: .00
            cpu.bx = 0xFF00; // OEM ID
            cpu.cx = 0x0000; // Serial
            cpu.bus.log_string("[DOS] Reported DOS Version 5.0");
        }

        // AH = 31h: Terminate and Stay Resident
        0x31 => {
            let return_code = cpu.get_al();
            let paras_to_keep = cpu.get_reg16(Register::DX);
            let tsr_psp = cpu.current_psp;

            cpu.bus.log_string(&format!(
                "[DOS] TSR Terminate (AH=31h) Code={:02X} Paras={:04X} PSP={:04X}",
                return_code, paras_to_keep, tsr_psp
            ));

            // Calculate where the resident block ends
            let resident_end = tsr_psp.wrapping_add(paras_to_keep);

            if cpu.restore_process_context() {
                cpu.bus.log_string(&format!(
                    "[DOS] TSR: Returning to Parent. Resident End={:04X}",
                    resident_end
                ));

                // TSR Logic: Ensure the heap pointer respects the resident memory.
                // If the parent's heap pointer is "behind" the resident block, bump it forward.
                if cpu.heap_pointer < resident_end {
                    cpu.bus.log_string(&format!(
                        "[DOS] TSR: Bumping Heap Pointer from {:04X} to {:04X}",
                        cpu.heap_pointer, resident_end
                    ));
                    cpu.heap_pointer = resident_end;
                }

                cpu.ax = return_code as u16; // Set return code (AL)
                cpu.set_cpu_flag(CpuFlags::CF, false);
            } else {
                cpu.state = CpuState::RebootShell;
            }
        }

        // AH = 33h: Get/Set Ctrl-Break Check
        0x33 => {
            let al = cpu.get_al();
            if al == 0x00 {
                // Get
                cpu.set_reg8(Register::DL, 0); // 0 = Off
            } else if al == 0x01 { // Set
                // Ignore setting, just return
            } else if al == 0x06 {
                // Get MS-DOS Version (True version)
                cpu.set_reg16(Register::BX, 0x3205); // 5.50
                cpu.set_reg8(Register::DL, 0); // Revision 0
                cpu.set_reg8(Register::DH, 0); // DOS in HMA?
            }
        }

        // AH = 35h: Get Interrupt Vector
        0x35 => {
            let int_num = cpu.get_al() as usize;
            let phys_addr = int_num * 4;

            let off_low = cpu.bus.read_8(phys_addr) as u16;
            let off_high = cpu.bus.read_8(phys_addr + 1) as u16;
            cpu.bx = (off_high << 8) | off_low;

            let seg_low = cpu.bus.read_8(phys_addr + 2) as u16;
            let seg_high = cpu.bus.read_8(phys_addr + 3) as u16;
            cpu.es = (seg_high << 8) | seg_low;
        }

        // AH=36h: Get Disk Free Space
        0x36 => {
            let dl = cpu.get_reg8(Register::DL);
            match cpu.bus.disk.get_disk_free_space(dl) {
                Ok((sectors, available, bytes_per_sec, total)) => {
                    cpu.set_reg16(Register::AX, sectors);
                    // Cap clusters to prevent overflow in old apps
                    cpu.set_reg16(Register::BX, std::cmp::min(available, 20000));
                    cpu.set_reg16(Register::CX, bytes_per_sec);
                    cpu.set_reg16(Register::DX, std::cmp::min(total, 20000));
                }
                Err(_) => {
                    cpu.set_reg16(Register::AX, 0xFFFF);
                }
            }
        }

        // AH=39h: Create Directory (MKDIR)
        0x39 => {
            // TODO: Implement MKDIR
            cpu.set_cpu_flag(CpuFlags::CF, true);
            cpu.ax = 0x03; // Path not found (stub)
        }

        // AH=3Ah: Remove Directory (RMDIR)
        0x3A => {
            // TODO: Implement RMDIR
            cpu.set_cpu_flag(CpuFlags::CF, true);
            cpu.ax = 0x03;
        }

        // AH=3Bh: Set Current Directory (CHDIR)
        0x3B => {
            let addr = cpu.get_physical_addr(cpu.ds, cpu.dx);
            let path = read_asciiz_string(&cpu.bus, addr);
            if cpu.bus.disk.set_current_directory(&path) {
                cpu.set_cpu_flag(CpuFlags::CF, false);
            } else {
                cpu.set_cpu_flag(CpuFlags::CF, true);
                cpu.ax = 0x03; // Path not found
            }
        }

        // AH=3Ch: Create File
        0x3C => {
            let addr = cpu.get_physical_addr(cpu.ds, cpu.dx);
            let filename = read_asciiz_string(&cpu.bus, addr);
            // Attributes in CX are ignored for now (TODO)
            match cpu.bus.disk.open_file(&filename, 0x02) {
                // 0x02 = Read/Write + Create
                Ok(handle) => {
                    cpu.ax = handle;
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                }
                Err(code) => {
                    cpu.ax = code as u16;
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                }
            }
        }

        // AH=3Dh: Open File
        0x3D => {
            let addr = cpu.get_physical_addr(cpu.ds, cpu.dx);
            let filename = read_asciiz_string(&cpu.bus, addr);
            let mode = cpu.get_al();

            cpu.bus.log_string(&format!(
                "[DEBUG] Open File: '{}' Mode={:02X}",
                filename, mode
            ));

            match cpu.bus.disk.open_file(&filename, mode) {
                Ok(handle) => {
                    cpu.ax = handle;
                    cpu.bus
                        .log_string(&format!("[DEBUG] Open Success, Handle={:04X}", handle));
                    // In real CPU, clear CF here
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                }
                Err(code) => {
                    cpu.ax = code as u16;
                    // In real CPU, set CF here
                    cpu.bus
                        .log_string(&format!("[DEBUG] Open Failed, Error={:04X}", code));
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                }
            }
        }

        // AH = 3Eh: Close File
        0x3E => {
            let handle = cpu.bx;
            cpu.bus.disk.close_file(handle);
        }

        // AH = 3Fh: Read from File (or Stdin)
        0x3F => {
            let handle = cpu.bx;
            let count = cpu.cx as usize;
            let mut buf_addr = cpu.get_physical_addr(cpu.ds, cpu.dx);

            cpu.bus.log_string(&format!(
                "[DEBUG] Read File Handle {:04X}, Count {:04X}",
                handle, count
            ));

            if handle == 0 {
                // STDIN
                let mut read_count = 0;
                for _ in 0..count {
                    if let Some(key) = cpu.bus.keyboard_buffer.pop_front() {
                        cpu.bus.write_8(buf_addr, (key & 0xFF) as u8);
                        buf_addr += 1;
                        read_count += 1;
                    } else {
                        break;
                    }
                }
                cpu.ax = read_count as u16;
                cpu.set_cpu_flag(CpuFlags::CF, false);
            } else {
                match cpu.bus.disk.read_file(handle, count) {
                    Ok(bytes) => {
                        for b in &bytes {
                            cpu.bus.write_8(buf_addr, *b);
                            buf_addr += 1;
                        }
                        cpu.ax = bytes.len() as u16;
                        cpu.set_cpu_flag(CpuFlags::CF, false);
                    }
                    Err(e) => {
                        cpu.bus
                            .log_string(&format!("[DEBUG] Read Failed, Error={:04X}", e));
                        cpu.ax = e;
                        cpu.set_cpu_flag(CpuFlags::CF, true);
                    }
                }
            }
        }

        // AH = 40h: Write to File (or Stdout)
        0x40 => {
            let handle = cpu.bx;
            let count = cpu.cx as usize;
            let buf_addr = cpu.get_physical_addr(cpu.ds, cpu.dx);

            cpu.bus.log_string(&format!(
                "[DEBUG] Write File Handle {:04X}, Count {:04X}",
                handle, count
            ));

            let mut data = Vec::with_capacity(count);
            for i in 0..count {
                data.push(cpu.bus.read_8(buf_addr + i));
            }

            if handle == 1 || handle == 2 {
                // STDOUT/STDERR
                for &byte in &data {
                    if byte == 0x07 {
                        play_sdl_beep(&mut cpu.bus);
                    }
                }
                let s = String::from_utf8_lossy(&data);
                // Log what is being printed to stdout
                cpu.bus.log_string(&format!("[STDOUT] {}", s.trim()));

                let visual_s = s.replace('\x07', "");
                crate::video::print_string(cpu, &visual_s);
                cpu.ax = count as u16;
            } else {
                match &mut cpu.bus.disk.write_file(handle, &data) {
                    Ok(written) => cpu.ax = *written,
                    Err(_) => {
                        cpu.bus.log_string("[DEBUG] Write Failed");
                        cpu.ax = 0
                    }
                }
            }
        }

        // AH = 42h: Move File Pointer
        0x42 => {
            let handle = cpu.bx;
            let offset_high = cpu.cx as u32;
            let offset_low = cpu.dx as u32;
            let offset = ((offset_high << 16) | offset_low) as i32;
            let whence = cpu.get_al();

            match cpu.bus.disk.seek_file(handle, offset as i64, whence) {
                Ok(new_pos) => {
                    cpu.dx = ((new_pos >> 16) & 0xFFFF) as u16;
                    cpu.ax = (new_pos & 0xFFFF) as u16;
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                }
                Err(e) => {
                    cpu.ax = e;
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                }
            }
        }

        // AH=43h: Get/Set File Attributes
        0x43 => {
            let al = cpu.get_reg8(Register::AL);
            if al == 0x00 {
                cpu.set_reg16(Register::CX, 0x20); // Archive
                cpu.set_cpu_flag(CpuFlags::CF, false);
            } else {
                cpu.set_cpu_flag(CpuFlags::CF, false);
            }
        }

        // AH = 44h: IOCTL (I/O Control)
        0x44 => {
            let al = cpu.get_al();
            let bx = cpu.bx; // Handle

            cpu.bus.log_string(&format!(
                "[DOS] IOCTL AH=44h AL={:02X} Handle={:04X}",
                al, bx
            ));

            match al {
                // Get Device Information
                0x00 => {
                    // Bit 7=1 (Char Dev), Bit 6=0 (EOF), Bit 0=1 (Console Input)
                    // For STDIN(0), STDOUT(1), STDERR(2), return 0x80D3 or similar.
                    if bx <= 2 {
                        // 1000 0000 1101 0011 = 80D3
                        // Bit 7: Char device
                        // Bit 6: EOF (0) - meaningful for files?
                        // Bit 5: Raw (Binary) mode? (0=Cooked, 1=Raw)
                        // Bit 4: Special?
                        // Bit 3: Clock?
                        // Bit 2: NUL?
                        // Bit 1: Stdout
                        // Bit 0: Stdin
                        cpu.dx = 0x80D3;
                    } else {
                        // File: Bit 7=0 (Block Dev), Bits 0-5 = Drive #
                        cpu.dx = 0x0002; // Drive C
                    }
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                    // cpu.bus.log_string(&format!("[DOS] IOCTL Get Device Info -> {:04X}", cpu.dx));
                }
                // Check if Block Device is Removable
                0x08 => {
                    // AX=0 (Removable), AX=1 (Fixed)
                    cpu.ax = 1; // Fixed drive
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                }
                _ => {
                    // Stub other subfunctions as success
                    cpu.ax = 0;
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                }
            }
        }
        // AH=47h: Get Current Directory
        0x47 => {
            let dl = cpu.get_dl(); // Drive (0=Default, 1=A, ...)
            let ds = cpu.ds;
            let si = cpu.get_reg16(Register::SI);
            let addr = cpu.get_physical_addr(ds, si);
            let cwd = cpu.bus.disk.get_current_directory();

            cpu.bus
                .log_string(&format!("[DOS] Get CWD (AH=47h) Drive={} -> {}", dl, cwd));

            // Write string to DS:SI
            let bytes = cwd.as_bytes();
            for (i, &b) in bytes.iter().enumerate() {
                cpu.bus.write_8(addr + i, b);
            }
            // Null Terminate
            cpu.bus.write_8(addr + bytes.len(), 0x00);

            // Zero out the rest of the 64-byte buffer for safety
            for i in (bytes.len() + 1)..64 {
                cpu.bus.write_8(addr + i, 0x00);
            }
            cpu.set_reg16(Register::AX, 0x0100); // Success
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH = 48h: Allocate Memory
        // BX = Number of Paragraphs (16 bytes) requested
        // Return: AX = Segment, or CF=1 + AX=Error, BX=Max Available
        0x48 => {
            let requested_paras = cpu.bx;
            let available_paras = if cpu.heap_pointer < 0xA000 {
                0xA000 - cpu.heap_pointer
            } else {
                0
            };

            cpu.bus.log_string(&format!(
                "[DEBUG] Alloc Mem: Request {:04X} paras. Heap at {:04X}, Avail {:04X}",
                requested_paras, cpu.heap_pointer, available_paras
            ));

            if requested_paras > available_paras {
                cpu.ax = 0x0008; // Insufficient memory
                cpu.bx = available_paras;
                cpu.set_cpu_flag(CpuFlags::CF, true);
                cpu.bus
                    .log_string("[DEBUG] Alloc Failed: Insufficient Memory");
            } else {
                cpu.ax = cpu.heap_pointer;
                cpu.heap_pointer += requested_paras;
                cpu.set_cpu_flag(CpuFlags::CF, false);
                cpu.bus
                    .log_string(&format!("[DEBUG] Alloc Success: {:04X}", cpu.ax));
            }
        }

        // AH = 49h: Free Memory Block
        // ES = Segment of the block to be freed
        0x49 => {
            let segment_to_free = cpu.es;

            // TODO: Replace this stub by actually marking the memory block in the MCB chain as free.

            cpu.bus.log_string(&format!(
                "[DOS] Freeing Memory Block at {:04X}",
                segment_to_free
            ));

            // Return Success
            cpu.set_cpu_flag(CpuFlags::CF, false);
            cpu.ax = 0;
        }

        // AH = 4Ah: Resize Memory Block
        0x4A => {
            let requested_size = cpu.get_reg16(Register::BX);
            let max_available = 0x9000; // Simulated available paragraphs

            cpu.bus.log_string(&format!(
                "[DEBUG] INT 21,4A Resize: Req {:04X}, Max {:04X}",
                requested_size, max_available
            ));

            if requested_size > max_available {
                cpu.set_reg16(Register::BX, max_available);
                cpu.set_reg16(Register::AX, 0x0008);
                cpu.set_cpu_flag(CpuFlags::CF, true);
            } else {
                cpu.set_cpu_flag(CpuFlags::CF, false);
            }
        }

        // AH = 4Ch: Terminate Program
        0x4C => {
            let exit_code = cpu.get_al();
            cpu.bus.log_string(&format!(
                "[DOS] Program Terminated (INT 21h, 4Ch). ExitCode={:02X}",
                exit_code
            ));

            // Try to restore parent process
            if cpu.restore_process_context() {
                cpu.bus.log_string("[DOS] Returning to Parent Process");
                // TODO: Set Return Code in Parent's AX?
                // DOS convention: AL = Return Code.
                // Since we restored the parent context, we should probably update AL in the restored context.
                // But `restore_process_context` already overwrote registers from stack.
                // We should update AX *after* restore.
                cpu.ax = exit_code as u16;
                // Clear CF to indicate success? Usually EXEC returns with Carry Clear.
                cpu.set_cpu_flag(CpuFlags::CF, false);
            } else {
                cpu.state = CpuState::RebootShell;
            }
        }

        // AH=4Eh (Find First) / AH=4Fh (Find Next)
        0x4E | 0x4F => {
            let dta_seg = cpu.bus.dta_segment;
            let dta_off = cpu.bus.dta_offset;
            let dta_phys = cpu.get_physical_addr(dta_seg, dta_off);

            const OFFSET_ATTR_SEARCH: usize = 12;
            const OFFSET_INDEX: usize = 13;

            let (index, search_attr, raw_pattern, search_id) = if ah == 0x4E {
                let name_addr = cpu.get_physical_addr(cpu.ds, cpu.dx);
                let mut pattern = read_asciiz_string(&cpu.bus, name_addr);

                // Heuristic Fix for d.com (and potentially others):
                // If the search pattern ends with the Current Directory name followed by a wildcard
                // (e.g. "C:\TEXT.*" or "C:\TEXT.???") WITHOUT a path separator, it implies
                // the program intended to search the CONTENTS, but concatenated CWD + wildcard blindly.
                // We detect this and insert the missing separator (e.g. "C:\TEXT\*.*").
                let cwd = cpu.bus.disk.get_current_directory();

                if !cwd.is_empty() {
                    let pattern_upper = pattern.to_uppercase();
                    let cwd_upper = cwd.to_uppercase();

                    // Check for common malformed patterns
                    // Check for "TEXT.*"
                    let suffix_dot_star = format!("{}.*", cwd_upper);
                    // Check for "TEXT.???"
                    let suffix_dot_ques = format!("{}.???", cwd_upper);

                    if pattern_upper.ends_with(&suffix_dot_star) {
                        // Replace "TEXT.*" with "TEXT\*.*"
                        // We assume .* was intended as *.* because we are fixing a directory listing
                        let broken_len = suffix_dot_star.len();
                        let new_len = pattern.len() - broken_len;
                        pattern.truncate(new_len);
                        pattern.push_str(&cwd); // Original case CWD? Or upper? Disk is case insensitive. taking from bus is safe.
                        pattern.push_str("\\*.*");
                        cpu.bus.log_string(&format!(
                            "[DOS] Heuristic Pattern Fix: Rewrote to '{}'",
                            pattern
                        ));
                    } else if pattern_upper.ends_with(&suffix_dot_ques) {
                        // Replace "TEXT.???" with "TEXT\*.*"
                        let broken_len = suffix_dot_ques.len();
                        let new_len = pattern.len() - broken_len;
                        pattern.truncate(new_len);
                        pattern.push_str(&cwd);
                        pattern.push_str("\\*.*");
                        cpu.bus.log_string(&format!(
                            "[DOS] Heuristic Pattern Fix: Rewrote to '{}'",
                            pattern
                        ));
                    }
                }

                // Create a new Search ID
                let sid = (cpu.bus.start_time.elapsed().as_nanos() & 0xFFFFFFFF) as u32;
                (0, cpu.cx, pattern, sid)
            } else {
                let idx = cpu.bus.read_16(dta_phys + OFFSET_INDEX) as usize;
                let attr = cpu.bus.read_8(dta_phys + OFFSET_ATTR_SEARCH) as u16;
                // Read Search ID
                let sid_lo = cpu.bus.read_16(dta_phys + 15) as u32;
                let sid_hi = cpu.bus.read_16(dta_phys + 17) as u32;
                let sid = (sid_hi << 16) | sid_lo;

                let filename_pattern = read_dta_template(&cpu.bus, dta_phys);

                // Retrieve Directory from Bus
                let dir_prefix = cpu
                    .bus
                    .search_handles
                    .get(&sid)
                    .cloned()
                    .unwrap_or_default();

                // Construct full pattern
                let full_pattern = if dir_prefix.is_empty() {
                    filename_pattern
                } else {
                    format!("{}\\{}", dir_prefix, filename_pattern)
                };

                (idx, attr, full_pattern, sid)
            };

            // Pass the full raw pattern to DiskController.
            // It will handle splitting path and pattern.
            let search_pattern = raw_pattern;

            match cpu
                .bus
                .disk
                .find_directory_entry(&search_pattern, index, search_attr)
            {
                Ok(entry) => {
                    cpu.bus.log_string(&format!(
                        "[DOS] FindFirst/Next Found: '{}' (Index {})",
                        entry.filename, index
                    ));
                    cpu.bus.write_8(dta_phys + 0, 3); // Drive C:
                    cpu.bus
                        .write_8(dta_phys + OFFSET_ATTR_SEARCH, search_attr as u8);
                    cpu.bus
                        .write_16(dta_phys + OFFSET_INDEX, (index + 1) as u16);

                    // Only write Search Pattern to DTA on FindFirst (AH=4E)
                    // FindNext must NOT overwrite the pattern it uses for searching!
                    if ah == 0x4E {
                        // Extract filename part from search_pattern (which is raw path for 4E)
                        let filename_part = if let Some(idx) =
                            search_pattern.rfind(|c| c == '\\' || c == '/' || c == ':')
                        {
                            &search_pattern[idx + 1..]
                        } else {
                            &search_pattern
                        };

                        let fcb_bytes = pattern_to_fcb(filename_part);
                        for i in 0..11 {
                            cpu.bus.write_8(dta_phys + 1 + i, fcb_bytes[i]);
                        }
                    }

                    // Unique ID / Search Handle generation for FindNext tracking
                    // We store the Search ID in bytes 15-18 (4 bytes)
                    let unique_id = search_id;
                    cpu.bus.write_16(dta_phys + 15, (unique_id & 0xFFFF) as u16);
                    cpu.bus.write_16(dta_phys + 17, (unique_id >> 16) as u16);

                    // Store Directory Context if FindFirst (AH=4E)
                    if ah == 0x4E {
                        // Extract Directory part from search_pattern (original raw pattern for 4E)
                        // disk.rs logic: split at last separator
                        let dir_part = if let Some(idx) =
                            search_pattern.rfind(|c| c == '\\' || c == '/' || c == ':')
                        {
                            if idx == 0 {
                                "\\"
                            } else {
                                &search_pattern[..idx]
                            }
                        } else {
                            ""
                        }
                        .to_string();

                        if !dir_part.is_empty() {
                            cpu.bus.search_handles.insert(unique_id, dir_part);
                        }
                    }
                    cpu.bus
                        .write_16(dta_phys + 19, (index as u16).wrapping_mul(3));

                    // File Attributes
                    let mut attr = if entry.is_dir { 0x10 } else { 0x20 };
                    if entry.filename == "RUSTDOS" {
                        attr = 0x08;
                    }
                    cpu.bus.write_8(dta_phys + 21, attr);

                    cpu.bus.write_16(dta_phys + 22, entry.dos_time);
                    cpu.bus.write_16(dta_phys + 24, entry.dos_date);
                    cpu.bus
                        .write_16(dta_phys + 26, (entry.size & 0xFFFF) as u16);
                    cpu.bus.write_16(dta_phys + 28, (entry.size >> 16) as u16);

                    // Filename at Offset 30
                    let name_start = dta_phys + 30;
                    for i in 0..13 {
                        cpu.bus.write_8(name_start + i, 0x00);
                    }
                    let name_bytes = entry.filename.as_bytes();
                    let len = std::cmp::min(name_bytes.len(), 12);
                    for i in 0..len {
                        cpu.bus.write_8(name_start + i, name_bytes[i]);
                    }

                    cpu.set_reg16(Register::AX, 0);
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                }
                Err(code) => {
                    cpu.bus.log_string(&format!(
                        "[DOS] FindFirst/Next Failed: Pattern='{}' Index={} Error={:02X}",
                        search_pattern, index, code
                    ));
                    cpu.set_reg16(Register::AX, code as u16);
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                }
            }
        }

        _ => {
            cpu.bus
                .log_string(&format!("[DOS] Unhandled INT 21h AH={:02X}", ah));
        }
    }
}
