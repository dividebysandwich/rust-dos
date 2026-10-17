//! The mach64's video BIOS, beyond the VGA's and VESA's: the data ATI's
//! drivers look for in the ROM (the " 761295520" signature, the ROM and
//! clock tables, the capability list), the accelerator CRTC set for VESA
//! modes, and the extended services, INT 10h AH=A0h and the far entry at
//! C000:0064h, with AL the function. ATI's Windows driver calls them to
//! find the card and its modes, and to set them.
//!
//! The services' layouts are from ATI's programmer's guides (the BIOS kit,
//! PRG-215R3) and what drivers 4.35 were seen to need of them.

use super::regs;
use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::video::vbe::VbeMode;
use iced_x86::Register;

/// Where the BIOS's tables are in its ROM segment.
const ROM_TABLE: u16 = 0x00A2;
const CLOCK_TABLE: u16 = 0x00C0;
/// The extended services' far entry point.
pub const ENTRY: u16 = 0x0064;
/// The capability list of AL=07h, its format and row size before it.
const CAPABILITIES: u16 = 0x0402;
/// The size of a mode table in the query structure. ATI's guide for the
/// Rage Pro gives 24h, but drivers 4.35 step through the tables 26h apart.
const MODE_TABLE: usize = 0x26;

/// A resolution the accelerator CRTC has timings for, by ATI's code for
/// it: horizontal values in pixels, vertical ones in scanlines, the clock
/// in units of 10 kHz, the sync polarities negative or not, and double
/// scanned (its vertical values then count scanlines).
struct Resolution {
    code: u8,
    width: u16,
    height: u16,
    h_total: u16,
    h_sync: u16,
    h_sync_width: u16,
    v_total: u16,
    v_sync: u16,
    v_sync_width: u16,
    clock: u16,
    negative: (bool, bool),
    double: bool,
}

const fn res(
    code: u8,
    size: (u16, u16),
    h: (u16, u16, u16),
    v: (u16, u16, u16),
    clock: u16,
    negative: (bool, bool),
    double: bool,
) -> Resolution {
    Resolution {
        code,
        width: size.0,
        height: size.1,
        h_total: h.0,
        h_sync: h.1,
        h_sync_width: h.2,
        v_total: v.0,
        v_sync: v.1,
        v_sync_width: v.2,
        clock,
        negative,
        double,
    }
}

/// The resolutions, in the order the query structure lists them: drivers
/// take the first for 640x480, and the low resolutions (E2h-E5h) come
/// after the standard ones. 60 Hz VESA timings; the low ones double
/// scanned, 320x200 from 640x400's 70 Hz.
static RESOLUTIONS: [Resolution; 8] = [
    res(
        0x12,
        (640, 480),
        (800, 656, 96),
        (525, 490, 2),
        2518,
        (true, true),
        false,
    ),
    res(
        0x6A,
        (800, 600),
        (1056, 840, 128),
        (628, 601, 4),
        4000,
        (false, false),
        false,
    ),
    res(
        0x55,
        (1024, 768),
        (1344, 1048, 136),
        (806, 771, 6),
        6500,
        (true, true),
        false,
    ),
    res(
        0x83,
        (1280, 1024),
        (1688, 1328, 112),
        (1066, 1025, 3),
        10800,
        (false, false),
        false,
    ),
    res(
        0xE3,
        (320, 240),
        (400, 328, 48),
        (525, 490, 2),
        1259,
        (true, true),
        true,
    ),
    res(
        0xE5,
        (400, 300),
        (528, 424, 64),
        (628, 601, 4),
        2000,
        (false, false),
        true,
    ),
    res(
        0xE4,
        (512, 384),
        (672, 528, 64),
        (806, 771, 6),
        3250,
        (true, true),
        true,
    ),
    res(
        0xE2,
        (320, 200),
        (400, 328, 48),
        (449, 413, 2),
        1259,
        (true, false),
        true,
    ),
];

/// The accelerator CRTC's values for a mode, as the mode tables have
/// them: horizontal ones in characters less one, vertical ones in
/// scanlines less one, the sync widths with bit 5 their negative
/// polarity, the clock in 10 kHz, and flags (bit 8 double scan, bit 9
/// interlace).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Crtc {
    h_total: u8,
    h_disp: u8,
    h_sync: u8,
    h_sync_width: u8,
    h_sync_delay: u8,
    v_total: u16,
    v_disp: u16,
    v_sync: u16,
    v_sync_width: u8,
    clock: u16,
    flags: u16,
}

impl Resolution {
    fn crtc(&self) -> Crtc {
        let lines = if self.double {
            2 * self.height
        } else {
            self.height
        };
        Crtc {
            h_total: (self.h_total / 8 - 1) as u8,
            h_disp: (self.width / 8 - 1) as u8,
            h_sync: (self.h_sync / 8 - 1) as u8,
            h_sync_width: (self.h_sync_width / 8) as u8 | if self.negative.0 { 0x20 } else { 0 },
            h_sync_delay: 0,
            v_total: self.v_total - 1,
            v_disp: lines - 1,
            v_sync: self.v_sync - 1,
            v_sync_width: self.v_sync_width as u8 | if self.negative.1 { 0x20 } else { 0 },
            clock: self.clock,
            flags: if self.double { 0x100 } else { 0 },
        }
    }
}

/// CRTC values for a mode of no resolution code: blanking a quarter of
/// the width, 60 Hz (70 at 400 lines and below).
fn generated_crtc(width: u16, height: u16, double: bool) -> Crtc {
    let chars = width / 8;
    let total = (chars + chars / 4).max(chars + 8);
    let lines = if double { 2 * height } else { height };
    let v_total = lines + lines / 20 + 10;
    let hz = if lines <= 400 { 70 } else { 60 };
    let clock = total as u32 * 8 * v_total as u32 * hz / 10_000;
    Crtc {
        h_total: (total - 1) as u8,
        h_disp: (chars - 1) as u8,
        h_sync: (chars + 1) as u8,
        h_sync_width: ((total - chars) / 2) as u8,
        h_sync_delay: 0,
        v_total: v_total - 1,
        v_disp: lines - 1,
        v_sync: lines + (v_total - lines) / 4 - 1,
        v_sync_width: 2,
        clock: clock as u16,
        flags: if double { 0x100 } else { 0 },
    }
}

/// The memory size as ATI codes it: 512 KB, 1, 2, 4, 6 or 8 MB.
fn memory_code(bytes: u32) -> Option<u8> {
    [512u32, 1024, 2048, 4096, 6144, 8192]
        .iter()
        .position(|&kb| bytes <= kb << 10)
        .map(|i| i as u8)
}

/// The ASIC's ID as the queries report it: 4700h a GT-A2, 479Ah a GT-B.
/// ATI's driver only offers 3D (ATI3DCIF) for the IDs it knows.
fn asic_id(bus: &Bus) -> u16 {
    if bus.mach64.rage2 { 0x479A } else { 0x4700 }
}

fn rom(offset: u16) -> usize {
    0xC0000 + offset as usize
}

/// Write the BIOS's data into the ROM, after the VGA BIOS's.
pub fn install_rom(bus: &mut Bus) {
    // The signature ATI's drivers search for, and the BIOS kit's mach64
    // marker.
    bus.write_rom(rom(0x30), b" 761295520");
    bus.write_rom(rom(0x43), &[0x40]);
    // The ROM table, its length before it, and the clock table it points
    // to: the internal PLL, 10 to 135 MHz, clock 3 the one programmed,
    // a 14.32 MHz reference divided by 36h, the memory clocks.
    bus.write_rom(rom(0x48), &ROM_TABLE.to_le_bytes());
    let mut table = vec![0u8; 0x14];
    table[0..2].copy_from_slice(&0x12u16.to_le_bytes());
    table[2 + 0x10..2 + 0x12].copy_from_slice(&CLOCK_TABLE.to_le_bytes());
    bus.write_rom(rom(ROM_TABLE - 2), &table);
    let mut clocks = [0u8; 0x20];
    clocks[0] = 4;
    for (at, value) in [
        (2, 1000u16),
        (4, 13500),
        (8, 1432),
        (0x0A, 0x36),
        (0x0E, 4000),
        (0x10, 6000),
        (0x12, 6000),
        (0x14, 6000),
    ] {
        clocks[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
    clocks[6] = 3;
    bus.write_rom(rom(CLOCK_TABLE), &clocks);
    bus.write_rom(rom(ENTRY), &[0xFE, 0x3A, crate::bios::FAR_ATI, 0xCB]);
    bus.write_rom(rom(CAPABILITIES - 2), &capabilities());
}

/// The hardware capability list (AL=07h): its format (0) and row length
/// (5), then for each mode width from the narrowest up and each depth
/// from the deepest down that fits in the memory, the width in
/// characters, the DAC types (all), the memory it needs (DRAM and VRAM
/// codes), the highest pixel clock in MHz and the depth's code; a row of
/// zeros ends it. ATI's HAL takes the first row at least as wide as a
/// mode and looks at that width's rows only.
fn capabilities() -> Vec<u8> {
    let sizes = [
        (320u32, 240u32),
        (400, 300),
        (512, 384),
        (640, 480),
        (800, 600),
        (1024, 768),
        (1280, 1024),
        (1600, 1200),
    ];
    let depths = [(6u8, 4u32), (5, 3), (4, 2), (3, 2), (2, 1)];
    let mut list = vec![0, 5];
    for (width, height) in sizes {
        for (code, bytes) in depths {
            if let Some(memory) = memory_code(width * height * bytes) {
                list.extend([(width / 8) as u8, 0xFF, memory << 4 | memory, 135, code]);
            }
        }
    }
    list.extend([0; 5]);
    list
}

/// What the BIOS sets up at POST: SCRATCH_REG1 says its ROM is at C000h,
/// and the registers are left at B000:F800h too, where ATI's Windows
/// driver reads FIFO_STAT in mode 12h without mapping them itself.
pub fn post(bus: &mut Bus) {
    bus.mach64_write(regs::SCRATCH_REG1, 0, u32::MAX);
    let config = bus.mach64.reg(regs::CONFIG_CNTL);
    bus.mach64_write(regs::CONFIG_CNTL, config | 4, u32::MAX);
}

/// Program the accelerator CRTC for `crtc` with depth `depth` (the
/// CRTC_PIX_WIDTH code) and the pitch `pitch` asks for (0: 1024 pixels,
/// 1: as it is, 2: the width), and its clock. The display stays as it is
/// (EXT_DISP_EN); CRTC_ENABLE is set.
fn load_crtc(bus: &mut Bus, crtc: &Crtc, depth: u8, pitch: u8, dac8: bool) -> bool {
    if !(1..=6).contains(&depth)
        || crtc.h_disp == 0
        || crtc.v_disp == 0
        || crtc.h_total < crtc.h_disp
        || crtc.v_total < crtc.v_disp
    {
        return false;
    }
    let write = |bus: &mut Bus, reg: usize, value: u32| bus.mach64_write(reg, value, u32::MAX);
    let gen_cntl = bus.mach64.reg(regs::CRTC_GEN_CNTL);
    write(bus, regs::CRTC_GEN_CNTL, gen_cntl | 0x40);
    write(
        bus,
        regs::CRTC_H_TOTAL_DISP,
        (crtc.h_disp as u32) << 16 | crtc.h_total as u32,
    );
    let polarity = |width: u8| ((width & 0x1F) as u32) << 16 | ((width & 0x20) as u32) << 16;
    write(
        bus,
        regs::CRTC_H_SYNC_STRT_WID,
        crtc.h_sync as u32 | (crtc.h_sync_delay as u32 & 7) << 8 | polarity(crtc.h_sync_width),
    );
    write(
        bus,
        regs::CRTC_V_TOTAL_DISP,
        (crtc.v_disp as u32) << 16 | crtc.v_total as u32,
    );
    write(
        bus,
        regs::CRTC_V_SYNC_STRT_WID,
        crtc.v_sync as u32 | polarity(crtc.v_sync_width),
    );
    write(bus, regs::OVR_WID_LEFT_RIGHT, 0);
    write(bus, regs::OVR_WID_TOP_BOTTOM, 0);
    if pitch != 1 {
        let pixels = if pitch == 0 {
            1024
        } else {
            (crtc.h_disp as u32 + 1) * 8
        };
        write(bus, regs::CRTC_OFF_PITCH, (pixels / 8) << 22);
    }
    if crtc.clock != 0 {
        bus.mach64.program_clock(crtc.clock as u64 * 10_000);
        write(bus, regs::CLOCK_CNTL, 3);
    }
    if dac8 {
        let dac = bus.mach64.reg(regs::DAC_CNTL);
        write(bus, regs::DAC_CNTL, dac | 0x100);
    }
    let mut gen_cntl = gen_cntl & !(0x700 | 0x43) | (depth as u32) << 8 | 1 << 25;
    gen_cntl |= (crtc.flags >> 8 & 3) as u32;
    write(bus, regs::CRTC_GEN_CNTL, gen_cntl);
    true
}

/// Show the accelerator CRTC's picture (or the VGA's), with the 8-bit DAC
/// or not.
fn select_display(bus: &mut Bus, accelerator: bool, dac8: bool) {
    let gen_cntl = bus.mach64.reg(regs::CRTC_GEN_CNTL);
    let gen_cntl = if accelerator {
        gen_cntl | 0x0300_0000
    } else {
        gen_cntl & !0x0100_0000
    };
    bus.mach64_write(regs::CRTC_GEN_CNTL, gen_cntl, u32::MAX);
    let dac = bus.mach64.reg(regs::DAC_CNTL) & !0x100;
    bus.mach64_write(
        regs::DAC_CNTL,
        dac | if accelerator && dac8 { 0x100 } else { 0 },
        u32::MAX,
    );
}

/// The CRTC_PIX_WIDTH code of a depth.
fn depth_code(bpp: u8) -> u8 {
    match bpp {
        4 => 1,
        8 => 2,
        15 => 3,
        16 => 4,
        24 => 5,
        _ => 6,
    }
}

/// The accelerator CRTC set for VESA mode `mode`, as ATI's BIOS sets it
/// for its modes, and the draw engine set to draw on its screen.
pub fn program_mode(bus: &mut Bus, mode: &VbeMode) {
    let resolution = RESOLUTIONS
        .iter()
        .find(|r| (r.width, r.height) == (mode.width, mode.height));
    let crtc = match resolution {
        Some(r) => r.crtc(),
        None => generated_crtc(mode.width, mode.height, mode.height < 350),
    };
    load_crtc(bus, &crtc, depth_code(mode.bpp), 2, false);
    select_display(bus, true, false);
    bus.mach64_set_bank(0);
    // The engine: 24 bits a pixel are drawn as three bytes of 8.
    let (code, pixels) = match mode.bpp {
        24 => (2, mode.width as u32 * 3),
        bpp => (depth_code(bpp) as u32, mode.width as u32),
    };
    let off_pitch = (pixels / 8) << 22;
    for (reg, value) in [
        (regs::DST_OFF_PITCH, off_pitch),
        (regs::SRC_OFF_PITCH, off_pitch),
        (
            regs::DP_PIX_WIDTH,
            code | code << 8 | code << 16 | code << 28,
        ),
        (regs::SC_LEFT_RIGHT, (pixels - 1) << 16),
        (regs::SC_TOP_BOTTOM, 0x3FFF << 16),
        (regs::DP_WRITE_MSK, u32::MAX),
        (regs::DST_CNTL, 3),
    ] {
        bus.mach64_write(reg, value, u32::MAX);
    }
}

/// The linear address a far pointer DX:BX of the caller points to: a
/// selector's in protected mode.
fn caller_pointer(cpu: &mut Cpu) -> u32 {
    let (segment, offset) = (cpu.dx(), cpu.bx());
    if !cpu.pm() {
        return cpu.real_linear(segment, offset);
    }
    cpu.fetch_descriptor(segment, 0)
        .map_or(0, |d| d.base().wrapping_add(offset as u32))
}

/// The query structure (AL=08h, AL=09h): a 20h-byte header on the card,
/// and with `modes` the mode tables after it.
fn query(bus: &Bus, modes: bool) -> Vec<u8> {
    let count = if modes { RESOLUTIONS.len() } else { 0 };
    let mut q = vec![0u8; 0x20 + count * MODE_TABLE];
    let len = q.len() as u16;
    let memory = memory_code(super::MEMORY).unwrap_or(3);
    let block_io = bus.mach64.block_io();
    q[0x00..0x02].copy_from_slice(&len.to_le_bytes());
    q[0x02] = 1;
    q[0x03] = count as u8;
    q[0x04..0x06].copy_from_slice(&0x20u16.to_le_bytes());
    q[0x06] = MODE_TABLE as u8;
    // The VGA is on, with all of its memory.
    q[0x07] = 1;
    q[0x08..0x0A].copy_from_slice(&asic_id(bus).to_le_bytes());
    q[0x0B] = memory;
    // The internal DAC, EDO memory, PCI.
    q[0x0D] = 2;
    q[0x0E] = 7;
    q[0x10..0x12].copy_from_slice(&aperture_mb(bus).to_le_bytes());
    // An 8 MB aperture where the PCI BIOS put it (ATI's driver only
    // takes the frame buffer's address with these bits set), the depths
    // (32 and 24 bits, 15 and 16), gamma and grey scale.
    q[0x12] = 0x42;
    q[0x13] = 0xA7;
    q[0x14] = 0x60;
    q[0x15] = block_io as u8;
    q[0x18..0x1A].copy_from_slice(&bus.mach64.io_base().to_le_bytes());
    for (i, r) in RESOLUTIONS.iter().enumerate().take(count) {
        let t = &mut q[0x20 + i * MODE_TABLE..0x20 + (i + 1) * MODE_TABLE];
        let fits = |bytes: u32| r.width as u32 * r.height as u32 * bytes <= super::MEMORY;
        let depth = [(6, 4), (5, 3), (4, 2)]
            .into_iter()
            .find(|&(_, b)| fits(b))
            .map_or(2, |(d, _)| d);
        let crtc = r.crtc();
        t[0x00..0x02].copy_from_slice(&r.width.to_le_bytes());
        t[0x02..0x04].copy_from_slice(&r.height.to_le_bytes());
        t[0x04] = depth;
        t[0x05] = r.code;
        t[0x0C..0x0E].copy_from_slice(&crtc.flags.to_le_bytes());
        t[0x0E] = crtc.h_total;
        t[0x0F] = crtc.h_disp;
        t[0x10] = crtc.h_sync;
        t[0x11] = crtc.h_sync_width;
        t[0x12..0x14].copy_from_slice(&crtc.v_total.to_le_bytes());
        t[0x14..0x16].copy_from_slice(&crtc.v_disp.to_le_bytes());
        t[0x16..0x18].copy_from_slice(&crtc.v_sync.to_le_bytes());
        t[0x18] = crtc.v_sync_width;
        // CLOCK_CNTL: the programmed clock.
        t[0x19] = 3;
        t[0x1A..0x1C].copy_from_slice(&crtc.clock.to_le_bytes());
    }
    q
}

/// Where the aperture is, in MB.
fn aperture_mb(bus: &Bus) -> u16 {
    (bus.mach64.aperture().unwrap_or(0) >> 20) as u16
}

/// A mode table the caller passes (AL=00h with CH=81h), from its offset 8
/// on, at linear address `at`.
fn caller_crtc(bus: &mut Bus, at: u32) -> Crtc {
    let mut t = [0u8; MODE_TABLE - 8];
    bus.guest_read_bytes(at, &mut t);
    let word = |i: usize| u16::from_le_bytes([t[i - 8], t[i - 7]]);
    Crtc {
        flags: word(0x0C),
        h_total: t[0x0E - 8],
        h_disp: t[0x0F - 8],
        h_sync: t[0x10 - 8],
        h_sync_width: t[0x11 - 8],
        v_total: word(0x12),
        v_disp: word(0x14),
        v_sync: word(0x16),
        v_sync_width: t[0x18 - 8],
        clock: word(0x1A),
        h_sync_delay: t[0x1D - 8] & 7,
    }
}

/// INT 10h AH=A0h.
pub fn extended(cpu: &mut Cpu) {
    services(cpu, 0xC000);
}

/// The far entry at C000:0064h, which ATI's Windows driver calls from
/// 16-bit protected mode with its selector for the ROM: pointers into the
/// ROM come back with that.
pub fn rom_entry(cpu: &mut Cpu) {
    let cs = cpu.cs();
    services(cpu, cs);
}

/// The extended services, function AL; AH returns 0 for done, 1 for
/// failed, 2 for a function there isn't.
fn services(cpu: &mut Cpu, rom_segment: u16) {
    let (al, ch, cl) = (
        cpu.get_al(),
        cpu.get_reg8(Register::CH),
        cpu.get_reg8(Register::CL),
    );
    cpu.bus.mach64.trace(|| {
        format!(
            "BIOS A0{:02X} BX={:04X} CX={:04X} DX={:04X}",
            al,
            cpu.bx(),
            cpu.cx(),
            cpu.dx()
        )
    });
    let ok = match al {
        // Load the accelerator CRTC: resolution code CH, or CH=81h a mode
        // table at DX:BX; CL the depth (bits 3-0), the 8-bit DAC (bit 4)
        // and the pitch (bits 7-6).
        0x00 => load(cpu, ch, cl),
        // Load it and show it.
        0x02 if matches!(ch, 0x80 | 0x81) => {
            let ok = load(cpu, ch, cl);
            if ok {
                select_display(&mut cpu.bus, true, cl & 0x10 != 0);
                cpu.set_reg8(Register::CL, cl & !0x20);
            }
            ok
        }
        // A built-in mode: the VESA mode of its size and depth, set as
        // the VESA BIOS sets it.
        0x02 => {
            let size = RESOLUTIONS
                .iter()
                .find(|r| r.code == ch)
                .map(|r| (r.width, r.height));
            let bpp = match cl & 0xF {
                2 => Some(8),
                3 => Some(15),
                4 => Some(16),
                6 => Some(32),
                _ => None,
            };
            let mode = crate::video::vbe::MODES
                .iter()
                .find(|m| Some((m.width, m.height)) == size && Some(m.bpp) == bpp);
            match mode {
                Some(mode) => {
                    let mode = *mode;
                    crate::interrupts::vbe::enter_mode(&mut cpu.bus, &mode, true, false);
                    true
                }
                None => false,
            }
        }
        // Select the display: the accelerator's (CL bit 0) or the VGA's,
        // the 8-bit DAC with bit 7.
        0x01 => {
            select_display(&mut cpu.bus, cl & 1 != 0, cl & 0x80 != 0);
            cpu.set_reg8(Register::CL, cl & !0x20);
            true
        }
        // The EEPROM: nothing in it, nothing kept.
        0x03 => {
            cpu.set_bx(0);
            true
        }
        // The aperture: always on, where the PCI BIOS put it.
        0x04 | 0x05 => true,
        // Short query: an 8 MB aperture at BX MB, the depths, the memory,
        // the ASIC.
        0x06 => {
            cpu.set_reg8(Register::AL, 0x42);
            cpu.set_bx(aperture_mb(&cpu.bus));
            cpu.set_reg8(Register::CH, 0xA7);
            cpu.set_reg8(Register::CL, memory_code(super::MEMORY).unwrap_or(3));
            cpu.set_dx(asic_id(&cpu.bus));
            true
        }
        0x07 => {
            cpu.set_dx(rom_segment);
            cpu.set_bx(CAPABILITIES);
            cpu.set_reg8(Register::AL, 0);
            true
        }
        // The query structure's size, then the structure at DX:BX; CL bit
        // 0 with the mode tables.
        0x08 => {
            cpu.set_cx(query(&cpu.bus, cl & 1 != 0).len() as u16);
            true
        }
        0x09 => {
            let at = caller_pointer(cpu);
            let q = query(&cpu.bus, cl & 1 != 0);
            if at != 0 {
                cpu.bus.guest_write_bytes(at, &q);
            }
            at != 0
        }
        // The clock table: an internal PLL, no fixed frequencies.
        0x0A => {
            cpu.set_reg8(Register::AL, 4);
            cpu.set_dx(rom_segment);
            cpu.set_bx(0);
            cpu.set_cx(CLOCK_TABLE);
            true
        }
        // Program clock 3 for BX in 10 kHz.
        0x0B => {
            cpu.bus.mach64.program_clock(cpu.bx() as u64 * 10_000);
            cpu.bus.mach64_settle();
            cpu.set_reg8(Register::AL, 4);
            true
        }
        // DPMS: standby and suspend turn the syncs off.
        0x0C => {
            let state = cl & 3;
            let gen_cntl = cpu.bus.mach64.reg(regs::CRTC_GEN_CNTL) & !0x0C;
            let syncs = if state & 1 != 0 { 4 } else { 0 } | if state >= 2 { 8 } else { 0 };
            cpu.bus
                .mach64_write(regs::CRTC_GEN_CNTL, gen_cntl | syncs, u32::MAX);
            true
        }
        0x0D..=0x10 => {
            cpu.set_reg8(Register::CL, 0);
            true
        }
        // No external storage.
        0x11 => {
            cpu.set_reg8(Register::CL, 0x80);
            cpu.set_reg8(Register::CH, 0);
            cpu.set_bx(0);
            cpu.set_dx(0);
            true
        }
        // The ports: block I/O at DX (CX=1), or the sparse ones.
        0x12 => {
            let block_io = cpu.bus.mach64.block_io();
            cpu.set_cx(block_io as u16);
            cpu.set_dx(cpu.bus.mach64.io_base());
            cpu.set_bx(0);
            cpu.set_reg8(Register::AL, 0);
            true
        }
        // No DDC.
        0x13 => {
            cpu.set_bx(0);
            cpu.set_reg8(Register::AL, 0);
            true
        }
        _ => {
            cpu.set_reg8(Register::AH, 2);
            cpu.bus
                .log_string(&format!("[MACH64] BIOS function A0{:02X}h not there", al));
            return;
        }
    };
    cpu.set_reg8(Register::AH, if ok { 0 } else { 1 });
    cpu.bus.mach64.flush_trace();
}

/// AL=00h: the CRTC loaded from resolution code `ch` or the caller's
/// table.
fn load(cpu: &mut Cpu, ch: u8, cl: u8) -> bool {
    let crtc = match ch {
        0x81 => {
            let at = caller_pointer(cpu);
            if at < 8 {
                return false;
            }
            caller_crtc(&mut cpu.bus, at)
        }
        _ => match RESOLUTIONS.iter().find(|r| r.code == ch) {
            Some(r) => r.crtc(),
            None => return false,
        },
    };
    load_crtc(&mut cpu.bus, &crtc, cl & 0xF, cl >> 6, cl & 0x10 != 0)
}

/// The VBE's protected-mode interface on the card: setting the window's
/// pages and the display start through the block I/O ports at `io` (its
/// MEM_VGA_WP_SEL and RP_SEL, and CRTC_OFF_PITCH), the palette through
/// the VGA's DAC.
pub fn pm_table(io: u16) -> Vec<u8> {
    let port = |reg: usize| (io + 4 * reg as u16).to_le_bytes();
    let (wp, rp, off_pitch) = (
        port(regs::MEM_VGA_WP_SEL),
        port(regs::MEM_VGA_RP_SEL),
        port(regs::CRTC_OFF_PITCH),
    );
    // Bank DX of 64 KB: pages 2*DX and 2*DX+1.
    #[rustfmt::skip]
    let set_window = [
        0x50,                         // push eax
        0x52,                         // push edx
        0x0F, 0xB7, 0xC2,             // movzx eax,dx
        0xD1, 0xE0,                   // shl eax,1
        0x89, 0xC2,                   // mov edx,eax
        0x42,                         // inc edx
        0xC1, 0xE2, 0x10,             // shl edx,16
        0x09, 0xD0,                   // or eax,edx
        0x66, 0xBA, wp[0], wp[1],     // mov dx,MEM_VGA_WP_SEL
        0xEF,                         // out dx,eax
        0x66, 0xBA, rp[0], rp[1],     // mov dx,MEM_VGA_RP_SEL
        0xEF,                         // out dx,eax
        0x5A,                         // pop edx
        0x58,                         // pop eax
        0xC3,                         // ret
    ];
    // DX:CX is the start in bytes over 4; the CRTC's offset is in units
    // of 8 bytes. With BL=80h it returns once the retrace began.
    #[rustfmt::skip]
    let set_start = [
        0x50,                         // push eax
        0x51,                         // push ecx
        0x52,                         // push edx
        0x0F, 0xB7, 0xC2,             // movzx eax,dx
        0xC1, 0xE0, 0x10,             // shl eax,16
        0x66, 0x89, 0xC8,             // mov ax,cx
        0xD1, 0xE8,                   // shr eax,1
        0x25, 0xFF, 0xFF, 0x0F, 0x00, // and eax,0FFFFFh
        0x89, 0xC1,                   // mov ecx,eax
        0x66, 0xBA, off_pitch[0], off_pitch[1], // mov dx,CRTC_OFF_PITCH
        0xED,                         // in eax,dx
        0x25, 0x00, 0x00, 0xF0, 0xFF, // and eax,0FFF00000h
        0x09, 0xC8,                   // or eax,ecx
        0xEF,                         // out dx,eax
        0xF6, 0xC3, 0x80,             // test bl,80h
        0x74, 0x0E,                   // jz done
        0x66, 0xBA, 0xDA, 0x03,       // mov dx,3DAh
        0xEC,                         // wait1: in al,dx
        0xA8, 0x08,                   // test al,8
        0x75, 0xFB,                   // jnz wait1
        0xEC,                         // wait2: in al,dx
        0xA8, 0x08,                   // test al,8
        0x74, 0xFB,                   // jz wait2
        0x5A,                         // done: pop edx
        0x59,                         // pop ecx
        0x58,                         // pop eax
        0xC3,                         // ret
    ];
    let ports: Vec<u8> = [
        wp,
        rp,
        off_pitch,
        0x3DAu16.to_le_bytes(),
        0x3C8u16.to_le_bytes(),
        0x3C9u16.to_le_bytes(),
    ]
    .into_iter()
    .flatten()
    .chain([0xFF, 0xFF, 0xFF, 0xFF])
    .collect();
    let mut table = vec![0u8; 8];
    let mut offsets = Vec::new();
    for part in [
        &set_window[..],
        &set_start[..],
        crate::interrupts::vbe::PM_SET_PALETTE,
        &ports[..],
    ] {
        offsets.push(table.len() as u16);
        table.extend_from_slice(part);
    }
    for (i, offset) in offsets.into_iter().enumerate() {
        table[i * 2..i * 2 + 2].copy_from_slice(&offset.to_le_bytes());
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_resolutions_crtc_values() {
        let crtc = RESOLUTIONS[0].crtc();
        assert_eq!(
            (crtc.h_total, crtc.h_disp, crtc.h_sync, crtc.h_sync_width),
            (99, 79, 81, 12 | 0x20)
        );
        assert_eq!((crtc.v_total, crtc.v_disp, crtc.v_sync), (524, 479, 489));
        let double = RESOLUTIONS[4].crtc();
        assert_eq!((double.v_disp, double.flags), (479, 0x100));
    }

    #[test]
    fn the_capability_list_ends_with_zeros() {
        let list = capabilities();
        assert_eq!(&list[..2], &[0, 5]);
        assert_eq!((list.len() - 2) % 5, 0);
        assert!(list.ends_with(&[0; 5]));
        // 320x240 in 32 bits first.
        assert_eq!(&list[2..7], &[40, 0xFF, 0x00, 135, 6]);
    }
}
