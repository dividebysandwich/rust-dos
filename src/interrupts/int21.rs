use iced_x86::Register;
use crate::cpu::{Cpu, CpuState};
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
                cpu.set_flag(crate::cpu::FLAG_CF, false);
            } else {
                match cpu.bus.disk.read_file(handle, count) {
                    Ok(bytes) => {
                        for b in &bytes {
                            cpu.bus.write_8(buf_addr, *b);
                            buf_addr += 1;
                        }
                        cpu.ax = bytes.len() as u16;
                        cpu.set_flag(crate::cpu::FLAG_CF, false);
                    }
                    Err(e) => {
                        cpu.ax = e;
                        cpu.set_flag(crate::cpu::FLAG_CF, true);
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
                    cpu.set_flag(crate::cpu::FLAG_CF, false);
                }
                Err(e) => {
                    cpu.ax = e;
                    cpu.set_flag(crate::cpu::FLAG_CF, true);
                }
            }
        }

        // AH=43h: Get/Set File Attributes
        0x43 => {
            let al = cpu.get_reg8(Register::AL);
            if al == 0x00 {
                cpu.set_reg16(Register::CX, 0x20); // Archive
                cpu.set_flag(crate::cpu::FLAG_CF, false);
            } else {
                cpu.set_flag(crate::cpu::FLAG_CF, false);
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
            cpu.set_flag(crate::cpu::FLAG_CF, false);
        }

        // AH = 4Ah: Resize Memory Block
        0x4A => {
            let requested_size = cpu.get_reg16(Register::BX);
            let max_available = 0x9000; // Simulated available paragraphs

            cpu.bus.log_string(&format!("[DEBUG] INT 21,4A Resize: Req {:04X}, Max {:04X}", requested_size, max_available));

            if requested_size > max_available {
                cpu.set_reg16(Register::BX, max_available);
                cpu.set_reg16(Register::AX, 0x0008);
                cpu.set_flag(crate::cpu::FLAG_CF, true);
            } else {
                cpu.set_flag(crate::cpu::FLAG_CF, false);
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
                    cpu.set_flag(crate::cpu::FLAG_CF, false);
                }
                Err(code) => {
                    cpu.set_reg16(Register::AX, code as u16);
                    cpu.set_flag(crate::cpu::FLAG_CF, true);
                }
            }
        }

        _ => cpu.bus.log_string(&format!("[DOS] Unhandled Call Int 0x21 AH={:02X}", ah)),
    }
}