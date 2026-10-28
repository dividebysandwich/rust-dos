//! The 3dfx device: a Voodoo Graphics or Voodoo2 add-on card as
//! DirectDraw's second device, beside the display adapter, as 3dfx's
//! DirectX drivers made it. DirectDrawEnumerate lists it with the GUID,
//! description and driver name those drivers registered, and
//! DirectDrawCreate with that GUID makes a DirectDraw object whose
//! display is the card.
//!
//! Its modes are the card's full-screen ones at 16 bits a pixel (5-6-5),
//! as many as its frame buffer holds two colour buffers of. In
//! exclusive mode, SetDisplayMode opens the card through Glide's
//! hardware layer (`glide::api::open`): the card takes the monitor from
//! the VGA through its pass-through, with two colour buffers and an
//! auxiliary (depth) buffer when the memory holds one. RestoreDisplayMode,
//! the end of exclusive mode, the DirectDraw object's end and the
//! process's give the VGA its picture back.
//!
//! The primary surface and its flip chain are the card's front and back
//! buffers, and a Z buffer is its auxiliary buffer: such a surface's
//! pixels are `MEM_CARD`, its `bits` the buffer (`BUFFER_` of Glide).
//! Flip swaps the card's buffers, so the surfaces keep their buffers.
//! Lock gives the card's linear frame buffer in the process, rows of
//! 1024 pixels, with lfbMode set to read and write the surface's
//! buffer; as the linear frame buffer shows one buffer at a time, one
//! of the card's surfaces is locked at a time. Blits fill the card's
//! buffers with its fastfill and copy through its linear frame buffer.
//! Other surfaces of the device are in system memory.

use super::com::Guid;
use super::surface::{self, MEM_CARD, Surface};
use super::{DD_OK, DDERR_INVALIDMODE, DDERR_NOEXCLUSIVEMODE, DDERR_SURFACEBUSY, guid, state};
use crate::cpu::Cpu;
use crate::voodoo::regs::{
    CLIP_LEFT_RIGHT, CLIP_LOW_Y_HIGH_Y, COLOR1, FASTFILL_CMD, FBZ_MODE, LFB_MODE, ZA_COLOR,
};
use crate::win32::glide;
use crate::win32::glide::api::{self, BUFFER_AUXBUFFER, LFB_READ_ONLY, LFB_WRITE_ONLY};
use crate::win32::glide::hw::{self, Card};

/// The GUIDs, descriptions and driver names 3dfx's drivers registered
/// under DirectDrawDrivers: Windows 98's "3Dfx Voodoo" driver for a
/// Voodoo Graphics, 3dfx's 3.03 driver for a Voodoo2.
const VOODOO1: Device = Device {
    guid: guid(
        0x3A0C_FD01,
        0x9320,
        0x11CF,
        [0xAC, 0xA1, 0x00, 0xA0, 0x24, 0x13, 0xC2, 0xE2],
    ),
    description: b"3dfx DirectX Driver\0",
    name: b"mm3dfx\0",
};
const VOODOO2: Device = Device {
    guid: guid(
        0x472B_EA00,
        0x40DF,
        0x11D1,
        [0xA9, 0xDF, 0x00, 0x60, 0x97, 0xC2, 0xED, 0xB2],
    ),
    description: b"Voodoo2 DirectX 6 Driver\0",
    name: b"3dfxV2\0",
};

/// How DirectDrawEnumerate shows the device.
pub struct Device {
    pub guid: Guid,
    /// Its description and driver name, with their NULs.
    pub description: &'static [u8],
    pub name: &'static [u8],
}

/// The card's sizes, the Voodoo Graphics' first.
const SIZES: [(u32, u32); 5] = [(512, 384), (640, 400), (640, 480), (800, 600), (1024, 768)];

// fbzMode's and lfbMode's bits.
const ENRECTCLIP: u32 = 1;
const RGBWRMASK: u32 = 1 << 9;
const ZAWRMASK: u32 = 1 << 10;
const ENDITHER: u32 = 1 << 8;
const ENZBIAS: u32 = 1 << 16;
const LFB_FORMAT: u32 = 0xF;
const LFB_WRITE_BUFFER: u32 = 3 << 4;
const LFB_READ_BUFFER: u32 = 3 << 6;
const LFB_ENPIXPIPE: u32 = 1 << 8;
const LFB_YORIGIN: u32 = 1 << 13;

/// The card's buffers' pitch in its linear frame buffer: 1024 pixels.
pub const PITCH: u32 = 2048;

/// The device's state: the system's, as there is one card.
#[derive(Clone, Debug, Default)]
pub struct CardDisplay {
    /// The DirectDraw object of the device in exclusive mode, and the
    /// one whose mode the card shows, or 0.
    pub exclusive: u32,
    pub owner: u32,
    /// The mode's width and height, 0 while the card is closed.
    pub mode: (u32, u32),
    /// Whether the mode has an auxiliary buffer.
    pub aux: bool,
}

crate::state_fields!(CardDisplay {
    exclusive,
    owner,
    mode,
    aux
});

/// The add-on card the machine has: a Voodoo Graphics or Voodoo2.
fn board(cpu: &Cpu) -> Option<hw::Board> {
    hw::board(cpu).filter(|b| b.card != Card::Banshee)
}

/// The 3dfx device, if the machine has an add-on card.
pub fn device(cpu: &Cpu) -> Option<&'static Device> {
    Some(match board(cpu)?.card {
        Card::Voodoo2 => &VOODOO2,
        _ => &VOODOO1,
    })
}

/// The card's PCI device ID (3dfx's vendor ID is 0x121A).
pub fn pci_device(cpu: &Cpu) -> u32 {
    match board(cpu).map(|b| b.card) {
        Some(Card::Voodoo2) => 2,
        _ => 1,
    }
}

/// Whether `guid` is the 3dfx device's.
pub fn is_device(cpu: &Cpu, guid: &Guid) -> bool {
    device(cpu).is_some_and(|d| d.guid == *guid)
}

/// Bytes of one of the card's buffers at `w` x `h`, as Glide lays them
/// out (`hw::open_voodoo`): rows of 64-pixel tiles on a Voodoo Graphics,
/// 32 on a Voodoo2, in whole 4 KB pages. None if the card can't show it.
fn buffer_bytes(card: Card, w: u32, h: u32) -> Option<u32> {
    let v2 = card == Card::Voodoo2;
    let tile = if v2 { 32 } else { 64 };
    let tiles = w.div_ceil(tile);
    if (!v2 && tiles > 15) || tiles > 63 || h > 1023 {
        return None;
    }
    let pages = (tiles * tile * h * 2).div_ceil(4096);
    (pages <= 0x1FF).then_some(pages * 4096)
}

/// The modes the device offers: the card's sizes at 16 bits, those its
/// frame buffer holds two colour buffers of.
pub fn modes(cpu: &Cpu) -> Vec<(u32, u32, u32)> {
    let Some(b) = board(cpu) else {
        return Vec::new();
    };
    let sizes = if b.card == Card::Voodoo2 { 5 } else { 4 };
    SIZES[..sizes]
        .iter()
        .filter(|&&(w, h)| buffer_bytes(b.card, w, h).is_some_and(|n| 2 * n <= b.fb_bytes))
        .map(|&(w, h)| (w, h, 16))
        .collect()
}

/// The card's frame buffer, and how much of it the mode's buffers
/// leave.
pub fn video_memory(cpu: &mut Cpu) -> (u32, u32) {
    let Some(b) = board(cpu) else {
        return (0, 0);
    };
    let c = state(cpu).card.clone();
    let used = buffer_bytes(b.card, c.mode.0, c.mode.1).map_or(0, |n| n * (2 + c.aux as u32));
    (b.fb_bytes, b.fb_bytes.saturating_sub(used))
}

/// The card's texture memory, its units' together, and how much of it
/// the 3dfx device's textures in video memory leave, those in
/// `leaving_out` not counted. They are kept in system memory, and the
/// Direct3D HAL puts them on the card as it draws with them, but they
/// take room here as on the card, so that a program that makes textures
/// until there is no more room (Unreal) stops where it did on the card.
pub fn texture_memory(cpu: &mut Cpu, leaving_out: &[u32]) -> (u32, u32) {
    let Some(b) = board(cpu) else {
        return (0, 0);
    };
    let total = b.tmus * b.tmu_bytes;
    let st = state(cpu);
    let used: u32 = st
        .objects
        .iter()
        .enumerate()
        .filter(|(i, _)| !leaving_out.contains(&(*i as u32 + 1)))
        .filter_map(|(_, o)| o.as_ref())
        .filter(|o| {
            st.object(o.owner)
                .and_then(|d| d.ddraw.as_ref())
                .is_some_and(|d| d.card)
        })
        .filter_map(|o| o.surface.as_ref())
        .filter(|s| {
            s.caps[0] & surface::DDSCAPS_TEXTURE != 0
                && s.caps[0] & surface::DDSCAPS_VIDEOMEMORY != 0
        })
        .map(|s| s.size())
        .sum();
    (total, total.saturating_sub(used))
}

/// The mode the device shows: width, height, bits a pixel and bytes a
/// row. 640 x 480 while it has none.
pub fn current(cpu: &mut Cpu) -> (u32, u32, u32, u32) {
    match state(cpu).card.mode {
        (0, _) => (640, 480, 16, PITCH),
        (w, h) => (w, h, 16, PITCH),
    }
}

/// The card's video timing and the time now, while the device's mode
/// shows: its own frame, from its video registers.
pub fn timing(cpu: &mut Cpu) -> Option<(crate::video::crt::CrtTiming, u64)> {
    if state(cpu).card.mode.0 == 0 {
        return None;
    }
    let t = cpu.bus.voodoo.as_ref()?.timing();
    (t.frame_ns() > 0 && t.line_ns > 0).then(|| (t, cpu.bus.clock.now_ns()))
}

/// SetDisplayMode of the DirectDraw object `owner`, which is in
/// exclusive mode: the card opens at `w` x `h`. Its surfaces are lost.
pub fn set_mode(cpu: &mut Cpu, owner: u32, w: u32, h: u32, bpp: u32, refresh: u32) -> u32 {
    if state(cpu).card.exclusive != owner {
        return DDERR_NOEXCLUSIVEMODE;
    }
    let Some(b) = board(cpu) else {
        return DDERR_INVALIDMODE;
    };
    if !modes(cpu).contains(&(w, h, bpp)) {
        return DDERR_INVALIDMODE;
    }
    let aux = buffer_bytes(b.card, w, h).is_some_and(|n| 3 * n <= b.fb_bytes);
    lost(cpu);
    let hz = if refresh == 0 { 60 } else { refresh };
    let ok = api::open(
        cpu,
        w,
        h,
        hz,
        api::COLORFORMAT_ARGB,
        api::ORIGIN_UPPER_LEFT,
        2,
        aux as u32,
    );
    let card = &mut state(cpu).card;
    if !ok {
        card.mode = (0, 0);
        card.owner = 0;
        return DDERR_INVALIDMODE;
    }
    card.mode = (w, h);
    card.aux = aux;
    card.owner = owner;
    // Pixels as they are given, which a dithered write of the linear
    // frame buffer changes.
    api::dither_mode(cpu, api::DITHER_DISABLE);
    DD_OK
}

/// Close the card, if a DirectDraw object has it open: the VGA has the
/// monitor again, and the card's surfaces are lost.
pub fn close(cpu: &mut Cpu) {
    if state(cpu).card.owner == 0 {
        return;
    }
    lost(cpu);
    api::close(cpu);
    let card = &mut state(cpu).card;
    card.owner = 0;
    card.mode = (0, 0);
    card.aux = false;
}

/// The surfaces in the card's buffers lose them.
fn lost(cpu: &mut Cpu) {
    for id in state(cpu).ids(|o| o.surface.as_ref().is_some_and(|s| s.memory == MEM_CARD)) {
        if let Some(s) = state(cpu).surface_mut(id) {
            s.memory = surface::MEM_NONE;
            s.bits = 0;
            s.lost = true;
            s.locked = false;
        }
    }
}

/// The DirectDraw object `id` of the device leaves exclusive mode: the
/// card closes if it is its.
pub fn end_exclusive(cpu: &mut Cpu, id: u32) {
    let card = &mut state(cpu).card;
    if card.exclusive == id {
        card.exclusive = 0;
    }
    if state(cpu).card.owner == id {
        close(cpu);
    }
}

/// The DirectDraw object `id` goes.
pub fn object_gone(cpu: &mut Cpu, id: u32) {
    end_exclusive(cpu, id);
}

/// A process ended. Glide closes the card it opened for the process
/// (`glide::process_ended`, after DirectDraw's); the device forgets it.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let s = state(cpu);
    let gone = |id: u32| s.object(id).is_some_and(|o| o.pid == pid);
    let (exclusive, owner) = (gone(s.card.exclusive), gone(s.card.owner));
    if exclusive {
        s.card.exclusive = 0;
    }
    if owner {
        s.card = CardDisplay::default();
        lost(cpu);
    }
}

/// Whether the DirectDraw object `id` is the 3dfx device's: for
/// Direct3D, which draws with the card on it.
#[allow(dead_code)] // For Direct3D's HAL.
pub fn is_card_object(cpu: &mut Cpu, id: u32) -> bool {
    state(cpu)
        .object(id)
        .and_then(|o| o.ddraw.as_ref())
        .is_some_and(|d| d.card)
}

/// Which of the card's buffers the surface `id` is (`BUFFER_` of
/// Glide: front, back or auxiliary), for Direct3D. None if it isn't one
/// of them, or is lost.
#[allow(dead_code)] // For Direct3D's HAL.
pub fn card_buffer(cpu: &mut Cpu, id: u32) -> Option<u32> {
    let s = state(cpu).surface(id)?;
    (s.memory == MEM_CARD && !s.lost).then_some(s.bits)
}

/// The buffer a surface of the device's is to have, by its caps: None
/// for one in system memory.
pub fn role(s: &Surface) -> Option<u32> {
    let caps = s.caps[0];
    if caps & surface::DDSCAPS_ZBUFFER != 0 {
        Some(BUFFER_AUXBUFFER)
    } else if s.is_primary() || caps & surface::DDSCAPS_FRONTBUFFER != 0 {
        Some(api::BUFFER_FRONTBUFFER)
    } else if caps & (surface::DDSCAPS_BACKBUFFER | surface::DDSCAPS_FLIP) != 0 {
        Some(api::BUFFER_BACKBUFFER)
    } else {
        None
    }
}

/// Whether the card can give a surface of `w` x `h` the buffer `role`:
/// the mode is set, and is the size of a colour buffer; a Z buffer is
/// as big as the mode at most, and the mode has an auxiliary buffer.
pub fn has_buffer(cpu: &mut Cpu, role: u32, w: u32, h: u32) -> bool {
    let c = state(cpu).card.clone();
    if c.mode.0 == 0 {
        return false;
    }
    if role == BUFFER_AUXBUFFER {
        c.aux && w <= c.mode.0 && h <= c.mode.1
    } else {
        (w, h) == c.mode
    }
}

/// The surface of a buffer of the card's other than `except` that is
/// locked, or has a DC.
fn busy(cpu: &mut Cpu, except: u32) -> bool {
    let list = state(cpu).ids(|o| {
        o.surface
            .as_ref()
            .is_some_and(|s| s.memory == MEM_CARD && (s.locked || s.dc != 0))
    });
    list.iter().any(|&id| id != except)
}

/// The lfbMode format of a buffer's pixels: 5-6-5, or 16-bit depth for
/// the auxiliary buffer.
fn format(role: u32) -> u32 {
    if role == BUFFER_AUXBUFFER {
        api::LFBWRITEMODE_ZA16
    } else {
        api::LFBWRITEMODE_565
    }
}

/// Lock the buffer of the surface `id` for reading and writing through
/// the linear frame buffer: where the process sees it, or the error.
pub fn lock(cpu: &mut Cpu, id: u32, role: u32) -> Result<u32, u32> {
    if busy(cpu, id) {
        return Err(DDERR_SURFACEBUSY);
    }
    let origin = api::ORIGIN_UPPER_LEFT;
    let mode = format(role);
    let write =
        api::lfb_lock(cpu, LFB_WRITE_ONLY, role, mode, origin, false).ok_or(DDERR_SURFACEBUSY)?;
    if api::lfb_lock(cpu, LFB_READ_ONLY, role, mode, origin, false).is_none() {
        api::lfb_unlock(cpu, LFB_WRITE_ONLY, role);
        return Err(DDERR_SURFACEBUSY);
    }
    // The program's pixels undithered, whatever Direct3D draws with.
    let fbz = glide::state(cpu).gc.fbz_mode & !(ENZBIAS | ENDITHER);
    hw::write(cpu, hw::ALL, FBZ_MODE, fbz);
    Ok(write.ptr)
}

/// Unlock the buffer `role`.
pub fn unlock(cpu: &mut Cpu, role: u32) {
    api::lfb_unlock(cpu, LFB_WRITE_ONLY, role);
    api::lfb_unlock(cpu, LFB_READ_ONLY, role);
}

/// Whether byte `at` of a card surface's rows is one of its pixels':
/// columns past its width are other rows' tiles, or none.
fn inside(s: &Surface, at: u32) -> bool {
    at % PITCH < s.width * 2 && at / PITCH < s.height
}

/// Read runs of `len` bytes of a card surface's buffer, rows of `PITCH`
/// bytes, into `data` one after another: the first from byte `offset`,
/// each of the others `PITCH` bytes after the one before. Bytes past
/// its width read as 0. The linear frame buffer is set up once for all
/// of them.
pub fn read(cpu: &mut Cpu, s: &Surface, offset: u32, len: usize, data: &mut [u8]) {
    if len == 0 {
        return;
    }
    let saved = glide::state(cpu).gc.lfb_mode;
    let mode = (saved & !(LFB_READ_BUFFER | LFB_YORIGIN)) | s.bits << 6;
    hw::write(cpu, hw::ALL, LFB_MODE, mode);
    for (run, y) in data.chunks_mut(len).zip(0u32..) {
        read_run(cpu, s, offset + y * PITCH, run);
    }
    hw::write(cpu, hw::ALL, LFB_MODE, saved);
}

/// `read` of one run, the frame buffer set up for it.
fn read_run(cpu: &mut Cpu, s: &Surface, offset: u32, data: &mut [u8]) {
    let end = offset + data.len() as u32;
    let mut at = offset & !3;
    while at < end {
        let value = if inside(s, at) {
            hw::lfb_read(cpu, at)
        } else {
            0
        };
        for (i, byte) in value.to_le_bytes().iter().enumerate() {
            let b = at + i as u32;
            if (offset..end).contains(&b) {
                data[(b - offset) as usize] = *byte;
            }
        }
        at += 4;
    }
}

/// Write runs of `len` bytes of `data`, one after another, into a card
/// surface's buffer: the first at byte `offset`, each of the others
/// `PITCH` bytes after the one before. Bytes past its width are left.
/// The linear frame buffer is set up once for all of them.
pub fn write(cpu: &mut Cpu, s: &Surface, offset: u32, len: usize, data: &[u8]) {
    if len == 0 {
        return;
    }
    let g = glide::state(cpu).gc.clone();
    let target = if s.bits == BUFFER_AUXBUFFER {
        0
    } else {
        s.bits
    };
    let mode = (g.lfb_mode & !(LFB_WRITE_BUFFER | LFB_YORIGIN | LFB_FORMAT | LFB_ENPIXPIPE))
        | target << 4
        | format(s.bits);
    hw::write(cpu, hw::ALL, LFB_MODE, mode);
    hw::write(cpu, hw::ALL, FBZ_MODE, g.fbz_mode & !(ENZBIAS | ENDITHER));
    for (run, y) in data.chunks(len).zip(0u32..) {
        write_run(cpu, s, offset + y * PITCH, run);
    }
    hw::write(cpu, hw::ALL, LFB_MODE, g.lfb_mode);
    hw::write(cpu, hw::ALL, FBZ_MODE, g.fbz_mode);
}

/// `write` of one run, the frame buffer set up for it.
fn write_run(cpu: &mut Cpu, s: &Surface, offset: u32, data: &[u8]) {
    let end = offset + data.len() as u32;
    let mut at = offset & !3;
    while at < end {
        if inside(s, at) {
            let mut value = 0u32;
            let mut mask = 0u32;
            for i in 0..4 {
                let b = at + i;
                if (offset..end).contains(&b) {
                    value |= (data[(b - offset) as usize] as u32) << (8 * i);
                    mask |= 0xFF << (8 * i);
                }
            }
            hw::lfb_write(cpu, at, value, mask);
        }
        at += 4;
    }
}

/// Fill rectangles of a card surface's buffer with `value` (a 5-6-5
/// colour, or a depth) by the card's fastfill.
pub fn fill(cpu: &mut Cpu, s: &Surface, rects: &[crate::win32::gdi32::Rect], value: u32) {
    let g = glide::state(cpu).gc.clone();
    let fbz = if s.bits == BUFFER_AUXBUFFER {
        hw::write(cpu, hw::ALL, ZA_COLOR, value & 0xFFFF);
        ENRECTCLIP | ZAWRMASK
    } else {
        // 5-6-5 to 8-8-8, each channel's top bits again below it, which
        // the fill without dithering takes back to the same 5-6-5.
        let (r, gr, b) = ((value >> 11) & 0x1F, (value >> 5) & 0x3F, value & 0x1F);
        let rgb = (r << 3 | r >> 2) << 16 | (gr << 2 | gr >> 4) << 8 | (b << 3 | b >> 2);
        hw::write(cpu, hw::ALL, COLOR1, rgb);
        ENRECTCLIP | RGBWRMASK | s.bits << 14
    };
    hw::write(cpu, hw::ALL, FBZ_MODE, fbz);
    for r in rects {
        let (left, top) = (r.left.max(0) as u32, r.top.max(0) as u32);
        let (right, bottom) = (r.right.max(0) as u32, r.bottom.max(0) as u32);
        hw::write(cpu, hw::ALL, CLIP_LEFT_RIGHT, left << 16 | right);
        hw::write(cpu, hw::ALL, CLIP_LOW_Y_HIGH_Y, top << 16 | bottom);
        hw::write(cpu, hw::ALL, FASTFILL_CMD, 1);
    }
    hw::write(cpu, hw::ALL, FBZ_MODE, g.fbz_mode);
    hw::write(cpu, hw::ALL, CLIP_LEFT_RIGHT, g.clip_left_right);
    hw::write(cpu, hw::ALL, CLIP_LOW_Y_HIGH_Y, g.clip_bottom_top);
    hw::write(cpu, hw::ALL, COLOR1, g.color1);
    hw::write(cpu, hw::ALL, ZA_COLOR, g.za_color);
}

/// Flip: the card's back buffer shows, at the next vertical retrace
/// unless `vsync` is false.
pub fn swap(cpu: &mut Cpu, vsync: bool) {
    api::buffer_swap(cpu, vsync as u32);
}

/// When the card's swaps still waiting for their retraces are all done,
/// by the clock's nanoseconds: None while none waits
/// (grBufferNumPending gives 0).
pub fn swaps_pending_until(cpu: &Cpu) -> Option<u64> {
    let due = cpu
        .bus
        .voodoo
        .as_ref()?
        .swaps_done_at(cpu.bus.clock.now_ticks())?;
    Some(crate::timer::mul_div_ceil(
        due,
        1_000_000_000,
        crate::timer::PIT_HZ,
    ))
}
