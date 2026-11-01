//! Vertex buffers: IDirect3DVertexBuffer and 7. A vertex buffer is an
//! array of vertices of one FVF code in the process's memory, which the
//! program fills between Lock and Unlock and draws with
//! DrawPrimitiveVB. ProcessVertices puts vertices of another buffer
//! through a device's transform and lighting into it, as screen
//! vertices.
//!
//! With D3DVOP_CLIP, into a buffer made without D3DVBCAPS_DONOTCLIP, the
//! buffer also keeps each vertex's clip coordinates, as DirectX 7 keeps
//! them beside a buffer that can be clipped, and drawing it clips its
//! primitives at the view volume as if the vertices had come through
//! the pipeline then. A vertex behind the eye has no screen position a
//! triangle could be clipped from: without them, a triangle through the
//! near plane would be drawn across the screen.

use super::super::{DDERR_INVALIDOBJECT, DDERR_INVALIDPARAMS};
use super::draw::{device_setup, read_vertices};
use super::pipeline::{self, Layout, Vertex};
use super::{D3D_OK, D3DERR_VERTEXBUFFEROPTIMIZED, probe, write_dwords};
use crate::cpu::Cpu;
use crate::win32::{com, kernel32};
use std::sync::Arc;

/// A vertex buffer.
#[derive(Clone, Debug, Default)]
pub struct VertexBuffer {
    /// D3DVERTEXBUFFERDESC's caps, FVF code and number of vertices.
    pub caps: u32,
    pub fvf: u32,
    pub count: u32,
    /// Where its vertices are.
    pub memory: u32,
    pub locked: bool,
    pub optimized: bool,
    /// For each vertex ProcessVertices put here with D3DVOP_CLIP: what it
    /// kept of it. A draw takes them only for a vertex still at the
    /// screen position they map to, so a vertex the program wrote over
    /// since isn't taken for them. Shared, so the copies of the buffer
    /// that Lock and the draws make don't copy them. In save states: a
    /// program may process a buffer once and draw it for as long as its
    /// view stays, or be saved between the two.
    pub clipped: Arc<Vec<Option<Clipped>>>,
}

/// What ProcessVertices keeps of a vertex it clipped.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Clipped {
    /// Its clip coordinates.
    pub clip: [f32; 4],
    /// Its distances from the user's clip planes.
    pub planes: [f32; 6],
}

crate::state_fields!(VertexBuffer {
    caps,
    fvf,
    count,
    memory,
    locked,
    optimized,
    clipped
});

crate::state_fields!(Clipped { clip, planes });

/// The most vertices a buffer has.
const MAX_VERTICES: u32 = 0x1_0000;

/// D3DVBCAPS_OPTIMIZED.
const OPTIMIZED: u32 = 0x8000_0000;

/// D3DVBCAPS_DONOTCLIP: a buffer without room for clip coordinates.
const DONOTCLIP: u32 = 0x1;

/// A new vertex buffer of `count` vertices of the FVF code `fvf`.
pub fn make(cpu: &mut Cpu, caps: u32, fvf: u32, count: u32) -> Result<VertexBuffer, u32> {
    let layout = Layout::of(fvf).ok_or(DDERR_INVALIDPARAMS)?;
    if count == 0 || count > MAX_VERTICES {
        return Err(DDERR_INVALIDPARAMS);
    }
    let memory = kernel32::alloc_pages(cpu, layout.size * count).ok_or(com::E_OUTOFMEMORY)?;
    Ok(VertexBuffer {
        caps,
        fvf,
        count,
        memory,
        ..VertexBuffer::default()
    })
}

/// The vertex buffer `b` goes: its memory with it.
pub fn destroyed(cpu: &mut Cpu, b: VertexBuffer) {
    if b.memory != 0 {
        kernel32::free_pages(cpu, b.memory);
    }
}

/// The vertex buffer of the interface `this`: its vtable and id.
pub fn buffer_of(cpu: &mut Cpu, this: u32) -> Result<(usize, u32), u32> {
    let (vtable, id) = super::of(cpu, this).ok_or(DDERR_INVALIDOBJECT)?;
    get(cpu, id).ok_or(DDERR_INVALIDOBJECT)?;
    Ok((vtable, id))
}

fn get(cpu: &mut Cpu, id: u32) -> Option<&mut VertexBuffer> {
    super::object(cpu, id)?.buffer.as_mut()
}

/// Lock(this, flags, out, size): where its vertices are, and how many
/// bytes they take. The flags don't matter.
pub fn lock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, out, size] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(buffer_of(cpu, this));
    let Some(b) = get(cpu, id).cloned() else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if b.optimized {
        return Some(D3DERR_VERTEXBUFFEROPTIMIZED);
    }
    if !com::write_out(cpu, out, b.memory) {
        return Some(DDERR_INVALIDPARAMS);
    }
    if size != 0 {
        let bytes = Layout::of(b.fvf).map_or(0, |l| l.size) * b.count;
        com::write_out(cpu, size, bytes);
    }
    if let Some(b) = get(cpu, id) {
        b.locked = true;
    }
    Some(D3D_OK)
}

/// Unlock(this).
pub fn unlock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(buffer_of(cpu, args[0]));
    if let Some(b) = get(cpu, id) {
        b.locked = false;
    }
    Some(D3D_OK)
}

/// GetVertexBufferDesc(this, desc): the caps, FVF code and number of
/// vertices, after the size the program put there, which stays.
pub fn get_vertex_buffer_desc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(buffer_of(cpu, this));
    if !probe(cpu, at, 16, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let b = get(cpu, id).cloned().unwrap_or_default();
    write_dwords(cpu, at + 4, &[b.caps, b.fvf, b.count]);
    Some(D3D_OK)
}

/// Optimize(this, device, flags): it can't be locked any more.
pub fn optimize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(buffer_of(cpu, args[0]));
    if let Some(b) = get(cpu, id) {
        b.optimized = true;
        b.caps |= OPTIMIZED;
    }
    Some(D3D_OK)
}

// D3DVOP_ and D3DPV_ flags.
const D3DVOP_LIGHT: u32 = 0x400;
const D3DVOP_TRANSFORM: u32 = 0x1;
const D3DVOP_CLIP: u32 = 0x4;
const D3DVOP_EXTENTS: u32 = 0x8;
const D3DPV_DONOTCOPYDATA: u32 = 0x1;

/// ProcessVertices(this, operations, index, count, source, source
/// index, device, flags).
pub fn process_vertices(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, op, index, count, src, src_index, device, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let Ok((_, sid)) = buffer_of(cpu, src) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let Some(s) = get(cpu, sid).cloned() else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let Some(layout) = Layout::of(s.fvf) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if src_index.checked_add(count).is_none_or(|end| end > s.count) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let at = s.memory + src_index * layout.size;
    let vertices = hr!(read_vertices(cpu, at, &layout, count));
    process(cpu, this, op, index, &layout, &vertices, device, flags)
}

/// ProcessVerticesStrided(this, operations, index, count, data, fvf,
/// device, flags) of IDirect3DVertexBuffer7.
pub fn process_vertices_strided(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, op, index, count, data, fvf, device, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let Some(layout) = Layout::of(fvf) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let vertices = hr!(super::draw::read_strided_vertices(
        cpu, data, &layout, count
    ));
    process(cpu, this, op, index, &layout, &vertices, device, flags)
}

/// Put `vertices` of `layout` through the device's pipeline into the
/// buffer `this` from its vertex `index` on: each gets x, y, z and rhw
/// on the screen (four floats, whatever the buffer's position has room
/// for), and the colors and texture coordinates the buffer has. With
/// D3DVOP_CLIP the buffer keeps their clip coordinates too.
#[allow(clippy::too_many_arguments)]
fn process(
    cpu: &mut Cpu,
    this: u32,
    op: u32,
    index: u32,
    layout: &Layout,
    vertices: &[Vertex],
    device: u32,
    flags: u32,
) -> Option<u32> {
    let (_, id) = hr!(buffer_of(cpu, this));
    let Some(d) = get(cpu, id).cloned() else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let Some(dl) = Layout::of(d.fvf) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if index > d.count || vertices.len() as u32 > d.count - index {
        return Some(DDERR_INVALIDPARAMS);
    }
    let (did, setup) = hr!(device_setup(cpu, device, layout, op & D3DVOP_LIGHT != 0));
    let processed = pipeline::process(&setup, vertices, layout);
    let end = d.memory + d.count * dl.size;
    let clip = op & (D3DVOP_CLIP | D3DVOP_TRANSFORM) == D3DVOP_CLIP | D3DVOP_TRANSFORM
        && d.caps & DONOTCLIP == 0
        && dl.rhw;
    for (i, (p, v)) in processed.iter().zip(vertices).enumerate() {
        let base = d.memory + (index + i as u32) * dl.size;
        let screen = if p.tl || op & D3DVOP_TRANSFORM == 0 {
            [
                p.clip[0],
                p.clip[1],
                p.clip[2],
                if p.tl { p.rhw } else { 1.0 },
            ]
        } else {
            setup.view_map.screen(p.clip)
        };
        let room = (end - base).min(16) / 4;
        write_dwords(cpu, base, &screen.map(f32::to_bits)[..room as usize]);
        let lit = op & D3DVOP_LIGHT != 0 && setup.lighting.is_some();
        let copy = flags & D3DPV_DONOTCOPYDATA == 0;
        if let Some(at) = dl.diffuse
            && (lit || copy)
        {
            let c = if lit {
                pipeline::color(p.diffuse)
            } else {
                v.diffuse.unwrap_or(0xFFFF_FFFF)
            };
            cpu.bus.guest_write_32(base + at, c);
        }
        if let Some(at) = dl.specular
            && (lit || copy)
        {
            let c = if lit {
                pipeline::color(p.specular)
            } else {
                v.specular.unwrap_or(0)
            };
            cpu.bus.guest_write_32(base + at, c);
        }
        if copy {
            for t in 0..dl.textures {
                let (at, dims) = dl.tex[t];
                let words: Vec<u32> = p.tex[t][..dims as usize]
                    .iter()
                    .map(|f| f.to_bits())
                    .collect();
                write_dwords(cpu, base + at, &words);
            }
        }
    }
    // The copy `d` goes first, so the buffer's clip coordinates aren't
    // shared and change in place.
    drop(d);
    if let Some(b) = get(cpu, id) {
        let at = index as usize;
        if clip {
            let kept = Arc::make_mut(&mut b.clipped);
            kept.resize(b.count as usize, None);
            for (k, p) in kept[at..].iter_mut().zip(&processed) {
                *k = (!p.tl).then_some(Clipped {
                    clip: p.clip,
                    planes: p.planes,
                });
            }
        } else if b.clipped.len() >= at + processed.len() {
            Arc::make_mut(&mut b.clipped)[at..at + processed.len()].fill(None);
        }
    }
    let flags = if op & D3DVOP_EXTENTS != 0 {
        0
    } else {
        super::draw::D3DDP_DONOTUPDATEEXTENTS
    };
    super::draw::account(
        cpu,
        did,
        &setup,
        &processed,
        pipeline::Drawn::default(),
        flags,
    );
    Some(D3D_OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::savestate::{Reader, State, Writer};

    #[test]
    fn a_save_state_keeps_the_clip_coordinates() {
        let kept = Clipped {
            clip: [1.0, 2.0, 3.0, -4.0],
            planes: [0.5; 6],
        };
        let buffer = VertexBuffer {
            count: 2,
            clipped: Arc::new(vec![Some(kept), None]),
            ..VertexBuffer::default()
        };
        let mut w = Writer::new();
        buffer.save(&mut w);
        let mut loaded = VertexBuffer::default();
        loaded.load(&mut Reader::new(&w.buf)).unwrap();
        assert_eq!(*loaded.clipped, *buffer.clipped);
    }
}
