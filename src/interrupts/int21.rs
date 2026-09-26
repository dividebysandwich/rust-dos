use chrono::{Datelike, Timelike};
use iced_x86::Register;

use super::utils::{pattern_to_fcb, read_asciiz_string, read_dta_template};
use crate::audio::play_sdl_beep;
use crate::cpu::{Cpu, CpuFlags, CpuState};
use crate::disk::{CharDevice, DriveKind, parse_drive_prefix};
use crate::diskio;
use crate::disknoise::Access;
use crate::dos_data;
use crate::dos_files;
use crate::video::print_char;

/// A RETF for the case map routine of the country information.
const CASE_MAP_ROUTINE: usize = 0xFF0FF;
/// Volume serial number of drive A:; each drive's is its number more.
pub const VOLUME_SERIAL: u32 = 0x1234_0000;

/// What DOS does on entry to INT 21h: save the caller's registers on its
/// stack (AX, BX, CX, DX, SI, DI, BP, DS, ES, below the interrupt's return
/// frame) and SS:SP in the running process's PSP (2Eh), where they are
/// when the process a program makes from it ends (`Cpu::terminate`).
fn save_caller(cpu: &mut Cpu) {
    let psp = cpu.current_psp;
    if psp == 0 || cpu.pm() {
        return;
    }
    let (ss, sp) = (cpu.ss(), cpu.sp().wrapping_sub(18));
    let registers = [cpu.ax(), cpu.bx(), cpu.cx(), cpu.dx(), cpu.si(), cpu.di(), cpu.bp(), cpu.ds(), cpu.es()];
    for (i, value) in registers.into_iter().enumerate() {
        let at = ss as usize * 16 + sp.wrapping_add(2 * i as u16) as usize;
        cpu.bus.write_16(at, value);
    }
    let base = psp as usize * 16;
    cpu.bus.write_16(base + 0x2E, sp);
    cpu.bus.write_16(base + 0x30, ss);
}

/// The open file (its System File Table entry) handle `handle` of the
/// running process refers to.
fn file_of(cpu: &Cpu, handle: u16) -> Option<u16> {
    dos_files::sft_of(&cpu.bus, cpu.current_psp, handle)
}

/// A handle of the running process for the file just opened at `sft`.
fn attach(cpu: &mut Cpu, sft: u16) -> Result<u16, u8> {
    dos_files::attach(&mut cpu.bus, cpu.current_psp, sft)
}

/// Return a result the DOS way: AX and CF clear, or the error code in AX
/// and CF set.
fn set_result(cpu: &mut Cpu, result: Result<u16, u8>) {
    match result {
        Ok(ax) => {
            cpu.set_ax(ax);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        Err(code) => {
            cpu.set_ax(code as u16);
            cpu.set_cpu_flag(CpuFlags::CF, true);
        }
    }
}

/// The 34-byte country information of AH=38h for the USA: m/d/y dates,
/// "$" currency, "," thousands, "." decimals, 12-hour clock.
fn write_country_info(cpu: &mut Cpu, addr: usize) {
    let mut info = [0u8; 34];
    info[0] = 0; // date format: USA
    info[2] = b'$';
    info[7] = b',';
    info[9] = b'.';
    info[11] = b'-';
    info[13] = b':';
    info[15] = 0; // currency symbol before the value
    info[16] = 2; // decimal digits
    info[17] = 0; // 12-hour clock
    // Case map routine (far pointer): a RETF.
    cpu.bus.write_rom(CASE_MAP_ROUTINE, &[0xCB]);
    let case_map = ((CASE_MAP_ROUTINE - 0xF0000) as u32) | 0xF000_0000;
    info[18..22].copy_from_slice(&case_map.to_le_bytes());
    info[22] = b',';
    cpu.bus.load_bytes(addr, &info);
}

/// Allocate the largest available free MCB for a child process about to be
/// loaded via EXEC, and return its first usable paragraph (the new PSP seg).
/// The MCB owner is temporarily set to the placeholder 0xFFFF and must be
/// patched to the child's PSP by the caller once load_executable sets it.
/// Returns None if no free memory could be allocated.
fn find_child_load_segment(cpu: &mut Cpu) -> Option<u16> {
    // Walk the chain to find the largest free block: in conventional
    // memory, unless the program asked for upper memory and linked it.
    let upper = cpu.alloc_strategy & 0xC0 != 0 && cpu.bus.umb.is_some_and(|u| u.linked);
    let chain = crate::mcb::walk(&cpu.bus);
    let largest = chain
        .iter()
        .filter(|(s, m)| m.is_free() && (upper || *s < crate::mcb::umb_cover_seg(&cpu.bus)))
        .map(|(_, m)| m.size)
        .max()
        .unwrap_or(0);

    if largest == 0 {
        return None;
    }

    // Grab the whole block — DOS convention is "give the child everything;
    // the child's own startup will AH=4A down to what it needs".
    match crate::mcb::alloc(&mut cpu.bus, 0xFFFF, largest) {
        Ok(seg) => Some(seg),
        Err(_) => None,
    }
}

/// 0-based drive for a DOS drive code where 0 = default, 1 = A:, ...
fn dos_drive_number(cpu: &Cpu, code: u8) -> u8 {
    if code == 0 {
        cpu.bus.disk.get_current_drive()
    } else {
        code - 1
    }
}

/// INT 21h AX=440Dh, generic IOCTL for block devices: BL = drive (0 =
/// default), CX = category and function, DS:DX -> parameter block. Get
/// Device Parameters (0860h) and Get Media ID (0866h) are how programs tell
/// floppies from hard disks; the other functions succeed without doing
/// anything. CD-ROMs are redirector drives, which have no block device.
fn generic_block_ioctl(cpu: &mut Cpu) {
    let drive = dos_drive_number(cpu, cpu.get_reg8(Register::BL));
    let (kind, layout) = match (cpu.bus.disk.drive_kind(drive), cpu.bus.disk.layout(drive)) {
        (Some(DriveKind::CdRom), _) => return set_result(cpu, Err(0x01)), // invalid function
        (Some(kind), Some(layout)) => (kind, layout),
        _ => return set_result(cpu, Err(0x0F)), // invalid drive
    };
    let block = cpu.get_physical_addr(cpu.ds(), cpu.dx());
    match cpu.cx() {
        0x0860 => {
            // 00: special functions, which the caller sets.
            let device_type = match (kind, layout.sectors_per_track) {
                (DriveKind::Floppy, 8 | 9) if layout.cylinders() <= 40 => 0x00, // 320/360 KB
                (DriveKind::Floppy, 15) => 0x01,                               // 1.2 MB
                (DriveKind::Floppy, 9) => 0x02,                                // 720 KB
                (DriveKind::Floppy, 36) => 0x09,                               // 2.88 MB
                (DriveKind::Floppy, _) => 0x07,                                // 1.44 MB
                _ => 0x05,                                                     // fixed disk
            };
            cpu.bus.write_8(block + 0x01, device_type);
            cpu.bus.write_16(block + 0x02, if kind.is_removable() { 0 } else { 1 }); // 02: bit 0 = fixed
            cpu.bus.write_16(block + 0x04, layout.cylinders());
            cpu.bus.write_8(block + 0x06, 0); // 06: media type, the drive's own
            for (i, &b) in layout.bpb().iter().enumerate() {
                cpu.bus.write_8(block + 0x07 + i, b);
            }
        }
        0x0866 => {
            cpu.bus.write_16(block, 0); // 00: info level
            let serial = cpu.bus.disk.volume_serial(drive).unwrap_or(VOLUME_SERIAL + drive as u32);
            cpu.bus.write_32(block + 0x02, serial);
            let label = cpu.bus.disk.volume_label(drive).unwrap_or_default();
            let label = if label.is_empty() { "NO NAME".to_string() } else { label };
            let padded = label.bytes().chain(std::iter::repeat(b' ')).take(11);
            for (i, b) in padded.chain(layout.fs_type().iter().copied()).enumerate() {
                cpu.bus.write_8(block + 0x06 + i, b);
            }
        }
        _ => {}
    }
    set_result(cpu, Ok(0));
}

/// Disk access through an open file: `bytes` moved (`data` tells a read
/// from a write), or a call that moves none. It takes the time the drive's
/// speed says and makes its noise (see `diskio`); devices take none.
fn file_io(cpu: &mut Cpu, handle: u16, bytes: u32, data: Option<bool>) {
    let disk = &cpu.bus.disk;
    let (Some(drive), None, Some(key)) = (disk.handle_drive(handle), disk.handle_device(handle), disk.handle_key(handle))
    else {
        return;
    };
    let access = match data {
        Some(write) => Access::File { write, key },
        None => Access::Other,
    };
    cpu.bus.drive_activity(drive, bytes, access);
}

/// A character from the keyboard the way DOS's console driver reads it: an
/// extended key (cursor keys, function keys) comes as 00h, and its scan
/// code on the next read.
fn con_read(cpu: &mut Cpu) -> Option<u8> {
    if let Some(scan) = cpu.con_pending_scan.take() {
        return Some(scan);
    }
    let key = cpu.bus.keyboard_buffer.pop_front()?;
    let (scan, ascii) = ((key >> 8) as u8, key as u8);
    if ascii == 0 || (ascii == 0xE0 && scan != 0) {
        cpu.con_pending_scan = Some(scan);
        return Some(0);
    }
    Some(ascii)
}

/// Write a character to standard output (handle 1) as DOS's character
/// functions do: to the screen (a bell beeps), or to where the command
/// line redirected it.
fn stdout_char(cpu: &mut Cpu, byte: u8) {
    if let Some(sft) = file_of(cpu, 1).filter(|&sft| cpu.bus.disk.handle_device(sft) != Some(CharDevice::Con)) {
        let _ = cpu.bus.disk.write_file(sft, &[byte]);
    } else if byte == 0x07 {
        play_sdl_beep(&mut cpu.bus);
    } else {
        print_char(&mut cpu.bus, byte);
    }
}

/// A line being typed at the keyboard for INT 21h AH=0Ah or a read of
/// CON, which waits for more keys until Enter.
#[derive(Clone, Debug, Default)]
pub struct ConLine {
    /// The buffer of the call it is for (DS, DX): another call starts a
    /// new line.
    buffer: (u16, u16),
    text: Vec<u8>,
}

/// Echo a character read from the console at the cursor, as DOS does.
fn con_echo(cpu: &mut Cpu, text: &[u8]) {
    for &b in text {
        print_char(&mut cpu.bus, b);
    }
}

/// Edit a line from the keyboard as DOS's console does, for AH=0Ah and
/// reads of CON: typed characters are echoed while fewer than `max` are
/// there (a beep past that), Backspace takes one back, and Esc starts the
/// line again after a backslash. The line once Enter is pressed; None
/// while it waits for keys (`hle_wait`), the line so far kept for the call
/// that goes on.
fn edit_line(cpu: &mut Cpu, max: usize) -> Option<Vec<u8>> {
    let buffer = (cpu.ds(), cpu.dx());
    let mut line = cpu.con_line.take().filter(|l| l.buffer == buffer).unwrap_or(ConLine { buffer, text: Vec::new() });
    loop {
        let Some(c) = con_read(cpu) else {
            cpu.con_line = Some(line);
            cpu.hle_wait();
            return None;
        };
        match c {
            0x0D => return Some(line.text),
            0x08 => {
                if line.text.pop().is_some() {
                    con_echo(cpu, b"\x08 \x08");
                }
            }
            0x1B => {
                con_echo(cpu, b"\\\r\n");
                line.text.clear();
            }
            // An extended key: its scan code comes next, and neither is typed.
            0x00 => {
                con_read(cpu);
            }
            _ if c < 0x20 => {}
            _ if line.text.len() < max => {
                line.text.push(c);
                con_echo(cpu, &[c]);
            }
            _ => play_sdl_beep(&mut cpu.bus),
        }
    }
}

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    save_caller(cpu);
    let (ax, bx, ds) = (cpu.ax(), cpu.bx(), cpu.ds());
    dos_data::enter_dos(&mut cpu.bus, ax, bx, ds);
    dispatch(cpu, ah);
    dos_data::leave_dos(&mut cpu.bus, cpu.current_psp);
    // Remember the error of a failed handle or file call for AH=59h. The
    // calls in this range that don't report through CF leave it as the
    // caller had it.
    let reports_cf = !matches!(ah, 0x4C | 0x4D | 0x50 | 0x51 | 0x54 | 0x55 | 0x59 | 0x62);
    if (0x39..=0x6C).contains(&ah) && reports_cf && cpu.get_cpu_flag(CpuFlags::CF) {
        cpu.last_dos_error = cpu.ax();
    }
}

fn dispatch(cpu: &mut Cpu, ah: u8) {
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
            if cpu.terminate(0) {
                cpu.bus
                    .log_string("[DOS] AH=00: Returning to Parent Process");
            }
        }

        // AH=11h (Find First FCB) / AH=12h (Find Next FCB)
        0x11 | 0x12 => super::fcb::find(cpu, ah == 0x11),

        // The FCB file functions of DOS 1, see fcb.rs.
        0x0F | 0x16 => super::fcb::open(cpu, ah == 0x16),
        0x10 => super::fcb::close(cpu),
        0x13 => super::fcb::delete(cpu),
        0x14 | 0x15 => super::fcb::sequential(cpu, ah == 0x15),
        0x17 => super::fcb::rename(cpu),
        0x21 | 0x22 => super::fcb::random(cpu, ah == 0x22),
        0x23 => super::fcb::file_size(cpu),
        0x24 => super::fcb::set_random(cpu),
        0x27 | 0x28 => super::fcb::random_block(cpu, ah == 0x28),

        // AH=1Bh: Allocation info for the default drive; AH=1Ch: for drive DL
        // (0=default, 1=A). Returns AL=sectors per cluster, CX=bytes per
        // sector, DX=total clusters and DS:BX -> media ID byte.
        0x1B | 0x1C => {
            let dl = if ah == 0x1B { 0 } else { cpu.get_dl() };
            let drive = dos_drive_number(cpu, dl);
            match cpu.bus.disk.drive_geometry(drive) {
                Some((spc, bps, total)) => {
                    cpu.set_reg8(Register::AL, spc as u8);
                    cpu.set_cx(bps);
                    cpu.set_dx(total);
                    let media = cpu.bus.disk.media_descriptor(drive);
                    cpu.bus.write_8(dos_data::address(dos_data::MEDIA_ID), media);
                    cpu.set_ds(dos_data::SEGMENT);
                    cpu.set_bx(dos_data::MEDIA_ID);
                }
                None => cpu.set_reg8(Register::AL, 0xFF), // Invalid drive
            }
        }

        // AH=1Fh: DPB of the default drive; AH=32h: DPB of drive DL
        // (0=default, 1=A). DS:BX -> DPB with AL=00h, or AL=FFh for invalid
        // drives and CD-ROMs (redirector drives have no DPB).
        0x1F | 0x32 => {
            let dl = if ah == 0x1F { 0 } else { cpu.get_dl() };
            let drive = dos_drive_number(cpu, dl);
            match cpu.bus.disk.drive_kind(drive) {
                Some(kind) if kind != DriveKind::CdRom => {
                    cpu.set_reg8(Register::AL, 0x00);
                    cpu.set_ds(dos_data::SEGMENT);
                    cpu.set_bx(dos_data::dpb(drive));
                }
                _ => cpu.set_reg8(Register::AL, 0xFF),
            }
        }

        // AH = 02h: Output Character (DL = Char)
        0x02 => {
            let char_byte = cpu.get_dl();
            stdout_char(cpu, char_byte);
            cpu.set_reg8(Register::AL, char_byte);
        }

        // AH = 06h: Direct Console I/O
        0x06 => {
            let dl = cpu.get_reg8(Register::DL);

            if dl == 0xFF {
                // --- INPUT (Non-Blocking) ---
                if let Some(ascii) = con_read(cpu) {
                    // Key Available: Return ASCII and Clear Zero Flag
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
                stdout_char(cpu, dl);
                // AL is officially undefined on output, but we leave it alone.
            }
        }

        // AH = 01h: Read Character from Standard Input With Echo
        // Blocks until a key is available. Returns AL = ASCII, echoes to STDOUT.
        // If AL == 0 on return, the next call will return the scan code for
        // extended keys (arrows/function keys).
        0x01 => {
            let pending = cpu.con_pending_scan.is_some();
            if let Some(ascii) = con_read(cpu) {
                cpu.set_reg8(Register::AL, ascii);
                if ascii != 0 && !pending {
                    print_char(&mut cpu.bus, ascii);
                }
            } else {
                cpu.hle_wait();
            }
        }

        // AH = 07h: Direct Console Input Without Echo
        0x07 => {
            if let Some(ascii) = con_read(cpu) {
                cpu.set_reg8(Register::AL, ascii);
            } else {
                cpu.hle_wait();
            }
        }

        // AH = 08h: Direct Console Input Without Echo (checks Ctrl-Break).
        // Functionally identical to AH=07h for us; we don't model Ctrl-Break.
        0x08 => {
            if let Some(ascii) = con_read(cpu) {
                cpu.set_reg8(Register::AL, ascii);
            } else {
                cpu.hle_wait();
            }
        }

        // AH = 0Ah: Buffered Keyboard Input into DS:DX: at most (its first
        // byte) - 1 characters, their count at DS:DX+1, and the line with
        // its CR from DS:DX+2. Only the CR is echoed at the end.
        0x0A => {
            let buffer = cpu.get_physical_addr(cpu.ds(), cpu.dx());
            let max = cpu.bus.read_8(buffer) as usize;
            if max > 0
                && let Some(line) = edit_line(cpu, max - 1)
            {
                con_echo(cpu, b"\r");
                cpu.bus.write_8(buffer + 1, line.len() as u8);
                cpu.bus.load_bytes(buffer + 2, &line);
                cpu.bus.write_8(buffer + 2 + line.len(), 0x0D);
            }
        }

        // AH = 0Bh: Check Standard Input Status
        // Returns AL = 0xFF if a character is ready, 0x00 if not.
        0x0B => {
            if cpu.bus.keyboard_buffer.is_empty() && cpu.con_pending_scan.is_none() {
                cpu.set_reg8(Register::AL, 0x00);
            } else {
                cpu.set_reg8(Register::AL, 0xFF);
            }
        }

        // AH = 09h: Print String (Ends in '$')
        0x09 => {
            let mut offset = cpu.dx();
            loop {
                let char_byte = cpu.bus.read_8(cpu.get_physical_addr(cpu.ds(), offset));
                if char_byte == b'$' {
                    break;
                }
                stdout_char(cpu, char_byte);
                offset += 1;
            }
        }

        // AH = 0Ch: Clear Keyboard Buffer and Invoke Keyboard Function
        // AL = Function to execute after clearing (1, 6, 7, 8, 0xA)
        0x0C => {
            let next_fn = cpu.get_al();

            cpu.bus.keyboard_buffer.clear();
            cpu.con_pending_scan = None;
            cpu.con_pending.clear();

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
            let ds = cpu.ds();
            let dx = cpu.get_reg16(Register::DX);
            cpu.bus.dta_segment = ds;
            cpu.bus.dta_offset = dx;
        }

        // AH = 29h: Parse Filename
        // AH = 29h: Parse a file name at DS:SI into the FCB at ES:DI.
        0x29 => super::fcb::parse_filename(cpu),

        // AH = 25h: Set Interrupt Vector
        0x25 => {
            let int_num = cpu.get_al() as usize;
            let new_off = cpu.dx();
            let new_seg = cpu.ds();
            let phys_addr = int_num * 4;

            cpu.bus.write_8(phys_addr, (new_off & 0xFF) as u8);
            cpu.bus.write_8(phys_addr + 1, (new_off >> 8) as u8);
            cpu.bus.write_8(phys_addr + 2, (new_seg & 0xFF) as u8);
            cpu.bus.write_8(phys_addr + 3, (new_seg >> 8) as u8);
        }

        // AH = 4Bh: Load and Execute Program (EXEC)
        0x4B => {
            let mode = cpu.get_al();
            let name_addr = cpu.get_physical_addr(cpu.ds(), cpu.dx());
            let filename = read_asciiz_string(&cpu.bus, name_addr);

            // --- Stub out copy-protection / disk-swap helpers ---
            // MicroProse games (F-117, F-19, Gunship, Covert Action) ship two
            // runtime helpers that we can't satisfy without the original
            // install disks:
            //
            //   DSWAP.EXE  — verifies the install disk via INT 13h sector
            //                reads. Without a real disk image the check
            //                always fails and hangs on "insert disk".
            //
            //   PLAYER.EXE — animates title / cutscene screens. Its stream
            //                decoder does a runtime integrity check on a
            //                buffer that's supposed to be filled by the
            //                copy-protection path; when the data is zero
            //                (our case) it loops forever looking for its
            //                terminator word.
            //
            // Both are non-essential for gameplay. Short-circuit them with
            // the exit code the parent expects. F117.COM tests `AL != 0` after
            // DSWAP, so we return 1 there; it doesn't inspect PLAYER's exit
            // code at all so 0 is fine.
            // Programs load by physical address here, where a DOS machine
            // of Windows' 386 enhanced mode wouldn't see them: it has
            // conventional memory of its own.
            if matches!(mode, 0x00 | 0x01) && cpu.v86() && !cpu.conventional_memory_in_place() {
                cpu.bus.log_string(&format!("[DOS] EXEC of '{}' in a virtual machine with memory of its own refused", filename));
                set_result(cpu, Err(0x08));
                return;
            }
            let upper = filename.to_ascii_uppercase();
            let short = upper.rsplit(&['\\', '/', ':'][..]).next().unwrap_or("");
            if mode == 0x00 {
                match short {
                    "DSWAP.EXE" => {
                        cpu.bus.log_string(&format!(
                            "[DOS] EXEC: short-circuiting DSWAP.EXE ({}), exit=01",
                            filename
                        ));
                        cpu.last_child_exit = 0x0001;
                        cpu.set_cpu_flag(CpuFlags::CF, false);
                        cpu.set_reg16(Register::AX, 0x0000);
                        return;
                    }
                    "PLAYER.EXE" => {
                        cpu.bus.log_string(&format!(
                            "[DOS] EXEC: short-circuiting PLAYER.EXE ({}), exit=00",
                            filename
                        ));
                        cpu.last_child_exit = 0x0000;
                        cpu.set_cpu_flag(CpuFlags::CF, false);
                        cpu.set_reg16(Register::AX, 0x0000);
                        return;
                    }
                    _ => {}
                }
            }

            cpu.bus.log_string(&format!(
                "[DOS] EXEC AH=4B Name='{}' AL={:02X}",
                filename, mode
            ));

            if mode == 0x00 || mode == 0x01 {
                // Load and Execute (AL=00h), or load for a debugger to run
                // (AL=01h). ES:BX points to Parameter Block
                // Offset 00: Segment of environment (word)
                // Offset 02: Pointer to command line (dword) -> Write to PSP 80h
                // Offset 06: Pointer to FCB 1 (dword) -> Write to PSP 5Ch
                // Offset 0A: Pointer to FCB 2 (dword) -> Write to PSP 6Ch

                let param_block = cpu.bx(); // Offset in ES
                let param_seg = cpu.es();
                let param_phys = cpu.get_physical_addr(param_seg, param_block);

                // Read Environment Segment
                let env_seg = cpu.bus.read_16(param_phys);

                // Read Command Line Pointer
                let cmd_off = cpu.bus.read_16(param_phys + 2);
                let cmd_seg = cpu.bus.read_16(param_phys + 4);

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

                // The environment ends with a double NUL; the program path
                // goes after it (below, once the program is known).
                if env_block.len() < 2
                    || env_block[env_block.len() - 1] != 0
                    || env_block[env_block.len() - 2] != 0
                {
                    env_block.push(0);
                    if env_block.len() == 1 || env_block[env_block.len() - 2] != 0 {
                        env_block.push(0);
                    }
                }

                // COMMAND.COM, wherever a program looks for it (the COMSPEC,
                // or C:\COMMAND.COM where there is none), is ours on Z:.
                let is_command_com = filename
                    .rsplit(['\\', '/', ':'])
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case("COMMAND.COM"));
                let target_filename = if is_command_com { "Z:\\COMMAND.COM".to_string() } else { filename.clone() };
                let target_cmd_tail_bytes = cmd_tail.clone();

                // DOS 3.0+ puts a word count of 1 and the program's fully
                // qualified path after the environment. Programs find their
                // own directory with it, and DOS extenders their EXE file.
                env_block.extend_from_slice(&[0x01, 0x00]);
                env_block.extend_from_slice(cpu.program_path(&target_filename).as_bytes());
                env_block.push(0);

                // The child's environment gets a memory block of its own,
                // freed with the child's other memory when it exits.
                let env_paras = env_block.len().div_ceil(16) as u16;
                let env_seg = match crate::mcb::alloc(&mut cpu.bus, 0xFFFF, env_paras) {
                    Ok(seg) => seg,
                    Err(_) => {
                        cpu.set_cpu_flag(CpuFlags::CF, true);
                        cpu.set_reg16(Register::AX, 0x08); // Insufficient memory
                        cpu.bus
                            .log_string("[DOS] EXEC: no memory for the environment");
                        return;
                    }
                };

                // Save the parent's full context (registers, SS:SP, CS:IP of
                // the instruction right after the INT 21 that got us here, PSP,
                // heap pointer). When the child calls AH=4Ch, the AH=4Ch
                // handler pops this context back, restoring the parent's stack
                // so the BOP trap's IRET-pop finds the parent's saved flags/CS/IP.
                cpu.save_process_context();
                // The child returns where the parent's INT 21h would.
                let frame = cpu.get_physical_addr(cpu.ss(), cpu.sp());
                let return_address = (cpu.bus.read_16(frame), cpu.bus.read_16(frame + 2));

                let parent_psp_before = cpu.current_psp;
                // Allocate the largest free MCB for the child. DOS gives the
                // child all available conventional memory; it will shrink via
                // AH=4A in its own startup if it wants to spawn nested children.
                let load_segment = match find_child_load_segment(cpu) {
                    Some(seg) => seg,
                    None => {
                        let _ = crate::mcb::free(&mut cpu.bus, env_seg);
                        cpu.restore_process_context();
                        cpu.set_cpu_flag(CpuFlags::CF, true);
                        cpu.set_reg16(Register::AX, 0x08); // Insufficient memory
                        cpu.bus
                            .log_string("[DOS] EXEC: no free memory for child");
                        return;
                    }
                };

                if cpu.load_executable(&target_filename, Some(load_segment)) {
                    if let Some(context) = cpu.process_stack.last_mut() {
                        context.child = load_segment;
                    }
                    // Patch the MCB we allocated above with placeholder owner
                    // 0xFFFF so it reflects the child's real PSP segment.
                    let mcb_seg = load_segment.wrapping_sub(1);
                    let m = crate::mcb::read_mcb(&cpu.bus, mcb_seg);
                    crate::mcb::write_mcb(
                        &mut cpu.bus,
                        mcb_seg,
                        &crate::mcb::Mcb {
                            signature: m.signature,
                            owner: load_segment,
                            size: m.size,
                        },
                    );

                    let psp_phys = cpu.get_physical_addr(load_segment, 0);

                    // The environment, owned by the child.
                    let env_phys = cpu.get_physical_addr(env_seg, 0);
                    cpu.bus.load_bytes(env_phys, &env_block);
                    let env_mcb = crate::mcb::read_mcb(&cpu.bus, env_seg - 1);
                    crate::mcb::write_mcb(
                        &mut cpu.bus,
                        env_seg - 1,
                        &crate::mcb::Mcb {
                            owner: load_segment,
                            ..env_mcb
                        },
                    );
                    cpu.bus.write_16(psp_phys + 0x2C, env_seg);

                    // Update PSP offset 0x16 (Parent PSP Segment)
                    cpu.bus.write_16(psp_phys + 0x16, parent_psp_before);
                    // Terminate address (INT 22h).
                    cpu.bus.write_16(psp_phys + 0x0A, return_address.0);
                    cpu.bus.write_16(psp_phys + 0x0C, return_address.1);

                    // The two FCBs of the parameter block (at +06h and
                    // +0Ah), and the AX the child starts with: AL (AH) FFh
                    // if the first (second) names a drive that isn't there.
                    let mut child_ax = 0u16;
                    for (i, pointer) in [param_phys + 6, param_phys + 0x0A].into_iter().enumerate() {
                        let (off, seg) = (cpu.bus.read_16(pointer), cpu.bus.read_16(pointer + 2));
                        let from = cpu.get_physical_addr(seg, off);
                        let fcb: Vec<u8> = (0..16).map(|j| cpu.bus.read_8(from + j)).collect();
                        cpu.bus.load_bytes(psp_phys + 0x5C + 0x10 * i, &fcb);
                        if fcb[0] != 0 && !cpu.bus.disk.is_mounted(fcb[0].wrapping_sub(1)) {
                            child_ax |= 0xFF << (8 * i);
                        }
                    }

                    // Write Command Tail to PSP+0x80
                    cpu.bus
                        .write_8(psp_phys + 0x80, target_cmd_tail_bytes.len() as u8);
                    for (i, &b) in target_cmd_tail_bytes.iter().enumerate() {
                        cpu.bus.write_8(psp_phys + 0x81 + i, b);
                    }
                    cpu.bus
                        .write_8(psp_phys + 0x81 + target_cmd_tail_bytes.len(), 0x0D);

                    if mode == 0x01 {
                        // Hand the child's entry point and stack, with the
                        // initial AX on it, to the debugger in the parameter
                        // block, and go back to it with the child's PSP
                        // current. The parent's context stays saved for when
                        // the child terminates.
                        let (entry, stack) = ((cpu.cs(), cpu.ip()), (cpu.ss(), cpu.sp().wrapping_sub(2)));
                        cpu.bus.write_16(cpu.get_physical_addr(stack.0, stack.1), child_ax);
                        cpu.bus.write_16(param_phys + 0x0E, stack.1);
                        cpu.bus.write_16(param_phys + 0x10, stack.0);
                        cpu.bus.write_16(param_phys + 0x12, entry.1);
                        cpu.bus.write_16(param_phys + 0x14, entry.0);
                        let parent = cpu.process_stack.last().expect("context saved above").regs.clone();
                        let heap_pointer = cpu.heap_pointer;
                        cpu.restore(&parent);
                        cpu.heap_pointer = heap_pointer;
                        cpu.set_cpu_flag(CpuFlags::CF, false);
                        cpu.bus.log_string(&format!(
                            "[DOS] EXEC loaded child PSP {:04X}, entry {:04X}:{:04X}",
                            load_segment, entry.0, entry.1
                        ));
                        return;
                    }

                    // Set up the child's register state per DOS convention.
                    // DS/ES already point to the PSP from load_executable.
                    cpu.set_ax(child_ax);
                    cpu.set_bx(0);
                    cpu.set_cx(0);
                    cpu.set_dx(0);
                    cpu.set_si(0);
                    cpu.set_di(0);
                    cpu.set_bp(0);

                    // Our BOP trap runs an implicit IRET-style pop after this
                    // handler returns: it pops IP, CS, flags from SS:SP. Since
                    // load_executable just switched SS:SP to the child's own
                    // stack, push the child's entry CS:IP plus sane flags so
                    // the pop lands us at the child's entry point rather than
                    // popping zeros off the bottom of its stack.
                    let entry_ip = cpu.ip();
                    let entry_cs = cpu.cs();
                    let entry_flags: u16 = 0x0202; // IF=1, reserved bit 1 always set
                    cpu.push(entry_flags);
                    cpu.push(entry_cs);
                    cpu.push(entry_ip);

                    cpu.bus.log_string(&format!(
                        "[DOS] EXEC transferred to child at {:04X}:{:04X}, parent PSP={:04X}",
                        entry_cs, entry_ip, parent_psp_before
                    ));
                } else {
                    // Load failed — release the MCBs we allocated, pop the
                    // context we just saved, and return an error to the parent.
                    let _ = crate::mcb::free(&mut cpu.bus, load_segment);
                    let _ = crate::mcb::free(&mut cpu.bus, env_seg);
                    cpu.restore_process_context();
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                    cpu.set_reg16(Register::AX, 0x02); // File not found
                }
            } else if mode == 0x03 {
                // AL=03h: Load Overlay. Loads an EXE (or raw binary) into
                // caller-provided memory and applies relocations, without
                // changing CS:IP or starting execution. Used by installers and
                // games that page-in optional modules at runtime.
                //
                // Parameter block pointed to by ES:BX:
                //   +0 (word): load segment for the overlay image
                //   +2 (word): relocation factor added to relocation targets
                let param_block = cpu.bx();
                let param_seg = cpu.es();
                let param_phys = cpu.get_physical_addr(param_seg, param_block);
                let overlay_seg = cpu.bus.read_16(param_phys);
                let reloc_factor = cpu.bus.read_16(param_phys + 2);

                if cpu.load_overlay(&filename, overlay_seg, reloc_factor) {
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                } else {
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                    cpu.set_reg16(Register::AX, 0x02); // File not found
                }
            } else {
                cpu.bus.log_string(&format!(
                    "[DOS] EXEC Unsupported Mode AL={:02X}",
                    mode
                ));
                cpu.set_cpu_flag(CpuFlags::CF, true);
                cpu.set_reg16(Register::AX, 0x01); // Invalid function
            }
        }

        // AH = 2Ch: Get System Time
        // Returns: CH=Hour, CL=Minute, DH=Second, DL=1/100s
        0x2C => {
            let now = cpu.bus.cmos.now();

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
            cpu.set_es(cpu.bus.dta_segment);
            cpu.set_reg16(Register::BX, cpu.bus.dta_offset);
        }

        // AH = 30h: Get DOS Version
        0x30 => {
            cpu.set_reg8(Register::AL, 5); // Major: 5
            cpu.set_reg8(Register::AH, 0); // Minor: .00
            cpu.set_bx(0xFF00); // OEM ID
            cpu.set_cx(0x0000); // Serial
        }

        // AH = 50h: Set current PSP to BX
        0x50 => {
            cpu.current_psp = cpu.bx();
        }

        // AH = 26h: A new PSP at DX, a copy of the running process's with
        // its handle table as it is.
        0x26 => {
            let parent = cpu.current_psp;
            let (segment, top) = (cpu.dx(), cpu.bus.read_16(parent as usize * 16 + 2));
            dos_files::new_psp(&mut cpu.bus, segment, parent, top, false);
        }

        // AH = 55h: A PSP at DX for a child of the running process, which
        // inherits its handles as EXEC's children do, with SI paragraphs
        // of memory. It becomes the current process (Windows makes its
        // tasks' PSPs so).
        0x55 => {
            let (segment, parent, paras) = (cpu.dx(), cpu.current_psp, cpu.si());
            dos_files::new_psp(&mut cpu.bus, segment, parent, segment.wrapping_add(paras), true);
            cpu.current_psp = segment;
            cpu.set_reg8(Register::AL, 0xF0);
        }

        // AH = 51h / 62h: Get current PSP into BX
        0x51 | 0x62 => {
            cpu.set_bx(cpu.current_psp);
        }

        // AH = 52h: Get List of Lists. ES:BX -> SYSVARS, first MCB at ES:[BX-2]
        0x52 => {
            cpu.set_es(dos_data::SEGMENT);
            cpu.set_bx(dos_data::SYSVARS);
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

            if !cpu.process_stack.is_empty() {
                // The program keeps DX paragraphs (at least its PSP); the
                // rest of its block goes back to the parent, which loaded
                // it (a game loading its sound driver, for one).
                let _ = crate::mcb::resize(&mut cpu.bus, tsr_psp, paras_to_keep.max(6));
            }
            if cpu.return_to_parent() {
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

                cpu.set_ax(return_code as u16); // Set return code (AL)
                cpu.set_cpu_flag(CpuFlags::CF, false);
            } else {
                // Started from the shell: stay resident under the next programs.
                cpu.keep_resident(tsr_psp, paras_to_keep);
                cpu.state = CpuState::RebootShell;
                cpu.errorlevel = return_code;
            }
            cpu.last_child_exit = 0x0300 | return_code as u16;
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
            cpu.set_bx((off_high << 8) | off_low);

            let seg_low = cpu.bus.read_8(phys_addr + 2) as u16;
            let seg_high = cpu.bus.read_8(phys_addr + 3) as u16;
            cpu.set_es((seg_high << 8) | seg_low);
        }

        // AH=36h: Get Disk Free Space
        0x36 => {
            let dl = cpu.get_reg8(Register::DL);
            match cpu.bus.disk.get_disk_free_space(dl) {
                Ok((sectors, available, bytes_per_sec, total)) => {
                    // Values per drive type come from DiskController and
                    // stay small enough for 16-bit free-space math.
                    cpu.set_reg16(Register::AX, sectors);
                    cpu.set_reg16(Register::BX, available);
                    cpu.set_reg16(Register::CX, bytes_per_sec);
                    cpu.set_reg16(Register::DX, total);
                }
                Err(_) => {
                    cpu.set_reg16(Register::AX, 0xFFFF);
                }
            }
        }

        // AH=39h: Create Directory (MKDIR)
        // DS:DX -> ASCIZ directory name
        0x39 => {
            let addr = cpu.get_physical_addr(cpu.ds(), cpu.dx());
            let path = read_asciiz_string(&cpu.bus, addr);
            match cpu.bus.disk.create_directory(&path) {
                Ok(()) => cpu.set_cpu_flag(CpuFlags::CF, false),
                Err(code) => {
                    cpu.set_ax(code as u16);
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                    cpu.bus
                        .log_string(&format!("[DOS] MKDIR '{}' -> Err {:02X}", path, code));
                }
            }
        }

        // AH=3Ah: Remove Directory (RMDIR)
        // DS:DX -> ASCIZ directory name
        0x3A => {
            let addr = cpu.get_physical_addr(cpu.ds(), cpu.dx());
            let path = read_asciiz_string(&cpu.bus, addr);
            match cpu.bus.disk.remove_directory(&path) {
                Ok(()) => cpu.set_cpu_flag(CpuFlags::CF, false),
                Err(code) => {
                    cpu.set_ax(code as u16);
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                    cpu.bus
                        .log_string(&format!("[DOS] RMDIR '{}' -> Err {:02X}", path, code));
                }
            }
        }

        // AH=3Bh: Set Current Directory (CHDIR)
        0x3B => {
            let addr = cpu.get_physical_addr(cpu.ds(), cpu.dx());
            let path = read_asciiz_string(&cpu.bus, addr);
            if cpu.bus.disk.set_current_directory(&path) {
                let drive = cpu.bus.disk.drive_of(&path).unwrap_or(cpu.bus.disk.get_current_drive());
                dos_data::write_cds(&mut cpu.bus, drive);
                cpu.set_cpu_flag(CpuFlags::CF, false);
            } else {
                cpu.set_cpu_flag(CpuFlags::CF, true);
                cpu.set_ax(0x03); // Path not found
            }
        }

        // AH=3Ch: Create File
        //
        // DOS standard behavior is create-OR-truncate, but because our
        // emulator is still crash-prone on real game binaries, an eager
        // truncate wipes user data (roster files, saved games, regn.3dg
        // theatre selection) whenever the program dies before its matching
        // write completes. Prefer the safer "open for read/write, create if
        // missing, do NOT truncate" semantics here. Programs that genuinely
        // want to shrink an existing file can still do so via AH=40h with
        // CX=0 once they have real data to write.
        0x3C => {
            let addr = cpu.get_physical_addr(cpu.ds(), cpu.dx());
            let filename = read_asciiz_string(&cpu.bus, addr);
            // Attributes in CX are ignored for now (TODO)
            let opened = cpu.bus.disk.create_file(&filename, cpu.current_psp);
            match opened.and_then(|sft| attach(cpu, sft).map(|handle| (sft, handle))) {
                Ok((sft, handle)) => {
                    file_io(cpu, sft, diskio::CREATE_BYTES, None);
                    cpu.set_ax(handle);
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                }
                Err(code) => {
                    cpu.bus.log_string(&format!(
                        "[DOS] Create File '{}' Failed, Error={:02X}",
                        filename, code
                    ));
                    cpu.set_ax(code as u16);
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                }
            }
        }

        // AH=3Dh: Open File
        0x3D => {
            let addr = cpu.get_physical_addr(cpu.ds(), cpu.dx());
            let filename = read_asciiz_string(&cpu.bus, addr);
            let mode = cpu.get_al();

            let opened = cpu.bus.disk.open_file(&filename, mode, cpu.current_psp);
            match opened.and_then(|sft| attach(cpu, sft).map(|handle| (sft, handle))) {
                Ok((sft, handle)) => {
                    file_io(cpu, sft, diskio::OPEN_BYTES, None);
                    cpu.set_ax(handle);
                    // In real CPU, clear CF here
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                }
                Err(code) => {
                    cpu.set_ax(code as u16);
                    // In real CPU, set CF here
                    cpu.bus.log_string(&format!(
                        "[DOS] Open File '{}' Mode={:02X} Failed, Error={:04X}",
                        filename, mode, code
                    ));
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                }
            }
        }

        // AH = 3Eh: Close File
        0x3E => {
            let handle = cpu.bx();
            match dos_files::close(&mut cpu.bus, cpu.current_psp, handle) {
                Ok(()) => cpu.set_cpu_flag(CpuFlags::CF, false),
                Err(code) => set_result(cpu, Err(code)),
            }
        }

        // AH = 3Fh: Read from File (or Stdin)
        0x3F => {
            let handle = cpu.bx();
            let count = cpu.cx() as usize;
            let mut buf_addr = cpu.get_physical_addr(cpu.ds(), cpu.dx());

            let Some(sft) = file_of(cpu, handle) else {
                return set_result(cpu, Err(0x06));
            };
            if cpu.bus.disk.handle_device(sft) == Some(CharDevice::Con) {
                // The console: read a line at a time as DOS does,
                // waiting for Enter, and hand out the line with its CR LF
                // over as many reads as it takes.
                if cpu.con_pending.is_empty() && count > 0 {
                    let Some(line) = edit_line(cpu, 127) else { return };
                    con_echo(cpu, b"\r\n");
                    cpu.con_pending.extend(line);
                    cpu.con_pending.extend([0x0D, 0x0A]);
                }
                let taken: Vec<u8> = cpu.con_pending.drain(..count.min(cpu.con_pending.len())).collect();
                cpu.bus.load_bytes(buf_addr, &taken);
                cpu.set_ax(taken.len() as u16);
                cpu.set_cpu_flag(CpuFlags::CF, false);
            } else {
                match cpu.bus.disk.read_file(sft, count) {
                    Ok(bytes) => {
                        file_io(cpu, sft, bytes.len() as u32, Some(false));
                        for b in &bytes {
                            cpu.bus.write_8(buf_addr, *b);
                            buf_addr += 1;
                        }
                        cpu.set_ax(bytes.len() as u16);
                        cpu.set_cpu_flag(CpuFlags::CF, false);
                    }
                    Err(e) => {
                        cpu.bus.log_string(&format!(
                            "[DOS] Read Handle {:04X} Failed, Error={:04X}",
                            handle, e
                        ));
                        cpu.set_ax(e);
                        cpu.set_cpu_flag(CpuFlags::CF, true);
                    }
                }
            }
        }

        // AH = 40h: Write to File (or Stdout)
        //
        // DOS standard: with CX=0 this would truncate the file at the
        // current seek position. We don't perform that truncation right now
        // because programs in this emulator still crash mid-flow and would
        // take real user data (rosters, saves, copied region files) down
        // with them. Log the call so we can spot legitimate truncations
        // while tracing and re-enable later if something depends on it.
        0x40 => {
            let handle = cpu.bx();
            let count = cpu.cx() as usize;
            let buf_addr = cpu.get_physical_addr(cpu.ds(), cpu.dx());

            let Some(sft) = file_of(cpu, handle) else {
                return set_result(cpu, Err(0x06));
            };
            // Handles 1 and 2 are the screen unless the command line
            // redirected them.
            let console = cpu.bus.disk.handle_device(sft) == Some(CharDevice::Con);
            if count == 0 && !console {
                cpu.bus.log_string(&format!(
                    "[DOS] AH=40h CX=0 on handle {:04X} — truncate-at-pos NOT performed (safety)",
                    handle
                ));
                cpu.set_ax(0);
                cpu.set_cpu_flag(CpuFlags::CF, false);
                return;
            }

            let mut data = Vec::with_capacity(count);
            for i in 0..count {
                data.push(cpu.bus.read_8(buf_addr + i));
            }

            if console {
                // STDOUT/STDERR, or CON opened by name
                for &byte in &data {
                    if byte == 0x07 {
                        play_sdl_beep(&mut cpu.bus);
                    }
                }
                let s = String::from_utf8_lossy(&data);
                let visual_s = s.replace('\x07', "");
                crate::video::print_string(cpu, &visual_s);
                cpu.set_ax(count as u16);
            } else {
                match cpu.bus.disk.write_file(sft, &data) {
                    Ok(written) => {
                        file_io(cpu, sft, written as u32, Some(true));
                        cpu.set_ax(written);
                        cpu.set_cpu_flag(CpuFlags::CF, false);
                    }
                    Err(code) => {
                        // e.g. 05h on a file opened read-only (CD-ROM)
                        cpu.bus.log_string(&format!(
                            "[DOS] Write Handle {:04X} Failed, Error={:02X}",
                            handle, code
                        ));
                        cpu.set_ax(code as u16);
                        cpu.set_cpu_flag(CpuFlags::CF, true);
                    }
                }
            }
        }

        // AH = 42h: Move File Pointer
        0x42 => {
            let handle = cpu.bx();
            let offset_high = cpu.cx() as u32;
            let offset_low = cpu.dx() as u32;
            let offset = ((offset_high << 16) | offset_low) as i32;
            let whence = cpu.get_al();

            let Some(sft) = file_of(cpu, handle) else {
                return set_result(cpu, Err(0x06));
            };
            match cpu.bus.disk.seek_file(sft, offset as i64, whence) {
                Ok(new_pos) => {
                    file_io(cpu, sft, diskio::SEEK_BYTES, None);
                    cpu.set_dx(((new_pos >> 16) & 0xFFFF) as u16);
                    cpu.set_ax((new_pos & 0xFFFF) as u16);
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                }
                Err(e) => {
                    cpu.set_ax(e);
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                }
            }
        }

        // AH=43h: Get/Set File Attributes
        // AL=00: Get attributes for file at DS:DX -> CX
        // AL=01: Set attributes (CX = new attributes)
        // Attribute bits: 0x01=R/O, 0x02=Hidden, 0x04=System, 0x10=Directory, 0x20=Archive
        0x43 => {
            let al = cpu.get_reg8(Register::AL);
            let addr = cpu.get_physical_addr(cpu.ds(), cpu.dx());
            let filename = read_asciiz_string(&cpu.bus, addr);
            match al {
                0x00 => match cpu.bus.disk.get_file_attribute(&filename) {
                    Ok(attr) => {
                        cpu.set_reg16(Register::CX, attr);
                        cpu.set_cpu_flag(CpuFlags::CF, false);
                    }
                    Err(code) => {
                        cpu.set_ax(code as u16);
                        cpu.set_cpu_flag(CpuFlags::CF, true);
                    }
                },
                0x01 => {
                    let new_attr = cpu.cx();
                    match cpu.bus.disk.set_file_attribute(&filename, new_attr) {
                        Ok(()) => {
                            cpu.set_cpu_flag(CpuFlags::CF, false);
                        }
                        Err(code) => {
                            cpu.set_ax(code as u16);
                            cpu.set_cpu_flag(CpuFlags::CF, true);
                        }
                    }
                }
                _ => {
                    cpu.set_ax(0x01); // invalid function
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                }
            }
        }

        // AH = 44h: IOCTL (I/O Control)
        0x44 => {
            let al = cpu.get_al();
            // The open file of the handle in BX, for the calls that take one.
            let sft = file_of(cpu, cpu.bx());
            let device = sft.and_then(|sft| cpu.bus.disk.handle_device(sft));

            // cpu.bus.log_string(&format!(
            //     "[DOS] IOCTL AH=44h AL={:02X} Handle={:04X}",
            //     al, bx
            // ));

            match al {
                // Get Device Information
                0x00 => {
                    // Bit 7=1 (Char Dev), Bit 6=0 (EOF), Bit 0=1 (Console Input)
                    // For STDIN(0), STDOUT(1), STDERR(2), return 0x80D3 or similar.
                    let Some(sft) = sft else {
                        return set_result(cpu, Err(0x06));
                    };
                    if sft == crate::disk::SFT_AUX || sft == crate::disk::SFT_PRN {
                        // AUX and PRN: character devices, not EOF.
                        cpu.set_dx(0x80C0);
                    } else if device == Some(CharDevice::Nul) {
                        // Character device, NUL.
                        cpu.set_dx(0x8084);
                    } else if device == Some(CharDevice::Emm) {
                        // The expanded memory manager: a character device
                        // taking IOCTL.
                        cpu.set_dx(0xC080);
                    } else if device == Some(CharDevice::Con) {
                        // 1000 0000 1101 0011 = 80D3
                        // Bit 7: Char device
                        // Bit 6: EOF (0) - meaningful for files?
                        // Bit 5: Raw (Binary) mode? (0=Cooked, 1=Raw)
                        // Bit 4: Special?
                        // Bit 3: Clock?
                        // Bit 2: NUL?
                        // Bit 1: Stdout
                        // Bit 0: Stdin
                        cpu.set_dx(0x80D3);
                    } else {
                        // File: Bit 7=0 (Block Dev), Bits 0-5 = Drive # (0=A)
                        let drive = cpu
                            .bus
                            .disk
                            .handle_drive(sft)
                            .unwrap_or(cpu.bus.disk.get_current_drive());
                        cpu.set_dx(drive as u16);
                    }
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                    // cpu.bus.log_string(&format!("[DOS] IOCTL Get Device Info -> {:04X}", cpu.dx()));
                }
                // Check if Block Device is Removable (BL = drive, 0=default)
                0x08 => {
                    let drive = dos_drive_number(cpu, cpu.get_reg8(Register::BL));
                    match cpu.bus.disk.drive_kind(drive) {
                        None => {
                            cpu.set_ax(0x0F); // Invalid drive
                            cpu.set_cpu_flag(CpuFlags::CF, true);
                        }
                        Some(DriveKind::CdRom) => {
                            // Redirector drives don't support this call
                            cpu.set_ax(0x01);
                            cpu.set_cpu_flag(CpuFlags::CF, true);
                        }
                        Some(kind) => {
                            // AX=0 (Removable), AX=1 (Fixed)
                            cpu.set_ax(if kind.is_removable() { 0 } else { 1 });
                            cpu.set_cpu_flag(CpuFlags::CF, false);
                        }
                    }
                }
                // Check if Block Device is Remote (BL = drive, 0=default).
                // Bit 12 of DX marks a remote (redirected) drive, which is how
                // programs recognise MSCDEX CD-ROM drives. Local drives get
                // the same attribute bits DOSBox reports.
                0x09 => {
                    let drive = dos_drive_number(cpu, cpu.get_reg8(Register::BL));
                    match cpu.bus.disk.drive_kind(drive) {
                        None => {
                            cpu.set_ax(0x0F);
                            cpu.set_cpu_flag(CpuFlags::CF, true);
                        }
                        Some(kind) => {
                            cpu.set_dx(if kind == DriveKind::CdRom { 0x1000 } else { 0x0802 });
                            cpu.set_cpu_flag(CpuFlags::CF, false);
                        }
                    }
                }
                0x0D => generic_block_ioctl(cpu),
                // The expanded memory manager is always ready, which is
                // how programs tell EMMXXXX0 from a file of that name; it
                // hands no control data to programs.
                0x06 | 0x07 if device == Some(CharDevice::Emm) => {
                    cpu.set_reg8(Register::AL, 0xFF);
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                }
                // What Windows takes expanded memory over with.
                0x02 if device == Some(CharDevice::Emm) => {
                    let (buffer, size) = (cpu.get_physical_addr(cpu.ds(), cpu.dx()), cpu.cx());
                    let result = crate::ems::ioctl_read(&mut cpu.bus, buffer, size);
                    set_result(cpu, result);
                }
                _ => {
                    // Stub other subfunctions as success
                    cpu.set_ax(0);
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                }
            }
        }
        // AH=47h: Get Current Directory
        0x47 => {
            let dl = cpu.get_dl(); // Drive (0=Default, 1=A, ...)
            let ds = cpu.ds();
            let si = cpu.get_reg16(Register::SI);
            let addr = cpu.get_physical_addr(ds, si);
            let drive = dos_drive_number(cpu, dl);
            let Some(cwd) = cpu.bus.disk.get_current_directory_of(drive) else {
                cpu.set_ax(0x0F); // Invalid drive
                cpu.set_cpu_flag(CpuFlags::CF, true);
                return;
            };

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

        // AH = 48h: Allocate Memory (MCB chain, first-fit)
        //   BX = paragraphs requested
        //   Return on success: AX = segment of first usable paragraph,
        //     CF = 0. BX is unchanged.
        //   Return on failure: AX = 0008 (insufficient memory), BX = size of
        //     largest available block, CF = 1.
        0x48 => {
            let requested = cpu.bx();
            let owner = if cpu.current_psp != 0 {
                cpu.current_psp
            } else {
                // Before any program is running (shell), give blocks an
                // "owner" equal to the allocating segment so they aren't
                // confused with free blocks.
                0x0008
            };
            match crate::mcb::alloc_strategy(&mut cpu.bus, owner, requested, cpu.alloc_strategy) {
                Ok(segment) => {
                    cpu.set_ax(segment);
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                }
                Err(max_free) => {
                    cpu.set_ax(crate::mcb::ERR_INSUFFICIENT as u16);
                    cpu.set_bx(max_free);
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                    cpu.bus.log_string(&format!(
                        "[DOS] Alloc {:04X} paras failed (max free {:04X})",
                        requested, max_free
                    ));
                }
            }
        }

        // AH = 49h: Free Memory Block
        //   ES = segment returned by AH=48h
        0x49 => {
            let segment_to_free = cpu.es();
            match crate::mcb::free(&mut cpu.bus, segment_to_free) {
                Ok(()) => {
                    cpu.set_cpu_flag(CpuFlags::CF, false);
                    cpu.set_ax(0);
                }
                Err(code) => {
                    cpu.set_ax(code as u16);
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                    cpu.bus.log_string(&format!(
                        "[DOS] Free {:04X} -> Err {:02X}",
                        segment_to_free, code
                    ));
                }
            }
        }

        // AH = 4Ah: Resize Memory Block
        //   ES = segment to resize, BX = new size in paragraphs
        //   On failure: AX = 0008, BX = largest size the block could be grown to.
        0x4A => {
            let segment = cpu.es();
            let requested_size = cpu.get_reg16(Register::BX);
            match crate::mcb::resize(&mut cpu.bus, segment, requested_size) {
                Ok(()) => cpu.set_cpu_flag(CpuFlags::CF, false),
                Err(max) => {
                    cpu.set_reg16(Register::BX, max);
                    cpu.set_reg16(Register::AX, crate::mcb::ERR_INSUFFICIENT as u16);
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                    cpu.bus.log_string(&format!(
                        "[DOS] Resize {:04X} to {:04X} paras failed (max {:04X})",
                        segment, requested_size, max
                    ));
                }
            }
        }

        // AH = 4Ch: Terminate Program
        // AL = exit code. Record for AH=4Dh (Get Return Code) and free any
        // MCBs owned by the terminating PSP.
        0x4C => {
            let exit_code = cpu.get_al();
            cpu.bus.log_string(&format!(
                "[DOS] Program Terminated (INT 21h, 4Ch). ExitCode={:02X}",
                exit_code
            ));

            // Kept for the parent to retrieve, or as the ERRORLEVEL. High
            // byte = termination type 0 (normal). A process a program made
            // returns with the registers its parent saved.
            if !cpu.started_by_exec(cpu.current_psp) {
                cpu.terminate(exit_code);
            } else if cpu.terminate(exit_code) {
                cpu.bus.log_string("[DOS] Returning to Parent Process");
                cpu.set_ax(exit_code as u16);
                cpu.set_cpu_flag(CpuFlags::CF, false);
            }
        }

        // AH = 4Dh: Get Return Code of child process (ERRORLEVEL)
        //   AL = exit code, AH = termination type (0=normal, 1=Ctrl-C, 2=crit, 3=TSR)
        // Subsequent calls return zero until the next child terminates.
        0x4D => {
            cpu.set_ax(cpu.last_child_exit);
            cpu.last_child_exit = 0;
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH=4Eh (Find First) / AH=4Fh (Find Next)
        0x4E | 0x4F => {
            let dta_seg = cpu.bus.dta_segment;
            let dta_off = cpu.bus.dta_offset;
            let dta_phys = cpu.get_physical_addr(dta_seg, dta_off);

            const OFFSET_ATTR_SEARCH: usize = 12;
            const OFFSET_INDEX: usize = 13;

            let (index, search_attr, raw_pattern, search_id) = if ah == 0x4E {
                let name_addr = cpu.get_physical_addr(cpu.ds(), cpu.dx());
                let mut pattern = read_asciiz_string(&cpu.bus, name_addr);

                // Heuristic Fix for d.com (and potentially others):
                // If the search pattern ends with the Current Directory name followed by a wildcard
                // (e.g. "C:\TEXT.*" or "C:\TEXT.???") WITHOUT a path separator, it implies
                // the program intended to search the CONTENTS, but concatenated CWD + wildcard blindly.
                // We detect this and insert the missing separator (e.g. "C:\TEXT\*.*").
                // The CWD is that of the drive the pattern names.
                let spec_drive = parse_drive_prefix(&pattern)
                    .0
                    .unwrap_or(cpu.bus.disk.get_current_drive());
                let cwd = cpu
                    .bus
                    .disk
                    .get_current_directory_of(spec_drive)
                    .unwrap_or_default();

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
                cpu.bus.search_serial = cpu.bus.search_serial.wrapping_add(1);
                let sid = cpu.bus.search_serial;
                (0, cpu.cx(), pattern, sid)
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

                // Construct full pattern. FindFirst stores a fully qualified
                // directory ("D:\\SUB" or "D:\\"), so later drive or directory
                // changes don't redirect the search.
                let full_pattern = if dir_prefix.is_empty() {
                    filename_pattern
                } else if dir_prefix.ends_with('\\') {
                    format!("{}{}", dir_prefix, filename_pattern)
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
                    // Drive the search runs on (1 = A:)
                    let search_drive = parse_drive_prefix(&search_pattern)
                        .0
                        .unwrap_or(cpu.bus.disk.get_current_drive());
                    cpu.bus.write_8(dta_phys + 0, search_drive + 1);
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

                    // Store Directory Context if FindFirst (AH=4E), fully
                    // qualified with drive and current directory.
                    if ah == 0x4E {
                        if let Some(dir) = cpu.bus.disk.qualify_directory(&search_pattern) {
                            cpu.bus.search_handles.insert(unique_id, dir);
                        }
                    }
                    cpu.bus
                        .write_16(dta_phys + 19, (index as u16).wrapping_mul(3));

                    // File Attributes
                    cpu.bus.write_8(dta_phys + 21, entry.attr);

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

        // AH = 0Dh: Disk reset (flush buffers). Nothing is buffered.
        0x0D => {}

        // AH = 2Ah: Get date. CX=year, DH=month, DL=day, AL=day of week.
        0x2A => {
            let now = cpu.bus.cmos.now();
            cpu.set_cx(now.year() as u16);
            cpu.set_dx(((now.month() as u16) << 8) | now.day() as u16);
            cpu.set_reg8(Register::AL, now.weekday().num_days_from_sunday() as u8);
        }

        // AH = 2Bh: Set date (CX year, DH month, DL day), AH = 2Dh: Set
        // time (CH hour, CL minute, DH second, DL hundredths): AL=FFh if
        // it isn't one.
        0x2B | 0x2D => {
            let now = cpu.bus.cmos.now();
            let at = if ah == 0x2B {
                chrono::NaiveDate::from_ymd_opt(cpu.cx() as i32, (cpu.dx() >> 8) as u32, cpu.get_dl() as u32)
                    .filter(|d| (1980..=2099).contains(&d.year()))
                    .map(|date| date.and_time(now.time()))
            } else {
                let (h, m) = ((cpu.cx() >> 8) as u32, (cpu.cx() & 0xFF) as u32);
                chrono::NaiveTime::from_hms_milli_opt(h, m, (cpu.dx() >> 8) as u32, cpu.get_dl() as u32 * 10)
                    .filter(|_| cpu.get_dl() < 100)
                    .map(|time| now.date().and_time(time))
            };
            match at {
                Some(at) => {
                    cpu.bus.cmos.set_now(at);
                    cpu.set_reg8(Register::AL, 0);
                }
                None => cpu.set_reg8(Register::AL, 0xFF),
            }
        }

        // AH = 34h: Address of the InDOS flag. DOS services run in one step
        // here, so it's never set when a program looks.
        0x34 => {
            cpu.set_es(dos_data::SEGMENT);
            cpu.set_bx(dos_data::INDOS);
        }

        // AX = 5D06h: The swappable data area in DS:SI, its size in CX and
        // the size of the part always swapped in DX.
        0x5D if cpu.get_al() == 0x06 => {
            cpu.set_ds(dos_data::SEGMENT);
            cpu.set_si(dos_data::SDA);
            cpu.set_cx(dos_data::SDA_SIZE);
            cpu.set_dx(dos_data::SDA_ALWAYS);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH = 37h: Get (AL=00h) or set (AL=01h) the switch character,
        // which stays '/'.
        0x37 => match cpu.get_al() {
            0x00 => {
                cpu.set_reg8(Register::AL, 0);
                cpu.set_reg8(Register::DL, b'/');
            }
            0x01 => cpu.set_reg8(Register::AL, 0),
            _ => cpu.set_reg8(Register::AL, 0xFF),
        },

        // AH = 38h: Get (or with DX=FFFFh set) country information.
        0x38 => {
            if cpu.dx() != 0xFFFF {
                let addr = cpu.get_physical_addr(cpu.ds(), cpu.dx());
                write_country_info(cpu, addr);
            }
            cpu.set_bx(1); // country code: USA
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH = 41h: Delete file (DS:DX).
        0x41 => {
            let filename = read_asciiz_string(&cpu.bus, cpu.get_physical_addr(cpu.ds(), cpu.dx()));
            let result = cpu.bus.disk.delete_file(&filename).map(|_| cpu.ax());
            set_result(cpu, result);
        }

        // AH = 45h: Duplicate handle BX. AH = 46h: make handle CX refer to
        // the file of handle BX.
        0x45 | 0x46 => {
            let (psp, handle, target, ax) = (cpu.current_psp, cpu.bx(), cpu.cx(), cpu.ax());
            let result = if ah == 0x46 {
                dos_files::force_duplicate(&mut cpu.bus, psp, handle, target).map(|()| ax)
            } else {
                dos_files::duplicate(&mut cpu.bus, psp, handle)
            };
            set_result(cpu, result);
        }

        // AH = 56h: Rename file DS:DX to ES:DI.
        0x56 => {
            let from = read_asciiz_string(&cpu.bus, cpu.get_physical_addr(cpu.ds(), cpu.dx()));
            let to = read_asciiz_string(&cpu.bus, cpu.get_physical_addr(cpu.es(), cpu.di()));
            let result = cpu.bus.disk.rename_file(&from, &to).map(|_| cpu.ax());
            set_result(cpu, result);
        }

        // AH = 57h: Get (AL=0) or set (AL=1) a file's date and time.
        0x57 => {
            let Some(handle) = file_of(cpu, cpu.bx()) else {
                return set_result(cpu, Err(0x06));
            };
            if cpu.get_al() == 0 {
                match cpu.bus.disk.file_time(handle) {
                    Ok((time, date)) => {
                        cpu.set_cx(time);
                        cpu.set_dx(date);
                        cpu.set_cpu_flag(CpuFlags::CF, false);
                    }
                    Err(e) => set_result(cpu, Err(e)),
                }
            } else {
                let result = cpu.bus.disk.set_file_time(handle, cpu.cx(), cpu.dx());
                set_result(cpu, result.map(|()| 0));
            }
        }

        // AH = 58h: Get (AL=00h) or set (AL=01h, BX) the memory allocation
        // strategy: first, best or last fit (0-2), in upper memory only
        // (40h) or first (80h); get (AL=02h) or set (AL=03h, BX) whether
        // upper memory is linked to conventional memory.
        0x58 => {
            let ok = match cpu.get_al() {
                0x00 => {
                    cpu.set_ax(cpu.alloc_strategy);
                    true
                }
                0x01 if matches!(cpu.bx() & 0x3F, 0..=2) && matches!(cpu.bx() & !0x3F, 0x00 | 0x40 | 0x80) => {
                    cpu.alloc_strategy = cpu.bx();
                    true
                }
                0x02 => {
                    let linked = cpu.bus.umb.is_some_and(|u| u.linked);
                    cpu.set_ax(linked as u16);
                    true
                }
                0x03 if cpu.bx() <= 1 => {
                    let on = cpu.bx() == 1;
                    crate::mcb::link_upper(&mut cpu.bus, on).is_ok()
                }
                _ => false,
            };
            if !ok {
                cpu.set_ax(0x01);
            }
            cpu.set_cpu_flag(CpuFlags::CF, !ok);
        }

        // AH = 59h: Extended error information for the last failed call.
        0x59 => {
            let error = cpu.last_dos_error;
            cpu.set_ax(error);
            // Class: file/item not found, or other; action: abort; locus:
            // block device.
            let class = if matches!(error, 2 | 3) { 8 } else { 13 };
            cpu.set_bx((class << 8) | 4);
            cpu.set_reg8(Register::CH, 2);
        }

        // AH = 5Ah: Create a temporary file in the directory at DS:DX,
        // whose name is appended there.
        0x5A => {
            let addr = cpu.get_physical_addr(cpu.ds(), cpu.dx());
            let mut dir = read_asciiz_string(&cpu.bus, addr);
            if !dir.is_empty() && !dir.ends_with('\\') {
                dir.push('\\');
            }
            let created = cpu.bus.disk.create_temp_file(&dir, cpu.current_psp);
            match created.and_then(|(sft, name)| attach(cpu, sft).map(|handle| (sft, handle, name))) {
                Ok((sft, handle, name)) => {
                    file_io(cpu, sft, diskio::CREATE_BYTES, None);
                    for (i, b) in name.bytes().chain(std::iter::once(0)).enumerate() {
                        cpu.bus.write_8(addr + i, b);
                    }
                    set_result(cpu, Ok(handle));
                }
                Err(e) => set_result(cpu, Err(e)),
            }
        }

        // AH = 5Bh: Create a new file (fails if it exists).
        0x5B => {
            let filename = read_asciiz_string(&cpu.bus, cpu.get_physical_addr(cpu.ds(), cpu.dx()));
            let created = cpu.bus.disk.create_new_file(&filename, cpu.current_psp);
            let result = created.and_then(|sft| attach(cpu, sft).map(|handle| (sft, handle)));
            if let Ok((sft, _)) = result {
                file_io(cpu, sft, diskio::CREATE_BYTES, None);
            }
            set_result(cpu, result.map(|(_, handle)| handle));
        }

        // AH = 60h: Canonical ("true") name of DS:SI into ES:DI.
        0x60 => {
            let name = read_asciiz_string(&cpu.bus, cpu.get_physical_addr(cpu.ds(), cpu.si()));
            match cpu.bus.disk.qualify_path(&name) {
                Some(full) => {
                    let dest = cpu.get_physical_addr(cpu.es(), cpu.di());
                    for (i, b) in full.bytes().take(127).chain(std::iter::once(0)).enumerate() {
                        cpu.bus.write_8(dest + i, b);
                    }
                    set_result(cpu, Ok(cpu.ax()));
                }
                None => set_result(cpu, Err(0x03)),
            }
        }

        // AH = 65h: Extended country information and character case
        // conversion.
        0x65 => match cpu.get_al() {
            0x01 => {
                // Info ID 1, length, country 1, code page 437, then the
                // country information of AH=38h.
                let dest = cpu.get_physical_addr(cpu.es(), cpu.di());
                cpu.bus.write_8(dest, 0x01);
                cpu.bus.write_16(dest + 1, 38);
                cpu.bus.write_16(dest + 3, 1);
                cpu.bus.write_16(dest + 5, 437);
                write_country_info(cpu, dest + 7);
                cpu.set_cx(41);
                cpu.set_cpu_flag(CpuFlags::CF, false);
            }
            0x20 => {
                let upper = cpu.get_dl().to_ascii_uppercase();
                cpu.set_reg8(Register::DL, upper);
                cpu.set_cpu_flag(CpuFlags::CF, false);
            }
            0x21 | 0x22 => {
                let addr = cpu.get_physical_addr(cpu.ds(), cpu.dx());
                let len = if cpu.get_al() == 0x21 { cpu.cx() as usize } else { usize::MAX };
                for i in 0..len {
                    let b = cpu.bus.read_8(addr + i);
                    if len == usize::MAX && b == 0 {
                        break;
                    }
                    cpu.bus.write_8(addr + i, b.to_ascii_uppercase());
                }
                cpu.set_cpu_flag(CpuFlags::CF, false);
            }
            _ => set_result(cpu, Err(0x01)),
        },

        // AH = 66h: Get (AL=1) or set (AL=2) the global code page.
        0x66 => {
            if cpu.get_al() == 0x01 {
                cpu.set_bx(437);
                cpu.set_dx(437);
            }
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }

        // AH = 67h: Room for BX handles in the running process's table.
        0x67 => {
            let (psp, count) = (cpu.current_psp, cpu.bx());
            let result = dos_files::set_handle_count(&mut cpu.bus, psp, count);
            set_result(cpu, result.map(|()| cpu.ax()));
        }

        // AH = 68h / 6Ah: Commit file. Nothing to do.
        0x68 | 0x6A => cpu.set_cpu_flag(CpuFlags::CF, false),

        // AX = 6C00h: Extended open/create. BL = access mode, DL = action
        // (low nibble: file exists, 0 fail / 1 open / 2 replace; high
        // nibble: file missing, 0 fail / 1 create), DS:SI = name. Returns
        // the handle in AX and the action taken in CX.
        0x6C => {
            let filename = read_asciiz_string(&cpu.bus, cpu.get_physical_addr(cpu.ds(), cpu.si()));
            let mode = cpu.get_reg8(Register::BL);
            let action = cpu.get_dl();
            let exists = cpu.bus.disk.is_file(&filename);
            let psp = cpu.current_psp;
            let result = match (exists, action & 0x0F, action >> 4) {
                (true, 1, _) => cpu.bus.disk.open_file(&filename, mode, psp).map(|h| (h, 1)),
                (true, 2, _) => cpu.bus.disk.create_file(&filename, psp).map(|h| (h, 3)),
                (true, _, _) => Err(0x50),
                (false, _, 1) => cpu.bus.disk.create_file(&filename, psp).map(|h| (h, 2)),
                (false, _, _) => Err(0x02),
            };
            match result.and_then(|(sft, taken)| attach(cpu, sft).map(|handle| (sft, handle, taken))) {
                Ok((sft, handle, taken)) => {
                    let bytes = if taken == 1 { diskio::OPEN_BYTES } else { diskio::CREATE_BYTES };
                    file_io(cpu, sft, bytes, None);
                    cpu.set_cx(taken);
                    set_result(cpu, Ok(handle));
                }
                Err(e) => set_result(cpu, Err(e)),
            }
        }

        // AX = 71xxh: the long file name functions of Windows 95, which
        // aren't there: AX=7100h and CF set, as plain DOS answers.
        0x71 => {
            cpu.set_ax(0x7100);
            cpu.set_cpu_flag(CpuFlags::CF, true);
        }

        _ => {
            // Match DOSBox's default behaviour for unknown INT 21h
            // functions: clear AL, leave AH and CF alone. This keeps
            // callers on the "function not supported" fallback path
            // (including MicroProse's mirror-AX TSR probes, AX=FFFFh /
            // BFBFh). Don't pretend to load files here — that convinces
            // the game its TSR serviced the call and it stops doing the
            // plain INT 21h AH=3Fh reads DOSBox-running VGAME relies on.
            let al_val = cpu.get_al();
            cpu.set_reg8(Register::AL, 0);
            cpu.bus.log_string(&format!(
                "[DOS] Unhandled INT 21h AH={:02X} AL={:02X} BX={:04X} CX={:04X} DX={:04X} DS={:04X} ES={:04X} SI={:04X} DI={:04X}",
                ah,
                al_val,
                cpu.bx(), cpu.cx(), cpu.dx(), cpu.ds(), cpu.es(), cpu.si(), cpu.di()
            ));
        }
    }
}

crate::state_fields!(ConLine { buffer, text });
