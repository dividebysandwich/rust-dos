//! What the OpenGL renderer draws (`voodoo_renderer=opengl`). The software
//! rasterizer goes on keeping the card's memory, which games read back,
//! save states keep and screenshots show (drawing only what they can still
//! see, `backlog`); the mirror records the same drawing for the front end
//! to do again with OpenGL, at a higher resolution, for the window:
//! triangles as vertices carrying the values the card iterates, fastfills,
//! the pixels frame buffer writes left, the textures the triangles use
//! decoded to ARGB as the card reads them, and the buffers' pixels whenever
//! their layout changes or a state loads.

use super::NONE;
use super::raster::{RasterState, Stride, TmuRaster, TriParams, fetch_texel};
use super::texture::{PAGE_SHIFT, Tmu};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

/// A vertex: where it is in the buffer in pixels, rows counted from the
/// top of the buffer (the Y origin applied), and the values the card
/// iterates there: the colour in 8-bit units, Z in 16-bit units, W, and
/// each texture unit's S, T and W (16.32 values as numbers).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub color: [f32; 4],
    pub zw: [f32; 2],
    pub tex: [[f32; 3]; 2],
}

/// A texture unit as triangles draw with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TexUnit {
    /// The texture (`Command::Texture`).
    pub texture: u32,
    pub mode: u32,
    /// The host requested point sampling; OpenGL may still blend mip levels
    /// when anisotropic filtering is enabled.
    pub unfiltered: bool,
    /// The size of level 0 in texels, which S and T count in.
    pub width: u32,
    pub height: u32,
    /// The level the texture starts with, and the level of detail's limits
    /// and bias, in 8.8.
    pub first_level: i32,
    pub lodmin: i32,
    pub lodmax: i32,
    pub lodbias: i32,
    pub detailmax: i32,
    pub detailbias: i32,
    pub detailscale: u32,
}

/// Everything the triangles of a `Draw` draw with besides their vertices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrawState {
    /// The colour buffer drawn into (its word offset in the frame buffer).
    pub dest: u32,
    pub fbz_color_path: u32,
    pub fbz_mode: u32,
    pub alpha_mode: u32,
    pub fog_mode: u32,
    pub za_color: u32,
    pub chroma_key: u32,
    /// chromaRange, a Voodoo2's (`RasterState::chroma_rejects`).
    pub chroma_range: u32,
    pub color0: u32,
    pub color1: u32,
    pub fog_color: u32,
    pub stipple: u32,
    pub yorigin: u32,
    /// The clip rectangle in buffer pixels (left, right, top, bottom
    /// rows), when fbzMode clips.
    pub clip: Option<[u32; 4]>,
    /// Whether there is an auxiliary (depth or alpha) buffer.
    pub aux: bool,
    pub fogblend: [u8; 64],
    /// The fog table deltas as the pipeline uses them: the chip's bits,
    /// negated where fogMode bit 7 and the delta's bit 1 say so.
    pub fogdelta: [i16; 64],
    /// TMU 0's configuration instead of its texels (`RasterState`).
    pub send_config: Option<u32>,
    pub tmu: [Option<TexUnit>; 2],
}

/// Triangles, three vertices each.
#[derive(Clone, Debug)]
pub struct Draw {
    pub state: DrawState,
    pub vertices: Vec<Vertex>,
}

/// A fastfill of a rectangle of buffer pixels (left, right, top, bottom,
/// the right and bottom ones not filled): the colour buffer `dest` with
/// `color` (RGB), the auxiliary buffer with `aux`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    pub dest: u32,
    pub rect: [u32; 4],
    pub color: Option<u32>,
    pub aux: Option<u16>,
    /// Whether the auxiliary buffer holds alpha rather than depth.
    pub alpha_planes: bool,
    /// With fbzMode's dithering, the 5-6-5 values the card fills a 4x4
    /// block of pixels with, by buffer row and column (each `& 3`). Boxed,
    /// so that it doesn't make every command bigger.
    pub pattern: Option<Box<[u16; 16]>>,
}

/// Pixels of a row as frame buffer writes left them, 5-6-5 colours in the
/// colour buffer `dest`, or with `dest` None depths (or alphas) in the
/// auxiliary buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pixels {
    pub dest: Option<u32>,
    pub x: u32,
    pub y: u32,
    pub values: Vec<u16>,
}

/// Where the buffers are: their size in pixels, how their rows lie in
/// memory (the colour buffers' and the auxiliary buffer's, linear or in
/// a Banshee's tiles), and the word offsets of the colour buffers and the
/// auxiliary buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    pub stride: Stride,
    pub aux_stride: Stride,
    pub color: Vec<u32>,
    pub aux: Option<u32>,
}

/// What the layout is worked out from, which the card compares on every
/// command without building a `Layout`: the size, the strides and the
/// byte offsets of the colour buffers and the auxiliary buffer (`NONE`
/// where there is none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutKey {
    pub width: u32,
    pub height: u32,
    pub stride: Stride,
    pub aux_stride: Stride,
    pub color: [u32; 3],
    pub aux: u32,
}

impl LayoutKey {
    pub fn layout(&self) -> Layout {
        Layout {
            width: self.width,
            height: self.height,
            stride: self.stride,
            aux_stride: self.aux_stride,
            color: self
                .color
                .iter()
                .filter(|&&offs| offs != NONE)
                .map(|&offs| offs / 2)
                .collect(),
            aux: (self.aux != NONE).then_some(self.aux / 2),
        }
    }

    /// The buffers: each one's word offset, its key in the recording (the
    /// offset for a colour buffer, None for the auxiliary buffer) and its
    /// stride.
    fn buffers(&self) -> impl Iterator<Item = (u32, Option<u32>, Stride)> + '_ {
        let color = self.color.iter().filter(|&&offs| offs != NONE);
        let color = color.map(|&offs| (offs / 2, Some(offs / 2), self.stride));
        let aux = (self.aux != NONE).then_some((self.aux / 2, None, self.aux_stride));
        color.chain(aux)
    }

    /// Where word `at` of the frame buffer is in the buffers: the buffer's
    /// key (`buffers`) and the pixel's column and row, if it is a pixel
    /// of one.
    pub fn locate(&self, at: u32) -> Option<(Option<u32>, u32, u32)> {
        self.buffers().find_map(|(base, key, stride)| {
            let (x, y) = pixel_at(stride, at.checked_sub(base)? as usize)?;
            (x < self.width && y < self.height).then_some((key, x, y))
        })
    }

    /// Whether bytes `start..end` of the frame buffer hold any of the
    /// buffers' memory.
    pub fn overlaps(&self, start: u32, end: u32) -> bool {
        self.buffers().any(|(base, _, stride)| {
            let (base, span) = (base as u64 * 2, span(stride, self.height) as u64 * 2);
            (start as u64) < base + span && base < end as u64
        })
    }

    /// The buffers' rows that bytes `start..end` of the frame buffer may
    /// hold pixels of: the buffer's key (`buffers`) and rows `first..last`.
    pub fn rows_in(&self, start: u32, end: u32) -> Vec<(Option<u32>, u32, u32)> {
        self.buffers()
            .filter_map(|(base, key, stride)| {
                let (base, words) = (base as u64 * 2, stride.words.max(1) as u64);
                let span = span(stride, self.height) as u64 * 2;
                let (start, end) = ((start as u64).max(base), (end as u64).min(base + span));
                if start >= end {
                    return None;
                }
                let (w0, w1) = ((start - base) / 2, (end - base).div_ceil(2));
                // Rows, or with tiles bands of 32 rows.
                let (r0, r1) = (w0 / words, (w1 - 1) / words + 1);
                let (r0, r1) = if stride.tiled {
                    (r0 * 32, r1 * 32)
                } else {
                    (r0, r1)
                };
                let r1 = r1.min(self.height as u64);
                (r0 < r1).then_some((key, r0 as u32, r1 as u32))
            })
            .collect()
    }
}

/// The words a buffer of `height` rows of `stride` spans.
pub fn span(stride: Stride, height: u32) -> usize {
    if stride.tiled {
        (height as usize).div_ceil(32) * stride.words
    } else {
        height as usize * stride.words
    }
}

/// The column and row of the pixel `at` words from a buffer's first.
pub fn pixel_at(stride: Stride, at: usize) -> Option<(u32, u32)> {
    if stride.words == 0 {
        return None;
    }
    let (x, y) = if stride.tiled {
        // Rows of tiles of 64 pixels by 32 rows (`Stride::row`).
        let (band, within) = (at / stride.words, at % stride.words);
        let (tile, word) = (within / 2048, within % 2048);
        (tile * 64 + word % 64, band * 32 + word / 64)
    } else {
        (at % stride.words, at / stride.words)
    };
    Some((x as u32, y as u32))
}

/// The buffers' pixels, `width * height` each, in `Layout` order.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub layout: Layout,
    pub color: Vec<Vec<u16>>,
    pub aux: Option<Vec<u16>>,
}

/// A mipmap level: its size and ARGB texels, row by row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Level {
    pub width: u32,
    pub height: u32,
    pub argb: Vec<u32>,
}

/// A texture, new or in place of the one with its number.
#[derive(Clone, Debug)]
pub struct Texture {
    pub id: u32,
    pub levels: Vec<Level>,
}

/// What to draw, in order.
#[derive(Clone, Debug)]
pub enum Command {
    /// Start over from the buffers' pixels, in a layout that may be new.
    Resync(Box<Snapshot>),
    Draw(Box<Draw>),
    Fill(Fill),
    Pixels(Pixels),
    Texture(Box<Texture>),
    FreeTexture(u32),
}

/// The drawing since the last frame, and what the card shows now.
#[derive(Clone, Debug)]
pub struct Frame {
    pub commands: Vec<Command>,
    /// The colour buffer shown, and whether the card shows it rather than
    /// the VGA its picture.
    pub front: Option<u32>,
    pub output: bool,
    pub width: u32,
    pub height: u32,
    /// The gamma table (clutData): 33 entries of RGB.
    pub clut: [u32; 33],
    /// A Banshee's or Voodoo3's picture, which its video processor
    /// composes, while the card shows it.
    pub scanout: Option<Box<Scanout>>,
}

impl Frame {
    /// Whether the renderer's picture is the one to show: the card shows
    /// its own output, and it is a colour buffer or a Banshee's scanout.
    /// A frontend that can't compose a scanout never asks a Banshee to
    /// record, so this holds for both frontends.
    pub fn shown(&self) -> bool {
        self.output && (self.front.is_some() || self.scanout.is_some())
    }
}

/// A surface a Banshee's video processor shows: its first byte in the
/// card's memory, its stride in bytes (a row's, or a row of tiles' when
/// tiled), its pixel format (the vidProcCfg field), the CLUT half it goes
/// through (0 or 256) unless it bypasses the CLUT, and the recording's
/// colour buffer it is (a word offset), when the OpenGL renderer draws
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Surface {
    pub start: u32,
    pub stride: u32,
    pub tiled: bool,
    pub format: u32,
    pub clut: u32,
    pub bypass: bool,
    pub buffer: Option<u32>,
}

/// The overlay window: its surface, where it is on the screen (left,
/// top, right, bottom, all inclusive) and its scaling: the source pixels
/// a screen pixel steps across and down, and the first row's offset, in
/// 20-bit fractions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overlay {
    pub surface: Surface,
    pub rect: [u32; 4],
    pub dudx: u32,
    pub dvdy: u32,
    pub v_offset: u32,
}

/// The hardware cursor: its top left corner on the screen, whether it is
/// in X11 mode, its two colours (RGB) and its two bit planes, 16 bytes a
/// row of 64 pixels, 64 rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub x: i32,
    pub y: i32,
    pub x11: bool,
    pub colors: [u32; 2],
    pub pattern: Vec<u8>,
}

/// What a Banshee's or Voodoo3's video processor shows, for the OpenGL
/// renderer to compose as `Banshee::render` draws it: the desktop of
/// `width` by `height` pixels, each line twice in half mode and each
/// pixel twice across when `wide`, the overlay window over it and the
/// cursor on top, through the CLUT (512 entries of RGB). The memory the desktop and the overlay show (bytes from their
/// first on), where they aren't buffers the renderer draws, comes when
/// the picture may have changed since the last frame; None keeps what
/// came before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scanout {
    pub width: u32,
    pub height: u32,
    pub half: bool,
    pub wide: bool,
    pub desktop: Surface,
    pub overlay: Option<Overlay>,
    pub cursor: Option<Cursor>,
    pub clut: Vec<u32>,
    pub desktop_memory: Option<Vec<u8>>,
    pub overlay_memory: Option<Vec<u8>>,
}

/// Frames a texture stays decoded without being drawn with.
const UNUSED_FRAMES: u64 = 600;
/// Commands that pile up without a front end taking them are dropped for
/// the buffers' pixels.
const MAX_COMMANDS: usize = 500_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TexKey {
    unit: u8,
    base: u32,
    format: u32,
    wmask: i32,
    hmask: i32,
    lodmask: u32,
    first: i32,
    last: i32,
    /// The colours of its palette or NCC table, hashed (`lookup_hash`).
    lookup: u64,
}

struct Cached {
    id: u32,
    /// The bytes of texture RAM its levels come from, all of it if they
    /// wrap.
    start: u32,
    end: u32,
    /// The unit's write count it was last known good at.
    good_at: u32,
    used: u64,
}

/// The texture a unit drew with last, while its memory stays as it was:
/// the next triangle with the same one needs no look-up.
#[derive(Clone, Copy)]
struct LastTexture {
    key: TexKey,
    writes: u32,
    id: u32,
    frame: u64,
}

/// The recording.
pub struct Mirror {
    commands: Vec<Command>,
    textures: HashMap<TexKey, Cached>,
    next_id: u32,
    frame: u64,
    /// The layout of the last snapshot; None to take one.
    layout: Option<LayoutKey>,
    /// The row of pixels each buffer's frame buffer writes go on adding
    /// to, one a buffer, and the one written last: commands once the
    /// writes go elsewhere or something draws.
    open: Vec<Pixels>,
    last_open: usize,
    last_texture: [Option<LastTexture>; 2],
    /// Each unit's look-up table as hashed last: its change count
    /// (`Tmu::lookup_key`) and the hash of its colours.
    lookups: [(u64, u64); 2],
}

impl Default for Mirror {
    fn default() -> Self {
        Self::new()
    }
}

impl Mirror {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            textures: HashMap::new(),
            next_id: 1,
            frame: 0,
            layout: None,
            open: Vec::new(),
            last_open: 0,
            last_texture: [None; 2],
            lookups: [(0, 0); 2],
        }
    }

    /// The layout the next command is in: a snapshot first if it is new,
    /// or if one is wanted.
    pub(crate) fn needs_snapshot(&self, layout: &LayoutKey) -> bool {
        self.layout.as_ref() != Some(layout)
    }

    /// The layout of the last snapshot, while it is good.
    pub(crate) fn layout(&self) -> Option<&LayoutKey> {
        self.layout.as_ref()
    }

    pub(crate) fn resync(&mut self, snapshot: Snapshot, key: LayoutKey) {
        // What was recorded before is drawn over.
        self.commands.retain(|c| matches!(c, Command::Texture(_) | Command::FreeTexture(_)));
        self.open.clear();
        self.layout = Some(key);
        self.commands.push(Command::Resync(Box::new(snapshot)));
    }

    /// Take a snapshot before the next command: the memory changed under
    /// the recording (a state loaded).
    pub(crate) fn invalidate(&mut self) {
        self.layout = None;
    }

    /// Forget the decoded textures: the TMUs start over.
    pub(crate) fn forget_textures(&mut self) {
        self.last_texture = [None; 2];
        self.lookups = [(0, 0); 2];
        for (_, cached) in self.textures.drain() {
            self.commands.push(Command::FreeTexture(cached.id));
        }
    }

    fn push(&mut self, command: Command) {
        // The frame buffer writes before a drawing come before it.
        if matches!(command, Command::Draw(_) | Command::Fill(_)) {
            self.close_rows();
        }
        self.push_command(command);
    }

    fn push_command(&mut self, command: Command) {
        if self.commands.len() >= MAX_COMMANDS {
            // Nobody draws them: start over from the pixels when someone
            // does.
            self.commands.clear();
            self.open.clear();
            self.forget_textures();
            self.layout = None;
        }
        self.commands.push(command);
    }

    /// The rows of pixels being written, as commands.
    fn close_rows(&mut self) {
        for row in std::mem::take(&mut self.open) {
            self.push_command(Command::Pixels(row));
        }
    }

    /// A triangle the software rasterizer draws with `st` and `p`, from
    /// `verts` (in pixels), with `texcount` texture units.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn triangle(
        &mut self,
        st: &RasterState,
        p: &TriParams,
        verts: [(f32, f32); 3],
        texcount: usize,
        tmus: &[Tmu],
        stipple: u32,
    ) {
        let mut tmu = [None, None];
        for (unit, slot) in tmu.iter_mut().enumerate().take(texcount) {
            if let Some(t) = &st.tmu[unit]
                && t.lodmin < 8 << 8
            {
                *slot = Some(self.texture(unit, t, &tmus[unit]));
            }
        }
        let state = draw_state(st, stipple, tmu);
        let flip = st.fbz_mode & (1 << 17) != 0;
        let (ax, ay) = ((p.ax >> 4) as f64, (p.ay >> 4) as f64);
        let vertices = verts.map(|(x, y)| {
            // The card iterates from vertex A's pixel, sampling each pixel
            // at its top left corner; here the values are at the vertex.
            let (dx, dy) = (x as f64 - 0.5 - ax, y as f64 - 0.5 - ay);
            let at = |start: f64, ddx: f64, ddy: f64| start + dx * ddx + dy * ddy;
            let color = |start: i32, ddx: i32, ddy: i32| (at(start as f64, ddx as f64, ddy as f64) / 4096.0) as f32;
            let wide = |start: i64, ddx: i64, ddy: i64| (at(start as f64, ddx as f64, ddy as f64) / 4_294_967_296.0) as f32;
            let mut tex = [[0.0; 3]; 2];
            for (unit, t) in tex.iter_mut().enumerate().take(texcount) {
                let q = &p.tmu[unit];
                *t = [wide(q.starts, q.dsdx, q.dsdy), wide(q.startt, q.dtdx, q.dtdy), wide(q.startw, q.dwdx, q.dwdy)];
            }
            Vertex {
                pos: [x, if flip { st.yorigin as f32 + 1.0 - y } else { y }],
                color: [
                    color(p.startr, p.drdx, p.drdy),
                    color(p.startg, p.dgdx, p.dgdy),
                    color(p.startb, p.dbdx, p.dbdy),
                    color(p.starta, p.dadx, p.dady),
                ],
                zw: [color(p.startz, p.dzdx, p.dzdy), wide(p.startw, p.dwdx, p.dwdy)],
                tex,
            }
        });
        // LFB writes between identical draws must stay between them,
        // not be replayed after a merged draw.
        self.close_rows();
        if let Some(Command::Draw(draw)) = self.commands.last_mut()
            && draw.state == state
        {
            draw.vertices.extend(vertices);
            return;
        }
        self.push(Command::Draw(Box::new(Draw { state, vertices: vertices.to_vec() })));
    }

    /// A fastfill of rows `y0..y1` (as triangles count them), columns
    /// `x0..x1`, with `dither` the colour's pattern if it is dithered, by
    /// rows as triangles count them.
    pub(crate) fn fastfill(
        &mut self,
        st: &RasterState,
        (x0, x1, y0, y1): (i32, i32, i32, i32),
        rgb: bool,
        aux: bool,
        dither: Option<&[u16; 16]>,
    ) {
        let fbz = st.fbz_mode;
        let (top, bottom) = if fbz & (1 << 17) != 0 {
            (st.yorigin as i32 + 1 - y1, st.yorigin as i32 + 1 - y0)
        } else {
            (y0, y1)
        };
        let rect = [x0.max(0) as u32, x1.max(0) as u32, top.max(0) as u32, bottom.max(0) as u32];
        if rect[0] >= rect[1] || rect[2] >= rect[3] {
            return;
        }
        self.push(Command::Fill(Fill {
            dest: st.dest as u32,
            rect,
            color: rgb.then_some(st.color1 & 0xFF_FFFF),
            aux: (aux && st.aux.is_some()).then_some(st.za_color as u16),
            alpha_planes: fbz & (1 << 18) != 0,
            pattern: dither.filter(|_| rgb).map(|d| {
                // Buffer row r is row yorigin - r as the card counts them.
                let flip = fbz & (1 << 17) != 0;
                Box::new(std::array::from_fn(|i| {
                    let (r, x) = (i as u32 >> 2, i & 3);
                    let y = if flip { st.yorigin.wrapping_sub(r) } else { r };
                    d[(y & 3) as usize * 4 + x]
                }))
            }),
        }));
    }

    /// A rectangle of a buffer filled with one value, as a Voodoo2's
    /// blitter fills whole pages.
    pub(crate) fn fill(&mut self, fill: Fill) {
        self.push(Command::Fill(fill));
    }

    /// Pixel `x` of buffer row `y` is `value` after a frame buffer write:
    /// in colour buffer `dest`, or with None in the auxiliary buffer, of
    /// `width` pixels.
    pub(crate) fn pixel(&mut self, dest: Option<u32>, x: u32, y: u32, value: u16, width: u32) {
        let goes_on = |p: &Pixels| p.dest == dest && p.y == y && p.x + p.values.len() as u32 == x;
        if let Some(p) = self.open.get_mut(self.last_open)
            && goes_on(p)
        {
            p.values.push(value);
            return;
        }
        let at = self.open.iter().position(|p| p.dest == dest);
        if let Some(i) = at
            && goes_on(&self.open[i])
        {
            self.open[i].values.push(value);
            self.last_open = i;
            return;
        }
        // Room for the rest of the row, which writes usually go on to.
        let mut values = Vec::with_capacity(width.saturating_sub(x).max(1) as usize);
        values.push(value);
        let row = Pixels { dest, x, y, values };
        match at {
            Some(i) => {
                let done = std::mem::replace(&mut self.open[i], row);
                self.push_command(Command::Pixels(done));
                self.last_open = i;
            }
            None => {
                self.open.push(row);
                self.last_open = self.open.len() - 1;
            }
        }
    }

    /// The texture unit `unit` draws with, decoded if it isn't yet or its
    /// RAM or colours changed since.
    fn texture(&mut self, unit: usize, t: &TmuRaster, tmu: &Tmu) -> TexUnit {
        let first = (t.lodmin >> 8).clamp(0, 8);
        let last = (t.lodmax >> 8).clamp(first, 8);
        let key = TexKey {
            unit: unit as u8,
            base: t.lodoffset[0],
            format: (t.mode >> 8) & 0xF,
            wmask: t.wmask,
            hmask: t.hmask,
            lodmask: t.lodmask,
            first,
            last,
            lookup: self.lookup_hash(unit, t, tmu),
        };
        let frame = self.frame;
        let unit_info = |id: u32| TexUnit {
            texture: id,
            mode: t.mode,
            unfiltered: t.unfiltered,
            width: t.wmask as u32 + 1,
            height: t.hmask as u32 + 1,
            first_level: first,
            lodmin: t.lodmin,
            lodmax: t.lodmax,
            lodbias: t.lodbias,
            detailmax: t.detailmax,
            detailbias: t.detailbias,
            detailscale: t.detailscale,
        };
        if let Some(last) = &mut self.last_texture[unit]
            && last.key == key
            && last.writes == tmu.writes
        {
            if last.frame != frame {
                last.frame = frame;
                if let Some(cached) = self.textures.get_mut(&key) {
                    cached.used = frame;
                }
            }
            return unit_info(last.id);
        }
        let existing = self.textures.get(&key).map(|cached| cached.id);
        let good = match self.textures.get_mut(&key) {
            Some(cached) if cached.good_at == tmu.writes => true,
            Some(cached) => {
                let pages = &tmu.page_writes;
                let (start, end) = ((cached.start >> PAGE_SHIFT) as usize, (cached.end >> PAGE_SHIFT) as usize);
                let range = pages.get(start..=end.min(pages.len().saturating_sub(1))).unwrap_or(pages);
                // Writes since count up from it (wrapping).
                let good = range.iter().all(|&w| w.wrapping_sub(cached.good_at) as i32 <= 0);
                if good {
                    cached.good_at = tmu.writes;
                }
                good
            }
            None => false,
        };
        let id = match (good, existing) {
            (true, Some(id)) => {
                if let Some(cached) = self.textures.get_mut(&key) {
                    cached.used = frame;
                }
                id
            }
            _ => {
                let id = existing.unwrap_or_else(|| {
                    self.next_id = self.next_id.wrapping_add(1).max(1);
                    self.next_id
                });
                let (start, end) = (t.lodoffset[0], t.lodoffset[8].wrapping_add(8));
                let (start, end) = if end > start && end <= t.mask + 1 { (start, end - 1) } else { (0, t.mask) };
                self.textures.insert(key, Cached { id, start, end, good_at: tmu.writes, used: frame });
                self.push(Command::Texture(Box::new(Texture { id, levels: decode(t, first, last) })));
                id
            }
        };
        self.last_texture[unit] = Some(LastTexture { key, writes: tmu.writes, id, frame });
        unit_info(id)
    }

    /// The colours unit `unit`'s look-up table gives the texture `t`, as a
    /// hash; 0 for a format without one. A program may load a texture's
    /// palette each time before it draws with it. The table's change count
    /// then goes up every time, while its colours are the ones the texture
    /// was decoded with before.
    fn lookup_hash(&mut self, unit: usize, t: &TmuRaster, tmu: &Tmu) -> u64 {
        let changes = tmu.lookup_key(t.mode);
        if changes == 0 {
            return 0;
        }
        let (seen, hash) = self.lookups[unit];
        if seen == changes {
            return hash;
        }
        let mut hasher = DefaultHasher::new();
        // The kind of table, as `lookup_key` gives it in bits 41:40.
        (changes >> 40).hash(&mut hasher);
        t.lookup.hash(&mut hasher);
        let hash = hasher.finish() | 1;
        self.lookups[unit] = (changes, hash);
        hash
    }

    /// What was recorded since the last frame, the textures not drawn
    /// with for long given up.
    pub(crate) fn take(&mut self) -> Vec<Command> {
        self.frame += 1;
        let frame = self.frame;
        let mut unused = Vec::new();
        self.textures.retain(|_, cached| {
            let keep = frame - cached.used < UNUSED_FRAMES;
            if !keep {
                unused.push(cached.id);
            }
            keep
        });
        for last in &mut self.last_texture {
            if last.is_some_and(|l| unused.contains(&l.id)) {
                *last = None;
            }
        }
        self.close_rows();
        self.commands.extend(unused.into_iter().map(Command::FreeTexture));
        std::mem::take(&mut self.commands)
    }
}

fn draw_state(st: &RasterState, stipple: u32, tmu: [Option<TexUnit>; 2]) -> DrawState {
    let clip = (st.fbz_mode & 1 != 0).then(|| {
        let (left, right, low, high) = st.clip();
        [left as u32, right as u32, low as u32, high as u32]
    });
    DrawState {
        dest: st.dest as u32,
        fbz_color_path: st.fbz_color_path,
        fbz_mode: st.fbz_mode,
        alpha_mode: st.alpha_mode,
        fog_mode: st.fog_mode,
        za_color: st.za_color,
        chroma_key: st.chroma_key,
        chroma_range: st.chroma_range,
        color0: st.color0,
        color1: st.color1,
        fog_color: st.fog_color,
        stipple,
        yorigin: st.yorigin,
        clip,
        aux: st.aux.is_some(),
        fogblend: st.fogblend,
        fogdelta: st.fogdelta.map(|d| {
            let delta = (d as i32 & st.fogdelta_mask) as i16;
            if st.fog_mode & (1 << 7) != 0 && d & 2 != 0 { -delta } else { delta }
        }),
        send_config: st.send_config,
        tmu,
    }
}

/// Levels `first..=last` of the texture `t` draws with, as its texels
/// read: a level the texture hasn't (with the LOD split) is the next one
/// up, twice the size.
pub fn decode(t: &TmuRaster, first: i32, last: i32) -> Vec<Level> {
    (first..=last)
        .map(|level| {
            let (width, height) = ((t.wmask >> level) + 1, (t.hmask >> level) + 1);
            let mut source = level;
            if (t.lodmask >> source) & 1 == 0 {
                source += 1;
            }
            let source = source.clamp(0, 8);
            let shift = source - level;
            let (smax, tmax) = (t.wmask >> source, t.hmask >> source);
            let texbase = t.lodoffset[source as usize];
            let mut argb = Vec::with_capacity((width * height) as usize);
            for tc in 0..height {
                for s in 0..width {
                    argb.push(fetch_texel(t, texbase, smax, (s >> shift) & smax, (tc >> shift) & tmax));
                }
            }
            Level { width: width as u32, height: height as u32, argb }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A colour buffer at 64 KB, in linear rows of 100 pixels or in tiles
    /// two a row; depth at 256 KB, linear rows of 64 pixels.
    fn key(tiled: bool) -> LayoutKey {
        let stride = if tiled {
            Stride {
                words: 2 * 2048,
                tiled: true,
            }
        } else {
            Stride::linear(100)
        };
        LayoutKey {
            width: 96,
            height: 40,
            stride,
            aux_stride: Stride::linear(64),
            color: [0x1_0000, NONE, NONE],
            aux: 0x4_0000,
        }
    }

    #[test]
    fn words_are_located_in_the_buffers() {
        let linear = key(false);
        let base = 0x1_0000 / 2;
        assert_eq!(linear.locate(base + 3 * 100 + 7), Some((Some(base), 7, 3)));
        // Past the row's 96 pixels, and below the last row.
        assert_eq!(linear.locate(base + 3 * 100 + 98), None);
        assert_eq!(linear.locate(base + 40 * 100), None);
        assert_eq!(linear.locate(0x4_0000 / 2 + 64 + 5), Some((None, 5, 1)));
        let tiled = key(true);
        // Pixel (65, 33): the second tile of the second row of tiles.
        let at = base + 2 * 2048 + 2048 + 64 + 1;
        assert_eq!(tiled.locate(at), Some((Some(base), 65, 33)));
        assert_eq!(tiled.locate(base - 1), None);
    }

    #[test]
    fn byte_ranges_cover_rows() {
        let linear = key(false);
        let base = 0x1_0000;
        // Bytes 10..12 of row 2 and the start of row 3.
        assert_eq!(
            linear.rows_in(base + 410, base + 602),
            [(Some(base / 2), 2, 4)]
        );
        // From before the buffer to past its end: all its rows.
        assert_eq!(linear.rows_in(0, 0x2_0000), [(Some(base / 2), 0, 40)]);
        // A tiled buffer's band of 32 rows, the second cut at the height.
        let tiled = key(true);
        let band = base + 2 * 4096;
        assert_eq!(
            tiled.rows_in(band + 6, band + 8),
            [(Some(base / 2), 32, 40)]
        );
        assert_eq!(tiled.rows_in(band - 2, band + 2), [(Some(base / 2), 0, 40)]);
        assert!(tiled.rows_in(0x3_0000, 0x3_0010).is_empty());
        assert_eq!(
            tiled.rows_in(0x4_0000 + 128, 0x4_0000 + 130),
            [(None, 1, 2)]
        );
    }
}
