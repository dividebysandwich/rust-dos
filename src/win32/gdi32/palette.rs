//! Logical palettes. The screen is 32 bits a pixel, so a palette only
//! says what PALETTEINDEX colors and DIB_PAL_COLORS tables are: realizing
//! one changes nothing on the screen.

use super::{
    DEFAULT_PALETTE, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, Kind, Palette, TRUE,
    add, dc, gdi, init, set_error, static_colors,
};
use crate::cpu::Cpu;
use crate::win32::user32::{read_words, write_words};

const PC_RESERVED: u8 = 1;
const SYSPAL_STATIC: u32 = 1;
const CLR_INVALID: u32 = 0xFFFF_FFFF;

fn entry_of(w: u32) -> [u8; 4] {
    w.to_le_bytes()
}

fn word_of(e: [u8; 4]) -> u32 {
    u32::from_le_bytes(e)
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
            Some(TRUE)
        }
        _ => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            Some(FALSE)
        }
    }
}

/// AnimatePalette(hpal, start, count, entries): only the entries that
/// are PC_RESERVED change.
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
    if let Some(super::Object {
        kind: Kind::Palette { palette },
        ..
    }) = gdi(cpu).get_mut(handle)
    {
        for (i, w) in words.iter().enumerate() {
            let e = &mut palette.entries[start + i];
            if e[3] & PC_RESERVED != 0 {
                *e = entry_of(*w);
            }
        }
    }
    Some(TRUE)
}

/// SelectPalette(hdc, hpal, background): the palette it had.
pub fn select_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, handle) = (args[0], args[1]);
    if gdi(cpu).palette(handle).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    }
    let old = dc::with(cpu, hdc, |d| {
        std::mem::replace(&mut d.attrs.palette, handle)
    });
    match old {
        Some(old) => {
            dc::free_if_deleted(cpu, old);
            Some(old)
        }
        None => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            Some(0)
        }
    }
}

/// RealizePalette(hdc): how many entries of its palette went into the
/// system palette. On a screen without a palette, none.
pub fn realize_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if gdi(cpu).dc(args[0]).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0xFFFF_FFFF);
    }
    Some(0)
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
pub fn get_system_palette_use(_cpu: &mut Cpu, _args: &[u32]) -> Option<u32> {
    Some(SYSPAL_STATIC)
}

/// SetSystemPaletteUse(hdc, use): the use it had. A screen without a
/// palette keeps its static colors.
pub fn set_system_palette_use(_cpu: &mut Cpu, _args: &[u32]) -> Option<u32> {
    Some(SYSPAL_STATIC)
}

/// GetNearestPaletteIndex(hpal, color).
pub fn get_nearest_palette_index(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(p) = gdi(cpu).palette(args[0]).cloned() else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(CLR_INVALID);
    };
    let colors: Vec<u32> = p
        .entries
        .iter()
        .map(|e| e[0] as u32 | (e[1] as u32) << 8 | (e[2] as u32) << 16)
        .collect();
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
}
