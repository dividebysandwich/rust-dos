//! IDirectDrawClipper: where blits to a surface may draw. A clipper is a
//! window's, which clips to what of the window shows on the screen, or
//! has a list of rectangles of its own.

use super::com::{self, CLASS_E_NOAGGREGATION, Guid};
use super::{
    DD_OK, DDERR_ALREADYINITIALIZED, DDERR_CLIPPERISUSINGHWND, DDERR_INVALIDCLIPLIST,
    DDERR_INVALIDOBJECT, DDERR_INVALIDPARAMS, DDERR_NOCLIPLIST, DDERR_NOTINITIALIZED,
    DDERR_REGIONTOOSMALL, Object, V_CLIPPER, state,
};
use crate::cpu::Cpu;
use crate::win32::gdi32::dc::{DCX_CLIPSIBLINGS, DCX_WINDOW};
use crate::win32::gdi32::{Rect, Region};

#[derive(Clone, Debug, Default)]
pub struct Clipper {
    /// Made by CoCreateInstance, it needs Initialize first.
    pub initialized: bool,
    pub hwnd: u32,
    /// SetClipList's rectangles, if it set any.
    pub list: Option<Vec<[i32; 4]>>,
}

/// RGNDATAHEADER's size, and RDH_RECTANGLES.
const HEADER: u32 = 32;
const RDH_RECTANGLES: u32 = 1;

fn clipper_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    let (vtable, id) = super::this(cpu, this).ok_or(DDERR_INVALIDOBJECT)?;
    if vtable != V_CLIPPER {
        return Err(DDERR_INVALIDOBJECT);
    }
    Ok(id)
}

fn clipper(cpu: &mut Cpu, id: u32) -> Option<&mut Clipper> {
    state(cpu).object_mut(id)?.clipper.as_mut()
}

/// A clipper that may be used: one that was initialized.
fn ready(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    let id = clipper_of(cpu, this)?;
    if !clipper(cpu, id).is_some_and(|c| c.initialized) {
        return Err(DDERR_NOTINITIALIZED);
    }
    Ok(id)
}

/// A new clipper of the DirectDraw object `owner` (0 for none), its
/// interface at `out`.
fn new_clipper(cpu: &mut Cpu, owner: u32, creator: u32, initialized: bool, out: u32) -> u32 {
    let id = super::add_object(
        cpu,
        Object {
            owner,
            creator,
            clipper: Some(Clipper {
                initialized,
                ..Clipper::default()
            }),
            ..Object::default()
        },
    );
    let hr = super::give(cpu, id, V_CLIPPER, out);
    if hr != DD_OK {
        super::destroy(cpu, id);
    }
    hr
}

/// DirectDrawCreateClipper(flags, out, outer).
pub fn direct_draw_create_clipper(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [_, out, outer] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if outer != 0 {
        return Some(CLASS_E_NOAGGREGATION);
    }
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    Some(new_clipper(cpu, 0, 0, true, out))
}

/// CreateClipper(this, flags, out, outer).
pub fn create_clipper(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, out, outer] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, dd) = hr!(super::ddraw::ddraw_of(cpu, this));
    if outer != 0 {
        return Some(CLASS_E_NOAGGREGATION);
    }
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let creator = super::iface(cpu, dd, vtable);
    Some(new_clipper(cpu, dd, creator, true, out))
}

/// CoCreateInstance's clipper: one that needs Initialize.
pub fn create_instance(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    if super::vtable_for(V_CLIPPER, iid).is_none() {
        com::write_out(cpu, out, 0);
        return com::E_NOINTERFACE;
    }
    new_clipper(cpu, 0, 0, false, out)
}

/// Initialize(this, ddraw, flags).
pub fn initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(clipper_of(cpu, this));
    if flags != 0 {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(c) = clipper(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if c.initialized {
        return Some(DDERR_ALREADYINITIALIZED);
    }
    c.initialized = true;
    Some(DD_OK)
}

/// The region the clipper at the interface `iface` clips to, if it has
/// one: its window's visible part on the screen, or its list.
pub fn region(cpu: &mut Cpu, iface: u32) -> Option<Region> {
    let (_, id) = super::this(cpu, iface)?;
    let c = state(cpu).object(id)?.clipper.clone()?;
    if c.hwnd != 0 {
        let user = &cpu.bus.win32.user;
        if !user.is_window(c.hwnd) {
            return Some(Region::new());
        }
        return Some(crate::win32::user32::paint::visible_region(
            user,
            c.hwnd,
            DCX_WINDOW | DCX_CLIPSIBLINGS,
        ));
    }
    let list = c.list?;
    let mut region = Region::new();
    for [l, t, r, b] in list {
        region = region.or_rect(&Rect::new(l, t, r, b));
    }
    Some(region)
}

/// GetClipList(this, rect, data, size).
pub fn get_clip_list(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, rect_at, data, size_at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    hr!(ready(cpu, this));
    if size_at == 0 || !cpu.bus.guest_probe(size_at, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(mut region) = region(cpu, this) else {
        return Some(DDERR_NOCLIPLIST);
    };
    if rect_at != 0 {
        if !cpu.bus.guest_probe(rect_at, 16, false) {
            return Some(DDERR_INVALIDPARAMS);
        }
        let r = Rect::read(cpu, rect_at);
        region = region.and_rect(&r);
    }
    let rects = region.rects.clone();
    let needed = HEADER + 16 * rects.len() as u32;
    if data == 0 {
        cpu.bus.guest_write_32(size_at, needed);
        return Some(DD_OK);
    }
    if cpu.bus.guest_read_32(size_at) < needed {
        cpu.bus.guest_write_32(size_at, needed);
        return Some(DDERR_REGIONTOOSMALL);
    }
    if !cpu.bus.guest_probe(data, needed as usize, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let bounds = region.bounds();
    for (i, v) in [
        HEADER,
        RDH_RECTANGLES,
        rects.len() as u32,
        16 * rects.len() as u32,
    ]
    .into_iter()
    .enumerate()
    {
        cpu.bus.guest_write_32(data + 4 * i as u32, v);
    }
    bounds.write(cpu, data + 16);
    for (i, r) in rects.iter().enumerate() {
        r.write(cpu, data + HEADER + 16 * i as u32);
    }
    cpu.bus.guest_write_32(size_at, needed);
    Some(DD_OK)
}

/// SetClipList(this, data, flags).
pub fn set_clip_list(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, data, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(ready(cpu, this));
    if flags != 0 {
        return Some(DDERR_INVALIDPARAMS);
    }
    if clipper(cpu, id).is_some_and(|c| c.hwnd != 0) {
        return Some(DDERR_CLIPPERISUSINGHWND);
    }
    let list = if data != 0 {
        if !cpu.bus.guest_probe(data, HEADER as usize, false) {
            return Some(DDERR_INVALIDPARAMS);
        }
        let count = cpu.bus.guest_read_32(data + 8);
        let len = (count as usize).saturating_mul(16);
        if count > 0x10000 || !cpu.bus.guest_probe(data + HEADER, len, false) {
            return Some(DDERR_INVALIDCLIPLIST);
        }
        let mut list = Vec::new();
        for i in 0..count {
            let r = Rect::read(cpu, data + HEADER + 16 * i);
            list.push([r.left, r.top, r.right, r.bottom]);
        }
        Some(list)
    } else {
        None
    };
    if let Some(c) = clipper(cpu, id) {
        c.list = list;
    }
    Some(DD_OK)
}

/// GetHWnd(this, out).
pub fn get_hwnd(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(ready(cpu, args[0]));
    let hwnd = clipper(cpu, id).map_or(0, |c| c.hwnd);
    if !com::write_out(cpu, args[1], hwnd) {
        return Some(DDERR_INVALIDPARAMS);
    }
    Some(DD_OK)
}

/// SetHWnd(this, flags, hwnd).
pub fn set_hwnd(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, hwnd] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(ready(cpu, this));
    if hwnd != 0 && !cpu.bus.win32.user.is_window(hwnd) {
        return Some(DDERR_INVALIDPARAMS);
    }
    if let Some(c) = clipper(cpu, id) {
        c.hwnd = hwnd;
        c.list = None;
    }
    Some(DD_OK)
}

/// IsClipListChanged(this, out).
pub fn is_clip_list_changed(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(ready(cpu, args[0]));
    if !com::write_out(cpu, args[1], 0) {
        return Some(DDERR_INVALIDPARAMS);
    }
    Some(DD_OK)
}

crate::state_fields!(Clipper {
    initialized,
    hwnd,
    list
});
