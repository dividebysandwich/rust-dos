use chrono::{Local, Timelike};
use iced_x86::Register;

use super::utils::{pattern_to_fcb, read_asciiz_string, read_dta_template};
use crate::audio::play_sdl_beep;
use crate::cpu::{Cpu, CpuFlags, CpuState};
use crate::video::print_char;

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    match ah {
        // AH = 00h: Terminate Program (Legacy Method)
        0x00 => {
            cpu.bus
                .log_string("[DOS] Program Terminated (Legacy INT 20h/21h AH=00).");
            cpu.state = CpuState::RebootShell;
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
            // We simulate C: as default.
            cpu.set_reg8(Register::AL, 2);
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

            match cpu.bus.disk.open_file(&filename, mode) {
                Ok(handle) => {
                    cpu.ax = handle;
                    // In real CPU, clear CF here
                }
                Err(code) => {
                    cpu.ax = code as u16;
                    // In real CPU, set CF here
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
                    Err(_) => cpu.ax = 0,
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

            match al {
                // Get Device Information
                0x00 => {
                    // Bit 7=1 (Char Dev), Bit 6=0 (EOF), Bit 0=1 (Console Input)
                    // For STDIN(0), STDOUT(1), STDERR(2), return 0x80D3 or similar.
                    if bx <= 2 {
                        cpu.dx = 0x80D3;
                    } else {
                        // File: Bit 7=0 (Block Dev), Bits 0-5 = Drive #
                        cpu.dx = 0x0002; // Drive C
                    }
                    cpu.set_cpu_flag(CpuFlags::CF, false);
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
            let ds = cpu.ds;
            let si = cpu.get_reg16(Register::SI);
            let addr = cpu.get_physical_addr(ds, si);
            let cwd = cpu.bus.disk.get_current_directory();

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

            // Very simple allocator stub:
            // We pretend there is a heap at 0x2000 (after the loaded COM/EXE at 0x1000).
            // TODO: Actual memory manager struct.

            // Check if request is obviously bad (> 640KB)
            if requested_paras > 0xA000 {
                cpu.ax = 0x0008; // Insufficient memory
                cpu.bx = 0x9000; // Say we have ~576KB free
                cpu.set_cpu_flag(CpuFlags::CF, true);
            } else {
                // Return a hardcoded free segment.
                // TODO: FIXME! Consecutive calls will return the SAME address in this stub.
                cpu.ax = 0x2000;
                cpu.set_cpu_flag(CpuFlags::CF, false);
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
            cpu.bus
                .log_string("[DOS] Program Terminated (INT 21h, 4Ch).");
            cpu.state = CpuState::RebootShell;
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

        _ => cpu
            .bus
            .log_string(&format!("[DOS] Unhandled Call Int 0x21 AH={:02X}", ah)),
    }
}
