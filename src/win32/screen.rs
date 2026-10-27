//! The display adapter as the built-in Windows drives it: the modes it
//! sets, its video memory, where the picture in it starts, and the
//! palette of 8-bit modes. The desktop (USER32), GDI's drawing on the
//! screen and DirectDraw all go through here.
//!
//! There are two kinds of adapter:
//!
//! - Most cards: the VESA BIOS's 4 MB of video memory and its modes, set
//!   as the BIOS sets them, and the VGA's DAC.
//! - The 3dfx Banshee and Voodoo3 (`voodoo::banshee`): the card's own
//!   16 MB, which its 3D engine draws into, in the card's extended modes.
//!   They are set through its registers as 3dfx's Windows driver set
//!   them: the VGA's timing registers with the card's extra bits, its
//!   pixel clock PLL, and the video processor's format, size, stride and
//!   start; 8-bit modes go through its colour lookup table. Programs reach
//!   the memory through BAR1, linear below the tile base, which a mode
//!   set puts at the end of the memory.
//!
//! The desktop is 640x480 at 32 bits a pixel on the first kind, and at
//! 16 (5-6-5) on the second, as Windows 95 with a Voodoo3 was usually
//! set up; it is at the start of video memory. With `win32_depth=8` it
//! is 640x480 at 8 bits a pixel on both, its colours GDI's system
//! palette, which the DAC (or the card's lookup table) holds.

use crate::cpu::Cpu;
use crate::video::crt::CrtTiming;
use crate::video::vbe::{self, LFB_BASE, VRAM_SIZE, VbeMode};
use crate::voodoo::banshee as card;

/// A display mode: its size, bits a pixel and bytes a row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub bpp: u32,
    pub pitch: u32,
}

/// The desktop's size.
const DESKTOP: (u32, u32) = (640, 480);

/// The VBE mode of the desktop on the VESA kind: 640x480 at 32 bits a
/// pixel (24 on an S3 ViRGE).
const DESKTOP_MODE: u16 = 0x112;

/// The sizes of the modes DirectDraw offers, which have numbers of the
/// VESA kind's own where the BIOS's list hasn't them.
pub const SIZES: [(u32, u32); 6] = [
    (320, 200),
    (320, 240),
    (640, 400),
    (640, 480),
    (800, 600),
    (1024, 768),
];

/// Whether the display adapter is a Banshee or Voodoo3, whose own memory
/// and modes the screen is in.
pub fn is_card(cpu: &Cpu) -> bool {
    cpu.bus.banshee().is_some()
}

/// The bytes of video memory.
pub fn memory_size(cpu: &Cpu) -> u32 {
    if is_card(cpu) {
        card::MEMORY as u32
    } else {
        VRAM_SIZE as u32
    }
}

/// The physical address of video memory's first byte: the VESA linear
/// frame buffer, or the card's BAR1.
pub fn physical_base(cpu: &Cpu) -> u32 {
    match cpu.bus.banshee() {
        Some(b) => b.pci.bar1,
        None => cpu.bus.vbe.lfb_base.unwrap_or(LFB_BASE as u32),
    }
}

/// Make the card's BAR1 linear over all of its memory, as a mode set
/// does, if it isn't over any yet (the tile base still at 0, as at
/// power-on): before a program reaches the memory through it.
pub fn linear_memory(cpu: &mut Cpu) {
    let base = cpu
        .bus
        .banshee()
        .map(|b| b.reg(card::LFB_MEMORY_CONFIG) & 0x1FFF);
    if base == Some(0) {
        cpu.bus
            .voodoo_write_32(card::LFB_MEMORY_CONFIG, (card::MEMORY >> 12) as u32);
    }
}

/// The desktop's mode: 8 bits a pixel with `win32_depth=8`, else 16 on
/// the card, 24 (packed) on an S3 ViRGE, whose VESA mode `DESKTOP_MODE`
/// is that, and 32 on the others.
pub fn desktop(cpu: &Cpu) -> Mode {
    let bpp = if super::version::palette_desktop(cpu) {
        8
    } else if is_card(cpu) {
        16
    } else if cpu.bus.vga.adapter.is_virge() {
        24
    } else {
        32
    };
    let (width, height) = DESKTOP;
    Mode {
        width,
        height,
        bpp,
        pitch: width * bpp / 8,
    }
}

/// The mode the adapter shows now, if it shows one of its own (a VESA
/// mode, or the card's extended one): from its registers.
pub fn mode(cpu: &Cpu) -> Option<Mode> {
    if let Some(b) = cpu.bus.banshee() {
        if !b.extended() {
            return None;
        }
        let cfg = b.reg(card::VID_PROC_CFG);
        let (width, mut height) = b.desktop_size();
        if cfg & card::VIDPROC_HALF_MODE != 0 {
            height /= 2;
        }
        let bpp = match (cfg >> 18) & 7 {
            0 => 8,
            1 => 16,
            2 => 24,
            _ => 32,
        };
        let pitch = b.reg(card::VID_DESKTOP_OVERLAY_STRIDE) & 0x3FFF;
        return Some(Mode {
            width,
            height,
            bpp,
            pitch,
        });
    }
    if cpu.bus.video_mode != crate::video::VideoMode::Vesa {
        return None;
    }
    let m = cpu.bus.vbe.mode?;
    Some(Mode {
        width: m.width as u32,
        height: m.height as u32,
        bpp: m.bpp as u32,
        pitch: cpu.bus.vbe.pitch,
    })
}

/// Set the mode `width` x `height` at `bpp` bits a pixel, its picture at
/// the start of video memory. The VESA kind sets the sizes of its list
/// and of `SIZES`, at 8, 16 and 32 bits (24 on an S3 ViRGE); the card
/// any size with whole characters (8 pixels) up to 1600x1200 at 8, 16 and
/// 32 bits that fits its memory. The picture is black: the VESA kind
/// clears all of video memory, as the BIOS does, the card the picture's
/// bytes. None if the adapter has no such mode.
pub fn set_mode(cpu: &mut Cpu, width: u32, height: u32, bpp: u32) -> Option<Mode> {
    if is_card(cpu) {
        card_set_mode(cpu, width, height, bpp)?;
        let len = width * bpp / 8 * height;
        write(cpu, 0, &vec![0; len as usize]);
    } else {
        let mode = vbe_mode(cpu, width, height, bpp)?;
        crate::interrupts::vbe::enter_mode(&mut cpu.bus, &mode, true, false);
    }
    mode(cpu)
}

/// Set the desktop's mode. At 8 bits a pixel the DAC gets the system
/// palette.
pub fn set_desktop_mode(cpu: &mut Cpu) {
    if super::version::palette_desktop(cpu) {
        let d = desktop(cpu);
        set_mode(cpu, d.width, d.height, d.bpp);
        super::gdi32::palette::show_system_palette(cpu);
        return;
    }
    if is_card(cpu) {
        let d = desktop(cpu);
        set_mode(cpu, d.width, d.height, d.bpp);
        return;
    }
    let packed24 = cpu.bus.vga.adapter.is_virge();
    if let Some(mode) = vbe::find_mode(DESKTOP_MODE, packed24) {
        crate::interrupts::vbe::enter_mode(&mut cpu.bus, &mode, true, false);
    }
}

/// Put the adapter back in text mode (mode 3), as the BIOS sets it: the
/// card's own modes end.
pub fn set_text_mode(cpu: &mut Cpu) {
    // The BIOS's data it writes is reached by its physical address.
    super::dos(cpu, |cpu| crate::interrupts::int10::set_mode(cpu, 3));
}

/// The VBE mode of a size and depth, the list's or one of DirectDraw's
/// own (`SIZES`), by a number no mode of the list has.
fn vbe_mode(cpu: &Cpu, w: u32, h: u32, bpp: u32) -> Option<VbeMode> {
    let packed = cpu.bus.vga.adapter.is_virge();
    let truecolor = if packed { 24 } else { 32 };
    if ![8, 16, truecolor].contains(&bpp) {
        return None;
    }
    let listed = vbe::MODES.iter().find(|m| {
        let depth = match m.bpp {
            32 if packed => 24,
            d => d as u32,
        };
        m.width as u32 == w && m.height as u32 == h && depth == bpp
    });
    if let Some(m) = listed {
        return vbe::find_mode(m.number, packed);
    }
    let index = SIZES.iter().position(|&s| s == (w, h))?;
    // Small modes are shown doubled, at the timing of the doubled lines.
    let number = 0x1F0
        + index as u16 * 4
        + match bpp {
            8 => 0,
            16 => 1,
            _ => 2,
        };
    let shown = if w < 640 { h * 2 } else { h };
    Some(VbeMode {
        number,
        width: w as u16,
        height: h as u16,
        bpp: bpp as u8,
        timing: vbe::timing_for(shown as u16),
    })
}

// Video memory.

/// Read video memory from byte `offset` into `data`; what is past its end
/// reads as 0.
pub fn read(cpu: &Cpu, offset: u32, data: &mut [u8]) {
    let size = memory_size(cpu) as usize;
    let start = (offset as usize).min(size);
    let end = (start + data.len()).min(size);
    data[end - start..].fill(0);
    let (data, len) = (&mut data[..end - start], end - start);
    if len == 0 {
        return;
    }
    match &cpu.bus.voodoo {
        Some(v) if v.banshee().is_some() => v.banshee_memory_read(start as u32, data),
        _ => data.copy_from_slice(&cpu.bus.vbe.vram[start..end]),
    }
}

/// Write `data` into video memory at byte `offset`: what is past its end
/// goes nowhere. The picture shows it.
pub fn write(cpu: &mut Cpu, offset: u32, data: &[u8]) {
    let size = memory_size(cpu) as usize;
    let start = (offset as usize).min(size);
    let end = (start + data.len()).min(size);
    if start == end {
        return;
    }
    let data = &data[..end - start];
    if let Some(v) = &mut cpu.bus.voodoo
        && v.banshee().is_some()
    {
        v.banshee_memory_write(start as u32, data);
        return;
    }
    cpu.bus.vbe.vram[start..end].copy_from_slice(data);
    if cpu.bus.vbe.mode.is_some()
        && let Some((first, last)) = cpu.bus.vbe.frame_rows(start, end - start)
    {
        cpu.bus.vga.mark_dirty_rows(first, last);
    }
}

// What the picture is.

/// Show the picture at byte `offset` of video memory, from the next
/// frame: VBE's display start, or the card's vidDesktopStartAddr.
pub fn show(cpu: &mut Cpu, offset: u32) {
    if is_card(cpu) {
        cpu.bus
            .voodoo_write_32(card::VID_DESKTOP_START_ADDR, offset);
        return;
    }
    cpu.bus.vbe.start = offset;
    cpu.bus.note_display_start();
    cpu.bus.vga.mark_dirty_full();
}

/// The lines the display shows each of the mode's lines on: 2 for the
/// small modes, which are shown doubled.
pub fn line_scale(cpu: &Cpu) -> u32 {
    match cpu.bus.banshee() {
        Some(b) if b.reg(card::VID_PROC_CFG) & card::VIDPROC_HALF_MODE != 0 => 2,
        Some(_) => 1,
        None => cpu.bus.vbe.scale().max(1),
    }
}

/// The display's timing.
pub fn timing(cpu: &mut Cpu) -> CrtTiming {
    cpu.bus.vga.timing()
}

/// The colors of the 8-bit modes' 256, as COLORREFs: the DAC's, or the
/// card's colour lookup table's first.
pub fn colors(cpu: &Cpu) -> Vec<u32> {
    if let Some(b) = cpu.bus.banshee() {
        return b.clut[..256]
            .iter()
            .map(|&c| (c >> 16) & 0xFF | c & 0xFF00 | (c & 0xFF) << 16)
            .collect();
    }
    let shift = if cpu.bus.vga.dac_8bit { 0 } else { 2 };
    (0..256)
        .map(|i| {
            let p = &cpu.bus.vga.palette[i * 3..i * 3 + 3];
            let [r, g, b] = [p[0] << shift, p[1] << shift, p[2] << shift];
            r as u32 | (g as u32) << 8 | (b as u32) << 16
        })
        .collect()
}

/// Set the colors from `first` on to `colors` (COLORREFs): in the DAC,
/// or through the card's dacAddr and dacData into its lookup table.
pub fn set_colors(cpu: &mut Cpu, first: usize, colors: &[u32]) {
    let colors = &colors[..colors.len().min(256usize.saturating_sub(first))];
    if is_card(cpu) {
        for (i, &c) in colors.iter().enumerate() {
            let rgb = (c & 0xFF) << 16 | c & 0xFF00 | (c >> 16) & 0xFF;
            cpu.bus.voodoo_write_32(card::DAC_ADDR, (first + i) as u32);
            cpu.bus.voodoo_write_32(card::DAC_DATA, rgb);
        }
        return;
    }
    let shift = if cpu.bus.vga.dac_8bit { 0 } else { 2 };
    let vga = &mut cpu.bus.vga;
    for (i, c) in colors.iter().enumerate() {
        let rgb = [*c as u8, (*c >> 8) as u8, (*c >> 16) as u8];
        for (k, v) in rgb.iter().enumerate() {
            vga.palette[(first + i) * 3 + k] = v >> shift;
        }
    }
    vga.mark_dirty_full();
}

// The desktop's rows, for GDI, which draws on 32-bit pixels, or on the
// 8-bit pixels of a desktop of 256 colours.

/// The desktop's rows `top..top + rows` as 32-bit pixels (0x00RRGGBB),
/// `width * 4` bytes a row, or as they are at 8 bits a pixel.
pub fn load_desktop(cpu: &Cpu, top: u32, rows: u32) -> Vec<u8> {
    load_desktop_area(cpu, top, rows, 0, desktop(cpu).width)
}

/// The pixels `left..left + cols` of the desktop's rows
/// `top..top + rows`, as `load_desktop` gives them: `cols` pixels a
/// row.
pub fn load_desktop_area(cpu: &Cpu, top: u32, rows: u32, left: u32, cols: u32) -> Vec<u8> {
    let d = desktop(cpu);
    let bytes = d.bpp / 8;
    let row = (cols * bytes) as usize;
    let mut raw = vec![0u8; row * rows as usize];
    if row as u32 == d.pitch {
        read(cpu, top * d.pitch, &mut raw);
    } else if row > 0 {
        for (r, out) in raw.chunks_exact_mut(row).enumerate() {
            read(cpu, (top + r as u32) * d.pitch + left * bytes, out);
        }
    }
    match d.bpp {
        32 | 8 => raw,
        24 => raw
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[b, g, r]| [b, g, r, 0])
            .collect(),
        _ => raw
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| from_565(u16::from_le_bytes(*p)).to_le_bytes())
            .collect(),
    }
}

/// Store pixels as `load_desktop_area` gives them, `cols` a row, as the
/// pixels `left..left + cols` of the desktop's rows from `top` on.
pub fn store_desktop_area(cpu: &mut Cpu, top: u32, left: u32, cols: u32, data: &[u8]) {
    let d = desktop(cpu);
    let bytes = d.bpp / 8;
    let converted: Vec<u8>;
    let raw = if d.bpp == 32 || d.bpp == 8 {
        data
    } else {
        let pixels = data.as_chunks::<4>().0.iter();
        converted = if d.bpp == 24 {
            pixels.flat_map(|&[b, g, r, _]| [b, g, r]).collect()
        } else {
            pixels
                .flat_map(|p| to_565(u32::from_le_bytes(*p)).to_le_bytes())
                .collect()
        };
        &converted
    };
    let row = (cols * bytes) as usize;
    if row as u32 == d.pitch {
        write(cpu, top * d.pitch, raw);
    } else if row > 0 {
        for (r, line) in raw.chunks_exact(row).enumerate() {
            write(cpu, (top + r as u32) * d.pitch + left * bytes, line);
        }
    }
}

/// A 5-6-5 pixel as 0x00RRGGBB, each channel widened by repeating its top
/// bits.
pub fn from_565(p: u16) -> u32 {
    let p = p as u32;
    let r = (p >> 11) & 0x1F;
    let g = (p >> 5) & 0x3F;
    let b = p & 0x1F;
    (r << 3 | r >> 2) << 16 | (g << 2 | g >> 4) << 8 | (b << 3 | b >> 2)
}

/// 0x00RRGGBB as a 5-6-5 pixel.
pub fn to_565(v: u32) -> u16 {
    ((v >> 8) & 0xF800 | (v >> 5) & 0x07E0 | (v >> 3) & 0x001F) as u16
}

// The card's modes.

/// A mode's timing: the pixel clock in Hz, the horizontal display end,
/// sync start, sync end and total in pixels, the same for lines, and
/// whether the syncs are negative.
struct Timing {
    clock: u64,
    h: [u32; 4],
    v: [u32; 4],
    negative: bool,
}

/// The timing of a picture `width` x `lines`: VESA's for its standard
/// sizes, and for others one at about 60 Hz with blanking of their
/// proportions.
fn card_timing(width: u32, lines: u32) -> Timing {
    let t = |clock, h, v, negative| Timing {
        clock,
        h,
        v,
        negative,
    };
    match (width, lines) {
        (640, 400) => t(25_175_000, [640, 656, 752, 800], [400, 412, 414, 449], true),
        (640, 480) => t(25_175_000, [640, 656, 752, 800], [480, 490, 492, 525], true),
        (800, 600) => t(
            40_000_000,
            [800, 840, 968, 1056],
            [600, 601, 605, 628],
            false,
        ),
        (1024, 768) => t(
            65_000_000,
            [1024, 1048, 1184, 1344],
            [768, 771, 777, 806],
            true,
        ),
        (1280, 1024) => t(
            108_000_000,
            [1280, 1328, 1440, 1688],
            [1024, 1025, 1028, 1066],
            false,
        ),
        (1600, 1200) => t(
            162_000_000,
            [1600, 1664, 1856, 2160],
            [1200, 1201, 1204, 1250],
            false,
        ),
        _ => {
            let blank = (width / 4).next_multiple_of(8).max(64);
            let h = [
                width,
                width + blank / 4,
                width + blank / 4 + blank / 2,
                width + blank,
            ];
            let vblank = (lines / 20).max(12);
            let v = [lines, lines + 3, lines + 7, lines + vblank];
            let clock = h[3] as u64 * v[3] as u64 * 60;
            t(clock, h, v, false)
        }
    }
}

/// The PLL's register value for the clock nearest `hz`: N, M and K for
/// (N + 2) / ((M + 2) * 2^K) times 14.318 MHz.
fn pll(hz: u64) -> u32 {
    let mut best = (u64::MAX, 0);
    for k in 0..4u64 {
        for m in 0..64u64 {
            // The N nearest for this M and K.
            let n2 = (hz * ((m + 2) << k) + 7_159_090) / 14_318_180;
            let n = n2.clamp(2, 257) - 2;
            let f = (n + 2) * 14_318_180 / ((m + 2) << k);
            let off = f.abs_diff(hz);
            if off < best.0 {
                best = (off, (n << 8 | m << 2 | k) as u32);
            }
        }
    }
    best.1
}

/// Set the card's extended mode `width` x `height` at `bpp`, as 3dfx's
/// driver does: the VGA's timing registers and the card's extra bits,
/// the PLL, and the video processor. Modes less than 400 pixels wide run
/// at half the clock, and those less than 300 lines high show each line
/// twice (half mode).
fn card_set_mode(cpu: &mut Cpu, width: u32, height: u32, bpp: u32) -> Option<()> {
    let format = match bpp {
        8 => 0,
        16 => 1,
        32 => 3,
        _ => return None,
    };
    if width == 0 || height == 0 || !width.is_multiple_of(8) || width > 1600 || height > 1200 {
        return None;
    }
    let pitch = width * bpp / 8;
    if pitch as u64 * height as u64 > card::MEMORY as u64 {
        return None;
    }
    let half = height < 300;
    let lines = if half { height * 2 } else { height };
    let narrow = width < 400;
    let t = if narrow {
        let t = card_timing(width * 2, lines);
        Timing {
            clock: t.clock / 2,
            h: t.h.map(|x| x / 2),
            ..t
        }
    } else {
        card_timing(width, lines)
    };
    let chars = t.h.map(|x| x / 8);
    let [hde, hss, hse, htotal] = chars;
    let [vde, vss, vse, vtotal] = t.v;
    // CRTC values: horizontal in characters, the totals less 5 and 2.
    let ht = htotal - 5;
    let hd = hde - 1;
    let (hbs, hbe) = (hde, htotal - 1);
    let vt = vtotal - 2;
    let vd = vde - 1;
    let (vbs, vbe) = (vde, vtotal - 1);
    let bit = |v: u32, n: u32, to: u32| ((v >> n) & 1) << to;
    let overflow = bit(vt, 8, 0)
        | bit(vd, 8, 1)
        | bit(vss, 8, 2)
        | bit(vbs, 8, 3)
        | 1 << 4
        | bit(vt, 9, 5)
        | bit(vd, 9, 6)
        | bit(vss, 9, 7);
    let crtc: [(u8, u32); 25] = [
        (0x00, ht),
        (0x01, hd),
        (0x02, hbs),
        (0x03, 0x80 | hbe & 0x1F),
        (0x04, hss),
        (0x05, (hbe & 0x20) << 2 | hse & 0x1F),
        (0x06, vt),
        (0x07, overflow),
        (0x08, 0),
        (0x09, bit(vbs, 9, 5) | 1 << 6),
        (0x0A, 0x20),
        (0x0B, 0),
        (0x0C, 0),
        (0x0D, 0),
        (0x0E, 0),
        (0x0F, 0),
        (0x10, vss),
        // The retrace's end, the vertical interrupt held off, and the
        // registers 0-7 writable.
        (0x11, vse & 0x0F | 0x20),
        (0x12, vd),
        (0x13, pitch / 8),
        (0x14, 0),
        (0x15, vbs),
        (0x16, vbe),
        (0x17, 0xC3),
        (0x18, 0xFF),
    ];
    // The card's bits 8 of the horizontal total and display end, and 10
    // of the vertical total, display end and retrace start.
    let ext_h = bit(ht, 8, 0) | bit(hd, 8, 2);
    let ext_v = bit(vt, 10, 0) | bit(vd, 10, 2) | bit(vss, 10, 6);
    let cfg = card::VIDPROC_ENABLE
        | format << 18
        | if format == 0 {
            0
        } else {
            card::VIDPROC_DESKTOP_CLUT_BYPASS
        }
        | if half { card::VIDPROC_HALF_MODE } else { 0 };
    let bus = &mut cpu.bus;
    // The video processor off while the timing changes.
    let old_cfg = bus.banshee().map_or(0, |b| b.reg(card::VID_PROC_CFG));
    bus.voodoo_write_32(card::VID_PROC_CFG, old_cfg & !card::VIDPROC_ENABLE);
    let init0 = bus.banshee().map_or(0, |b| b.reg(card::VGA_INIT0));
    bus.voodoo_write_32(
        card::VGA_INIT0,
        init0 | card::VGAINIT0_EXTENDED_SHIFT_OUT | card::VGAINIT0_DAC_8BIT,
    );
    bus.voodoo_write_32(card::PLL_CTRL0, pll(t.clock));
    bus.voodoo_write_32(card::DAC_MODE, 0);
    // The PLL's clock, colour, and the syncs' polarity.
    let misc = 0x2F | if t.negative { 0xC0 } else { 0 };
    bus.io_write(0x3C2, misc);
    // Characters of 8 pixels.
    bus.io_write(0x3C4, 0x01);
    bus.io_write(0x3C5, 0x01);
    // Registers 0-7 writable first.
    bus.io_write(0x3D4, 0x11);
    bus.io_write(0x3D5, 0x20);
    for (index, value) in crtc {
        bus.io_write(0x3D4, index);
        bus.io_write(0x3D5, value as u8);
    }
    for (index, value) in [(0x1A, ext_h), (0x1B, ext_v)] {
        bus.io_write(0x3D4, index);
        bus.io_write(0x3D5, value as u8);
    }
    bus.vga.set_fixed_timing(None);
    // BAR1 is linear over the whole memory: the tile base at its end.
    bus.voodoo_write_32(card::LFB_MEMORY_CONFIG, (card::MEMORY >> 12) as u32);
    bus.voodoo_write_32(card::VID_SCREEN_SIZE, width | height << 12);
    bus.voodoo_write_32(card::VID_DESKTOP_START_ADDR, 0);
    bus.voodoo_write_32(card::VID_DESKTOP_OVERLAY_STRIDE, pitch);
    bus.voodoo_write_32(card::VID_PROC_CFG, cfg);
    bus.vga.mark_dirty_full();
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_of_565_go_round() {
        for p in [0u16, 0xFFFF, 0xF800, 0x07E0, 0x001F, 0x1234, 0x8410] {
            assert_eq!(to_565(from_565(p)), p);
        }
        assert_eq!(from_565(0xF800), 0xFF_0000);
        assert_eq!(to_565(0x12_3456), 0x11AA);
    }

    #[test]
    fn the_pll_comes_near() {
        for hz in [25_175_000u64, 40_000_000, 65_000_000, 12_587_500] {
            let v = pll(hz) as u64;
            let (k, m, n) = (v & 3, (v >> 2) & 0x3F, (v >> 8) & 0xFF);
            let f = (n + 2) * 14_318_180 / ((m + 2) << k);
            assert!(f.abs_diff(hz) * 200 < hz, "{hz}: {f}");
        }
    }
}
