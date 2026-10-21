//! Glide's utility functions (the `gu` ones): fog tables, the old
//! combine functions in one call, triangles and polygons clipped to the
//! clip window, and the texture memory manager that hands out texture
//! memory and keeps what each texture is (`GrMipMapId_t`).

use super::api::{self, TexInfo, Vertex};
use super::state;
use crate::cpu::Cpu;

/// A texture the memory manager has given memory to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MipMap {
    pub tmu: u32,
    pub start: u32,
    pub size: u32,
    pub even_odd: u32,
    pub info: TexInfo,
    pub mm_mode: u32,
    pub clamp: [u32; 2],
    pub filter: [u32; 2],
    pub lod_bias: f32,
    pub trilinear: bool,
    pub width: u32,
    pub height: u32,
}

crate::state_fields!(MipMap {
    tmu,
    start,
    size,
    even_odd,
    info,
    mm_mode,
    clamp,
    filter,
    lod_bias,
    trilinear,
    width,
    height
});

crate::state_fields!(TexInfo {
    small_lod,
    large_lod,
    aspect,
    format
});

/// The texture memory manager.
#[derive(Clone, Debug, Default)]
pub struct Gu {
    /// The textures by their ids.
    pub mipmaps: Vec<MipMap>,
    /// The first free byte of each texture unit's memory.
    pub free: [u32; 2],
    /// The texture each unit maps, or NULL_MIPMAP.
    pub current: [u32; 2],
}

crate::state_fields!(Gu {
    mipmaps,
    free,
    current
});

/// GR_NULL_MIPMAP_HANDLE.
pub const NULL_MIPMAP: u32 = u32::MAX;

fn gu(cpu: &mut Cpu) -> &mut Gu {
    &mut state(cpu).gu
}

// --- Fog ---

/// The W of fog table entry `i` (guFogTableIndexToW).
pub fn fog_table_index_to_w(i: i32) -> f32 {
    let e = 3.0 + (i >> 2) as f64;
    (2f64.powf(e) as f32) / (8 - (i & 3)) as f32
}

/// A fog table from a function of W from 0 to 1.
fn fog_from(f: impl Fn(f32) -> f32) -> [u8; api::FOG_TABLE_SIZE] {
    std::array::from_fn(|i| (f(fog_table_index_to_w(i as i32)).clamp(0.0, 1.0) * 255.0) as u8)
}

/// Fog growing as 1 - e^(-density * w), 255 at the far end
/// (guFogGenerateExp).
pub fn fog_generate_exp(density: f32) -> [u8; api::FOG_TABLE_SIZE] {
    let far = density * fog_table_index_to_w(api::FOG_TABLE_SIZE as i32 - 1);
    let scale = 1.0 / (1.0 - (-far).exp());
    fog_from(|w| (1.0 - (-(density * w)).exp()) * scale)
}

/// Fog growing as 1 - e^(-(density * w)^2) (guFogGenerateExp2).
pub fn fog_generate_exp2(density: f32) -> [u8; api::FOG_TABLE_SIZE] {
    let far = density * fog_table_index_to_w(api::FOG_TABLE_SIZE as i32 - 1);
    let scale = 1.0 / (1.0 - (-(far * far)).exp());
    fog_from(|w| {
        let d = density * w;
        (1.0 - (-(d * d)).exp()) * scale
    })
}

/// Fog growing in a straight line from `near` to `far` (in W)
/// (guFogGenerateLinear).
pub fn fog_generate_linear(near: f32, far: f32) -> [u8; api::FOG_TABLE_SIZE] {
    fog_from(|w| (w.min(65535.0) - near) / (far - near))
}

// --- The old combine functions ---

/// guColorCombineFunction: the colour combine unit for one of Glide
/// 1's functions.
pub fn color_combine_function(cpu: &mut Cpu, function: u32) {
    use api::*;
    api::set_delta0(cpu, false);
    let (f, factor, local, other, invert) = match function {
        0 => (
            COMBINE_FUNCTION_ZERO,
            COMBINE_FACTOR_NONE,
            COMBINE_LOCAL_NONE,
            COMBINE_OTHER_NONE,
            false,
        ),
        1 => (
            COMBINE_FUNCTION_LOCAL,
            COMBINE_FACTOR_NONE,
            COMBINE_LOCAL_CONSTANT,
            COMBINE_OTHER_NONE,
            false,
        ),
        2 | 3 => (
            COMBINE_FUNCTION_LOCAL,
            COMBINE_FACTOR_NONE,
            COMBINE_LOCAL_ITERATED,
            COMBINE_OTHER_NONE,
            false,
        ),
        4 => (
            COMBINE_FUNCTION_SCALE_OTHER,
            COMBINE_FACTOR_ONE,
            COMBINE_LOCAL_NONE,
            COMBINE_OTHER_TEXTURE,
            false,
        ),
        5 => (
            COMBINE_FUNCTION_SCALE_OTHER,
            COMBINE_FACTOR_LOCAL,
            COMBINE_LOCAL_CONSTANT,
            COMBINE_OTHER_TEXTURE,
            false,
        ),
        6 | 7 => (
            COMBINE_FUNCTION_SCALE_OTHER,
            COMBINE_FACTOR_LOCAL,
            COMBINE_LOCAL_ITERATED,
            COMBINE_OTHER_TEXTURE,
            false,
        ),
        8 | 14 => (
            COMBINE_FUNCTION_SCALE_OTHER_ADD_LOCAL_ALPHA,
            COMBINE_FACTOR_LOCAL,
            COMBINE_LOCAL_ITERATED,
            COMBINE_OTHER_TEXTURE,
            false,
        ),
        9 => (
            COMBINE_FUNCTION_SCALE_OTHER,
            COMBINE_FACTOR_LOCAL_ALPHA,
            COMBINE_LOCAL_NONE,
            COMBINE_OTHER_TEXTURE,
            false,
        ),
        10 | 15 => (
            COMBINE_FUNCTION_SCALE_OTHER_ADD_LOCAL,
            COMBINE_FACTOR_LOCAL_ALPHA,
            COMBINE_LOCAL_ITERATED,
            COMBINE_OTHER_TEXTURE,
            false,
        ),
        11 => (
            COMBINE_FUNCTION_SCALE_OTHER_ADD_LOCAL,
            COMBINE_FACTOR_ONE,
            COMBINE_LOCAL_ITERATED,
            COMBINE_OTHER_TEXTURE,
            false,
        ),
        12 => (
            COMBINE_FUNCTION_SCALE_OTHER_MINUS_LOCAL,
            COMBINE_FACTOR_ONE,
            COMBINE_LOCAL_ITERATED,
            COMBINE_OTHER_TEXTURE,
            false,
        ),
        13 => (
            COMBINE_FUNCTION_BLEND,
            COMBINE_FACTOR_TEXTURE_ALPHA,
            COMBINE_LOCAL_CONSTANT,
            COMBINE_OTHER_ITERATED,
            false,
        ),
        16 => (
            COMBINE_FUNCTION_ZERO,
            COMBINE_FACTOR_NONE,
            COMBINE_LOCAL_NONE,
            COMBINE_OTHER_NONE,
            true,
        ),
        _ => return,
    };
    if matches!(function, 3 | 7) {
        api::set_delta0(cpu, true);
    }
    api::color_combine(cpu, f, factor, local, other, invert);
}

/// guAlphaSource: the alpha combine unit for one of Glide 1's alpha
/// sources.
pub fn alpha_source(cpu: &mut Cpu, source: u32) {
    use api::*;
    let (f, factor, local, other) = match source {
        0 => (
            COMBINE_FUNCTION_LOCAL,
            COMBINE_FACTOR_NONE,
            COMBINE_LOCAL_CONSTANT,
            COMBINE_OTHER_NONE,
        ),
        1 => (
            COMBINE_FUNCTION_LOCAL,
            COMBINE_FACTOR_NONE,
            COMBINE_LOCAL_ITERATED,
            COMBINE_OTHER_NONE,
        ),
        2 => (
            COMBINE_FUNCTION_SCALE_OTHER,
            COMBINE_FACTOR_ONE,
            COMBINE_LOCAL_NONE,
            COMBINE_OTHER_TEXTURE,
        ),
        3 => (
            COMBINE_FUNCTION_SCALE_OTHER,
            COMBINE_FACTOR_LOCAL,
            COMBINE_LOCAL_ITERATED,
            COMBINE_OTHER_TEXTURE,
        ),
        _ => return,
    };
    api::alpha_combine(cpu, f, factor, local, other, false);
}

/// guTexCombineFunction and grTexCombineFunction: a texture unit's
/// combine unit for one of Glide 1's functions.
pub fn tex_combine_function(cpu: &mut Cpu, tmu: u32, function: u32) {
    use api::*;
    // GR_COMBINE_FACTOR_DETAIL_FACTOR has texture alpha's value.
    let (f, factor, invert) = match function {
        0 => (COMBINE_FUNCTION_ZERO, COMBINE_FACTOR_NONE, false),
        1 => (COMBINE_FUNCTION_LOCAL, COMBINE_FACTOR_NONE, false),
        2 => (COMBINE_FUNCTION_SCALE_OTHER, COMBINE_FACTOR_ONE, false),
        3 => (
            COMBINE_FUNCTION_SCALE_OTHER_ADD_LOCAL,
            COMBINE_FACTOR_ONE,
            false,
        ),
        4 => (COMBINE_FUNCTION_SCALE_OTHER, COMBINE_FACTOR_LOCAL, false),
        5 => (
            COMBINE_FUNCTION_SCALE_OTHER_MINUS_LOCAL,
            COMBINE_FACTOR_ONE,
            false,
        ),
        6 => (
            COMBINE_FUNCTION_BLEND,
            COMBINE_FACTOR_ONE_MINUS_TEXTURE_ALPHA,
            false,
        ),
        7 => (COMBINE_FUNCTION_BLEND, COMBINE_FACTOR_TEXTURE_ALPHA, false),
        8 => (
            COMBINE_FUNCTION_BLEND,
            COMBINE_FACTOR_ONE_MINUS_LOD_FRACTION,
            false,
        ),
        9 => (COMBINE_FUNCTION_BLEND, COMBINE_FACTOR_LOD_FRACTION, false),
        10 => (COMBINE_FUNCTION_ZERO, COMBINE_FACTOR_NONE, true),
        _ => return,
    };
    api::tex_combine(cpu, tmu, f, factor, f, factor, invert, invert);
}

// --- Clipping ---

/// Where a clipped edge crosses: `a` + `d` of the way to `b`.
fn lerp(a: &Vertex, b: &Vertex, d: f32) -> Vertex {
    let l = |p: f32, q: f32| p + d * (q - p);
    let mut v = Vertex {
        x: l(a.x, b.x),
        y: l(a.y, b.y),
        z: l(a.z, b.z),
        r: l(a.r, b.r),
        g: l(a.g, b.g),
        b: l(a.b, b.b),
        ooz: l(a.ooz, b.ooz),
        a: l(a.a, b.a),
        oow: l(a.oow, b.oow),
        tmu: a.tmu,
    };
    for t in 0..2 {
        v.tmu[t].sow = l(a.tmu[t].sow, b.tmu[t].sow);
        v.tmu[t].tow = l(a.tmu[t].tow, b.tmu[t].tow);
        v.tmu[t].oow = l(a.tmu[t].oow, b.tmu[t].oow);
    }
    v
}

/// One edge of the clip window: the vertices inside it, and where the
/// polygon's edges cross it (Sutherland and Hodgman).
fn clip_edge(
    input: &[Vertex],
    inside: impl Fn(&Vertex) -> bool,
    cross: impl Fn(&Vertex, &Vertex) -> Vertex,
) -> Vec<Vertex> {
    let mut out = Vec::with_capacity(input.len() + 2);
    for (i, b) in input.iter().enumerate() {
        let a = &input[(i + input.len() - 1) % input.len()];
        match (inside(a), inside(b)) {
            (true, true) => out.push(*b),
            (true, false) => out.push(cross(a, b)),
            (false, true) => {
                out.push(cross(a, b));
                out.push(*b);
            }
            (false, false) => {}
        }
    }
    out
}

/// A polygon clipped to the clip window, its vertices snapped to
/// sixteenths of a pixel as Glide snaps them.
fn clip(cpu: &mut Cpu, vertices: &[Vertex]) -> Vec<Vertex> {
    let [x0, y0, x1, y1] = state(cpu).gc.clip.map(|v| v as f32);
    let mut p = vertices.to_vec();
    if p.is_empty() {
        return p;
    }
    p = clip_edge(
        &p,
        |v| v.x < x1,
        |a, b| {
            let mut v = lerp(a, b, (x1 - a.x) / (b.x - a.x));
            v.x = x1;
            v
        },
    );
    p = clip_edge(
        &p,
        |v| v.y < y1,
        |a, b| {
            let mut v = lerp(a, b, (y1 - a.y) / (b.y - a.y));
            v.y = y1;
            v
        },
    );
    p = clip_edge(
        &p,
        |v| v.x >= x0,
        |a, b| {
            let mut v = lerp(a, b, (x0 - a.x) / (b.x - a.x));
            v.x = x0;
            v
        },
    );
    p = clip_edge(
        &p,
        |v| v.y >= y0,
        |a, b| {
            let mut v = lerp(a, b, (y0 - a.y) / (b.y - a.y));
            v.y = y0;
            v
        },
    );
    const SNAP: f32 = (3 << 18) as f32;
    for v in &mut p {
        v.x = (v.x + SNAP) - SNAP;
        v.y = (v.y + SNAP) - SNAP;
    }
    p
}

/// guDrawTriangleWithClip: a triangle cut to the clip window.
pub fn draw_triangle_with_clip(cpu: &mut Cpu, a: &Vertex, b: &Vertex, c: &Vertex) {
    let [x0, y0, x1, y1] = state(cpu).gc.clip.map(|v| v as f32);
    let inside = |v: &Vertex| v.x >= x0 && v.x < x1 && v.y >= y0 && v.y < y1;
    if inside(a) && inside(b) && inside(c) {
        api::draw_triangle(cpu, a, b, c);
        return;
    }
    let p = clip(cpu, &[*a, *b, *c]);
    api::draw_polygon(cpu, &p);
}

/// guDrawPolygonVertexListWithClip: a convex polygon cut to the clip
/// window.
pub fn draw_polygon_with_clip(cpu: &mut Cpu, vertices: &[Vertex]) {
    let p = clip(cpu, vertices);
    api::draw_polygon(cpu, &p);
}

// --- The texture memory manager ---

/// Memory for a texture (guTexAllocateMemory): its id, or NULL_MIPMAP
/// if the unit's memory is full.
pub fn tex_allocate(cpu: &mut Cpu, m: MipMap) -> u32 {
    if !m.info_valid() {
        return NULL_MIPMAP;
    }
    let tmu = m.tmu.min(1) as usize;
    let size = api::tex_texture_mem_required(cpu, m.even_odd, &m.info);
    let max = api::tex_max_address(cpu, m.tmu) + 8;
    let start = gu(cpu).free[tmu];
    // A texture doesn't cross a 2 MB boundary of a Voodoo's memory.
    let start = if !super::hw::banshee(cpu) && start / 0x20_0000 != (start + size - 1) / 0x20_0000 {
        (start + size - 1) & !0x1F_FFFF
    } else {
        start
    };
    if size == 0 || start + size > max {
        return NULL_MIPMAP;
    }
    let g = gu(cpu);
    g.free[tmu] = start + size;
    g.mipmaps.push(MipMap { start, size, ..m });
    g.mipmaps.len() as u32 - 1
}

impl MipMap {
    fn info_valid(&self) -> bool {
        self.info.small_lod <= 8
            && self.info.large_lod <= self.info.small_lod
            && self.info.aspect <= 6
            && self.info.format <= 0xF
            && (1..=3).contains(&self.even_odd)
    }
}

pub fn mipmap(cpu: &mut Cpu, id: u32) -> Option<MipMap> {
    gu(cpu).mipmaps.get(id as usize).copied()
}

/// New attributes for a texture (guTexChangeAttributes): false if it
/// would need more memory than it has.
pub fn tex_change_attributes(cpu: &mut Cpu, id: u32, update: impl FnOnce(&mut MipMap)) -> bool {
    let Some(mut m) = mipmap(cpu, id) else {
        return false;
    };
    update(&mut m);
    if !m.info_valid() || api::tex_texture_mem_required(cpu, m.even_odd, &m.info) > m.size {
        return false;
    }
    gu(cpu).mipmaps[id as usize] = m;
    true
}

/// Map a texture (guTexSource): its address, filters, clamping and
/// mipmapping.
pub fn tex_source(cpu: &mut Cpu, id: u32) {
    let Some(m) = mipmap(cpu, id) else {
        return;
    };
    api::tex_mipmap_mode(cpu, m.tmu, m.mm_mode, m.trilinear);
    api::tex_source(cpu, m.tmu, m.start, m.even_odd, &m.info);
    api::tex_clamp_mode(cpu, m.tmu, m.clamp[0], m.clamp[1]);
    api::tex_filter_mode(cpu, m.tmu, m.filter[0], m.filter[1]);
    api::tex_lod_bias_value(cpu, m.tmu, m.lod_bias);
    gu(cpu).current[m.tmu.min(1) as usize] = id;
}

/// The texture a unit maps (guTexGetCurrentMipMap).
pub fn tex_current(cpu: &mut Cpu, tmu: u32) -> u32 {
    gu(cpu)
        .current
        .get(tmu as usize)
        .copied()
        .unwrap_or(NULL_MIPMAP)
}

/// The texture memory still free (guTexMemQueryAvail).
pub fn tex_mem_query_avail(cpu: &mut Cpu, tmu: u32) -> u32 {
    let max = api::tex_max_address(cpu, tmu) + 8;
    max.saturating_sub(gu(cpu).free[tmu.min(1) as usize])
}

/// All texture memory free again (guTexMemReset).
pub fn tex_mem_reset(cpu: &mut Cpu) {
    *gu(cpu) = Gu {
        current: [NULL_MIPMAP; 2],
        ..Gu::default()
    };
}
