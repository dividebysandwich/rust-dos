//! OLE's standard font object (StdFont), which OleCreateFontIndirect
//! makes: a font described by its name, size in points, weight, character
//! set and style, with a GDI font made from that description when asked
//! for. Visual Basic's runtime makes one for each form and control.
//!
//! The object has the interfaces IFont, IFontDisp (its IDispatch),
//! IPersistStream, IConnectionPointContainer and IPersistPropertyBag,
//! and two connection points, for IPropertyNotifySink and
//! IFontEventsDisp, whose sinks hear of each property changed. The
//! HFONTs the objects make are kept in a list, each with the references
//! the objects and the program hold, as Windows keeps them.
//!
//! What the methods do follows Wine's oleaut32/olefont.c and connpt.c
//! (LGPL 2.1 or later):
//!
//! Copyright 1999 Francis Beaudet
//! Copyright 2006 (Google) Benjamin Arai
//! Copyright 2000 Huw D M Davies for CodeWeavers

use super::{connpt, dll_index, insert, vtable};
use crate::cpu::Cpu;
use crate::savestate::Since;
use crate::win32::com::{
    self, E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, Guid, S_FALSE, S_OK,
};
use crate::win32::gdi32;
use crate::win32::task::{self, Next, Task};

const IID_IFONT: Guid = Guid::new(
    0xBEF6_E002,
    0xA874,
    0x101A,
    [0x8B, 0xBA, 0x00, 0xAA, 0x00, 0x30, 0x0C, 0xAB],
);
const IID_IFONTDISP: Guid = Guid::new(
    0xBEF6_E003,
    0xA874,
    0x101A,
    [0x8B, 0xBA, 0x00, 0xAA, 0x00, 0x30, 0x0C, 0xAB],
);
const CLSID_STDFONT: Guid = Guid::new(
    0x0BE3_5203,
    0x8F91,
    0x11CE,
    [0x9D, 0xE3, 0x00, 0xAA, 0x00, 0x4B, 0xB8, 0x51],
);
pub(super) const IID_IDISPATCH: Guid = Guid::new(0x0002_0400, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
pub(super) const IID_IPERSIST: Guid = Guid::new(0x0000_010C, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
pub(super) const IID_IPERSISTSTREAM: Guid =
    Guid::new(0x0000_0109, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IPERSISTPROPERTYBAG: Guid = Guid::new(
    0x37D8_4F60,
    0x42CB,
    0x11CE,
    [0x81, 0x35, 0x00, 0xAA, 0x00, 0x4B, 0xB8, 0x51],
);

/// HRESULTs.
pub(super) const E_FAIL: u32 = 0x8000_4005;
pub(super) const CTL_E_INVALIDPROPERTYVALUE: u32 = 0x800A_017C;
pub(super) const DISP_E_UNKNOWNINTERFACE: u32 = 0x8002_0001;
pub(super) const DISP_E_MEMBERNOTFOUND: u32 = 0x8002_0003;
pub(super) const DISP_E_PARAMNOTOPTIONAL: u32 = 0x8002_000F;
pub(super) const DISP_E_BADPARAMCOUNT: u32 = 0x8002_000E;
const DISP_E_UNKNOWNNAME: u32 = super::typelib::DISP_E_UNKNOWNNAME;

/// The properties' DISPIDs.
const DISPID_FONT_NAME: u32 = 0;
const DISPID_FONT_SIZE: u32 = 2;
const DISPID_FONT_BOLD: u32 = 3;
const DISPID_FONT_ITALIC: u32 = 4;
const DISPID_FONT_UNDER: u32 = 5;
const DISPID_FONT_STRIKE: u32 = 6;
const DISPID_FONT_WEIGHT: u32 = 7;
const DISPID_FONT_CHARSET: u32 = 8;

/// IFontDisp's names, by their DISPIDs, as STDOLE2.TLB has them.
const NAMES: &[(&str, u32)] = &[
    ("Name", DISPID_FONT_NAME),
    ("Size", DISPID_FONT_SIZE),
    ("Bold", DISPID_FONT_BOLD),
    ("Italic", DISPID_FONT_ITALIC),
    ("Underline", DISPID_FONT_UNDER),
    ("Strikethrough", DISPID_FONT_STRIKE),
    ("Weight", DISPID_FONT_WEIGHT),
    ("Charset", DISPID_FONT_CHARSET),
];

/// IDispatch::Invoke's kinds of call.
pub(super) const DISPATCH_PROPERTYGET: u32 = 2;
pub(super) const DISPATCH_PROPERTYPUT: u32 = 4;

/// VARIANT types.
pub(super) const VT_I2: u16 = 2;
const VT_CY: u16 = 6;
const VT_BSTR: u16 = 8;
const VT_BOOL: u16 = 11;

const FW_NORMAL: i16 = 400;
const FW_BOLD: i16 = 700;
const DEFAULT_CHARSET: u8 = 1;
const OUT_CHARACTER_PRECIS: u8 = 2;
/// GetDeviceCaps' LOGPIXELSY.
pub(super) const LOGPIXELSY: u32 = 90;

/// The serialized font's attributes (IPersistStream).
const PERSIST_ITALIC: u8 = 0x02;
const PERSIST_UNDERLINE: u8 = 0x04;
const PERSIST_STRIKETHROUGH: u8 = 0x08;

/// The two connection points, by their places.
const PROPERTY_NOTIFY: u32 = 0;
pub(super) const FONT_EVENTS: u32 = 1;

/// A standard font object.
#[derive(Clone, Debug, Default)]
pub struct Font {
    pub pid: u32,
    pub refs: u32,
    /// FONTDESC: the face name, the size in points times 10000 (a CY),
    /// the weight, the character set and the styles.
    pub name: String,
    pub size: i64,
    pub weight: i16,
    pub charset: i16,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// The HFONT made from the description, 0 before one is, and whether
    /// the description changed since.
    pub hfont: u32,
    pub dirty: bool,
    /// The face the HFONT was made with.
    pub face: String,
    /// The height in pixels of the HFONT's characters without their
    /// internal leading.
    pub real_height: i32,
    /// IFont::SetRatio's logical units per HIMETRIC.
    pub cy_logical: i32,
    pub cy_himetric: i32,
    /// The interfaces: IFont, IFontDisp, IPersistStream,
    /// IConnectionPointContainer, IPersistPropertyBag, and the two
    /// connection points.
    pub ifaces: Vec<u32>,
    /// Each connection point's sinks, by their cookies less one: the
    /// interface Advise got from the sink, 0 for a free cookie.
    pub sinks: [Vec<u32>; 2],
}

crate::state_fields!(Font {
    pid,
    refs,
    name,
    size,
    weight,
    charset,
    italic,
    underline,
    strike,
    hfont,
    dirty,
    face,
    real_height,
    cy_logical,
    cy_himetric,
    ifaces,
    sinks
});

/// An HFONT the objects made: the process, the references of the
/// objects that use it, and those with the program's (AddRefHfont)
/// added.
#[derive(Clone, Debug, Default)]
pub struct HfontItem {
    pub pid: u32,
    pub hfont: u32,
    pub int_refs: i32,
    pub total_refs: i32,
}

crate::state_fields!(HfontItem {
    pid,
    hfont,
    int_refs,
    total_refs
});

/// The font objects and their HFONTs, and the picture objects
/// (picture.rs).
#[derive(Clone, Debug, Default)]
pub struct Fonts {
    pub objects: Vec<Option<Font>>,
    pub hfonts: Vec<HfontItem>,
    pub pictures: Vec<Option<super::picture::Picture>>,
}

crate::state_fields!(Fonts {
    objects,
    hfonts,
    pictures
});

/// The W32 section's version that added the font objects.
pub const FONTS_SAVED: u16 = 120;

pub type Saved = Since<FONTS_SAVED, Fonts>;

/// The interfaces' places in `Font::ifaces`.
const I_FONT: usize = 0;
const I_DISPATCH: usize = 1;
const I_PERSIST: usize = 2;
const I_CONTAINER: usize = 3;
const I_BAG: usize = 4;
const I_POINT: usize = 5;

/// The vtables of the interfaces before the connection points, in the
/// order of `Font::ifaces`.
const VTABLES: [&str; 5] = [
    "IFont",
    "IFontDisp",
    "IFontPersistStream",
    "IFontConnectionPointContainer",
    "IFontPersistPropertyBag",
];

fn fonts(cpu: &mut Cpu) -> &mut Fonts {
    &mut cpu.bus.win32.oleaut32.fonts
}

fn get(cpu: &mut Cpu, id: usize) -> &mut Font {
    fonts(cpu).objects[id].as_mut().expect("a font object")
}

/// The font object of the interface `this`, one of those in `VTABLES`.
fn object(cpu: &mut Cpu, this: u32) -> Option<usize> {
    let (v, id) = com::interface_of(cpu, dll_index(), this)?;
    if !VTABLES.iter().any(|&name| vtable(name) == v) {
        return None;
    }
    fonts(cpu).objects.get(id as usize)?.as_ref()?;
    Some(id as usize)
}

/// The sinks of the connection point `point` of the object `id`, if it is
/// there (connpt.rs).
pub(super) fn sinks(cpu: &mut Cpu, id: usize, point: u32) -> Option<&mut Vec<u32>> {
    let o = fonts(cpu).objects.get_mut(id)?.as_mut()?;
    o.sinks.get_mut(point as usize)
}

/// Whether `this` is one of the font objects' interfaces, for the
/// functions that take an IFont of another's (IFont::IsEqual).
fn is_font(cpu: &mut Cpu, this: u32) -> bool {
    object(cpu, this).is_some()
}

/// A process ended: its objects went with its heap, and its HFONTs with
/// its GDI objects.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let f = fonts(cpu);
    for slot in f.objects.iter_mut() {
        if slot.as_ref().is_some_and(|o| o.pid == pid) {
            *slot = None;
        }
    }
    f.hfonts.retain(|h| h.pid != pid);
}

/// A FONTDESC read from the program: its fields, Wine's order.
struct Desc {
    name: String,
    size: i64,
    weight: i16,
    charset: i16,
    italic: bool,
    underline: bool,
    strike: bool,
}

/// OleCreateFontIndirect(desc, iid, out): a new font object of the
/// FONTDESC `desc`, its interface `iid` at `out`. With no FONTDESC the
/// font is 8-point System. OLEPRO32 exports the function too.
pub fn create_font_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[desc, iid, out] = args else {
        unreachable!()
    };
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(E_POINTER);
    }
    cpu.bus.guest_write_32(out, 0);
    let desc = if desc == 0 {
        Desc {
            name: "System".into(),
            size: 80000,
            weight: 0,
            charset: 0,
            italic: false,
            underline: false,
            strike: false,
        }
    } else {
        if !cpu.bus.guest_probe(desc, 32, false) {
            return Some(E_POINTER);
        }
        let name_at = cpu.bus.guest_read_32(desc + 4);
        let Some(name) = super::wide_string(cpu, name_at) else {
            return Some(CTL_E_INVALIDPROPERTYVALUE);
        };
        let mut size = [0u8; 8];
        cpu.bus.guest_read_bytes(desc + 8, &mut size);
        Desc {
            name,
            size: i64::from_le_bytes(size),
            weight: cpu.bus.guest_read_16(desc + 16) as i16,
            charset: cpu.bus.guest_read_16(desc + 18) as i16,
            italic: cpu.bus.guest_read_32(desc + 20) != 0,
            underline: cpu.bus.guest_read_32(desc + 24) != 0,
            strike: cpu.bus.guest_read_32(desc + 28) != 0,
        }
    };
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(E_INVALIDARG);
    };
    let font = Font {
        name: desc.name,
        size: desc.size,
        weight: desc.weight,
        charset: desc.charset,
        italic: desc.italic,
        underline: desc.underline,
        strike: desc.strike,
        ..Font::default()
    };
    let Some(id) = construct(cpu, font) else {
        return Some(E_OUTOFMEMORY);
    };
    // The new object's one reference is the caller's if it has the
    // interface asked for, and goes otherwise.
    let hr = query(cpu, id, &iid, out);
    let calls = release(cpu, id).unwrap_or_default();
    crate::win32::ole32::call_all(cpu, hr, &calls)
}

/// A new font object of the description `font` has, with one reference:
/// its id, or None when memory is out.
fn construct(cpu: &mut Cpu, mut font: Font) -> Option<usize> {
    font.pid = crate::win32::user32::current_pid(cpu);
    font.refs = 1;
    font.dirty = true;
    font.hfont = 0;
    font.cy_logical = gdi32::dc::device_cap(cpu, LOGPIXELSY) as i32;
    font.cy_himetric = 2540;
    font.sinks = Default::default();
    font.ifaces = Vec::new();
    let id = insert(&mut fonts(cpu).objects, font);
    let mut ifaces = Vec::new();
    for name in VTABLES {
        ifaces.push(com::new_interface(
            cpu,
            dll_index(),
            vtable(name),
            id as u32,
            0,
        ));
    }
    for point in [PROPERTY_NOTIFY, FONT_EVENTS] {
        ifaces.push(connpt::new_point(cpu, connpt::FONT, id, point));
    }
    if ifaces.contains(&None) {
        for at in ifaces.into_iter().flatten() {
            com::free_interface(cpu, at);
        }
        fonts(cpu).objects[id] = None;
        return None;
    }
    get(cpu, id).ifaces = ifaces.into_iter().flatten().collect();
    Some(id)
}

/// The object's interface `iid` at `out`, with a reference added.
pub(super) fn query(cpu: &mut Cpu, id: usize, iid: &Guid, out: u32) -> u32 {
    let o = get(cpu, id);
    let index = match *iid {
        _ if *iid == com::IID_IUNKNOWN || *iid == IID_IFONT => I_FONT,
        _ if *iid == IID_IDISPATCH || *iid == IID_IFONTDISP => I_DISPATCH,
        _ if *iid == IID_IPERSIST || *iid == IID_IPERSISTSTREAM => I_PERSIST,
        _ if *iid == connpt::IID_ICONNECTIONPOINTCONTAINER => I_CONTAINER,
        _ if *iid == IID_IPERSISTPROPERTYBAG => I_BAG,
        _ => {
            com::write_out(cpu, out, 0);
            return E_NOINTERFACE;
        }
    };
    let iface = o.ifaces[index];
    add_ref(cpu, id);
    com::write_out(cpu, out, iface);
    S_OK
}

pub(super) fn add_ref(cpu: &mut Cpu, id: usize) -> u32 {
    let o = get(cpu, id);
    o.refs = o.refs.saturating_add(1);
    o.refs
}

/// Release a reference: None while others are left, else the calls that
/// release the sinks the gone object held.
fn release(cpu: &mut Cpu, id: usize) -> Option<Vec<(u32, u32)>> {
    let o = get(cpu, id);
    o.refs = o.refs.saturating_sub(1);
    if o.refs > 0 {
        return None;
    }
    let o = fonts(cpu).objects[id].take().expect("a font object");
    let calls = connpt::release_calls(cpu, o.sinks.iter().flatten().copied());
    for &at in &o.ifaces {
        com::free_interface(cpu, at);
    }
    // The last font object gone, the HFONTs go, whatever the program
    // holds of them; otherwise this one's reference to its own.
    let pid = o.pid;
    let others = fonts(cpu).objects.iter().flatten().any(|f| f.pid == pid);
    if others {
        dec_int_ref(cpu, pid, o.hfont);
    } else {
        let gone: Vec<u32> = fonts(cpu)
            .hfonts
            .iter()
            .filter(|h| h.pid == pid)
            .map(|h| h.hfont)
            .collect();
        fonts(cpu).hfonts.retain(|h| h.pid != pid);
        // DeleteObject's work: one still selected into a DC goes when
        // it is selected out.
        for h in gone {
            gdi32::delete_object(cpu, &[h]);
        }
    }
    Some(calls)
}

/// IUnknown::Release's work for any of the object's interfaces: the
/// references left, after the calls that release the sinks of an object
/// that went.
pub(super) fn release_ref(cpu: &mut Cpu, id: usize) -> Option<u32> {
    match release(cpu, id) {
        None => Some(get(cpu, id).refs),
        Some(calls) => crate::win32::ole32::call_all(cpu, 0, &calls),
    }
}

fn item(cpu: &mut Cpu, pid: u32, hfont: u32) -> Option<&mut HfontItem> {
    fonts(cpu)
        .hfonts
        .iter_mut()
        .find(|h| h.pid == pid && h.hfont == hfont)
}

/// An object's reference to `hfont` added (IFont::Clone); S_FALSE when
/// the list hasn't it.
fn inc_int_ref(cpu: &mut Cpu, pid: u32, hfont: u32) -> u32 {
    match item(cpu, pid, hfont) {
        Some(h) => {
            h.int_refs += 1;
            h.total_refs += 1;
            S_OK
        }
        None => S_FALSE,
    }
}

/// An object's reference to `hfont` released: the HFONT is deleted when
/// no object and no reference of the program's holds it.
fn dec_int_ref(cpu: &mut Cpu, pid: u32, hfont: u32) -> u32 {
    let Some(h) = item(cpu, pid, hfont) else {
        return S_FALSE;
    };
    h.int_refs -= 1;
    h.total_refs -= 1;
    if h.int_refs == 0 && h.total_refs == 0 {
        fonts(cpu)
            .hfonts
            .retain(|h| !(h.pid == pid && h.hfont == hfont));
        gdi32::delete_object(cpu, &[hfont]);
    }
    S_OK
}

/// MulDiv: a * b / c rounded to the nearest, -1 when c is 0 or the
/// result doesn't fit.
pub(super) fn mul_div(a: i32, b: i32, c: i32) -> i32 {
    if c == 0 {
        return -1;
    }
    let p = a as i64 * b as i64;
    let half = (c as i64).abs() / 2;
    let q = if (p < 0) != (c < 0) {
        (p - if p < 0 { half } else { -half }) / c as i64
    } else {
        (p + if p < 0 { -half } else { half }) / c as i64
    };
    i32::try_from(q).unwrap_or(-1)
}

/// Make the HFONT of the description if it changed since the last one,
/// and take the face and character set the font got, and its height.
fn realize(cpu: &mut Cpu, id: usize) {
    if !get(cpu, id).dirty {
        return;
    }
    let pid = get(cpu, id).pid;
    let old = std::mem::take(&mut get(cpu, id).hfont);
    let old_face = std::mem::take(&mut get(cpu, id).face);
    if old != 0 {
        dec_int_ref(cpu, pid, old);
    }
    let o = get(cpu, id).clone();
    // A name changed since the last font: Windows looks for a character
    // set of the new face (Wine enumerates the face's), which the default
    // one gets.
    let charset = if !old_face.is_empty() && !old_face.eq_ignore_ascii_case(&o.name) {
        DEFAULT_CHARSET
    } else {
        o.charset as u8
    };
    // Standard ratio is 72 points to 2540 HIMETRIC, 18 to 635, and
    // SetRatio's is taken relative to it.
    let size = o.size as i32;
    let height = mul_div(
        size,
        o.cy_logical.wrapping_mul(635),
        o.cy_himetric.wrapping_mul(18),
    );
    let height = if height % 10000 > 5000 {
        -height / 10000 - 1
    } else {
        -height / 10000
    };
    let page = crate::win32::nls::ansi(cpu);
    let mut log = gdi32::font::logfont(height, 0, o.weight as i32, charset, 0, "");
    let face = super::ansi(page, &o.name);
    let n = face.len().min(31);
    log[28..28 + n].copy_from_slice(&face[..n]);
    log[20] = o.italic as u8;
    log[21] = o.underline as u8;
    log[22] = o.strike as u8;
    log[24] = OUT_CHARACTER_PRECIS;
    let realized = gdi32::font::realize_log(cpu, &log);
    let hfont = gdi32::font::new_font(cpu, log);
    fonts(cpu).hfonts.push(HfontItem {
        pid,
        hfont,
        int_refs: 1,
        total_refs: 1,
    });
    let o = get(cpu, id);
    o.hfont = hfont;
    o.dirty = false;
    // The name and character set are the font's that was chosen.
    if !realized.face.eq_ignore_ascii_case(&o.name) {
        o.name = realized.face.to_string();
    }
    o.face = o.name.clone();
    o.charset = realized.charset as i16;
    o.real_height = realized.height - realized.internal_leading;
}

/// A property changed: the sinks of both connection points hear of it
/// (connpt.rs).
fn changed(cpu: &mut Cpu, id: usize, dispid: u32) -> Option<u32> {
    let o = get(cpu, id);
    o.dirty = true;
    let [notify, events] = o.sinks.clone();
    let name = NAMES
        .iter()
        .find(|&&(_, d)| d == dispid)
        .map_or("", |&(n, _)| n);
    connpt::notify(cpu, connpt::FONT, id, dispid, &notify, &events, name)
}

/// LOAD's and SAVE's scratch space: the bytes, and the count Read or
/// Write gives.
const P_BYTES: u32 = 0;
const P_COUNT: u32 = 0x10C;
const P_SCRATCH: u32 = 0x110;
/// The serialized font's bytes before the name.
const P_HEAD: u32 = 11;

/// IStream's Read and Write.
const READ: u32 = 3;
const WRITE: u32 = 4;

/// The font object of a task's variable 1, if it is still there.
fn task_font(cpu: &mut Cpu, t: &Task) -> Option<usize> {
    let id = t.var(1) as usize;
    fonts(cpu).objects.get(id)?.as_ref()?;
    Some(id)
}

/// LOAD: variables the object and the stream. The bytes are version 1,
/// the character set (2 bytes), the attributes, the weight (2), the size
/// (the CY's low 4), the name's length and the name in the ANSI code
/// page.
pub fn load_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let s = t.scratch;
    let stream = t.var(2);
    let read = crate::win32::ole32::method(cpu, stream, READ).unwrap_or(0);
    let count = cpu.bus.guest_read_32(s + P_COUNT);
    match t.step {
        0 => Next::call(read, &[stream, s + P_BYTES, P_HEAD, s + P_COUNT], 1),
        1 => {
            if value >> 31 != 0 || count != P_HEAD || cpu.bus.guest_read_8(s) != 1 {
                return Next::Done(E_FAIL);
            }
            let len = cpu.bus.guest_read_8(s + P_HEAD - 1) as u32;
            cpu.bus.guest_write_32(s + P_COUNT, 0);
            Next::call(read, &[stream, s + P_HEAD, len, s + P_COUNT], 2)
        }
        _ => {
            let mut head = [0u8; P_HEAD as usize];
            cpu.bus.guest_read_bytes(s, &mut head);
            let len = head[10] as u32;
            if value >> 31 != 0 || count != len {
                return Next::Done(E_FAIL);
            }
            let mut name = vec![0u8; len as usize];
            cpu.bus.guest_read_bytes(s + P_HEAD, &mut name);
            let name = String::from_utf16_lossy(&crate::win32::nls::ansi(cpu).decode(&name));
            let Some(id) = task_font(cpu, t) else {
                return Next::Done(E_FAIL);
            };
            let (pid, old) = {
                let o = get(cpu, id);
                o.charset = i16::from_le_bytes([head[1], head[2]]);
                o.italic = head[3] & PERSIST_ITALIC != 0;
                o.strike = head[3] & PERSIST_STRIKETHROUGH != 0;
                o.underline = head[3] & PERSIST_UNDERLINE != 0;
                o.weight = i16::from_le_bytes([head[4], head[5]]);
                o.size = u32::from_le_bytes([head[6], head[7], head[8], head[9]]) as i64;
                o.name = name;
                o.dirty = true;
                // No font made before is the one to compare the name
                // with, as Wine has it: the character set read is kept.
                o.face.clear();
                (o.pid, std::mem::take(&mut o.hfont))
            };
            dec_int_ref(cpu, pid, old);
            Next::Done(S_OK)
        }
    }
}

/// The serialized font (LOAD's form).
fn serialized(cpu: &mut Cpu, id: usize) -> Vec<u8> {
    let page = crate::win32::nls::ansi(cpu);
    let o = get(cpu, id);
    let mut attributes = 0;
    if o.italic {
        attributes |= PERSIST_ITALIC;
    }
    if o.strike {
        attributes |= PERSIST_STRIKETHROUGH;
    }
    if o.underline {
        attributes |= PERSIST_UNDERLINE;
    }
    let mut name = super::ansi(page, &o.name);
    name.truncate(255);
    let mut b = vec![1];
    b.extend_from_slice(&o.charset.to_le_bytes());
    b.push(attributes);
    b.extend_from_slice(&o.weight.to_le_bytes());
    b.extend_from_slice(&(o.size as u32).to_le_bytes());
    b.push(name.len() as u8);
    b.extend_from_slice(&name);
    b
}

/// SAVE: variables the object and the stream.
pub fn save_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let s = t.scratch;
    let stream = t.var(2);
    match t.step {
        0 => {
            let Some(id) = task_font(cpu, t) else {
                return Next::Done(E_FAIL);
            };
            let bytes = serialized(cpu, id);
            cpu.bus.guest_write_bytes(s + P_BYTES, &bytes);
            t.set(3, bytes.len() as u32);
            let write = crate::win32::ole32::method(cpu, stream, WRITE).unwrap_or(0);
            Next::call(
                write,
                &[stream, s + P_BYTES, bytes.len() as u32, s + P_COUNT],
                1,
            )
        }
        _ => {
            let written = cpu.bus.guest_read_32(s + P_COUNT);
            Next::Done(if value >> 31 != 0 || written != t.var(3) {
                E_FAIL
            } else {
                S_OK
            })
        }
    }
}

// IUnknown, for every interface of the object but the connection
// points' (connpt.rs).

pub fn query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, iid, out] = args else {
        unreachable!()
    };
    if out == 0 {
        return Some(E_POINTER);
    }
    let (Some(id), Some(iid)) = (object(cpu, this), Guid::read(cpu, iid)) else {
        return Some(E_INVALIDARG);
    };
    Some(query(cpu, id, &iid, out))
}

pub fn method_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(object(cpu, args[0]).map_or(0, |id| add_ref(cpu, id)))
}

pub fn method_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match object(cpu, args[0]) {
        Some(id) => release_ref(cpu, id),
        None => Some(0),
    }
}

// IFont.

/// The object of `this`, realized, or the HRESULT when `this` isn't one
/// or `out` is NULL.
fn realized(cpu: &mut Cpu, this: u32, out: u32) -> Result<usize, u32> {
    let id = object(cpu, this).ok_or(E_INVALIDARG)?;
    if out == 0 {
        return Err(E_POINTER);
    }
    realize(cpu, id);
    Ok(id)
}

/// A getter: what `value` gives of the realized object, written at the
/// out pointer (args[1]) as a 32-bit value.
fn get_32(cpu: &mut Cpu, args: &[u32], value: fn(&Font) -> u32) -> Option<u32> {
    match realized(cpu, args[0], args[1]) {
        Ok(id) => {
            let v = value(get(cpu, id));
            com::write_out(cpu, args[1], v);
            Some(S_OK)
        }
        Err(e) => Some(e),
    }
}

/// A 16-bit getter (Weight, Charset).
fn get_16(cpu: &mut Cpu, args: &[u32], value: fn(&Font) -> i16) -> Option<u32> {
    match realized(cpu, args[0], args[1]) {
        Ok(id) => {
            let v = value(get(cpu, id));
            if cpu.bus.guest_probe(args[1], 2, true) {
                cpu.bus.guest_write_16(args[1], v as u16);
            }
            Some(S_OK)
        }
        Err(e) => Some(e),
    }
}

/// A setter: `set` changes the object, and the sinks hear of `dispid`.
fn put(cpu: &mut Cpu, this: u32, dispid: u32, set: impl FnOnce(&mut Font)) -> Option<u32> {
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    set(get(cpu, id));
    changed(cpu, id, dispid)
}

pub fn get_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match realized(cpu, args[0], args[1]) {
        Ok(id) => {
            let name = get(cpu, id).name.clone();
            let b = super::bstr(cpu, &name);
            com::write_out(cpu, args[1], b);
            Some(if b == 0 { E_OUTOFMEMORY } else { S_OK })
        }
        Err(e) => Some(e),
    }
}

pub fn put_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(name) = super::wide_string(cpu, args[1]) else {
        return Some(CTL_E_INVALIDPROPERTYVALUE);
    };
    put(cpu, args[0], DISPID_FONT_NAME, |o| o.name = name)
}

/// get_Size: the size of the font chosen, from its height, in points
/// times 10000 at the object's ratio.
pub fn get_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match realized(cpu, args[0], args[1]) {
        Ok(id) => {
            let o = get(cpu, id);
            let lo = mul_div(
                o.real_height,
                o.cy_himetric.wrapping_mul(72 * 10000),
                o.cy_logical.wrapping_mul(2540),
            );
            if cpu.bus.guest_probe(args[1], 8, true) {
                cpu.bus.guest_write_32(args[1], lo as u32);
                cpu.bus.guest_write_32(args[1] + 4, 0);
            }
            Some(S_OK)
        }
        Err(e) => Some(e),
    }
}

/// put_Size(this, size): the CY by value, of which the low half is
/// taken.
pub fn put_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let lo = args[1];
    put(cpu, args[0], DISPID_FONT_SIZE, |o| o.size = lo as i64)
}

pub fn get_bold(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_32(cpu, args, |o| (o.weight > 550) as u32)
}

pub fn put_bold(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let bold = args[1] != 0;
    put(cpu, args[0], DISPID_FONT_BOLD, |o| {
        o.weight = if bold { FW_BOLD } else { FW_NORMAL }
    })
}

pub fn get_italic(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_32(cpu, args, |o| o.italic as u32)
}

pub fn put_italic(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = args[1] != 0;
    put(cpu, args[0], DISPID_FONT_ITALIC, |o| o.italic = v)
}

pub fn get_underline(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_32(cpu, args, |o| o.underline as u32)
}

pub fn put_underline(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = args[1] != 0;
    put(cpu, args[0], DISPID_FONT_UNDER, |o| o.underline = v)
}

pub fn get_strikethrough(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_32(cpu, args, |o| o.strike as u32)
}

pub fn put_strikethrough(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = args[1] != 0;
    put(cpu, args[0], DISPID_FONT_STRIKE, |o| o.strike = v)
}

pub fn get_weight(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_16(cpu, args, |o| o.weight)
}

pub fn put_weight(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = args[1] as i16;
    put(cpu, args[0], DISPID_FONT_WEIGHT, |o| o.weight = v)
}

pub fn get_charset(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_16(cpu, args, |o| o.charset)
}

pub fn put_charset(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = args[1] as i16;
    put(cpu, args[0], DISPID_FONT_CHARSET, |o| o.charset = v)
}

pub fn get_hfont(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_32(cpu, args, |o| o.hfont)
}

/// Clone(this, out): a new object of the same description, ratio and
/// HFONT, with no sinks.
pub fn clone(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, out] = args else { unreachable!() };
    if out == 0 {
        return Some(E_POINTER);
    }
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    com::write_out(cpu, out, 0);
    let o = get(cpu, id).clone();
    let Some(new) = construct(cpu, o.clone()) else {
        return Some(E_OUTOFMEMORY);
    };
    let n = get(cpu, new);
    n.hfont = o.hfont;
    n.dirty = o.dirty;
    n.face = o.face.clone();
    n.real_height = o.real_height;
    n.cy_logical = o.cy_logical;
    n.cy_himetric = o.cy_himetric;
    let iface = n.ifaces[I_FONT];
    if o.hfont != 0 {
        inc_int_ref(cpu, o.pid, o.hfont);
    }
    com::write_out(cpu, out, iface);
    Some(S_OK)
}

/// IsEqual(this, other): S_OK when the two descriptions are the same,
/// the names compared as CompareString compares them.
pub fn is_equal(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, other] = args else { unreachable!() };
    if other == 0 {
        return Some(E_POINTER);
    }
    let Some(a) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if !is_font(cpu, other) {
        return Some(S_FALSE);
    }
    let b = object(cpu, other).expect("a font object");
    let (a, b) = (get(cpu, a).clone(), get(cpu, b).clone());
    let same = a.size == b.size
        && a.weight == b.weight
        && a.charset == b.charset
        && a.italic == b.italic
        && a.underline == b.underline
        && a.strike == b.strike
        && a.name == b.name;
    Some(if same { S_OK } else { S_FALSE })
}

pub fn set_ratio(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, logical, himetric] = args else {
        unreachable!()
    };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let (logical, himetric) = (logical as i32, himetric as i32);
    if logical == 0 || himetric == 0 {
        return Some(E_FAIL);
    }
    // 1 and 1 keeps the ratio as it is, and doesn't fail.
    if logical == 1 && himetric == 1 {
        return Some(S_OK);
    }
    let o = get(cpu, id);
    o.cy_logical = logical;
    o.cy_himetric = himetric;
    o.dirty = true;
    Some(S_OK)
}

/// QueryTextMetrics(this, metrics): the HFONT's TEXTMETRICW on a DC of
/// the screen.
pub fn query_text_metrics(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, out] = args else { unreachable!() };
    let id = match realized(cpu, this, out) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let hfont = get(cpu, id).hfont;
    let dc = gdi32::dc::create_compatible_dc(cpu, &[0])?;
    let old = gdi32::dc::select_object(cpu, &[dc, hfont])?;
    let tm = gdi32::font::text_metric_wide(cpu, dc);
    gdi32::dc::select_object(cpu, &[dc, old])?;
    gdi32::dc::delete_dc(cpu, &[dc])?;
    if let Some(tm) = tm
        && cpu.bus.guest_probe(out, tm.len(), true)
    {
        cpu.bus.guest_write_bytes(out, &tm);
    }
    Some(S_OK)
}

pub fn add_ref_hfont(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, hfont] = args else { unreachable!() };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if hfont == 0 {
        return Some(E_INVALIDARG);
    }
    let pid = get(cpu, id).pid;
    Some(match item(cpu, pid, hfont) {
        Some(h) => {
            h.total_refs += 1;
            S_OK
        }
        None => S_FALSE,
    })
}

pub fn release_hfont(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, hfont] = args else { unreachable!() };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if hfont == 0 {
        return Some(E_INVALIDARG);
    }
    let pid = get(cpu, id).pid;
    Some(match item(cpu, pid, hfont) {
        Some(h) => {
            h.total_refs -= 1;
            if h.total_refs >= 0 { S_OK } else { S_FALSE }
        }
        None => S_FALSE,
    })
}

/// SetHdc: E_NOTIMPL, as Wine has it.
pub fn set_hdc(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(E_NOTIMPL)
}

// IFontDisp.

pub fn get_type_info_count(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let found = object(cpu, args[0]).is_some();
    get_type_info_count_of(cpu, found, args[1])
}

/// GetTypeInfoCount's work for the font and picture objects: one type
/// information at `out`, when `found` says `this` is one of theirs.
pub(super) fn get_type_info_count_of(cpu: &mut Cpu, found: bool, out: u32) -> Option<u32> {
    if !found {
        return Some(E_INVALIDARG);
    }
    Some(if com::write_out(cpu, out, 1) {
        S_OK
    } else {
        E_POINTER
    })
}

/// GetTypeInfo(this, index, lcid, out): STDOLE2.TLB's IFontDisp, which
/// the built-in library doesn't have yet (stdole.rs).
pub fn get_type_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, index, _lcid, out] = args else {
        unreachable!()
    };
    if object(cpu, this).is_none() {
        return Some(E_INVALIDARG);
    }
    com::write_out(cpu, out, 0);
    if index != 0 {
        return Some(E_FAIL);
    }
    Some(super::typelib::TYPE_E_ELEMENTNOTFOUND)
}

/// GetIDsOfNames(this, iid, names, count, lcid, ids): the DISPIDs of
/// IFontDisp's properties, the names matched without regard to case.
pub fn get_ids_of_names(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if object(cpu, args[0]).is_none() {
        return Some(E_INVALIDARG);
    }
    ids_of_names(cpu, args, NAMES)
}

/// GetIDsOfNames' work for the font and picture objects: the DISPIDs of
/// the names in `table`, whose members take no parameters.
pub(super) fn ids_of_names(cpu: &mut Cpu, args: &[u32], table: &[(&str, u32)]) -> Option<u32> {
    let &[_this, _iid, names, count, _lcid, ids] = args else {
        unreachable!()
    };
    if count == 0 {
        return Some(E_INVALIDARG);
    }
    let mut hr = S_OK;
    for i in 0..count {
        let at = cpu.bus.guest_read_32(names + 4 * i);
        let name = super::wide_string(cpu, at).unwrap_or_default();
        // The names after the first are the method's parameters, which
        // properties have none of.
        let id = if i == 0 {
            table
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(&name))
                .map(|&(_, d)| d)
        } else {
            None
        };
        let id = id.unwrap_or_else(|| {
            hr = DISP_E_UNKNOWNNAME;
            u32::MAX
        });
        cpu.bus.guest_write_32(ids + 4 * i, id);
    }
    Some(hr)
}

/// Invoke(this, dispid, iid, lcid, flags, params, result, excepinfo,
/// argerr): a property got or put.
pub fn invoke(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, dispid, iid, _lcid, flags, params, result, _, _] = args else {
        unreachable!()
    };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if Guid::read(cpu, iid) != Some(com::GUID_NULL) {
        return Some(DISP_E_UNKNOWNINTERFACE);
    }
    let font = get(cpu, id).ifaces[I_FONT];
    if flags & DISPATCH_PROPERTYGET != 0 {
        if result == 0 {
            return Some(DISP_E_PARAMNOTOPTIONAL);
        }
        let (vt, hr) = match dispid {
            DISPID_FONT_NAME => (VT_BSTR, get_name(cpu, &[font, result + 8])?),
            DISPID_FONT_SIZE => (VT_CY, get_size(cpu, &[font, result + 8])?),
            DISPID_FONT_WEIGHT => (VT_I2, get_weight(cpu, &[font, result + 8])?),
            DISPID_FONT_CHARSET => (VT_I2, get_charset(cpu, &[font, result + 8])?),
            DISPID_FONT_BOLD | DISPID_FONT_ITALIC | DISPID_FONT_UNDER | DISPID_FONT_STRIKE => {
                realize(cpu, id);
                let o = get(cpu, id);
                let v = match dispid {
                    DISPID_FONT_BOLD => o.weight > 550,
                    DISPID_FONT_ITALIC => o.italic,
                    DISPID_FONT_UNDER => o.underline,
                    _ => o.strike,
                };
                cpu.bus
                    .guest_write_16(result + 8, if v { 0xFFFF } else { 0 });
                (VT_BOOL, S_OK)
            }
            _ => return Some(DISP_E_MEMBERNOTFOUND),
        };
        cpu.bus.guest_write_16(result, vt);
        return Some(hr);
    }
    if flags & DISPATCH_PROPERTYPUT == 0 {
        return Some(DISP_E_MEMBERNOTFOUND);
    }
    if params == 0 {
        return Some(DISP_E_PARAMNOTOPTIONAL);
    }
    if cpu.bus.guest_read_32(params + 8) != 1 {
        return Some(DISP_E_BADPARAMCOUNT);
    }
    let arg = cpu.bus.guest_read_32(params);
    let vt = match dispid {
        DISPID_FONT_NAME => VT_BSTR,
        DISPID_FONT_SIZE => VT_CY,
        DISPID_FONT_WEIGHT | DISPID_FONT_CHARSET => VT_I2,
        DISPID_FONT_BOLD | DISPID_FONT_ITALIC | DISPID_FONT_UNDER | DISPID_FONT_STRIKE => VT_BOOL,
        _ => return Some(DISP_E_MEMBERNOTFOUND),
    };
    let out = match super::variant::change_from(cpu, arg, vt, 0) {
        Ok(out) => out,
        Err(e) => return Some(e),
    };
    use super::variant::Out;
    match (dispid, out) {
        (DISPID_FONT_NAME, Out::Str(units)) => {
            let name = String::from_utf16_lossy(&units);
            put(cpu, font, dispid, |o| o.name = name)
        }
        (_, Out::Bits(_, bits)) => match dispid {
            DISPID_FONT_SIZE => put_size(cpu, &[font, bits as u32, (bits >> 32) as u32]),
            DISPID_FONT_WEIGHT => put_weight(cpu, &[font, bits as u16 as i16 as u32]),
            DISPID_FONT_CHARSET => put_charset(cpu, &[font, bits as u16 as i16 as u32]),
            DISPID_FONT_BOLD => put_bold(cpu, &[font, (bits != 0) as u32]),
            DISPID_FONT_ITALIC => put_italic(cpu, &[font, (bits != 0) as u32]),
            DISPID_FONT_UNDER => put_underline(cpu, &[font, (bits != 0) as u32]),
            _ => put_strikethrough(cpu, &[font, (bits != 0) as u32]),
        },
        _ => Some(super::variant::DISP_E_TYPEMISMATCH),
    }
}

// IPersistStream and IPersistPropertyBag.

pub fn get_class_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if object(cpu, args[0]).is_none() {
        return Some(E_INVALIDARG);
    }
    Some(if CLSID_STDFONT.write(cpu, args[1]) {
        S_OK
    } else {
        E_POINTER
    })
}

/// IsDirty: S_OK, as Wine has it, so the font is always saved.
pub fn is_dirty(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_OK)
}

pub fn load(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, stream] = args else {
        unreachable!()
    };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let vars = vec![super::FONT_LOAD, id as u32, stream];
    task::start(cpu, task::OLEAUT32_OBJECTS, vars, P_SCRATCH)
}

pub fn save(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, stream, _clear_dirty] = args else {
        unreachable!()
    };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let vars = vec![super::FONT_SAVE, id as u32, stream];
    task::start(cpu, task::OLEAUT32_OBJECTS, vars, P_SCRATCH)
}

pub fn get_size_max(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, out] = args else { unreachable!() };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 || !cpu.bus.guest_probe(out, 8, true) {
        return Some(E_POINTER);
    }
    let n = serialized(cpu, id).len() as u32;
    cpu.bus.guest_write_32(out, n);
    cpu.bus.guest_write_32(out + 4, 0);
    Some(S_OK)
}

/// IPersistPropertyBag::InitNew: S_OK, the description as it is.
pub fn init_new(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_OK)
}

/// IPersistPropertyBag::Load(this, bag, log): the description from the
/// bag's properties isn't read yet; the call does what a call of a
/// missing function does.
pub fn bag_load(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    crate::win32::not_written(cpu, super::DLL.name, "IPersistPropertyBag::Load", E_NOTIMPL)
}

/// IPersistPropertyBag::Save: E_NOTIMPL, as Wine has it.
pub fn bag_save(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(E_NOTIMPL)
}

// IConnectionPointContainer.

/// EnumConnectionPoints: E_NOTIMPL, as Wine has it.
pub fn enum_connection_points(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(E_NOTIMPL)
}

pub fn find_connection_point(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, iid, out] = args else {
        unreachable!()
    };
    let (Some(id), Some(iid)) = (object(cpu, this), Guid::read(cpu, iid)) else {
        return Some(E_INVALIDARG);
    };
    let point = match iid {
        _ if iid == connpt::IID_IPROPERTYNOTIFYSINK => PROPERTY_NOTIFY,
        _ if iid == connpt::IID_IFONTEVENTSDISP => FONT_EVENTS,
        _ => {
            com::write_out(cpu, out, 0);
            return Some(connpt::CONNECT_E_NOCONNECTION);
        }
    };
    let iface = get(cpu, id).ifaces[I_POINT + point as usize];
    add_ref(cpu, id);
    Some(if com::write_out(cpu, out, iface) {
        S_OK
    } else {
        E_POINTER
    })
}
