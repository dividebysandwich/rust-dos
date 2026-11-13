//! OPENGL32.DLL: OpenGL 1.1, as Windows 98 had it with 3dfx's OpenGL
//! ICD for the Banshee and Voodoo3 installed: the WGL functions that
//! make contexts and swap buffers, and OpenGL drawn by the card. A
//! ToadForce has the same ICD, with its own vendor and renderer names.
//!
//! It is a layer over DirectDraw's Direct3D, as D3D8.DLL is. Each
//! context has a DirectDraw object of its own and a Direct3D device of
//! version 8 in DirectDraw's table, which draws with the HAL on the card
//! (`ddraw::d3d::hal`: on a 3dfx card through Glide's layer,
//! `glide::api`, and on a ToadForce with its own commands). Its color
//! buffer is a 16-bit render target and its depth buffer a 16-bit Z
//! buffer, both in video memory and the size of the window's client
//! area; SwapBuffers copies the color buffer into the client area where
//! the window can be seen, as Direct3D 8's windowed Present does. A
//! game that set a full-screen mode with ChangeDisplaySettings has a
//! window that covers the screen, so its frames fill the screen.
//!
//! OpenGL's state is kept here (`Context`) and put on the inner device
//! as it draws (`draw.rs`): enables, blending, the alpha and depth
//! tests, culling, fog and the texture environments become render
//! states and texture stages, the matrices its transforms, the viewport
//! and depth range its viewport. What Direct3D 7 has no state for, the
//! scissor box, polygon offset and the color mask, the inner device has
//! as fields of its own that the HAL puts on the card; the color mask
//! is also Direct3D 8's write mask, for a card that writes some
//! channels of a color and not others. Textures are DirectDraw texture
//! surfaces (`texture.rs`), 5-6-5 or 4-4-4-4 as 3dfx's ICD kept them,
//! with the mipmap levels the program gives; vertices from glBegin and
//! glEnd and from vertex arrays go through Direct3D's pipeline on the
//! processor (`ddraw::d3d::pipeline`), with OpenGL's clip space z of -w
//! to w taken to Direct3D's 0 to w.
//!
//! Every OpenGL 1.1 function is exported, since programs ask for all of
//! them with GetProcAddress (Quake III's QGL does). Those that aren't
//! here, such as display lists, evaluators, selection, feedback and the
//! accumulation buffer, do nothing and say so in the log. Lighting is
//! computed on the processor (`light.rs`, `shade.rs`), as a Voodoo3
//! driver had to with no transform and lighting hardware on the card,
//! and the vertices reach Direct3D's pipeline lit. The extensions are
//! GL_ARB_multitexture (on a Voodoo3, with its two texture units; a
//! Banshee has one and not the extension), GL_EXT_bgra,
//! GL_EXT_compiled_vertex_array, GL_EXT_texture_env_add and
//! WGL_EXT_swap_control.
//!
//! On a machine with a Voodoo2 beside a display adapter that has no
//! ICD of its own (one that is neither a Banshee, a Voodoo3 nor a
//! ToadForce), the DLL is 3dfx's Voodoo2 OpenGL driver (3DFXVGL.DLL),
//! which games of the time loaded themselves or had in their own folder
//! as OPENGL32.DLL: a context's DirectDraw object is the card's, full
//! screen, and its color and depth buffers are the card's back (or
//! front) buffer and auxiliary buffer, the screen's top left the
//! window's client area (`wgl.rs`). SwapBuffers swaps the card's
//! buffers. Beside one of those display adapters, OpenGL draws on the
//! display adapter, as Windows' OPENGL32.DLL used the ICD of the
//! display's driver.
//!
//! On a machine with none of these cards there is no ICD: the DLL
//! loads, but there are no pixel formats and wglCreateContext fails, as
//! on a machine with Windows' own OPENGL32.DLL and a card without an
//! OpenGL driver of its own.
//!
//! The pixel formats and wglChoosePixelFormat follow Mesa 6.2's 3dfx
//! driver (src/mesa/drivers/glide/fxwgl.c), which is under the MIT
//! license: Copyright (C) 1999-2003 Brian Paul, by David Bucciarelli,
//! Brian Paul, Daryll Strauss, Keith Whitwell, Daniel Borca and others.
//! The list of exports and their arguments is Wine 1.2.3's
//! (dlls/opengl32/opengl32.spec), and wglGetProcAddress and glGetString
//! follow its wgl.c, which are under the GNU Lesser General Public
//! License 2.1 or later: Copyright (c) 1999 Lionel Ulmer, Copyright (c)
//! 2005 Raphael Junqueira.

mod attrib;
mod draw;
mod exports;
mod imm;
mod light;
mod matrix;
mod shade;
mod state;
mod texture;
pub mod wgl;

pub use matrix::{mult as mult_matrix, ortho};
pub use state::get_string;
pub use texture::{GL_TEXTURE_2D, rgba_of, tex_image_rgba};

use super::ddraw::d3d::pipeline::{IDENTITY, Matrix};
use super::ddraw::stamp::Stamped;
use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "OPENGL32.DLL",
    base: 0xBF74_0000,
    apis: exports::APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: &[],
};

const fn stdcall(
    name: &'static str,
    ordinal: u16,
    args: u8,
    handler: super::stubs::Handler,
) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

// Errors (glGetError).
pub const GL_NO_ERROR: u32 = 0;
pub const GL_INVALID_ENUM: u32 = 0x0500;
pub const GL_INVALID_VALUE: u32 = 0x0501;
pub const GL_INVALID_OPERATION: u32 = 0x0502;
pub const GL_STACK_OVERFLOW: u32 = 0x0503;
pub const GL_STACK_UNDERFLOW: u32 = 0x0504;
pub const GL_OUT_OF_MEMORY: u32 = 0x0505;

/// The texture units: two, as many as a Voodoo3 has.
pub const UNITS: usize = 2;
/// How deep the matrix stacks are, as the ICD has them.
pub const MODELVIEW_DEPTH: usize = 32;
pub const PROJECTION_DEPTH: usize = 2;
pub const TEXTURE_DEPTH: usize = 2;
/// The largest texture: the card's 256 by 256.
pub const MAX_TEXTURE_SIZE: u32 = 256;

/// A vertex array: whether it is on, and glXxxPointer's size, type,
/// stride and address.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Array {
    pub enabled: bool,
    pub size: u32,
    pub kind: u32,
    pub stride: u32,
    pub at: u32,
}

/// A vertex as glBegin and glEnd gather it: the position, the color
/// (ARGB) and the color a back face takes, which differs from it only
/// with two-sided lighting, and each unit's texture coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImmVertex {
    pub position: [f32; 4],
    pub color: u32,
    pub back: u32,
    pub tex: [[f32; 4]; UNITS],
}

/// A texture object: its name, the levels it has (sizes, the format
/// they were given in), its DirectDraw surface and parameters.
#[derive(Clone, Debug, Default)]
pub struct Texture {
    pub name: u32,
    /// The size of level 0, and the internal format glTexImage2D gave.
    pub width: u32,
    pub height: u32,
    pub internal: u32,
    /// Whether the texels have alpha: 4-4-4-4, or 5-6-5 without.
    pub alpha: bool,
    /// The levels given so far, a bit each.
    pub defined: u32,
    /// The surfaces of its levels (DirectDraw's), and the interface of
    /// the first that it holds.
    pub levels: Vec<u32>,
    pub iface: u32,
    pub min_filter: u32,
    pub mag_filter: u32,
    pub wrap_s: u32,
    pub wrap_t: u32,
}

/// A rendering context: what its draws are made with (`Settings`, whose
/// fields are read and written as the context's own), and the rest.
///
/// The first is stamped (`ddraw::stamp::Stamped`): every change to it
/// gives the context a new stamp, and the Direct3D state made from it
/// for one draw (`draw::Applied`) is the next draw's while the stamp
/// is the same. The fields here are those no such state is made from:
/// what glVertex and its like set many times between two draws (the
/// current color, the vertices under way), the lights and texture
/// matrices, which the vertices go through here, and what only other
/// functions read.
#[derive(Clone, Debug, Default)]
pub struct Context {
    settings: Stamped<Settings>,
    /// The Direct3D state of `settings`, with the stamp it was made
    /// under (`draw::applied_of`). Not saved: it is made again.
    pub(super) applied: Option<(u64, Option<draw::Applied>)>,
    /// The strings glGetString gives, in the process's memory.
    pub strings: u32,
    pub error: u32,
    pub matrix_mode: u32,
    pub texture_matrix: Vec<Vec<Matrix>>,
    pub clear_color: [f32; 4],
    pub clear_depth: f32,
    pub clear_stencil: u32,
    /// The pixel store: unpack's and pack's alignment, row length, rows
    /// and pixels skipped.
    pub unpack: [u32; 4],
    pub pack: [u32; 4],
    pub active_unit: u32,
    pub client_unit: u32,
    /// The lights, the materials and the lighting model.
    pub lighting: light::Lighting,
    /// glPushAttrib's stack.
    pub attribs: Vec<attrib::Attrib>,
    /// The current color, texture coordinates and normal.
    pub color: [f32; 4],
    pub tex_coord: [[f32; 4]; UNITS],
    pub normal: [f32; 3],
    /// The vertex, color, normal and texture coordinate arrays.
    pub vertex_array: Array,
    pub color_array: Array,
    pub normal_array: Array,
    pub tex_array: [Array; UNITS],
    /// glBegin's primitive under way and its vertices.
    pub begin: Option<u32>,
    pub vertices: Vec<ImmVertex>,
    pub swap_interval: u32,
    /// The time (ns) a window's next swap waits for: the start of the
    /// frame's picture the swap interval's frames after the last swap.
    pub swap_at: u64,
    /// Whether it drew since the buffers were last swapped or flushed.
    pub drawn: bool,
}

impl std::ops::Deref for Context {
    type Target = Settings;

    fn deref(&self) -> &Settings {
        &self.settings
    }
}

/// Any change to what draws are made with: the context's stamp changes
/// with it.
impl std::ops::DerefMut for Context {
    fn deref_mut(&mut self) -> &mut Settings {
        &mut self.settings
    }
}

impl Context {
    /// The stamp of what the context's draws are made with: the same
    /// for as long as none of it changed.
    pub fn stamp(&self) -> u64 {
        Stamped::stamp(&self.settings)
    }
}

/// What a context's draws are made with: its buffers, matrices,
/// viewport, capabilities and textures.
#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub pid: u32,
    /// The window it draws into, and its pixel format.
    pub window: u32,
    pub format: u32,
    /// Its DirectDraw object and the IDirectDraw7 it holds of it.
    pub dd: u32,
    pub dd_iface: u32,
    /// The Direct3D device that draws, and the interface it holds of it.
    pub inner: u32,
    pub inner_iface: u32,
    /// The color and depth buffers (DirectDraw's surfaces) and the
    /// interfaces of them it holds; their size.
    pub target: u32,
    pub target_iface: u32,
    pub depth: u32,
    pub depth_iface: u32,
    /// On the Voodoo2, the interface it holds of the primary surface
    /// (the card's front buffer, with the back buffer the color buffer
    /// is when the format is double buffered).
    pub primary_iface: u32,
    /// The size of the window's client area: the color and depth
    /// buffers', or on the Voodoo2 the part of the card's screen at
    /// its top left that OpenGL draws in.
    pub width: u32,
    pub height: u32,
    pub modelview: Vec<Matrix>,
    pub projection: Vec<Matrix>,
    /// x, y, width and height, from the bottom left.
    pub viewport: [i32; 4],
    pub depth_range: [f32; 2],
    pub scissor: [i32; 4],
    /// The capabilities glEnable turns on (`CAP_`), and texturing of
    /// each unit.
    pub caps: u32,
    pub texture_2d: [bool; UNITS],
    pub blend: [u32; 2],
    pub alpha_func: u32,
    pub alpha_ref: f32,
    pub depth_func: u32,
    pub depth_mask: bool,
    pub cull_face: u32,
    pub front_face: u32,
    pub offset: [f32; 2],
    pub polygon_mode: u32,
    pub shade_model: u32,
    pub color_mask: [bool; 4],
    pub fog_mode: u32,
    /// Fog's density, start and end.
    pub fog: [f32; 3],
    pub fog_color: [f32; 4],
    pub line_width: f32,
    pub point_size: f32,
    pub draw_buffer: u32,
    pub read_buffer: u32,
    pub env_mode: [u32; UNITS],
    pub env_color: [[f32; 4]; UNITS],
    /// The texture bound to each unit, by its name (0 for the default
    /// texture).
    pub bound: [u32; UNITS],
    pub textures: Vec<Texture>,
}

/// OpenGL's state: the system's.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// The contexts, by their handles' numbers.
    pub contexts: Vec<Option<Context>>,
    /// The context each thread has current, and the DC it draws to:
    /// (thread, context, DC).
    pub current: Vec<(u32, u32, u32)>,
    /// The pixel format of each window that has one (window, format).
    pub formats: Vec<(u32, u32)>,
    /// The draws that wait to go to Direct3D as one (`draw::Waiting`).
    pub waiting: Option<draw::Waiting>,
    /// The functions that do nothing whose first call was logged.
    pub logged: Vec<u16>,
    /// The same for GLU32.DLL (glu32/), which works on OPENGL32's
    /// contexts.
    pub glu_logged: Vec<u16>,
}

pub fn state(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.opengl32
}

/// Handles of contexts are this plus their places.
const HGLRC_BASE: u32 = 0x0001_0000;

/// The place of the context `hglrc`, if there is one.
pub fn context_index(cpu: &mut Cpu, hglrc: u32) -> Option<usize> {
    let i = hglrc.checked_sub(HGLRC_BASE)? as usize;
    state(cpu).contexts.get(i)?.as_ref()?;
    Some(i)
}

/// The handle of the context at `index`.
pub fn hglrc(index: usize) -> u32 {
    HGLRC_BASE + index as u32
}

/// The running thread's current context, and the DC it draws to.
pub fn current(cpu: &mut Cpu) -> Option<(usize, u32)> {
    let tid = super::user32::current_tid(cpu);
    let s = state(cpu);
    let &(_, h, dc) = s.current.iter().find(|c| c.0 == tid)?;
    let i = h.checked_sub(HGLRC_BASE)? as usize;
    s.contexts.get(i)?.as_ref()?;
    Some((i, dc))
}

/// The running thread's current context.
pub fn ctx(cpu: &mut Cpu) -> Option<&mut Context> {
    let (i, _) = current(cpu)?;
    state(cpu).contexts.get_mut(i)?.as_mut()
}

/// Record the error `e` in the current context, unless one is there
/// already: glGetError gives the first.
pub fn error(cpu: &mut Cpu, e: u32) {
    if let Some(c) = ctx(cpu)
        && c.error == GL_NO_ERROR
    {
        c.error = e;
    }
}

/// The argument at `i` as a float.
pub fn f(args: &[u32], i: usize) -> f32 {
    f32::from_bits(args.get(i).copied().unwrap_or(0))
}

/// The two arguments at `i` as a double.
pub fn d(args: &[u32], i: usize) -> f64 {
    let lo = args.get(i).copied().unwrap_or(0) as u64;
    let hi = args.get(i + 1).copied().unwrap_or(0) as u64;
    f64::from_bits(hi << 32 | lo)
}

/// A signed integer `c` of `bits` bits as table 2.6 converts it to a
/// float from -1 to 1: (2c + 1) / (2^bits - 1).
pub fn signed_unit(c: f64, bits: u32) -> f32 {
    ((2.0 * c + 1.0) / ((1u64 << bits) - 1) as f64) as f32
}

/// Read `n` floats at `at`.
pub fn read_floats(cpu: &mut Cpu, at: u32, n: usize) -> Option<Vec<f32>> {
    if at == 0 || !cpu.bus.guest_probe(at, n * 4, false) {
        return None;
    }
    Some(
        (0..n as u32)
            .map(|i| f32::from_bits(cpu.bus.guest_read_32(at + 4 * i)))
            .collect(),
    )
}

/// Read `n` doubles at `at`.
pub fn read_doubles(cpu: &mut Cpu, at: u32, n: usize) -> Option<Vec<f64>> {
    if at == 0 || !cpu.bus.guest_probe(at, n * 8, false) {
        return None;
    }
    Some(
        (0..n as u32)
            .map(|i| {
                let lo = cpu.bus.guest_read_32(at + 8 * i) as u64;
                let hi = cpu.bus.guest_read_32(at + 8 * i + 4) as u64;
                f64::from_bits(hi << 32 | lo)
            })
            .collect(),
    )
}

/// A context's state as a new one has it, for the window `window` of
/// the pixel format `format`.
fn new_context(pid: u32, window: u32, format: u32) -> Context {
    let settings = Settings {
        pid,
        window,
        format,
        modelview: vec![IDENTITY],
        projection: vec![IDENTITY],
        depth_range: [0.0, 1.0],
        texture_2d: [false; UNITS],
        blend: [state::GL_ONE, state::GL_ZERO],
        alpha_func: state::GL_ALWAYS,
        depth_func: state::GL_LESS,
        depth_mask: true,
        cull_face: state::GL_BACK,
        front_face: state::GL_CCW,
        polygon_mode: state::GL_FILL,
        shade_model: state::GL_SMOOTH,
        color_mask: [true; 4],
        fog_mode: state::GL_EXP,
        fog: [1.0, 0.0, 1.0],
        line_width: 1.0,
        point_size: 1.0,
        draw_buffer: state::GL_BACK_BUFFER,
        read_buffer: state::GL_BACK_BUFFER,
        env_mode: [texture::GL_MODULATE; UNITS],
        caps: CAP_DITHER,
        ..Settings::default()
    };
    Context {
        settings: Stamped::new(settings),
        matrix_mode: matrix::GL_MODELVIEW,
        texture_matrix: vec![vec![IDENTITY]; UNITS],
        clear_depth: 1.0,
        unpack: [4, 0, 0, 0],
        pack: [4, 0, 0, 0],
        active_unit: 0,
        client_unit: 0,
        color: [1.0; 4],
        tex_coord: [[0.0, 0.0, 0.0, 1.0]; UNITS],
        normal: [0.0, 0.0, 1.0],
        swap_interval: 1,
        ..Context::default()
    }
}

// What glEnable turns on, a bit each.
pub const CAP_BLEND: u32 = 1;
pub const CAP_ALPHA_TEST: u32 = 2;
pub const CAP_DEPTH_TEST: u32 = 4;
pub const CAP_CULL_FACE: u32 = 8;
pub const CAP_POLYGON_OFFSET_FILL: u32 = 0x10;
pub const CAP_SCISSOR_TEST: u32 = 0x20;
pub const CAP_FOG: u32 = 0x40;
pub const CAP_DITHER: u32 = 0x80;
pub const CAP_LIGHTING: u32 = 0x100;
pub const CAP_STENCIL_TEST: u32 = 0x200;
pub const CAP_COLOR_MATERIAL: u32 = 0x400;
pub const CAP_NORMALIZE: u32 = 0x800;
pub const CAP_LINE_SMOOTH: u32 = 0x1000;
pub const CAP_POLYGON_OFFSET_LINE: u32 = 0x2000;
pub const CAP_POLYGON_OFFSET_POINT: u32 = 0x4000;
pub const CAP_CLIP_PLANES: u32 = 0x8000;
pub const CAP_TEXTURE_GEN: u32 = 0x1_0000;
pub const CAP_POLYGON_SMOOTH: u32 = 0x2_0000;
pub const CAP_POINT_SMOOTH: u32 = 0x4_0000;
pub const CAP_LINE_STIPPLE: u32 = 0x8_0000;
pub const CAP_POLYGON_STIPPLE: u32 = 0x10_0000;
pub const CAP_LOGIC_OP: u32 = 0x20_0000;

/// A function that isn't here: it does nothing, and its first call is
/// logged. Its name is the stub's that called it.
fn missing(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let base = DLL.base + 0x1000;
    let stub = cpu.eip().wrapping_sub(4).wrapping_sub(base) / super::stubs::STUB;
    let id = stub as u16;
    if super::trace3d::on(cpu) {
        let name = exports::APIS.get(stub as usize).map_or("?", |a| a.name);
        super::trace3d::gl_missing(cpu, name);
    }
    if !state(cpu).logged.contains(&id) {
        state(cpu).logged.push(id);
        let name = exports::APIS.get(stub as usize).map_or("?", |a| a.name);
        cpu.bus.log_string(&format!(
            "[W32] OPENGL32.{}: not implemented, ignored",
            name
        ));
    }
    Some(0)
}

/// A process ended: its contexts go. DirectDraw's state goes with its
/// own `process_ended`, which takes the objects they had there.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let s = state(cpu);
    for slot in s.contexts.iter_mut() {
        if slot.as_ref().is_some_and(|c| c.pid == pid) {
            *slot = None;
        }
    }
    let contexts = std::mem::take(&mut s.contexts);
    s.current.retain(|c| {
        c.1 >= HGLRC_BASE
            && contexts
                .get((c.1 - HGLRC_BASE) as usize)
                .is_some_and(Option::is_some)
    });
    s.contexts = contexts;
}

crate::state_fields!(Array {
    enabled,
    size,
    kind,
    stride,
    at
});
crate::state_fields!(ImmVertex {
    position,
    color,
    back,
    tex
});
crate::state_fields!(Texture {
    name,
    width,
    height,
    internal,
    alpha,
    defined,
    levels,
    iface,
    min_filter,
    mag_filter,
    wrap_s,
    wrap_t
});
crate::state_fields!(Settings {
    pid,
    window,
    format,
    dd,
    dd_iface,
    inner,
    inner_iface,
    target,
    target_iface,
    depth,
    depth_iface,
    primary_iface,
    width,
    height,
    modelview,
    projection,
    viewport,
    depth_range,
    scissor,
    caps,
    texture_2d,
    blend,
    alpha_func,
    alpha_ref,
    depth_func,
    depth_mask,
    cull_face,
    front_face,
    offset,
    polygon_mode,
    shade_model,
    color_mask,
    fog_mode,
    fog,
    fog_color,
    line_width,
    point_size,
    draw_buffer,
    read_buffer,
    env_mode,
    env_color,
    bound,
    textures
});
crate::state_fields!(Context {
    settings,
    strings,
    error,
    matrix_mode,
    texture_matrix,
    clear_color,
    clear_depth,
    clear_stencil,
    unpack,
    pack,
    active_unit,
    client_unit,
    lighting,
    attribs,
    color,
    tex_coord,
    normal,
    vertex_array,
    color_array,
    normal_array,
    tex_array,
    begin,
    vertices,
    swap_interval,
    swap_at,
    drawn
} skip { applied });
crate::state_fields!(State {
    contexts,
    current,
    formats,
    waiting
} skip { logged, glu_logged });

/// Whether the draws that wait (`draw::Waiting`) may go on waiting
/// through a call of the built-in function `id`: the function reads
/// neither the buffers nor a texture's image, changes neither, and
/// frees nothing the draws name. Those are the functions that give
/// vertices and draw them, which add to what waits or flush it
/// themselves, and the ones that only set or read a context's state,
/// which the draws hold by value. Every other function of any DLL
/// flushes: one this list forgets costs a flush, and no picture.
pub fn keeps_waiting(id: u16) -> bool {
    /// The functions by the start of their names.
    const STARTS: &[&str] = &[
        "glVertex",
        "glColor",
        "glTexCoord",
        "glMultiTexCoord",
        "glNormal",
        "glRect",
        "glTexEnv",
        "glTexParameter",
        "glFog",
        "glLight",
        "glMaterial",
        "glRotate",
        "glScale",
        "glTranslate",
        "glLoadMatrix",
        "glMultMatrix",
        "glStencil",
    ];
    const NAMES: &[&str] = &[
        "glBegin",
        "glEnd",
        "glArrayElement",
        "glDrawArrays",
        "glDrawElements",
        "glInterleavedArrays",
        "glVertexPointer",
        "glNormalPointer",
        "glEdgeFlagPointer",
        "glIndexPointer",
        "glLockArraysEXT",
        "glUnlockArraysEXT",
        "glEnableClientState",
        "glDisableClientState",
        "glBindTexture",
        "glActiveTextureARB",
        "glEnable",
        "glDisable",
        "glAlphaFunc",
        "glBlendFunc",
        "glDepthFunc",
        "glDepthMask",
        "glDepthRange",
        "glCullFace",
        "glFrontFace",
        "glShadeModel",
        "glPolygonMode",
        "glPolygonOffset",
        "glLineWidth",
        "glPointSize",
        "glHint",
        "glMatrixMode",
        "glLoadIdentity",
        "glPushMatrix",
        "glPopMatrix",
        "glOrtho",
        "glFrustum",
        "glViewport",
        "glScissor",
        "glPushAttrib",
        "glPopAttrib",
        "glGetError",
    ];
    static BITS: std::sync::OnceLock<(u16, Vec<u64>)> = std::sync::OnceLock::new();
    let (first, bits) = BITS.get_or_init(|| {
        let dll = super::stubs::DLLS
            .iter()
            .position(|dll| std::ptr::eq(*dll, &DLL))
            .expect("OPENGL32 is a built-in DLL");
        let mut bits = vec![0u64; DLL.apis.len().div_ceil(64)];
        for (at, api) in DLL.apis.iter().enumerate() {
            let keeps = NAMES.contains(&api.name) || STARTS.iter().any(|s| api.name.starts_with(s));
            // A function without a handler of its own does nothing,
            // which is no reason to know what it would have done.
            if keeps && !std::ptr::fn_addr_eq(api.handler, missing as super::stubs::Handler) {
                bits[at / 64] |= 1 << (at % 64);
            }
        }
        (super::stubs::id(dll, 0), bits)
    });
    let at = id.wrapping_sub(*first) as usize;
    bits.get(at / 64).is_some_and(|word| word >> (at % 64) & 1 != 0)
}

pub use draw::flush_waiting;

#[cfg(test)]
mod waiting_tests {
    use super::*;
    use crate::win32::stubs;

    fn id(name: &str) -> u16 {
        let dll = stubs::DLLS
            .iter()
            .position(|dll| std::ptr::eq(*dll, &DLL))
            .unwrap();
        let api = DLL.apis.iter().position(|api| api.name == name).unwrap();
        stubs::id(dll, api)
    }

    #[test]
    fn the_draws_that_wait_do_through_vertices_and_states_only() {
        for name in [
            "glBegin",
            "glVertex3f",
            "glColor4ub",
            "glTexCoord2f",
            "glMultiTexCoord2fARB",
            "glEnd",
            "glDrawElements",
            "glBindTexture",
            "glEnable",
            "glLoadMatrixf",
            "glTexParameteri",
        ] {
            assert!(keeps_waiting(id(name)), "{name}");
        }
        // What reads the buffers or a texture's image, changes them or
        // frees them, and a function OpenGL here doesn't have.
        for name in [
            "glClear",
            "glReadPixels",
            "glFlush",
            "glFinish",
            "glTexImage2D",
            "glTexSubImage2D",
            "glCopyTexImage2D",
            "glDeleteTextures",
            "wglSwapBuffers",
            "wglMakeCurrent",
            "wglDeleteContext",
            "glCallList",
        ] {
            assert!(!keeps_waiting(id(name)), "{name}");
        }
        // No function of another DLL: KERNEL32's first.
        assert!(!keeps_waiting(stubs::id(0, 0)));
    }

    #[test]
    fn a_saved_state_has_the_draws_that_wait() {
        use crate::savestate::{Reader, State as _, Writer};
        let mut state = State::default();
        state.contexts.push(Some(new_context(1, 2, 3)));
        let before = draw::waiting_for_tests();
        state.waiting = Some(before.clone());
        let mut w = Writer::default();
        state.save(&mut w);
        let mut loaded = State::default();
        loaded.load(&mut Reader::new(&w.buf)).unwrap();
        let after = loaded.waiting.expect("the draws that waited");
        assert!(draw::same_waiting(&before, &after));
    }
}
