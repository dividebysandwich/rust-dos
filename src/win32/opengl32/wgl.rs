//! WGL: pixel formats, rendering contexts and swapping buffers, and the
//! ICD's side of GDI's ChoosePixelFormat, DescribePixelFormat,
//! SetPixelFormat, GetPixelFormat and SwapBuffers, which on Windows go
//! to the ICD of the DC's device.
//!
//! The ICD has the pixel formats 3dfx's driver had on a Banshee or
//! Voodoo3, which draw only in 16 bits: 5-6-5 color, single or double
//! buffered, with a 16-bit depth buffer and no stencil, alpha or
//! accumulation buffer. A window's pixel format is set once.

use super::super::ddraw::d3d::device::{self as inner_device, Device as Inner, Viewport7};
use super::super::ddraw::d3d::pipeline::{IDENTITY, Matrix};
use super::super::ddraw::d3d::{D3dObject, hal, shadow};
use super::super::ddraw::directdraw::{self, DDSCL_NORMAL};
use super::super::ddraw::surface::{
    self as dd_surface, DDPF_RGB, DDPF_ZBUFFER, DDSCAPS_3DDEVICE, DDSCAPS_OFFSCREENPLAIN,
    DDSCAPS_VIDEOMEMORY, DDSCAPS_ZBUFFER, DDSD_CAPS, DDSD_HEIGHT, DDSD_PIXELFORMAT, DDSD_WIDTH,
    Desc,
};
use super::super::ddraw::{self, Format, V_DD7, V_DEVICE7, V_SURFACE7};
use super::{Context, DLL, context_index, current, hglrc, new_context, state};
use crate::cpu::Cpu;

const TRUE: u32 = 1;
const FALSE: u32 = 0;

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_PIXEL_FORMAT: u32 = 2000;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;

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
/// table for cards before the Voodoo4.
const FORMATS: [u32; 2] = [
    PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL,
    PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER | PFD_SWAP_COPY,
];

/// Whether the ICD is there: the display adapter is a Banshee or
/// Voodoo3.
pub fn has_icd(cpu: &Cpu) -> bool {
    cpu.bus.vga.adapter.voodoo_board().is_some()
}

/// The formats there are.
pub fn describe_formats(cpu: &Cpu) -> u32 {
    if has_icd(cpu) {
        FORMATS.len() as u32
    } else {
        0
    }
}

/// Whether the pixel format `format` is double buffered.
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

/// Write the descriptor of the pixel format `format` (from 1) at `at`.
fn write_descriptor(cpu: &mut Cpu, at: u32, format: u32) {
    let flags = FORMATS[format as usize - 1];
    let mut b = [0u8; PFD_SIZE as usize];
    b[0..2].copy_from_slice(&(PFD_SIZE as u16).to_le_bytes());
    b[2..4].copy_from_slice(&1u16.to_le_bytes());
    b[4..8].copy_from_slice(&flags.to_le_bytes());
    // PFD_TYPE_RGBA, 16 bits of color: red 5 at 11, green 6 at 5, blue
    // 5 at 0, no alpha.
    b[8] = 0;
    b[9] = 16;
    b[10..18].copy_from_slice(&[5, 11, 6, 5, 5, 0, 0, 0]);
    // No accumulation buffer, depth 16, no stencil or aux buffers, the
    // main plane.
    b[23] = 16;
    cpu.bus.guest_write_bytes(at, &b);
}

/// The pixel format that best matches the descriptor at `at`, as
/// Mesa's wglChoosePixelFormat picks it: 0 for none.
fn choose(cpu: &mut Cpu, at: u32) -> u32 {
    if !has_icd(cpu) || at == 0 || !cpu.bus.guest_probe(at, PFD_SIZE as usize, false) {
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
    // 24 and 32 bits are taken as the 16 the card has.
    let color = if color == 24 || color == 32 {
        16
    } else {
        color
    };
    for (i, &have) in FORMATS.iter().enumerate() {
        if color > 0 && color != 16 {
            continue;
        }
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

/// The extensions, as glGetString(GL_EXTENSIONS) gives them.
pub const EXTENSIONS: &str = "GL_ARB_multitexture GL_EXT_compiled_vertex_array \
    GL_EXT_texture_env_add WGL_EXT_swap_control";
/// The WGL extensions, as wglGetExtensionsStringEXT gives them.
pub const WGL_EXTENSIONS: &str = "WGL_EXT_extensions_string WGL_EXT_swap_control";

/// Write the strings into a new page of the process: its address.
fn write_strings(cpu: &mut Cpu) -> Option<u32> {
    let board = super::super::glide::hw::board(cpu)?;
    let at = super::super::kernel32::alloc_pages(cpu, 0x1000)?;
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
    // A Banshee's one texture unit has no multitexture, as Mesa's
    // driver leaves it out with one.
    let extensions = if board.tmus > 1 {
        EXTENSIONS
    } else {
        EXTENSIONS.trim_start_matches("GL_ARB_multitexture ")
    };
    let strings = [
        (VENDOR_AT, "3Dfx Interactive Inc."),
        (RENDERER_AT, renderer.as_str()),
        (VERSION_AT, "1.1.0"),
        (EXTENSIONS_AT, extensions),
        (WGL_EXTENSIONS_AT, WGL_EXTENSIONS),
    ];
    for (offset, s) in strings {
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0);
        cpu.bus.guest_write_bytes(at + offset, &bytes);
    }
    Some(at)
}

/// A surface of the DirectDraw object `dd` in video memory, 16 bits:
/// the render target or the depth buffer. Its id and the interface of
/// it that is held.
fn make_buffer(cpu: &mut Cpu, dd: u32, size: (u32, u32), depth: bool) -> Option<(u32, u32)> {
    let level = directdraw::ddraw(cpu, dd).map_or(0, |d| d.level);
    let format = if depth {
        Format {
            flags: DDPF_ZBUFFER,
            bits: 16,
            g: 0xFFFF,
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
    inner: u32,
    target: u32,
    depth: u32,
    size: (u32, u32),
}

/// Make the context's color and depth buffers again if the window's
/// client area isn't their size, or they were lost: false if they
/// can't be made.
pub fn fit_buffers(cpu: &mut Cpu, index: usize) -> bool {
    let Some(c) = state(cpu).contexts[index].as_ref().map(|c| Buffers {
        window: c.window,
        dd: c.dd,
        inner: c.inner,
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
    if let Some(i) = inner_device::get(cpu, c.inner) {
        i.target = target;
        i.target_iface = target_iface;
        i.depth = depth;
    }
    if let Some(Some(c)) = state(cpu).contexts.get_mut(index) {
        c.target = target;
        c.target_iface = target_iface;
        c.depth = depth;
        c.depth_iface = depth_iface;
        c.width = size.0;
        c.height = size.1;
    }
    super::draw::clear_buffers(cpu, index, true, true);
    true
}

/// Let go of the context's color and depth buffers.
fn drop_buffers(cpu: &mut Cpu, index: usize) {
    let Some(Some(c)) = state(cpu).contexts.get_mut(index) else {
        return;
    };
    let target_iface = std::mem::take(&mut c.target_iface);
    let depth_iface = std::mem::take(&mut c.depth_iface);
    c.target = 0;
    c.depth = 0;
    let inner = c.inner;
    let held = inner_device::get(cpu, inner).map(|i| {
        i.target = 0;
        i.depth = 0;
        std::mem::take(&mut i.target_iface)
    });
    for iface in [held.unwrap_or(0), target_iface, depth_iface] {
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
    if !has_icd(cpu) || hwnd == 0 || format == 0 {
        super::super::gdi32::set_error(cpu, ERROR_INVALID_PIXEL_FORMAT);
        return Some(0);
    }
    let Some(strings) = write_strings(cpu) else {
        super::super::gdi32::set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
        return Some(0);
    };

    // The DirectDraw object, in a window.
    let dd = directdraw::new_ddraw(cpu, true, false);
    let dd_iface = ddraw::iface(cpu, dd, V_DD7);
    if dd_iface == 0 {
        ddraw::destroy(cpu, dd);
        super::super::kernel32::free_pages(cpu, strings);
        return Some(0);
    }
    ddraw::add_ref(cpu, dd_iface);
    directdraw::set_cooperative_level(cpu, &[dd_iface, hwnd, DDSCL_NORMAL]);

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
    hal::created(cpu);

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
    if !fit_buffers(cpu, index) {
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
pub fn swap_dc(cpu: &mut Cpu, hdc: u32) -> bool {
    let hwnd = window_of(cpu, hdc);
    let tid = super::super::user32::current_tid(cpu);
    let s = state(cpu);
    // The thread's context, or another of the window's.
    let mine = s.current.iter().find(|c| c.0 == tid).map(|c| c.1);
    let index = mine
        .and_then(|h| h.checked_sub(hglrc(0)))
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
        });
    let Some(index) = index else {
        return false;
    };
    present(cpu, index);
    true
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
    if target != 0 {
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
    // A window whose size changed has buffers of its new size next.
    fit_buffers(cpu, index);
}

/// wglSwapBuffers(hdc).
pub fn swap_buffers(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(swap_dc(cpu, args[0]) as u32)
}

/// wglSwapLayerBuffers(hdc, planes): the main plane's.
pub fn swap_layer_buffers(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    // WGL_SWAP_MAIN_PLANE.
    if args[1] & 1 == 0 {
        return Some(FALSE);
    }
    Some(swap_dc(cpu, args[0]) as u32)
}

/// wglSwapIntervalEXT(interval): kept. Frames are shown as they are
/// swapped.
pub fn swap_interval(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
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
pub fn inner_viewport(c: &Context) -> Option<(Viewport7, Matrix)> {
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
