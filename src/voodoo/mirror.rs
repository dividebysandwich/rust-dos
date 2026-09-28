//! What the OpenGL renderer draws (`voodoo_renderer=opengl`). The software
//! rasterizer goes on keeping the card's memory, which games read back,
//! save states keep and screenshots show (drawing only what they can still
//! see, `backlog`); the mirror records the same drawing for the front end
//! to do again with OpenGL, at a higher resolution, for the window:
//! triangles as vertices carrying the values the card iterates, fastfills,
//! the pixels frame buffer writes left, the textures the triangles use
//! decoded to ARGB as the card reads them, and the buffers' pixels whenever
//! their layout changes or a state loads.

use super::raster::{RasterState, TmuRaster, TriParams, fetch_texel};
use super::texture::{PAGE_SHIFT, Tmu};
use std::collections::HashMap;

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
    pub fogdelta: [u8; 64],
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

/// Where the buffers are: their size in pixels, and the word offsets of
/// the colour buffers and the auxiliary buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    pub rowpixels: u32,
    pub color: Vec<u32>,
    pub aux: Option<u32>,
}

/// What the layout is worked out from, which the card compares on every
/// command without building a `Layout`: the size, the row length and the
/// byte offsets of the colour buffers and the auxiliary buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutKey {
    pub width: u32,
    pub height: u32,
    pub rowpixels: u32,
    pub color: [u32; 3],
    pub aux: u32,
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
        }
    }

    /// The layout the next command is in: a snapshot first if it is new,
    /// or if one is wanted.
    pub(crate) fn needs_snapshot(&self, layout: &LayoutKey) -> bool {
        self.layout.as_ref() != Some(layout)
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
        if let Some(Command::Draw(draw)) = self.commands.last_mut()
            && draw.state == state
        {
            draw.vertices.extend(vertices);
            return;
        }
        self.push(Command::Draw(Box::new(Draw { state, vertices: vertices.to_vec() })));
    }

    /// A fastfill of rows `y0..y1` (as triangles count them), columns
    /// `x0..x1`.
    pub(crate) fn fastfill(&mut self, st: &RasterState, (x0, x1, y0, y1): (i32, i32, i32, i32), rgb: bool, aux: bool) {
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
        }));
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
            lookup: tmu.lookup_key(t.mode),
        };
        let frame = self.frame;
        let unit_info = |id: u32| TexUnit {
            texture: id,
            mode: t.mode,
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
        let (lr, ly) = (st.clip_left_right, st.clip_low_y_high_y);
        [(lr >> 16) & 0x3FF, lr & 0x3FF, (ly >> 16) & 0x3FF, ly & 0x3FF]
    });
    DrawState {
        dest: st.dest as u32,
        fbz_color_path: st.fbz_color_path,
        fbz_mode: st.fbz_mode,
        alpha_mode: st.alpha_mode,
        fog_mode: st.fog_mode,
        za_color: st.za_color,
        chroma_key: st.chroma_key,
        color0: st.color0,
        color1: st.color1,
        fog_color: st.fog_color,
        stipple,
        yorigin: st.yorigin,
        clip,
        aux: st.aux.is_some(),
        fogblend: st.fogblend,
        fogdelta: st.fogdelta,
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
