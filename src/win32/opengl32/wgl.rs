//! WGL: pixel formats, rendering contexts and swapping buffers, and the
//! ICD's side of GDI's ChoosePixelFormat, DescribePixelFormat,
//! SetPixelFormat, GetPixelFormat and SwapBuffers, which on Windows go
//! to the loaded OpenGL32's wgl functions and through them to the ICD
//! of the DC's device. When the program's own OPENGL32.DLL replaces
//! this one, GDI32 calls that DLL's functions instead (gdi32's
//! `guest_wgl`).
//!
//! The ICD has the pixel formats 3dfx's driver had on a Banshee or
//! Voodoo3, which draw only in 16 bits: 5-6-5 color, single or double
//! buffered, with a 16-bit depth buffer and no stencil, alpha or
//! accumulation buffer. A window's pixel format is set once.
//!
//! On a Voodoo2 the ICD is the one of 3dfx's Voodoo2 driver
//! (3DFXVGL.DLL), which draws full screen on the card: its two pixel
//! formats are 5-6-5 with a 16-bit depth buffer, single or double
//! buffered, as the Banshee's are, but they also say they support GDI
//! and have an 8-bit stencil and a 64-bit accumulation buffer, which
//! that driver kept in software. wglMakeCurrent opens the card at the
//! smallest of its modes that holds the window's client area and a
//! depth buffer, and fails when none does, as that driver did with a
//! window of 1024x768 on a card with 4 MB of frame buffer. OpenGL's
//! window is the top left corner of the card's screen, and SwapBuffers
//! swaps the card's buffers.

use super::super::ddraw::d3d::device::{self as inner_device, Device as Inner, Viewport7};
use super::super::ddraw::d3d::pipeline::{IDENTITY, Matrix};
use super::super::ddraw::d3d::{D3dObject, hal, shadow};
use super::super::ddraw::directdraw::{
    self, DDSCL_EXCLUSIVE, DDSCL_FULLSCREEN, DDSCL_NORMAL, DDSCL_NOWINDOWCHANGES,
};
use super::super::ddraw::surface::{
    self as dd_surface, DDFLIP_NOVSYNC, DDFLIP_WAIT, DDPF_RGB, DDPF_ZBUFFER, DDSCAPS_3DDEVICE,
    DDSCAPS_COMPLEX, DDSCAPS_FLIP, DDSCAPS_OFFSCREENPLAIN, DDSCAPS_PRIMARYSURFACE,
    DDSCAPS_VIDEOMEMORY, DDSCAPS_ZBUFFER, DDSD_BACKBUFFERCOUNT, DDSD_CAPS, DDSD_HEIGHT,
    DDSD_PIXELFORMAT, DDSD_WIDTH, Desc,
};
use super::super::ddraw::{self, Format, V_DD7, V_DEVICE7, V_SURFACE7, voodoo};
use super::super::glide::hw::Card;
use super::{DLL, context_index, current, hglrc, new_context, state};
use crate::cpu::Cpu;

const TRUE: u32 = 1;
const FALSE: u32 = 0;

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_PIXEL_FORMAT: u32 = 2000;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_INVALID_DATA: u32 = 13;

// PIXELFORMATDESCRIPTOR's flags.
const PFD_DOUBLEBUFFER: u32 = 0x1;
const PFD_STEREO: u32 = 0x2;
const PFD_DRAW_TO_WINDOW: u32 = 0x4;
const PFD_DRAW_TO_BITMAP: u32 = 0x8;
const PFD_SUPPORT_GDI: u32 = 0x10;
const PFD_SUPPORT_OPENGL: u32 = 0x20;
const PFD_SWAP_COPY: u32 = 0x400;
const PFD_DOUBLEBUFFER_DONTCARE: u32 = 0x4000_0000;
const PFD_STEREO_DONTCARE: u32 = 0x8000_0000;

/// PIXELFORMATDESCRIPTOR's size.
const PFD_SIZE: u32 = 40;

/// The ICD's pixel formats, from 1: their flags. Both are 5-6-5 with a
/// 16-bit depth buffer, as Mesa's fxwgl.c has the first two of its
/// table for cards before the Voodoo4. A ToadForce's are of the
/// desktop's colours (`color_bits`).
const FORMATS: [u32; 2] = [
    PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL,
    PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER | PFD_SWAP_COPY,
];

/// The pixel formats of 3dfx's Voodoo2 ICD, from 1: their flags. Both
/// are 5-6-5 with a 16-bit depth buffer, an 8-bit stencil buffer and a
/// 64-bit accumulation buffer, as its DescribePixelFormat gives them.
const VOODOO2_FORMATS: [u32; 2] = [
    PFD_DRAW_TO_WINDOW | PFD_SUPPORT_GDI | PFD_SUPPORT_OPENGL,
    PFD_DRAW_TO_WINDOW | PFD_SUPPORT_GDI | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
];

/// Which ICD the machine has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icd {
    /// None: no pixel formats.
    None,
    /// The display adapter's: 3dfx's for the Banshee and Voodoo3, or
    /// the ToadForce's. A context draws in a window.
    Windowed,
    /// 3dfx's for the Voodoo2, an add-on card: a context draws full
    /// screen on the card.
    FullScreen,
}

/// The ICD the machine has: the display adapter's when that is a
/// Banshee, a Voodoo3 or a ToadForce with Direct3D's HAL, the
/// Voodoo2's when the machine has one.
pub fn icd(cpu: &Cpu) -> Icd {
    if cpu.bus.vga.adapter.voodoo_board().is_some() || hal::toadforce::there(cpu) {
        Icd::Windowed
    } else if super::super::glide::hw::board(cpu).is_some_and(|b| b.card == Card::Voodoo2) {
        Icd::FullScreen
    } else {
        Icd::None
    }
}

/// The flags of the pixel formats there are, from 1.
fn formats(cpu: &Cpu) -> &'static [u32] {
    match icd(cpu) {
        Icd::Windowed => &FORMATS,
        Icd::FullScreen => &VOODOO2_FORMATS,
        Icd::None => &[],
    }
}

/// The formats there are.
pub fn describe_formats(cpu: &Cpu) -> u32 {
    formats(cpu).len() as u32
}

/// Whether the pixel format `format` is double buffered: the second of
/// either ICD's.
pub fn double_buffered(format: u32) -> bool {
    FORMATS
        .get((format as usize).wrapping_sub(1))
        .is_some_and(|f| f & PFD_DOUBLEBUFFER != 0)
}

/// The window of the DC `hdc`, 0 for none.
pub fn window_of(cpu: &mut Cpu, hdc: u32) -> u32 {
    cpu.bus.win32.gdi.dc(hdc).map_or(0, |d| d.hwnd)
}

/// The pixel format of the window `hwnd`, 0 for none.
fn format_of(cpu: &mut Cpu, hwnd: u32) -> u32 {
    state(cpu)
        .formats
        .iter()
        .find(|f| f.0 == hwnd)
        .map_or(0, |f| f.1)
}

/// The bits of a pixel of a window's color buffer: 16, 5-6-5, or on a
/// ToadForce with a 32-bit desktop 32, 8 bits a channel. A window's
/// buffers are of the desktop's format on a real driver, whose
/// SwapBuffers copies the back buffer to the screen as it is; the card
/// then copies it with one `BLT` (`d3d8::copy::present_window`). The
/// 3dfx cards draw 16 bits only.
pub fn color_bits(cpu: &mut Cpu) -> u32 {
    let wide = crate::win32::screen::is_toadforce(cpu) && ddraw::display::current(cpu).2 == 32;
    if wide { 32 } else { 16 }
}

/// Write the descriptor of the pixel format `format` (from 1) at `at`.
fn write_descriptor(cpu: &mut Cpu, at: u32, format: u32) {
    let flags = formats(cpu)[format as usize - 1];
    let mut b = [0u8; PFD_SIZE as usize];
    b[0..2].copy_from_slice(&(PFD_SIZE as u16).to_le_bytes());
    b[2..4].copy_from_slice(&1u16.to_le_bytes());
    b[4..8].copy_from_slice(&flags.to_le_bytes());
    // PFD_TYPE_RGBA, 16 bits of color: red 5 at 11, green 6 at 5, blue
    // 5 at 0, no alpha.
    b[8] = 0;
    b[9] = 16;
    b[10..18].copy_from_slice(&[5, 11, 6, 5, 5, 0, 0, 0]);
    if color_bits(cpu) == 32 {
        // 24 bits of color: red 8 at 16, green 8 at 8, blue 8 at 0.
        b[9] = 24;
        b[10..18].copy_from_slice(&[8, 16, 8, 8, 8, 0, 0, 0]);
    }
    // No accumulation buffer, depth 16, no stencil or aux buffers, the
    // main plane. The Voodoo2's ICD has a 64-bit accumulation buffer,
    // 16 bits each, and an 8-bit stencil buffer.
    b[23] = 16;
    if icd(cpu) == Icd::FullScreen {
        b[18..23].copy_from_slice(&[64, 16, 16, 16, 16]);
        b[24] = 8;
    }
    cpu.bus.guest_write_bytes(at, &b);
}

/// The pixel format that best matches the descriptor at `at`, as
/// Mesa's wglChoosePixelFormat picks it: 0 for none. The Voodoo2's ICD
/// takes any number of color bits for its 16.
fn choose(cpu: &mut Cpu, at: u32) -> u32 {
    let icd = icd(cpu);
    if icd == Icd::None || at == 0 || !cpu.bus.guest_probe(at, PFD_SIZE as usize, false) {
        return 0;
    }
    let mut b = [0u8; PFD_SIZE as usize];
    cpu.bus.guest_read_bytes(at, &mut b);
    let size = u16::from_le_bytes([b[0], b[1]]);
    let version = u16::from_le_bytes([b[2], b[3]]);
    if size as u32 != PFD_SIZE || version != 1 {
        return 0;
    }
    let flags = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    let (kind, color, alpha) = (b[8], b[9], b[16]);
    // 24 and 32 bits are taken as 16: whichever of the three is asked
    // for, the formats have the window's colour bits, 16 or on a
    // ToadForce the desktop's 24 (`color_bits`).
    let color = if color == 24 || color == 32 {
        16
    } else {
        color
    };
    if color > 0 && color != 16 && icd == Icd::Windowed {
        return 0;
    }
    for (i, &have) in formats(cpu).iter().enumerate() {
        let needs = [
            PFD_DRAW_TO_WINDOW,
            PFD_DRAW_TO_BITMAP,
            PFD_SUPPORT_GDI,
            PFD_SUPPORT_OPENGL,
        ];
        if needs.iter().any(|&n| flags & n != 0 && have & n == 0) {
            continue;
        }
        if flags & PFD_DOUBLEBUFFER_DONTCARE == 0
            && flags & PFD_DOUBLEBUFFER != have & PFD_DOUBLEBUFFER
        {
            continue;
        }
        if flags & PFD_STEREO_DONTCARE == 0 && flags & PFD_STEREO != 0 {
            continue;
        }
        // No alpha buffer.
        if alpha > 0 {
            continue;
        }
        if kind == 0 {
            return i as u32 + 1;
        }
    }
    0
}

/// ChoosePixelFormat(hdc, descriptor) and wglChoosePixelFormat.
pub fn choose_pixel_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let format = choose(cpu, args[1]);
    if format == 0 {
        super::super::gdi32::set_error(cpu, ERROR_INVALID_PIXEL_FORMAT);
    }
    Some(format)
}

/// DescribePixelFormat(hdc, format, size, descriptor) and
/// wglDescribePixelFormat: the number of formats, the descriptor of
/// `format` written if there is room for it.
pub fn describe_pixel_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [_, format, size, at] = args[..] else {
        return Some(0);
    };
    let count = describe_formats(cpu);
    if at == 0 {
        return Some(count);
    }
    if format == 0 || format > count {
        super::super::gdi32::set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    if size >= PFD_SIZE && cpu.bus.guest_probe(at, PFD_SIZE as usize, true) {
        write_descriptor(cpu, at, format);
    }
    Some(count)
}

/// SetPixelFormat(hdc, format, descriptor) and wglSetPixelFormat: a
/// window's format is set once.
pub fn set_pixel_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hdc, format, _] = args[..] else {
        return Some(FALSE);
    };
    let hwnd = window_of(cpu, hdc);
    if format == 0 || format > describe_formats(cpu) || hwnd == 0 {
        super::super::gdi32::set_error(cpu, ERROR_INVALID_PIXEL_FORMAT);
        return Some(FALSE);
    }
    match format_of(cpu, hwnd) {
        0 => state(cpu).formats.push((hwnd, format)),
        f if f == format => {}
        _ => {
            super::super::gdi32::set_error(cpu, ERROR_INVALID_PIXEL_FORMAT);
            return Some(FALSE);
        }
    }
    Some(TRUE)
}

/// GetPixelFormat(hdc) and wglGetPixelFormat.
pub fn get_pixel_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hwnd = window_of(cpu, args[0]);
    Some(format_of(cpu, hwnd))
}

/// The window `hwnd` went: its pixel format with it.
pub fn window_destroyed(cpu: &mut Cpu, hwnd: u32) {
    state(cpu).formats.retain(|f| f.0 != hwnd);
}

// Contexts.

/// The strings glGetString gives: vendor, renderer, version and
/// extensions, at these offsets of a context's page.
pub const VENDOR_AT: u32 = 0;
pub const RENDERER_AT: u32 = 0x40;
pub const VERSION_AT: u32 = 0xC0;
pub const EXTENSIONS_AT: u32 = 0x100;
pub const WGL_EXTENSIONS_AT: u32 = 0x300;

/// The extensions, as glGetString(GL_EXTENSIONS) gives them: those of
/// 3dfx's Banshee and Voodoo3 ICD (3DFXOGL.DLL) that are here. Its own
/// list has more (paletted textures, texture compression, point
/// parameters and others), which aren't. GL_EXT_bgra is in it, and
/// Myth II 1.8.3 takes an OpenGL 1.1 device only with it.
pub const EXTENSIONS: &str = "GL_ARB_multitexture GL_EXT_bgra \
    GL_EXT_compiled_vertex_array GL_EXT_texture_env_add WGL_EXT_swap_control";
/// The WGL extensions, as wglGetExtensionsStringEXT gives them.
pub const WGL_EXTENSIONS: &str = "WGL_EXT_extensions_string WGL_EXT_swap_control";

/// The extensions of the Voodoo2's ICD that are here: its own list
/// has more (paletted textures, point parameters and others), which
/// aren't.
pub const VOODOO2_EXTENSIONS: &str = "GL_ARB_multitexture GL_EXT_bgra \
    GL_EXT_compiled_vertex_array WGL_EXT_swap_control";

/// The extensions of a Banshee or Voodoo3 with `tmus` texture units. A
/// Banshee's one unit has no multitexture, as Mesa's driver leaves it
/// out with one.
fn icd_extensions(tmus: u32) -> &'static str {
    if tmus > 1 {
        EXTENSIONS
    } else {
        EXTENSIONS
            .strip_prefix("GL_ARB_multitexture ")
            .expect("EXTENSIONS starts with GL_ARB_multitexture")
    }
}

/// Write the strings into a new page of the process: its address.
fn write_strings(cpu: &mut Cpu) -> Option<u32> {
    if hal::toadforce::there(cpu) {
        // The card's own names. The version and the extensions are
        // what the driver does on the Direct3D device today.
        let memory = cpu.bus.toadforce.as_ref().map_or(0, |card| card.memory());
        let renderer = format!("ToadForce/{} MB", memory >> 20);
        let strings = [
            (VENDOR_AT, "Rust-DOS"),
            (RENDERER_AT, renderer.as_str()),
            (VERSION_AT, "1.1.0"),
            (EXTENSIONS_AT, EXTENSIONS),
            (WGL_EXTENSIONS_AT, WGL_EXTENSIONS),
        ];
        return place_strings(cpu, &strings);
    }
    let board = super::super::glide::hw::board(cpu)?;
    let (renderer, version, extensions) = if board.card == Card::Voodoo2 {
        // As 3DFXVGL.DLL gives them, which a program loads itself or
        // finds as the OPENGL32.DLL of its own folder ("stand-alone").
        (
            format!(
                "3Dfx/Voodoo2/{} TMUs/{} MB",
                board.tmus,
                board.fb_bytes >> 20
            ),
            "1.1.0 3Dfx stand-alone (May 26 1999)",
            VOODOO2_EXTENSIONS,
        )
    } else {
        let name = if board.voodoo3 {
            "Voodoo3"
        } else {
            "Voodoo Banshee"
        };
        let renderer = format!(
            "3Dfx/{} (tm)/{} TMUs/{} MB SDRAM/ICD (Jun  5 2000)",
            name,
            board.tmus,
            board.fb_bytes >> 20
        );
        (renderer, "1.1.0", icd_extensions(board.tmus))
    };
    let strings = [
        (VENDOR_AT, "3Dfx Interactive Inc."),
        (RENDERER_AT, renderer.as_str()),
        (VERSION_AT, version),
        (EXTENSIONS_AT, extensions),
        (WGL_EXTENSIONS_AT, WGL_EXTENSIONS),
    ];
    place_strings(cpu, &strings)
}

/// The strings in a new page of the process, each at its offset.
fn place_strings(cpu: &mut Cpu, strings: &[(u32, &str)]) -> Option<u32> {
    let at = super::super::kernel32::alloc_pages(cpu, 0x1000)?;
    for &(offset, s) in strings {
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0);
        cpu.bus.guest_write_bytes(at + offset, &bytes);
    }
    Some(at)
}

/// A surface of the DirectDraw object `dd` in video memory: the render
/// target, of `color_bits`, or the depth buffer, 16 bits. Its id and
/// the interface of it that is held.
fn make_buffer(cpu: &mut Cpu, dd: u32, size: (u32, u32), depth: bool) -> Option<(u32, u32)> {
    let level = directdraw::ddraw(cpu, dd).map_or(0, |d| d.level);
    let wide = color_bits(cpu) == 32;
    let format = if depth {
        Format {
            flags: DDPF_ZBUFFER,
            bits: 16,
            g: 0xFFFF,
            ..Format::default()
        }
    } else if wide {
        Format {
            flags: DDPF_RGB,
            bits: 32,
            r: 0xFF_0000,
            g: 0xFF00,
            b: 0xFF,
            ..Format::default()
        }
    } else {
        Format {
            flags: DDPF_RGB,
            bits: 16,
            r: 0xF800,
            g: 0x07E0,
            b: 0x001F,
            ..Format::default()
        }
    };
    let caps = if depth {
        DDSCAPS_ZBUFFER | DDSCAPS_VIDEOMEMORY
    } else {
        DDSCAPS_3DDEVICE | DDSCAPS_OFFSCREENPLAIN | DDSCAPS_VIDEOMEMORY
    };
    let desc = Desc {
        flags: DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT,
        width: size.0,
        height: size.1,
        format,
        caps: [caps, 0, 0, 0],
        ..Desc::default()
    };
    let ids = dd_surface::make(cpu, dd, 7, level, &desc).ok()?;
    let iface = ddraw::iface(cpu, ids[0], V_SURFACE7);
    if iface == 0 {
        ddraw::destroy(cpu, ids[0]);
        return None;
    }
    ddraw::add_ref(cpu, iface);
    Some((ids[0], iface))
}

/// The size of the client area of the window `hwnd`, 1 by 1 at least.
fn client_size(cpu: &mut Cpu, hwnd: u32) -> (u32, u32) {
    let r = cpu.bus.win32.user.screen_client(hwnd);
    (
        (r.right - r.left).max(1) as u32,
        (r.bottom - r.top).max(1) as u32,
    )
}

/// What `fit_buffers` looks at of a context.
struct Buffers {
    window: u32,
    dd: u32,
    target: u32,
    depth: u32,
    size: (u32, u32),
}

/// Make the context's color and depth buffers again if the window's
/// client area isn't their size, or they were lost: false if they
/// can't be made.
pub fn fit_buffers(cpu: &mut Cpu, index: usize) -> bool {
    if full_screen(cpu, index) {
        return fit_card(cpu, index);
    }
    let Some(c) = state(cpu).contexts[index].as_ref().map(|c| Buffers {
        window: c.window,
        dd: c.dd,
        target: c.target,
        depth: c.depth,
        size: (c.width, c.height),
    }) else {
        return false;
    };
    let size = client_size(cpu, c.window);
    let lost = |cpu: &mut Cpu, id: u32| {
        ddraw::state(cpu)
            .surface(id)
            .is_none_or(|s| s.lost || s.memory == dd_surface::MEM_NONE)
    };
    if c.target != 0 && c.size == size && !lost(cpu, c.target) && !lost(cpu, c.depth) {
        return true;
    }
    drop_buffers(cpu, index);
    let Some((target, target_iface)) = make_buffer(cpu, c.dd, size, false) else {
        return false;
    };
    let Some((depth, depth_iface)) = make_buffer(cpu, c.dd, size, true) else {
        ddraw::release(cpu, target_iface);
        return false;
    };
    ddraw::add_ref(cpu, target_iface);
    install_buffers(cpu, index, (target, target_iface), (depth, depth_iface), size);
    true
}

/// Give the context at `index` and its inner device the color buffer
/// `target` and the depth buffer `depth` (ids and held interfaces, the
/// color buffer's held twice, for the context and the device), the
/// window's client area being `size`, and clear them.
fn install_buffers(
    cpu: &mut Cpu,
    index: usize,
    target: (u32, u32),
    depth: (u32, u32),
    size: (u32, u32),
) {
    let inner = state(cpu).contexts[index].as_ref().map_or(0, |c| c.inner);
    if let Some(i) = inner_device::get(cpu, inner) {
        (i.target, i.target_iface) = target;
        i.depth = depth.0;
    }
    if let Some(Some(c)) = state(cpu).contexts.get_mut(index) {
        (c.target, c.target_iface) = target;
        (c.depth, c.depth_iface) = depth;
        (c.width, c.height) = size;
    }
    super::draw::clear_buffers(cpu, index, true, true);
}

/// Whether the context at `index` draws full screen on the Voodoo2:
/// its DirectDraw object is the card's.
fn full_screen(cpu: &mut Cpu, index: usize) -> bool {
    let dd = state(cpu).contexts[index].as_ref().map_or(0, |c| c.dd);
    dd != 0 && directdraw::is_card(cpu, dd)
}

/// The full-screen context at `index` on the Voodoo2: open the card at
/// the smallest of its modes that holds the window's client area and a
/// depth buffer, with its front buffer, back buffer and depth buffer
/// as surfaces, unless it is open in one already. False if no mode
/// holds the window, or the card can't be opened: the card then gives
/// the VGA its picture back.
fn fit_card(cpu: &mut Cpu, index: usize) -> bool {
    let Some(c) = state(cpu).contexts[index].as_ref() else {
        return false;
    };
    let (window, dd_iface, target, depth) = (c.window, c.dd_iface, c.target, c.depth);
    let size = client_size(cpu, window);
    let lost = |cpu: &mut Cpu, id: u32| voodoo::card_buffer(cpu, id).is_none();
    let (mode_w, mode_h, _, _) = voodoo::current(cpu);
    let fits = size.0 <= mode_w && size.1 <= mode_h;
    if target != 0 && fits && !lost(cpu, target) && !lost(cpu, depth) {
        if let Some(Some(c)) = state(cpu).contexts.get_mut(index) {
            (c.width, c.height) = size;
        }
        return true;
    }
    drop_buffers(cpu, index);
    if open_card(cpu, index, size).is_some() {
        return true;
    }
    // Out of exclusive mode, which closes the card if it is open.
    directdraw::set_cooperative_level(cpu, &[dd_iface, 0, DDSCL_NORMAL]);
    false
}

/// Open the card for the context at `index`, whose window's client
/// area is `size`, and give the context its buffers: None if no mode
/// holds the window, or the card can't be opened.
fn open_card(cpu: &mut Cpu, index: usize, size: (u32, u32)) -> Option<()> {
    let c = state(cpu).contexts[index].as_ref()?;
    let (window, dd, dd_iface, format) = (c.window, c.dd, c.dd_iface, c.format);
    let (w, h) = voodoo::depth_modes(cpu)
        .into_iter()
        .find(|&(w, h)| size.0 <= w && size.1 <= h)?;
    let level = DDSCL_EXCLUSIVE | DDSCL_FULLSCREEN | DDSCL_NOWINDOWCHANGES;
    let ok = Some(super::super::ddraw::DD_OK);
    if directdraw::set_cooperative_level(cpu, &[dd_iface, window, level]) != ok {
        return None;
    }
    if Some(voodoo::set_mode(cpu, dd, w, h, 16, 0)) != ok {
        return None;
    }
    // The primary surface, with a back buffer when the format is
    // double buffered.
    let double = double_buffered(format);
    let (flags, caps) = if double {
        (
            DDSD_CAPS | DDSD_BACKBUFFERCOUNT,
            DDSCAPS_PRIMARYSURFACE | DDSCAPS_3DDEVICE | DDSCAPS_FLIP | DDSCAPS_COMPLEX,
        )
    } else {
        (DDSD_CAPS, DDSCAPS_PRIMARYSURFACE | DDSCAPS_3DDEVICE)
    };
    let desc = Desc {
        flags,
        backbuffers: double as u32,
        caps: [caps, 0, 0, 0],
        ..Desc::default()
    };
    let ids = dd_surface::make(cpu, dd, 7, level, &desc).ok()?;
    let primary_iface = ddraw::iface(cpu, ids[0], V_SURFACE7);
    let target = ids[ids.len() - 1];
    let target_iface = ddraw::iface(cpu, target, V_SURFACE7);
    if primary_iface == 0 || target_iface == 0 {
        ddraw::destroy(cpu, ids[0]);
        return None;
    }
    ddraw::add_ref(cpu, primary_iface);
    // One for the context, one for the device.
    ddraw::add_ref(cpu, target_iface);
    ddraw::add_ref(cpu, target_iface);
    let Some(depth) = make_buffer(cpu, dd, (w, h), true) else {
        ddraw::release(cpu, target_iface);
        ddraw::release(cpu, target_iface);
        ddraw::release(cpu, primary_iface);
        return None;
    };
    if let Some(Some(c)) = state(cpu).contexts.get_mut(index) {
        c.primary_iface = primary_iface;
    }
    install_buffers(cpu, index, (target, target_iface), depth, size);
    Some(())
}

/// Let go of the context's color and depth buffers.
fn drop_buffers(cpu: &mut Cpu, index: usize) {
    let Some(Some(c)) = state(cpu).contexts.get_mut(index) else {
        return;
    };
    let target_iface = std::mem::take(&mut c.target_iface);
    let depth_iface = std::mem::take(&mut c.depth_iface);
    let primary_iface = std::mem::take(&mut c.primary_iface);
    c.target = 0;
    c.depth = 0;
    let inner = c.inner;
    let held = inner_device::get(cpu, inner).map(|i| {
        i.target = 0;
        i.depth = 0;
        std::mem::take(&mut i.target_iface)
    });
    for iface in [held.unwrap_or(0), target_iface, depth_iface, primary_iface] {
        if iface != 0 {
            ddraw::release(cpu, iface);
        }
    }
}

/// wglCreateContext(hdc): a context for the DC's window, which has a
/// pixel format, drawn by the card.
pub fn create_context(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hdc = args[0];
    let hwnd = window_of(cpu, hdc);
    if cpu.bus.win32.gdi.dc(hdc).is_none() {
        super::super::gdi32::set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    }
    let format = format_of(cpu, hwnd);
    let icd = icd(cpu);
    if icd == Icd::None || hwnd == 0 || format == 0 {
        super::super::gdi32::set_error(cpu, ERROR_INVALID_PIXEL_FORMAT);
        return Some(0);
    }
    let Some(strings) = write_strings(cpu) else {
        super::super::gdi32::set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
        return Some(0);
    };

    // The DirectDraw object: in a window, or the Voodoo2's, which takes
    // the card full screen when the context is first made current.
    let card = icd == Icd::FullScreen;
    let dd = directdraw::new_ddraw(cpu, true, card);
    let dd_iface = ddraw::iface(cpu, dd, V_DD7);
    if dd_iface == 0 {
        ddraw::destroy(cpu, dd);
        super::super::kernel32::free_pages(cpu, strings);
        return Some(0);
    }
    ddraw::add_ref(cpu, dd_iface);
    if !card {
        directdraw::set_cooperative_level(cpu, &[dd_iface, hwnd, DDSCL_NORMAL]);
    }

    // The Direct3D device that draws.
    let tmus = super::super::ddraw::d3d::hal(cpu, dd).unwrap_or(1);
    let inner = Inner::new(8, dd, 0, 0, tmus);
    let inner_id = ddraw::add_object(
        cpu,
        ddraw::Object {
            owner: dd,
            d3d: Some(D3dObject {
                device: Some(inner),
                ..D3dObject::default()
            }),
            ..ddraw::Object::default()
        },
    );
    let inner_iface = ddraw::iface(cpu, inner_id, V_DEVICE7);
    ddraw::add_ref(cpu, inner_iface);
    hal::created(cpu, dd);

    let pid = super::super::user32::current_pid(cpu);
    let mut c = new_context(pid, hwnd, format);
    c.dd = dd;
    c.dd_iface = dd_iface;
    c.inner = inner_id;
    c.inner_iface = inner_iface;
    c.strings = strings;
    let s = state(cpu);
    let index = match s.contexts.iter().position(Option::is_none) {
        Some(i) => {
            s.contexts[i] = Some(c);
            i
        }
        None => {
            s.contexts.push(Some(c));
            s.contexts.len() - 1
        }
    };
    if !card && !fit_buffers(cpu, index) {
        destroy(cpu, index);
        super::super::gdi32::set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
        return Some(0);
    }
    Some(hglrc(index))
}

/// The context at `index` goes, with what it holds.
fn destroy(cpu: &mut Cpu, index: usize) {
    super::texture::release_all(cpu, index);
    drop_buffers(cpu, index);
    let Some(c) = state(cpu).contexts.get_mut(index).and_then(Option::take) else {
        return;
    };
    let h = hglrc(index);
    state(cpu).current.retain(|x| x.1 != h);
    if c.inner_iface != 0 {
        ddraw::release(cpu, c.inner_iface);
    }
    if c.dd_iface != 0 {
        ddraw::release(cpu, c.dd_iface);
    }
    if c.strings != 0 {
        super::super::kernel32::free_pages(cpu, c.strings);
    }
}

/// wglCreateLayerContext(hdc, layer): the main plane's is a context.
pub fn create_layer_context(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[1] != 0 {
        super::super::gdi32::set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    create_context(cpu, &args[..1])
}

/// wglDeleteContext(hglrc).
pub fn delete_context(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(index) = context_index(cpu, args[0]) else {
        super::super::gdi32::set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    destroy(cpu, index);
    Some(TRUE)
}

/// wglMakeCurrent(hdc, hglrc): the context current in this thread,
/// drawing to the DC's window, or none with a NULL context.
pub fn make_current(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hdc, h] = args[..] else {
        return Some(FALSE);
    };
    let tid = super::super::user32::current_tid(cpu);
    if h == 0 {
        state(cpu).current.retain(|c| c.0 != tid);
        return Some(TRUE);
    }
    let Some(index) = context_index(cpu, h) else {
        super::super::gdi32::set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    // Current in another thread.
    if state(cpu).current.iter().any(|c| c.1 == h && c.0 != tid) {
        super::super::gdi32::set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    let hwnd = window_of(cpu, hdc);
    let format = state(cpu).contexts[index].as_ref().map_or(0, |c| c.format);
    if hwnd == 0 || format_of(cpu, hwnd) != format {
        super::super::gdi32::set_error(cpu, ERROR_INVALID_PIXEL_FORMAT);
        return Some(FALSE);
    }
    let first = {
        let c = state(cpu).contexts[index].as_mut()?;
        c.window = hwnd;
        c.viewport == [0; 4] && c.scissor == [0; 4]
    };
    if !fit_buffers(cpu, index) {
        super::super::gdi32::set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
        return Some(FALSE);
    }
    let s = state(cpu);
    s.current.retain(|c| c.0 != tid);
    s.current.push((tid, h, hdc));
    // The viewport and scissor box start as the whole window the first
    // time the context is made current.
    if first && let Some(c) = s.contexts[index].as_mut() {
        c.viewport = [0, 0, c.width as i32, c.height as i32];
        c.scissor = c.viewport;
    }
    Some(TRUE)
}

/// wglGetCurrentContext().
pub fn get_current_context(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(current(cpu).map_or(0, |(i, _)| hglrc(i)))
}

/// wglGetCurrentDC().
pub fn get_current_dc(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(current(cpu).map_or(0, |(_, dc)| dc))
}

/// wglGetProcAddress(name): OpenGL's functions and the extensions'.
pub fn get_proc_address(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(name) = super::super::winmm::read_string(cpu, args[0], 256) else {
        return Some(0);
    };
    let Some(dll) = super::super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
    else {
        return Some(0);
    };
    let found = super::exports::APIS.iter().position(|a| a.name == name);
    Some(found.map_or(0, |i| super::super::stubs::stub_address(dll, i)))
}

/// wglGetDefaultProcAddress(name): none.
pub fn get_default_proc_address(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// What the ICD doesn't have: layer planes, fonts, copying contexts.
pub fn fail(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(FALSE)
}

/// wglCopyContext(from, to, mask).
pub fn copy_context(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(FALSE)
}

/// wglShareLists(a, b): the contexts' textures aren't shared, which
/// programs with one context don't miss.
pub fn share_lists(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ok = context_index(cpu, args[0]).is_some() && context_index(cpu, args[1]).is_some();
    Some(ok as u32)
}

/// SwapBuffers(hdc): show what the context drawing to the DC's window
/// drew. For GDI's SwapBuffers and wglSwapBuffers.
///
/// A double-buffered context in a window swaps as 3dfx's ICD does with
/// its swap interval (1 unless the program set another): no more than
/// once in that many frames of the display. The swap shows at once, and
/// a later one called before the next frame's picture begins (that
/// many frames on) waits for it, the call coming back then. The card's
/// own swaps on the Voodoo2 wait for the retrace in the card
/// (`swap_card`). Without it a game that draws a frame in less than a
/// frame's time, as Oni's menus do, swapped hundreds of times a second
/// of emulated time, and each swap's drawing cost the host as much as
/// one at the display's rate.
fn swap_paced(cpu: &mut Cpu, hdc: u32) -> Option<u32> {
    let Some(index) = dc_context(cpu, hdc) else {
        return Some(FALSE);
    };
    let paced = state(cpu).contexts[index]
        .as_ref()
        .filter(|c| c.swap_interval > 0 && double_buffered(c.format))
        .map(|c| (c.swap_interval, c.swap_at));
    let paced = paced.filter(|_| !full_screen(cpu, index));
    if let Some((_, at)) = paced
        && at.div_ceil(1000) > cpu.bus.clock.now_micros()
    {
        // The call runs again when the retrace comes.
        cpu.set_eip(cpu.eip().wrapping_sub(4));
        return directdraw::wait_until(cpu, at, TRUE);
    }
    present(cpu, index);
    if let Some((interval, _)) = paced {
        let (timing, now) = ddraw::display::timing(cpu);
        let at = timing.next_display(now) + u64::from(interval - 1) * timing.frame_ns();
        if let Some(Some(c)) = state(cpu).contexts.get_mut(index) {
            c.swap_at = at;
        }
    }
    Some(TRUE)
}

/// The context whose drawing the DC `hdc`'s window shows: the running
/// thread's, or another of the window's.
fn dc_context(cpu: &mut Cpu, hdc: u32) -> Option<usize> {
    let hwnd = window_of(cpu, hdc);
    let tid = super::super::user32::current_tid(cpu);
    let s = state(cpu);
    let mine = s.current.iter().find(|c| c.0 == tid).map(|c| c.1);
    mine.and_then(|h| h.checked_sub(hglrc(0)))
        .map(|i| i as usize)
        .filter(|&i| {
            s.contexts
                .get(i)
                .and_then(|c| c.as_ref())
                .is_some_and(|c| c.window == hwnd)
        })
        .or_else(|| {
            s.contexts
                .iter()
                .position(|c| c.as_ref().is_some_and(|c| c.window == hwnd))
        })
}

/// Copy the context's color buffer into its window's client area,
/// where the window can be seen.
pub fn present(cpu: &mut Cpu, index: usize) {
    let Some((target, window)) = state(cpu).contexts[index]
        .as_ref()
        .map(|c| (c.target, c.window))
    else {
        return;
    };
    let card = full_screen(cpu, index);
    if card {
        swap_card(cpu, index);
    } else if target != 0 {
        shadow::expose(cpu, target);
        super::super::d3d8::copy::present_window(cpu, target, None, None, window);
        // A frame, as the card's buffer swaps are while it shows its
        // desktop, for `cycles=auto`.
        if let Some(v) = cpu.bus.voodoo.as_mut() {
            v.swapped += 1;
        }
    }
    if let Some(Some(c)) = state(cpu).contexts.get_mut(index) {
        c.drawn = false;
    }
    // A window whose size changed has buffers of its new size next. The
    // Voodoo2's card opens only for a context made current.
    let h = hglrc(index);
    if !card || state(cpu).current.iter().any(|c| c.1 == h) {
        fit_buffers(cpu, index);
    }
}

/// The full-screen context at `index` on the Voodoo2: the card swaps
/// its buffers at a retrace, or at once with a swap interval of 0. A
/// single-buffered context draws into the front buffer, which shows.
fn swap_card(cpu: &mut Cpu, index: usize) {
    let Some(c) = state(cpu).contexts[index].as_ref() else {
        return;
    };
    let (primary_iface, interval, format) = (c.primary_iface, c.swap_interval, c.format);
    if primary_iface == 0 {
        return;
    }
    if !double_buffered(format) {
        // Nothing swaps, but it is a frame, as a window's is, for
        // `cycles=auto`.
        if let Some(v) = cpu.bus.voodoo.as_mut() {
            v.swapped += 1;
        }
        return;
    }
    let flags = DDFLIP_WAIT | if interval == 0 { DDFLIP_NOVSYNC } else { 0 };
    dd_surface::flip(cpu, &[primary_iface, 0, flags]);
}

/// wglSwapBuffers(hdc).
pub fn swap_buffers(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    swap_paced(cpu, args[0])
}

/// wglSwapLayerBuffers(hdc, planes): the main plane's.
pub fn swap_layer_buffers(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    // WGL_SWAP_MAIN_PLANE.
    if args[1] & 1 == 0 {
        return Some(FALSE);
    }
    swap_paced(cpu, args[0])
}

/// wglSwapIntervalEXT(interval): the frames of the display a swap of
/// the current context waits for (`swap_paced`, `swap_card`), 0 for
/// none. A negative interval fails with ERROR_INVALID_DATA, as
/// WGL_EXT_swap_control has it: kept, it would put a window's next swap
/// years ahead.
pub fn swap_interval(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] > i32::MAX as u32 {
        super::super::gdi32::set_error(cpu, ERROR_INVALID_DATA);
        return Some(FALSE);
    }
    let Some(c) = super::ctx(cpu) else {
        return Some(FALSE);
    };
    c.swap_interval = args[0];
    Some(TRUE)
}

/// wglGetSwapIntervalEXT().
pub fn get_swap_interval(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(super::ctx(cpu).map_or(0, |c| c.swap_interval))
}

/// wglGetExtensionsStringEXT() and wglGetExtensionsStringARB(hdc).
pub fn get_extensions_string(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let at = super::ctx(cpu).map_or(0, |c| c.strings);
    Some(if at == 0 { 0 } else { at + WGL_EXTENSIONS_AT })
}

/// The viewport the inner device draws with: OpenGL's, from the
/// bottom left, cut to the context's buffers, as Direct3D's from their
/// top left. With it the scale and offset in clip space that keep
/// vertices where OpenGL's whole viewport puts them, for one that goes
/// past the buffers' edges (the identity for one inside them). None if
/// none of it is on the buffers.
pub fn inner_viewport(c: &super::Settings) -> Option<(Viewport7, Matrix)> {
    let [x, y, w, h] = c.viewport.map(i64::from);
    let (width, height) = (i64::from(c.width), i64::from(c.height));
    let (left, right) = (x.max(0), (x + w).min(width));
    let (bottom, top) = (y.max(0), (y + h).min(height));
    if right <= left || top <= bottom {
        return None;
    }
    let (cw, ch) = (right - left, top - bottom);
    let mut fit = IDENTITY;
    fit[0] = w as f32 / cw as f32;
    fit[5] = h as f32 / ch as f32;
    fit[12] = (2 * (x - left) + w - cw) as f32 / cw as f32;
    fit[13] = (2 * (y - bottom) + h - ch) as f32 / ch as f32;
    let viewport = Viewport7 {
        x: left as u32,
        y: (height - top) as u32,
        width: cw as u32,
        height: ch as u32,
        min_z: c.depth_range[0],
        max_z: c.depth_range[1],
    };
    Some((viewport, fit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_banshee_has_the_voodoo3s_extensions_but_multitexture() {
        let names = |s: &'static str| s.split(' ').collect::<Vec<_>>();
        let voodoo3 = names(icd_extensions(2));
        let banshee = names(icd_extensions(1));
        assert!(voodoo3.contains(&"GL_ARB_multitexture"));
        assert!(!banshee.contains(&"GL_ARB_multitexture"));
        // Myth II 1.8.3 takes an OpenGL 1.1 device only with GL_EXT_bgra.
        assert!(voodoo3.contains(&"GL_EXT_bgra"));
        assert!(banshee.contains(&"GL_EXT_bgra"));
        assert_eq!(banshee.len() + 1, voodoo3.len());
    }
}
