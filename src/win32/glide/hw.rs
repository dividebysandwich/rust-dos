//! The card under Glide: which 3dfx board the machine has, its
//! registers, its texture memory, its linear frame buffer mapped into
//! the process, and its display.
//!
//! A Voodoo Graphics or Voodoo2 is an add-on card beside the display
//! adapter. Glide sets its video timing and the layout of its frame
//! buffer in the fbiInit registers (initEnable in its PCI configuration
//! lets them be written), starts a Voodoo Graphics' video clock, and
//! turns the VGA pass-through off with fbiInit0 bit 0: the card has the
//! monitor until the pass-through is turned back on. Its buffers are
//! where the fbiInit registers put them, and its linear frame buffer,
//! 4 MB from offset 400000h of its window, has rows of 1024 pixels.
//!
//! A Banshee or Voodoo3 is the display adapter. Glide takes the display
//! from the desktop or DirectDraw (`display::take`) in a mode of the
//! size it shows, and the rest of the card's 16 MB as a block of
//! DirectDraw's video memory, so that no surface is put there while
//! Glide has it. The 3D engine's colour buffers and depth buffer are at
//! the top, tiled, and the video processor's overlay shows the front
//! one, which a swap moves to the next one (leftOverlayBuf). Textures
//! take the memory below the buffers. Closing gives the display back
//! (`display::give_back`): the desktop's mode, repainted, or text mode.
//! The linear frame buffer is the tiled aperture of BAR1 from the
//! buffers' start (lfbMemoryConfig), rows of 4096 bytes, as 3dfx's
//! library had it; writes through the pixel pipeline and in other
//! formats go to the 3D engine's linear frame buffer in BAR0, rows of
//! 2048 pixels.

use super::state;
use crate::cpu::Cpu;
use crate::voodoo::banshee::{self as b, render3d};
use crate::voodoo::regs::*;
use crate::win32::ddraw::display;
use crate::win32::memory::{MEM_MAPPED, PAGE, PAGE_READWRITE, SHARED, SYSTEM, USER, WRITABLE};

/// Which chip the board has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Card {
    Voodoo1,
    Voodoo2,
    /// A Banshee or Voodoo3: the display adapter.
    Banshee,
}

/// The board, as Glide reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Board {
    pub card: Card,
    /// A Voodoo3 (a Banshee with two texture units).
    pub voodoo3: bool,
    /// Its texture units.
    pub tmus: u32,
    /// Bytes of frame buffer memory, and of each texture unit's (on a
    /// Banshee or Voodoo3 the same 16 MB).
    pub fb_bytes: u32,
    pub tmu_bytes: u32,
}

/// The board the machine has, if it has a 3dfx card.
pub fn board(cpu: &Cpu) -> Option<Board> {
    use crate::voodoo::Board as B;
    let v = cpu.bus.voodoo.as_ref()?;
    const MB: u32 = 1 << 20;
    let (card, voodoo3, tmus, fb, tmu) = match v.board {
        B::Standard => (Card::Voodoo1, false, 1, 2 * MB, 2 * MB),
        B::Max => (Card::Voodoo1, false, 2, 4 * MB, 4 * MB),
        B::Voodoo2 => (Card::Voodoo2, false, 2, 4 * MB, 2 * MB),
        B::Voodoo2Max => (Card::Voodoo2, false, 2, 4 * MB, 4 * MB),
        B::Banshee => (Card::Banshee, false, 1, 16 * MB, 16 * MB),
        B::Voodoo3 => (Card::Banshee, true, 2, 16 * MB, 16 * MB),
    };
    Some(Board {
        card,
        voodoo3,
        tmus,
        fb_bytes: fb,
        tmu_bytes: tmu,
    })
}

/// Whether the card is a Banshee or Voodoo3.
pub fn banshee(cpu: &Cpu) -> bool {
    cpu.bus.banshee().is_some()
}

// --- Registers ---

/// The chips a register write goes to (bits 13:10 of its address): all
/// of them, the FBI, or one texture unit.
pub const ALL: u32 = 0;
pub const FBI: u32 = 1;

/// The chip bits of texture unit `tmu`.
pub fn tmu_chip(tmu: u32) -> u32 {
    2 << tmu.min(2)
}

/// The window offset of 3D register `reg` for the chips `chips`.
fn reg_offset(cpu: &Cpu, chips: u32, reg: usize) -> u32 {
    let offset = (chips & 0xF) << 10 | (reg as u32 & 0xFF) << 2;
    if banshee(cpu) {
        0x20_0000 | offset
    } else {
        offset
    }
}

/// Write the 3D register `reg` of the chips `chips`.
pub fn write(cpu: &mut Cpu, chips: u32, reg: usize, value: u32) {
    let offset = reg_offset(cpu, chips, reg);
    cpu.bus.voodoo_write(offset, value, u32::MAX);
}

/// Write a float register.
pub fn write_f(cpu: &mut Cpu, chips: u32, reg: usize, value: f32) {
    write(cpu, chips, reg, value.to_bits());
}

/// Read the 3D register `reg`.
pub fn read(cpu: &Cpu, reg: usize) -> u32 {
    cpu.bus.voodoo_read_32(reg_offset(cpu, 0, reg))
}

/// A Banshee's init or video register at `offset` of BAR0.
fn ext(cpu: &Cpu, offset: u32) -> u32 {
    cpu.bus.banshee().map_or(0, |b| b.reg(offset))
}

fn set_ext(cpu: &mut Cpu, offset: u32, value: u32) {
    cpu.bus.voodoo_write(offset, value, u32::MAX);
}

/// A byte of the PCI configuration of a Voodoo Graphics or Voodoo2
/// written, as the PCI BIOS would.
fn pci_write(cpu: &mut Cpu, reg: u8, value: u8) {
    let moved = cpu
        .bus
        .voodoo
        .as_mut()
        .is_some_and(|v| v.config_write(reg, value));
    if moved {
        cpu.bus.voodoo_moved();
    }
}

fn pci_read_32(cpu: &Cpu, reg: u8) -> u32 {
    let Some(v) = &cpu.bus.voodoo else { return 0 };
    (0..4)
        .map(|i| (v.config_read(reg + i) as u32) << (8 * i))
        .sum()
}

// --- The linear frame buffer and texture memory ---

/// A dword of the 3D engine's linear frame buffer: rows of 1024 pixels
/// on a Voodoo Graphics or Voodoo2, 2048 on a Banshee.
pub fn lfb_read(cpu: &Cpu, offset: u32) -> u32 {
    let (base, span) = if banshee(cpu) {
        (0x100_0000, 0xFF_FFFC)
    } else {
        (0x40_0000, 0x3F_FFFC)
    };
    cpu.bus.voodoo_read_32(base + (offset & span))
}

/// Write the bytes `mask` selects of a dword of the linear frame
/// buffer.
pub fn lfb_write(cpu: &mut Cpu, offset: u32, value: u32, mask: u32) {
    let (base, span) = if banshee(cpu) {
        (0x100_0000, 0xFF_FFFC)
    } else {
        (0x40_0000, 0x3F_FFFC)
    };
    cpu.bus.voodoo_write(base + (offset & span), value, mask);
}

/// Write a dword of texture unit `tmu`'s texture space on a Voodoo
/// Graphics or Voodoo2: `offset` is LOD << 17 | t << 9 | s << 1.
pub fn texture_write(cpu: &mut Cpu, tmu: u32, offset: u32, value: u32) {
    let at = 0x80_0000 + (tmu.min(1) << 21) + (offset & 0x1F_FFFC);
    cpu.bus.voodoo_write(at, value, u32::MAX);
}

/// Write a dword of a Banshee's texture download space: memory from
/// texture unit 0's texBaseAddr on.
pub fn texture_port_write(cpu: &mut Cpu, offset: u32, value: u32) {
    cpu.bus
        .voodoo_write(0x60_0000 + (offset & 0x1F_FFFC), value, u32::MAX);
}

/// A dword of a Banshee's memory at `addr`, read through BAR1 where
/// that is linear (below lfbMemoryConfig's tile base), or None.
pub fn memory_read(cpu: &Cpu, addr: u32) -> Option<u32> {
    let b = cpu.bus.banshee()?;
    let addr = addr & (b::MEMORY as u32 - 1) & !3;
    (b.linear_addr(addr) == addr).then(|| cpu.bus.voodoo_read_32(b::LINEAR + addr))
}

// --- The display ---

/// A view of the card mapped into the process that has it open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub at: u32,
    pub len: u32,
}

crate::state_fields!(View { at, len });

/// What opening a Voodoo Graphics or Voodoo2 changed, to put back.
#[derive(Clone, Debug, Default)]
pub struct Saved {
    /// initEnable.
    pub init_enable: u32,
}

crate::state_fields!(Saved { init_enable });

/// The display Glide opened and the buffers the card draws into.
#[derive(Clone, Debug, Default)]
pub struct Screen {
    /// Whether the display is open (grSstWinOpen).
    pub open: bool,
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
    /// Colour buffers (2 or 3) and auxiliary buffers (0 or 1).
    pub color_buffers: u32,
    pub aux_buffers: u32,
    /// A Banshee's colour buffers, auxiliary buffer and the tiles a row
    /// of each has (128 bytes by 32 rows each); the bytes of a buffer.
    pub color: [u32; 3],
    pub aux: u32,
    pub tiles: u32,
    pub buffer_bytes: u32,
    /// Which of `color` the front and back buffers are.
    pub front: u32,
    pub back: u32,
    /// The texture memory each texture unit has: where Glide's address
    /// 0 is in the card's memory, and the bytes.
    pub tex_base: [u32; 2],
    pub tex_size: [u32; 2],
    /// The linear frame buffer as the process sees it: a Voodoo
    /// Graphics' or Voodoo2's 4 MB, a Banshee's tiled aperture.
    pub view: View,
    /// A Banshee's 3D linear frame buffer, for writes through the pixel
    /// pipeline.
    pub pipe_view: View,
    pub saved: Saved,
}

crate::state_fields!(Screen {
    open,
    width,
    height,
    refresh,
    color_buffers,
    aux_buffers,
    color,
    aux,
    tiles,
    buffer_bytes,
    front,
    back,
    tex_base,
    tex_size,
    view,
    pipe_view,
    saved
});

/// The texture memory of texture unit `tmu` Glide has: where its
/// address 0 is in the card's memory and how many bytes. Without the
/// display open (Direct3D drawing into DirectDraw's surfaces), the whole
/// of the unit's memory from 0.
pub fn texture_memory(cpu: &mut Cpu, tmu: u32) -> (u32, u32) {
    let screen = &state(cpu).screen;
    if screen.open {
        let t = tmu.min(1) as usize;
        return (screen.tex_base[t], screen.tex_size[t]);
    }
    (0, board(cpu).map_or(0, |b| b.tmu_bytes))
}

/// Glide's buffers on a Banshee: the colour buffers and auxiliary
/// buffer's addresses, tiles a row, bytes each, where they start (the
/// textures' end). None if they don't fit.
struct BansheeLayout {
    color: [u32; 3],
    aux: u32,
    tiles: u32,
    bytes: u32,
    start: u32,
}

fn banshee_layout(
    width: u32,
    height: u32,
    colors: u32,
    auxes: u32,
    floor: u32,
) -> Option<BansheeLayout> {
    let tiles = (width * 2).div_ceil(128);
    let bytes = tiles * 4096 * height.div_ceil(32);
    let total = bytes * (colors + auxes);
    // At least 1 MB for textures above `floor`.
    let start = (b::MEMORY as u32).checked_sub(total)? & !0xFFF;
    if start < floor + (1 << 20) || tiles > 0x7F {
        return None;
    }
    let mut color = [0; 3];
    for (i, c) in color.iter_mut().enumerate().take(colors as usize) {
        *c = start + i as u32 * bytes;
    }
    Some(BansheeLayout {
        color,
        aux: start + colors * bytes,
        tiles,
        bytes,
        start,
    })
}

/// Open the card's display at `width` x `height`, `refresh` Hz, with
/// `colors` colour buffers and `auxes` auxiliary buffers. False if the
/// board can't: no card, a size it doesn't have, not memory enough.
pub fn open_display(
    cpu: &mut Cpu,
    width: u32,
    height: u32,
    refresh: u32,
    colors: u32,
    auxes: u32,
) -> bool {
    let Some(board) = board(cpu) else {
        return false;
    };
    if width == 0 || height == 0 || !(2..=3).contains(&colors) || auxes > 1 {
        return false;
    }
    let ok = match board.card {
        Card::Banshee => open_banshee(cpu, width, height, colors, auxes),
        _ => open_voodoo(cpu, board, width, height, colors, auxes),
    };
    if !ok {
        return false;
    }
    let s = &mut state(cpu).screen;
    s.open = true;
    (s.width, s.height, s.refresh) = (width, height, refresh);
    (s.color_buffers, s.aux_buffers) = (colors, auxes);
    s.front = 0;
    s.back = 1;
    true
}

/// A Voodoo Graphics or Voodoo2: the frame buffer's layout and the video
/// timing in the fbiInit and video registers, and the pass-through off.
fn open_voodoo(
    cpu: &mut Cpu,
    board: Board,
    width: u32,
    height: u32,
    colors: u32,
    auxes: u32,
) -> bool {
    let v2 = board.card == Card::Voodoo2;
    // A Voodoo Graphics' library can't triple buffer; its dimensions are
    // 10 bits, a Voodoo2's 11.
    let max = if v2 { 2047 } else { 1023 };
    if (!v2 && colors == 3) || width > max || height > max {
        return false;
    }
    // Rows of tiles: 64 pixels wide on a Voodoo Graphics, 32 on a
    // Voodoo2; buffers of whole 4 KB pages.
    let tile = if v2 { 32 } else { 64 };
    let tiles = width.div_ceil(tile);
    if (!v2 && tiles > 15) || tiles > 63 {
        return false;
    }
    let pages = (tiles * tile * height * 2).div_ceil(4096);
    if pages > 0x1FF || pages * 4096 * (colors + auxes) > board.fb_bytes {
        return false;
    }
    let saved = pci_read_32(cpu, 0x40);
    state(cpu).screen.saved.init_enable = saved;
    pci_write(cpu, 0x40, (saved as u8) | 1);

    // The layout.
    let init1 = read_init(cpu, FBI_INIT1) & !(0xF << 4 | 1 << 24 | 1 << 8);
    let init2 = read_init(cpu, FBI_INIT2) & !(0x1FF << 11 | 1 << 4);
    // fbiInit3 bit 6, which a Voodoo2 comes out of reset with, turns the
    // texture units off.
    let init3 = read_init(cpu, FBI_INIT3) & !(0x3FF << 22 | 1 << 6);
    let triple_alone = colors == 3 && auxes == 0;
    let mut init1 = init1;
    if v2 {
        init1 |= ((tiles >> 1) & 0xF) << 4 | ((tiles >> 5) & 1) << 24;
        let init6 = read_init(cpu, FBI_INIT6) & !(1 << 30);
        write(cpu, FBI, FBI_INIT6, init6 | (tiles & 1) << 30);
        let init5 = read_init(cpu, FBI_INIT5) & !(3 << 9);
        let three_and_aux = if colors == 3 && auxes == 1 { 2 << 9 } else { 0 };
        write(cpu, FBI, FBI_INIT5, init5 | three_and_aux);
    } else {
        init1 |= tiles << 4;
    }
    write(cpu, FBI, FBI_INIT1, init1);
    write(
        cpu,
        FBI,
        FBI_INIT2,
        init2 | pages << 11 | (triple_alone as u32) << 4,
    );
    write(cpu, FBI, FBI_INIT3, init3 | (height - 1) << 22);

    // The video timing: a quarter of the line and a twentieth of the
    // frame blanked.
    let hsync_on = (width / 16).max(8);
    let htotal = width + (width / 4).max(32);
    let vsync_on = 3;
    let vtotal = height + (height / 20).max(10);
    write(
        cpu,
        FBI,
        H_SYNC,
        (htotal - hsync_on - 2) << 16 | (hsync_on - 1),
    );
    write(cpu, FBI, V_SYNC, (vtotal - vsync_on) << 16 | vsync_on);
    write(cpu, FBI, BACK_PORCH, 16 << 16 | (hsync_on * 2));
    write(cpu, FBI, VIDEO_DIMENSIONS, (height - 1) << 16 | (width - 1));
    // A Voodoo Graphics' video clock, then the monitor.
    if !v2 {
        pci_write(cpu, 0xC0, 0);
    }
    let init0 = read_init(cpu, FBI_INIT0);
    write(cpu, FBI, FBI_INIT0, init0 | 1);
    let s = &mut state(cpu).screen;
    s.tex_base = [0, 0];
    s.tex_size = [board.tmu_bytes; 2];
    true
}

/// An fbiInit register as the card has it.
fn read_init(cpu: &Cpu, reg: usize) -> u32 {
    cpu.bus.voodoo.as_ref().map_or(0, |v| v.reg[reg])
}

/// A Banshee or Voodoo3: the display taken from the desktop or
/// DirectDraw (`display::take`) in a mode of the size Glide shows, and
/// the rest of the card's memory a block of DirectDraw's video memory,
/// so that no surface gets it while Glide has it. Glide's buffers are at
/// its top, tiled, and the overlay shows the front one over the mode's
/// picture; the textures have the memory below them.
fn open_banshee(cpu: &mut Cpu, width: u32, height: u32, colors: u32, auxes: u32) -> bool {
    if width > 1600 || height > 1200 {
        return false;
    }
    // Small modes are shown doubled, as the card's library did.
    let doubled = width <= 400;
    let (dw, dh) = if doubled {
        (width * 2, height * 2)
    } else {
        (width, height)
    };
    // What the picture of the mode will take, and whether the buffers
    // fit above it, before anything changes.
    let floor = (dw * 2 * dh).next_multiple_of(0x1000);
    let Some(layout) = banshee_layout(width, height, colors, auxes, floor) else {
        return false;
    };
    let Some(mode) = display::take(cpu, dw, dh, 16) else {
        return false;
    };
    let Ok(block) = display::alloc(cpu, b::MEMORY as u32 - floor) else {
        display::give_back(cpu);
        return false;
    };

    let tiles = layout.tiles;
    let stride = tiles << 16 | (mode.pitch & 0x7FFF);
    let mut cfg = ext(cpu, b::VID_PROC_CFG)
        | 1 << 21
        | b::VIDPROC_OVERLAY_TILED
        | b::VIDPROC_OVERLAY_ENABLE
        | b::VIDPROC_OVERLAY_CLUT_SEL;
    if doubled {
        cfg |= b::VIDPROC_H_SCALE | b::VIDPROC_V_SCALE;
    }
    set_ext(cpu, b::VID_DESKTOP_OVERLAY_STRIDE, stride);
    set_ext(cpu, b::VID_OVERLAY_START_COORDS, 0);
    set_ext(
        cpu,
        b::VID_OVERLAY_END_SCREEN_COORDS,
        (dw - 1) | (dh - 1) << 12,
    );
    set_ext(cpu, b::VID_OVERLAY_DUDX, (width << 20) / dw);
    set_ext(cpu, b::VID_OVERLAY_DVDY, (height << 20) / dh);
    set_ext(cpu, b::VID_OVERLAY_DVDY_OFFSET, 0);
    set_ext(cpu, b::VID_PROC_CFG, cfg);
    // The aperture: tiled from the buffers on, rows of 4096 bytes; linear
    // below them.
    set_ext(
        cpu,
        b::LFB_MEMORY_CONFIG,
        (layout.start >> 12) & 0x1FFF | 2 << 13 | tiles << 16,
    );
    let misc = ext(cpu, b::MISC_INIT0) & !(0xFFF << 18);
    set_ext(cpu, b::MISC_INIT0, misc | (height - 1) << 18);

    // The 3D engine's buffers, the front one shown.
    let tiled = 1 << 15 | tiles;
    write(cpu, ALL, render3d::COL_BUFFER_STRIDE, tiled);
    write(cpu, ALL, render3d::AUX_BUFFER_STRIDE, tiled);
    write(cpu, ALL, render3d::AUX_BUFFER_ADDR, layout.aux);
    write(cpu, ALL, render3d::COL_BUFFER_ADDR, layout.color[1]);
    write(cpu, ALL, render3d::LEFT_OVERLAY_BUF, layout.color[0]);
    write(cpu, ALL, render3d::SWAP_PENDING, 1);
    write(cpu, ALL, SWAPBUFFER_CMD, 0);

    let s = &mut state(cpu).screen;
    s.color = layout.color;
    s.aux = layout.aux;
    s.tiles = tiles;
    s.buffer_bytes = layout.bytes;
    // The textures from the block's start; the two units share it, a half
    // each.
    let room = layout.start - block;
    let tmus = board(cpu).map_or(1, |b| b.tmus);
    let s = &mut state(cpu).screen;
    if tmus > 1 {
        let half = (room / 2) & !0xFFF;
        s.tex_base = [block, block + half];
        s.tex_size = [half, room - half];
    } else {
        s.tex_base = [block, block];
        s.tex_size = [room, 0];
    }
    true
}

/// Close the card's display and give the screen back.
pub fn close_display(cpu: &mut Cpu) {
    let Some(board) = board(cpu) else { return };
    if !state(cpu).screen.open {
        return;
    }
    unmap_views(cpu);
    match board.card {
        Card::Banshee => close_banshee(cpu),
        card => {
            let init0 = read_init(cpu, FBI_INIT0);
            write(cpu, FBI, FBI_INIT0, init0 & !1);
            if card == Card::Voodoo1 {
                pci_write(cpu, 0xE0, 0);
            }
            let saved = state(cpu).screen.saved.init_enable;
            pci_write(cpu, 0x40, saved as u8);
        }
    }
    let s = &mut state(cpu).screen;
    s.open = false;
}

/// Give the display back (`display::give_back`): the desktop's mode,
/// repainted, or text mode, and the card's memory DirectDraw's again.
fn close_banshee(cpu: &mut Cpu) {
    display::give_back(cpu);
}

/// Set the gamma table: `gamma` 1.0 is linear.
pub fn set_gamma(cpu: &mut Cpu, gamma: f32) {
    let Some(board) = board(cpu) else { return };
    let gamma = if gamma.is_finite() && gamma > 0.0 {
        gamma as f64
    } else {
        1.0
    };
    let level = |i: u32, n: u32| -> u32 {
        let v = (i as f64 / n as f64).powf(1.0 / gamma) * 255.0 + 0.5;
        (v as u32).min(255)
    };
    if board.card == Card::Banshee {
        // The overlay's lookup table, the card's second.
        for i in 0..256 {
            let v = level(i, 255);
            set_ext(cpu, b::DAC_ADDR, 256 + i);
            set_ext(cpu, b::DAC_DATA, v << 16 | v << 8 | v);
        }
        return;
    }
    for i in 0..=32 {
        let v = level(i, 32);
        write(cpu, FBI, CLUT_DATA, i << 24 | v << 16 | v << 8 | v);
    }
}

// --- Views of the linear frame buffer ---

/// Map the card's window from `phys` on, `len` bytes, into the running
/// process: where, or None if its address space has no room.
fn map_view(cpu: &mut Cpu, phys: u32, len: u32) -> Option<View> {
    let len = len.next_multiple_of(PAGE);
    let at = crate::win32::with_process(cpu, |p, bus| {
        let at = p
            .memory
            .reserve_region(None, len, SHARED, SYSTEM, PAGE_READWRITE, MEM_MAPPED)?;
        for page in 0..len / PAGE {
            let offset = page * PAGE;
            p.memory
                .map(bus, at + offset, phys + offset, WRITABLE | USER)?;
        }
        Some(at)
    })?;
    cpu.tlb.flush();
    Some(View { at, len })
}

fn unmap_view(cpu: &mut Cpu, view: View) {
    if view.at == 0 {
        return;
    }
    crate::win32::with_process(cpu, |p, bus| {
        for page in 0..view.len / PAGE {
            p.memory.unmap(bus, view.at + page * PAGE);
        }
        p.memory.unreserve(view.at);
    });
    cpu.tlb.flush();
}

/// Take the views away from the running process, which has the card
/// open.
fn unmap_views(cpu: &mut Cpu) {
    let s = &mut state(cpu).screen;
    let views = [
        std::mem::take(&mut s.view),
        std::mem::take(&mut s.pipe_view),
    ];
    for view in views {
        unmap_view(cpu, view);
    }
}

/// The process's view of the linear frame buffer, mapped if it isn't
/// yet: a Voodoo Graphics' or Voodoo2's 4 MB, a Banshee's aperture over
/// Glide's buffers.
pub fn view(cpu: &mut Cpu) -> Option<u32> {
    let s = &state(cpu).screen;
    if s.view.at != 0 {
        return Some(s.view.at);
    }
    let (phys, len) = if banshee(cpu) {
        let s = &state(cpu).screen;
        let rows = s.height.div_ceil(32) * 32;
        let buffers = s.color_buffers + s.aux_buffers;
        let start = s.color[0];
        let bar1 = cpu.bus.banshee()?.pci.bar1;
        (bar1 + start, buffers * rows * 4096)
    } else {
        let base = cpu.bus.voodoo.as_ref()?.base()?;
        (base + 0x40_0000, 0x40_0000)
    };
    let view = map_view(cpu, phys, len)?;
    state(cpu).screen.view = view;
    Some(view.at)
}

/// Where buffer `index` of the aperture (colour buffers, then the
/// auxiliary one) starts in a Banshee's view.
pub fn aperture_offset(cpu: &mut Cpu, addr: u32) -> u32 {
    let s = &state(cpu).screen;
    let rows = s.height.div_ceil(32) * 32;
    let index = addr.wrapping_sub(s.color[0]) / s.buffer_bytes.max(1);
    index * rows * 4096
}

/// A Banshee's 3D linear frame buffer in the process, mapped if it
/// isn't yet: rows of 2048 pixels.
pub fn pipe_view(cpu: &mut Cpu) -> Option<u32> {
    let s = &state(cpu).screen;
    if s.pipe_view.at != 0 {
        return Some(s.pipe_view.at);
    }
    let rows = s.height.div_ceil(32) * 32;
    let len = (rows * 8192).min(16 << 20);
    let phys = cpu.bus.banshee()?.pci.bar0 + 0x100_0000;
    let view = map_view(cpu, phys, len)?;
    state(cpu).screen.pipe_view = view;
    Some(view.at)
}
