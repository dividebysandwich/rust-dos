//! Drawing: DrawPrimitive and its kin (indexed, strided, from vertex
//! buffers), Begin, Vertex and End, and ComputeSphereVisibility. Each
//! reads the program's vertices, gives them with the device's state to
//! the pipeline (`pipeline.rs`), and counts what was drawn in the
//! device's statistics and clip status.
//!
//! The rasterizer is the card's (`hal.rs`). Without a card to draw on,
//! what comes out of the pipeline is only counted (`pipeline::Counter`).

use super::super::DDERR_INVALIDPARAMS;
use super::caps::GUARD_BAND;
use super::device::{
    Begin, Device, RS_AMBIENT, RS_AMBIENTMATERIALSOURCE, RS_CLIPPING, RS_CLIPPLANEENABLE,
    RS_COLORVERTEX, RS_CULLMODE, RS_DIFFUSEMATERIALSOURCE, RS_EMISSIVEMATERIALSOURCE, RS_FILLMODE,
    RS_FOGDENSITY, RS_FOGENABLE, RS_FOGEND, RS_FOGSTART, RS_FOGTABLEMODE, RS_FOGVERTEXMODE,
    RS_LIGHTING, RS_LOCALVIEWER, RS_NORMALIZENORMALS, RS_SHADEMODE, RS_SPECULARENABLE,
    RS_SPECULARMATERIALSOURCE, TSS_TEXCOORDINDEX, device_of, get, version,
};
use super::pipeline::{
    self, Fog, Layout, Light, Lighting, Material, Prim, Processed, Setup, Vertex, ViewMap,
};
use super::{
    D3D_OK, D3DERR_INBEGIN, D3DERR_INVALIDPRIMITIVETYPE, D3DERR_INVALIDVERTEXTYPE,
    D3DERR_NOTINBEGIN, D3DERR_VERTEXBUFFERLOCKED, probe, read_bytes, read_dwords,
};
use crate::cpu::Cpu;

// DrawPrimitive's flags.
pub const D3DDP_DONOTCLIP: u32 = 0x4;
pub const D3DDP_DONOTUPDATEEXTENTS: u32 = 0x8;
pub const D3DDP_DONOTLIGHT: u32 = 0x10;

/// The FVF code of a vertex type: IDirect3DDevice2's D3DVERTEXTYPE, or
/// an FVF code from IDirect3DDevice3 on.
fn vertex_format(vtable: usize, vt: u32) -> Result<Layout, u32> {
    let fvf = if version(vtable) <= 2 {
        match vt {
            1 => pipeline::FVF_VERTEX,
            2 => pipeline::FVF_LVERTEX,
            3 => pipeline::FVF_TLVERTEX,
            _ => return Err(D3DERR_INVALIDVERTEXTYPE),
        }
    } else {
        vt
    };
    Layout::of(fvf).ok_or(DDERR_INVALIDPARAMS)
}

fn check_kind(kind: u32) -> Result<(), u32> {
    if (pipeline::POINTLIST..=pipeline::TRIANGLEFAN).contains(&kind) {
        Ok(())
    } else {
        Err(D3DERR_INVALIDPRIMITIVETYPE)
    }
}

/// `count` vertices of `layout` at `at`.
pub fn read_vertices(
    cpu: &mut Cpu,
    at: u32,
    layout: &Layout,
    count: u32,
) -> Result<Vec<Vertex>, u32> {
    let len = (layout.size as u64) * count as u64;
    if len > 0x100_0000 || count > 0 && !probe(cpu, at, len as u32, false) {
        return Err(DDERR_INVALIDPARAMS);
    }
    let bytes = read_bytes(cpu, at, len as u32);
    Ok(bytes
        .chunks_exact(layout.size.max(1) as usize)
        .map(|v| layout.read(v))
        .collect())
}

/// `count` indices (WORDs) at `at`.
fn read_indices(cpu: &mut Cpu, at: u32, count: u32) -> Result<Vec<u32>, u32> {
    if count > 0x10_0000 || count > 0 && !probe(cpu, at, count * 2, false) {
        return Err(DDERR_INVALIDPARAMS);
    }
    let bytes = read_bytes(cpu, at, count * 2);
    Ok(bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]) as u32)
        .collect())
}

/// A D3DLIGHT7's words as the pipeline has a light.
pub fn light7(w: &[u32; 26]) -> Light {
    let f = |i: usize| f32::from_bits(w[i]);
    let c = |i: usize| [f(i), f(i + 1), f(i + 2), f(i + 3)];
    Light {
        kind: w[0],
        diffuse: c(1),
        specular: c(5),
        ambient: c(9),
        position: [f(13), f(14), f(15)],
        direction: [f(16), f(17), f(18)],
        range: f(19),
        falloff: f(20),
        attenuation: [f(21), f(22), f(23)],
        theta: f(24),
        phi: f(25),
    }
}

/// D3DMATERIAL7's words (or D3DMATERIAL's after its size) as the
/// pipeline has a material.
pub fn material(w: &[u32]) -> Material {
    let f = |i: usize| w.get(i).map_or(0.0, |&v| f32::from_bits(v));
    let c = |i: usize| [f(i), f(i + 1), f(i + 2), f(i + 3)];
    Material {
        diffuse: c(0),
        ambient: c(4),
        specular: c(8),
        emissive: c(12),
        power: f(16),
    }
}

/// The view's mapping of a device of `version`: Direct3D 7's viewport,
/// or the current viewport's for the others (the render target, if it
/// has none). Direct3D 7's viewport is drawn with no more than the
/// part of it on the render target, as Wine has it (its
/// WINED3D_LIMIT_VIEWPORT): a 640x480 viewport on a 128x128 target maps
/// the view onto the 128x128.
fn view_map(cpu: &mut Cpu, d: &Device, version: u32, viewport: u32) -> ViewMap {
    if version < 7
        && let Some(m) = super::viewport::view_map(cpu, viewport)
    {
        return m;
    }
    let v = &d.viewport;
    let (tw, th) = super::super::state(cpu)
        .surface(d.target)
        .map_or((u32::MAX, u32::MAX), |s| (s.width, s.height));
    let limit = |size: u32, at: u32, most: u32| match most.saturating_sub(at) {
        0 => size,
        room => size.min(room),
    };
    let (w, h) = (limit(v.width, v.x, tw), limit(v.height, v.y, th));
    ViewMap::dx7(v.x, v.y, w, h, v.min_z, v.max_z)
}

/// The pipeline's setup for vertices of `layout` drawn by the device
/// `id` through an interface of `version`, with DrawPrimitive's `flags`.
pub fn setup(cpu: &mut Cpu, id: u32, version: u32, layout: &Layout, flags: u32) -> Option<Setup> {
    let viewport = get(cpu, id)?.current.0;
    setup_in(cpu, id, version, layout, flags, viewport)
}

/// The same, with the viewport `viewport` for one of Direct3D 3 to 6
/// (an execute buffer's).
pub fn setup_in(
    cpu: &mut Cpu,
    id: u32,
    version: u32,
    layout: &Layout,
    flags: u32,
    viewport: u32,
) -> Option<Setup> {
    let d = get(cpu, id)?.clone();
    let view_map = view_map(cpu, &d, version, viewport);
    let s = |r: u32| d.state(r);
    let mut planes = Vec::new();
    if version >= 7 {
        for (i, p) in d.planes.iter().enumerate() {
            if s(RS_CLIPPLANEENABLE) & 1 << i != 0 {
                planes.push(*p);
            }
        }
    }
    let lit = !layout.rhw
        && match version {
            7 | 8 => s(RS_LIGHTING) != 0,
            3 => s(RS_LIGHTING) != 0 && layout.normal.is_some(),
            _ => layout.normal.is_some() && flags & D3DDP_DONOTLIGHT == 0,
        };
    let lighting = if lit {
        let (lights, material) = if version >= 7 {
            let lights = d
                .lights
                .iter()
                .filter(|l| l.enabled)
                .map(|l| light7(&l.light))
                .collect();
            (lights, material(&d.material))
        } else {
            let lights = super::light::lights_of(cpu, viewport);
            let m = super::material::of_handle(cpu, &d, d.light_states[1]).unwrap_or_default();
            (lights, m)
        };
        let a = pipeline::rgba(s(RS_AMBIENT));
        Some(Lighting {
            lights,
            material,
            ambient: [a[0], a[1], a[2], 0.0],
            specular: s(RS_SPECULARENABLE) != 0,
            local_viewer: s(RS_LOCALVIEWER) != 0,
            normalize: s(RS_NORMALIZENORMALS) != 0,
            color_vertex: s(RS_COLORVERTEX) != 0,
            sources: [
                s(RS_DIFFUSEMATERIALSOURCE),
                s(RS_SPECULARMATERIALSOURCE),
                s(RS_AMBIENTMATERIALSOURCE),
                s(RS_EMISSIVEMATERIALSOURCE),
            ],
        })
    } else {
        None
    };
    let fog = (!layout.rhw
        && s(RS_FOGENABLE) != 0
        && s(RS_FOGVERTEXMODE) != pipeline::FOG_NONE
        && s(RS_FOGTABLEMODE) == pipeline::FOG_NONE)
        .then(|| Fog {
            mode: s(RS_FOGVERTEXMODE),
            start: d.state_f(RS_FOGSTART),
            end: d.state_f(RS_FOGEND),
            density: d.state_f(RS_FOGDENSITY),
            range: s(48) != 0,
        });
    let tex = |stage: u32| (d.stage(stage, TSS_TEXCOORDINDEX) & 7) as usize;
    Some(Setup {
        world: d.transforms[1],
        view: d.transforms[2],
        projection: d.transforms[3],
        view_map,
        planes,
        lighting,
        fog,
        clip: s(RS_CLIPPING) != 0 && flags & D3DDP_DONOTCLIP == 0,
        guard_band: GUARD_BAND,
        cull: s(RS_CULLMODE),
        fill: s(RS_FILLMODE),
        shade: s(RS_SHADEMODE),
        tex_index: [tex(0), tex(1)],
    })
}

/// Count what was drawn, and the vertices' clip codes and extents, in
/// the device's statistics and clip status.
pub fn account(
    cpu: &mut Cpu,
    id: u32,
    setup: &Setup,
    verts: &[Processed],
    drawn: pipeline::Drawn,
    flags: u32,
) {
    let (intersection, union) = pipeline::clip_union(verts);
    let extents = if flags & D3DDP_DONOTUPDATEEXTENTS == 0 {
        pipeline::extents(setup, verts)
    } else {
        None
    };
    let Some(d) = get(cpu, id) else {
        return;
    };
    d.stats[0] = d.stats[0].wrapping_add(drawn.triangles);
    d.stats[1] = d.stats[1].wrapping_add(drawn.lines);
    d.stats[2] = d.stats[2].wrapping_add(drawn.points);
    d.stats[4] = d.stats[4].wrapping_add(verts.len() as u32);
    let cs = &mut d.clip_status;
    // D3DCLIPSTATUS_STATUS, and the union and intersection of the codes.
    if cs[0] & 1 != 0 {
        cs[1] |= union | intersection << 12;
    }
    // D3DCLIPSTATUS_EXTENTS2: the rectangle grows to take them in.
    if let (true, Some(e)) = (cs[0] & 2 != 0, extents) {
        let f = |w: u32| f32::from_bits(w);
        cs[2] = f(cs[2]).min(e[0]).to_bits();
        cs[3] = f(cs[3]).max(e[1]).to_bits();
        cs[4] = f(cs[4]).min(e[2]).to_bits();
        cs[5] = f(cs[5]).max(e[3]).to_bits();
    }
}

/// Draw `kind` primitives of `vertices` (in the order `order`, or as
/// they are) with the device `id`.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    cpu: &mut Cpu,
    id: u32,
    version: u32,
    layout: &Layout,
    kind: u32,
    vertices: &[Vertex],
    order: Option<&[u32]>,
    flags: u32,
) -> Result<(), u32> {
    check_kind(kind)?;
    let Some(setup) = setup(cpu, id, version, layout, flags) else {
        return Err(DDERR_INVALIDPARAMS);
    };
    let processed = pipeline::process(&setup, vertices, layout);
    let all: Vec<u32>;
    let order = match order {
        Some(o) => o,
        None => {
            all = (0..vertices.len() as u32).collect();
            &all
        }
    };
    let prims = pipeline::assemble(kind, order).ok_or(D3DERR_INVALIDPRIMITIVETYPE)?;
    let drawn = rasterize(cpu, id, &setup, &processed, &prims);
    account(cpu, id, &setup, &processed, drawn, flags);
    Ok(())
}

/// Draw `prims` of `verts` on the card, if the device can draw there,
/// or only count them.
fn rasterize(
    cpu: &mut Cpu,
    id: u32,
    setup: &Setup,
    verts: &[Processed],
    prims: &[Prim],
) -> pipeline::Drawn {
    let card = get(cpu, id)
        .cloned()
        .and_then(|d| super::hal::bind(cpu, &d, setup));
    match card {
        Some(card) => {
            let mut raster = super::hal::Raster::new(cpu, card);
            let drawn = pipeline::draw(setup, verts, prims, &mut raster);
            raster.finish();
            // The target's pixels changed, for a texture made of it.
            if let Some(target) = get(cpu, id).map(|d| d.target) {
                super::super::surface::changed(cpu, target);
            }
            drawn
        }
        None => pipeline::draw(setup, verts, prims, &mut pipeline::Counter::default()),
    }
}

/// Draw primitives of vertices already through the pipeline (an execute
/// buffer's, or a vertex buffer's after ProcessVertices).
pub fn draw_processed(
    cpu: &mut Cpu,
    id: u32,
    setup: &Setup,
    verts: &[Processed],
    prims: &[Prim],
) -> pipeline::Drawn {
    let drawn = rasterize(cpu, id, setup, verts, prims);
    if let Some(d) = get(cpu, id) {
        d.stats[0] = d.stats[0].wrapping_add(drawn.triangles);
        d.stats[1] = d.stats[1].wrapping_add(drawn.lines);
        d.stats[2] = d.stats[2].wrapping_add(drawn.points);
    }
    drawn
}

fn in_begin(cpu: &mut Cpu, id: u32) -> bool {
    get(cpu, id).is_some_and(|d| d.begin.is_some())
}

/// DrawPrimitive(this, type, vertex type, vertices, count, flags).
pub fn draw_primitive(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, kind, vt, at, count, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    if in_begin(cpu, id) {
        return Some(D3DERR_INBEGIN);
    }
    let layout = hr!(vertex_format(vtable, vt));
    let vertices = hr!(read_vertices(cpu, at, &layout, count));
    hr!(draw(
        cpu,
        id,
        version(vtable),
        &layout,
        kind,
        &vertices,
        None,
        flags
    ));
    Some(D3D_OK)
}

/// DrawIndexedPrimitive(this, type, vertex type, vertices, count,
/// indices, index count, flags).
pub fn draw_indexed_primitive(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, kind, vt, at, count, indices, icount, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    if in_begin(cpu, id) {
        return Some(D3DERR_INBEGIN);
    }
    let layout = hr!(vertex_format(vtable, vt));
    let vertices = hr!(read_vertices(cpu, at, &layout, count));
    let order = hr!(read_indices(cpu, indices, icount));
    hr!(draw(
        cpu,
        id,
        version(vtable),
        &layout,
        kind,
        &vertices,
        Some(&order),
        flags
    ));
    Some(D3D_OK)
}

/// The vertices of D3DDRAWPRIMITIVESTRIDEDDATA at `at`: each part of a
/// vertex from its own array, `count` of them.
pub fn read_strided_vertices(
    cpu: &mut Cpu,
    at: u32,
    layout: &Layout,
    count: u32,
) -> Result<Vec<Vertex>, u32> {
    if !probe(cpu, at, 96, false) || count > 0x10_0000 {
        return Err(DDERR_INVALIDPARAMS);
    }
    let words = read_dwords::<24>(cpu, at);
    let part = |i: usize| (words[2 * i], words[2 * i + 1]);
    let read =
        |cpu: &mut Cpu, (ptr, stride): (u32, u32), i: u32, len: u32| -> Result<Vec<u8>, u32> {
            let at = ptr.wrapping_add(stride.wrapping_mul(i));
            if !probe(cpu, at, len, false) {
                return Err(DDERR_INVALIDPARAMS);
            }
            Ok(read_bytes(cpu, at, len))
        };
    let f = |b: &[u8], i: usize| {
        f32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]])
    };
    let d = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let mut out = Vec::with_capacity(count.min(0x1_0000) as usize);
    for i in 0..count {
        let n = if layout.rhw { 4 } else { 3 };
        let p = read(cpu, part(0), i, 4 * n)?;
        let mut v = Vertex {
            position: [
                f(&p, 0),
                f(&p, 1),
                f(&p, 2),
                if layout.rhw { f(&p, 3) } else { 1.0 },
            ],
            ..Vertex::default()
        };
        if layout.normal.is_some() {
            let b = read(cpu, part(1), i, 12)?;
            v.normal = [f(&b, 0), f(&b, 1), f(&b, 2)];
        }
        if layout.diffuse.is_some() {
            v.diffuse = Some(d(&read(cpu, part(2), i, 4)?));
        }
        if layout.specular.is_some() {
            v.specular = Some(d(&read(cpu, part(3), i, 4)?));
        }
        for t in 0..layout.textures {
            let dims = layout.tex[t].1;
            let b = read(cpu, part(4 + t), i, 4 * dims)?;
            for k in 0..dims as usize {
                v.tex[t][k] = f(&b, k);
            }
        }
        out.push(v);
    }
    Ok(out)
}

/// DrawPrimitiveStrided(this, type, fvf, data, count, flags).
pub fn draw_primitive_strided(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, kind, fvf, at, count, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    let layout = hr!(vertex_format(vtable, fvf));
    let vertices = hr!(read_strided_vertices(cpu, at, &layout, count));
    hr!(draw(
        cpu,
        id,
        version(vtable),
        &layout,
        kind,
        &vertices,
        None,
        flags
    ));
    Some(D3D_OK)
}

/// DrawIndexedPrimitiveStrided(this, type, fvf, data, count, indices,
/// index count, flags).
pub fn draw_indexed_primitive_strided(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, kind, fvf, at, count, indices, icount, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    let layout = hr!(vertex_format(vtable, fvf));
    let vertices = hr!(read_strided_vertices(cpu, at, &layout, count));
    let order = hr!(read_indices(cpu, indices, icount));
    hr!(draw(
        cpu,
        id,
        version(vtable),
        &layout,
        kind,
        &vertices,
        Some(&order),
        flags
    ));
    Some(D3D_OK)
}

/// The vertices `start` to `start + count` of the vertex buffer `vb`,
/// and its layout.
fn buffer_vertices(
    cpu: &mut Cpu,
    vb: u32,
    start: u32,
    count: u32,
) -> Result<(Layout, Vec<Vertex>), u32> {
    let (_, bid) = super::vertexbuffer::buffer_of(cpu, vb)?;
    let b = super::object(cpu, bid)
        .and_then(|o| o.buffer.clone())
        .ok_or(DDERR_INVALIDPARAMS)?;
    if b.locked {
        return Err(D3DERR_VERTEXBUFFERLOCKED);
    }
    let layout = Layout::of(b.fvf).ok_or(DDERR_INVALIDPARAMS)?;
    if start.checked_add(count).is_none_or(|end| end > b.count) {
        return Err(DDERR_INVALIDPARAMS);
    }
    let at = b.memory + start * layout.size;
    Ok((layout, read_vertices(cpu, at, &layout, count)?))
}

/// DrawPrimitiveVB(this, type, buffer, start, count, flags).
pub fn draw_primitive_vb(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, kind, vb, start, count, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    let (layout, vertices) = hr!(buffer_vertices(cpu, vb, start, count));
    hr!(draw(
        cpu,
        id,
        version(vtable),
        &layout,
        kind,
        &vertices,
        None,
        flags
    ));
    Some(D3D_OK)
}

/// DrawIndexedPrimitiveVB(this, type, buffer, indices, index count,
/// flags) of IDirect3DDevice3: indices into the whole buffer.
pub fn draw_indexed_primitive_vb3(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, kind, vb, indices, icount, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    let (_, bid) = hr!(super::vertexbuffer::buffer_of(cpu, vb));
    let count = super::object(cpu, bid)
        .and_then(|o| o.buffer.as_ref().map(|b| b.count))
        .unwrap_or(0);
    let (layout, vertices) = hr!(buffer_vertices(cpu, vb, 0, count));
    let order = hr!(read_indices(cpu, indices, icount));
    hr!(draw(
        cpu,
        id,
        version(vtable),
        &layout,
        kind,
        &vertices,
        Some(&order),
        flags
    ));
    Some(D3D_OK)
}

/// DrawIndexedPrimitiveVB(this, type, buffer, start, count, indices,
/// index count, flags) of IDirect3DDevice7: indices from `start` on.
pub fn draw_indexed_primitive_vb7(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, kind, vb, start, count, indices, icount, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    let (layout, vertices) = hr!(buffer_vertices(cpu, vb, start, count));
    let order = hr!(read_indices(cpu, indices, icount));
    hr!(draw(
        cpu,
        id,
        version(vtable),
        &layout,
        kind,
        &vertices,
        Some(&order),
        flags
    ));
    Some(D3D_OK)
}

/// Begin(this, type, vertex type, flags).
pub fn begin(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, kind, vt, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    start_begin(cpu, this, kind, vt, flags, None)
}

/// BeginIndexed(this, type, vertex type, vertices, count, flags).
pub fn begin_indexed(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, kind, vt, at, count, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    start_begin(cpu, this, kind, vt, flags, Some((at, count)))
}

fn start_begin(
    cpu: &mut Cpu,
    this: u32,
    kind: u32,
    vt: u32,
    flags: u32,
    array: Option<(u32, u32)>,
) -> Option<u32> {
    let (vtable, id) = hr!(device_of(cpu, this));
    if in_begin(cpu, id) {
        return Some(D3DERR_INBEGIN);
    }
    hr!(check_kind(kind));
    let layout = hr!(vertex_format(vtable, vt));
    if let Some((at, count)) = array {
        let len = layout.size.saturating_mul(count);
        if count > 0 && !probe(cpu, at, len, false) {
            return Some(DDERR_INVALIDPARAMS);
        }
    }
    let (array, count) = array.unwrap_or((0, 0));
    if let Some(d) = get(cpu, id) {
        d.begin = Some(Begin {
            kind,
            fvf: layout.fvf,
            flags,
            indexed: array != 0,
            array,
            count,
            ..Begin::default()
        });
    }
    Some(D3D_OK)
}

/// Vertex(this, vertex): one more vertex of Begin's primitive.
pub fn vertex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    let Some(b) = get(cpu, id).and_then(|d| d.begin.clone()) else {
        return Some(D3DERR_NOTINBEGIN);
    };
    let size = Layout::of(b.fvf).map_or(0, |l| l.size);
    if !probe(cpu, at, size, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let bytes = read_bytes(cpu, at, size);
    if let Some(b) = get(cpu, id).and_then(|d| d.begin.as_mut()) {
        b.vertices.extend_from_slice(&bytes);
    }
    Some(D3D_OK)
}

/// Index(this, index): one more index of BeginIndexed's primitive.
pub fn index(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, i] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    let Some(b) = get(cpu, id).and_then(|d| d.begin.as_mut()) else {
        return Some(D3DERR_NOTINBEGIN);
    };
    b.indices.push(i & 0xFFFF);
    Some(D3D_OK)
}

/// End(this, flags): draw Begin's primitive.
pub fn end(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    let Some(b) = get(cpu, id).and_then(|d| d.begin.take()) else {
        return Some(D3DERR_NOTINBEGIN);
    };
    let Some(layout) = Layout::of(b.fvf) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vertices, order) = if b.indexed {
        let v = hr!(read_vertices(cpu, b.array, &layout, b.count));
        (v, Some(b.indices))
    } else {
        let v: Vec<Vertex> = b
            .vertices
            .chunks_exact(layout.size as usize)
            .map(|v| layout.read(v))
            .collect();
        (v, None)
    };
    if vertices.len() < pipeline::min_vertices(b.kind) && !b.indexed {
        return Some(D3D_OK);
    }
    hr!(draw(
        cpu,
        id,
        version(vtable),
        &layout,
        b.kind,
        &vertices,
        order.as_deref(),
        b.flags
    ));
    Some(D3D_OK)
}

/// D3DSTATUS_CLIPUNIONLEFT and D3DSTATUS_CLIPINTERSECTIONLEFT, shifted
/// for the other planes.
const UNION_LEFT: u32 = 0x1;
const INTERSECTION_LEFT: u32 = 0x1000;

/// Where a sphere is to a plane (normal, distance from the origin): in
/// part outside, all outside or inside (ported from Wine's in_plane).
fn in_plane(plane: u32, normal: [f32; 3], origin: f32, center: [f32; 3], radius: f32) -> u32 {
    let norm = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
    let distance =
        (origin + normal[0] * center[0] + normal[1] * center[1] + normal[2] * center[2]) / norm;
    if distance.abs() < radius {
        UNION_LEFT << plane
    } else if distance < -radius {
        (UNION_LEFT | INTERSECTION_LEFT) << plane
    } else {
        0
    }
}

/// ComputeSphereVisibility(this, centers, radii, count, flags, out):
/// for each sphere, the planes of the view volume it is in part and all
/// outside of.
pub fn compute_sphere_visibility(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, centers, radii, count, _, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if count > 0x10_0000
        || count > 0
            && (!probe(cpu, centers, count * 12, false)
                || !probe(cpu, radii, count * 4, false)
                || !probe(cpu, out, count * 4, true))
    {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(d) = get(cpu, id).map(|d| d.transforms.clone()) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let m = pipeline::multiply(&pipeline::multiply(&d[1], &d[2]), &d[3]);
    // m[4 * (row - 1) + (column - 1)] is _row column.
    let e = |r: usize, c: usize| m[4 * (r - 1) + (c - 1)];
    let col = |c: usize| [e(1, c), e(2, c), e(3, c)];
    let add = |a: [f32; 3], b: [f32; 3]| [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let planes = [
        (add(col(4), col(1)), e(4, 4) + e(4, 1)),
        (sub(col(4), col(1)), e(4, 4) - e(4, 1)),
        (sub(col(4), col(2)), e(4, 4) - e(4, 2)),
        (add(col(4), col(2)), e(4, 4) + e(4, 2)),
        (col(3), e(4, 3)),
        (sub(col(4), col(3)), e(4, 4) - e(4, 3)),
    ];
    for i in 0..count {
        let c = read_dwords::<3>(cpu, centers + 12 * i).map(f32::from_bits);
        let r = super::read_f32(cpu, radii + 4 * i);
        let mut result = 0;
        for (j, (normal, origin)) in planes.iter().enumerate() {
            result |= in_plane(j as u32, *normal, *origin, c, r);
        }
        cpu.bus.guest_write_32(out + 4 * i, result);
    }
    Some(D3D_OK)
}

/// The device of the `version` whose state ProcessVertices uses.
pub fn device_setup(
    cpu: &mut Cpu,
    device_iface: u32,
    layout: &Layout,
    lit: bool,
) -> Result<(u32, Setup), u32> {
    let (vtable, id) = device_of(cpu, device_iface).map_err(|_| DDERR_INVALIDPARAMS)?;
    let flags = if lit { 0 } else { D3DDP_DONOTLIGHT };
    let mut setup = setup(cpu, id, version(vtable), layout, flags).ok_or(DDERR_INVALIDPARAMS)?;
    if !lit {
        setup.lighting = None;
    }
    Ok((id, setup))
}
