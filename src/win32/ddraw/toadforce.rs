//! DirectDraw's blits done by a ToadForce (`crate::toadforce`): colour
//! fills and copies of the same size between surfaces in card memory, as
//! the card's `FILL` and `BLT` commands. The driver here builds the
//! words a guest's driver would put in the command ring and hands them
//! to the card (`Bus::toadforce_submit`).
//!
//! A copy from a surface in system memory to one in card memory is the
//! card's too. The driver writes the source's rectangle into a block of
//! card memory it takes for the copy, and the card copies from there.
//! Written by the processor, the destination's rows would have to be
//! in card memory first: a destination the host renderer draws is read
//! back from the GPU for every such blit, which a program that puts
//! its interface on the back buffer this way does every frame (No One
//! Lives Forever's menus).
//!
//! Only blits the card does exactly as DirectDraw's own code does them
//! come here, so a picture doesn't depend on which of the two drew it.
//! A stretch isn't one: the card takes the source pixel under each
//! destination pixel's centre, and DirectDraw's code the one under its
//! corner. Mirrored blits, destination colour keys and keys of a range
//! of colours aren't the card's at all.

use super::blt::Key;
use super::display;
use super::surface::{self, Surface};
use crate::cpu::Cpu;
use crate::toadforce::caps;
use crate::toadforce::command::encode;
use crate::toadforce::format::{self, Format};
use crate::win32::gdi32::Rect;
use crate::win32::screen;

/// The surface `s` as the card's commands name it, its pixels in
/// `format`: None if the card takes no such surface.
fn card_surface(cpu: &Cpu, s: &Surface, format: Format) -> Option<format::Surface> {
    if s.memory != surface::MEM_VIDEO || s.lost || s.compressed() {
        return None;
    }
    let words = [s.bits, s.pitch, s.width, s.height, format.code()];
    format::Surface::parse(&words, caps::BLIT_FORMATS, screen::memory_size(cpu)).ok()
}

/// The card's format for pixels of `bytes` bytes: for those of 4, the
/// one with alpha if `all_bits` of a pixel count.
fn card_format(bytes: u32, all_bits: bool) -> Option<Format> {
    match bytes {
        1 => Some(Format::P8),
        2 => Some(Format::R5G6B5),
        4 if all_bits => Some(Format::A8R8G8B8),
        4 => Some(Format::X8R8G8B8),
        _ => None,
    }
}

fn card_rect(r: &Rect) -> format::Rect {
    format::Rect {
        left: r.left as u32,
        top: r.top as u32,
        width: r.width() as u32,
        height: r.height() as u32,
    }
}

/// The blit a card does: what `fill` and `copy` take of `blt::Op`.
pub(super) struct Blit<'a> {
    /// The destination, and the rectangles of it to draw, which lie
    /// inside it and don't overlap.
    pub dst: &'a Surface,
    pub rects: &'a [Rect],
    /// The destination's rectangle before clipping, which the source's
    /// rectangle goes to.
    pub area: Rect,
}

/// Fill the blit's rectangles with `value`: true if the card did.
pub(super) fn fill(cpu: &mut Cpu, blit: &Blit, value: u32) -> bool {
    if !screen::is_toadforce(cpu) {
        return false;
    }
    let Some(format) = card_format(blit.dst.bytes_per_pixel(), false) else {
        return false;
    };
    let Some(dst) = card_surface(cpu, blit.dst, format) else {
        return false;
    };
    let mut words = Vec::new();
    for r in blit.rects {
        encode::fill(&mut words, &dst, &card_rect(r), value);
    }
    submit(cpu, blit, &words);
    true
}

/// Copy the rectangle `from` of the surface `src` to the blit's area,
/// leaving out the source's pixels in `key`: true if the card did.
pub(super) fn copy(
    cpu: &mut Cpu,
    blit: &Blit,
    src: &Surface,
    from: &Rect,
    key: Option<(u32, u32)>,
) -> bool {
    if !screen::is_toadforce(cpu) {
        return false;
    }
    let bytes = blit.dst.bytes_per_pixel();
    if src.bytes_per_pixel() != bytes
        || from.width() != blit.area.width()
        || from.height() != blit.area.height()
    {
        return false;
    }
    // The key as the card compares one: a single value, in every bit
    // its format defines.
    let (format, key) = match key.map(|k| Key::new(k, src)) {
        None => (card_format(bytes, false), None),
        Some(Key::Exact { mask, value }) => {
            let format = [false, true]
                .into_iter()
                .filter_map(|all| card_format(bytes, all))
                .find(|f| {
                    let bits = f.defined_bits();
                    mask & bits == bits && (mask == bits || bytes < 4) && value & !bits == 0
                });
            (format, Some(value))
        }
        Some(Key::Ranges { .. }) => return false,
    };
    let Some(format) = format else {
        return false;
    };
    let Some(dst) = card_surface(cpu, blit.dst, format) else {
        return false;
    };
    // A source in system memory is copied from a block of card memory
    // that holds its rectangle, whose top left is then the block's.
    let staged = src.memory == surface::MEM_SYSTEM;
    let inside = from.left >= 0
        && from.top >= 0
        && from.right as i64 <= src.width as i64
        && from.bottom as i64 <= src.height as i64;
    let (source, origin) = if staged {
        if !inside || src.lost || src.compressed() {
            return false;
        }
        let Some(source) = stage(cpu, src, from, format) else {
            return false;
        };
        (source, (from.left, from.top))
    } else {
        let Some(source) = card_surface(cpu, src, format) else {
            return false;
        };
        (source, (0, 0))
    };
    // Each rectangle's part of the source, which has to be in it.
    let (dx, dy) = (from.left - blit.area.left, from.top - blit.area.top);
    let (dx, dy) = (dx - origin.0, dy - origin.1);
    let pairs: Vec<(format::Rect, format::Rect)> = blit
        .rects
        .iter()
        .map(|r| {
            let s = Rect::new(r.left + dx, r.top + dy, r.right + dx, r.bottom + dy);
            (card_rect(r), s)
        })
        .filter(|(_, s)| s.left >= 0 && s.top >= 0)
        .map(|(d, s)| (d, card_rect(&s)))
        .filter(|(_, s)| source.holds(s))
        .collect();
    // The block goes back whatever comes of the blit. The card runs
    // the commands queued below before anything else writes its
    // memory (`Bus::toadforce_submit`), the next blit's block too.
    if staged {
        display::free(cpu, source.offset);
    }
    if pairs.len() != blit.rects.len() {
        return false;
    }
    // DirectDraw reads the whole source before it writes. The card does
    // for one command: several whose source the ones before them drew
    // over would not.
    let overlaps = |a: (u32, u32), b: (u32, u32)| a.0 < b.1 && b.0 < a.1;
    let unsafe_order = pairs.len() > 1
        && pairs.iter().any(|(d, _)| {
            pairs
                .iter()
                .any(|(_, s)| overlaps(dst.span(d), source.span(s)))
        });
    if unsafe_order {
        return false;
    }
    let mut words = Vec::new();
    for (d, s) in &pairs {
        encode::blt(&mut words, &dst, d, &source, s, key);
    }
    submit(cpu, blit, &words);
    true
}

/// A window's back buffer `back` put on the screen: its rectangle
/// `from` copied to the rectangle `to` of the picture shown, where
/// `rects` of the screen are. True if the card did, which it does for
/// a back buffer of the screen's format, copied without a stretch: a
/// host renderer then copies its own pixels, and nothing of the back
/// buffer is read back for the processor to copy.
pub fn present(cpu: &mut Cpu, back: &Surface, from: &Rect, to: &Rect, rects: &[Rect]) -> bool {
    if !screen::is_toadforce(cpu) {
        return false;
    }
    let (width, height, bpp, pitch) = display::current(cpu);
    let masks = match bpp {
        16 => (0xF800, 0x07E0, 0x001F),
        32 => (0xFF_0000, 0xFF00, 0xFF),
        _ => return false,
    };
    let f = &back.format;
    if (f.bits, f.r, f.g, f.b) != (bpp, masks.0, masks.1, masks.2)
        || from.width() != to.width()
        || from.height() != to.height()
    {
        return false;
    }
    let Some(format) = card_format(bpp / 8, false) else {
        return false;
    };
    let Some(source) = card_surface(cpu, back, format) else {
        return false;
    };
    // The picture shown starts card memory.
    let words = [0, pitch, width, height, format.code()];
    let Ok(shown) = format::Surface::parse(&words, caps::BLIT_FORMATS, screen::memory_size(cpu))
    else {
        return false;
    };
    let (dx, dy) = (from.left - to.left, from.top - to.top);
    let mut commands = Vec::new();
    for r in rects {
        let s = Rect::new(r.left + dx, r.top + dy, r.right + dx, r.bottom + dy);
        if s.left < 0 || s.top < 0 || !source.holds(&card_rect(&s)) || !shown.holds(&card_rect(r)) {
            return false;
        }
        encode::blt(&mut commands, &shown, &card_rect(r), &source, &card_rect(&s), None);
    }
    cpu.bus.toadforce_submit(&commands);
    true
}

/// The rectangle `from` of the surface `src`, which is in system
/// memory, written to a block of card memory taken for it: the block
/// as a surface of `format`, its rows one after another. None when
/// card memory has no room, and the processor then copies. The caller
/// gives the block back (`display::free`) once the copy is queued.
fn stage(cpu: &mut Cpu, src: &Surface, from: &Rect, format: Format) -> Option<format::Surface> {
    let bytes = src.bytes_per_pixel();
    let (width, height) = (from.width() as u32, from.height() as u32);
    if width == 0 || height == 0 {
        return None;
    }
    let pitch = width * bytes;
    let start = display::alloc(cpu, pitch * height).ok()?;
    let words = [start, pitch, width, height, format.code()];
    let Ok(staged) = format::Surface::parse(&words, caps::BLIT_FORMATS, screen::memory_size(cpu))
    else {
        display::free(cpu, start);
        return None;
    };
    let mut data = vec![0u8; (pitch * height) as usize];
    let at = from.top as u32 * src.pitch + from.left as u32 * bytes;
    surface::read_rows(cpu, src, at, pitch as usize, &mut data);
    screen::write(cpu, start, &data);
    Some(staged)
}

/// Hand the card the commands, and count what they drew of the picture
/// shown as the program's (`screen::drew_rects`): the card wrote its
/// memory itself, past `screen::write`.
fn submit(cpu: &mut Cpu, blit: &Blit, words: &[u32]) {
    cpu.bus.toadforce_submit(words);
    let d = blit.dst;
    screen::drew_rects(cpu, d.bits, d.pitch, d.bytes_per_pixel(), blit.rects);
}
