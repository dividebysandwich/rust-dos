//! DCIMAN32.DLL: the Display Control Interface manager, which gives
//! programs the display driver's frame buffer to write to (DCI), and
//! the regions windows and DCs show on screen. Windows 95 and 98 both
//! have it; ActiveMovie's QUARTZ.DLL imports it, so ActiveMovie doesn't
//! load without it.
//!
//! On Windows 98 its functions thunk to 16-bit DCIMAN.DLL. That one's
//! DCIOpenProvider creates a DC for the display driver and keeps it only
//! if the driver answers Escape(QUERYESCSUPPORT) for DCICOMMAND; the
//! built-in Windows has no display driver with that escape, so there is
//! no DCI provider, as on a Windows 98 whose driver has no DCI. Programs
//! then draw through GDI or DirectDraw: QUARTZ's video renderer does.
//! GetDCRegionData and GetWindowRegionData don't need DCI, and give the
//! visible region as Windows does.

use super::gdi32;
use super::stubs::{Api, Conv, Dll, Handler};
use super::user32::paint;
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "DCIMAN32.DLL",
    base: 0xBF68_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: &[],
};

/// DCI's results (dciddi.h).
const DCI_FAIL_INVALIDSURFACE: u32 = -3i32 as u32;
const DCI_FAIL_UNSUPPORTED: u32 = -4i32 as u32;

const fn stdcall(name: &'static str, ordinal: u16, args: u8, handler: Handler) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

/// The functions, with Windows 98 SE's ordinals. Append only (see
/// `stubs`). DllEntryPoint and the thunk data, for 16-bit DCIMAN.DLL,
/// are left out.
static APIS: &[Api] = &[
    stdcall("DCIBeginAccess", 1, 5, invalid_surface),
    stdcall("DCICloseProvider", 2, 1, dci_close_provider),
    stdcall("DCICreateOffscreen", 3, 10, create_surface::<9>),
    stdcall("DCICreateOverlay", 4, 3, create_surface::<2>),
    stdcall("DCICreatePrimary", 5, 2, create_surface::<1>),
    stdcall("DCIDestroy", 6, 1, nothing),
    stdcall("DCIDraw", 7, 1, invalid_surface),
    stdcall("DCIEndAccess", 8, 1, nothing),
    stdcall("DCIEnum", 9, 5, unsupported),
    stdcall("DCIOpenProvider", 10, 0, dci_open_provider),
    stdcall("DCISetClipList", 11, 2, invalid_surface),
    stdcall("DCISetDestination", 12, 3, invalid_surface),
    stdcall("DCISetSrcDestClip", 13, 4, invalid_surface),
    stdcall("GetDCRegionData", 15, 3, get_dc_region_data),
    stdcall("GetWindowRegionData", 16, 3, get_window_region_data),
    stdcall("WinWatchClose", 17, 1, nothing),
    stdcall("WinWatchDidStatusChange", 18, 1, nothing),
    stdcall("WinWatchGetClipList", 19, 4, nothing),
    stdcall("WinWatchNotify", 20, 3, nothing),
    stdcall("WinWatchOpen", 21, 1, nothing),
];

/// DCIOpenProvider(): the DC of the display driver if it is a DCI
/// provider, which none here is, so NULL.
fn dci_open_provider(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// DCICloseProvider(hdc): deletes the DC DCIOpenProvider gave.
fn dci_close_provider(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] != 0 {
        gdi32::dc::delete_dc(cpu, &[args[0]])?;
    }
    Some(0)
}

/// DCICreatePrimary, DCICreateOffscreen and DCICreateOverlay, whose
/// argument `OUT` is where the surface goes: without a provider there is
/// none, so the pointer is NULL and the result DCI_FAIL_UNSUPPORTED.
fn create_surface<const OUT: usize>(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let out = args[OUT];
    if out != 0 && cpu.bus.guest_probe(out, 4, true) {
        cpu.bus.guest_write_32(out, 0);
    }
    Some(DCI_FAIL_UNSUPPORTED)
}

/// A function of a surface, which can't have come from here.
fn invalid_surface(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DCI_FAIL_INVALIDSURFACE)
}

/// DCIEnum(hdc, dst, src, callback, context): no provider to enumerate
/// surfaces of.
fn unsupported(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DCI_FAIL_UNSUPPORTED)
}

/// What returns nothing, and the WinWatch functions: WinWatchOpen gives
/// no watch, so the others get none to work on.
fn nothing(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// GetDCRegionData(hdc, size, data): the DC's visible region as an
/// RGNDATA, in screen coordinates, as DCIMAN.DLL gets it from GDI's
/// InquireVisRgn and GetRegionData. 0 for what isn't a DC.
fn get_dc_region_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hdc, size, out] = args else {
        unreachable!()
    };
    let Some(vis) = gdi32::dc::visible_on_surface(cpu, hdc) else {
        return Some(0);
    };
    Some(gdi32::rgn::write_region_data(cpu, &vis, size, out))
}

/// GetWindowRegionData(hwnd, size, data): what GetDCRegionData gives for
/// a DC of the window's client area from the cache, which DCIMAN.DLL
/// gets with GetDCEx(hwnd, 0, DCX_CACHE | DCX_USESTYLE) and releases.
fn get_window_region_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, size, out] = args else {
        unreachable!()
    };
    let flags = gdi32::dc::DCX_CACHE | gdi32::dc::DCX_USESTYLE;
    let hdc = paint::get_dc_ex(cpu, &[hwnd, 0, flags])?;
    if hdc == 0 {
        return Some(0);
    }
    let bytes = get_dc_region_data(cpu, &[hdc, size, out]);
    paint::release_dc(cpu, &[hwnd, hdc])?;
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win32::gdi32::{Rect, Region};
    use crate::win32::user32::window::{WS_POPUP, WS_VISIBLE, Window};
    use crate::win32::user32::{self, DESKTOP, user};

    /// Where the RGNDATA goes.
    const OUT: u32 = 0x0008_0000;

    /// A top-level window, `rect` its frame and `client` its client
    /// area on the screen, put on top of the others.
    fn top_level(cpu: &mut Cpu, rect: Rect, client: Rect) -> u32 {
        let w = Window {
            style: WS_POPUP | WS_VISIBLE,
            parent: DESKTOP,
            rect,
            client,
            ..Window::default()
        };
        let state = user(cpu);
        let hwnd = state.new_window(w).expect("a window");
        if let Some(d) = state.window_mut(DESKTOP) {
            d.children.insert(0, hwnd);
        }
        hwnd
    }

    /// The rectangles of the RGNDATA at OUT, which took `bytes`, after
    /// checking its header.
    fn rects(cpu: &mut Cpu, bytes: u32) -> Vec<Rect> {
        let count = cpu.bus.guest_read_32(OUT + 8);
        assert_eq!(cpu.bus.guest_read_32(OUT), 32, "dwSize");
        assert_eq!(cpu.bus.guest_read_32(OUT + 4), 1, "RDH_RECTANGLES");
        assert_eq!(bytes, 32 + 16 * count);
        (0..count)
            .map(|i| Rect::read(cpu, OUT + 32 + 16 * i))
            .collect()
    }

    /// A window's region is its client area less the window over it, in
    /// screen coordinates, not from the client area's corner.
    #[test]
    fn region_data_is_the_visible_region_on_the_screen() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        user32::init(&mut cpu);
        let client = Rect::new(110, 70, 290, 240);
        let hwnd = top_level(&mut cpu, Rect::new(100, 50, 300, 250), client);
        let over = Rect::new(250, 0, 400, 100);
        top_level(&mut cpu, over, over);
        let expected = Region::from_rect(client).diff(&Region::from_rect(over));
        assert!(
            expected.rects.len() > 1,
            "the window over it cuts a corner off"
        );

        let bytes = get_window_region_data(&mut cpu, &[hwnd, 0x100, OUT]).unwrap();
        assert_eq!(rects(&mut cpu, bytes), expected.rects);
        assert_eq!(Rect::read(&mut cpu, OUT + 16), client, "rcBound");

        let hdc = paint::get_dc(&mut cpu, &[hwnd]).unwrap();
        let bytes = get_dc_region_data(&mut cpu, &[hdc, 0x100, OUT]).unwrap();
        assert_eq!(rects(&mut cpu, bytes), expected.rects);
        paint::release_dc(&mut cpu, &[hwnd, hdc]);

        // Too small a buffer takes nothing; none gives the size.
        let needed = 32 + 16 * expected.rects.len() as u32;
        let args = [hwnd, needed - 1, OUT];
        assert_eq!(get_window_region_data(&mut cpu, &args), Some(0));
        assert_eq!(
            get_window_region_data(&mut cpu, &[hwnd, 0, 0]),
            Some(needed)
        );
        assert_eq!(get_dc_region_data(&mut cpu, &[0x1234, 0x100, OUT]), Some(0));
    }
}
