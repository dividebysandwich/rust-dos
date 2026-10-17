//! A render: the object list walked a tile at a time, each tile's planes
//! run through the ISP's cells to find the surface each pixel shows, and
//! those surfaces textured and shaded by the TSP into the tile.
//!
//! The ISP follows Imagination's simulator of the PCX2's (`simulat3`,
//! `HWISPRenderer`, `ExpandInstruction`, `SurfProcess`): each pixel has a
//! cell with an "I" register, the object being looked at, and a "U"
//! register, the surface in front so far. A plane's instruction says how
//! it changes them. Depths are 1/w, larger closer; the cells compare them
//! as the floats the planes hold, where the chip has fixed point.
//!
//! Where the simulator skips a translucent pass's planes, this goes on as
//! the chip does: `begin_trans` shades what the cells hold into the tile,
//! clears their surfaces (not their depths), and the pass's objects in
//! front of what was drawn are shaded over it in turn.

use super::Chip;
use super::regs;
use super::tsp::{Shader, Tsp};

/// The tile width: the ISP's cells.
pub const CELLS: usize = 32;

/// The TLB's entries.
const PAGES: usize = 256;

/// Object pointers.
const TILE_HEADER: u32 = 1 << 30;
const LINK: u32 = 1 << 29;
const VERY_LAST: u32 = 1 << 31;

/// Where a render reads its parameters from: the machine's memory, which
/// the card reads by bus mastering.
pub struct Memory<'a> {
    pub ram: &'a [u8],
}

impl Memory<'_> {
    fn read(&self, phys: u32) -> u32 {
        let at = phys as usize;
        self.ram.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// The parameter space, through the TLB.
struct Params<'a> {
    tlb: [u32; PAGES],
    page_shift: u32,
    mem: Memory<'a>,
}

impl<'a> Params<'a> {
    fn new(registers: &[u32], mem: Memory<'a>) -> Self {
        let mut tlb = [0; PAGES];
        for (entry, &raw) in tlb.iter_mut().zip(&registers[regs::TLB..regs::TLB + PAGES]) {
            *entry = page_address(raw);
        }
        // The driver fills the entries with blocks of the page size that
        // follow each other in memory, so the first two give the size.
        // Tomb Raider's are 16 KB, as PAGE_CTRL doesn't tell.
        let page_shift = match tlb[1].wrapping_sub(tlb[0]) {
            0x1000 if registers[regs::TLB + 1] != 0 => 12,
            0x2000 if registers[regs::TLB + 1] != 0 => 13,
            _ => 14,
        };
        Self { tlb, page_shift, mem }
    }

    /// The dword at byte `addr` of the parameter space. The pages need
    /// not be aligned to their size.
    fn dword(&self, addr: u32) -> u32 {
        let page = (addr >> self.page_shift) as usize % PAGES;
        self.mem.read(self.tlb[page].wrapping_add(addr & ((1 << self.page_shift) - 1)))
    }
}

/// A TLB entry's page: its physical address in 4 KB units, or the address
/// itself where it is one (4 KB aligned and at 1 MB or above, which no
/// page number in 4 GB is).
fn page_address(entry: u32) -> u32 {
    if entry & 0xFFF == 0 && entry >= 0x10_0000 { entry } else { entry << 12 }
}

/// Where the object list is: the parameter space when OBJECT_OFFSET's bit
/// 0 is set, as Tomb Raider has it, otherwise the machine's memory. A
/// PCX1's is always in the machine's memory.
struct Objects<'p, 'a> {
    params: &'p Params<'a>,
    physical: bool,
}

impl Objects<'_, '_> {
    fn dword(&self, addr: u32) -> u32 {
        if self.physical { self.params.mem.read(addr) } else { self.params.dword(addr) }
    }
}

/// A plane, as the ISP reads it: depth A·x + B·y + C, its instruction and
/// its TSP tag.
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub instr: u8,
    pub tag: u32,
}

/// A tile of the picture.
pub struct Tile {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// 0x00RRGGBB, a row at a time.
    pub pixels: Vec<u32>,
}

/// A render's result: its tiles, and the clocks the chip's two halves
/// took: the ISP a plane a line of a tile, the TSP a pixel it shades.
#[derive(Default)]
pub struct Rendered {
    pub tiles: Vec<Tile>,
    pub isp_clocks: u64,
    pub tsp_clocks: u64,
}

/// Render the scene the registers describe.
pub fn render(
    chip: Chip,
    registers: &[u32],
    textures: &[u8],
    mem: Memory,
    shader: &Shader,
) -> Rendered {
    let params = Params::new(registers, mem);
    let pcx2 = chip == Chip::Pcx2;
    let physical = !pcx2 || registers[regs::OBJECT_OFFSET] & 1 == 0;
    let objects = Objects { params: &params, physical };
    // The PCX1's planes, and the PCX2's unless IEEEFP asks for floats.
    let format = if pcx2 && registers[regs::IEEEFP] & 1 != 0 {
        PlaneFormat::Float
    } else {
        PlaneFormat::Fixed
    };
    let tsp = Tsp::new(chip, registers, textures, shader);
    // The tiles and their planes first, then the tiles rendered, each by
    // itself, so on as many threads as there are.
    let mut tiles: Vec<(u32, std::ops::Range<usize>)> = Vec::new();
    let mut planes = Vec::new();
    // The list: a tile header, then pointers to the tile's objects; links
    // continue it elsewhere.
    let mut at = registers[regs::OBJECT_OFFSET] & !3;
    let mut word = objects.dword(at);
    let mut steps = 0;
    while steps < 1 << 20 {
        steps += 1;
        if word & TILE_HEADER == 0 {
            break;
        }
        let header = word;
        let first_plane = planes.len();
        let mut last = false;
        loop {
            // A list that never ends (or a runaway) stops the render.
            steps += 1;
            if steps >= 1 << 20 || planes.len() >= 1 << 22 {
                last = true;
                break;
            }
            at = at.wrapping_add(4);
            word = objects.dword(at);
            // A translucent pass's start, which the cells see in its
            // planes. (The PCX1 has neither that nor links.)
            if pcx2 && word & VERY_LAST != 0 && word & LINK != 0 {
                word &= !(VERY_LAST | LINK);
            }
            let mut links = 0;
            while pcx2 && word & LINK != 0 && word & TILE_HEADER == 0 && links < 10 {
                at = (word & 0x00FF_FFFF) << 2;
                word = objects.dword(at);
                links += 1;
            }
            if word & TILE_HEADER != 0 {
                break;
            }
            fetch_planes(&params, format, word, &mut planes);
            if word & VERY_LAST != 0 {
                last = true;
                break;
            }
        }
        tiles.push((header, first_plane..planes.len()));
        if last {
            break;
        }
    }
    let threads = threads().min(tiles.len()).max(1);
    let render_some = |k: usize| -> Vec<(usize, Tile, u64)> {
        (k..tiles.len())
            .step_by(threads)
            .map(|i| {
                let (header, range) = &tiles[i];
                let mut shaded = 0;
                let tile = render_tile(*header, &planes[range.clone()], &tsp, &mut shaded);
                (i, tile, shaded)
            })
            .collect()
    };
    let isp_clocks = tiles.iter().map(|(header, range)| range.len() as u64 * ((header >> 5 & 0x3FF) as u64 + 1)).sum();
    let mut done: Vec<(usize, Tile, u64)> = if threads == 1 {
        render_some(0)
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = (1..threads).map(|k| scope.spawn(move || render_some(k))).collect();
            let mut all = render_some(0);
            for handle in handles {
                all.extend(handle.join().expect("a PowerVR render thread"));
            }
            all
        })
    };
    done.sort_unstable_by_key(|(i, _, _)| *i);
    Rendered {
        isp_clocks,
        tsp_clocks: done.iter().map(|(_, _, shaded)| shaded).sum(),
        tiles: done.into_iter().map(|(_, tile, _)| tile).collect(),
    }
}

/// The threads a render uses.
fn threads() -> usize {
    #[cfg(target_arch = "wasm32")]
    return 1;
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::available_parallelism().map_or(1, |n| n.get().min(8))
}

/// How planes are stored.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PlaneFormat {
    /// Four dwords: A, B and C as IEEE floats, then the instruction in
    /// bits 0-3 and the tag above.
    Float,
    /// Three dwords (`pkisp.c`, `PackPlane`): A as a 20-bit float in bits
    /// 0-19, the tag's bits 17-12 in 20-25 and the instruction in 26-29;
    /// B as a 20-bit float with the tag's bits 11-0 in 20-31; C as an
    /// integer.
    Fixed,
}

/// A 20-bit float (`pto20b.h`): a 15-bit magnitude, its sign in bit 15
/// and a left shift in bits 16-19, as a depth step. The chip adds A and B
/// to C·256 in a 40-bit adder and compares its top 32 bits, the depths
/// here being those as fractions of 2^31.
fn from_20bit(raw: u32) -> f32 {
    let magnitude = ((raw & 0x7FFF) << (raw >> 16 & 15)) as f32 / (1u64 << 39) as f32;
    if raw & 0x8000 != 0 { -magnitude } else { magnitude }
}

/// The planes an object pointer points to.
fn fetch_planes(params: &Params, format: PlaneFormat, pointer: u32, planes: &mut Vec<Plane>) {
    let addr = (pointer & 0x7FFFF) << 2;
    let count = pointer >> 19 & 0x3FF;
    for i in 0..count {
        planes.push(match format {
            PlaneFormat::Float => {
                let at = addr.wrapping_add(16 * i);
                let word = params.dword(at + 12);
                Plane {
                    a: f32::from_bits(params.dword(at)),
                    b: f32::from_bits(params.dword(at + 4)),
                    c: f32::from_bits(params.dword(at + 8)),
                    instr: (word & 15) as u8,
                    tag: word >> 4,
                }
            }
            PlaneFormat::Fixed => {
                let at = addr.wrapping_add(12 * i);
                let (a, b) = (params.dword(at), params.dword(at + 4));
                Plane {
                    a: from_20bit(a),
                    b: from_20bit(b),
                    c: params.dword(at + 8) as i32 as f32 / 2_147_483_648.0,
                    instr: (a >> 26 & 15) as u8,
                    tag: (a >> 8 & 0x3_F000) | b >> 20,
                }
            }
        });
    }
}

// --- The cells ---

mod instr {
    pub const FORW_VISIB: u8 = 0x0;
    pub const FORW_INVIS: u8 = 0x1;
    pub const FORW_PERP: u8 = 0x2;
    pub const TEST_SHAD_FORW: u8 = 0x3;
    pub const TEST_SHAD_PERP: u8 = 0x4;
    pub const REV_VISIB: u8 = 0x5;
    pub const REV_INVIS: u8 = 0x6;
    pub const REV_REPLACE_IF: u8 = 0x7;
    pub const FORW_VISIB_FP: u8 = 0x8;
    pub const FORW_INVIS_FP: u8 = 0x9;
    pub const FORW_PERP_FP: u8 = 0xA;
    pub const TEST_SHAD_FORW_FP: u8 = 0xB;
    pub const TEST_SHAD_PERP_FP: u8 = 0xC;
    pub const TEST_SHADOW_REV: u8 = 0xD;
    pub const TEST_LIGHT_REV: u8 = 0xE;
    pub const BEGIN_TRANS: u8 = 0xF;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadI {
    Nop,
    Load,
    Further,
    Closer,
    InvisForw,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadU {
    Nop,
    Load,
    Closer,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TestShad {
    Nop,
    Closer,
    ShadowFurther,
    LightFurther,
}

/// What an instruction has the cells do (the simulator's `cell_control`).
#[derive(Clone, Copy)]
struct Control {
    i_load: LoadI,
    u_load: LoadU,
    test_shad: TestShad,
    visible: bool,
    clear_u_id: bool,
    perpendicular: bool,
    mux_u: bool,
}

/// What an instruction leaves for the next one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Previous {
    None,
    ShadowForward,
    Shadow,
    Light,
    Translucent,
    ReplaceIf,
}

/// `ExpandInstruction`: an instruction's control, for the first object of
/// a span, the second, or a later one.
fn expand(instr: u8, second: bool, previous: Previous) -> (Control, Previous) {
    use instr::*;
    let c = |i_load, u_load, visible, perpendicular, mux_u| Control {
        i_load,
        u_load,
        test_shad: TestShad::Nop,
        visible,
        clear_u_id: false,
        perpendicular,
        mux_u,
    };
    let first_u = if second { LoadU::Load } else { LoadU::Closer };
    let (mut control, next) = match instr {
        FORW_VISIB => (c(LoadI::Further, LoadU::Nop, true, false, false), Previous::None),
        FORW_VISIB_FP => (c(LoadI::Load, first_u, true, false, true), Previous::None),
        FORW_INVIS => (c(LoadI::Further, LoadU::Nop, false, false, false), Previous::None),
        FORW_INVIS_FP => (c(LoadI::Load, first_u, false, false, true), Previous::None),
        FORW_PERP => (c(LoadI::Further, LoadU::Nop, false, true, false), Previous::None),
        FORW_PERP_FP => (c(LoadI::Load, first_u, true, true, true), Previous::None),
        REV_VISIB => (c(LoadI::Closer, LoadU::Nop, true, false, false), Previous::None),
        REV_INVIS => (c(LoadI::Closer, LoadU::Nop, false, false, false), Previous::None),
        REV_REPLACE_IF => (c(LoadI::Closer, LoadU::Nop, false, false, false), Previous::ReplaceIf),
        TEST_SHAD_FORW => (c(LoadI::Further, LoadU::Nop, true, false, false), Previous::ShadowForward),
        TEST_SHAD_FORW_FP => (c(LoadI::Load, first_u, true, false, true), Previous::ShadowForward),
        TEST_SHAD_PERP => (c(LoadI::Further, LoadU::Nop, false, true, false), Previous::ShadowForward),
        TEST_SHAD_PERP_FP => (c(LoadI::Load, first_u, false, true, true), Previous::ShadowForward),
        TEST_SHADOW_REV => (c(LoadI::Closer, LoadU::Nop, false, false, false), Previous::Shadow),
        TEST_LIGHT_REV => (c(LoadI::Closer, LoadU::Nop, false, false, false), Previous::Light),
        _ => (c(LoadI::Nop, LoadU::Closer, true, false, true), Previous::Translucent),
    };
    match previous {
        Previous::ShadowForward => {
            control.test_shad = TestShad::Closer;
            control.i_load = LoadI::Load;
            control.mux_u = true;
        }
        Previous::Shadow => {
            control.test_shad = TestShad::ShadowFurther;
            control.u_load = LoadU::Nop;
        }
        Previous::Light => {
            control.test_shad = TestShad::LightFurther;
            control.u_load = LoadU::Nop;
        }
        Previous::Translucent => {
            control.clear_u_id = true;
            control.u_load = LoadU::Nop;
        }
        Previous::ReplaceIf => control.i_load = LoadI::InvisForw,
        Previous::None => {}
    }
    (control, next)
}

/// The tile's cells, a pixel each.
struct Cells {
    i_depth: Vec<f32>,
    u_depth: Vec<f32>,
    i_id: Vec<u32>,
    u_id: Vec<u32>,
    i_visible: Vec<bool>,
    i_forward: Vec<bool>,
    shad_temp: Vec<bool>,
    u_visible: Vec<bool>,
    u_shadow: Vec<bool>,
}

impl Cells {
    fn new(n: usize) -> Self {
        Self {
            i_depth: vec![0.0; n],
            u_depth: vec![0.0; n],
            i_id: vec![0; n],
            u_id: vec![0; n],
            i_visible: vec![false; n],
            i_forward: vec![false; n],
            shad_temp: vec![false; n],
            u_visible: vec![false; n],
            u_shadow: vec![false; n],
        }
    }

    /// `SurfProcess` for every cell: the plane with depth `depth(i)` at
    /// cell i, under `control`.
    fn process(&mut self, control: Control, tag: u32, depth: impl Fn(usize) -> f32) {
        // The common cases on their own, for speed: an edge, and an
        // object's first plane.
        if !control.clear_u_id && control.test_shad == TestShad::Nop {
            match (control.i_load, control.u_load) {
                (LoadI::Further, LoadU::Nop) if control.perpendicular => {
                    for i in 0..self.i_depth.len() {
                        let c = depth(i);
                        if c < 0.0 {
                            self.i_depth[i] = c;
                            self.i_id[i] = tag;
                            self.i_visible[i] = control.visible;
                            self.i_forward[i] = true;
                        }
                    }
                    return;
                }
                (LoadI::Load, LoadU::Closer | LoadU::Load) if control.mux_u => {
                    let always = control.u_load == LoadU::Load;
                    for i in 0..self.i_depth.len() {
                        let c = depth(i);
                        let i_visible = self.i_visible[i];
                        if always || (self.i_depth[i] >= self.u_depth[i] && i_visible) || !self.u_visible[i] {
                            self.u_visible[i] = i_visible;
                            self.u_shadow[i] = false;
                            self.shad_temp[i] = false;
                            self.u_depth[i] = self.i_depth[i];
                            self.u_id[i] = self.i_id[i];
                        }
                        self.i_visible[i] = control.visible;
                        self.i_forward[i] = true;
                        self.i_depth[i] = c;
                        self.i_id[i] = tag;
                    }
                    return;
                }
                _ => {}
            }
        }
        for i in 0..self.i_depth.len() {
            let c = depth(i);
            if control.clear_u_id {
                self.u_id[i] = 0;
            }
            let mux = if control.mux_u { self.u_depth[i] } else { c };
            let i_depth = self.i_depth[i];
            let i_gte = i_depth >= mux;
            let i_lte = i_depth <= mux;
            let (i_visible, i_forward) = (self.i_visible[i], self.i_forward[i]);
            let (load_i, new_forward) = match control.i_load {
                LoadI::Nop => (false, i_forward),
                LoadI::Load => (true, true),
                LoadI::Further => {
                    let load = if control.perpendicular { c < 0.0 } else { i_gte };
                    (load, if load { true } else { i_forward })
                }
                LoadI::Closer => (!i_gte, if !i_gte { false } else { i_forward }),
                LoadI::InvisForw => {
                    let load = !i_visible && i_forward;
                    (load, if load { false } else { i_forward })
                }
            };
            let new_i_visible = if load_i { control.visible } else { i_visible };
            let load_u = match control.u_load {
                LoadU::Nop => false,
                LoadU::Load => true,
                LoadU::Closer => (i_gte && i_visible) || !self.u_visible[i],
            };
            if load_u {
                self.u_visible[i] = i_visible;
                self.u_shadow[i] = false;
                self.shad_temp[i] = false;
            } else {
                match control.test_shad {
                    TestShad::Nop => {}
                    TestShad::Closer => self.shad_temp[i] = !i_lte,
                    TestShad::ShadowFurther => self.u_shadow[i] |= i_lte && self.shad_temp[i],
                    TestShad::LightFurther => {
                        self.u_shadow[i] = !((i_lte && self.shad_temp[i]) || !self.u_shadow[i]);
                    }
                }
            }
            if load_u {
                self.u_depth[i] = i_depth;
                self.u_id[i] = self.i_id[i];
            }
            self.i_visible[i] = new_i_visible;
            self.i_forward[i] = new_forward;
            if load_i {
                self.i_depth[i] = c;
                self.i_id[i] = tag;
            }
        }
    }
}

/// A tile: its planes through the cells, and what they find shaded,
/// counting the pixels shaded into `shaded`.
fn render_tile(header: u32, planes: &[Plane], tsp: &Tsp, shaded: &mut u64) -> Tile {
    let width = ((header & 0x1F) + 1) * CELLS as u32;
    let height = (header >> 5 & 0x3FF) + 1;
    let x0 = (header >> 15 & 0x1F) * CELLS as u32;
    let y0 = header >> 20 & 0x3FF;
    let (w, h) = (width as usize, height as usize);
    let mut cells = Cells::new(w * h);
    let mut pixels = vec![0u32; w * h];
    let xs: Vec<f32> = (0..w * h).map(|i| (x0 as usize + i % w) as f32).collect();
    let ys: Vec<f32> = (0..w * h).map(|i| (y0 as usize + i / w) as f32).collect();
    let mut objects = 0;
    let mut previous = Previous::None;
    for plane in planes {
        let first = matches!(
            plane.instr,
            instr::FORW_VISIB_FP | instr::FORW_INVIS_FP | instr::TEST_SHAD_FORW_FP
        );
        if first {
            objects += 1;
        }
        let (control, next) = expand(plane.instr, objects == 2 && first, previous);
        previous = next;
        let (a, b, c) = (plane.a, plane.b, plane.c);
        // An edge everything in the tile is inside of changes nothing. (A
        // plane's least value over the tile is at a corner; the margin
        // covers the rounding of the pixels between.)
        if control.perpendicular
            && control.i_load == LoadI::Further
            && control.u_load == LoadU::Nop
            && control.test_shad == TestShad::Nop
            && !control.clear_u_id
        {
            let (x1, y1) = ((x0 + width - 1) as f32, (y0 + height - 1) as f32);
            let (x0, y0) = (x0 as f32, y0 as f32);
            let margin = (a.abs() * x1 + b.abs() * y1 + c.abs()) * 1e-5;
            let least = [a * x0 + b * y0, a * x1 + b * y0, a * x0 + b * y1, a * x1 + b * y1]
                .into_iter()
                .fold(f32::INFINITY, f32::min)
                + c;
            if least > margin {
                continue;
            }
        }
        cells.process(control, plane.tag, |i| a * xs[i] + b * ys[i] + c);
        if plane.instr == instr::BEGIN_TRANS {
            *shaded += shade(&cells, tsp, x0, y0, w, &mut pixels);
        }
    }
    // After the region's last object a flush down the pipeline puts what
    // the cells look at in front, if it is, as the next object would.
    let flush = Control {
        i_load: LoadI::Nop,
        u_load: LoadU::Closer,
        test_shad: TestShad::Nop,
        visible: true,
        clear_u_id: false,
        perpendicular: false,
        mux_u: true,
    };
    cells.process(flush, 0, |_| 0.0);
    *shaded += shade(&cells, tsp, x0, y0, w, &mut pixels);
    Tile { x: x0, y: y0, width, height, pixels }
}

/// The TSP: the surfaces the cells hold, shaded into the tile. Returns
/// the pixels it shaded.
fn shade(cells: &Cells, tsp: &Tsp, x0: u32, y0: u32, w: usize, pixels: &mut [u32]) -> u64 {
    let mut shaded = 0;
    for (i, pixel) in pixels.iter_mut().enumerate() {
        let tag = cells.u_id[i];
        if tag != 0 {
            let (x, y) = (x0 as i32 + (i % w) as i32, y0 as i32 + (i / w) as i32);
            *pixel = tsp.shade(x, y, tag, cells.u_depth[i], cells.u_shadow[i], *pixel);
            shaded += 1;
        }
    }
    shaded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_bit_floats_are_shifted_magnitudes() {
        assert_eq!(from_20bit(15 << 16 | 0x4000), 1.0 / 1024.0);
        assert_eq!(from_20bit(9 << 16 | 0x4000), 1.0 / 65536.0);
        assert_eq!(from_20bit(9 << 16 | 0x8000 | 0x4000), -1.0 / 65536.0);
        assert_eq!(from_20bit(0), 0.0);
    }

    #[test]
    fn fixed_planes_carry_the_tag_and_instruction_across_a_and_b() {
        let tag = 0x2_5ABC;
        let a = 0x0B << 26 | (tag & 0x3_F000) << 8 | 9 << 16 | 0x4000;
        let b = tag << 20 | 0x8000 | 9 << 16 | 0x2000;
        let c = 0x4000_0000u32;
        let mut ram = vec![0u8; 0x2_0000];
        for (i, w) in [a, b, c].into_iter().enumerate() {
            ram[0x1_0000 + 4 * i..][..4].copy_from_slice(&w.to_le_bytes());
        }
        let mut registers = vec![0; regs::COUNT];
        // Pages of 4 KB from 0, so the parameter space is the memory.
        for (i, entry) in registers[regs::TLB..regs::TLB + PAGES].iter_mut().enumerate() {
            *entry = i as u32;
        }
        let params = Params::new(&registers, Memory { ram: &ram });
        let mut planes = Vec::new();
        fetch_planes(&params, PlaneFormat::Fixed, 1 << 19 | 0x1_0000 >> 2, &mut planes);
        let p = planes[0];
        assert_eq!((p.instr, p.tag), (0x0B, tag));
        assert_eq!((p.a, p.b, p.c), (1.0 / 65536.0, -1.0 / 131072.0, 0.5));
    }
}
