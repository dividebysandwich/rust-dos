use iced_x86::Register;
use chrono::{Local, Timelike};

use crate::cpu::{Cpu, CpuState, CpuFlags};
use crate::video::{print_char};
use crate::audio::play_sdl_beep;
use super::utils::{read_asciiz_string, read_dta_template, pattern_to_fcb};

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    match ah {
        // AH = 00h: Terminate Program (Legacy Method)
        0x00 => {
            cpu.bus.log_string("[DOS] Program Terminated (Legacy INT 20h/21h AH=00).");
            cpu.state = CpuState::RebootShell;
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
                cpu.bus.write_16(phys_sp & 0xFFFFF, saved_ip.wrapping_sub(4));
            }
        }

        // AH = 09h: Print String (Ends in '$')
        0x09 => {
            let mut offset = cpu.dx;
            loop {
                let char_byte = cpu.bus.read_8(cpu.get_physical_addr(cpu.ds, offset));
                if char_byte == b'$' { break; }

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
            // TODO: Implement multiple drives. Currently hardcoded to C: (2)
            cpu.set_reg8(Register::AL, 2);
        }

        // AH=1Ah: Set Disk Transfer Area (DTA) Address
        0x1A => {
            let ds = cpu.ds;
            let dx = cpu.get_reg16(Register::DX);
            cpu.bus.dta_segment = ds;
            cpu.bus.dta_offset = dx;
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
            if al == 0x00 { // Get
                cpu.set_reg8(Register::DL, 0); // 0 = Off
            } else if al == 0x01 { // Set
                // Ignore setting, just return
            } else if al == 0x06 { // Get MS-DOS Version (True version)
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

        // AH = 3Dh: Open File
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
            for i in 0..64 {
                cpu.bus.write_8(addr + i, 0x00);
            }
            cpu.set_reg16(Register::AX, 0x0100);
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
            
            cpu.bus.log_string(&format!("[DOS] Freeing Memory Block at {:04X}", segment_to_free));

            // Return Success
            cpu.set_cpu_flag(CpuFlags::CF, false);
            cpu.ax = 0; 
        }
        
        // AH = 4Ah: Resize Memory Block
        0x4A => {
            let requested_size = cpu.get_reg16(Register::BX);
            let max_available = 0x9000; // Simulated available paragraphs

            cpu.bus.log_string(&format!("[DEBUG] INT 21,4A Resize: Req {:04X}, Max {:04X}", requested_size, max_available));

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
            cpu.bus.log_string("[DOS] Program Terminated (INT 21h, 4Ch).");
            cpu.state = CpuState::RebootShell;
        }

        // AH=4Eh (Find First) / AH=4Fh (Find Next)
        0x4E | 0x4F => {
            let dta_seg = cpu.bus.dta_segment;
            let dta_off = cpu.bus.dta_offset;
            let dta_phys = cpu.get_physical_addr(dta_seg, dta_off);

            const OFFSET_ATTR_SEARCH: usize = 12;
            const OFFSET_INDEX: usize = 13;

            let (index, search_attr, raw_pattern) = if ah == 0x4E {
                let name_addr = cpu.get_physical_addr(cpu.ds, cpu.dx);
                let pattern = read_asciiz_string(&cpu.bus, name_addr);
                (0, cpu.cx, pattern)
            } else {
                let idx = cpu.bus.read_16(dta_phys + OFFSET_INDEX) as usize;
                let attr = cpu.bus.read_8(dta_phys + OFFSET_ATTR_SEARCH) as u16;
                let pattern = read_dta_template(&cpu.bus, dta_phys);
                (idx, attr, pattern)
            };

            let search_pattern = if let Some(idx) = raw_pattern.rfind('\\') {
                raw_pattern[idx+1..].to_string()
            } else if let Some(idx) = raw_pattern.rfind(':') {
                raw_pattern[idx+1..].to_string()
            } else {
                raw_pattern
            };

            match cpu.bus.disk.find_directory_entry(&search_pattern, index, search_attr) {
                Ok(entry) => {
                    cpu.bus.write_8(dta_phys + 0, 3); // Drive C:
                    cpu.bus.write_8(dta_phys + OFFSET_ATTR_SEARCH, search_attr as u8);
                    cpu.bus.write_16(dta_phys + OFFSET_INDEX, (index + 1) as u16);

                    let fcb_bytes = pattern_to_fcb(&search_pattern);
                    for i in 0..11 {
                        cpu.bus.write_8(dta_phys + 1 + i, fcb_bytes[i]);
                    }

                    // Unique ID generation for FindNext tracking
                    let unique_id = (index as u32).wrapping_add(0x12345678);
                    cpu.bus.write_16(dta_phys + 15, (unique_id & 0xFFFF) as u16);
                    cpu.bus.write_16(dta_phys + 17, (unique_id >> 16) as u16);
                    cpu.bus.write_16(dta_phys + 19, (index as u16).wrapping_mul(3));

                    // File Attributes
                    let mut attr = if entry.is_dir { 0x10 } else { 0x20 };
                    if entry.filename == "RUSTDOS" { attr = 0x08; }
                    cpu.bus.write_8(dta_phys + 21, attr);

                    cpu.bus.write_16(dta_phys + 22, entry.dos_time);
                    cpu.bus.write_16(dta_phys + 24, entry.dos_date);
                    cpu.bus.write_16(dta_phys + 26, (entry.size & 0xFFFF) as u16);
                    cpu.bus.write_16(dta_phys + 28, (entry.size >> 16) as u16);

                    // Filename at Offset 30
                    let name_start = dta_phys + 30;
                    for i in 0..13 { cpu.bus.write_8(name_start + i, 0x00); }
                    let name_bytes = entry.filename.as_bytes();
                    let len = std::cmp::min(name_bytes.len(), 12);
                    for i in 0..len {
                        cpu.bus.write_8(name_start + i, name_bytes[i]);
                    }

                    cpu.set_reg16(Register::AX, 0);
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                }
                Err(code) => {
                    cpu.set_reg16(Register::AX, code as u16);
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                }
            }
        }

        _ => cpu.bus.log_string(&format!("[DOS] Unhandled Call Int 0x21 AH={:02X}", ah)),
    }
}