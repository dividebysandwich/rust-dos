//! Logical palettes and the system palette.
//!
//! On a desktop of 32, 24 or 16 bits a pixel a palette only says what
//! PALETTEINDEX colors and DIB_PAL_COLORS tables are: realizing one
//! changes nothing on the screen.
//!
//! On a desktop of 256 colours (`win32_depth=8`) the screen's pixels are
//! indices into the system palette, which the DAC holds, as with a
//! palette display driver of Windows 3.1 and 95. Its 20 static colors
//! are at its ends (2 with SYSPAL_NOSTATIC), and realizing a logical
//! palette maps each of its entries to an entry of the system palette:
//!
//! - In the foreground (the palette USER's SelectPalette made the
//!   primary one, selected into a DC of the foreground window without
//!   bForceBackground), every entry but the static ones is freed first.
//!   In the background, only the entries the same palette took last
//!   time that no other palette maps to.
//! - An entry with PC_EXPLICIT is the system palette's entry its low
//!   word names.
//! - One with PC_RESERVED takes a free entry of its own, which no other
//!   palette's entry maps to and AnimatePalette changes.
//! - One with PC_NOCOLLAPSE takes a free entry.
//! - The others take an entry of the same color if there is one, else a
//!   free entry.
//! - When no entry is free, the nearest color of those taken.
//!
//! RealizePalette returns how many entries' mappings changed, and when
//! the primary palette changed any in a window's DC, USER sends
//! WM_PALETTECHANGED to the top-level windows (as Wine's
//! UserRealizePalette does). Realizing the palette realized last again
//! does nothing until its entries change.

use super::{
    DEFAULT_PALETTE, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, Kind, Palette, TRUE,
    add, dc, gdi, init, set_error, static_colors,
};
use crate::cpu::Cpu;
use crate::win32::user32::{read_words, write_words};
use crate::win32::version::palette_desktop;

pub const PC_RESERVED: u8 = 1;
pub const PC_EXPLICIT: u8 = 2;
pub const PC_NOCOLLAPSE: u8 = 4;
pub const SYSPAL_ERROR: u32 = 0;
pub const SYSPAL_STATIC: u32 = 1;
pub const SYSPAL_NOSTATIC: u32 = 2;
const CLR_INVALID: u32 = 0xFFFF_FFFF;

/// What an entry of the system palette is.
pub const USE_FREE: u8 = 0;
pub const USE_STATIC: u8 = 1;
/// A logical palette's color, which others of the same color share.
pub const USE_SHARED: u8 = 2;
/// A PC_RESERVED entry's, which is its palette's alone.
pub const USE_RESERVED: u8 = 3;

fn entry_of(w: u32) -> [u8; 4] {
    w.to_le_bytes()
}

fn word_of(e: [u8; 4]) -> u32 {
    u32::from_le_bytes(e)
}

/// An entry's color as a COLORREF.
fn rgb(e: [u8; 4]) -> u32 {
    e[0] as u32 | (e[1] as u32) << 8 | (e[2] as u32) << 16
}

/// Whether the system palette's entry `i` is a static color under a
/// SetSystemPaletteUse setting: 20 are with SYSPAL_STATIC, black and
/// white with SYSPAL_NOSTATIC.
fn is_static(static_use: u32, i: usize) -> bool {
    if static_use == SYSPAL_NOSTATIC {
        i == 0 || i == 255
    } else {
        !(10..246).contains(&i)
    }
}

/// What the system palette's entries are with nothing realized.
pub fn static_uses(static_use: u32) -> Vec<u8> {
    (0..256)
        .map(|i| {
            if is_static(static_use, i) {
                USE_STATIC
            } else {
                USE_FREE
            }
        })
        .collect()
}

/// The system palette's colors, as COLORREFs: the colors of a desktop of
/// 256 colours.
pub fn system_colors(cpu: &mut Cpu) -> Vec<u32> {
    init(cpu);
    gdi(cpu).system_palette.iter().map(|&e| rgb(e)).collect()
}

/// Put the system palette in the DAC, on a desktop of 256 colours that
/// shows (not while DirectDraw has the display).
pub fn show_system_palette(cpu: &mut Cpu) {
    show_system_entries(cpu, 0, 255);
}

/// Put the system palette's entries `first..=last` in the DAC, as
/// `show_system_palette` does.
fn show_system_entries(cpu: &mut Cpu, first: usize, last: usize) {
    if !palette_desktop(cpu) || crate::win32::ddraw::owns_display(cpu) {
        return;
    }
    let colors = system_colors(cpu);
    crate::win32::screen::set_colors(cpu, first, &colors[first..=last.min(255)]);
}

/// How colors become the pixels of a palette device (`Surface::device`)
/// in a DC: through its logical palette's entries, mapped to the system
/// palette, and the static colors.
#[derive(Clone, Debug)]
pub struct DeviceMap {
    /// The system palette's index of each of the logical palette's
    /// entries, and their colors.
    pub map: Vec<u8>,
    pub logical: Vec<u32>,
    /// The static colors, and their indices.
    pub statics: Vec<(u32, u8)>,
}

impl DeviceMap {
    /// The map of the logical palette `hpal`: as it was realized, or for
    /// one that wasn't, its colors' nearest in the system palette.
    pub fn new(cpu: &mut Cpu, hpal: u32) -> DeviceMap {
        init(cpu);
        let state = gdi(cpu);
        let system = &state.system_palette;
        let uses = &state.system_use;
        let statics: Vec<(u32, u8)> = (0..256)
            .filter(|&i| uses.get(i) == Some(&USE_STATIC))
            .map(|i| (rgb(system[i]), i as u8))
            .collect();
        let (logical, mut map) = match state.palette(hpal) {
            Some(p) => (
                p.entries.iter().map(|&e| rgb(e)).collect::<Vec<u32>>(),
                p.mapping.clone(),
            ),
            None => (Vec::new(), Vec::new()),
        };
        if map.len() != logical.len() {
            let taken: Vec<(u32, u8)> = (0..256)
                .filter(|&i| uses.get(i).is_some_and(|&u| u != USE_FREE))
                .map(|i| (rgb(system[i]), i as u8))
                .collect();
            map = logical.iter().map(|&c| nearest_of(&taken, c)).collect();
        }
        DeviceMap {
            map,
            logical,
            statics,
        }
    }

    /// PALETTEINDEX(`i`): the logical palette's entry, wrapping around
    /// its end.
    pub fn index(&self, i: usize) -> u32 {
        match self.map.len() {
            0 => 0,
            n => self.map[i % n] as u32,
        }
    }

    /// An RGB color, as pens, brushes and text draw it: the nearest
    /// static color.
    pub fn solid(&self, color: u32) -> u32 {
        nearest_of(&self.statics, color) as u32
    }

    /// PALETTERGB: the logical palette's nearest entry.
    pub fn palette_rgb(&self, color: u32) -> u32 {
        if self.logical.is_empty() {
            return self.solid(color);
        }
        self.index(super::surface::nearest(&self.logical, color) as usize)
    }

    /// A DIB's or bitmap's color `color` in a blit: the nearest of the
    /// logical palette's colors and the static ones. `hint` is the
    /// pixel's index in its color table: where the logical palette has
    /// the same color at the same index, that entry, so a DIB whose
    /// table is the palette keeps its indices even where colors repeat.
    pub fn blit_color(&self, color: u32, hint: Option<u32>) -> u32 {
        if let Some(i) = hint
            && self.logical.get(i as usize) == Some(&color)
        {
            return self.map[i as usize] as u32;
        }
        let mut best = (u32::MAX, 0);
        let candidates = self
            .logical
            .iter()
            .zip(&self.map)
            .map(|(&c, &i)| (c, i))
            .chain(self.statics.iter().copied());
        for (c, i) in candidates {
            let d = distance(c, color);
            if d < best.0 {
                best = (d, i as u32);
                if d == 0 {
                    break;
                }
            }
        }
        best.1
    }
}

fn distance(a: u32, b: u32) -> u32 {
    let d = |s: u32| ((a >> s) & 0xFF).abs_diff((b >> s) & 0xFF);
    let (r, g, b) = (d(0), d(8), d(16));
    r * r + g * g + b * b
}

/// The index of the nearest color of `table` (color, index) to `color`.
fn nearest_of(table: &[(u32, u8)], color: u32) -> u8 {
    let mut best = (u32::MAX, 0);
    for &(c, i) in table {
        let d = distance(c, color);
        if d < best.0 {
            best = (d, i);
            if d == 0 {
                break;
            }
        }
    }
    best.1
}

/// The halftone palette's colors: the static colors at its ends, a
/// 6x6x6 color cube and grays between.
pub fn halftone_colors() -> Vec<u32> {
    let statics = static_colors();
    let mut colors = vec![0u32; 256];
    colors[..10].copy_from_slice(&statics[..10]);
    colors[246..].copy_from_slice(&statics[10..]);
    for r in 0..6u32 {
        for g in 0..6u32 {
            for b in 0..6u32 {
                let i = (r + g * 6 + b * 36 + 10) as usize;
                colors[i] = (r * 51) | ((g * 51) << 8) | ((b * 51) << 16);
            }
        }
    }
    for i in 226..246u32 {
        let v = ((i - 226) * 255 / 19) & 0xFF;
        colors[i as usize] = v | v << 8 | v << 16;
    }
    colors
}

/// CreatePalette(logpalette).
pub fn create_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if at == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let head = cpu.bus.guest_read_32(at);
    let count = (head >> 16) as usize;
    let entries = read_words(cpu, at + 4, count);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let palette = Palette {
        entries: entries.into_iter().map(entry_of).collect(),
        mapping: Vec::new(),
    };
    Some(add(cpu, Kind::Palette { palette }))
}

/// CreateHalftonePalette(hdc).
pub fn create_halftone_palette(cpu: &mut Cpu, _args: &[u32]) -> Option<u32> {
    let palette = Palette {
        entries: halftone_colors()
            .into_iter()
            .map(|c| [c as u8, (c >> 8) as u8, (c >> 16) as u8, 0])
            .collect(),
        mapping: Vec::new(),
    };
    Some(add(cpu, Kind::Palette { palette }))
}

/// GetPaletteEntries(hpal, start, count, entries): how many it copied,
/// or with no `entries` how many the palette has.
pub fn get_palette_entries(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (handle, start, count, out) = (args[0], args[1] as usize, args[2] as usize, args[3]);
    let Some(p) = gdi(cpu).palette(handle).cloned() else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    let n = p.entries.len();
    if out == 0 {
        return Some(n as u32);
    }
    if start >= n {
        return Some(0);
    }
    let end = (start + count).min(n);
    let words: Vec<u32> = p.entries[start..end].iter().map(|&e| word_of(e)).collect();
    write_words(cpu, out, &words);
    Some(words.len() as u32)
}

/// The palette `handle` changed or went: realizing it maps it again.
pub fn unrealize(cpu: &mut Cpu, handle: u32) {
    let state = gdi(cpu);
    if state.last_realized == handle {
        state.last_realized = 0;
    }
}

/// SetPaletteEntries(hpal, start, count, entries).
pub fn set_palette_entries(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (handle, start, count, at) = (args[0], args[1] as usize, args[2] as usize, args[3]);
    init(cpu);
    if handle == gdi(cpu).stock(DEFAULT_PALETTE) {
        return Some(0);
    }
    let Some(n) = gdi(cpu).palette(handle).map(|p| p.entries.len()) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    if start >= n {
        return Some(0);
    }
    let end = (start + count).min(n);
    let words = read_words(cpu, at, end - start);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if let Some(super::Object {
        kind: Kind::Palette { palette },
        ..
    }) = gdi(cpu).get_mut(handle)
    {
        for (i, w) in words.iter().enumerate() {
            palette.entries[start + i] = entry_of(*w);
        }
    }
    unrealize(cpu, handle);
    Some(words.len() as u32)
}

/// ResizePalette(hpal, entries).
pub fn resize_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let n = args[1] as usize;
    if n == 0 || n > 0x1000 {
        return Some(FALSE);
    }
    match gdi(cpu).get_mut(args[0]) {
        Some(super::Object {
            kind: Kind::Palette { palette },
            ..
        }) => {
            palette.entries.resize(n, [0; 4]);
            palette.mapping.clear();
            unrealize(cpu, args[0]);
            Some(TRUE)
        }
        _ => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            Some(FALSE)
        }
    }
}

/// A palette is deleted: it is no longer the primary one or the one
/// realized last.
pub fn palette_deleted(cpu: &mut Cpu, handle: u32) {
    let state = gdi(cpu);
    if state.primary_palette == handle {
        state.primary_palette = 0;
    }
    // Its entries stay taken until the next foreground realization, but
    // no longer as its own.
    for o in state.system_owner.iter_mut().filter(|o| **o == handle) {
        *o = 0;
    }
    unrealize(cpu, handle);
}

/// AnimatePalette(hpal, start, count, entries): only the entries that
/// are PC_RESERVED change. On a desktop of 256 colours, the entries of
/// the system palette they took change with them, and the screen at
/// once.
pub fn animate_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (handle, start, count, at) = (args[0], args[1] as usize, args[2] as usize, args[3]);
    let Some(n) = gdi(cpu).palette(handle).map(|p| p.entries.len()) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    if start >= n {
        return Some(FALSE);
    }
    let end = (start + count).min(n);
    let words = read_words(cpu, at, end - start);
    // The system palette's entries that changed, lowest and highest.
    let mut changed: Option<(usize, usize)> = None;
    let state = gdi(cpu);
    let mut system = std::mem::take(&mut state.system_palette);
    let uses = std::mem::take(&mut state.system_use);
    let owners = std::mem::take(&mut state.system_owner);
    if let Some(super::Object {
        kind: Kind::Palette { palette },
        ..
    }) = state.get_mut(handle)
    {
        for (i, w) in words.iter().enumerate() {
            let e = &mut palette.entries[start + i];
            if e[3] & PC_RESERVED == 0 {
                continue;
            }
            *e = entry_of(*w);
            if let Some(&m) = palette.mapping.get(start + i)
                && uses.get(m as usize) == Some(&USE_RESERVED)
                && owners.get(m as usize) == Some(&handle)
            {
                let m = m as usize;
                system[m][..3].copy_from_slice(&e[..3]);
                changed = Some(changed.map_or((m, m), |(lo, hi)| (lo.min(m), hi.max(m))));
            }
        }
    }
    state.system_palette = system;
    state.system_use = uses;
    state.system_owner = owners;
    if let Some((first, last)) = changed {
        show_system_entries(cpu, first, last);
    }
    Some(TRUE)
}

/// SelectPalette(hdc, hpal, background): the palette it had. As USER's
/// SelectPalette, it makes the palette the primary one when it isn't
/// selected as a background palette into a DC of the foreground window
/// or one of its children.
pub fn select_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, handle, background) = (args[0], args[1], args[2] & 0xFFFF != 0);
    if gdi(cpu).palette(handle).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    }
    let old = dc::with(cpu, hdc, |d| {
        std::mem::replace(&mut d.attrs.palette, handle)
    });
    let Some(old) = old else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    let hwnd = gdi(cpu).dc(hdc).map_or(0, |d| d.hwnd);
    if !background && handle != gdi(cpu).stock(DEFAULT_PALETTE) && hwnd != 0 {
        let user = &cpu.bus.win32.user;
        let foreground = user.input.foreground;
        if foreground != 0
            && (foreground == hwnd || user.is_descendant(foreground, hwnd))
            && gdi(cpu).primary_palette != handle
        {
            // A palette realized last in the background is realized
            // again, in the foreground.
            gdi(cpu).primary_palette = handle;
            unrealize(cpu, handle);
        }
    }
    dc::free_if_deleted(cpu, old);
    Some(old)
}

/// Realize the palette of the DC `hdc` on a desktop of 256 colours: how
/// many of its entries' mappings changed, and whether it is the primary
/// palette.
fn realize(cpu: &mut Cpu, hdc: u32) -> (u32, bool) {
    let hpal = gdi(cpu).dc(hdc).map_or(0, |d| d.attrs.palette);
    let state = gdi(cpu);
    if hpal == state.stock(DEFAULT_PALETTE) {
        return (0, false);
    }
    let Some(palette) = state.palette(hpal).cloned() else {
        return (0, false);
    };
    if state.last_realized == hpal && palette.mapping.len() == palette.entries.len() {
        return (0, false);
    }
    state.last_realized = hpal;
    let primary = hpal == state.primary_palette;
    // How many entries of the other palettes' mappings point at each
    // entry of the system palette.
    let mut shared = [0u32; 256];
    for o in state.objects.iter().flatten() {
        if let Kind::Palette { palette } = &o.kind {
            for &m in &palette.mapping {
                shared[m as usize] += 1;
            }
        }
    }
    for &m in &palette.mapping {
        shared[m as usize] -= 1;
    }
    let mut uses = std::mem::take(&mut state.system_use);
    let mut owners = std::mem::take(&mut state.system_owner);
    owners.resize(256, 0);
    let mut system = std::mem::take(&mut state.system_palette);
    let before = system.clone();
    for i in 0..256 {
        // In the foreground every entry but the static ones is freed; in
        // the background, those the palette's last realization took
        // that no other palette maps to.
        let free = if primary {
            uses[i] != USE_STATIC
        } else {
            owners[i] == hpal && shared[i] == 0
        };
        if free {
            uses[i] = USE_FREE;
            owners[i] = 0;
        }
    }
    // Free entries are taken from the bottom up.
    let mut next_free = 0usize;
    let mut take = |uses: &mut [u8], system: &mut [[u8; 4]], e: [u8; 4], how: u8| {
        while next_free < 256 && uses[next_free] != USE_FREE {
            next_free += 1;
        }
        if next_free == 256 {
            return None;
        }
        uses[next_free] = how;
        owners[next_free] = hpal;
        system[next_free] = [e[0], e[1], e[2], 0];
        Some(next_free as u8)
    };
    let nearest_taken = |uses: &[u8], system: &[[u8; 4]], color: u32| {
        let taken: Vec<(u32, u8)> = (0..256)
            .filter(|&i| uses[i] == USE_STATIC || uses[i] == USE_SHARED)
            .map(|i| (rgb(system[i]), i as u8))
            .collect();
        nearest_of(&taken, color)
    };
    let mut mapping = Vec::with_capacity(palette.entries.len());
    for &e in &palette.entries {
        let color = rgb(e);
        let index = if e[3] & PC_EXPLICIT != 0 {
            e[0]
        } else if e[3] & (PC_RESERVED | PC_NOCOLLAPSE) != 0 {
            let how = if e[3] & PC_RESERVED != 0 {
                USE_RESERVED
            } else {
                USE_SHARED
            };
            take(&mut uses, &mut system, e, how)
                .unwrap_or_else(|| nearest_taken(&uses, &system, color))
        } else {
            let same = (0..256).find(|&i| {
                (uses[i] == USE_STATIC || uses[i] == USE_SHARED) && rgb(system[i]) == color
            });
            match same {
                Some(i) => i as u8,
                None => take(&mut uses, &mut system, e, USE_SHARED)
                    .unwrap_or_else(|| nearest_taken(&uses, &system, color)),
            }
        };
        mapping.push(index);
    }
    let changed = mapping
        .iter()
        .enumerate()
        .filter(|&(i, m)| palette.mapping.get(i) != Some(m))
        .count() as u32;
    let state = gdi(cpu);
    state.system_use = uses;
    state.system_owner = owners;
    let differs = |i: &usize| system[*i] != before[*i];
    let shown = (0..256).find(differs).zip((0..256).rev().find(differs));
    state.system_palette = system;
    if let Some(super::Object {
        kind: Kind::Palette { palette },
        ..
    }) = state.get_mut(hpal)
    {
        palette.mapping = mapping;
    }
    if let Some((first, last)) = shown {
        show_system_entries(cpu, first, last);
    }
    (changed, primary)
}

/// RealizePalette(hdc): how many entries of its palette went into the
/// system palette. On a screen without a palette, none. When the primary
/// palette changed mappings in a window's DC, the top-level windows get
/// WM_PALETTECHANGED.
pub fn realize_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hdc = args[0];
    let Some(hwnd) = gdi(cpu).dc(hdc).map(|d| d.hwnd) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0xFFFF_FFFF);
    };
    if !palette_desktop(cpu) {
        return Some(0);
    }
    let (changed, primary) = realize(cpu, hdc);
    if changed == 0 || !primary || hwnd == 0 {
        return Some(changed);
    }
    let vars = vec![super::CB_PALETTE_CHANGED, changed, hwnd, 0];
    crate::win32::task::start(cpu, super::TASK_CALLBACK, vars, 0)
}

/// GetSystemPaletteEntries(hdc, start, count, entries).
pub fn get_system_palette_entries(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (start, count, out) = (args[1] as usize, args[2] as usize, args[3]);
    init(cpu);
    let system = gdi(cpu).system_palette.clone();
    if out == 0 {
        return Some(system.len() as u32);
    }
    if start >= system.len() {
        return Some(0);
    }
    let end = (start + count).min(system.len());
    let words: Vec<u32> = system[start..end].iter().map(|&e| word_of(e)).collect();
    write_words(cpu, out, &words);
    Some(words.len() as u32)
}

/// GetSystemPaletteUse(hdc).
pub fn get_system_palette_use(cpu: &mut Cpu, _args: &[u32]) -> Option<u32> {
    init(cpu);
    if !palette_desktop(cpu) {
        return Some(SYSPAL_STATIC);
    }
    Some(gdi(cpu).static_use)
}

/// SetSystemPaletteUse(hdc, use): the use it had. A screen without a
/// palette keeps its static colors. On a desktop of 256 colours,
/// SYSPAL_NOSTATIC leaves black and white the only static colors, and
/// SYSPAL_STATIC puts the 20 back.
pub fn set_system_palette_use(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    init(cpu);
    if !palette_desktop(cpu) {
        return Some(SYSPAL_STATIC);
    }
    let new = args[1];
    if new != SYSPAL_STATIC && new != SYSPAL_NOSTATIC {
        return Some(SYSPAL_ERROR);
    }
    let state = gdi(cpu);
    let old = state.static_use;
    if old == new {
        return Some(old);
    }
    state.static_use = new;
    for i in 0..256 {
        let (was, now) = (is_static(old, i), is_static(new, i));
        if was && !now {
            state.system_use[i] = USE_FREE;
            state.system_owner[i] = 0;
        } else if now && !was {
            let c = static_colors()[if i < 10 { i } else { i - 236 }];
            state.system_palette[i] = entry_of(c);
            state.system_use[i] = USE_STATIC;
            state.system_owner[i] = 0;
        }
    }
    state.last_realized = 0;
    show_system_palette(cpu);
    Some(old)
}

/// GetNearestPaletteIndex(hpal, color).
pub fn get_nearest_palette_index(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(p) = gdi(cpu).palette(args[0]).cloned() else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(CLR_INVALID);
    };
    let colors: Vec<u32> = p.entries.iter().map(|&e| rgb(e)).collect();
    if colors.is_empty() {
        return Some(CLR_INVALID);
    }
    Some(super::surface::nearest(&colors, args[1] & 0xFF_FFFF))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halftone_palette_has_the_static_colors_and_the_cube() {
        let c = halftone_colors();
        assert_eq!(c.len(), 256);
        assert_eq!(c[0], 0);
        assert_eq!(c[255], 0xFFFFFF);
        // Red 5, green 0, blue 0.
        assert_eq!(c[15], 0x0000FF);
    }

    /// A machine with a desktop of 256 colours and a memory DC, which
    /// realizes its palette in the background.
    fn palette_machine() -> (Cpu, u32) {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.win32.desktop_depth = crate::config::DesktopDepth::Palette;
        let hdc = dc::create_compatible_dc(&mut cpu, &[0]).unwrap();
        (cpu, hdc)
    }

    /// A palette of PC_RESERVED entries of the colors `colors`.
    fn reserved_palette(cpu: &mut Cpu, colors: impl Iterator<Item = u32>) -> u32 {
        let entries = colors.map(|c| entry_of(c | (PC_RESERVED as u32) << 24));
        let palette = Palette {
            entries: entries.collect(),
            mapping: Vec::new(),
        };
        add(cpu, Kind::Palette { palette })
    }

    fn select(cpu: &mut Cpu, hdc: u32, hpal: u32) {
        dc::with(cpu, hdc, |d| d.attrs.palette = hpal);
    }

    /// A fade in the background (SetPaletteEntries, then RealizePalette)
    /// gives back the entries the palette took the last time, so every
    /// frame gets entries of its own.
    #[test]
    fn a_background_realization_reuses_its_own_entries() {
        let (mut cpu, hdc) = palette_machine();
        let hpal = reserved_palette(&mut cpu, (0..200).map(|i| i << 8));
        select(&mut cpu, hdc, hpal);
        for frame in 1..4u32 {
            if let Some(super::super::Object {
                kind: Kind::Palette { palette },
                ..
            }) = gdi(&mut cpu).get_mut(hpal)
            {
                for (i, e) in palette.entries.iter_mut().enumerate() {
                    e[2] = frame as u8;
                    e[1] = i as u8;
                }
            }
            unrealize(&mut cpu, hpal);
            let (_, primary) = realize(&mut cpu, hdc);
            assert!(!primary);
            let state = gdi(&mut cpu);
            let mapping = state.palette(hpal).unwrap().mapping.clone();
            for (i, &m) in mapping.iter().enumerate() {
                assert_eq!(state.system_use[m as usize], USE_RESERVED, "frame {frame}");
                assert_eq!(state.system_palette[m as usize][2], frame as u8);
                assert_eq!(state.system_palette[m as usize][1], i as u8);
            }
        }
    }

    /// AnimatePalette changes the system entries a palette took only
    /// while they are still its own.
    #[test]
    fn animate_palette_leaves_another_palettes_entries() {
        let (mut cpu, hdc) = palette_machine();
        let a = reserved_palette(&mut cpu, [0x010101, 0x020202].into_iter());
        let b = reserved_palette(&mut cpu, [0x030303, 0x040404].into_iter());
        select(&mut cpu, hdc, a);
        realize(&mut cpu, hdc);
        let taken = gdi(&mut cpu).palette(a).unwrap().mapping.clone();
        // B becomes primary: every entry is freed, and B takes A's.
        gdi(&mut cpu).primary_palette = b;
        select(&mut cpu, hdc, b);
        realize(&mut cpu, hdc);
        assert_eq!(gdi(&mut cpu).palette(b).unwrap().mapping, taken);
        let at = 0x10000;
        write_words(&mut cpu, at, &[0x0105_0505, 0x0106_0606]);
        assert_eq!(animate_palette(&mut cpu, &[a, 0, 2, at]), Some(TRUE));
        let colors = system_colors(&mut cpu);
        let shown: Vec<u32> = taken.iter().map(|&m| colors[m as usize]).collect();
        assert_eq!(shown, [0x030303, 0x040404]);
        assert_eq!(animate_palette(&mut cpu, &[b, 0, 2, at]), Some(TRUE));
        let colors = system_colors(&mut cpu);
        let shown: Vec<u32> = taken.iter().map(|&m| colors[m as usize]).collect();
        assert_eq!(shown, [0x050505, 0x060606]);
    }

    #[test]
    fn the_static_colors_are_at_the_ends() {
        let u = static_uses(SYSPAL_STATIC);
        assert_eq!(u.iter().filter(|&&x| x == USE_STATIC).count(), 20);
        assert_eq!(
            (u[9], u[10], u[245], u[246]),
            (USE_STATIC, USE_FREE, USE_FREE, USE_STATIC)
        );
        let u = static_uses(SYSPAL_NOSTATIC);
        assert_eq!(u.iter().filter(|&&x| x == USE_STATIC).count(), 2);
    }
}
