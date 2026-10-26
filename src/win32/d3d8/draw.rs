//! Drawing: DrawPrimitive, DrawIndexedPrimitive and their UP forms
//! (vertices and indices in the program's memory), and
//! ProcessVertices. The vertices come from the streams as the vertex
//! shader says: an FVF code's from stream 0, a declaration's registers
//! from the streams it names. They go through Direct3D 7's pipeline
//! (`ddraw::d3d::draw`) on the inner device, or through the shader's
//! function (`shader.rs`) and then the pipeline's clipping and the HAL.
//! Patches the card can't draw.

use super::super::ddraw::d3d::draw as inner_draw;
use super::super::ddraw::d3d::pipeline::{self, Layout, Processed, Vertex};
use super::device::{Device, get, this_device};
use super::shader::{
    self, Decl, REG_DIFFUSE, REG_NORMAL, REG_POSITION, REG_SPECULAR, REG_TEXCOORD0,
};
use super::states::index_size;
use super::{D3D_OK, D3DERR_INVALIDCALL, state};
use crate::cpu::Cpu;

/// The vertices `count` primitives of `kind` take.
fn vertex_count(kind: u32, count: u32) -> Option<u32> {
    Some(match kind {
        pipeline::POINTLIST => count,
        pipeline::LINELIST => count.checked_mul(2)?,
        pipeline::LINESTRIP => count.checked_add(1)?,
        pipeline::TRIANGLELIST => count.checked_mul(3)?,
        pipeline::TRIANGLESTRIP | pipeline::TRIANGLEFAN => count.checked_add(2)?,
        _ => return None,
    })
}

/// A stream's vertices: where they start in memory and their stride.
#[derive(Clone, Copy, Debug, Default)]
struct Source {
    at: u32,
    stride: u32,
}

/// The streams of the device `d`, as its vertex buffers have them.
fn sources(cpu: &mut Cpu, d: &Device) -> Vec<Option<Source>> {
    d.streams
        .iter()
        .map(|&(vb, stride)| {
            let b = state(cpu).object(vb)?.buffer.clone()?;
            Some(Source {
                at: b.memory,
                stride,
            })
        })
        .collect()
}

/// The bytes of `count` vertices from vertex `first` of a stream, `len`
/// bytes of each read.
fn stream_bytes(cpu: &mut Cpu, s: Source, first: u32, count: u32, len: u32) -> Option<Vec<u8>> {
    if count == 0 {
        return Some(Vec::new());
    }
    let start = s.at as u64 + first as u64 * s.stride as u64;
    let span = (count as u64 - 1) * s.stride as u64 + len as u64;
    if span > 0x400_0000 || start + span > u32::MAX as u64 {
        return None;
    }
    if !cpu.bus.guest_probe(start as u32, span as usize, false) {
        return None;
    }
    let mut out = vec![0; span as usize];
    cpu.bus.guest_read_bytes(start as u32, &mut out);
    Some(out)
}

/// The vertex `k` of a stream's bytes.
fn vertex_bytes(bytes: &[u8], stride: u32, k: u32, len: u32) -> &[u8] {
    let at = (k * stride) as usize;
    bytes.get(at..at + len as usize).unwrap_or(&[])
}

/// How the device's vertices are read and processed now.
enum Shading {
    Fixed(Layout),
    /// A declaration's registers, drawn by the fixed-function pipeline,
    /// or run through a function.
    Decl(Decl, Vec<u32>),
}

fn shading(d: &Device) -> Option<Shading> {
    let h = d.vertex_shader;
    if shader::is_handle(h) {
        let s = shader::get_shader(d, h)?;
        let decl = shader::parse_decl(&s.decl)?;
        return Some(Shading::Decl(decl, s.function.clone()));
    }
    Some(Shading::Fixed(Layout::of(h)?))
}

/// The registers of `count` vertices from `first`, as a declaration
/// loads them.
fn registers(
    cpu: &mut Cpu,
    decl: &Decl,
    sources: &[Option<Source>],
    first: u32,
    count: u32,
) -> Option<Vec<[[f32; 4]; 16]>> {
    let mut out = vec![[[0.0, 0.0, 0.0, 1.0]; 16]; count as usize];
    let streams: Vec<u32> = {
        let mut s: Vec<u32> = decl.inputs.iter().map(|i| i.stream).collect();
        s.sort();
        s.dedup();
        s
    };
    for stream in streams {
        let src = (*sources.get(stream as usize)?)?;
        let inputs: Vec<_> = decl.inputs.iter().filter(|i| i.stream == stream).collect();
        let len = inputs
            .iter()
            .map(|i| i.offset + shader::type_size(i.kind).unwrap_or(4))
            .max()
            .unwrap_or(4);
        let bytes = stream_bytes(cpu, src, first, count, len)?;
        for (k, regs) in out.iter_mut().enumerate() {
            let v = vertex_bytes(&bytes, src.stride, k as u32, len);
            for i in &inputs {
                let at = i.offset as usize;
                regs[i.reg as usize] = shader::fetch(i.kind, v.get(at..).unwrap_or(&[]));
            }
        }
    }
    Some(out)
}

/// A fixed-function declaration's registers as an FVF vertex, and the
/// layout that has its parts.
fn decl_vertices(decl: &Decl, regs: &[[[f32; 4]; 16]]) -> (Layout, Vec<Vertex>) {
    let has = |r: u32| decl.inputs.iter().any(|i| i.reg == r);
    let layout = Layout {
        normal: has(REG_NORMAL).then_some(0),
        diffuse: has(REG_DIFFUSE).then_some(0),
        specular: has(REG_SPECULAR).then_some(0),
        ..Layout::default()
    };
    let vertices = regs
        .iter()
        .map(|r| {
            let p = r[REG_POSITION as usize];
            let n = r[REG_NORMAL as usize];
            let mut v = Vertex {
                position: [p[0], p[1], p[2], 1.0],
                normal: [n[0], n[1], n[2]],
                diffuse: layout
                    .diffuse
                    .map(|_| pipeline::color(r[REG_DIFFUSE as usize])),
                specular: layout
                    .specular
                    .map(|_| pipeline::color(r[REG_SPECULAR as usize])),
                ..Vertex::default()
            };
            for t in 0..8 {
                v.tex[t] = r[REG_TEXCOORD0 as usize + t];
            }
            v
        })
        .collect();
    (layout, vertices)
}

/// The most vertices one draw takes.
const MAX_VERTICES: u32 = 0x10_0000;

/// Draw `kind` primitives of the `count` vertices from `first` of the
/// streams `sources`, in the order `order` (places among them) or as
/// they are.
fn draw_vertices(
    cpu: &mut Cpu,
    id: u32,
    kind: u32,
    sources: &[Option<Source>],
    first: u32,
    count: u32,
    order: Option<&[u32]>,
) -> u32 {
    // As many as D3D7's draws take: each is held on the host, and a
    // stride of 0 lets a small buffer stand for any count.
    if count > MAX_VERTICES {
        return D3DERR_INVALIDCALL;
    }
    let Some(d) = get(cpu, id).cloned() else {
        return D3DERR_INVALIDCALL;
    };
    let Some(shading) = shading(&d) else {
        return D3DERR_INVALIDCALL;
    };
    match shading {
        Shading::Fixed(layout) => {
            let Some(src) = sources.first().copied().flatten() else {
                return D3DERR_INVALIDCALL;
            };
            let len = layout.size.min(src.stride.max(1));
            let Some(bytes) = stream_bytes(cpu, src, first, count, len) else {
                return D3DERR_INVALIDCALL;
            };
            let vertices: Vec<Vertex> = (0..count)
                .map(|k| layout.read(vertex_bytes(&bytes, src.stride, k, len)))
                .collect();
            let _ = inner_draw::draw(cpu, d.inner, 8, &layout, kind, &vertices, order, 0);
        }
        Shading::Decl(decl, function) => {
            let Some(regs) = registers(cpu, &decl, sources, first, count) else {
                return D3DERR_INVALIDCALL;
            };
            if function.is_empty() {
                let (layout, vertices) = decl_vertices(&decl, &regs);
                let _ = inner_draw::draw(cpu, d.inner, 8, &layout, kind, &vertices, order, 0);
            } else {
                run_function(cpu, &d, kind, &function, &regs, order);
            }
        }
    }
    D3D_OK
}

/// Run the vertices' registers through a shader's function and draw
/// what comes out.
fn run_function(
    cpu: &mut Cpu,
    d: &Device,
    kind: u32,
    function: &[u32],
    regs: &[[[f32; 4]; 16]],
    order: Option<&[u32]>,
) {
    let layout = Layout::default();
    let Some(setup) = inner_draw::setup(cpu, d.inner, 8, &layout, 0) else {
        return;
    };
    let constants: Vec<[f32; 4]> = d.constants.iter().map(|c| c.map(f32::from_bits)).collect();
    let clamp = |c: [f32; 4]| c.map(|x| x.clamp(0.0, 1.0));
    let verts: Vec<Processed> = regs
        .iter()
        .map(|r| {
            let o = shader::run(function, r, &constants);
            let mut specular = clamp(o.colors[1]);
            specular[3] = o.fog.clamp(0.0, 1.0);
            let mut p = Processed {
                clip: o.position,
                diffuse: clamp(o.colors[0]),
                specular,
                tex: o.tex,
                ..Processed::default()
            };
            p.code = pipeline::codes(&setup, &p);
            p
        })
        .collect();
    let all: Vec<u32>;
    let order = match order {
        Some(o) => o,
        None => {
            all = (0..regs.len() as u32).collect();
            &all
        }
    };
    if let Some(prims) = pipeline::assemble(kind, order) {
        inner_draw::draw_processed(cpu, d.inner, &setup, &verts, &prims);
    }
}

/// DrawPrimitive(this, type, start vertex, primitives).
pub fn draw_primitive(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, kind, start, prims] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    let Some(count) = vertex_count(kind, prims) else {
        return Some(D3DERR_INVALIDCALL);
    };
    if prims == 0 {
        return Some(D3D_OK);
    }
    let sources = sources(cpu, &d);
    Some(draw_vertices(cpu, id, kind, &sources, start, count, None))
}

/// `count` indices of `size` bytes at `at`, each with `base` added.
fn read_indices(cpu: &mut Cpu, at: u32, size: u32, count: u32, base: u32) -> Option<Vec<u32>> {
    let len = count as u64 * size as u64;
    if len > 0x400_0000 || !cpu.bus.guest_probe(at, len as usize, false) {
        return None;
    }
    let mut bytes = vec![0; len as usize];
    cpu.bus.guest_read_bytes(at, &mut bytes);
    Some(
        bytes
            .chunks_exact(size as usize)
            .map(|b| {
                let i = if size == 4 {
                    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
                } else {
                    u16::from_le_bytes([b[0], b[1]]) as u32
                };
                i.wrapping_add(base)
            })
            .collect(),
    )
}

/// Draw `kind` primitives of the vertices `indices` (in the streams).
fn draw_indexed(
    cpu: &mut Cpu,
    id: u32,
    kind: u32,
    sources: &[Option<Source>],
    indices: &[u32],
) -> u32 {
    let (Some(&lo), Some(&hi)) = (indices.iter().min(), indices.iter().max()) else {
        return D3D_OK;
    };
    if hi - lo > 0x10_0000 {
        return D3DERR_INVALIDCALL;
    }
    let order: Vec<u32> = indices.iter().map(|&i| i - lo).collect();
    draw_vertices(cpu, id, kind, sources, lo, hi - lo + 1, Some(&order))
}

/// DrawIndexedPrimitive(this, type, min index, vertices, start index,
/// primitives): the vertices are the ones the indices name.
pub fn draw_indexed_primitive(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, kind, _, _, start, prims] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    let Some(count) = vertex_count(kind, prims) else {
        return Some(D3DERR_INVALIDCALL);
    };
    if prims == 0 {
        return Some(D3D_OK);
    }
    let (ib, base) = d.indices;
    let Some(b) = state(cpu).object(ib).and_then(|o| o.buffer.clone()) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let size = index_size(b.format);
    let end = (start as u64 + count as u64) * size as u64;
    if end > b.size as u64 {
        return Some(D3DERR_INVALIDCALL);
    }
    let Some(indices) = read_indices(cpu, b.memory + start * size, size, count, base) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let sources = sources(cpu, &d);
    Some(draw_indexed(cpu, id, kind, &sources, &indices))
}

/// Stream 0 (and the index buffer) as the UP methods leave them: none.
fn reset_stream(cpu: &mut Cpu, id: u32, indices: bool) {
    let Some(d) = get(cpu, id) else {
        return;
    };
    let old = std::mem::take(&mut d.streams[0]).0;
    let old_ib = if indices {
        std::mem::take(&mut d.indices).0
    } else {
        0
    };
    for b in [old, old_ib] {
        if b != 0 {
            super::let_go(cpu, b);
        }
    }
}

/// DrawPrimitiveUP(this, type, primitives, vertices, stride).
pub fn draw_primitive_up(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, kind, prims, at, stride] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    let Some(count) = vertex_count(kind, prims) else {
        return Some(D3DERR_INVALIDCALL);
    };
    if at == 0 || stride == 0 {
        return Some(D3DERR_INVALIDCALL);
    }
    reset_stream(cpu, id, false);
    if prims == 0 {
        return Some(D3D_OK);
    }
    let sources = [Some(Source { at, stride })];
    Some(draw_vertices(cpu, id, kind, &sources, 0, count, None))
}

/// DrawIndexedPrimitiveUP(this, type, min index, vertices, primitives,
/// indices, index format, vertices, stride).
pub fn draw_indexed_primitive_up(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, kind, _, _, prims, index_at, format, at, stride] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    let Some(count) = vertex_count(kind, prims) else {
        return Some(D3DERR_INVALIDCALL);
    };
    if at == 0 || stride == 0 || index_at == 0 {
        return Some(D3DERR_INVALIDCALL);
    }
    reset_stream(cpu, id, true);
    if prims == 0 {
        return Some(D3D_OK);
    }
    let size = index_size(format);
    let Some(indices) = read_indices(cpu, index_at, size, count, 0) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let sources = [Some(Source { at, stride })];
    Some(draw_indexed(cpu, id, kind, &sources, &indices))
}

/// ProcessVertices(this, source start, dest index, count, dest buffer,
/// flags): the vertices of stream 0 transformed and lit as an FVF code
/// says, written into a buffer of transformed vertices.
pub fn process_vertices(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, start, dest_index, count, dest, flags] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    let Some(Shading::Fixed(layout)) = shading(&d) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let Ok((_, out)) = super::resource::buffer_of(cpu, dest) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let Some(out_layout) = Layout::of(out.format).filter(|l| l.rhw) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let end = (dest_index as u64 + count as u64) * out_layout.size as u64;
    if out.index || end > out.size as u64 {
        return Some(D3DERR_INVALIDCALL);
    }
    let sources = sources(cpu, &d);
    let Some(src) = sources.first().copied().flatten() else {
        return Some(D3DERR_INVALIDCALL);
    };
    let len = layout.size.min(src.stride.max(1));
    let Some(bytes) = stream_bytes(cpu, src, start, count, len) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let vertices: Vec<Vertex> = (0..count)
        .map(|k| layout.read(vertex_bytes(&bytes, src.stride, k, len)))
        .collect();
    let Some(setup) = inner_draw::setup(cpu, d.inner, 8, &layout, 0) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let processed = pipeline::process(&setup, &vertices, &layout);
    // D3DPV_DONOTCOPYDATA: only the position.
    let position_only = flags & 1 != 0;
    for (k, p) in processed.iter().enumerate() {
        let s = if p.tl {
            [p.clip[0], p.clip[1], p.clip[2], p.rhw]
        } else {
            setup.view_map.screen(p.clip)
        };
        let at = out.memory + (dest_index + k as u32) * out_layout.size;
        for (i, v) in s.iter().enumerate() {
            cpu.bus.guest_write_32(at + 4 * i as u32, v.to_bits());
        }
        if position_only {
            continue;
        }
        if let Some(o) = out_layout.diffuse {
            cpu.bus.guest_write_32(at + o, pipeline::color(p.diffuse));
        }
        if let Some(o) = out_layout.specular {
            cpu.bus.guest_write_32(at + o, pipeline::color(p.specular));
        }
        for t in 0..out_layout.textures {
            let (o, dims) = out_layout.tex[t];
            for c in 0..dims {
                let v = p.tex[t][c as usize];
                cpu.bus.guest_write_32(at + o + 4 * c, v.to_bits());
            }
        }
    }
    Some(D3D_OK)
}

/// DrawRectPatch(this, handle, segments, info) and DrawTriPatch: the
/// card has no patches.
pub fn draw_patch(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(this_device(cpu, args[0]));
    Some(D3DERR_INVALIDCALL)
}

/// DeletePatch(this, handle).
pub fn delete_patch(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(this_device(cpu, args[0]));
    Some(D3DERR_INVALIDCALL)
}
