//! The VESA BIOS Extensions, VBE 2.0: INT 10h AH=4Fh, and the video
//! BIOS's VBE data (strings, the mode list and the window function) in the
//! ROM at C000h. The card behind them is `video::vbe`.

use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::video::vbe::{self, VbeMode, LFB_BASE, MODES, VRAM_SIZE, WINDOW_SIZE};
use crate::video::VideoMode;
use iced_x86::Register;

/// Where the VBE data is in the video BIOS segment C000h.
const ROM_SEGMENT: u16 = 0xC000;
const OEM_STRING: u16 = 0x0100;
const VENDOR_NAME: u16 = 0x0120;
const PRODUCT_NAME: u16 = 0x0130;
const PRODUCT_REVISION: u16 = 0x0140;
const MODE_LIST: u16 = 0x0180;
/// The window function modes point at (WinFuncPtr), for programs that
/// switch banks with a far call instead of INT 10h.
pub const WINDOW_FUNCTION: u16 = 0x01C0;
/// The protected-mode interface (function 0Ah).
pub const PM_TABLE: u16 = 0x0200;

const OEM: &[u8] = b"rust-dos VBE 2.0\0";
const VENDOR: &[u8] = b"rust-dos\0";
const PRODUCT: &[u8] = b"rust-dos SVGA\0";
const REVISION: &[u8] = b"2.0\0";

/// VBE status in AX: supported (4Fh) and successful (AH=0), failed, or
/// not possible in the current mode.
const SUCCESS: u16 = 0x004F;
const FAILED: u16 = 0x014F;
const INVALID_NOW: u16 = 0x024F;

fn rom(offset: u16) -> usize {
    ((ROM_SEGMENT as usize) << 4) + offset as usize
}

/// Write the VBE data into the video BIOS.
pub fn install_rom(bus: &mut Bus) {
    for (offset, text) in [(OEM_STRING, OEM), (VENDOR_NAME, VENDOR), (PRODUCT_NAME, PRODUCT), (PRODUCT_REVISION, REVISION)] {
        bus.load_bytes(rom(offset), text);
    }
    let mut list: Vec<u8> = MODES.iter().flat_map(|m| m.number.to_le_bytes()).collect();
    list.extend([0xFF, 0xFF]);
    bus.load_bytes(rom(MODE_LIST), &list);
    bus.load_bytes(rom(WINDOW_FUNCTION), &[0xFE, 0x39, crate::bios::SERVICE_VBE_WINDOW, 0xCB]);
    bus.load_bytes(rom(PM_TABLE), &pm_table());
}

/// Set Window in protected mode: bank DX of window BL (only A exists)
/// goes to the Super VGA's bank register, CRTC 6Ah.
#[rustfmt::skip]
const PM_SET_WINDOW: &[u8] = &[
    0x52,                   // push edx
    0x50,                   // push eax
    0xB0, 0x6A,             // mov al,6Ah
    0x88, 0xD4,             // mov ah,dl
    0x66, 0xBA, 0xD4, 0x03, // mov dx,3D4h
    0x66, 0xEF,             // out dx,ax      ; index, then data at 3D5h
    0x58,                   // pop eax
    0x5A,                   // pop edx
    0xC3,                   // ret
];

/// Set Display Start in protected mode: DX:CX is the start in bytes over 4
/// (CRTC 0Dh, 0Ch and 69h). With BL=80h it returns once the vertical
/// retrace that shows it has begun.
#[rustfmt::skip]
const PM_SET_DISPLAY_START: &[u8] = &[
    0x50,                   // push eax
    0x53,                   // push ebx
    0x52,                   // push edx
    0x88, 0xD7,             // mov bh,dl
    0x66, 0xBA, 0xD4, 0x03, // mov dx,3D4h
    0xB0, 0x0D,             // mov al,0Dh
    0x88, 0xCC,             // mov ah,cl
    0x66, 0xEF,             // out dx,ax
    0xB0, 0x0C,             // mov al,0Ch
    0x88, 0xEC,             // mov ah,ch
    0x66, 0xEF,             // out dx,ax
    0xB0, 0x69,             // mov al,69h
    0x88, 0xFC,             // mov ah,bh
    0x66, 0xEF,             // out dx,ax
    0xF6, 0xC3, 0x80,       // test bl,80h
    0x74, 0x0E,             // jz done
    0x66, 0xBA, 0xDA, 0x03, // mov dx,3DAh
    0xEC,                   // wait1: in al,dx
    0xA8, 0x08,             // test al,8
    0x75, 0xFB,             // jnz wait1     ; out of any retrace
    0xEC,                   // wait2: in al,dx
    0xA8, 0x08,             // test al,8
    0x74, 0xFB,             // jz wait2      ; into the next
    0x5A,                   // done: pop edx
    0x5B,                   // pop ebx
    0x58,                   // pop eax
    0xC3,                   // ret
];

/// Set Primary Palette in protected mode: CX entries from DX on, from
/// ES:EDI as blue, green, red and padding.
#[rustfmt::skip]
const PM_SET_PALETTE: &[u8] = &[
    0x50,                   // push eax
    0x51,                   // push ecx
    0x52,                   // push edx
    0x57,                   // push edi
    0x88, 0xD0,             // mov al,dl
    0x66, 0xBA, 0xC8, 0x03, // mov dx,3C8h
    0xEE,                   // out dx,al
    0x66, 0x42,             // inc dx
    0x66, 0x85, 0xC9,       // test cx,cx
    0x74, 0x15,             // jz done
    0x26, 0x8A, 0x47, 0x02, // next: mov al,es:[edi+2]
    0xEE,                   // out dx,al
    0x26, 0x8A, 0x47, 0x01, // mov al,es:[edi+1]
    0xEE,                   // out dx,al
    0x26, 0x8A, 0x07,       // mov al,es:[edi]
    0xEE,                   // out dx,al
    0x83, 0xC7, 0x04,       // add edi,4
    0x66, 0x49,             // dec cx
    0x75, 0xEB,             // jnz next
    0x5F,                   // done: pop edi
    0x5A,                   // pop edx
    0x59,                   // pop ecx
    0x58,                   // pop eax
    0xC3,                   // ret
];

/// The ports the protected-mode code uses; no memory-mapped registers.
#[rustfmt::skip]
const PM_PORTS: &[u8] = &[0xD4, 0x03, 0xD5, 0x03, 0xDA, 0x03, 0xC8, 0x03, 0xC9, 0x03, 0xFF, 0xFF, 0xFF, 0xFF];

/// The table function 0Ah returns: the offsets of the three routines and
/// of the port list, then the list and the 32-bit code, each routine
/// ending in a near RET.
fn pm_table() -> Vec<u8> {
    let mut table = vec![0u8; 8];
    let mut offsets = Vec::new();
    for part in [PM_SET_WINDOW, PM_SET_DISPLAY_START, PM_SET_PALETTE, PM_PORTS] {
        offsets.push(table.len() as u16);
        table.extend_from_slice(part);
    }
    for (i, offset) in offsets.into_iter().enumerate() {
        table[i * 2..i * 2 + 2].copy_from_slice(&offset.to_le_bytes());
    }
    table
}

fn write_bytes(bus: &mut Bus, addr: usize, bytes: &[u8]) {
    for (i, &b) in bytes.iter().enumerate() {
        bus.write_8(addr + i, b);
    }
}

/// INT 10h AH=4Fh.
pub fn handle(cpu: &mut Cpu) {
    let es_di = cpu.get_physical_addr(cpu.es(), cpu.di());
    let status = match cpu.get_al() {
        // Waiting for the retrace: the call runs again, registers intact.
        0x07 => match display_start(cpu) {
            Some(status) => status,
            None => return,
        },
        0x00 => controller_info(cpu, es_di),
        0x01 => mode_info(cpu, cpu.cx(), es_di),
        0x02 => set_mode(cpu, cpu.bx()),
        0x03 => {
            let mode = match (cpu.bus.video_mode, cpu.bus.vbe.mode) {
                (VideoMode::Vesa, Some(mode)) => mode.number | if cpu.bus.vbe.lfb { 0x4000 } else { 0 },
                _ => cpu.bus.read_8(0x0449) as u16,
            };
            cpu.set_bx(mode);
            SUCCESS
        }
        0x04 => save_restore_state(cpu),
        0x05 => window(cpu),
        0x06 => scan_line_length(cpu),
        0x08 => dac_width(cpu),
        0x09 => palette(cpu, es_di),
        // The protected-mode interface: ES:DI, CX bytes long.
        0x0A if cpu.get_reg8(Register::BL) == 0 => {
            cpu.set_es(ROM_SEGMENT);
            cpu.set_di(PM_TABLE);
            cpu.set_cx(pm_table().len() as u16);
            SUCCESS
        }
        _ => FAILED,
    };
    cpu.set_ax(status);
}

/// Function 00h: the VbeInfoBlock. Callers asking for VBE 2.0 (with "VBE2"
/// in the buffer) get 512 bytes with the mode list and the strings in the
/// buffer, where protected-mode programs can reach them; others get the
/// 256 bytes of VBE 1.x pointing into the ROM.
fn controller_info(cpu: &mut Cpu, addr: usize) -> u16 {
    let vbe2 = (0..4).map(|i| cpu.bus.read_8(addr + i)).eq(*b"VBE2");
    let (seg, off) = (cpu.es(), cpu.di());
    let far = |offset: u16| (ROM_SEGMENT as u32) << 16 | offset as u32;
    let mut block = vec![0u8; if vbe2 { 512 } else { 256 }];
    block[0..4].copy_from_slice(b"VESA");
    block[4..6].copy_from_slice(&0x0200u16.to_le_bytes());
    // Capabilities: the DAC can switch to 8 bits per color.
    block[10..14].copy_from_slice(&1u32.to_le_bytes());
    block[18..20].copy_from_slice(&((VRAM_SIZE / 0x10000) as u16).to_le_bytes());
    if vbe2 {
        // The mode list in the reserved area, the strings in OemData.
        let in_buffer = |at: usize| (seg as u32) << 16 | off.wrapping_add(at as u16) as u32;
        let mut list: Vec<u8> = MODES.iter().flat_map(|m| m.number.to_le_bytes()).collect();
        list.extend([0xFF, 0xFF]);
        block[34..34 + list.len()].copy_from_slice(&list);
        block[14..18].copy_from_slice(&in_buffer(34).to_le_bytes());
        block[20..22].copy_from_slice(&0x0200u16.to_le_bytes());
        let mut at = 256;
        for (field, text) in [(6, OEM), (22, VENDOR), (26, PRODUCT), (30, REVISION)] {
            block[at..at + text.len()].copy_from_slice(text);
            block[field..field + 4].copy_from_slice(&in_buffer(at).to_le_bytes());
            at += text.len();
        }
    } else {
        block[6..10].copy_from_slice(&far(OEM_STRING).to_le_bytes());
        block[14..18].copy_from_slice(&far(MODE_LIST).to_le_bytes());
    }
    write_bytes(&mut cpu.bus, addr, &block);
    SUCCESS
}

/// The color masks of a direct color mode: size and position of red,
/// green, blue and the reserved bits.
fn color_masks(bpp: u8) -> [u8; 8] {
    match bpp {
        15 => [5, 10, 5, 5, 5, 0, 1, 15],
        16 => [5, 11, 6, 5, 5, 0, 0, 0],
        32 => [8, 16, 8, 8, 8, 0, 8, 24],
        _ => [0; 8],
    }
}

/// Function 01h: the ModeInfoBlock of mode CX.
fn mode_info(cpu: &mut Cpu, number: u16, addr: usize) -> u16 {
    let Some(mode) = vbe::find_mode(number) else {
        return FAILED;
    };
    let mut block = [0u8; 256];
    let pitch = mode.width as usize * mode.bytes_per_pixel();
    // Supported, extended information, color, graphics, linear frame buffer.
    block[0..2].copy_from_slice(&0x009Bu16.to_le_bytes());
    block[2] = 0x07; // window A: there, readable, writable
    block[4..6].copy_from_slice(&((WINDOW_SIZE / 1024) as u16).to_le_bytes()); // granularity, KB
    block[6..8].copy_from_slice(&((WINDOW_SIZE / 1024) as u16).to_le_bytes()); // size, KB
    block[8..10].copy_from_slice(&0xA000u16.to_le_bytes());
    let window_function = (ROM_SEGMENT as u32) << 16 | WINDOW_FUNCTION as u32;
    block[12..16].copy_from_slice(&window_function.to_le_bytes());
    block[16..18].copy_from_slice(&(pitch as u16).to_le_bytes());
    block[18..20].copy_from_slice(&mode.width.to_le_bytes());
    block[20..22].copy_from_slice(&mode.height.to_le_bytes());
    block[22] = 8; // character cell
    block[23] = 16;
    block[24] = 1; // planes
    block[25] = mode.bpp;
    block[26] = 1; // banks
    block[27] = if mode.bpp == 8 { 4 } else { 6 }; // packed pixel or direct color
    let pages = VRAM_SIZE / (pitch * mode.height as usize);
    block[29] = (pages - 1).min(255) as u8;
    block[30] = 1;
    block[31..39].copy_from_slice(&color_masks(mode.bpp));
    block[40..44].copy_from_slice(&(LFB_BASE as u32).to_le_bytes());
    write_bytes(&mut cpu.bus, addr, &block);
    SUCCESS
}

/// Function 02h: set mode BX (bit 14 with the linear frame buffer, bit 15
/// keeping video memory). Standard modes go to AH=00h.
fn set_mode(cpu: &mut Cpu, bx: u16) -> u16 {
    let keep = bx & 0x8000 != 0;
    if bx & 0x1FF < 0x100 {
        super::int10::set_mode(cpu, (bx & 0x7F) as u8 | if keep { 0x80 } else { 0 });
        return SUCCESS;
    }
    let Some(mode) = vbe::find_mode(bx) else {
        return FAILED;
    };
    enter_mode(&mut cpu.bus, mode, bx & 0x4000 != 0, keep);
    // BIOS data area: the mode byte S3 BIOSes use (68h-80h), and the text
    // grid of 8x16 characters.
    cpu.bus.write_8(0x0449, (mode.number - 0x98) as u8);
    cpu.bus.write_16(0x044A, mode.width / 8);
    cpu.bus.write_8(0x0484, (mode.height / 16 - 1) as u8);
    cpu.bus.write_16(0x0485, 16);
    cpu.bus.write_8(0x0462, 0);
    SUCCESS
}

/// Switch the card to a VESA mode.
pub fn enter_mode(bus: &mut Bus, mode: &'static VbeMode, lfb: bool, keep: bool) {
    bus.log_string(&format!(
        "[BIOS] Switch to VESA mode {:03X}h ({}x{}, {} bits per pixel)",
        mode.number, mode.width, mode.height, mode.bpp
    ));
    bus.vbe.set_mode(mode, lfb, keep);
    bus.video_mode = VideoMode::Vesa;
    // The VGA side runs as a 256-color mode, at the mode's own timing.
    bus.vga.set_video_mode(VideoMode::Vesa);
    bus.vga.set_fixed_timing(Some(mode.timing));
    bus.vga.mark_dirty_full();
}

/// Function 04h: the size of the state buffer (DL=0), and saving (DL=1)
/// or restoring (DL=2) the VBE state at ES:BX. It fits one 64-byte block.
fn save_restore_state(cpu: &mut Cpu) -> u16 {
    let addr = cpu.get_physical_addr(cpu.es(), cpu.bx());
    match cpu.get_reg8(Register::DL) {
        0x00 => cpu.set_bx(1),
        0x01 => {
            let vbe = &cpu.bus.vbe;
            let mode = vbe.mode.map_or(0, |m| m.number) | if vbe.lfb { 0x4000 } else { 0 };
            let (bank, pitch, start) = (vbe.bank, vbe.pitch, vbe.start);
            cpu.bus.write_16(addr, mode);
            cpu.bus.write_32(addr + 2, bank);
            cpu.bus.write_32(addr + 6, pitch);
            cpu.bus.write_32(addr + 10, start);
        }
        0x02 => {
            let mode = cpu.bus.read_16(addr);
            if let Some(m) = vbe::find_mode(mode).filter(|_| mode != 0) {
                if cpu.bus.vbe.mode != Some(m) {
                    enter_mode(&mut cpu.bus, m, mode & 0x4000 != 0, true);
                }
                cpu.bus.vbe.bank = cpu.bus.read_32(addr + 2) % (VRAM_SIZE / WINDOW_SIZE) as u32;
                cpu.bus.vbe.pitch = cpu.bus.read_32(addr + 6);
                cpu.bus.vbe.start = cpu.bus.read_32(addr + 10);
                cpu.bus.vga.mark_dirty_full();
            }
        }
        _ => return FAILED,
    }
    SUCCESS
}

/// Function 05h, and the window function: BH=0 sets window BL (only
/// window A, 0) to position DX, in 64 KB units; BH=1 returns it in DX.
fn window(cpu: &mut Cpu) -> u16 {
    if cpu.get_reg8(Register::BL) != 0 {
        return FAILED;
    }
    match cpu.get_reg8(Register::BH) {
        0x00 => {
            let bank = cpu.dx() as usize;
            if bank >= VRAM_SIZE / WINDOW_SIZE {
                return FAILED;
            }
            cpu.bus.vbe.bank = bank as u32;
        }
        0x01 => cpu.set_dx(cpu.bus.vbe.bank as u16),
        _ => return FAILED,
    }
    SUCCESS
}

/// The window function modes point at, called with a far call.
pub fn window_call(cpu: &mut Cpu) {
    let status = window(cpu);
    cpu.set_ax(status);
}

/// Function 06h: set the scan line length in pixels (BL=0) or bytes
/// (BL=2), get it (BL=1) or the maximum (BL=3). Returns the bytes per
/// line in BX, pixels in CX and the scan lines that fit in DX.
fn scan_line_length(cpu: &mut Cpu) -> u16 {
    let Some(mode) = cpu.bus.vbe.mode.filter(|_| cpu.bus.video_mode == VideoMode::Vesa) else {
        return INVALID_NOW;
    };
    let bytes_per_pixel = mode.bytes_per_pixel() as u32;
    let pitch = match cpu.get_reg8(Register::BL) {
        0x00 => cpu.cx() as u32 * bytes_per_pixel,
        0x01 => cpu.bus.vbe.pitch,
        0x02 => cpu.cx() as u32,
        0x03 => {
            // In the steps of 8 bytes lengths are set in.
            let max = (VRAM_SIZE as u32 / mode.height as u32).min(0xFFFF) / 8 * 8;
            cpu.set_bx(max as u16);
            cpu.set_cx((max / bytes_per_pixel) as u16);
            cpu.set_dx(mode.height);
            return SUCCESS;
        }
        _ => return FAILED,
    };
    if matches!(cpu.get_reg8(Register::BL), 0x00 | 0x02) {
        // Whole pixels, in steps of 8 bytes, and a screen that fits.
        let pitch = pitch.div_ceil(8) * 8;
        let pitch = pitch.div_ceil(bytes_per_pixel) * bytes_per_pixel;
        if pitch < mode.width as u32 * bytes_per_pixel || pitch * mode.height as u32 > VRAM_SIZE as u32 || pitch > 0xFFFF {
            return INVALID_NOW;
        }
        cpu.bus.vbe.pitch = pitch;
        cpu.bus.vga.mark_dirty_full();
    }
    let pitch = cpu.bus.vbe.pitch;
    cpu.set_bx(pitch as u16);
    cpu.set_cx((pitch / bytes_per_pixel) as u16);
    cpu.set_dx(cpu.bus.vbe.max_lines().min(0xFFFF) as u16);
    SUCCESS
}

/// Function 07h: set the display start to pixel CX of scan line DX
/// (BL=0, or BL=80h at the next vertical retrace), or get it (BL=1).
/// None while it waits for the retrace.
fn display_start(cpu: &mut Cpu) -> Option<u16> {
    let Some(mode) = cpu.bus.vbe.mode.filter(|_| cpu.bus.video_mode == VideoMode::Vesa) else {
        return Some(INVALID_NOW);
    };
    let bytes_per_pixel = mode.bytes_per_pixel() as u32;
    let pitch = cpu.bus.vbe.pitch.max(1);
    match cpu.get_reg8(Register::BL) {
        bl @ (0x00 | 0x80) => {
            let start = cpu.dx() as u32 * pitch + cpu.cx() as u32 * bytes_per_pixel;
            if start as usize + pitch as usize * mode.height as usize > VRAM_SIZE {
                return Some(FAILED);
            }
            cpu.bus.vbe.start = start;
            if bl == 0x80 && !wait_for_retrace(cpu) {
                return None;
            }
            cpu.bios_wait_until = None;
        }
        0x01 => {
            let start = cpu.bus.vbe.start;
            cpu.set_reg8(Register::BH, 0);
            cpu.set_cx((start % pitch / bytes_per_pixel) as u16);
            cpu.set_dx((start / pitch) as u16);
        }
        _ => return Some(FAILED),
    }
    Some(SUCCESS)
}

/// Wait in emulated time for the next vertical retrace to begin: true once
/// it has; until then the call is retried, as the BIOS's waits are.
fn wait_for_retrace(cpu: &mut Cpu) -> bool {
    let now = cpu.bus.clock.now_ticks();
    let until = *cpu.bios_wait_until.get_or_insert_with(|| {
        let now_ns = cpu.bus.clock.now_ns();
        let at = cpu.bus.vga.timing().next_retrace(now_ns);
        (at as u128 * crate::timer::PIT_HZ as u128 / 1_000_000_000) as u64 + 1
    });
    if now >= until {
        cpu.bios_wait_until = None;
        cpu.bus.sync_display();
        return true;
    }
    cpu.hle_wait();
    let clock = &mut cpu.bus.clock;
    clock.deadline = clock.deadline.min(clock.icount_at(until));
    false
}

/// Function 08h: set the DAC to BH bits per color (6 or 8; BL=0) or get
/// the width (BL=1). BH returns the width.
fn dac_width(cpu: &mut Cpu) -> u16 {
    match cpu.get_reg8(Register::BL) {
        0x00 => {
            let wide = cpu.get_reg8(Register::BH) >= 8;
            if cpu.bus.vga.dac_8bit != wide {
                cpu.bus.vga.dac_8bit = wide;
                cpu.bus.vga.mark_dirty_full();
            }
        }
        0x01 => {}
        _ => return FAILED,
    }
    cpu.set_reg8(Register::BH, if cpu.bus.vga.dac_8bit { 8 } else { 6 });
    SUCCESS
}

/// Function 09h: set (BL=0, or 80h at the retrace) or get (BL=1) CX
/// palette entries from DX on, 4 bytes each at ES:DI: blue, green, red and
/// a padding byte.
fn palette(cpu: &mut Cpu, addr: usize) -> u16 {
    let start = cpu.dx() as usize;
    let count = (cpu.cx() as usize).min(256usize.saturating_sub(start));
    let mask = cpu.bus.vga.dac_value_mask();
    match cpu.get_reg8(Register::BL) {
        0x00 | 0x80 => {
            for i in 0..count {
                let entry = addr + i * 4;
                let (b, g, r) = (cpu.bus.read_8(entry), cpu.bus.read_8(entry + 1), cpu.bus.read_8(entry + 2));
                let at = (start + i) * 3;
                cpu.bus.vga.palette[at..at + 3].copy_from_slice(&[r & mask, g & mask, b & mask]);
            }
            cpu.bus.vga.mark_dirty_full();
        }
        0x01 => {
            for i in 0..count {
                let at = (start + i) * 3;
                let (r, g, b) = (cpu.bus.vga.palette[at], cpu.bus.vga.palette[at + 1], cpu.bus.vga.palette[at + 2]);
                write_bytes(&mut cpu.bus, addr + i * 4, &[b, g, r, 0]);
            }
        }
        _ => return FAILED,
    }
    SUCCESS
}
