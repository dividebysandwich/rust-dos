//! GLIDE3X.DLL: 3dfx's Glide 3, which Windows programs of 1998 and
//! after drew with on a Voodoo, and which 3dfx's OpenGL drivers for the
//! Voodoo2, Banshee and Voodoo3 sit on.
//!
//! Glide 3 is Glide 2 with a new way to pass vertices and new numbers
//! for a few arguments. Most of it is therefore Glide 2's code (`api`),
//! and many of its functions are GLIDE2X.DLL's own (`exports`). What it
//! changes is here:
//!
//! * Vertices are the program's own structures, which grVertexLayout
//!   describes by where each parameter is in them. Each vertex is read
//!   into Glide 2's `Vertex` and drawn as Glide 2 draws one. The layout
//!   also gives the hint Glide 2's programs gave with grHints: which
//!   texture coordinates differ from one unit to the other.
//! * grCoordinateSpace can put vertices in clip coordinates, which
//!   Glide 3 projects through grViewport and grDepthRange.
//! * grDrawVertexArray and grDrawVertexArrayContiguous draw points,
//!   lines, polygons, strips, fans and lists of triangles, and a strip
//!   or a fan can go on from one call to the next.
//! * A texture's sizes and aspect ratio are log2 (`lod_glide2`), the
//!   ranges of grTexMultibaseAddress count the other way, and the
//!   texture tables go to every texture unit.
//! * grGet and grGetString answer what grSstQueryHardware did, and
//!   grQueryResolutions lists the displays grSstWinOpen can open.
//!
//! (Issue #457.)

use super::api::{self, TexInfo, TmuVertex, Vertex};
use super::exports as x;
use super::file3df;
use super::{hw, state};
use crate::cpu::Cpu;
use crate::win32::stubs::{Api, Conv, Handler, Var};

const FALSE: u32 = 0;
const TRUE: u32 = 1;

const fn stdcall(name: &'static str, ordinal: u16, args: u8, handler: Handler) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

fn f(v: u32) -> f32 {
    f32::from_bits(v)
}

// --- Sizes of textures ---

/// Glide 2's GR_LOD_* of a texture's longer side (0 for 256 texels, 8
/// for 1) from Glide 3's GR_LOD_LOG2_* (8 for 256, 0 for 1), or None
/// for a size Glide doesn't have.
pub(super) fn lod_glide2(log2: u32) -> Option<u32> {
    (log2 <= 8).then(|| 8 - log2)
}

/// Glide 3's GR_LOD_LOG2_* from Glide 2's GR_LOD_*.
pub(super) fn lod_log2(lod: u32) -> u32 {
    8 - lod.min(8)
}

/// Glide 2's GR_ASPECT_* (0 for 8x1, 3 for 1x1, 6 for 1x8) from Glide
/// 3's GR_ASPECT_LOG2_*, log2 of the width over the height (3 for 8x1,
/// -3 for 1x8), or None for a ratio Glide doesn't have.
pub(super) fn aspect_glide2(log2: u32) -> Option<u32> {
    let log2 = log2 as i32;
    (-3..=3).contains(&log2).then(|| (3 - log2) as u32)
}

/// Glide 3's GR_ASPECT_LOG2_* from Glide 2's GR_ASPECT_*.
pub(super) fn aspect_log2(aspect: u32) -> u32 {
    (3 - aspect.min(6) as i32) as u32
}

/// A texture (Glide 3's GrTexInfo) at `at` in Glide 2's terms, and
/// where its texels are. Sizes Glide doesn't have become ones `api`
/// turns down.
fn tex_info(cpu: &mut Cpu, at: u32) -> Option<(TexInfo, u32)> {
    let mut w = [0u32; 5];
    for (i, v) in w.iter_mut().enumerate() {
        *v = cpu.bus.guest_read_32(at.wrapping_add(4 * i as u32));
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    let info = TexInfo {
        small_lod: lod_glide2(w[0]).unwrap_or(u32::MAX),
        large_lod: lod_glide2(w[1]).unwrap_or(u32::MAX),
        aspect: aspect_glide2(w[2]).unwrap_or(u32::MAX),
        format: w[3],
    };
    Some((info, w[4]))
}

// --- State ---

// The parameters of a vertex (GR_PARAM_*), by their place in
// `State::layout`: S and T of texture units 0 to 2 from ST on, and
// their 1/W from QT on.
const XY: usize = 0;
const Z: usize = 1;
const W: usize = 2;
const Q: usize = 3;
const FOG: usize = 4;
const A: usize = 5;
const RGB: usize = 6;
const PARGB: usize = 7;
const ST: usize = 8;
const QT: usize = 11;
const PARAMS: usize = 14;

/// A parameter grVertexLayout turned off.
const OFF: u32 = u32::MAX;

/// The place in `State::layout` of a GR_PARAM_*.
fn param_index(param: u32) -> Option<usize> {
    Some(match param {
        0x01 => XY,
        0x02 => Z,
        0x03 => W,
        0x04 => Q,
        0x05 => FOG,
        0x10 => A,
        0x20 => RGB,
        0x30 => PARGB,
        0x40..=0x42 => ST + (param - 0x40) as usize,
        0x50..=0x52 => QT + (param - 0x50) as usize,
        _ => return None,
    })
}

/// Glide 3's own state, beside Glide 2's (`api::Context`).
#[derive(Clone, Debug)]
pub struct State {
    /// Where each parameter is in a vertex, or OFF.
    pub layout: [u32; PARAMS],
    /// Vertices are in clip coordinates (grCoordinateSpace).
    pub clip: bool,
    /// grViewport's X, Y, width and height.
    pub viewport: [i32; 4],
    /// grDepthRange's near and far.
    pub depth_range: [f32; 2],
    /// Each texture unit's scale of S and T in clip coordinates: its
    /// texture's size in Glide 2's texel coordinates, 256 on the
    /// longer side.
    pub st_scale: [[f32; 2]; 2],
    /// The strip's last two vertices or the fan's first and last, how
    /// many of them there are, and whether a strip's next triangle
    /// turns the other way: for a strip or fan that goes on.
    pub strip: [Vertex; 2],
    pub strip_len: u32,
    pub strip_odd: bool,
    /// Points and lines drawn (grGet's GR_STATS_POINTS and
    /// GR_STATS_LINES).
    pub points: u32,
    pub lines: u32,
}

crate::state_fields!(State {
    layout,
    clip,
    viewport,
    depth_range,
    st_scale,
    strip,
    strip_len,
    strip_odd,
    points,
    lines
});

impl Default for State {
    fn default() -> Self {
        State {
            layout: [OFF; PARAMS],
            clip: false,
            viewport: [0; 4],
            depth_range: [0.0, 1.0],
            st_scale: [[256.0; 2]; 2],
            strip: [Vertex::default(); 2],
            strip_len: 0,
            strip_odd: false,
            points: 0,
            lines: 0,
        }
    }
}

fn g3(cpu: &mut Cpu) -> &mut State {
    &mut state(cpu).g3
}

/// The hint grHints gave in Glide 2 (GR_HINT_STWHINT), from the layout:
/// a unit with a 1/W of its own, and the second unit's S and T.
fn set_hint(cpu: &mut Cpu) {
    let layout = g3(cpu).layout;
    let mut hint = 0;
    if layout[QT] != OFF {
        hint |= api::STWHINT_W_DIFF_TMU0;
    }
    if layout[ST + 1] != OFF {
        hint |= api::STWHINT_ST_DIFF_TMU1;
    }
    if layout[QT + 1] != OFF {
        hint |= api::STWHINT_W_DIFF_TMU1;
    }
    api::hints(cpu, api::HINT_STWHINT, hint);
}

// --- Vertices ---

/// The vertex at `at`, read by the layout and projected from clip
/// coordinates if they are in use, or None if a page isn't there.
fn vertex(cpu: &mut Cpu, at: u32) -> Option<Vertex> {
    let s = g3(cpu);
    let (layout, clip, viewport, depth_range, st_scale) =
        (s.layout, s.clip, s.viewport, s.depth_range, s.st_scale);
    let mut word = |p: usize, k: u32| -> Option<u32> {
        let offset = layout[p];
        (offset != OFF).then(|| {
            cpu.bus
                .guest_read_32(at.wrapping_add(offset).wrapping_add(4 * k))
        })
    };
    let (x, y) = (word(XY, 0).map(f), word(XY, 1).map(f));
    let (z, w, q) = (word(Z, 0).map(f), word(W, 0).map(f), word(Q, 0).map(f));
    let packed = word(PARGB, 0);
    let rgb = [word(RGB, 0), word(RGB, 1), word(RGB, 2)].map(|c| c.map_or(0.0, f));
    let alpha = word(A, 0).map_or(0.0, f);
    let st = [0, 1].map(|t| [word(ST + t, 0), word(ST + t, 1)].map(|c| c.map_or(0.0, f)));
    let qt = [word(QT, 0).map(f), word(QT + 1, 0).map(f)];
    if cpu.bus.guest_faulted() {
        return None;
    }
    let mut v = Vertex::default();
    // Packed colours win over float ones: grVertexLayout turns one off
    // as it turns the other on.
    let [mut r, mut g, mut b] = rgb;
    let mut a = alpha;
    if let Some(argb) = packed {
        let byte = |shift: u32| ((argb >> shift) & 0xFF) as f32;
        [a, r, g, b] = [byte(24), byte(16), byte(8), byte(0)];
    } else if clip {
        // Clip coordinates have float colours from 0 to 1.
        [r, g, b, a] = [r, g, b, a].map(|c| c * 255.0);
    }
    (v.r, v.g, v.b, v.a) = (r, g, b, a);
    let (x, y) = (x.unwrap_or(0.0), y.unwrap_or(0.0));
    if !clip {
        // Window coordinates: X and Y on the screen, Z the depth from 0
        // to 65535, and Q 1/W.
        (v.x, v.y) = (x, y);
        v.ooz = z.unwrap_or(0.0);
        v.oow = q.unwrap_or(1.0);
        for t in 0..2 {
            v.tmu[t] = TmuVertex {
                sow: st[t][0],
                tow: st[t][1],
                oow: qt[t].unwrap_or(v.oow),
            };
        }
    } else {
        // Clip coordinates: X, Y and Z over W into the viewport and the
        // depth range, and S and T from 0 to 1 over the texture.
        let oow = match (w, q) {
            (Some(w), _) => 1.0 / w,
            (None, Some(q)) => q,
            (None, None) => 1.0,
        };
        let [vx, vy, vw, vh] = viewport.map(|n| n as f32);
        let (hw, hh) = (vw * 0.5, vh * 0.5);
        let [near, far] = depth_range;
        let hd = (far - near) * 0.5 * 65535.0;
        let od = (far + near) * 0.5 * 65535.0;
        v.x = x * oow * hw + vx + hw;
        v.y = y * oow * hh + vy + hh;
        v.ooz = z.unwrap_or(0.0) * oow * hd + od;
        v.oow = oow;
        for t in 0..2 {
            v.tmu[t] = TmuVertex {
                sow: st[t][0] * oow * st_scale[t][0],
                tow: st[t][1] * oow * st_scale[t][1],
                oow: qt[t].map_or(oow, |q| q * oow),
            };
        }
    }
    v.z = v.ooz;
    Some(v)
}

/// At most this many vertices an array draws; a bad count draws
/// nothing.
const MAX_VERTICES: u32 = 1 << 16;

/// The vertices of grDrawVertexArray: `n` pointers at `at`.
fn vertex_pointers(cpu: &mut Cpu, n: u32, at: u32) -> Option<Vec<Vertex>> {
    if n > MAX_VERTICES {
        return Some(Vec::new());
    }
    (0..n)
        .map(|i| {
            let p = cpu.bus.guest_read_32(at.wrapping_add(4 * i));
            if cpu.bus.guest_faulted() {
                return None;
            }
            vertex(cpu, p)
        })
        .collect()
}

/// The vertices of grDrawVertexArrayContiguous: `n` of them `stride`
/// bytes apart from `at`.
fn vertex_run(cpu: &mut Cpu, n: u32, at: u32, stride: u32) -> Option<Vec<Vertex>> {
    if n > MAX_VERTICES {
        return Some(Vec::new());
    }
    (0..n)
        .map(|i| vertex(cpu, at.wrapping_add(i.wrapping_mul(stride))))
        .collect()
}

// grDrawVertexArray's modes.
const POINTS: u32 = 0;
const LINE_STRIP: u32 = 1;
const LINES: u32 = 2;
const POLYGON: u32 = 3;
const TRIANGLE_STRIP: u32 = 4;
const TRIANGLE_FAN: u32 = 5;
const TRIANGLES: u32 = 6;
const TRIANGLE_STRIP_CONTINUE: u32 = 7;
const TRIANGLE_FAN_CONTINUE: u32 = 8;

fn draw_point(cpu: &mut Cpu, p: &Vertex) {
    api::draw_point(cpu, p);
    g3(cpu).points = g3(cpu).points.wrapping_add(1);
}

fn draw_line(cpu: &mut Cpu, p: &Vertex, q: &Vertex) {
    api::draw_line(cpu, p, q);
    g3(cpu).lines = g3(cpu).lines.wrapping_add(1);
}

/// Draw `list` as `mode` says.
fn draw_array(cpu: &mut Cpu, mode: u32, list: &[Vertex]) {
    match mode {
        POINTS => list.iter().for_each(|p| draw_point(cpu, p)),
        LINE_STRIP => list.windows(2).for_each(|l| draw_line(cpu, &l[0], &l[1])),
        LINES => list
            .as_chunks::<2>()
            .0
            .iter()
            .for_each(|[p, q]| draw_line(cpu, p, q)),
        POLYGON => api::draw_polygon(cpu, list),
        TRIANGLES => list.as_chunks::<3>().0.iter().for_each(|[p, q, r]| {
            api::draw_triangle(cpu, p, q, r);
        }),
        TRIANGLE_STRIP | TRIANGLE_STRIP_CONTINUE => {
            strip(cpu, list, mode == TRIANGLE_STRIP_CONTINUE)
        }
        TRIANGLE_FAN | TRIANGLE_FAN_CONTINUE => fan(cpu, list, mode == TRIANGLE_FAN_CONTINUE),
        _ => {}
    }
}

/// A strip: each vertex after the first two makes a triangle with the
/// two before it, every second one turned round so that all face the
/// same way. One that goes on starts from where the last one ended.
fn strip(cpu: &mut Cpu, list: &[Vertex], go_on: bool) {
    let s = g3(cpu);
    let (mut last, mut n, mut odd) = if go_on {
        (s.strip, s.strip_len, s.strip_odd)
    } else {
        ([Vertex::default(); 2], 0, false)
    };
    for v in list {
        if n == 2 {
            let [a, b] = last;
            if odd {
                api::draw_triangle(cpu, &b, &a, v);
            } else {
                api::draw_triangle(cpu, &a, &b, v);
            }
            odd = !odd;
        }
        last = [last[1], *v];
        n = (n + 1).min(2);
    }
    let s = g3(cpu);
    (s.strip, s.strip_len, s.strip_odd) = (last, n, odd);
}

/// A fan: each vertex after the first two makes a triangle with the
/// first and the one before it.
fn fan(cpu: &mut Cpu, list: &[Vertex], go_on: bool) {
    let s = g3(cpu);
    let (mut ends, mut n) = if go_on {
        (s.strip, s.strip_len)
    } else {
        ([Vertex::default(); 2], 0)
    };
    for v in list {
        match n {
            0 => ends[0] = *v,
            1 => ends[1] = *v,
            _ => {
                api::draw_triangle(cpu, &ends[0], &ends[1], v);
                ends[1] = *v;
            }
        }
        n = (n + 1).min(2);
    }
    let s = g3(cpu);
    (s.strip, s.strip_len, s.strip_odd) = (ends, n, false);
}

// --- Functions this library has only as names ---

macro_rules! missing {
    ($($handler:ident $name:literal),* $(,)?) => {
        $(fn $handler(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
            x::missing_in(cpu, "GLIDE3X", $name)
        })*
    };
}

missing!(
    missing_gr_splash "grSplash",
    missing_gr_sst_config_pipeline "grSstConfigPipeline",
    missing_gr_sst_vid_mode "grSstVidMode",
    missing_gu_encode_rle16 "guEncodeRLE16",
    missing_pci_fetch_register "_pciFetchRegister",
    missing_pci_update_register "_pciUpdateRegister",
    missing_pci_close "pciClose",
    missing_pci_device_exists "pciDeviceExists",
    missing_pci_find_card "pciFindCard",
    missing_pci_find_card_multi "pciFindCardMulti",
    missing_pci_find_card_multi_func "pciFindCardMultiFunc",
    missing_pci_find_free_mtrr "pciFindFreeMTRR",
    missing_pci_find_mtrr_match "pciFindMTRRMatch",
    missing_pci_get_config_data "pciGetConfigData",
    missing_pci_get_error_code "pciGetErrorCode",
    missing_pci_get_error_string "pciGetErrorString",
    missing_pci_linear_range_set_permission "pciLinearRangeSetPermission",
    missing_pci_map_card "pciMapCard",
    missing_pci_map_card_multi "pciMapCardMulti",
    missing_pci_map_physical_device_to_linear "pciMapPhysicalDeviceToLinear",
    missing_pci_map_physical_to_linear "pciMapPhysicalToLinear",
    missing_pci_open "pciOpen",
    missing_pci_open_ex "pciOpenEx",
    missing_pci_output_debug_string "pciOutputDebugString",
    missing_pci_set_config_data "pciSetConfigData",
    missing_pci_set_mtrr "pciSetMTRR",
    missing_pci_set_mtrr_amd_k6 "pciSetMTRRAmdK6",
    missing_pci_set_pass_through_base "pciSetPassThroughBase",
    missing_pci_unmap_physical "pciUnmapPhysical",
    missing_pio_in_byte "pioInByte",
    missing_pio_in_long "pioInLong",
    missing_pio_in_word "pioInWord",
    missing_pio_out_byte "pioOutByte",
    missing_pio_out_long "pioOutLong",
    missing_pio_out_word "pioOutWord",
);

/// Functions with nothing to do: grFinish and grFlush, as the emulated
/// card draws at once, and grSetNumPendingBuffers.
fn nothing(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

// --- Opening the card ---

/// What grSstWinOpen returns: an address in the library's data, where
/// a program that looks finds zeros.
fn context() -> u32 {
    super::super::stubs::var_address(dll_index(), V_CONTEXT)
}

/// GLIDE3X.DLL's place in `stubs::DLLS`.
fn dll_index() -> usize {
    super::super::stubs::DLLS
        .iter()
        .position(|d| std::ptr::eq(*d, &super::DLL3))
        .expect("GLIDE3X is a built-in DLL")
}

// The library's variables: the context, and grGetString's strings.
const V_CONTEXT: usize = 0;
const V_STRINGS: usize = 1;
/// The bytes each of grGetString's strings has.
const STRING_SIZE: u32 = 64;

pub(super) static VARS: &[Var] = &[
    Var {
        name: "context",
        ordinal: 0,
        size: 64,
    },
    Var {
        name: "strings",
        ordinal: 0,
        size: 5 * STRING_SIZE,
    },
];

/// The size of each of Glide 3's resolutions: Glide 2's sixteen, then
/// the larger ones Glide 3 added, GR_RESOLUTION_1152x864 (0x10) to
/// GR_RESOLUTION_2048x2048 (0x17).
fn resolution_size(resolution: u32) -> Option<(u32, u32)> {
    const MORE: [(u32, u32); 8] = [
        (1152, 864),
        (1280, 960),
        (1600, 1024),
        (1792, 1344),
        (1856, 1392),
        (1920, 1440),
        (2048, 1536),
        (2048, 2048),
    ];
    api::resolution_size(resolution)
        .or_else(|| MORE.get(resolution.checked_sub(0x10)? as usize).copied())
}

/// GR_RESOLUTION_* past the last, for grQueryResolutions.
const RESOLUTIONS: u32 = 0x18;

/// grSstWinOpen: Glide 2's, with Glide 3's resolutions, which returns a
/// context. A new context has no vertex parameters, window coordinates,
/// and a viewport and depth range of the whole screen.
fn gr_sst_win_open(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let Some((w, h)) = resolution_size(a[1]) else {
        return Some(0);
    };
    if !api::open(cpu, w, h, api::refresh_hz(a[2]), a[3], a[4], a[5], a[6]) {
        return Some(0);
    }
    let (w, h) = api::screen_size(cpu);
    *g3(cpu) = State {
        viewport: [0, 0, w as i32, h as i32],
        ..State::default()
    };
    set_hint(cpu);
    Some(context())
}

fn gr_sst_win_close(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    x::gr_sst_win_close(cpu, a)?;
    Some(TRUE)
}

/// grSelectContext: the one context there is.
fn gr_select_context(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let open = state(cpu).owner == crate::win32::user32::current_pid(cpu);
    Some((open && a[0] == context()) as u32)
}

/// GrResolution's GR_QUERY_ANY.
const QUERY_ANY: u32 = u32::MAX;

/// grQueryResolutions: the resolutions, refresh rates and numbers of
/// buffers grSstWinOpen can open that match the template's, written to
/// `output` unless it is null, and their bytes. Every refresh rate is
/// listed as 60 Hz unless the template names one: the emulated monitor
/// shows them all.
fn gr_query_resolutions(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let mut t = [0u32; 4];
    for (i, v) in t.iter_mut().enumerate() {
        *v = cpu.bus.guest_read_32(a[0].wrapping_add(4 * i as u32));
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    let refresh = if t[1] == QUERY_ANY { 0 } else { t[1] };
    if refresh > 8 {
        return Some(0);
    }
    let mut found = Vec::new();
    for res in 0..RESOLUTIONS {
        let Some((w, h)) = resolution_size(res) else {
            continue;
        };
        for colors in 2..=3 {
            for auxes in 0..=1 {
                let wanted = |field: u32, v: u32| field == QUERY_ANY || field == v;
                if wanted(t[0], res)
                    && wanted(t[2], colors)
                    && wanted(t[3], auxes)
                    && hw::fits(cpu, w, h, colors, auxes)
                {
                    found.push([res, refresh, colors, auxes]);
                }
            }
        }
    }
    if a[1] != 0 {
        let bytes: Vec<u8> = found
            .iter()
            .flatten()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        if !cpu.bus.guest_probe(a[1], bytes.len(), true) {
            return None;
        }
        cpu.bus.guest_write_bytes(a[1], &bytes);
    }
    Some(16 * found.len() as u32)
}

// --- Asking about the board ---

// grGet's questions.
const BITS_DEPTH: u32 = 0x01;
const BITS_RGBA: u32 = 0x02;
const FIFO_FULLNESS: u32 = 0x03;
const FOG_TABLE_ENTRIES: u32 = 0x04;
const GAMMA_TABLE_ENTRIES: u32 = 0x05;
const GLIDE_STATE_SIZE: u32 = 0x06;
const GLIDE_VERTEXLAYOUT_SIZE: u32 = 0x07;
const IS_BUSY: u32 = 0x08;
const LFB_PIXEL_PIPE: u32 = 0x09;
const MAX_TEXTURE_SIZE: u32 = 0x0A;
const MAX_TEXTURE_ASPECT_RATIO: u32 = 0x0B;
const MEMORY_FB: u32 = 0x0C;
const MEMORY_TMU: u32 = 0x0D;
const MEMORY_UMA: u32 = 0x0E;
const NUM_BOARDS: u32 = 0x0F;
const NON_POWER_OF_TWO_TEXTURES: u32 = 0x10;
const NUM_FB: u32 = 0x11;
const NUM_SWAP_HISTORY_BUFFER: u32 = 0x12;
const NUM_TMU: u32 = 0x13;
const PENDING_BUFFERSWAPS: u32 = 0x14;
const REVISION_FB: u32 = 0x15;
const REVISION_TMU: u32 = 0x16;
const STATS_LINES: u32 = 0x17;
const STATS_PIXELS_AFUNC_FAIL: u32 = 0x18;
const STATS_PIXELS_CHROMA_FAIL: u32 = 0x19;
const STATS_PIXELS_DEPTHFUNC_FAIL: u32 = 0x1A;
const STATS_PIXELS_IN: u32 = 0x1B;
const STATS_PIXELS_OUT: u32 = 0x1C;
const STATS_PIXELS: u32 = 0x1D;
const STATS_POINTS: u32 = 0x1E;
const STATS_TRIANGLES_IN: u32 = 0x1F;
const STATS_TRIANGLES_OUT: u32 = 0x20;
const STATS_TRIANGLES: u32 = 0x21;
const SUPPORTS_PASSTHRU: u32 = 0x23;
const TEXTURE_ALIGN: u32 = 0x24;
const VIDEO_POSITION: u32 = 0x25;
const VIEWPORT: u32 = 0x26;
const WDEPTH_MIN_MAX: u32 = 0x27;
const ZDEPTH_MIN_MAX: u32 = 0x28;
const VERTEX_PARAMETER: u32 = 0x29;
const BITS_GAMMA: u32 = 0x2A;

/// One of the card's pixel counters, as grSstPerfStats reads them.
fn pixel_counter(cpu: &mut Cpu, reg: usize) -> u32 {
    hw::read(cpu, reg) & 0xFF_FFFF
}

/// The answer to grGet's question `what`, as 32-bit words, or None for
/// a question this library doesn't answer. `asked` is the first word
/// the program passed, which GR_VERTEX_PARAMETER reads.
fn get(cpu: &mut Cpu, what: u32, asked: u32) -> Option<Vec<u32>> {
    use crate::voodoo::regs::*;
    if what == NUM_BOARDS {
        return Some(vec![api::query(cpu).is_some() as u32]);
    }
    let board = api::query(cpu)?;
    let banshee = board.card == hw::Card::Banshee;
    let g = &state(cpu).gc;
    let [drawn, processed] = g.triangles;
    let words = match what {
        BITS_DEPTH => vec![16],
        BITS_RGBA => vec![5, 6, 5, 0],
        FIFO_FULLNESS => vec![0, 0],
        FOG_TABLE_ENTRIES => vec![api::FOG_TABLE_SIZE as u32],
        // A Voodoo Graphics' or Voodoo2's 33rd entry is the 32nd again.
        GAMMA_TABLE_ENTRIES => vec![if banshee { 256 } else { 32 }],
        GLIDE_STATE_SIZE => vec![STATE_SIZE as u32],
        GLIDE_VERTEXLAYOUT_SIZE => vec![LAYOUT_SIZE as u32],
        IS_BUSY => vec![api::sst_is_busy(cpu) as u32],
        LFB_PIXEL_PIPE => vec![TRUE],
        MAX_TEXTURE_SIZE => vec![256],
        MAX_TEXTURE_ASPECT_RATIO => vec![3],
        MEMORY_FB => vec![board.fb_bytes],
        MEMORY_TMU => vec![hw::texture_memory(cpu, 0).1],
        // A Banshee's textures are in its frame buffer.
        MEMORY_UMA => vec![if banshee {
            hw::texture_memory(cpu, 0).1
        } else {
            0
        }],
        NON_POWER_OF_TWO_TEXTURES => vec![FALSE],
        // The frame buffer chips (Pixelfx), two only for a pair of
        // Voodoo2s in SLI, which the emulator doesn't have; not the
        // colour buffers.
        NUM_FB => vec![1],
        NUM_SWAP_HISTORY_BUFFER => vec![0],
        NUM_TMU => vec![board.tmus],
        PENDING_BUFFERSWAPS => vec![api::buffer_num_pending(cpu)],
        REVISION_FB | REVISION_TMU => vec![match (board.card, what) {
            (hw::Card::Voodoo1, REVISION_FB) => 2,
            (hw::Card::Voodoo1, _) => 1,
            (hw::Card::Voodoo2, REVISION_FB) => 4,
            (hw::Card::Voodoo2, _) => 2,
            _ => 1,
        }],
        STATS_LINES => vec![g3(cpu).lines],
        STATS_POINTS => vec![g3(cpu).points],
        STATS_TRIANGLES_IN | STATS_TRIANGLES => vec![processed],
        STATS_TRIANGLES_OUT => vec![drawn],
        STATS_PIXELS_AFUNC_FAIL => vec![pixel_counter(cpu, FBI_AFUNC_FAIL)],
        STATS_PIXELS_CHROMA_FAIL => vec![pixel_counter(cpu, FBI_CHROMA_FAIL)],
        STATS_PIXELS_DEPTHFUNC_FAIL => vec![pixel_counter(cpu, FBI_ZFUNC_FAIL)],
        STATS_PIXELS_IN => vec![pixel_counter(cpu, FBI_PIXELS_IN)],
        STATS_PIXELS_OUT => vec![pixel_counter(cpu, FBI_PIXELS_OUT)],
        SUPPORTS_PASSTHRU => vec![!banshee as u32],
        TEXTURE_ALIGN => vec![if banshee { 16 } else { 8 }],
        VIDEO_POSITION => vec![0, api::sst_video_line(cpu)],
        // Integers, as 3dfx's grGet casts them (FxI32).
        VIEWPORT => g3(cpu).viewport.map(|n| n as u32).to_vec(),
        WDEPTH_MIN_MAX => vec![0, 0xFFFF],
        ZDEPTH_MIN_MAX => vec![0xFFFF, 0],
        VERTEX_PARAMETER => {
            let offset = g3(cpu).layout[param_index(asked)?];
            if offset == OFF {
                return None;
            }
            vec![offset]
        }
        BITS_GAMMA => vec![8],
        _ => return None,
    };
    Some(words)
}

/// grGet(what, length, out): the answer's bytes, or 0 if `length`
/// isn't its size or the question is one this library doesn't answer.
fn gr_get(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let [what, length, out] = a[..3].try_into().ok()?;
    if out == 0 {
        return Some(0);
    }
    let asked = cpu.bus.guest_read_32(out);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let Some(words) = get(cpu, what, asked) else {
        return Some(0);
    };
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    if length as usize != bytes.len() {
        return Some(0);
    }
    if !cpu.bus.guest_probe(out, bytes.len(), true) {
        return None;
    }
    cpu.bus.guest_write_bytes(out, &bytes);
    Some(length)
}

/// grGetString: the library's strings, each in a place of its own in
/// the library's data, so that one stays as the next is asked for. No
/// extensions: grGetProcAddress has none to give.
fn gr_get_string(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    const EXTENSION: u32 = 0xA0;
    const HARDWARE: u32 = 0xA1;
    const RENDERER: u32 = 0xA2;
    const VENDOR: u32 = 0xA3;
    const VERSION: u32 = 0xA4;
    let board = api::query(cpu);
    let text = match a[0] {
        EXTENSION => "",
        HARDWARE => match board {
            Some(b) if b.voodoo3 => "Voodoo3",
            Some(b) if b.card == hw::Card::Banshee => "Voodoo Banshee",
            Some(b) if b.card == hw::Card::Voodoo2 => "Voodoo2",
            _ => "Voodoo Graphics",
        },
        RENDERER => "Glide",
        VENDOR => "3Dfx Interactive",
        // 3dfx's last Glide 3 for each card.
        VERSION => match board {
            Some(b) if b.voodoo3 => "3.10.00.0658",
            Some(b) if b.card == hw::Card::Banshee => "3.10.00.0350",
            _ => "3.03.00.0667",
        },
        _ => return Some(0),
    };
    let at =
        super::super::stubs::var_address(dll_index(), V_STRINGS) + STRING_SIZE * (a[0] - EXTENSION);
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    cpu.bus.guest_write_bytes(at, &bytes);
    Some(at)
}

/// grGetProcAddress: no extensions.
fn gr_get_proc_address(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

// --- State ---

/// grGlideGetState's bytes: Glide 2's, then Glide 3's own, then the
/// layout.
const STATE_SIZE: usize = api::STATE_SIZE + 4 * 11 + LAYOUT_SIZE;
/// grGlideGetVertexLayout's bytes.
const LAYOUT_SIZE: usize = 4 * PARAMS;

fn layout_bytes(s: &State) -> Vec<u8> {
    s.layout.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn set_layout_bytes(cpu: &mut Cpu, bytes: &[u8]) {
    for (i, w) in bytes.as_chunks::<4>().0.iter().take(PARAMS).enumerate() {
        g3(cpu).layout[i] = u32::from_le_bytes(*w);
    }
    set_hint(cpu);
}

fn gr_glide_get_state(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    if !cpu.bus.guest_probe(a[0], STATE_SIZE, true) {
        return None;
    }
    let mut out = api::get_state(cpu).to_vec();
    let s = g3(cpu);
    let mut words = vec![s.clip as u32];
    words.extend(s.viewport.map(|n| n as u32));
    words.extend(s.depth_range.map(f32::to_bits));
    words.extend(s.st_scale.as_flattened().iter().map(|v| v.to_bits()));
    out.extend(words.iter().flat_map(|w| w.to_le_bytes()));
    out.extend(layout_bytes(s));
    cpu.bus.guest_write_bytes(a[0], &out);
    Some(0)
}

fn gr_glide_set_state(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let data = x::bytes(cpu, a[0], STATE_SIZE)?;
    let (glide2, rest) = data.split_at(api::STATE_SIZE);
    api::set_state(cpu, glide2.try_into().ok()?);
    let words: Vec<u32> = rest[..4 * 11]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|w| u32::from_le_bytes(*w))
        .collect();
    let s = g3(cpu);
    s.clip = words[0] != 0;
    s.viewport = std::array::from_fn(|i| words[1 + i] as i32);
    s.depth_range = [f(words[5]), f(words[6])];
    s.st_scale = [[f(words[7]), f(words[8])], [f(words[9]), f(words[10])]];
    set_layout_bytes(cpu, &rest[4 * 11..]);
    Some(0)
}

fn gr_glide_get_vertex_layout(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    if !cpu.bus.guest_probe(a[0], LAYOUT_SIZE, true) {
        return None;
    }
    let bytes = layout_bytes(g3(cpu));
    cpu.bus.guest_write_bytes(a[0], &bytes);
    Some(0)
}

fn gr_glide_set_vertex_layout(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let bytes = x::bytes(cpu, a[0], LAYOUT_SIZE)?;
    set_layout_bytes(cpu, &bytes);
    Some(0)
}

/// grVertexLayout(param, offset, mode): where a parameter is in a
/// vertex, or that it isn't (GR_PARAM_DISABLE). Float and packed
/// colours turn each other off.
fn gr_vertex_layout(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let Some(p) = param_index(a[0]) else {
        return Some(0);
    };
    let on = a[2] != 0;
    let s = g3(cpu);
    s.layout[p] = if on { a[1] } else { OFF };
    if on && p == RGB {
        s.layout[PARGB] = OFF;
    }
    if on && p == PARGB {
        s.layout[RGB] = OFF;
    }
    set_hint(cpu);
    Some(0)
}

/// grReset(what): a statistic back to 0, or no vertex parameters.
fn gr_reset(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    match a[0] {
        STATS_POINTS => g3(cpu).points = 0,
        STATS_LINES => g3(cpu).lines = 0,
        STATS_TRIANGLES => state(cpu).gc.triangles = [0; 2],
        STATS_PIXELS => {
            x::gr_sst_reset_perf_stats(cpu, a)?;
        }
        VERTEX_PARAMETER => {
            g3(cpu).layout = [OFF; PARAMS];
            set_hint(cpu);
        }
        _ => return Some(FALSE),
    }
    Some(TRUE)
}

// grEnable's and grDisable's modes.
const ALLOW_MIPMAP_DITHER: u32 = 2;
const PASSTHRU: u32 = 3;

/// grEnable and grDisable: mipmaps dithered, as Glide 2's
/// GR_HINT_ALLOW_MIPMAP_DITHER; the monitor given to the VGA through
/// the pass-through, as Glide 2's grSstControl. The other modes have no
/// effect on the emulated card.
fn enable(cpu: &mut Cpu, mode: u32, on: bool) {
    match mode {
        ALLOW_MIPMAP_DITHER => api::hints(cpu, api::HINT_ALLOW_MIPMAP_DITHER, on as u32),
        PASSTHRU if state(cpu).owner == crate::win32::user32::current_pid(cpu) => {
            hw::set_active(cpu, !on);
        }
        _ => {}
    }
}

fn gr_enable(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    enable(cpu, a[0], true);
    Some(0)
}

fn gr_disable(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    enable(cpu, a[0], false);
    Some(0)
}

/// grCoordinateSpace: window (0) or clip (1) coordinates.
fn gr_coordinate_space(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    if a[0] <= 1 {
        g3(cpu).clip = a[0] == 1;
    }
    Some(0)
}

fn gr_viewport(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    g3(cpu).viewport = [a[0], a[1], a[2], a[3]].map(|n| n as i32);
    Some(0)
}

fn gr_depth_range(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    g3(cpu).depth_range = [f(a[0]), f(a[1])];
    Some(0)
}

/// grFogMode: Glide 3's sources of fog in Glide 2's numbers. Fog on the
/// fog coordinate (an extension this library doesn't list) and on Q
/// both read the table by 1/W.
fn gr_fog_mode(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let source = match a[0] & 0xFF {
        1 | 2 => api::FOG_WITH_TABLE,
        3 => api::FOG_WITH_ITERATED_Z,
        4 => api::FOG_WITH_ITERATED_ALPHA,
        _ => api::FOG_DISABLE,
    };
    api::fog_mode(cpu, a[0] & (api::FOG_MULT2 | api::FOG_ADD2) | source);
    Some(0)
}

/// grLoadGammaTable(n, red, green, blue): `n` entries of each from 0
/// to 255.
fn gr_load_gamma_table(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let n = a[0].min(256);
    let mut table = Vec::with_capacity(n as usize);
    for i in 0..n {
        let entry =
            [a[1], a[2], a[3]].map(|at| cpu.bus.guest_read_32(at.wrapping_add(4 * i)) as u8);
        table.push(entry);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    hw::load_gamma(cpu, &table);
    Some(0)
}

fn gu_gamma_correction_rgb(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    hw::set_gamma_rgb(cpu, [f(a[0]), f(a[1]), f(a[2])]);
    Some(0)
}

/// grLfbWriteRegion: Glide 2's, with an argument before the stride
/// that says whether the pixels go through the pixel pipeline, which
/// Glide 2's never do here either.
fn gr_lfb_write_region(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let args = [a[0], a[1], a[2], a[3], a[4], a[5], a[7], a[8]];
    x::gr_lfb_write_region(cpu, &args)
}

// --- Drawing ---

fn gr_draw_point(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let p = vertex(cpu, a[0])?;
    draw_point(cpu, &p);
    Some(0)
}

fn gr_draw_line(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let (p, q) = (vertex(cpu, a[0])?, vertex(cpu, a[1])?);
    draw_line(cpu, &p, &q);
    Some(0)
}

fn gr_draw_triangle(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let (p, q, r) = (vertex(cpu, a[0])?, vertex(cpu, a[1])?, vertex(cpu, a[2])?);
    api::draw_triangle(cpu, &p, &q, &r);
    Some(0)
}

/// grAADrawTriangle: drawn without its antialiased edges, as Glide 2's.
fn gr_aa_draw_triangle(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    gr_draw_triangle(cpu, a)
}

fn gr_draw_vertex_array(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let list = vertex_pointers(cpu, a[1], a[2])?;
    draw_array(cpu, a[0], &list);
    Some(0)
}

fn gr_draw_vertex_array_contiguous(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let list = vertex_run(cpu, a[1], a[2], a[3])?;
    draw_array(cpu, a[0], &list);
    Some(0)
}

// --- Textures ---

/// The texture units there are, for the tables that go to all of them.
fn all_tmus(cpu: &Cpu) -> std::ops::Range<u32> {
    0..hw::board(cpu).map_or(0, |b| b.tmus)
}

fn gr_tex_calc_mem_required(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let info = TexInfo {
        small_lod: lod_glide2(a[0]).unwrap_or(u32::MAX),
        large_lod: lod_glide2(a[1]).unwrap_or(u32::MAX),
        aspect: aspect_glide2(a[2]).unwrap_or(u32::MAX),
        format: a[3],
    };
    Some(api::tex_texture_mem_required(
        cpu,
        api::MIPMAPLEVELMASK_BOTH,
        &info,
    ))
}

fn gr_tex_texture_mem_required(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let (info, _) = tex_info(cpu, a[1])?;
    Some(api::tex_texture_mem_required(cpu, a[0], &info))
}

/// grTexSource, which also sets how S and T in clip coordinates scale
/// to the texture.
fn gr_tex_source(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let (info, _) = tex_info(cpu, a[3])?;
    api::tex_source(cpu, a[0], a[1], a[2], &info);
    if a[0] < 2 && info.aspect <= api::ASPECT_1X8 {
        let (w, h) = api::level_size(info.aspect, api::LOD_256);
        g3(cpu).st_scale[a[0] as usize] = [w as f32, h as f32];
    }
    Some(0)
}

/// grTexMultibaseAddress: GR_TEXBASE_* count from 32 to 1 (0) up to
/// 256 (3), where Glide 2's count down.
fn gr_tex_multibase_address(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let (info, _) = tex_info(cpu, a[4])?;
    let range = 3u32.checked_sub(a[1]).unwrap_or(u32::MAX);
    api::tex_multibase_address(cpu, a[0], range, a[2], a[3], &info);
    Some(0)
}

fn gr_tex_download_mip_map(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    let (info, data) = tex_info(cpu, a[3])?;
    let texels = x::bytes(cpu, data, x::mipmap_len(&info))?;
    api::tex_download_mipmap(cpu, a[0], a[1], a[2], &info, &texels);
    Some(0)
}

/// The arguments of grTexDownloadMipMapLevel and its partial form in
/// Glide 2's numbers: this level, the largest, and the aspect ratio.
fn level_args(a: &[u32]) -> Vec<u32> {
    let mut args = a.to_vec();
    args[2] = lod_glide2(a[2]).unwrap_or(u32::MAX);
    args[3] = lod_glide2(a[3]).unwrap_or(u32::MAX);
    args[4] = aspect_glide2(a[4]).unwrap_or(u32::MAX);
    args
}

fn gr_tex_download_mip_map_level(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    x::download_level(cpu, &level_args(a), 0, 255)
}

fn gr_tex_download_mip_map_level_partial(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    x::download_level(cpu, &level_args(a), a[8], a[9])?;
    Some(TRUE)
}

/// grTexDownloadTable(type, data): to every texture unit.
fn gr_tex_download_table(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    for tmu in all_tmus(cpu) {
        x::download_table(cpu, tmu, a[0], a[1], 0, 255)?;
    }
    Some(0)
}

fn gr_tex_download_table_partial(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    for tmu in all_tmus(cpu) {
        x::download_table(cpu, tmu, a[0], a[1], a[2], a[3])?;
    }
    Some(0)
}

/// grTexNCCTable(table): for every texture unit.
fn gr_tex_ncc_table(cpu: &mut Cpu, a: &[u32]) -> Option<u32> {
    for tmu in all_tmus(cpu) {
        api::tex_ncc_table(cpu, tmu, a[0]);
    }
    Some(0)
}

/// The functions, in the order of their names: the union of what 3dfx's
/// GLIDE3X.DLLs for the Voodoo2, Banshee and Voodoo3 export. Append
/// only (see `stubs`).
pub static APIS: &[Api] = &[
    stdcall("__pciFetchRegister@16", 1, 4, missing_pci_fetch_register),
    stdcall("__pciUpdateRegister@20", 2, 5, missing_pci_update_register),
    stdcall("_grAADrawTriangle@24", 3, 6, gr_aa_draw_triangle),
    stdcall("_grAlphaBlendFunction@16", 4, 4, x::gr_alpha_blend_function),
    stdcall("_grAlphaCombine@20", 5, 5, x::gr_alpha_combine),
    stdcall(
        "_grAlphaControlsITRGBLighting@4",
        6,
        1,
        x::gr_alpha_controls_itrgb_lighting,
    ),
    stdcall("_grAlphaTestFunction@4", 7, 1, x::gr_alpha_test_function),
    stdcall(
        "_grAlphaTestReferenceValue@4",
        8,
        1,
        x::gr_alpha_test_reference_value,
    ),
    stdcall("_grBufferClear@12", 9, 3, x::gr_buffer_clear),
    stdcall("_grBufferSwap@4", 10, 1, x::gr_buffer_swap),
    stdcall("_grCheckForRoom@4", 11, 1, x::gr_check_for_room),
    stdcall("_grChromakeyMode@4", 12, 1, x::gr_chromakey_mode),
    stdcall("_grChromakeyValue@4", 13, 1, x::gr_chromakey_value),
    stdcall("_grClipWindow@16", 14, 4, x::gr_clip_window),
    stdcall("_grColorCombine@20", 15, 5, x::gr_color_combine),
    stdcall("_grColorMask@8", 16, 2, x::gr_color_mask),
    stdcall("_grConstantColorValue@4", 17, 1, x::gr_constant_color_value),
    stdcall("_grCoordinateSpace@4", 18, 1, gr_coordinate_space),
    stdcall("_grCullMode@4", 19, 1, x::gr_cull_mode),
    stdcall("_grDepthBiasLevel@4", 20, 1, x::gr_depth_bias_level),
    stdcall(
        "_grDepthBufferFunction@4",
        21,
        1,
        x::gr_depth_buffer_function,
    ),
    stdcall("_grDepthBufferMode@4", 22, 1, x::gr_depth_buffer_mode),
    stdcall("_grDepthMask@4", 23, 1, x::gr_depth_mask),
    stdcall("_grDepthRange@8", 24, 2, gr_depth_range),
    stdcall("_grDisable@4", 25, 1, gr_disable),
    stdcall("_grDisableAllEffects@0", 26, 0, x::gr_disable_all_effects),
    stdcall("_grDitherMode@4", 27, 1, x::gr_dither_mode),
    stdcall("_grDrawLine@8", 28, 2, gr_draw_line),
    stdcall("_grDrawPoint@4", 29, 1, gr_draw_point),
    stdcall("_grDrawTriangle@12", 30, 3, gr_draw_triangle),
    stdcall("_grDrawVertexArray@12", 31, 3, gr_draw_vertex_array),
    stdcall(
        "_grDrawVertexArrayContiguous@16",
        32,
        4,
        gr_draw_vertex_array_contiguous,
    ),
    stdcall("_grEnable@4", 33, 1, gr_enable),
    stdcall("_grErrorSetCallback@4", 34, 1, x::gr_error_set_callback),
    stdcall("_grFinish@0", 35, 0, nothing),
    stdcall("_grFlush@0", 36, 0, nothing),
    stdcall("_grFogColorValue@4", 37, 1, x::gr_fog_color_value),
    stdcall("_grFogMode@4", 38, 1, gr_fog_mode),
    stdcall("_grFogTable@4", 39, 1, x::gr_fog_table),
    stdcall("_grGet@12", 40, 3, gr_get),
    stdcall("_grGetProcAddress@4", 41, 1, gr_get_proc_address),
    stdcall("_grGetString@4", 42, 1, gr_get_string),
    stdcall("_grGlideGetState@4", 43, 1, gr_glide_get_state),
    stdcall(
        "_grGlideGetVertexLayout@4",
        44,
        1,
        gr_glide_get_vertex_layout,
    ),
    stdcall("_grGlideInit@0", 45, 0, x::gr_glide_init),
    stdcall("_grGlideSetState@4", 46, 1, gr_glide_set_state),
    stdcall(
        "_grGlideSetVertexLayout@4",
        47,
        1,
        gr_glide_set_vertex_layout,
    ),
    stdcall("_grGlideShutdown@0", 48, 0, x::gr_glide_shutdown),
    stdcall("_grLfbConstantAlpha@4", 49, 1, x::gr_lfb_constant_alpha),
    stdcall("_grLfbConstantDepth@4", 50, 1, x::gr_lfb_constant_depth),
    stdcall("_grLfbLock@24", 51, 6, x::gr_lfb_lock),
    stdcall("_grLfbReadRegion@28", 52, 7, x::gr_lfb_read_region),
    stdcall("_grLfbUnlock@8", 53, 2, x::gr_lfb_unlock),
    stdcall(
        "_grLfbWriteColorFormat@4",
        54,
        1,
        x::gr_lfb_write_color_format,
    ),
    stdcall(
        "_grLfbWriteColorSwizzle@8",
        55,
        2,
        x::gr_lfb_write_color_swizzle,
    ),
    stdcall("_grLfbWriteRegion@36", 56, 9, gr_lfb_write_region),
    stdcall("_grLoadGammaTable@16", 57, 4, gr_load_gamma_table),
    stdcall("_grQueryResolutions@8", 58, 2, gr_query_resolutions),
    stdcall("_grRenderBuffer@4", 59, 1, x::gr_render_buffer),
    stdcall("_grReset@4", 60, 1, gr_reset),
    stdcall("_grSelectContext@4", 61, 1, gr_select_context),
    stdcall("_grSetNumPendingBuffers@4", 62, 1, nothing),
    stdcall("_grSplash@20", 63, 5, missing_gr_splash),
    stdcall(
        "_grSstConfigPipeline@12",
        64,
        3,
        missing_gr_sst_config_pipeline,
    ),
    stdcall("_grSstOrigin@4", 65, 1, x::gr_sst_origin),
    stdcall("_grSstSelect@4", 66, 1, x::gr_sst_select),
    stdcall("_grSstVidMode@8", 67, 2, missing_gr_sst_vid_mode),
    stdcall("_grSstWinClose@4", 68, 1, gr_sst_win_close),
    stdcall("_grSstWinOpen@28", 69, 7, gr_sst_win_open),
    stdcall("_grTexCalcMemRequired@16", 70, 4, gr_tex_calc_mem_required),
    stdcall("_grTexClampMode@12", 71, 3, x::gr_tex_clamp_mode),
    stdcall("_grTexCombine@28", 72, 7, x::gr_tex_combine),
    stdcall("_grTexDetailControl@16", 73, 4, x::gr_tex_detail_control),
    stdcall("_grTexDownloadMipMap@16", 74, 4, gr_tex_download_mip_map),
    stdcall(
        "_grTexDownloadMipMapLevel@32",
        75,
        8,
        gr_tex_download_mip_map_level,
    ),
    stdcall(
        "_grTexDownloadMipMapLevelPartial@40",
        76,
        10,
        gr_tex_download_mip_map_level_partial,
    ),
    stdcall("_grTexDownloadTable@8", 77, 2, gr_tex_download_table),
    stdcall(
        "_grTexDownloadTablePartial@16",
        78,
        4,
        gr_tex_download_table_partial,
    ),
    stdcall("_grTexFilterMode@12", 79, 3, x::gr_tex_filter_mode),
    stdcall("_grTexLodBiasValue@8", 80, 2, x::gr_tex_lod_bias_value),
    stdcall("_grTexMaxAddress@4", 81, 1, x::gr_tex_max_address),
    stdcall("_grTexMinAddress@4", 82, 1, x::gr_tex_min_address),
    stdcall("_grTexMipMapMode@12", 83, 3, x::gr_tex_mip_map_mode),
    stdcall("_grTexMultibase@8", 84, 2, x::gr_tex_multibase),
    stdcall("_grTexMultibaseAddress@20", 85, 5, gr_tex_multibase_address),
    stdcall("_grTexNCCTable@4", 86, 1, gr_tex_ncc_table),
    stdcall("_grTexSource@16", 87, 4, gr_tex_source),
    stdcall(
        "_grTexTextureMemRequired@8",
        88,
        2,
        gr_tex_texture_mem_required,
    ),
    stdcall("_grVertexLayout@12", 89, 3, gr_vertex_layout),
    stdcall("_grViewport@16", 90, 4, gr_viewport),
    stdcall("_gu3dfGetInfo@8", 91, 2, file3df::get_info3),
    stdcall("_gu3dfLoad@8", 92, 2, file3df::load3),
    stdcall("_guEncodeRLE16@16", 93, 4, missing_gu_encode_rle16),
    stdcall("_guFogGenerateExp2@8", 94, 2, x::gu_fog_generate_exp2),
    stdcall("_guFogGenerateExp@8", 95, 2, x::gu_fog_generate_exp),
    stdcall("_guFogGenerateLinear@12", 96, 3, x::gu_fog_generate_linear),
    stdcall("_guFogTableIndexToW@4", 97, 1, x::gu_fog_table_index_to_w),
    stdcall("_guGammaCorrectionRGB@12", 98, 3, gu_gamma_correction_rgb),
    stdcall("_pciClose@0", 99, 0, missing_pci_close),
    stdcall("_pciDeviceExists@4", 100, 1, missing_pci_device_exists),
    stdcall("_pciFindCard@12", 101, 3, missing_pci_find_card),
    stdcall("_pciFindCardMulti@16", 102, 4, missing_pci_find_card_multi),
    stdcall(
        "_pciFindCardMultiFunc@16",
        103,
        4,
        missing_pci_find_card_multi_func,
    ),
    stdcall("_pciFindFreeMTRR@4", 104, 1, missing_pci_find_free_mtrr),
    stdcall("_pciFindMTRRMatch@16", 105, 4, missing_pci_find_mtrr_match),
    stdcall("_pciGetConfigData@20", 106, 5, missing_pci_get_config_data),
    stdcall("_pciGetErrorCode@0", 107, 0, missing_pci_get_error_code),
    stdcall("_pciGetErrorString@0", 108, 0, missing_pci_get_error_string),
    stdcall(
        "_pciLinearRangeSetPermission@12",
        109,
        3,
        missing_pci_linear_range_set_permission,
    ),
    stdcall("_pciMapCard@20", 110, 5, missing_pci_map_card),
    stdcall("_pciMapCardMulti@24", 111, 6, missing_pci_map_card_multi),
    stdcall(
        "_pciMapPhysicalDeviceToLinear@16",
        112,
        4,
        missing_pci_map_physical_device_to_linear,
    ),
    stdcall(
        "_pciMapPhysicalToLinear@12",
        113,
        3,
        missing_pci_map_physical_to_linear,
    ),
    stdcall("_pciOpen@0", 114, 0, missing_pci_open),
    stdcall("_pciOpenEx@4", 115, 1, missing_pci_open_ex),
    stdcall(
        "_pciOutputDebugString@4",
        116,
        1,
        missing_pci_output_debug_string,
    ),
    stdcall("_pciSetConfigData@20", 117, 5, missing_pci_set_config_data),
    stdcall("_pciSetMTRR@16", 118, 4, missing_pci_set_mtrr),
    stdcall("_pciSetMTRRAmdK6@16", 119, 4, missing_pci_set_mtrr_amd_k6),
    stdcall(
        "_pciSetPassThroughBase@8",
        120,
        2,
        missing_pci_set_pass_through_base,
    ),
    stdcall("_pciUnmapPhysical@8", 121, 2, missing_pci_unmap_physical),
    stdcall("_pioInByte@4", 122, 1, missing_pio_in_byte),
    stdcall("_pioInLong@4", 123, 1, missing_pio_in_long),
    stdcall("_pioInWord@4", 124, 1, missing_pio_in_word),
    stdcall("_pioOutByte@8", 125, 2, missing_pio_out_byte),
    stdcall("_pioOutLong@8", 126, 2, missing_pio_out_long),
    stdcall("_pioOutWord@8", 127, 2, missing_pio_out_word),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lods_and_aspects_convert_both_ways() {
        for lod in 0..=8 {
            assert_eq!(lod_glide2(lod_log2(lod)), Some(lod));
        }
        for aspect in 0..=6 {
            assert_eq!(aspect_glide2(aspect_log2(aspect)), Some(aspect));
        }
        assert_eq!(lod_glide2(8), Some(api::LOD_256));
        assert_eq!(lod_glide2(0), Some(api::LOD_1));
        assert_eq!(lod_glide2(9), None);
        assert_eq!(aspect_glide2(3), Some(api::ASPECT_8X1));
        assert_eq!(aspect_glide2(-3i32 as u32), Some(api::ASPECT_1X8));
        assert_eq!(aspect_glide2(4), None);
    }

    #[test]
    fn glide3_has_glide2s_resolutions_and_more() {
        for res in 0..0x10 {
            assert_eq!(resolution_size(res), api::resolution_size(res));
        }
        assert_eq!(resolution_size(0x10), Some((1152, 864)));
        assert_eq!(resolution_size(0x11), Some((1280, 960)));
        assert_eq!(resolution_size(RESOLUTIONS - 1), Some((2048, 2048)));
        assert_eq!(resolution_size(RESOLUTIONS), None);
    }

    #[test]
    fn exports_are_sorted_and_numbered() {
        for (i, api) in APIS.iter().enumerate() {
            assert_eq!(api.ordinal as usize, i + 1, "{}", api.name);
            let at = api.name.rfind('@').unwrap();
            let bytes: u8 = api.name[at + 1..].parse().unwrap();
            assert_eq!(api.args * 4, bytes, "{}", api.name);
        }
        assert!(APIS.windows(2).all(|w| w[0].name < w[1].name));
    }
}
