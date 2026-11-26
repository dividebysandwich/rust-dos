//! IDirectDrawPalette: a palette of 2, 4, 16 or 256 entries. Set on the
//! primary surface of 8 bits, its entries are the screen's colors, and
//! SetEntries changes them at once: all of the DAC's in exclusive mode,
//! and below it the system palette's entries GDI maps them to (`show`).

use super::com::CLASS_E_NOAGGREGATION;
use super::{
    DD_OK, DDERR_ALREADYINITIALIZED, DDERR_INVALIDOBJECT, DDERR_INVALIDPARAMS,
    DDERR_NOCOOPERATIVELEVELSET, Object, V_PALETTE, state,
};
use crate::cpu::Cpu;
use crate::win32::gdi32::palette;

const DDPCAPS_4BIT: u32 = 0x1;
const DDPCAPS_8BITENTRIES: u32 = 0x2;
const DDPCAPS_8BIT: u32 = 0x4;
const DDPCAPS_INITIALIZE: u32 = 0x8;
const DDPCAPS_PRIMARYSURFACE: u32 = 0x10;
const DDPCAPS_PRIMARYSURFACELEFT: u32 = 0x20;
const DDPCAPS_ALLOW256: u32 = 0x40;
const DDPCAPS_VSYNC: u32 = 0x80;
const DDPCAPS_1BIT: u32 = 0x100;
const DDPCAPS_2BIT: u32 = 0x200;
const DDPCAPS_ALPHA: u32 = 0x400;

#[derive(Clone, Debug, Default)]
pub struct Palette {
    /// CreatePalette's flags.
    pub caps: u32,
    /// The entries: red, green, blue and flags each, or with
    /// DDPCAPS_8BITENTRIES a byte each.
    pub entries: Vec<u8>,
}

impl Palette {
    /// How many entries it has.
    fn len(&self) -> u32 {
        size_of(self.caps).unwrap_or(0)
    }

    /// Bytes an entry.
    fn entry_size(&self) -> u32 {
        if self.caps & DDPCAPS_8BITENTRIES != 0 {
            1
        } else {
            4
        }
    }

    /// Its entries as COLORREFs; None for one of indexes into another
    /// palette (DDPCAPS_8BITENTRIES).
    pub fn colorrefs(&self) -> Option<Vec<u32>> {
        if self.entry_size() != 4 {
            return None;
        }
        Some(
            self.entries
                .chunks(4)
                .map(|e| e[0] as u32 | (e[1] as u32) << 8 | (e[2] as u32) << 16)
                .collect(),
        )
    }
}

/// The entries of a palette with the flags `caps`, if they say one size.
fn size_of(caps: u32) -> Option<u32> {
    let sizes = [
        (DDPCAPS_1BIT, 2),
        (DDPCAPS_2BIT, 4),
        (DDPCAPS_4BIT, 16),
        (DDPCAPS_8BIT, 256),
    ];
    let mut found = sizes.iter().filter(|(f, _)| caps & f != 0);
    let (_, n) = found.next()?;
    if found.next().is_some() {
        return None;
    }
    Some(*n)
}

fn palette_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    let (vtable, id) = super::this(cpu, this).ok_or(DDERR_INVALIDOBJECT)?;
    if vtable != V_PALETTE {
        return Err(DDERR_INVALIDOBJECT);
    }
    Ok(id)
}

/// CreatePalette(this, flags, entries, out, outer).
pub fn create_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, caps, entries, out, outer] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, dd) = hr!(super::directdraw::ddraw_of(cpu, this));
    if outer != 0 {
        return Some(CLASS_E_NOAGGREGATION);
    }
    if super::directdraw::ddraw(cpu, dd).is_none_or(|d| d.level == 0) {
        return Some(DDERR_NOCOOPERATIVELEVELSET);
    }
    let known = DDPCAPS_4BIT
        | DDPCAPS_8BITENTRIES
        | DDPCAPS_8BIT
        | DDPCAPS_INITIALIZE
        | DDPCAPS_PRIMARYSURFACE
        | DDPCAPS_PRIMARYSURFACELEFT
        | DDPCAPS_ALLOW256
        | DDPCAPS_VSYNC
        | DDPCAPS_1BIT
        | DDPCAPS_2BIT
        | DDPCAPS_ALPHA;
    let Some(len) = size_of(caps) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if caps & !known != 0 || caps & DDPCAPS_8BITENTRIES != 0 && len == 256 {
        return Some(DDERR_INVALIDPARAMS);
    }
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let entry = if caps & DDPCAPS_8BITENTRIES != 0 {
        1
    } else {
        4
    };
    let bytes = (len * entry) as usize;
    if entries == 0 || !cpu.bus.guest_probe(entries, bytes, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let mut data = vec![0u8; bytes];
    cpu.bus.guest_read_bytes(entries, &mut data);
    if len == 256 && caps & DDPCAPS_ALLOW256 == 0 {
        system_colors(&mut data);
    }
    let creator = super::iface(cpu, dd, vtable);
    let id = super::add_object(
        cpu,
        Object {
            owner: dd,
            creator,
            palette: Some(Palette {
                caps,
                entries: data,
            }),
            ..Object::default()
        },
    );
    let hr = super::give(cpu, id, V_PALETTE, out);
    if hr != DD_OK {
        super::destroy(cpu, id);
        return Some(hr);
    }
    if super::version(vtable) >= 4 {
        super::add_ref(cpu, this);
        if let Some(o) = state(cpu).object_mut(id) {
            o.holds_creator = true;
        }
    }
    Some(DD_OK)
}

/// The first and last entries of an 8-bit palette without
/// DDPCAPS_ALLOW256 are the system's black and white.
fn system_colors(entries: &mut [u8]) {
    entries[..4].copy_from_slice(&[0, 0, 0, 0]);
    entries[1020..].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0]);
}

/// The palette `id` goes.
pub fn destroyed(_: &mut Cpu, _: u32) {}

/// GetCaps(this, out).
pub fn get_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(palette_of(cpu, args[0]));
    let caps = state(cpu)
        .object(id)
        .and_then(|o| o.palette.as_ref())
        .map_or(0, |p| p.caps);
    if !super::com::write_out(cpu, args[1], caps) {
        return Some(DDERR_INVALIDPARAMS);
    }
    Some(DD_OK)
}

/// Initialize(this, ddraw, flags, entries).
pub fn initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(palette_of(cpu, args[0]));
    Some(DDERR_ALREADYINITIALIZED)
}

/// The range of bytes `start` and `count` entries are, if they are in
/// the palette.
fn range(p: &Palette, start: u32, count: u32) -> Option<std::ops::Range<usize>> {
    let end = start.checked_add(count)?;
    if end > p.len() {
        return None;
    }
    let size = p.entry_size() as usize;
    Some(start as usize * size..end as usize * size)
}

/// GetEntries(this, flags, start, count, out).
pub fn get_entries(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, flags, start, count, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(palette_of(cpu, this));
    let Some(p) = state(cpu).object(id).and_then(|o| o.palette.clone()) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if flags != 0 {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(range) = range(&p, start, count) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if out == 0 || !cpu.bus.guest_probe(out, range.len(), true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    cpu.bus.guest_write_bytes(out, &p.entries[range]);
    Some(DD_OK)
}

/// SetEntries(this, flags, start, count, entries).
pub fn set_entries(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, flags, start, count, entries] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(palette_of(cpu, this));
    let Some(p) = state(cpu).object(id).and_then(|o| o.palette.clone()) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if flags != 0 {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(range) = range(&p, start, count) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if entries == 0 || !cpu.bus.guest_probe(entries, range.len(), false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let mut data = vec![0u8; range.len()];
    cpu.bus.guest_read_bytes(entries, &mut data);
    if let Some(p) = state(cpu).object_mut(id).and_then(|o| o.palette.as_mut()) {
        p.entries[range].copy_from_slice(&data);
        if p.caps & DDPCAPS_8BIT != 0 && p.caps & DDPCAPS_ALLOW256 == 0 {
            system_colors(&mut p.entries);
        }
    }
    // On the primary surface, the screen's colors change.
    let iface = state(cpu)
        .object(id)
        .map(|o| o.ifaces.iter().map(|i| i.address).collect::<Vec<_>>())
        .unwrap_or_default();
    let shown = state(cpu).objects.iter().flatten().any(|o| {
        o.surface
            .as_ref()
            .is_some_and(|s| s.is_primary() && iface.contains(&s.palette))
    });
    if shown {
        show(cpu, this);
    }
    Some(DD_OK)
}

/// The palette at the interface `pal` as COLORREFs, for GDI.
pub fn colors(cpu: &mut Cpu, pal: u32) -> Option<Vec<u32>> {
    if pal == 0 {
        return None;
    }
    let (_, id) = super::this(cpu, pal)?;
    state(cpu).object(id)?.palette.as_ref()?.colorrefs()
}

/// The entries of the palette at the interface `pal` with their flags;
/// None for one of indexes into another palette (DDPCAPS_8BITENTRIES).
fn entries(cpu: &mut Cpu, pal: u32) -> Option<Vec<[u8; 4]>> {
    let (_, id) = super::this(cpu, pal)?;
    let p = state(cpu).object(id)?.palette.as_ref()?;
    if p.entry_size() != 4 {
        return None;
    }
    Some(
        p.entries
            .chunks(4)
            .map(|e| [e[0], e[1], e[2], e[3]])
            .collect(),
    )
}

/// Show the palette at the interface `pal`, the primary surface's. In
/// exclusive mode, in an 8-bit mode DirectDraw set or on a desktop of
/// 256 colours, all of it goes in the DAC, and GDI's next RealizePalette
/// puts the system palette back even for the palette it realized last.
/// Below exclusive mode on a palette device (a desktop of 256 colours,
/// or a mode kept after exclusive mode ended) it shares the screen with
/// GDI, which realizes its entries in the foreground
/// (`gdi32::palette::realize_entries`).
pub fn show(cpu: &mut Cpu, pal: u32) {
    let owner = super::this(cpu, pal)
        .and_then(|(_, id)| state(cpu).object(id))
        .map_or(0, |o| o.owner);
    let exclusive = owner != 0 && state(cpu).display.exclusive == owner;
    if !exclusive {
        if let Some(entries) = entries(cpu, pal) {
            palette::realize_entries(cpu, &entries);
        }
        return;
    }
    let Some(colors) = colors(cpu, pal) else {
        return;
    };
    if super::display::owns_display(cpu) || palette::palette_device(cpu) {
        crate::win32::screen::set_colors(cpu, 0, &colors);
        crate::win32::gdi32::gdi(cpu).last_realized = 0;
    }
}

crate::state_fields!(Palette { caps, entries });
