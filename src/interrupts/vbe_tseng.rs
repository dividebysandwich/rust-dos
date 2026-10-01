//! VBE 1.2 on the Tseng ET4000 (`machine=svga_et4000`), as Tseng's later
//! BIOSes have it: the VESA modes are Tseng's own modes, seen through
//! the 64 KB window at A0000h that Segment Select (3CDh) banks, with no
//! linear frame buffer. 16 and 256 colours up to 1024x768, and 32K and
//! 64K colours through the Sierra DAC up to 800x600, in the 1 MB.

use super::int10;
use super::vbe::{FAILED, INVALID_NOW, SUCCESS};
use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::video::et4000;
use iced_x86::Register;

/// Where the ET4000's VBE data is in the video BIOS segment C000h.
const ROM_SEGMENT: u16 = 0xC000;
const MODE_LIST: u16 = 0x0300;
const OEM_STRING: u16 = 0x0340;
const OEM: &[u8] = b"Tseng Labs ET4000 VBE 1.2\0";

/// A VESA mode: the Tseng mode it is, and its bits per pixel (4 in
/// planes, 8 through the DAC's palette, 15 and 16 HiColor).
#[derive(Clone, Copy, Debug)]
struct Mode {
    number: u16,
    tseng: u8,
    bpp: u8,
}

const MODES: [Mode; 12] = [
    Mode { number: 0x100, tseng: 0x2F, bpp: 8 },
    Mode { number: 0x101, tseng: 0x2E, bpp: 8 },
    Mode { number: 0x102, tseng: 0x29, bpp: 4 },
    Mode { number: 0x103, tseng: 0x30, bpp: 8 },
    Mode { number: 0x104, tseng: 0x37, bpp: 4 },
    Mode { number: 0x105, tseng: 0x38, bpp: 8 },
    Mode { number: 0x10D, tseng: 0x13, bpp: 15 },
    Mode { number: 0x10E, tseng: 0x13, bpp: 16 },
    Mode { number: 0x110, tseng: 0x2E, bpp: 15 },
    Mode { number: 0x111, tseng: 0x2E, bpp: 16 },
    Mode { number: 0x113, tseng: 0x30, bpp: 15 },
    Mode { number: 0x114, tseng: 0x30, bpp: 16 },
];

/// The card's memory: 1 MB, 256 KB of it each plane.
const MEMORY: u32 = 1 << 20;

fn find(number: u16) -> Option<Mode> {
    MODES.iter().copied().find(|m| m.number == number & 0x1FF)
}

impl Mode {
    fn size(&self) -> (u16, u16) {
        match et4000::tseng_mode(self.tseng) {
            Some(m) => (m.width, m.height),
            None => (320, 200),
        }
    }

    /// Bytes from one scan line to the next, as the mode sets them.
    fn pitch(&self) -> u32 {
        let width = self.size().0 as u32;
        match self.bpp {
            4 => width / 8,
            8 => width,
            _ => width * 2,
        }
    }

    /// The memory a scan line's bytes are in: a plane's, or all of it.
    fn memory(&self) -> u32 {
        if self.bpp == 4 { MEMORY / 4 } else { MEMORY }
    }
}

/// Write the mode list and the OEM string into the video BIOS.
pub fn install_rom(bus: &mut Bus) {
    let rom = |offset: u16| ((ROM_SEGMENT as usize) << 4) + offset as usize;
    let mut list: Vec<u8> = MODES.iter().flat_map(|m| m.number.to_le_bytes()).collect();
    list.extend([0xFF, 0xFF]);
    bus.write_rom(rom(MODE_LIST), &list);
    bus.write_rom(rom(OEM_STRING), OEM);
}

/// INT 10h AH=4Fh on the ET4000.
pub fn handle(cpu: &mut Cpu) {
    let es_di = cpu.get_physical_addr(cpu.es(), cpu.di());
    let status = match cpu.get_al() {
        0x07 => match display_start(cpu) {
            Some(status) => status,
            None => return,
        },
        0x00 => controller_info(cpu, es_di),
        0x01 => mode_info(cpu, cpu.cx(), es_di),
        0x02 => set_mode(cpu, cpu.bx()),
        0x03 => {
            let mode = match cpu.bus.vga.et4000.vbe_mode {
                0 => cpu.bus.guest_read_8(0x0449) as u16,
                mode => mode,
            };
            cpu.set_bx(mode);
            SUCCESS
        }
        0x04 => save_restore_state(cpu),
        0x05 => window(cpu),
        0x06 => scan_line_length(cpu),
        // The DAC width, the palette and the protected-mode interface are
        // VBE 2.0's.
        _ => FAILED,
    };
    cpu.set_ax(status);
}

/// Function 00h: the 256 bytes of a VBE 1.2 VbeInfoBlock, pointing into
/// the ROM.
fn controller_info(cpu: &mut Cpu, addr: usize) -> u16 {
    let far = |offset: u16| (ROM_SEGMENT as u32) << 16 | offset as u32;
    let mut block = [0u8; 256];
    block[0..4].copy_from_slice(b"VESA");
    block[4..6].copy_from_slice(&0x0102u16.to_le_bytes());
    block[6..10].copy_from_slice(&far(OEM_STRING).to_le_bytes());
    block[14..18].copy_from_slice(&far(MODE_LIST).to_le_bytes());
    block[18..20].copy_from_slice(&((MEMORY >> 16) as u16).to_le_bytes());
    cpu.bus.guest_write_bytes(addr as u32, &block);
    SUCCESS
}

/// Function 01h: the ModeInfoBlock of mode CX, without the VBE 2.0
/// fields and without a linear frame buffer.
fn mode_info(cpu: &mut Cpu, number: u16, addr: usize) -> u16 {
    let Some(mode) = find(number) else {
        return FAILED;
    };
    let (width, height) = mode.size();
    let pitch = mode.pitch();
    let mut block = [0u8; 256];
    // Supported, extended information, colour, graphics; the BIOS's text
    // output but in HiColor.
    let attributes: u16 = if mode.bpp > 8 { 0x1B } else { 0x1F };
    block[0..2].copy_from_slice(&attributes.to_le_bytes());
    block[2] = 0x07; // window A: there, readable, writable
    block[4..6].copy_from_slice(&64u16.to_le_bytes()); // granularity, KB
    block[6..8].copy_from_slice(&64u16.to_le_bytes()); // size, KB
    block[8..10].copy_from_slice(&0xA000u16.to_le_bytes());
    let window_function = (ROM_SEGMENT as u32) << 16 | super::vbe::WINDOW_FUNCTION as u32;
    block[12..16].copy_from_slice(&window_function.to_le_bytes());
    block[16..18].copy_from_slice(&(pitch as u16).to_le_bytes());
    block[18..20].copy_from_slice(&width.to_le_bytes());
    block[20..22].copy_from_slice(&height.to_le_bytes());
    block[22] = 8; // character cell
    block[23] = 16;
    block[24] = if mode.bpp == 4 { 4 } else { 1 }; // planes
    block[25] = mode.bpp;
    block[26] = 1; // banks
    block[27] = match mode.bpp {
        4 => 3,  // planar
        8 => 4,  // packed pixel
        _ => 6,  // direct colour
    };
    block[29] = (mode.memory() / (pitch * height as u32)).saturating_sub(1).min(255) as u8;
    block[30] = 1;
    block[31..39].copy_from_slice(&super::vbe::color_masks(mode.bpp));
    cpu.bus.guest_write_bytes(addr as u32, &block);
    SUCCESS
}

/// Function 02h: set mode BX (bit 15 keeping video memory). There is no
/// linear frame buffer to ask for with bit 14.
fn set_mode(cpu: &mut Cpu, bx: u16) -> u16 {
    let keep = if bx & 0x8000 != 0 { 0x80 } else { 0 };
    if bx & 0x1FF < 0x100 {
        int10::set_mode(cpu, (bx & 0x7F) as u8 | keep);
        return SUCCESS;
    }
    let Some(mode) = find(bx).filter(|_| bx & 0x4000 == 0) else {
        return FAILED;
    };
    match mode.bpp {
        4 | 8 => int10::set_mode(cpu, mode.tseng | keep),
        bits => {
            int10::set_hicolor_mode(cpu, mode.tseng | keep, bits);
        }
    }
    cpu.bus.vga.et4000.vbe_mode = mode.number;
    SUCCESS
}

/// The VESA mode the card is in: the one set through function 02h, or
/// the Tseng mode (or 13h) it is.
fn current(cpu: &mut Cpu) -> Option<Mode> {
    match cpu.bus.vga.et4000.vbe_mode {
        0 => {
            let number = cpu.bus.guest_read_8(0x0449);
            let bpp = match et4000::tseng_mode(number).map(|m| m.kind) {
                Some(et4000::Kind::Planar) => 4,
                Some(et4000::Kind::Packed) => 8,
                _ if number == 0x13 => 8,
                _ => return None,
            };
            Some(Mode { number: number as u16, tseng: number, bpp })
        }
        number => find(number),
    }
}

/// The bytes a scan line has now: the CRTC Offset register, in words of a
/// plane in the 16-colour modes and in eight bytes in the chained ones.
fn current_pitch(bus: &Bus, mode: &Mode) -> u32 {
    let words = bus.vga.offset_words() as u32;
    if mode.bpp == 4 { words * 2 } else { words * 8 }
}

/// Function 04h: the size of the state buffer (DL=0), and saving (DL=1)
/// or restoring (DL=2) the mode, the bank, the scan line length and the
/// display start at ES:BX, in one 64-byte block.
fn save_restore_state(cpu: &mut Cpu) -> u16 {
    let addr = cpu.get_physical_addr(cpu.es(), cpu.bx()) as u32;
    match cpu.get_reg8(Register::DL) {
        0x00 => cpu.set_bx(1),
        0x01 => {
            let mode = current(cpu).map_or(0, |m| m.number);
            let bus = &mut cpu.bus;
            let (offset, high) = (bus.vga.crtc_regs[0x13], bus.vga.et4000.crtc(0x3F));
            let (start_high, start_low) = (bus.vga.crtc_regs[0x0C], bus.vga.crtc_regs[0x0D]);
            let (segment, cr33) = (bus.vga.et4000.segment, bus.vga.et4000.crtc(0x33));
            bus.guest_write_16(addr, mode);
            bus.guest_write_bytes(addr + 2, &[segment, offset, high, start_high, start_low, cr33]);
        }
        0x02 => {
            let mode = cpu.bus.guest_read_16(addr);
            let regs: Vec<u8> = (0..6).map(|i| cpu.bus.guest_read_8(addr + 2 + i)).collect();
            if mode != 0 && current(cpu).map(|m| m.number) != Some(mode) {
                set_mode(cpu, mode | 0x8000);
            }
            let bus = &mut cpu.bus;
            bus.vga.et4000.segment = regs[0];
            bus.vga.crtc_regs[0x13] = regs[1];
            bus.vga.et4000.set_offset_high(regs[2] & 0x80 != 0);
            bus.vga.crtc_regs[0x0C] = regs[3];
            bus.vga.crtc_regs[0x0D] = regs[4];
            bus.vga.et4000.set_start_high(regs[5] as usize);
            bus.note_display_start();
            bus.vga.mark_dirty_full();
        }
        _ => return FAILED,
    }
    SUCCESS
}

/// Function 05h, and the window function: BH=0 sets window A (BL=0) to
/// bank DX, for reads and writes; BH=1 returns it in DX.
pub fn window(cpu: &mut Cpu) -> u16 {
    if cpu.get_reg8(Register::BL) != 0 {
        return FAILED;
    }
    match cpu.get_reg8(Register::BH) {
        0x00 => {
            let bank = cpu.dx();
            if bank >= 16 {
                return FAILED;
            }
            cpu.bus.vga.et4000.segment = (bank as u8) * 0x11;
        }
        0x01 => cpu.set_dx(cpu.bus.vga.et4000.bank(true) as u16),
        _ => return FAILED,
    }
    SUCCESS
}

/// Function 06h: set the scan line length in pixels (BL=0) or bytes
/// (BL=2), get it (BL=1) or the longest (BL=3). Returns the bytes in BX,
/// the pixels in CX and the scan lines that fit in DX. Lengths are in
/// steps of a CRTC Offset unit: 2 bytes of a plane, or 8 chained bytes.
fn scan_line_length(cpu: &mut Cpu) -> u16 {
    let Some(mode) = current(cpu) else {
        return INVALID_NOW;
    };
    let (width, height) = mode.size();
    let unit = if mode.bpp == 4 { 2 } else { 8 };
    let to_pixels = |bytes: u32| match mode.bpp {
        4 => bytes * 8,
        8 => bytes,
        _ => bytes / 2,
    };
    let to_bytes = |pixels: u32| match mode.bpp {
        4 => pixels.div_ceil(8),
        8 => pixels,
        _ => pixels * 2,
    };
    let pitch = match cpu.get_reg8(Register::BL) {
        0x00 => to_bytes(cpu.cx() as u32),
        0x01 => current_pitch(&cpu.bus, &mode),
        0x02 => cpu.cx() as u32,
        0x03 => (mode.memory() / height as u32).min(511 * unit) / unit * unit,
        _ => return FAILED,
    };
    if matches!(cpu.get_reg8(Register::BL), 0x00 | 0x02) {
        let pitch = pitch.div_ceil(unit) * unit;
        if pitch < to_bytes(width as u32) || pitch * height as u32 > mode.memory() || pitch / unit > 511 {
            return INVALID_NOW;
        }
        let words = pitch / unit;
        cpu.bus.vga.crtc_regs[0x13] = words as u8;
        cpu.bus.vga.et4000.set_offset_high(words > 0xFF);
        cpu.bus.vga.mark_dirty_full();
    }
    let pitch = if cpu.get_reg8(Register::BL) == 0x03 { pitch } else { current_pitch(&cpu.bus, &mode) };
    cpu.set_bx(pitch as u16);
    cpu.set_cx(to_pixels(pitch) as u16);
    cpu.set_dx((mode.memory() / pitch.max(1)).min(0xFFFF) as u16);
    SUCCESS
}

/// Function 07h: set the display start to pixel CX of scan line DX (BL=0,
/// or BL=80h at the next vertical retrace), or get it (BL=1). None while
/// it waits for the retrace.
fn display_start(cpu: &mut Cpu) -> Option<u16> {
    let Some(mode) = current(cpu) else {
        return Some(INVALID_NOW);
    };
    let pitch = current_pitch(&cpu.bus, &mode).max(1);
    // The CRTC counts a plane's bytes: a chained mode's in fours.
    let per_unit = if mode.bpp == 4 { 1 } else { 4 };
    let x_bytes = |x: u32| match mode.bpp {
        4 => x / 8,
        8 => x,
        _ => x * 2,
    };
    match cpu.get_reg8(Register::BL) {
        bl @ (0x00 | 0x80) => {
            let start = cpu.dx() as u32 * pitch + x_bytes(cpu.cx() as u32);
            if start + pitch * mode.size().1 as u32 > mode.memory() {
                return Some(FAILED);
            }
            let address = start / per_unit;
            cpu.bus.sync_display();
            let bus = &mut cpu.bus;
            bus.vga.crtc_regs[0x0C] = (address >> 8) as u8;
            bus.vga.crtc_regs[0x0D] = address as u8;
            bus.vga.et4000.set_start_high((address >> 16) as usize);
            bus.note_display_start();
            if bl == 0x80 && !super::vbe::wait_for_retrace(cpu) {
                return None;
            }
            cpu.bios_wait_until = None;
        }
        0x01 => {
            let crtc = &cpu.bus.vga.crtc_regs;
            let address = cpu.bus.vga.et4000.start_high() << 16 | (crtc[0x0C] as usize) << 8 | crtc[0x0D] as usize;
            let start = address as u32 * per_unit;
            let x = start % pitch;
            cpu.set_reg8(Register::BH, 0);
            cpu.set_cx(match mode.bpp {
                4 => x * 8,
                8 => x,
                _ => x / 2,
            } as u16);
            cpu.set_dx((start / pitch) as u16);
        }
        _ => return Some(FAILED),
    }
    Some(SUCCESS)
}
