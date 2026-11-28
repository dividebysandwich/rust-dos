//! OLE's standard picture object (StdPicture), which OleLoadPicture and
//! OleCreatePictureIndirect make: a bitmap, icon or metafile with its
//! size in HIMETRIC units (hundredths of a millimeter), drawn into a DC
//! by IPicture::Render. Visual Basic's runtime loads the pictures of
//! forms and controls with it, from the program's form data, where each
//! is a BMP, JPEG, GIF, icon or cursor file after an 8-byte header ("lt"
//! and the length).
//!
//! The object has the interfaces IPicture, IPictureDisp (its
//! IDispatch), IPersistStream and IConnectionPointContainer, and one
//! connection point, for IPropertyNotifySink (connpt.rs).
//!
//! What the methods do follows Wine's oleaut32/olepicture.c (LGPL 2.1 or
//! later), with the pictures decoded here instead of by WIC:
//!
//! Copyright 2000 Huw D M Davies for CodeWeavers.
//! Copyright 2001 Marcus Meissner

use super::font::{
    CTL_E_INVALIDPROPERTYVALUE, DISP_E_BADPARAMCOUNT, DISP_E_MEMBERNOTFOUND,
    DISP_E_PARAMNOTOPTIONAL, DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, E_FAIL, IID_IDISPATCH,
    IID_IPERSIST, IID_IPERSISTSTREAM, LOGPIXELSY, VT_I2,
};
use super::{connpt, dll_index, insert, vtable};
use crate::cpu::Cpu;
use crate::win32::com::{
    self, E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, Guid, S_OK,
};
use crate::win32::gdi32;
use crate::win32::ole32::{task_alloc, task_free};
use crate::win32::task::{self, Next, Task};
use crate::win32::user32;

const IID_IPICTURE: Guid = Guid::new(
    0x7BF8_0980,
    0xBF32,
    0x101A,
    [0x8B, 0xBB, 0x00, 0xAA, 0x00, 0x30, 0x0C, 0xAB],
);
const IID_IPICTUREDISP: Guid = Guid::new(
    0x7BF8_0981,
    0xBF32,
    0x101A,
    [0x8B, 0xBB, 0x00, 0xAA, 0x00, 0x30, 0x0C, 0xAB],
);
const CLSID_STDPICTURE: Guid = Guid::new(
    0x0BE3_5204,
    0x8F91,
    0x11CE,
    [0x9D, 0xE3, 0x00, 0xAA, 0x00, 0x4B, 0xB8, 0x51],
);

const E_UNEXPECTED: u32 = 0x8000_FFFF;
const DISP_E_UNKNOWNNAME: u32 = super::typelib::DISP_E_UNKNOWNNAME;
const DISP_E_TYPEMISMATCH: u32 = super::variant::DISP_E_TYPEMISMATCH;

/// PICTYPEs.
const PICTYPE_UNINITIALIZED: i16 = -1;
const PICTYPE_NONE: i16 = 0;
const PICTYPE_BITMAP: i16 = 1;
const PICTYPE_METAFILE: i16 = 2;
const PICTYPE_ICON: i16 = 3;
const PICTYPE_ENHMETAFILE: i16 = 4;

/// get_Attributes' flags.
const PICTURE_SCALABLE: u32 = 1;
const PICTURE_TRANSPARENT: u32 = 2;

/// The properties' DISPIDs.
const DISPID_PICT_HANDLE: u32 = 0;
const DISPID_PICT_HPAL: u32 = 2;
const DISPID_PICT_TYPE: u32 = 3;
const DISPID_PICT_WIDTH: u32 = 4;
const DISPID_PICT_HEIGHT: u32 = 5;
const DISPID_PICT_RENDER: u32 = 6;

const DISPATCH_METHOD: u32 = 1;
const VT_I4: u16 = 3;

/// GetDeviceCaps' LOGPIXELSX.
const LOGPIXELSX: u32 = 88;

/// GDI's raster operations and mapping mode.
const SRCCOPY: u32 = 0x00CC_0020;
const SRCAND: u32 = 0x0088_00C6;
const SRCPAINT: u32 = 0x00EE_0086;
const MM_ANISOTROPIC: u32 = 8;
const DI_NORMAL: u32 = 3;
const CBM_INIT: u32 = 4;
const DIB_RGB_COLORS: u32 = 0;

/// A standard picture object.
#[derive(Clone, Debug, Default)]
pub struct Picture {
    pub pid: u32,
    pub refs: u32,
    /// PICTDESC: the type and the handle (the HBITMAP, HMETAFILE, HICON
    /// or HENHMETAFILE), a bitmap's palette.
    pub kind: i16,
    pub handle: u32,
    pub hpal: u32,
    /// A metafile's extents, as PICTDESC gives them.
    pub ext: (i32, i32),
    /// Whether the object deletes the picture when it goes.
    pub own: bool,
    /// The size in pixels, and in HIMETRIC units.
    pub width: i32,
    pub height: i32,
    pub himetric: (i32, i32),
    /// A bitmap with transparent pixels (a GIF's): its mask, 1 where it
    /// is transparent, and its pixels with those black.
    pub mask: u32,
    pub xor: u32,
    pub keep_original_format: bool,
    /// The DC SelectPicture selected the bitmap into, and the bitmap a
    /// new memory DC has (GDI's default one), which goes back into it.
    pub cur_dc: u32,
    pub stock_bitmap: u32,
    /// The interfaces: IPicture, IPictureDisp, IPersistStream,
    /// IConnectionPointContainer, and the connection point.
    pub ifaces: Vec<u32>,
    pub sinks: Vec<u32>,
    /// The file Load read the picture from, which SaveAsFile and Save
    /// write back while the picture hasn't changed since (`dirty`, which
    /// PictureChanged sets).
    pub data: Vec<u8>,
    pub dirty: bool,
}

crate::state_fields!(Picture {
    pid,
    refs,
    kind,
    handle,
    hpal,
    ext,
    own,
    width,
    height,
    himetric,
    mask,
    xor,
    keep_original_format,
    cur_dc,
    stock_bitmap,
    ifaces,
    sinks,
    data,
    dirty
});

/// The interfaces' places in `Picture::ifaces`.
const I_PICTURE: usize = 0;
const I_DISPATCH: usize = 1;
const I_PERSIST: usize = 2;
const I_CONTAINER: usize = 3;
const I_POINT: usize = 4;

/// The vtables of the interfaces before the connection point, in the
/// order of `Picture::ifaces`.
const VTABLES: [&str; 4] = [
    "IPicture",
    "IPictureDisp",
    "IPicturePersistStream",
    "IPictureConnectionPointContainer",
];

fn pictures(cpu: &mut Cpu) -> &mut Vec<Option<Picture>> {
    &mut cpu.bus.win32.oleaut32.fonts.pictures
}

fn get(cpu: &mut Cpu, id: usize) -> &mut Picture {
    pictures(cpu)[id].as_mut().expect("a picture object")
}

/// The picture object of the interface `this`, one of those in `VTABLES`.
fn object(cpu: &mut Cpu, this: u32) -> Option<usize> {
    let (v, id) = com::interface_of(cpu, dll_index(), this)?;
    if !VTABLES.iter().any(|&name| vtable(name) == v) {
        return None;
    }
    pictures(cpu).get(id as usize)?.as_ref()?;
    Some(id as usize)
}

/// The sinks of the object's connection point (connpt.rs).
pub(super) fn sinks(cpu: &mut Cpu, id: usize, point: u32) -> Option<&mut Vec<u32>> {
    let o = pictures(cpu).get_mut(id)?.as_mut()?;
    (point == 0).then_some(&mut o.sinks)
}

/// A process ended: its objects went with its heap.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    for slot in pictures(cpu).iter_mut() {
        if slot.as_ref().is_some_and(|o| o.pid == pid) {
            *slot = None;
        }
    }
}

/// The DPI of the display, x and y.
fn dpi(cpu: &mut Cpu) -> (i32, i32) {
    (
        gdi32::dc::device_cap(cpu, LOGPIXELSX) as i32,
        gdi32::dc::device_cap(cpu, LOGPIXELSY) as i32,
    )
}

/// Pixels to HIMETRIC units at the display's DPI.
fn to_himetric(cpu: &mut Cpu, w: i32, h: i32) -> (i32, i32) {
    let (dx, dy) = dpi(cpu);
    (mul_div(w, 2540, dx), mul_div(h, 2540, dy))
}

fn mul_div(a: i32, b: i32, c: i32) -> i32 {
    super::font::mul_div(a, b, c)
}

/// A new picture object with one reference, of no picture yet
/// (PICTYPE_UNINITIALIZED): its id, or None when memory is out.
fn construct(cpu: &mut Cpu, own: bool) -> Option<usize> {
    let picture = Picture {
        pid: user32::current_pid(cpu),
        refs: 1,
        kind: PICTYPE_UNINITIALIZED,
        own,
        keep_original_format: true,
        stock_bitmap: gdi32::bitmap::default_bitmap(cpu),
        ..Picture::default()
    };
    let id = insert(pictures(cpu), picture);
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
    ifaces.push(connpt::new_point(cpu, connpt::PICTURE, id, 0));
    if ifaces.contains(&None) {
        for at in ifaces.into_iter().flatten() {
            com::free_interface(cpu, at);
        }
        pictures(cpu)[id] = None;
        return None;
    }
    get(cpu, id).ifaces = ifaces.into_iter().flatten().collect();
    Some(id)
}

/// The object's interface `iid` at `out`, with a reference added.
pub(super) fn query(cpu: &mut Cpu, id: usize, iid: &Guid, out: u32) -> u32 {
    let index = match *iid {
        _ if *iid == com::IID_IUNKNOWN || *iid == IID_IPICTURE => I_PICTURE,
        _ if *iid == IID_IDISPATCH || *iid == IID_IPICTUREDISP => I_DISPATCH,
        _ if *iid == IID_IPERSIST || *iid == IID_IPERSISTSTREAM => I_PERSIST,
        _ if *iid == connpt::IID_ICONNECTIONPOINTCONTAINER => I_CONTAINER,
        _ => {
            com::write_out(cpu, out, 0);
            return E_NOINTERFACE;
        }
    };
    let iface = get(cpu, id).ifaces[index];
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
/// release the sinks the gone object held. A picture the object owns is
/// deleted with it.
fn release(cpu: &mut Cpu, id: usize) -> Option<Vec<(u32, u32)>> {
    let o = get(cpu, id);
    o.refs = o.refs.saturating_sub(1);
    if o.refs > 0 {
        return None;
    }
    let o = pictures(cpu)[id].take().expect("a picture object");
    let calls = connpt::release_calls(cpu, o.sinks.iter().copied());
    for &at in &o.ifaces {
        com::free_interface(cpu, at);
    }
    if o.own {
        destroy(cpu, &o);
    }
    Some(calls)
}

/// Delete the picture of `o`.
fn destroy(cpu: &mut Cpu, o: &Picture) {
    match o.kind {
        PICTYPE_BITMAP => {
            // DeleteObject's work: a bitmap still selected into a DC
            // (SelectPicture's) goes when it is selected out.
            for h in [o.handle, o.mask, o.xor] {
                if h != 0 {
                    gdi32::delete_object(cpu, &[h]);
                }
            }
        }
        PICTYPE_METAFILE => {
            gdi32::metafile::delete_meta_file(cpu, &[o.handle]);
        }
        PICTYPE_ICON => {
            user32::resource::destroy_icon(cpu, &[o.handle]);
        }
        _ => {}
    }
}

/// IUnknown::Release's work for any of the object's interfaces.
pub(super) fn release_ref(cpu: &mut Cpu, id: usize) -> Option<u32> {
    match release(cpu, id) {
        None => Some(get(cpu, id).refs),
        Some(calls) => crate::win32::ole32::call_all(cpu, 0, &calls),
    }
}

/// Take the bitmap `handle` as the object's picture: its size from the
/// bitmap.
fn set_bitmap(cpu: &mut Cpu, id: usize, handle: u32) {
    let (w, h) = gdi32::gdi(cpu)
        .bitmap(handle)
        .map_or((0, 0), |b| (b.width, b.height));
    let himetric = to_himetric(cpu, w, h);
    let o = get(cpu, id);
    o.kind = PICTYPE_BITMAP;
    o.handle = handle;
    o.width = w;
    o.height = h;
    o.himetric = himetric;
}

/// Take the icon `handle` as the object's picture, its size `size` in
/// pixels.
fn set_icon(cpu: &mut Cpu, id: usize, handle: u32, (w, h): (i32, i32)) {
    let himetric = to_himetric(cpu, w, h);
    let o = get(cpu, id);
    o.kind = PICTYPE_ICON;
    o.handle = handle;
    o.width = w;
    o.height = h;
    o.himetric = himetric;
}

/// OleCreatePictureIndirect(desc, iid, own, out): a new picture object
/// of the PICTDESC `desc`, or of no picture yet without one, its
/// interface `iid` at `out`. OLEPRO32 exports the function too.
pub fn create_picture_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[desc, iid, own, out] = args else {
        unreachable!()
    };
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(E_POINTER);
    }
    cpu.bus.guest_write_32(out, 0);
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(E_INVALIDARG);
    };
    let Some(id) = construct(cpu, own != 0) else {
        return Some(E_OUTOFMEMORY);
    };
    if desc != 0 {
        let kind = cpu.bus.guest_read_32(desc + 4) as i16;
        let words = [8, 12, 16].map(|o| cpu.bus.guest_read_32(desc + o));
        match kind {
            PICTYPE_BITMAP => {
                set_bitmap(cpu, id, words[0]);
                get(cpu, id).hpal = words[1];
            }
            PICTYPE_METAFILE => {
                let o = get(cpu, id);
                o.kind = kind;
                o.handle = words[0];
                o.ext = (words[1] as i32, words[2] as i32);
                o.himetric = o.ext;
            }
            PICTYPE_ICON => {
                let size =
                    user32::resource::icon(cpu, words[0]).map_or((0, 0), |i| (i.width, i.height));
                set_icon(cpu, id, words[0], size);
            }
            PICTYPE_NONE | PICTYPE_ENHMETAFILE => {
                // An enhanced metafile's frame isn't read: the built-in
                // GDI has no enhanced metafiles yet.
                let o = get(cpu, id);
                o.kind = kind;
                o.handle = words[0];
            }
            _ => {
                get(cpu, id).own = false;
                let calls = release(cpu, id).unwrap_or_default();
                return crate::win32::ole32::call_all(cpu, E_UNEXPECTED, &calls);
            }
        }
    }
    let hr = query(cpu, id, &iid, out);
    let calls = release(cpu, id).unwrap_or_default();
    crate::win32::ole32::call_all(cpu, hr, &calls)
}

/// OleLoadPicture(stream, size, runmode, iid, out): a new picture object
/// of the picture in the stream (IPersistStream::Load), its interface
/// `iid` at `out`. Its KeepOriginalFormat is the opposite of `runmode`.
pub fn load_picture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[stream, _size, runmode, iid, out] = args else {
        unreachable!()
    };
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(E_POINTER);
    }
    cpu.bus.guest_write_32(out, 0);
    if Guid::read(cpu, iid).is_none() {
        return Some(E_INVALIDARG);
    }
    let Some(id) = construct(cpu, true) else {
        return Some(E_OUTOFMEMORY);
    };
    get(cpu, id).keep_original_format = runmode == 0;
    let mut vars = vec![0; V_OUT + 1];
    vars[..3].copy_from_slice(&[super::PICTURE_LOAD, id as u32, stream]);
    vars[V_IID] = iid;
    vars[V_OUT] = out;
    task::start(cpu, task::OLEAUT32_OBJECTS, vars, L_SCRATCH)
}

/// LOAD's variables: the object, the stream, and then those below; and,
/// for OleLoadPicture, the IID and where the interface goes.
const V_STAT_FAILED: usize = 3;
const V_SIZE: usize = 4;
const V_TO_READ: usize = 5;
const V_HEADER_IS_DATA: usize = 6;
const V_BLOCK: usize = 7;
const V_CAPACITY: usize = 8;
const V_READ: usize = 9;
const V_LOADING: usize = 10;
const V_IID: usize = 11;
const V_OUT: usize = 12;

/// LOAD's scratch space: a STATSTG, the 8 bytes of a header, and the
/// count Read gives.
const L_STAT: u32 = 0;
const L_HEADER: u32 = 72;
const L_COUNT: u32 = 80;
const L_SCRATCH: u32 = 84;

/// LOAD's steps.
const STEP_STATED: u16 = 1;
const STEP_HEADER: u16 = 2;
const STEP_ALL: u16 = 3;
const STEP_SIZED: u16 = 4;

/// IStream's Read, Write and Stat.
const READ: u32 = 3;
const WRITE: u32 = 4;
const STAT: u32 = 12;
const STATFLAG_NONAME: u32 = 1;

/// What a stream that can't say its size is read in at first.
const CHUNK: u32 = 4096;

/// IPersistStream::Load(this, stream).
pub fn load(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, stream] = args else {
        unreachable!()
    };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let vars = vec![super::PICTURE_LOAD, id as u32, stream];
    task::start(cpu, task::OLEAUT32_OBJECTS, vars, L_SCRATCH)
}

/// LOAD: the stream's data is read (Wine's OLEPictureImpl_Load): its
/// size from Stat, then headers of 8 bytes until one is the start of a
/// picture's file, its length the "lt" header's if there was one, else
/// the rest of the stream; without a size from Stat, all there is. Then
/// the data is decoded, and for OleLoadPicture the object's interface
/// given.
pub fn load_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let s = t.scratch;
    let stream = t.var(2);
    let read = crate::win32::ole32::method(cpu, stream, READ).unwrap_or(0);
    match t.step {
        0 => {
            let stat = crate::win32::ole32::method(cpu, stream, STAT).unwrap_or(0);
            Next::call(stat, &[stream, s + L_STAT, STATFLAG_NONAME], STEP_STATED)
        }
        STEP_STATED => {
            let failed = value != S_OK;
            t.set(V_STAT_FAILED, failed as u32);
            let size = if failed {
                8
            } else {
                let high = cpu.bus.guest_read_32(s + L_STAT + 12);
                let low = cpu.bus.guest_read_32(s + L_STAT + 8);
                if high != 0 { u32::MAX } else { low }
            };
            t.set(V_SIZE, size);
            t.set(V_LOADING, 1);
            Next::call(read, &[stream, s + L_HEADER, 8, s + L_COUNT], STEP_HEADER)
        }
        STEP_HEADER => {
            let count = cpu.bus.guest_read_32(s + L_COUNT);
            if value != S_OK || count != 8 {
                return finish(cpu, t, if value != S_OK { value } else { E_FAIL });
            }
            header_step(cpu, t, read)
        }
        STEP_ALL => {
            let n = cpu.bus.guest_read_32(s + L_COUNT);
            let got = t.var(V_READ) + n;
            t.set(V_READ, got);
            if value != S_OK || n == 0 {
                return decoded(cpu, t);
            }
            let (block, capacity) = (t.var(V_BLOCK), t.var(V_CAPACITY));
            if got == capacity {
                let bigger = capacity.saturating_mul(2);
                let new = task_alloc(cpu, bigger);
                if new == 0 {
                    return finish(cpu, t, E_OUTOFMEMORY);
                }
                let mut bytes = vec![0; got as usize];
                cpu.bus.guest_read_bytes(block, &mut bytes);
                cpu.bus.guest_write_bytes(new, &bytes);
                task_free(cpu, block);
                t.set(V_BLOCK, new);
                t.set(V_CAPACITY, bigger);
            }
            let (block, capacity) = (t.var(V_BLOCK), t.var(V_CAPACITY));
            Next::call(
                read,
                &[stream, block + got, capacity - got, s + L_COUNT],
                STEP_ALL,
            )
        }
        _ => {
            let n = cpu.bus.guest_read_32(s + L_COUNT);
            let got = t.var(V_READ) + n;
            t.set(V_READ, got);
            let len = t.var(V_CAPACITY);
            if value != S_OK || n == 0 || got >= len {
                return decoded(cpu, t);
            }
            let block = t.var(V_BLOCK);
            Next::call(
                read,
                &[stream, block + got, len - got, s + L_COUNT],
                STEP_SIZED,
            )
        }
    }
}

/// The 8 bytes of a header read: another header to read, or the data.
fn header_step(cpu: &mut Cpu, t: &mut Task, read: u32) -> Next {
    let s = t.scratch;
    let stream = t.var(2);
    let mut header = [0u8; 8];
    cpu.bus.guest_read_bytes(s + L_HEADER, &mut header);
    let h0 = u32::from_le_bytes(header[..4].try_into().unwrap());
    let h1 = u32::from_le_bytes(header[4..].try_into().unwrap());
    let failed = t.var(V_STAT_FAILED) != 0;
    let size = t.var(V_SIZE);
    let header_read = t.var(V_LOADING) * 8;
    t.set(V_LOADING, t.var(V_LOADING) + 1);
    let mut to_read = t.var(V_TO_READ);
    if &header[..4] == b"lt\0\0"
        && (failed || u64::from(h1) + u64::from(header_read) <= u64::from(size))
    {
        to_read = h1;
        t.set(V_TO_READ, to_read);
        if failed {
            t.set(V_SIZE, h1.saturating_add(8));
            t.set(V_STAT_FAILED, 0);
        }
        if to_read == 0 {
            return data_step(cpu, t, false, read);
        }
        return Next::call(read, &[stream, s + L_HEADER, 8, s + L_COUNT], STEP_HEADER);
    }
    let is_data = &header[..4] == b"GIF8"
        || &header[..2] == b"BM"
        || header[..2] == [0xFF, 0xD8]
        || h0 == 1
        || h0 == 0x10000
        || h0 == 0x20000
        || h1 > t.var(V_SIZE)
        || h1 == 0;
    if is_data {
        let to_read = if to_read == 0 {
            t.var(V_SIZE).saturating_sub(8)
        } else {
            to_read.saturating_sub(8)
        };
        t.set(V_TO_READ, to_read);
        return data_step(cpu, t, true, read);
    }
    // A header of a kind not known: the data is the length it gives
    // after it, as Wine takes it.
    t.set(V_TO_READ, h1);
    data_step(cpu, t, false, read)
}

/// The headers read: the block the data goes into, with the last header
/// at its start when that was the data's, and the reads of the rest.
fn data_step(cpu: &mut Cpu, t: &mut Task, header_is_data: bool, read: u32) -> Next {
    let s = t.scratch;
    let stream = t.var(2);
    t.set(V_HEADER_IS_DATA, header_is_data as u32);
    let failed = t.var(V_STAT_FAILED) != 0;
    let len = if failed {
        CHUNK
    } else {
        t.var(V_TO_READ)
            .saturating_add(if header_is_data { 8 } else { 0 })
    };
    if len == 0 {
        // The "NONE" picture.
        if let Some(id) = task_picture(cpu, t) {
            get(cpu, id).kind = PICTYPE_NONE;
        }
        return finish(cpu, t, S_OK);
    }
    let block = task_alloc(cpu, len);
    if block == 0 {
        return finish(cpu, t, E_OUTOFMEMORY);
    }
    t.set(V_BLOCK, block);
    t.set(V_CAPACITY, len);
    let mut got = 0;
    if header_is_data {
        let mut header = [0u8; 8];
        cpu.bus.guest_read_bytes(s + L_HEADER, &mut header);
        let n = 8.min(len) as usize;
        cpu.bus.guest_write_bytes(block, &header[..n]);
        got = n as u32;
    }
    t.set(V_READ, got);
    if !failed && got >= len {
        return decoded(cpu, t);
    }
    let step = if failed { STEP_ALL } else { STEP_SIZED };
    Next::call(read, &[stream, block + got, len - got, s + L_COUNT], step)
}

/// The picture object of a task's variable 1, if it is still there.
fn task_picture(cpu: &mut Cpu, t: &Task) -> Option<usize> {
    let id = t.var(1) as usize;
    pictures(cpu).get(id)?.as_ref()?;
    Some(id)
}

/// The data read: decode it, free the block, and finish.
fn decoded(cpu: &mut Cpu, t: &mut Task) -> Next {
    let (block, len) = (t.var(V_BLOCK), t.var(V_READ));
    let hr = match task_picture(cpu, t) {
        Some(id) if len == 0 => {
            get(cpu, id).kind = PICTYPE_NONE;
            S_OK
        }
        Some(id) => {
            let hr = decode(cpu, id, block, len);
            let mut data = vec![0u8; len as usize];
            cpu.bus.guest_read_bytes(block, &mut data);
            let o = get(cpu, id);
            o.data = if hr == S_OK { data } else { Vec::new() };
            o.dirty = false;
            hr
        }
        None => E_FAIL,
    };
    task_free(cpu, block);
    t.set(V_BLOCK, 0);
    finish(cpu, t, hr)
}

/// The load is done with `hr`: for OleLoadPicture the object's interface
/// is given, or the object goes when the load failed.
fn finish(cpu: &mut Cpu, t: &mut Task, hr: u32) -> Next {
    if t.var(V_BLOCK) != 0 {
        task_free(cpu, t.var(V_BLOCK));
        t.set(V_BLOCK, 0);
    }
    if t.vars.len() <= V_OUT {
        return Next::Done(hr);
    }
    let Some(id) = task_picture(cpu, t) else {
        return Next::Done(E_FAIL);
    };
    let hr = if hr >> 31 == 0 {
        match Guid::read(cpu, t.var(V_IID)) {
            Some(iid) => query(cpu, id, &iid, t.var(V_OUT)),
            None => E_INVALIDARG,
        }
    } else {
        hr
    };
    // The object's own reference goes: with a failed load, or without
    // the interface asked for, the object with it. No program has had
    // the object yet, so no sink is advised to release.
    release(cpu, id);
    Next::Done(hr)
}

/// Decode the picture's file of `len` bytes at `at` into the object: a
/// BMP, JPEG or GIF file as a bitmap, an icon or cursor file as an icon,
/// a placeable metafile as a metafile.
fn decode(cpu: &mut Cpu, id: usize, at: u32, len: u32) -> u32 {
    let mut data = vec![0u8; len as usize];
    cpu.bus.guest_read_bytes(at, &mut data);
    let magic = u16::from_le_bytes([data[0], *data.get(1).unwrap_or(&0)]);
    match magic {
        0x4947 => decode_rgba(cpu, id, decode_gif(&data)),
        0xD8FF => decode_rgba(cpu, id, decode_jpeg(&data)),
        0x4D42 => decode_bmp(cpu, id, at, &data),
        0xCDD7 => decode_placeable(cpu, id, at, &data),
        0x0000 => decode_icon(cpu, id, at, &data),
        _ => {
            let emf = data.len() >= 44 && data[..4] == [1, 0, 0, 0] && &data[40..44] == b" EMF";
            let what = if emf {
                "an enhanced metafile, which the built-in GDI hasn't yet"
            } else {
                "of a format it doesn't know"
            };
            cpu.bus.log_string(&format!(
                "[W32] OLEAUT32: a picture's data is {what} ({len} bytes): the load fails"
            ));
            E_FAIL
        }
    }
}

/// A picture decoded: its size and its pixels, RGBA, the rows top first.
pub(super) struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

fn decode_jpeg(data: &[u8]) -> Result<Rgba, String> {
    use jpeg_decoder::PixelFormat;
    let mut decoder = jpeg_decoder::Decoder::new(data);
    let pixels = decoder.decode().map_err(|e| e.to_string())?;
    let info = decoder.info().ok_or("the JPEG has no header")?;
    let pixels = match info.pixel_format {
        PixelFormat::RGB24 => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        PixelFormat::L8 => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        PixelFormat::L16 => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[1], p[1], p[1], 255])
            .collect(),
        PixelFormat::CMYK32 => pixels
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                let k = 255 - p[3] as u32;
                let c = |v: u8| ((255 - v as u32) * k / 255) as u8;
                [c(p[0]), c(p[1]), c(p[2]), 255]
            })
            .collect(),
    };
    Ok(Rgba {
        width: info.width as u32,
        height: info.height as u32,
        pixels,
    })
}

/// A GIF's first picture on its logical screen; what the picture doesn't
/// cover, and its transparent color, transparent.
pub(super) fn decode_gif(data: &[u8]) -> Result<Rgba, String> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options
        .read_info(std::io::Cursor::new(data))
        .map_err(|e| e.to_string())?;
    let (width, height) = (decoder.width() as u32, decoder.height() as u32);
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let frame = decoder
        .read_next_frame()
        .map_err(|e| e.to_string())?
        .ok_or("the GIF has no pictures")?;
    for y in 0..frame.height as u32 {
        for x in 0..frame.width as u32 {
            let (cx, cy) = (x + frame.left as u32, y + frame.top as u32);
            if cx < width && cy < height {
                let from = ((y * frame.width as u32 + x) * 4) as usize;
                let to = ((cy * width + cx) * 4) as usize;
                pixels[to..to + 4].copy_from_slice(&frame.buffer[from..from + 4]);
            }
        }
    }
    Ok(Rgba {
        width,
        height,
        pixels,
    })
}

/// A packed 32-bit DIB of `width` by `height` pixels, top-down, in a
/// block of the task allocator: its address, or 0 when memory is out.
fn packed_dib(cpu: &mut Cpu, width: u32, height: u32, bgra: &[u8]) -> u32 {
    let block = task_alloc(cpu, 40 + bgra.len() as u32);
    if block == 0 {
        return 0;
    }
    let mut header = vec![0u8; 40];
    header[0..4].copy_from_slice(&40u32.to_le_bytes());
    header[4..8].copy_from_slice(&width.to_le_bytes());
    header[8..12].copy_from_slice(&(-(height as i32)).to_le_bytes());
    header[12..14].copy_from_slice(&1u16.to_le_bytes());
    header[14..16].copy_from_slice(&32u16.to_le_bytes());
    cpu.bus.guest_write_bytes(block, &header);
    cpu.bus.guest_write_bytes(block + 40, bgra);
    block
}

/// A bitmap like the display's of the packed DIB at `header`, its bits at
/// `bits`, as CreateDIBitmap makes it on a DC of the screen.
fn display_bitmap(cpu: &mut Cpu, header: u32, bits: u32) -> u32 {
    let hdc = user32::paint::get_dc(cpu, &[0]).unwrap_or(0);
    let bitmap = gdi32::bitmap::create_di_bitmap(
        cpu,
        &[hdc, header, CBM_INIT, bits, header, DIB_RGB_COLORS],
    )
    .unwrap_or(0);
    if hdc != 0 {
        user32::paint::release_dc(cpu, &[0, hdc]);
    }
    bitmap
}

/// A decoded picture as the object's bitmap. Transparent pixels (a GIF's)
/// make a mask and a copy with those black, as Render draws it.
fn decode_rgba(cpu: &mut Cpu, id: usize, rgba: Result<Rgba, String>) -> u32 {
    let rgba = match rgba {
        Ok(r) if r.width > 0 && r.height > 0 => r,
        Ok(_) => return E_FAIL,
        Err(e) => {
            cpu.bus
                .log_string(&format!("[W32] OLEAUT32: a picture didn't decode: {e}"));
            return E_FAIL;
        }
    };
    let (w, h) = (rgba.width, rgba.height);
    let bgra: Vec<u8> = rgba
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0], 0])
        .collect();
    let Some(bitmap) = bitmap_of(cpu, w, h, &bgra) else {
        return E_OUTOFMEMORY;
    };
    set_bitmap(cpu, id, bitmap);
    let transparent: Vec<bool> = rgba
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p[3] == 0)
        .collect();
    if transparent.iter().any(|&t| t) {
        // The mask: monochrome, 1 (white) where it is transparent, rows
        // of whole words.
        let stride = (w as usize).div_ceil(16) * 2;
        let mut bits = vec![0u8; stride * h as usize];
        for (i, _) in transparent.iter().enumerate().filter(|(_, t)| **t) {
            let (x, y) = (i % w as usize, i / w as usize);
            bits[y * stride + x / 8] |= 0x80 >> (x % 8);
        }
        let mask = gdi32::bitmap::new_ddb_with(cpu, w as i32, h as i32, 1, &bits);
        let black: Vec<u8> = bgra
            .as_chunks::<4>()
            .0
            .iter()
            .zip(&transparent)
            .flat_map(|(p, &t)| if t { [0; 4] } else { [p[0], p[1], p[2], 0] })
            .collect();
        let xor = bitmap_of(cpu, w, h, &black).unwrap_or(0);
        let o = get(cpu, id);
        o.mask = mask;
        o.xor = xor;
    }
    S_OK
}

/// A display bitmap of 32-bit pixels: None when memory is out.
fn bitmap_of(cpu: &mut Cpu, w: u32, h: u32, bgra: &[u8]) -> Option<u32> {
    let block = packed_dib(cpu, w, h, bgra);
    if block == 0 {
        return None;
    }
    let bitmap = display_bitmap(cpu, block, block + 40);
    task_free(cpu, block);
    (bitmap != 0).then_some(bitmap)
}

/// A BMP file, at `at` in the program's memory: its DIB as a bitmap of
/// the display's.
fn decode_bmp(cpu: &mut Cpu, id: usize, at: u32, data: &[u8]) -> u32 {
    if data.len() < 14 + 12 {
        return E_FAIL;
    }
    let bits = u32::from_le_bytes(data[10..14].try_into().unwrap());
    if bits as usize > data.len() {
        return E_FAIL;
    }
    let bitmap = display_bitmap(cpu, at + 14, at + bits);
    if bitmap == 0 {
        return E_FAIL;
    }
    set_bitmap(cpu, id, bitmap);
    S_OK
}

/// An icon or cursor file, at `at`: its 32-pixel-wide image if it has
/// several, else its first, as an icon (a cursor's with its hot spot).
fn decode_icon(cpu: &mut Cpu, id: usize, at: u32, data: &[u8]) -> u32 {
    let word = |i: usize| u16::from_le_bytes([data[i], data[i + 1]]);
    if data.len() < 6 {
        return E_FAIL;
    }
    let (kind, count) = (word(2), word(4) as usize);
    if count == 0 || data.len() < 6 + 16 * count {
        return E_FAIL;
    }
    let entry = |i: usize| &data[6 + 16 * i..6 + 16 * (i + 1)];
    let i = if count == 1 {
        0
    } else {
        (0..count).find(|&i| entry(i)[0] == 32).unwrap_or(0)
    };
    let e = entry(i);
    let (w, h) = (e[0] as u32, e[1] as u32);
    let size = u32::from_le_bytes(e[8..12].try_into().unwrap());
    let offset = u32::from_le_bytes(e[12..16].try_into().unwrap());
    if u64::from(offset) + u64::from(size) > data.len() as u64 {
        return E_FAIL;
    }
    let icon = if kind == 2 {
        // A cursor's resource starts with its hot spot.
        let block = task_alloc(cpu, size + 4);
        if block == 0 {
            return E_OUTOFMEMORY;
        }
        cpu.bus.guest_write_bytes(block, &e[4..8]);
        let mut bytes = vec![0u8; size as usize];
        cpu.bus.guest_read_bytes(at + offset, &mut bytes);
        cpu.bus.guest_write_bytes(block + 4, &bytes);
        let icon = user32::resource::create_icon_from_resource_ex(
            cpu,
            &[block, size + 4, 0, 0x0003_0000, w, h, 0],
        );
        task_free(cpu, block);
        icon
    } else {
        user32::resource::create_icon_from_resource_ex(
            cpu,
            &[at + offset, size, 1, 0x0003_0000, w, h, 0],
        )
    };
    match icon {
        Some(icon) if icon != 0 => {
            set_icon(cpu, id, icon, (w as i32, h as i32));
            S_OK
        }
        _ => E_FAIL,
    }
}

/// A placeable metafile (APM): its records after the 22-byte header as a
/// metafile, its size the header's at its units an inch.
fn decode_placeable(cpu: &mut Cpu, id: usize, at: u32, data: &[u8]) -> u32 {
    const HEADER: usize = 22;
    if data.len() < HEADER || data[..4] != [0xD7, 0xCD, 0xC6, 0x9A] {
        return E_FAIL;
    }
    let short = |i: usize| i16::from_le_bytes([data[i], data[i + 1]]) as i32;
    let (left, top, right, bottom, inch) = (short(6), short(8), short(10), short(12), short(14));
    let size = (data.len() - HEADER) as u32;
    let handle =
        gdi32::metafile::set_meta_file_bits_ex(cpu, &[size, at + HEADER as u32]).unwrap_or(0);
    if handle == 0 {
        return E_FAIL;
    }
    let o = get(cpu, id);
    o.kind = PICTYPE_METAFILE;
    o.handle = handle;
    o.ext = (0, 0);
    o.width = 0;
    o.height = 0;
    o.himetric = (
        mul_div(right - left, 2540, inch),
        mul_div(bottom - top, 2540, inch),
    );
    S_OK
}

// IUnknown, for every interface of the object but the connection
// point's (connpt.rs).

pub fn query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, iid, out] = args else {
        unreachable!()
    };
    if out == 0 {
        return Some(E_INVALIDARG);
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

// IPicture.

/// A getter: what `value` gives of the object, at the out pointer
/// (args[1]).
fn get_32(cpu: &mut Cpu, args: &[u32], value: fn(&Picture) -> Result<u32, u32>) -> Option<u32> {
    let Some(id) = object(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    match value(get(cpu, id)) {
        Ok(v) => {
            com::write_out(cpu, args[1], v);
            Some(S_OK)
        }
        Err(e) => Some(e),
    }
}

pub fn get_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_32(cpu, args, |o| {
        Ok(match o.kind {
            PICTYPE_NONE | PICTYPE_UNINITIALIZED => 0,
            _ => o.handle,
        })
    })
}

/// get_hPal(this, out): a bitmap's palette; E_FAIL for any other type,
/// which Wine's tests check on Windows for each PICTYPE.
pub fn get_hpal(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_32(cpu, args, |o| {
        if o.kind == PICTYPE_BITMAP {
            Ok(o.hpal)
        } else {
            Err(E_FAIL)
        }
    })
}

pub fn get_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 || !cpu.bus.guest_probe(args[1], 2, true) {
        return Some(E_POINTER);
    }
    let kind = get(cpu, id).kind;
    cpu.bus.guest_write_16(args[1], kind as u16);
    Some(S_OK)
}

pub fn get_width(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_32(cpu, args, |o| Ok(o.himetric.0 as u32))
}

pub fn get_height(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_32(cpu, args, |o| Ok(o.himetric.1 as u32))
}

/// Render(this, hdc, x, y, cx, cy, xsrc, ysrc, cxsrc, cysrc, bounds):
/// the part of the picture from (xsrc, ysrc), cxsrc by cysrc in
/// HIMETRIC units with y counted up from the bottom, drawn at (x, y), cx
/// by cy in the DC's logical units.
pub fn render(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, hdc, x, y, cx, cy, xs, ys, cxs, cys, _bounds] = args else {
        unreachable!()
    };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if cx == 0 || cy == 0 || cxs == 0 || cys == 0 {
        return Some(CTL_E_INVALIDPROPERTYVALUE);
    }
    let o = get(cpu, id).clone();
    let dst = [x, y, cx, cy];
    let src = [xs, ys, cxs, cys].map(|v| v as i32);
    match o.kind {
        PICTYPE_UNINITIALIZED | PICTYPE_NONE => {}
        PICTYPE_BITMAP => {
            let (mask, xor) = if o.mask != 0 {
                (o.mask, o.xor)
            } else {
                (0, o.handle)
            };
            render_masked(cpu, &o, hdc, dst, src, mask, xor);
        }
        PICTYPE_ICON => {
            // The whole icon, stretched to the destination, as Wine
            // draws it: the source rectangle isn't used.
            user32::paint::draw_icon_ex(cpu, &[hdc, x, y, o.handle, cx, cy, 0, 0, DI_NORMAL]);
        }
        PICTYPE_METAFILE => render_metafile(cpu, &o, hdc, dst, src),
        _ => {
            cpu.bus.log_string(
                "[W32] OLEAUT32: IPicture::Render of an enhanced metafile, which the \
                 built-in GDI hasn't yet",
            );
            return Some(E_NOTIMPL);
        }
    }
    Some(S_OK)
}

/// Draw a bitmap picture from a memory DC whose logical units are the
/// picture's HIMETRIC ones, y up: with a mask, the mask ANDed and the
/// bitmap with the masked pixels black ORed on, else the bitmap copied.
fn render_masked(
    cpu: &mut Cpu,
    o: &Picture,
    hdc: u32,
    [x, y, cx, cy]: [u32; 4],
    [xs, ys, cxs, cys]: [i32; 4],
    mask: u32,
    xor: u32,
) {
    let Some(dc) = gdi32::dc::create_compatible_dc(cpu, &[0]).filter(|&d| d != 0) else {
        return;
    };
    let (hw, hh) = o.himetric;
    gdi32::dc::set_map_mode(cpu, &[dc, MM_ANISOTROPIC]);
    gdi32::dc::set_window_org_ex(cpu, &[dc, 0, 0, 0]);
    gdi32::dc::set_window_ext_ex(cpu, &[dc, hw as u32, hh as u32, 0]);
    gdi32::dc::set_viewport_org_ex(cpu, &[dc, 0, o.height as u32, 0]);
    gdi32::dc::set_viewport_ext_ex(cpu, &[dc, o.width as u32, (-o.height) as u32, 0]);
    let blit = |cpu: &mut Cpu, ys: i32, rop: u32| {
        gdi32::draw::stretch_blt(
            cpu,
            &[
                hdc, x, y, cx, cy, dc, xs as u32, ys as u32, cxs as u32, cys as u32, rop,
            ],
        );
    };
    let old = if mask != 0 {
        let bk = gdi32::dc::set_bk_color(cpu, &[hdc, 0x00FF_FFFF]).unwrap_or(0);
        let text = gdi32::dc::set_text_color(cpu, &[hdc, 0]).unwrap_or(0);
        let old = gdi32::dc::select_object(cpu, &[dc, mask]).unwrap_or(0);
        blit(cpu, ys, SRCAND);
        gdi32::dc::select_object(cpu, &[dc, xor]);
        blit(cpu, ys, SRCPAINT);
        gdi32::dc::set_bk_color(cpu, &[hdc, bk]);
        gdi32::dc::set_text_color(cpu, &[hdc, text]);
        old
    } else {
        let old = gdi32::dc::select_object(cpu, &[dc, xor]).unwrap_or(0);
        blit(cpu, ys, SRCCOPY);
        old
    };
    if old != 0 {
        gdi32::dc::select_object(cpu, &[dc, old]);
    }
    gdi32::dc::delete_dc(cpu, &[dc]);
}

/// Play a metafile picture into the DC, its window the source rectangle
/// (y up) and its viewport the destination, the DC's mapping put back
/// after.
fn render_metafile(cpu: &mut Cpu, o: &Picture, hdc: u32, dst: [u32; 4], src: [i32; 4]) {
    let [x, y, cx, cy] = dst;
    let [xs, ys, cxs, cys] = src;
    let scratch = task_alloc(cpu, 32);
    if scratch == 0 {
        return;
    }
    let point = |cpu: &mut Cpu, i: u32| {
        (
            cpu.bus.guest_read_32(scratch + 8 * i),
            cpu.bus.guest_read_32(scratch + 8 * i + 4),
        )
    };
    let mode = gdi32::dc::set_map_mode(cpu, &[hdc, MM_ANISOTROPIC]).unwrap_or(0);
    gdi32::dc::set_window_org_ex(cpu, &[hdc, xs as u32, (o.himetric.1 - ys) as u32, scratch]);
    gdi32::dc::set_window_ext_ex(cpu, &[hdc, cxs as u32, (-cys) as u32, scratch + 8]);
    gdi32::dc::set_viewport_org_ex(cpu, &[hdc, x, y, scratch + 16]);
    gdi32::dc::set_viewport_ext_ex(cpu, &[hdc, cx, cy, scratch + 24]);
    gdi32::metafile::play_meta_file(cpu, &[hdc, o.handle]);
    let (wox, woy) = point(cpu, 0);
    let (wex, wey) = point(cpu, 1);
    let (vox, voy) = point(cpu, 2);
    let (vex, vey) = point(cpu, 3);
    gdi32::dc::set_window_ext_ex(cpu, &[hdc, wex, wey, 0]);
    gdi32::dc::set_window_org_ex(cpu, &[hdc, wox, woy, 0]);
    gdi32::dc::set_viewport_ext_ex(cpu, &[hdc, vex, vey, 0]);
    gdi32::dc::set_viewport_org_ex(cpu, &[hdc, vox, voy, 0]);
    gdi32::dc::set_map_mode(cpu, &[hdc, mode]);
    task_free(cpu, scratch);
}

/// set_hPal(this, hpal): a bitmap's palette.
pub fn set_hpal(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let o = get(cpu, id);
    if o.kind != PICTYPE_BITMAP {
        return Some(E_FAIL);
    }
    o.hpal = args[1];
    changed(cpu, id, DISPID_PICT_HPAL)
}

/// A property changed: the sinks hear of it.
fn changed(cpu: &mut Cpu, id: usize, dispid: u32) -> Option<u32> {
    let sinks = get(cpu, id).sinks.clone();
    connpt::notify(cpu, connpt::PICTURE, id, dispid, &sinks, &[], "")
}

pub fn get_cur_dc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let dc = get(cpu, id).cur_dc;
    com::write_out(cpu, args[1], dc);
    Some(S_OK)
}

/// SelectPicture(this, hdc, old_dc, bitmap): a bitmap selected into the
/// DC `hdc`, out of the one it was in before, which gets the bitmap a new
/// memory DC has, as Wine does it. The default bitmap can be in any
/// number of DCs, so it can go into each DC the picture leaves.
pub fn select_picture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, hdc, old_dc, out] = args else {
        unreachable!()
    };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let o = get(cpu, id).clone();
    if o.kind != PICTYPE_BITMAP {
        return Some(E_FAIL);
    }
    com::write_out(cpu, old_dc, o.cur_dc);
    if o.cur_dc != 0 {
        gdi32::dc::select_object(cpu, &[o.cur_dc, o.stock_bitmap]);
    }
    if hdc != 0 {
        gdi32::dc::select_object(cpu, &[hdc, o.handle]);
    }
    get(cpu, id).cur_dc = hdc;
    com::write_out(cpu, out, o.handle);
    Some(S_OK)
}

pub fn get_keep_original_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_32(cpu, args, |o| Ok(o.keep_original_format as u32))
}

pub fn put_keep_original_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    get(cpu, id).keep_original_format = args[1] != 0;
    Some(S_OK)
}

pub fn picture_changed(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    get(cpu, id).dirty = true;
    changed(cpu, id, DISPID_PICT_HANDLE)
}

/// The file of the picture `id` that SaveAsFile and Save write: the one
/// Load read, while the picture is as it made it. Ok(None) for no
/// picture; Err(E_NOTIMPL), with a log line, for a picture made from a
/// handle or changed since, whose file would be made from the handle,
/// which isn't written yet.
fn file_of(cpu: &mut Cpu, id: usize, what: &str) -> Result<Option<Vec<u8>>, u32> {
    let o = get(cpu, id);
    if o.kind == PICTYPE_NONE {
        return Ok(None);
    }
    if !o.data.is_empty() && !o.dirty {
        return Ok(Some(o.data.clone()));
    }
    let kind = o.kind;
    cpu.bus.log_string(&format!(
        "[W32] OLEAUT32: {what} of a picture (type {kind}) made from a handle, or changed \
         since it was loaded, isn't written yet: E_NOTIMPL"
    ));
    Err(E_NOTIMPL)
}

/// Start SAVE: `bytes` written to the stream with IStream::Write, the
/// count written to `size` if it isn't 0.
fn write_task(cpu: &mut Cpu, stream: u32, bytes: &[u8], size: u32) -> Option<u32> {
    let block = task_alloc(cpu, bytes.len().max(1) as u32);
    if block == 0 {
        return Some(E_OUTOFMEMORY);
    }
    cpu.bus.guest_write_bytes(block, bytes);
    let vars = vec![super::PICTURE_SAVE, stream, block, bytes.len() as u32, size];
    task::start(cpu, task::OLEAUT32_OBJECTS, vars, 4)
}

/// SAVE: the bytes written, the block freed, and the count given.
pub fn save_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let s = t.scratch;
    let (stream, block, len, size) = (t.var(1), t.var(2), t.var(3), t.var(4));
    match t.step {
        0 => {
            cpu.bus.guest_write_32(s, 0);
            let write = crate::win32::ole32::method(cpu, stream, WRITE).unwrap_or(0);
            Next::call(write, &[stream, block, len, s], 1)
        }
        _ => {
            task_free(cpu, block);
            if value == S_OK && size != 0 && cpu.bus.guest_probe(size, 4, true) {
                let written = cpu.bus.guest_read_32(s);
                cpu.bus.guest_write_32(size, written);
            }
            Next::Done(value)
        }
    }
}

/// SaveAsFile(this, stream, save_copy, size): the picture's file written
/// to the stream, as Wine does (and its tests of Windows check): nothing
/// for no picture, and E_FAIL without `save_copy` for a picture made
/// from a handle. A picture loaded from a file writes that file back
/// whatever `save_copy` is, which Visual Basic's runtime counts on: it
/// asks whether the original format is kept and then saves the form's
/// icon without the flag.
pub fn save_as_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, stream, save_copy, size] = args else {
        unreachable!()
    };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if stream == 0 {
        return Some(E_POINTER);
    }
    let o = get(cpu, id);
    let kept = !o.data.is_empty() && !o.dirty;
    if o.kind != PICTYPE_NONE && !kept && save_copy == 0 {
        return Some(E_FAIL);
    }
    match file_of(cpu, id, "IPicture::SaveAsFile") {
        Ok(None) => Some(S_OK),
        Ok(Some(bytes)) => write_task(cpu, stream, &bytes, size),
        Err(e) => Some(e),
    }
}

pub fn get_attributes(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_32(cpu, args, |o| {
        Ok(match o.kind {
            PICTYPE_BITMAP if o.mask != 0 => PICTURE_TRANSPARENT,
            PICTYPE_ICON => PICTURE_TRANSPARENT,
            PICTYPE_METAFILE | PICTYPE_ENHMETAFILE => PICTURE_TRANSPARENT | PICTURE_SCALABLE,
            _ => 0,
        })
    })
}

// IPictureDisp.

pub fn get_type_info_count(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let found = object(cpu, args[0]).is_some();
    super::font::get_type_info_count_of(cpu, found, args[1])
}

/// GetTypeInfo: STDOLE2.TLB's IPictureDisp, which the built-in library
/// doesn't have yet (stdole.rs).
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

/// IPictureDisp's names, by their DISPIDs, as STDOLE2.TLB has them.
const NAMES: &[(&str, u32)] = &[
    ("Handle", DISPID_PICT_HANDLE),
    ("hPal", DISPID_PICT_HPAL),
    ("Type", DISPID_PICT_TYPE),
    ("Width", DISPID_PICT_WIDTH),
    ("Height", DISPID_PICT_HEIGHT),
    ("Render", DISPID_PICT_RENDER),
];

pub fn get_ids_of_names(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if object(cpu, args[0]).is_none() {
        return Some(E_INVALIDARG);
    }
    super::font::ids_of_names(cpu, args, NAMES)
}

/// Invoke(this, dispid, iid, lcid, flags, params, result, excepinfo,
/// argerr): a property got, hPal put, or Render called.
pub fn invoke(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, dispid, iid, _lcid, flags, params, result, _, _] = args else {
        unreachable!()
    };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if Guid::read(cpu, iid) != Some(com::GUID_NULL) {
        return Some(DISP_E_UNKNOWNNAME);
    }
    if params == 0 {
        return Some(DISP_E_PARAMNOTOPTIONAL);
    }
    let count = cpu.bus.guest_read_32(params + 8);
    let picture = get(cpu, id).ifaces[I_PICTURE];
    if flags & DISPATCH_PROPERTYGET != 0 {
        if count != 0 {
            return Some(DISP_E_BADPARAMCOUNT);
        }
        if result == 0 {
            return Some(DISP_E_PARAMNOTOPTIONAL);
        }
        let (vt, hr) = match dispid {
            DISPID_PICT_HANDLE => (VT_I4, get_handle(cpu, &[picture, result + 8])?),
            DISPID_PICT_HPAL => (VT_I4, get_hpal(cpu, &[picture, result + 8])?),
            DISPID_PICT_TYPE => (VT_I2, get_type(cpu, &[picture, result + 8])?),
            DISPID_PICT_WIDTH => (VT_I4, get_width(cpu, &[picture, result + 8])?),
            DISPID_PICT_HEIGHT => (VT_I4, get_height(cpu, &[picture, result + 8])?),
            _ => return Some(DISP_E_MEMBERNOTFOUND),
        };
        cpu.bus.guest_write_16(result, vt);
        return Some(hr);
    }
    if flags & DISPATCH_PROPERTYPUT != 0 {
        if count != 1 {
            return Some(DISP_E_BADPARAMCOUNT);
        }
        if dispid != DISPID_PICT_HPAL {
            return Some(DISP_E_MEMBERNOTFOUND);
        }
        let arg = cpu.bus.guest_read_32(params);
        return match super::variant::change_from(cpu, arg, VT_I4, 0) {
            Ok(super::variant::Out::Bits(_, v)) => set_hpal(cpu, &[picture, v as u32]),
            Ok(_) => Some(DISP_E_TYPEMISMATCH),
            Err(e) => Some(e),
        };
    }
    if flags & DISPATCH_METHOD != 0 && dispid == DISPID_PICT_RENDER {
        if count != 10 {
            return Some(DISP_E_BADPARAMCOUNT);
        }
        // Each argument a VT_I4, the last first; the bounds rectangle
        // isn't passed on.
        let at = cpu.bus.guest_read_32(params);
        let mut values = [0u32; 10];
        for (i, v) in values.iter_mut().enumerate() {
            let arg = at + 16 * i as u32;
            if cpu.bus.guest_read_16(arg) != VT_I4 {
                return Some(DISP_E_TYPEMISMATCH);
            }
            *v = cpu.bus.guest_read_32(arg + 8);
        }
        let mut call = vec![picture];
        call.extend(values[1..].iter().rev());
        call.push(0);
        return render(cpu, &call);
    }
    Some(DISP_E_MEMBERNOTFOUND)
}

// IPersistStream.

pub fn get_class_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if object(cpu, args[0]).is_none() {
        return Some(E_INVALIDARG);
    }
    Some(if CLSID_STDPICTURE.write(cpu, args[1]) {
        S_OK
    } else {
        E_POINTER
    })
}

/// IsDirty: E_NOTIMPL, as Wine has it.
pub fn is_dirty(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(E_NOTIMPL)
}

/// Save(this, stream, clear_dirty): the "lt" header, which Load takes,
/// with the length of the picture's file, and the file (none for no
/// picture).
pub fn save(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, stream, _clear_dirty] = args else {
        unreachable!()
    };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if stream == 0 {
        return Some(E_POINTER);
    }
    let file = match file_of(cpu, id, "IPersistStream::Save") {
        Ok(file) => file.unwrap_or_default(),
        Err(e) => return Some(e),
    };
    let mut bytes = b"lt\0\0".to_vec();
    bytes.extend_from_slice(&(file.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&file);
    write_task(cpu, stream, &bytes, 0)
}

/// GetSizeMax(this, size): what Save writes, the header and the file.
pub fn get_size_max(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, size] = args else { unreachable!() };
    let Some(id) = object(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if size == 0 || !cpu.bus.guest_probe(size, 8, true) {
        return Some(E_INVALIDARG);
    }
    let len = match file_of(cpu, id, "IPersistStream::GetSizeMax") {
        Ok(file) => file.map_or(0, |f| f.len()),
        Err(e) => return Some(e),
    };
    cpu.bus.guest_write_32(size, 8 + len as u32);
    cpu.bus.guest_write_32(size + 4, 0);
    Some(S_OK)
}

// IConnectionPointContainer.

pub fn find_connection_point(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, iid, out] = args else {
        unreachable!()
    };
    let (Some(id), Some(iid)) = (object(cpu, this), Guid::read(cpu, iid)) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_POINTER);
    }
    if iid != connpt::IID_IPROPERTYNOTIFYSINK {
        com::write_out(cpu, out, 0);
        return Some(connpt::CONNECT_E_NOCONNECTION);
    }
    let iface = get(cpu, id).ifaces[I_POINT];
    add_ref(cpu, id);
    com::write_out(cpu, out, iface);
    Some(S_OK)
}
