//! The DirectDraw object: DirectDrawCreate(Ex) and CoCreateInstance make
//! it, and IDirectDraw, 2, 4 and 7 are its interfaces. It holds the
//! cooperative level the program set; the display's mode and exclusive
//! mode are the system's (`display.rs`). Its methods that make surfaces,
//! palettes and clippers are in their modules.

use super::com::{self, CLASS_E_NOAGGREGATION, Guid};
use super::{
    DD_OK, DDERR_ALREADYINITIALIZED, DDERR_EXCLUSIVEMODEALREADYSET, DDERR_HWNDALREADYSET,
    DDERR_INVALIDDIRECTDRAWGUID, DDERR_INVALIDOBJECT, DDERR_INVALIDPARAMS, DDERR_NOEXCLUSIVEMODE,
    DDERR_NOTINITIALIZED, DDERR_UNSUPPORTED, DDERR_VERTICALBLANKINPROGRESS, DDERR_WRONGMODE,
    IID_DD7, Object, V_DD1, V_DD7, display, state, surface,
};
use crate::cpu::Cpu;
use crate::win32::thread::{self, Wait};

/// What the DirectDraw object has of its own.
#[derive(Clone, Debug, Default)]
pub struct DirectDraw {
    /// Made by CoCreateInstance, it needs Initialize first.
    pub initialized: bool,
    /// The cooperative level's flags, 0 until one is set, and its window
    /// and focus window.
    pub level: u32,
    pub hwnd: u32,
    pub focus: u32,
    /// The mode the program set and found after a mode change of
    /// another object's: whether TestCooperativeLevel says so.
    pub mode_changed: bool,
}

pub const DDSCL_FULLSCREEN: u32 = 0x1;
pub const DDSCL_NOWINDOWCHANGES: u32 = 0x4;
pub const DDSCL_NORMAL: u32 = 0x8;
pub const DDSCL_EXCLUSIVE: u32 = 0x10;
const DDSCL_SETFOCUSWINDOW: u32 = 0x80;
const DDSCL_SETDEVICEWINDOW: u32 = 0x100;
const DDSCL_CREATEDEVICEWINDOW: u32 = 0x200;

/// The DirectDraw object `this` is an interface of, and that interface's
/// vtable: an error if it is none or hasn't been initialized.
pub fn ddraw_of(cpu: &mut Cpu, this: u32) -> Result<(usize, u32), u32> {
    let (vtable, id) = super::this(cpu, this).ok_or(DDERR_INVALIDOBJECT)?;
    let d = state(cpu)
        .object(id)
        .and_then(|o| o.ddraw.as_ref())
        .ok_or(DDERR_INVALIDOBJECT)?;
    if !d.initialized {
        return Err(DDERR_NOTINITIALIZED);
    }
    Ok((vtable, id))
}

pub fn ddraw(cpu: &mut Cpu, id: u32) -> Option<&mut DirectDraw> {
    state(cpu).object_mut(id)?.ddraw.as_mut()
}

/// A device GUID's pointer: NULL, or DDCREATE_HARDWAREONLY or
/// DDCREATE_EMULATIONONLY, is the display. There is no other device.
fn check_device(cpu: &mut Cpu, guid: u32) -> Result<(), u32> {
    if guid <= 2 {
        return Ok(());
    }
    match Guid::read(cpu, guid) {
        Some(_) => Err(DDERR_INVALIDDIRECTDRAWGUID),
        None => Err(DDERR_INVALIDPARAMS),
    }
}

/// A new DirectDraw object.
fn new_ddraw(cpu: &mut Cpu, initialized: bool) -> u32 {
    super::add_object(
        cpu,
        Object {
            ddraw: Some(DirectDraw {
                initialized,
                ..DirectDraw::default()
            }),
            ..Object::default()
        },
    )
}

/// DirectDrawCreate(guid, out, outer).
pub fn direct_draw_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [guid, out, outer] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if outer != 0 {
        return Some(CLASS_E_NOAGGREGATION);
    }
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    hr!(check_device(cpu, guid));
    let id = new_ddraw(cpu, true);
    Some(super::give(cpu, id, V_DD1, out))
}

/// DirectDrawCreateEx(guid, out, iid, outer): only IDirectDraw7.
pub fn direct_draw_create_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [guid, out, iid, outer] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if outer != 0 {
        return Some(CLASS_E_NOAGGREGATION);
    }
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    if Guid::read(cpu, iid) != Some(IID_DD7) {
        return Some(DDERR_INVALIDPARAMS);
    }
    hr!(check_device(cpu, guid));
    let id = new_ddraw(cpu, true);
    Some(super::give(cpu, id, V_DD7, out))
}

/// CoCreateInstance's DirectDraw: one that needs Initialize.
pub fn create_instance(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let Some(vtable) = super::vtable_for(V_DD1, iid) else {
        com::write_out(cpu, out, 0);
        return com::E_NOINTERFACE;
    };
    let id = new_ddraw(cpu, false);
    let hr = super::give(cpu, id, vtable, out);
    if hr != DD_OK {
        super::destroy(cpu, id);
    }
    hr
}

/// Initialize(this, guid).
pub fn initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((_, id)) = super::this(cpu, args[0]) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    hr!(check_device(cpu, args[1]));
    let Some(d) = ddraw(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if d.initialized {
        return Some(DDERR_ALREADYINITIALIZED);
    }
    d.initialized = true;
    Some(DD_OK)
}

/// The DirectDraw object `id` goes: the display it had is given back,
/// and what it made is left without it. Its surfaces lose their memory.
pub fn destroyed(cpu: &mut Cpu, id: u32) {
    display::object_gone(cpu, id);
    let made = state(cpu).ids(|o| o.owner == id);
    for object in made {
        surface::orphan(cpu, object);
        if let Some(o) = state(cpu).object_mut(object) {
            o.owner = 0;
        }
    }
}

/// Methods the display doesn't have.
pub fn unsupported(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DDERR_UNSUPPORTED)
}

/// Compact(this).
pub fn compact(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(ddraw_of(cpu, args[0]));
    Some(DD_OK)
}

// The cooperative level and the display mode.

/// SetCooperativeLevel(this, hwnd, flags).
pub fn set_cooperative_level(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, hwnd, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(ddraw_of(cpu, this));
    let exclusive = state(cpu).display.exclusive;
    let window = |cpu: &mut Cpu, hwnd: u32| cpu.bus.win32.user.is_window(hwnd);
    if flags & DDSCL_SETFOCUSWINDOW != 0 {
        if flags & !(DDSCL_SETFOCUSWINDOW | DDSCL_NOWINDOWCHANGES) != 0 {
            return Some(DDERR_INVALIDPARAMS);
        }
        if exclusive == id {
            return Some(DDERR_HWNDALREADYSET);
        }
        if !window(cpu, hwnd) {
            return Some(DDERR_INVALIDPARAMS);
        }
        if let Some(d) = ddraw(cpu, id) {
            d.focus = hwnd;
        }
        return Some(DD_OK);
    }
    let modes = flags & (DDSCL_NORMAL | DDSCL_EXCLUSIVE);
    if modes == 0 || modes == DDSCL_NORMAL | DDSCL_EXCLUSIVE {
        return Some(DDERR_INVALIDPARAMS);
    }
    if (flags & DDSCL_EXCLUSIVE != 0) != (flags & DDSCL_FULLSCREEN != 0) {
        return Some(DDERR_INVALIDPARAMS);
    }
    if flags & DDSCL_SETDEVICEWINDOW != 0 && flags & DDSCL_CREATEDEVICEWINDOW == 0 && hwnd == 0 {
        return Some(DDERR_INVALIDPARAMS);
    }
    if flags & DDSCL_EXCLUSIVE != 0 {
        if !window(cpu, hwnd) {
            return Some(DDERR_INVALIDPARAMS);
        }
        if exclusive != 0 && exclusive != id {
            return Some(DDERR_EXCLUSIVEMODEALREADYSET);
        }
        state(cpu).display.exclusive = id;
    } else {
        if hwnd != 0 && !window(cpu, hwnd) {
            return Some(DDERR_INVALIDPARAMS);
        }
        if exclusive == id {
            state(cpu).display.exclusive = 0;
        }
    }
    if let Some(d) = ddraw(cpu, id) {
        d.level = flags;
        d.hwnd = hwnd;
    }
    Some(DD_OK)
}

/// TestCooperativeLevel(this).
pub fn test_cooperative_level(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(ddraw_of(cpu, args[0]));
    let exclusive = state(cpu).display.exclusive;
    let Some(d) = ddraw(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let wants_exclusive = d.level & DDSCL_EXCLUSIVE != 0;
    let changed = d.mode_changed;
    Some(if wants_exclusive && exclusive != id {
        DDERR_NOEXCLUSIVEMODE
    } else if !wants_exclusive && exclusive != 0 {
        DDERR_EXCLUSIVEMODEALREADYSET
    } else if changed {
        DDERR_WRONGMODE
    } else {
        DD_OK
    })
}

/// SetDisplayMode(this, width, height, bpp), and IDirectDraw2's with
/// the refresh rate and flags after them.
pub fn set_display_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(ddraw_of(cpu, args[0]));
    let (w, h, bpp) = (args[1], args[2], args[3]);
    let exclusive = state(cpu).display.exclusive;
    if exclusive != 0 && exclusive != id {
        return Some(DDERR_NOEXCLUSIVEMODE);
    }
    let hr = display::set_mode(cpu, id, w, h, bpp);
    if hr == DD_OK {
        for other in state(cpu).ids(|o| o.ddraw.is_some()) {
            if let Some(d) = ddraw(cpu, other) {
                d.mode_changed = other != id;
            }
        }
    }
    Some(hr)
}

/// RestoreDisplayMode(this).
pub fn restore_display_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(ddraw_of(cpu, args[0]));
    let exclusive = state(cpu).display.exclusive;
    if exclusive != 0 && exclusive != id {
        return Some(DDERR_NOEXCLUSIVEMODE);
    }
    display::restore(cpu);
    for other in state(cpu).ids(|o| o.ddraw.is_some()) {
        if let Some(d) = ddraw(cpu, other) {
            d.mode_changed = false;
        }
    }
    Some(DD_OK)
}

/// GetDisplayMode(this, desc).
pub fn get_display_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, desc] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, _) = hr!(ddraw_of(cpu, this));
    let size = hr!(surface::desc_size(cpu, desc, super::version(vtable)));
    let (w, h, bpp, pitch) = display::current(cpu);
    let format = surface::rgb_format(bpp);
    let d = surface::Desc {
        flags: surface::DDSD_HEIGHT
            | surface::DDSD_WIDTH
            | surface::DDSD_PITCH
            | surface::DDSD_PIXELFORMAT
            | surface::DDSD_REFRESHRATE,
        width: w,
        height: h,
        pitch,
        refresh: 0,
        format,
        ..surface::Desc::default()
    };
    d.write(cpu, desc, size);
    Some(DD_OK)
}

// Capabilities.

const DDCAPS_BLT: u32 = 0x40;
const DDCAPS_BLTSTRETCH: u32 = 0x200;
const DDCAPS_GDI: u32 = 0x400;
const DDCAPS_PALETTE: u32 = 0x8000;
const DDCAPS_READSCANLINE: u32 = 0x2_0000;
const DDCAPS_COLORKEY: u32 = 0x40_0000;
const DDCAPS_BLTCOLORFILL: u32 = 0x400_0000;
const DDCAPS_CANCLIP: u32 = 0x2000_0000;
const DDCAPS_CANCLIPSTRETCHED: u32 = 0x4000_0000;
const DDCAPS_CANBLTSYSMEM: u32 = 0x8000_0000;
const DDCAPS2_NOPAGELOCKREQUIRED: u32 = 0x800;
const DDCAPS2_WIDESURFACES: u32 = 0x1000;
const DDCAPS2_CANRENDERWINDOWED: u32 = 0x8_0000;
const DDCKEYCAPS_DESTBLT: u32 = 0x1;
const DDCKEYCAPS_SRCBLT: u32 = 0x200;
/// Stretching and shrinking in both directions, by whole and other
/// factors, and mirroring.
const DDFXCAPS: u32 =
    0x40 | 0x80 | 0x400 | 0x800 | 0x1000 | 0x2000 | 0x4000 | 0x8000 | 0x1_0000 | 0x2_0000;
const DDPCAPS_8BIT: u32 = 0x4;
const DDPCAPS_PRIMARYSURFACE: u32 = 0x10;

/// The sizes of DDCAPS: DirectX 3's, 5's, and 6's and 7's.
const CAPS_DX3: u32 = 316;
const CAPS_DX5: u32 = 364;
const CAPS_DX6: u32 = 380;

/// Write a DDCAPS at `at`, of the size its dwSize says: the hardware's
/// (`hal`) or the emulation's.
fn write_caps(cpu: &mut Cpu, at: u32, hal: bool) -> Result<(), u32> {
    if !cpu.bus.guest_probe(at, 4, false) {
        return Err(DDERR_INVALIDPARAMS);
    }
    let size = cpu.bus.guest_read_32(at);
    if ![CAPS_DX3, CAPS_DX5, CAPS_DX6].contains(&size)
        || !cpu.bus.guest_probe(at, size as usize, true)
    {
        return Err(DDERR_INVALIDPARAMS);
    }
    cpu.bus.guest_fill(at + 4, size as usize - 4, 0);
    let caps = DDCAPS_BLT
        | DDCAPS_BLTSTRETCH
        | DDCAPS_GDI
        | DDCAPS_PALETTE
        | DDCAPS_COLORKEY
        | DDCAPS_BLTCOLORFILL
        | DDCAPS_CANCLIP
        | DDCAPS_CANCLIPSTRETCHED
        | DDCAPS_CANBLTSYSMEM
        | if hal { DDCAPS_READSCANLINE } else { 0 };
    let caps2 = DDCAPS2_NOPAGELOCKREQUIRED | DDCAPS2_WIDESURFACES | DDCAPS2_CANRENDERWINDOWED;
    let ckey = DDCKEYCAPS_SRCBLT | DDCKEYCAPS_DESTBLT;
    let (total, free) = if hal {
        display::video_memory(cpu)
    } else {
        (0, 0)
    };
    let surface_caps = surface::DDSCAPS_BACKBUFFER
        | surface::DDSCAPS_COMPLEX
        | surface::DDSCAPS_FLIP
        | surface::DDSCAPS_FRONTBUFFER
        | surface::DDSCAPS_OFFSCREENPLAIN
        | surface::DDSCAPS_PALETTE
        | surface::DDSCAPS_PRIMARYSURFACE
        | surface::DDSCAPS_SYSTEMMEMORY
        | surface::DDSCAPS_VISIBLE
        | if hal {
            surface::DDSCAPS_VIDEOMEMORY | surface::DDSCAPS_LOCALVIDMEM
        } else {
            0
        };
    let w = |cpu: &mut Cpu, offset: u32, value: u32| cpu.bus.guest_write_32(at + offset, value);
    w(cpu, 0x04, caps);
    w(cpu, 0x08, caps2);
    w(cpu, 0x0C, ckey);
    w(cpu, 0x10, DDFXCAPS);
    w(cpu, 0x18, DDPCAPS_8BIT | DDPCAPS_PRIMARYSURFACE);
    w(cpu, 0x3C, total);
    w(cpu, 0x40, free);
    w(cpu, 0x60, 4);
    // SRCCOPY's bit among the ROPs, for each kind of blit.
    let rop_word = 4 * (0xCC / 32);
    let rop_bit = 1 << (0xCC % 32);
    for rops in [0x64, 0xB8, 0xE4, 0x110] {
        w(cpu, rops + rop_word, rop_bit);
    }
    w(cpu, 0x84, surface_caps);
    for (caps_at, ckey_at, fx_at) in [
        (0xAC, 0xB0, 0xB4),
        (0xD8, 0xDC, 0xE0),
        (0x104, 0x108, 0x10C),
    ] {
        w(cpu, caps_at, caps);
        w(cpu, ckey_at, ckey);
        w(cpu, fx_at, DDFXCAPS);
    }
    if size >= CAPS_DX5 {
        w(cpu, 0x138, caps2);
    }
    if size >= CAPS_DX6 {
        w(cpu, 0x16C, surface_caps);
    }
    Ok(())
}

/// GetCaps(this, driver caps, emulation caps).
pub fn get_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, hal, hel] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    hr!(ddraw_of(cpu, this));
    if hal == 0 && hel == 0 {
        return Some(DDERR_INVALIDPARAMS);
    }
    if hal != 0 {
        hr!(write_caps(cpu, hal, true));
    }
    if hel != 0 {
        hr!(write_caps(cpu, hel, false));
    }
    Some(DD_OK)
}

/// GetAvailableVidMem(this, caps, total, free).
pub fn get_available_vid_mem(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, caps, total_at, free_at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    hr!(ddraw_of(cpu, this));
    if caps == 0 || !cpu.bus.guest_probe(caps, 4, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let (total, free) = display::video_memory(cpu);
    if total_at != 0 && !com::write_out(cpu, total_at, total) {
        return Some(DDERR_INVALIDPARAMS);
    }
    if free_at != 0 && !com::write_out(cpu, free_at, free) {
        return Some(DDERR_INVALIDPARAMS);
    }
    Some(DD_OK)
}

/// GetFourCCCodes(this, count, codes): there are none.
pub fn get_four_cc_codes(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, count, _] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    hr!(ddraw_of(cpu, this));
    if !com::write_out(cpu, count, 0) {
        return Some(DDERR_INVALIDPARAMS);
    }
    Some(DD_OK)
}

/// GetDeviceIdentifier(this, identifier, flags): DDDEVICEIDENTIFIER, or
/// IDirectDraw7's DDDEVICEIDENTIFIER2 with the WHQL level after it.
pub fn get_device_identifier(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, _) = hr!(ddraw_of(cpu, this));
    let size = if vtable == V_DD7 { 1068 } else { 1064 };
    if flags & !1 != 0 || at == 0 || !cpu.bus.guest_probe(at, size, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    cpu.bus.guest_fill(at, size, 0);
    cpu.bus.guest_write_bytes(at, b"display.drv");
    let name = cpu.bus.vga.adapter.name().as_bytes().to_vec();
    cpu.bus
        .guest_write_bytes(at + 512, &name[..name.len().min(511)]);
    // The driver's version: 4.10.0.1998, Windows 98's.
    cpu.bus.guest_write_32(at + 1024, 1998);
    cpu.bus.guest_write_32(at + 1028, 4 << 16 | 10);
    Some(DD_OK)
}

// The display's timing.

/// The display's lines and the line the beam is on, in the mode's
/// lines (a small mode's are shown twice).
fn scan(cpu: &mut Cpu) -> (u32, bool) {
    let (timing, now) = display::timing(cpu);
    let (line, _) = timing.position(now);
    let scale = crate::win32::screen::line_scale(cpu);
    let in_blank = line >= timing.display;
    (line / scale, in_blank)
}

/// GetMonitorFrequency(this, out).
pub fn get_monitor_frequency(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(ddraw_of(cpu, args[0]));
    let (timing, _) = display::timing(cpu);
    let hz = timing.hz().round() as u32;
    if !com::write_out(cpu, args[1], hz) {
        return Some(DDERR_INVALIDPARAMS);
    }
    Some(DD_OK)
}

/// GetScanLine(this, out).
pub fn get_scan_line(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(ddraw_of(cpu, args[0]));
    let (line, in_blank) = scan(cpu);
    if !com::write_out(cpu, args[1], line) {
        return Some(DDERR_INVALIDPARAMS);
    }
    Some(if in_blank {
        DDERR_VERTICALBLANKINPROGRESS
    } else {
        DD_OK
    })
}

/// GetVerticalBlankStatus(this, out).
pub fn get_vertical_blank_status(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(ddraw_of(cpu, args[0]));
    let (timing, now) = display::timing(cpu);
    let blank = timing.status(now) & 0x08 != 0;
    if !com::write_out(cpu, args[1], blank as u32) {
        return Some(DDERR_INVALIDPARAMS);
    }
    Some(DD_OK)
}

const DDWAITVB_BLOCKBEGIN: u32 = 1;
const DDWAITVB_BLOCKBEGINEVENT: u32 = 2;
const DDWAITVB_BLOCKEND: u32 = 4;

/// WaitForVerticalBlank(this, flags, event): the thread waits for the
/// next vertical retrace to begin, or for the next frame's picture to
/// begin after it.
pub fn wait_for_vertical_blank(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, flags, _] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    hr!(ddraw_of(cpu, this));
    let (timing, now) = display::timing(cpu);
    let until = match flags {
        DDWAITVB_BLOCKBEGIN => timing.next_retrace(now),
        DDWAITVB_BLOCKEND => timing.next_display(now),
        DDWAITVB_BLOCKBEGINEVENT => return Some(DDERR_UNSUPPORTED),
        _ => return Some(DDERR_INVALIDPARAMS),
    };
    wait_until(cpu, until, DD_OK)
}

/// Make the thread wait until the clock's `until` nanoseconds, the
/// method then returning `result`.
pub fn wait_until(cpu: &mut Cpu, until: u64, result: u32) -> Option<u32> {
    let deadline = until.div_ceil(1000);
    if deadline <= cpu.bus.clock.now_micros() {
        return Some(result);
    }
    cpu.set_eax(result);
    thread::block(
        cpu,
        Wait {
            deadline: Some(deadline),
            timeout_result: result,
            ..Wait::default()
        },
    );
    None
}

crate::state_fields!(DirectDraw {
    initialized,
    level,
    hwnd,
    focus,
    mode_changed
});
