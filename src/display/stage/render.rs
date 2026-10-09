//! The scene drawn with OpenGL: its meshes and pictures uploaded once, and
//! each view drawn multisampled into a target of its own, from which it is
//! copied to the window or a headset's eye.

use super::scene::{Alpha, Leds, Scene, Shading};
use super::gi;
use super::shadow::{self, Caster, Layer};
use crate::video::shader::Glsl;
use rust_dos::vr::VrQuality;
use glam::{Mat4, Vec3};
use glow::HasContext;

const COMMON: &str = include_str!("shader/common.glsl");
const MESH_VERT: &str = include_str!("shader/mesh.vert");
const LIT_FRAG: &str = include_str!("shader/lit.frag");
const SKY_FRAG: &str = include_str!("shader/sky.frag");
const SHADOW_FRAG: &str = include_str!("shader/shadow.frag");
const GRID_FRAG: &str = include_str!("shader/grid.frag");
const GI_UPDATE_FRAG: &str = include_str!("shader/gi_update.frag");
const AO_FRAG: &str = include_str!("shader/ao.frag");
const AO_BLUR_FRAG: &str = include_str!("shader/ao_blur.frag");

/// What `quality` works out: the screen's light as the picture's colours
/// in how many patches across and down, how many samples soften a shadow's
/// edge, whether the light bouncing around is worked out, and how many
/// samples the ambient occlusion takes at each pixel (none: none of it).
fn lighting(quality: VrQuality) -> ((i32, i32), u32, bool, u32) {
    match quality {
        VrQuality::Low => ((1, 1), 1, false, 0),
        VrQuality::Medium => ((2, 2), 6, true, 8),
        VrQuality::High => ((4, 3), 12, true, 12),
    }
}

/// How far around occluders darken the light from all around, in metres,
/// and how much.
const AO_RADIUS: f32 = 0.4;
const AO_STRENGTH: f32 = 0.5;

/// How much of the screen's light each new picture brings: a little of
/// the ones before stays, so that flicker doesn't strobe the room.
const GRID_TAKE: f32 = 0.7;
const FULLSCREEN_VERT: &str = include_str!("../../video/shader/vertex.glsl");

/// GL_TEXTURE_MAX_ANISOTROPY(_EXT) and its limit.
const TEXTURE_MAX_ANISOTROPY: u32 = 0x84FE;
const MAX_TEXTURE_MAX_ANISOTROPY: u32 = 0x84FF;

/// Where a view is seen from.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub view: Mat4,
    pub projection: Mat4,
}

impl View {
    pub fn view_projection(&self) -> Mat4 {
        self.projection * self.view
    }

    fn eye(&self) -> Vec3 {
        self.view.inverse().transform_point3(Vec3::ZERO)
    }
}

/// A box drawn over the scene in a colour of its own, unlit: a
/// controller, or its beam. The model matrix takes the unit cube (-0.5 to
/// 0.5) where it goes.
#[derive(Clone, Copy, Debug)]
pub struct Extra {
    pub model: Mat4,
    /// Linear RGB, and alpha.
    pub color: [f32; 4],
}

/// What a view is drawn into: its size, the colour format (RGBA8, or
/// SRGB8_ALPHA8 which encodes linear light itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub size: (u32, u32),
    pub srgb: bool,
}

struct GpuMesh {
    vao: glow::VertexArray,
    /// Its vertices and indices, deleted with it.
    buffers: [glow::Buffer; 2],
    count: i32,
    material: usize,
}

/// A multisampled colour and depth buffer to draw in, and a texture of the
/// same size the samples are resolved into.
struct Target {
    format: Format,
    framebuffer: glow::Framebuffer,
    color: glow::Renderbuffer,
    depth: glow::Renderbuffer,
    resolve: glow::Framebuffer,
    resolved: glow::Texture,
}

/// The shadows: their depth texture's layers, and which the lights and
/// the screen use.
struct Shadows {
    texture: glow::Texture,
    size: u32,
    layers: Vec<Layer>,
    /// By the scene's light: the first layer, or -1; 1 for a cube.
    lights: Vec<[i32; 2]>,
    /// The screen's first layer, or -1.
    screen: i32,
}

/// The light bounced around: the probes' sums, a row each (RGBA32F), and
/// their light for the picture now (see lit.frag's `u_gi`), which the
/// program updates.
struct Gi {
    layout: gi::Layout,
    probes: glow::Texture,
    light: glow::Texture,
    framebuffer: glow::Framebuffer,
    program: glow::Program,
    /// The picture or the screen's brightness changed since the update.
    stale: bool,
}

/// What the context offers that the views can be drawn faster with.
#[derive(Clone, Debug, Default)]
pub struct Features {
    pub renderer: String,
    /// Both eyes in one pass (GL_OVR_multiview2).
    pub multiview: bool,
    /// Multisampled into a texture with the samples resolved on the chip
    /// (GL_EXT_multisampled_render_to_texture).
    pub msrtt: bool,
    /// Both at once (GL_OVR_multiview_multisampled_render_to_texture).
    pub multiview_msrtt: bool,
}

impl Features {
    pub fn of(gl: &glow::Context) -> Self {
        let has = |name: &str| gl.supported_extensions().contains(name);
        Features {
            // SAFETY: see `GlScreen`.
            renderer: unsafe { gl.get_parameter_string(glow::RENDERER) },
            multiview: has("GL_OVR_multiview2"),
            msrtt: has("GL_EXT_multisampled_render_to_texture"),
            multiview_msrtt: has("GL_OVR_multiview_multisampled_render_to_texture"),
        }
    }

    pub fn describe(&self) -> String {
        let yes = |b: bool| if b { "yes" } else { "no" };
        format!(
            "OpenGL on {}: single-pass stereo {}, multisampling on the chip {}, both {}",
            self.renderer,
            yes(self.multiview),
            yes(self.msrtt),
            yes(self.multiview_msrtt)
        )
    }
}

/// What the graphics chip's time between two timestamps of a frame went
/// to, as `Clock` counts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// The screen's light and the light bounced around, for a new picture.
    Light,
    /// The ambient occlusion.
    Ao,
    /// The scene: sky, depth, lit meshes, and the samples resolved.
    Scene,
    /// Copies into the headset's images and the window's mirror.
    Copy,
}

impl Pass {
    const ALL: [Pass; 4] = [Pass::Light, Pass::Ao, Pass::Scene, Pass::Copy];

    fn name(self) -> &'static str {
        match self {
            Pass::Light => "light",
            Pass::Ao => "occlusion",
            Pass::Scene => "scene",
            Pass::Copy => "copies",
        }
    }
}

/// How long frames take the graphics chip, in all and by pass: timestamps
/// between the passes (never inside one, which a tiling chip would have to
/// split), read back a few frames later, when they are there.
struct Clock {
    /// The frame being drawn: its first timestamp, and each pass's last.
    current: Vec<(glow::Query, Option<Pass>)>,
    /// Frames drawn whose timestamps aren't read yet, oldest first.
    pending: std::collections::VecDeque<Vec<(glow::Query, Option<Pass>)>>,
    free: Vec<glow::Query>,
    /// Milliseconds by pass, and frames, since the last report.
    sums: [f64; 4],
    total: f64,
    frames: u32,
    /// The newest frame read's milliseconds in all.
    latest: Option<f32>,
}

impl Clock {
    /// Frames in flight at most; more go untimed.
    const IN_FLIGHT: usize = 4;

    fn new() -> Self {
        Clock { current: Vec::new(), pending: Default::default(), free: Vec::new(), sums: [0.0; 4], total: 0.0, frames: 0, latest: None }
    }

    fn stamp(&mut self, gl: &glow::Context, pass: Option<Pass>) {
        // SAFETY: see `GlScreen`.
        let query = self.free.pop().or_else(|| unsafe { gl.create_query() }.ok());
        if let Some(query) = query {
            // SAFETY: see `GlScreen`.
            unsafe { gl.query_counter(query, glow::TIMESTAMP) };
            self.current.push((query, pass));
        }
    }

    fn start(&mut self, gl: &glow::Context) {
        self.read(gl);
        self.free.extend(self.current.drain(..).map(|(q, _)| q));
        if self.pending.len() < Self::IN_FLIGHT {
            self.stamp(gl, None);
        }
    }

    /// `pass` ends now, if a frame is being timed.
    fn mark(&mut self, gl: &glow::Context, pass: Pass) {
        if !self.current.is_empty() {
            self.stamp(gl, Some(pass));
        }
    }

    fn end(&mut self) {
        if self.current.len() > 1 {
            self.pending.push_back(std::mem::take(&mut self.current));
        }
    }

    /// Count the frames whose timestamps are there.
    fn read(&mut self, gl: &glow::Context) {
        while let Some(frame) = self.pending.front() {
            let &(last, _) = frame.last().expect("a frame's timestamps");
            // SAFETY: see `GlScreen`.
            if unsafe { gl.get_query_parameter_u32(last, glow::QUERY_RESULT_AVAILABLE) } == 0 {
                break;
            }
            let frame = self.pending.pop_front().expect("the frame");
            // SAFETY: see `GlScreen`.
            let times: Vec<u64> = frame.iter().map(|&(q, _)| unsafe { gl.get_query_parameter_u64(q, glow::QUERY_RESULT) }).collect();
            for (pair, &(_, pass)) in times.windows(2).zip(&frame[1..]) {
                let ms = pair[1].saturating_sub(pair[0]) as f64 / 1e6;
                if let Some(at) = Pass::ALL.iter().position(|&p| Some(p) == pass) {
                    self.sums[at] += ms;
                }
            }
            let total = times[times.len() - 1].saturating_sub(times[0]) as f64 / 1e6;
            self.total += total;
            self.frames += 1;
            self.latest = Some(total as f32);
            self.free.extend(frame.into_iter().map(|(q, _)| q));
        }
    }

    /// The frames' average since the last report, by pass; counted again
    /// from here.
    fn report(&mut self) -> Option<String> {
        if self.frames == 0 {
            return None;
        }
        let n = self.frames as f64;
        let passes: Vec<String> = Pass::ALL
            .iter()
            .zip(self.sums)
            .filter(|(_, ms)| *ms > 0.0)
            .map(|(pass, ms)| format!("{} {:.2}", pass.name(), ms / n))
            .collect();
        let line = format!("The graphics chip takes {:.2} ms a frame ({})", self.total / n, passes.join(", "));
        (self.sums, self.total, self.frames) = ([0.0; 4], 0.0, 0);
        Some(line)
    }

    fn delete(self, gl: &glow::Context) {
        let queries = self.current.into_iter().chain(self.pending.into_iter().flatten()).map(|(q, _)| q);
        for query in queries.chain(self.free) {
            // SAFETY: see `GlScreen`.
            unsafe { gl.delete_query(query) };
        }
    }
}

/// The ambient occlusion: its programs, and what it is drawn into for
/// views of the size it was last.
struct Ao {
    estimate: glow::Program,
    blur: glow::Program,
    target: Option<AoTarget>,
}

/// At half a view's size: its depth, and the occlusion and its blur
/// across, each with its framebuffer.
struct AoTarget {
    size: (i32, i32),
    depth: glow::Texture,
    depth_framebuffer: glow::Framebuffer,
    pictures: [glow::Texture; 2],
    framebuffers: [glow::Framebuffer; 2],
}

impl AoTarget {
    fn new(gl: &glow::Context, size: (i32, i32)) -> Result<Self, String> {
        let (w, h) = size;
        // SAFETY: see `GlScreen`.
        unsafe {
            let texture = |format: u32, kind: u32, value: u32| -> Result<glow::Texture, String> {
                let t = gl.create_texture()?;
                gl.bind_texture(glow::TEXTURE_2D, Some(t));
                let none = glow::PixelUnpackData::Slice(None);
                gl.tex_image_2d(glow::TEXTURE_2D, 0, format as i32, w, h, 0, kind, value, none);
                let filter = if kind == glow::DEPTH_COMPONENT { glow::NEAREST } else { glow::LINEAR };
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
                Ok(t)
            };
            let depth = texture(glow::DEPTH_COMPONENT24, glow::DEPTH_COMPONENT, glow::UNSIGNED_INT)?;
            let pictures = [texture(glow::R8, glow::RED, glow::UNSIGNED_BYTE)?, texture(glow::R8, glow::RED, glow::UNSIGNED_BYTE)?];
            gl.bind_texture(glow::TEXTURE_2D, None);
            let depth_framebuffer = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(depth_framebuffer));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, glow::TEXTURE_2D, Some(depth), 0);
            gl.draw_buffer(glow::NONE);
            gl.read_buffer(glow::NONE);
            let mut complete = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            let mut framebuffers = Vec::new();
            for picture in pictures {
                let framebuffer = gl.create_framebuffer()?;
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(picture), 0);
                complete &= gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
                framebuffers.push(framebuffer);
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            let target = AoTarget { size, depth, depth_framebuffer, pictures, framebuffers: [framebuffers[0], framebuffers[1]] };
            if !complete {
                target.delete(gl);
                return Err("OpenGL can't draw the ambient occlusion".into());
            }
            Ok(target)
        }
    }

    fn delete(&self, gl: &glow::Context) {
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.delete_framebuffer(self.depth_framebuffer);
            gl.delete_texture(self.depth);
            for (framebuffer, picture) in self.framebuffers.iter().zip(&self.pictures) {
                gl.delete_framebuffer(*framebuffer);
                gl.delete_texture(*picture);
            }
        }
    }
}

/// Where a program's material uniforms are.
struct MaterialLocations {
    shading: Option<glow::UniformLocation>,
    base_color: Option<glow::UniformLocation>,
    has_base: Option<glow::UniformLocation>,
    emissive: Option<glow::UniformLocation>,
    has_emissive: Option<glow::UniformLocation>,
    cutoff: Option<glow::UniformLocation>,
}

impl MaterialLocations {
    fn of(gl: &glow::Context, program: glow::Program) -> Self {
        let u = |name: &str| uniform(gl, program, name);
        MaterialLocations {
            shading: u("u_shading"),
            base_color: u("u_base_color"),
            has_base: u("u_has_base"),
            emissive: u("u_emissive"),
            has_emissive: u("u_has_emissive"),
            cutoff: u("u_cutoff"),
        }
    }
}

/// The picture's colours in patches, for the light the screen gives.
struct Grid {
    framebuffer: glow::Framebuffer,
    texture: glow::Texture,
    size: (i32, i32),
    /// Nothing is in it yet.
    empty: bool,
}

pub struct Gpu {
    lit: glow::Program,
    sky: glow::Program,
    /// Only the depth, of what is in front: for the shadows, and first in
    /// each view, so that the lit program works out each pixel once.
    depth: glow::Program,
    grid_program: glow::Program,
    shadows: Option<Shadows>,
    grid: Grid,
    gi: Option<Gi>,
    ao: Option<Ao>,
    /// How long frames take the graphics chip, where they are timed.
    clock: Option<Clock>,
    /// How brightly the screen lights the room, times the scene's.
    glow: f32,
    /// For the sky's triangle, made from the vertex number.
    empty: glow::VertexArray,
    meshes: Vec<GpuMesh>,
    /// The unit cube the extras are drawn with.
    cube: GpuMesh,
    /// The scene's pictures, by their index.
    textures: Vec<glow::Texture>,
    target: Option<Target>,
    anisotropy: Option<f32>,
}

/// A program of the scene's, with `defines` before its shaders.
fn link(gl: &glow::Context, glsl: Glsl, defines: &str, vertex: &str, fragment: &str, name: &str) -> Result<glow::Program, String> {
    let preamble = glsl.preamble();
    let sources = [
        (glow::VERTEX_SHADER, format!("{}{}{}", preamble, defines, vertex)),
        (glow::FRAGMENT_SHADER, format!("{}{}{}{}", preamble, defines, COMMON, fragment)),
    ];
    // SAFETY: see `GlScreen`: the window's context is current.
    unsafe {
        let program = gl.create_program()?;
        let mut stages = Vec::new();
        let mut problem = None;
        for (kind, source) in sources {
            let stage = gl.create_shader(kind)?;
            gl.shader_source(stage, &source);
            gl.compile_shader(stage);
            gl.attach_shader(program, stage);
            stages.push(stage);
            if !gl.get_shader_compile_status(stage) {
                problem = Some(format!("the {} shader doesn't compile: {}", name, gl.get_shader_info_log(stage)));
                break;
            }
        }
        if problem.is_none() {
            gl.bind_attrib_location(program, 0, "a_position");
            gl.bind_attrib_location(program, 1, "a_normal");
            gl.bind_attrib_location(program, 2, "a_uv");
            gl.bind_frag_data_location(program, 0, "o_color");
            for i in 1..5 {
                gl.bind_frag_data_location(program, i, &format!("o_bake{}", i));
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                problem = Some(format!("the {} shader doesn't link: {}", name, gl.get_program_info_log(program)));
            }
        }
        for stage in stages {
            gl.detach_shader(program, stage);
            gl.delete_shader(stage);
        }
        if let Some(problem) = problem {
            gl.delete_program(program);
            return Err(problem);
        }
        Ok(program)
    }
}

/// Set a uniform of the program in use by name.
fn uniform(gl: &glow::Context, program: glow::Program, name: &str) -> Option<glow::UniformLocation> {
    // SAFETY: see `GlScreen`.
    unsafe { gl.get_uniform_location(program, name) }
}

impl Gpu {
    /// Delete everything it made, for another scene's: `gl` is the
    /// context it was made with, current.
    pub fn delete(self, gl: &glow::Context) {
        // SAFETY: see `GlScreen`.
        unsafe {
            for program in [self.lit, self.sky, self.depth, self.grid_program] {
                gl.delete_program(program);
            }
            if let Some(shadows) = self.shadows {
                gl.delete_texture(shadows.texture);
            }
            gl.delete_framebuffer(self.grid.framebuffer);
            gl.delete_texture(self.grid.texture);
            if let Some(gi) = self.gi {
                delete_gi(gl, gi);
            }
            if let Some(ao) = self.ao {
                gl.delete_program(ao.estimate);
                gl.delete_program(ao.blur);
                if let Some(target) = ao.target {
                    target.delete(gl);
                }
            }
            if let Some(clock) = self.clock {
                clock.delete(gl);
            }
            gl.delete_vertex_array(self.empty);
            for mesh in self.meshes.into_iter().chain([self.cube]) {
                gl.delete_vertex_array(mesh.vao);
                for buffer in mesh.buffers {
                    gl.delete_buffer(buffer);
                }
            }
            for texture in self.textures {
                gl.delete_texture(texture);
            }
            if let Some(target) = self.target {
                gl.delete_framebuffer(target.framebuffer);
                gl.delete_framebuffer(target.resolve);
                gl.delete_renderbuffer(target.color);
                gl.delete_renderbuffer(target.depth);
                gl.delete_texture(target.resolved);
            }
        }
    }

    /// Compile the programs and upload `scene`; `timed`, frames are timed
    /// (`frame_start`).
    pub fn new(gl: &glow::Context, glsl: Glsl, scene: &Scene, quality: VrQuality, timed: bool) -> Result<Self, String> {
        let (grid_size, taps, bounce, ao_samples) = lighting(quality);
        if glsl == Glsl::Es300 {
            return Err("the 3D view needs desktop OpenGL, not OpenGL ES".into());
        }
        let (lights, screen) = shadow::plan(scene);
        let layers = lights.iter().map(|(_, c)| c.layers().len()).sum::<usize>()
            + screen.as_ref().map_or(0, |c| c.layers().len());
        let defines = format!(
            "#define SHADOW_LAYERS {}\n#define SHADOW_TAPS {}\n#define GRID_W {}\n#define GRID_H {}\n#define AO_SAMPLES {}\n",
            layers.max(1),
            taps,
            grid_size.0,
            grid_size.1,
            ao_samples.max(1)
        );
        let mut programs = Vec::new();
        for (vertex, fragment, name) in [
            (MESH_VERT, LIT_FRAG, "scene"),
            (FULLSCREEN_VERT, SKY_FRAG, "sky"),
            (MESH_VERT, SHADOW_FRAG, "shadow"),
            (FULLSCREEN_VERT, GRID_FRAG, "screen light"),
        ] {
            match link(gl, glsl, &defines, vertex, fragment, name) {
                Ok(program) => programs.push(program),
                Err(e) => {
                    for program in programs {
                        // SAFETY: see `GlScreen`.
                        unsafe { gl.delete_program(program) };
                    }
                    return Err(e);
                }
            }
        }
        let [lit, sky, depth, grid_program] = programs[..] else { unreachable!() };
        let ao = if ao_samples > 0 && scene.ao {
            let estimate = link(gl, glsl, &defines, FULLSCREEN_VERT, AO_FRAG, "ambient occlusion");
            let blur = link(gl, glsl, &defines, FULLSCREEN_VERT, AO_BLUR_FRAG, "ambient occlusion's blur");
            match (estimate, blur) {
                (Ok(estimate), Ok(blur)) => Some(Ao { estimate, blur, target: None }),
                (estimate, blur) => {
                    for program in [&estimate, &blur].into_iter().flatten() {
                        // SAFETY: see `GlScreen`.
                        unsafe { gl.delete_program(*program) };
                    }
                    let problem = estimate.err().or(blur.err()).unwrap_or_default();
                    eprintln!("[VR] The ambient occlusion is left out: {}", problem);
                    None
                }
            }
        } else {
            None
        };
        let anisotropy = gl
            .supported_extensions()
            .iter()
            .any(|e| e == "GL_EXT_texture_filter_anisotropic" || e == "GL_ARB_texture_filter_anisotropic")
            // SAFETY: see `GlScreen`.
            .then(|| unsafe { gl.get_parameter_f32(MAX_TEXTURE_MAX_ANISOTROPY) }.min(16.0));
        // SAFETY: see `GlScreen`.
        let empty = unsafe { gl.create_vertex_array()? };
        let cube = upload_mesh(gl, &super::scene::unit_cube())?;
        let grid = make_grid(gl, grid_size)?;
        let mut gpu = Gpu {
            lit,
            sky,
            depth,
            grid_program,
            shadows: None,
            grid,
            gi: None,
            ao,
            clock: timed.then(Clock::new),
            glow: 1.0,
            empty,
            meshes: Vec::new(),
            cube,
            textures: Vec::new(),
            target: None,
            anisotropy,
        };
        gpu.upload(gl, scene)?;
        if layers > 0 {
            gpu.shadows = Some(gpu.bake_shadows(gl, scene, depth, &lights, screen.as_ref())?);
        }
        if scene.gi && bounce {
            let probes = scene.probes.get_or_init(|| {
                gpu.probes(gl, glsl, &defines, scene)
                    .map_err(|e| eprintln!("[VR] The light bouncing around the scene is left out: {}", e))
                    .ok()
            });
            if let Some(probes) = probes {
                match make_gi(gl, glsl, &defines, probes) {
                    Ok(gi) => gpu.gi = Some(gi),
                    Err(e) => eprintln!("[VR] The light bouncing around the scene is left out: {}", e),
                }
            }
        }
        Ok(gpu)
    }

    /// Draw the shadows' layers: the depth of what casts shadows, as each
    /// light sees it.
    fn bake_shadows(
        &self,
        gl: &glow::Context,
        scene: &Scene,
        program: glow::Program,
        lights: &[(usize, Caster)],
        screen: Option<&Caster>,
    ) -> Result<Shadows, String> {
        let mut layers = Vec::new();
        let mut by_light = vec![[-1, 0]; scene.lights.len()];
        for (i, caster) in lights {
            by_light[*i] = [layers.len() as i32, matches!(caster, Caster::Cube { .. }) as i32];
            layers.extend(caster.layers());
        }
        let screen_first = match screen {
            Some(caster) => {
                let first = layers.len() as i32;
                layers.extend(caster.layers());
                first
            }
            None => -1,
        };
        // Finer when there are few.
        let size: u32 = if layers.len() <= 8 { 2048 } else { 1024 };
        let u = |name: &str| uniform(gl, program, name);
        // SAFETY: see `GlScreen`.
        unsafe {
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(texture));
            let none = glow::PixelUnpackData::Slice(None);
            let (s, n) = (size as i32, layers.len() as i32);
            gl.tex_image_3d(glow::TEXTURE_2D_ARRAY, 0, glow::DEPTH_COMPONENT24 as i32, s, s, n, 0, glow::DEPTH_COMPONENT, glow::UNSIGNED_INT, none);
            for (key, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_COMPARE_MODE, glow::COMPARE_REF_TO_TEXTURE),
                (glow::TEXTURE_COMPARE_FUNC, glow::LEQUAL),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, key, value as i32);
            }
            let framebuffer = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.draw_buffer(glow::NONE);
            gl.read_buffer(glow::NONE);
            gl.viewport(0, 0, s, s);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.disable(glow::CULL_FACE);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            gl.depth_mask(true);
            gl.enable(glow::POLYGON_OFFSET_FILL);
            gl.polygon_offset(1.1, 2.0);
            gl.use_program(Some(program));
            gl.uniform_matrix_4_f32_slice(u("u_model").as_ref(), false, &Mat4::IDENTITY.to_cols_array());
            gl.uniform_1_i32(u("u_base").as_ref(), 0);
            gl.active_texture(glow::TEXTURE0);
            let (view_projection, base_color, has_base, cutoff) =
                (u("u_view_projection"), u("u_base_color"), u("u_has_base"), u("u_cutoff"));
            let mut problem = None;
            for (index, layer) in layers.iter().enumerate() {
                gl.framebuffer_texture_layer(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, Some(texture), 0, index as i32);
                if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                    problem = Some("OpenGL can't draw the shadows".to_string());
                    break;
                }
                gl.clear_depth_f32(1.0);
                gl.clear(glow::DEPTH_BUFFER_BIT);
                gl.uniform_matrix_4_f32_slice(view_projection.as_ref(), false, &layer.view_projection.to_cols_array());
                for mesh in &self.meshes {
                    let material = &scene.materials[mesh.material];
                    if !material.casts_shadow() {
                        continue;
                    }
                    let cut = match material.alpha {
                        Alpha::Mask(cut) => cut,
                        _ => -1.0,
                    };
                    gl.uniform_1_f32(cutoff.as_ref(), cut);
                    if cut >= 0.0 {
                        let bound = material.base_texture.and_then(|t| self.textures.get(t.image).copied());
                        gl.uniform_4_f32_slice(base_color.as_ref(), &material.base_color);
                        gl.uniform_1_i32(has_base.as_ref(), bound.is_some() as i32);
                        gl.bind_texture(glow::TEXTURE_2D, bound);
                    }
                    gl.bind_vertex_array(Some(mesh.vao));
                    gl.draw_elements(glow::TRIANGLES, mesh.count, glow::UNSIGNED_INT, 0);
                }
            }
            gl.disable(glow::POLYGON_OFFSET_FILL);
            gl.draw_buffer(glow::COLOR_ATTACHMENT0);
            gl.read_buffer(glow::COLOR_ATTACHMENT0);
            gl.delete_framebuffer(framebuffer);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
            restore(gl);
            if let Some(problem) = problem {
                gl.delete_texture(texture);
                return Err(problem);
            }
            Ok(Shadows { texture, size, layers, lights: by_light, screen: screen_first })
        }
    }

    /// Take the light the screen gives from its new picture `screen`, and
    /// the light it bounces around `scene`: once for each picture, before
    /// the views are drawn.
    pub fn prepare(&mut self, gl: &glow::Context, scene: &Scene, screen: glow::Texture) {
        let program = self.grid_program;
        let take = if self.grid.empty { 1.0 } else { GRID_TAKE };
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.grid.framebuffer));
            // And a column for the whole picture's.
            gl.viewport(0, 0, self.grid.size.0 + 1, self.grid.size.1);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::FRAMEBUFFER_SRGB);
            gl.color_mask(true, true, true, true);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
            gl.use_program(Some(program));
            gl.uniform_1_i32(uniform(gl, program, "u_screen").as_ref(), 0);
            gl.uniform_1_f32(uniform(gl, program, "u_take").as_ref(), take);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(screen));
            gl.bind_vertex_array(Some(self.empty));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            restore(gl);
        }
        self.grid.empty = false;
        if let Some(gi) = &mut self.gi {
            gi.stale = true;
        }
        self.update_gi(gl, scene);
        self.mark(gl, Pass::Light);
    }

    /// A frame begins, to be timed if frames are.
    pub fn frame_start(&mut self, gl: &glow::Context) {
        if let Some(clock) = &mut self.clock {
            clock.start(gl);
        }
    }

    /// `pass` of the frame ends.
    pub fn mark(&mut self, gl: &glow::Context, pass: Pass) {
        if let Some(clock) = &mut self.clock {
            clock.mark(gl, pass);
        }
    }

    /// The frame is drawn.
    pub fn frame_end(&mut self) {
        if let Some(clock) = &mut self.clock {
            clock.end();
        }
    }

    /// The frames' times since the last report, in a line.
    pub fn timing_report(&mut self) -> Option<String> {
        self.clock.as_mut()?.report()
    }

    /// Set the lighting's uniforms of `program` (the scene's or the bake's)
    /// in use, and bind its textures, for `scene` with `screen` showing on
    /// its screen. `srgb`: the target encodes linear light itself.
    fn bind_lighting(&self, gl: &glow::Context, program: glow::Program, scene: &Scene, screen: Option<glow::Texture>, srgb: bool) {
        let u = |name: &str| uniform(gl, program, name);
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.uniform_3_f32_slice(u("u_sun").as_ref(), &scene.sun.to_array());
            gl.uniform_1_i32(u("u_sky").as_ref(), scene.sky as i32);
            gl.uniform_1_i32(u("u_encode").as_ref(), !srgb as i32);
            gl.uniform_3_f32_slice(u("u_ambient").as_ref(), &scene.ambient.to_array());
            gl.uniform_1_f32(u("u_exposure").as_ref(), scene.exposure);
            let lights = &scene.lights[..scene.lights.len().min(super::scene::MAX_LIGHTS)];
            let mut pos = Vec::new();
            let mut color = Vec::new();
            let mut dir = Vec::new();
            let mut cone = Vec::new();
            for light in lights {
                use super::scene::LightKind;
                let (kind, (inner, outer)) = match light.kind {
                    LightKind::Directional => (0.0, (1.0, 0.0)),
                    LightKind::Point => (1.0, (1.0, 0.0)),
                    LightKind::Spot { inner_cos, outer_cos } => (2.0, (inner_cos, outer_cos)),
                };
                pos.extend(light.position.extend(kind).to_array());
                color.extend(light.color.to_array());
                dir.extend(light.direction.to_array());
                cone.extend([inner, outer]);
            }
            gl.uniform_1_i32(u("u_lights").as_ref(), lights.len() as i32);
            if !lights.is_empty() {
                gl.uniform_4_f32_slice(u("u_light_pos[0]").as_ref(), &pos);
                gl.uniform_3_f32_slice(u("u_light_color[0]").as_ref(), &color);
                gl.uniform_3_f32_slice(u("u_light_dir[0]").as_ref(), &dir);
                gl.uniform_2_f32_slice(u("u_light_cone[0]").as_ref(), &cone);
            }
            gl.uniform_1_i32(u("u_base").as_ref(), 0);
            gl.uniform_1_i32(u("u_emissive_map").as_ref(), 1);
            gl.uniform_1_i32(u("u_screen").as_ref(), 2);
            gl.uniform_1_i32(u("u_glow_grid").as_ref(), 3);
            gl.uniform_1_i32(u("u_shadows").as_ref(), 4);
            gl.uniform_1_i32(u("u_gi").as_ref(), 5);
            gl.uniform_1_i32(u("u_has_screen").as_ref(), screen.is_some() as i32);
            let s = &scene.screen;
            let (origin, across, down) = s.frame();
            gl.uniform_3_f32_slice(u("u_glow_origin").as_ref(), &origin.to_array());
            gl.uniform_3_f32_slice(u("u_glow_across").as_ref(), &across.to_array());
            gl.uniform_3_f32_slice(u("u_glow_down").as_ref(), &down.to_array());
            gl.uniform_1_f32(u("u_glow").as_ref(), self.glow_strength(scene));
            let point = s.center + s.normal * shadow::SCREEN_OFFSET;
            gl.uniform_3_f32_slice(u("u_glow_point").as_ref(), &point.to_array());
            let basis = glam::Mat3::from_cols(s.right, s.up(), s.normal);
            gl.uniform_matrix_3_f32_slice(u("u_glow_basis").as_ref(), false, &basis.to_cols_array());
            gl.uniform_2_f32_slice(u("u_glow_size").as_ref(), &s.size.to_array());
            let mut light_shadows = vec![[-1, 0]; lights.len()];
            match &self.shadows {
                Some(shadows) => {
                    for (to, from) in light_shadows.iter_mut().zip(&shadows.lights) {
                        *to = *from;
                    }
                    let matrices: Vec<f32> = shadows.layers.iter().flat_map(|l| l.view_projection.to_cols_array()).collect();
                    let texels: Vec<f32> = shadows.layers.iter().flat_map(|l| l.texel).collect();
                    gl.uniform_matrix_4_f32_slice(u("u_shadow_matrix[0]").as_ref(), false, &matrices);
                    gl.uniform_2_f32_slice(u("u_shadow_texel[0]").as_ref(), &texels);
                    gl.uniform_1_f32(u("u_shadow_size").as_ref(), shadows.size as f32);
                    gl.uniform_1_i32(u("u_glow_shadow").as_ref(), shadows.screen);
                }
                None => gl.uniform_1_i32(u("u_glow_shadow").as_ref(), -1),
            }
            if !lights.is_empty() {
                let flat: Vec<i32> = light_shadows.iter().flatten().copied().collect();
                gl.uniform_2_i32_slice(u("u_light_shadow[0]").as_ref(), &flat);
            }
            gl.uniform_1_i32(u("u_has_gi").as_ref(), self.gi.is_some() as i32);
            if let Some(gi) = &self.gi {
                let [nx, ny, nz] = gi.layout.count.map(|c| c as f32);
                gl.uniform_3_f32_slice(u("u_gi_low").as_ref(), &gi.layout.low.to_array());
                gl.uniform_3_f32_slice(u("u_gi_step").as_ref(), &gi.layout.step.to_array());
                gl.uniform_3_f32_slice(u("u_gi_count").as_ref(), &[nx, ny, nz]);
            }
            gl.active_texture(glow::TEXTURE5);
            gl.bind_texture(glow::TEXTURE_3D, self.gi.as_ref().map(|g| g.light));
            gl.active_texture(glow::TEXTURE4);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, self.shadows.as_ref().map(|s| s.texture));
            gl.active_texture(glow::TEXTURE3);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.grid.texture));
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, screen);
        }
    }

    /// The ambient occlusion of `view`, `size` big, at half that: None
    /// without it.
    fn draw_ao(&mut self, gl: &glow::Context, scene: &Scene, view: &View, size: (u32, u32)) -> Option<glow::Texture> {
        let half = (((size.0 / 2).max(1)) as i32, ((size.1 / 2).max(1)) as i32);
        let ao = self.ao.as_mut()?;
        if ao.target.as_ref().is_some_and(|t| t.size != half) {
            ao.target.take().expect("the target").delete(gl);
        }
        if ao.target.is_none() {
            match AoTarget::new(gl, half) {
                Ok(target) => ao.target = Some(target),
                Err(e) => {
                    eprintln!("[VR] The ambient occlusion is left out: {}", e);
                    self.ao = None;
                    return None;
                }
            }
        }
        let (estimate, blur) = (ao.estimate, ao.blur);
        let target = ao.target.as_ref().expect("the target");
        let (depth, depth_framebuffer, pictures, framebuffers) = (target.depth, target.depth_framebuffer, target.pictures, target.framebuffers);
        let (w, h) = half;
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(depth_framebuffer));
            gl.viewport(0, 0, w, h);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.disable(glow::FRAMEBUFFER_SRGB);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            gl.depth_mask(true);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            self.draw_depth(gl, scene, &view.view_projection());
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.bind_vertex_array(Some(self.empty));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(depth));
            let inverse = view.projection.inverse().to_cols_array();

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffers[0]));
            gl.use_program(Some(estimate));
            let u = |name: &str| uniform(gl, estimate, name);
            gl.uniform_1_i32(u("u_depth").as_ref(), 1);
            gl.uniform_matrix_4_f32_slice(u("u_projection").as_ref(), false, &view.projection.to_cols_array());
            gl.uniform_matrix_4_f32_slice(u("u_inverse_projection").as_ref(), false, &inverse);
            gl.uniform_2_f32_slice(u("u_size").as_ref(), &[w as f32, h as f32]);
            gl.uniform_1_f32(u("u_radius").as_ref(), AO_RADIUS);
            gl.uniform_1_f32(u("u_strength").as_ref(), AO_STRENGTH);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);

            // Across into the second picture, then down back into the first.
            gl.use_program(Some(blur));
            let u = |name: &str| uniform(gl, blur, name);
            gl.uniform_1_i32(u("u_ao").as_ref(), 0);
            gl.uniform_1_i32(u("u_depth").as_ref(), 1);
            gl.uniform_matrix_4_f32_slice(u("u_inverse_projection").as_ref(), false, &inverse);
            gl.uniform_2_f32_slice(u("u_size").as_ref(), &[w as f32, h as f32]);
            gl.active_texture(glow::TEXTURE0);
            for (from, to, step) in [(0, 1, [1.0 / w as f32, 0.0]), (1, 0, [0.0, 1.0 / h as f32])] {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffers[to]));
                gl.bind_texture(glow::TEXTURE_2D, Some(pictures[from]));
                gl.uniform_2_f32_slice(u("u_step").as_ref(), &step);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, None);
            restore(gl);
        }
        Some(pictures[0])
    }

    /// Draw the depth of the opaque meshes as `view_projection` sees
    /// them.
    fn draw_depth(&self, gl: &glow::Context, scene: &Scene, view_projection: &Mat4) {
        let program = self.depth;
        let u = |name: &str| uniform(gl, program, name);
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.use_program(Some(program));
            gl.uniform_matrix_4_f32_slice(u("u_view_projection").as_ref(), false, &view_projection.to_cols_array());
            gl.uniform_matrix_4_f32_slice(u("u_model").as_ref(), false, &Mat4::IDENTITY.to_cols_array());
            gl.uniform_1_i32(u("u_base").as_ref(), 0);
            let (base_color, has_base, cutoff) = (u("u_base_color"), u("u_has_base"), u("u_cutoff"));
            gl.active_texture(glow::TEXTURE0);
            for mesh in &self.meshes {
                let material = &scene.materials[mesh.material];
                if material.alpha == Alpha::Blend {
                    continue;
                }
                let cut = match material.alpha {
                    Alpha::Mask(cut) => cut,
                    _ => -1.0,
                };
                gl.uniform_1_f32(cutoff.as_ref(), cut);
                if cut >= 0.0 {
                    let bound = material.base_texture.and_then(|t| self.textures.get(t.image).copied());
                    gl.uniform_4_f32_slice(base_color.as_ref(), &material.base_color);
                    gl.uniform_1_i32(has_base.as_ref(), bound.is_some() as i32);
                    gl.bind_texture(glow::TEXTURE_2D, bound);
                }
                if material.double_sided {
                    gl.disable(glow::CULL_FACE);
                } else {
                    gl.enable(glow::CULL_FACE);
                    gl.cull_face(glow::BACK);
                }
                gl.bind_vertex_array(Some(mesh.vao));
                gl.draw_elements(glow::TRIANGLES, mesh.count, glow::UNSIGNED_INT, 0);
            }
        }
    }

    /// How brightly the screen lights the room, times the scene's.
    pub fn set_glow(&mut self, glow: f32) {
        if glow != self.glow {
            self.glow = glow;
            if let Some(gi) = &mut self.gi {
                gi.stale = true;
            }
        }
    }

    /// How brightly the screen lights the room now: not before its first
    /// picture.
    fn glow_strength(&self, scene: &Scene) -> f32 {
        if self.grid.empty || scene.screen.size.x <= 0.0 { 0.0 } else { scene.screen_glow * self.glow }
    }

    /// Set a mesh's material's uniforms, and bind its textures to units 0
    /// and 1; glowing if `glow`.
    fn bind_material(&self, gl: &glow::Context, at: &MaterialLocations, material: &super::scene::Material, glow: bool) {
        let kind = match material.shading {
            Shading::Lit => 0,
            Shading::Unlit => 1,
            Shading::Screen => 2,
            Shading::Floor => 3,
        };
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.uniform_1_i32(at.shading.as_ref(), kind);
            gl.uniform_4_f32_slice(at.base_color.as_ref(), &material.base_color);
            let glow = if glow { 1.0 } else { 0.0 };
            gl.uniform_3_f32_slice(at.emissive.as_ref(), &material.emissive.map(|c| c * glow));
            let cut = match material.alpha {
                Alpha::Mask(cut) => cut,
                _ => -1.0,
            };
            gl.uniform_1_f32(at.cutoff.as_ref(), cut);
            for (unit, texture, flag) in [(0, material.base_texture, &at.has_base), (1, material.emissive_texture, &at.has_emissive)] {
                let bound = texture.and_then(|t| Some((self.textures.get(t.image).copied()?, t.repeat)));
                gl.uniform_1_i32(flag.as_ref(), bound.is_some() as i32);
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(glow::TEXTURE_2D, bound.map(|(t, _)| t));
                if let Some((_, repeat)) = bound {
                    let wrap = if repeat { glow::REPEAT } else { glow::CLAMP_TO_EDGE } as i32;
                    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, wrap);
                    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, wrap);
                }
            }
        }
    }

    /// The probes of `scene`: as worked out before for it, or from the
    /// cache, or worked out now, in two passes: the second sees the
    /// surfaces lit also by the light the first found bouncing around.
    fn probes(&mut self, gl: &glow::Context, glsl: Glsl, defines: &str, scene: &Scene) -> Result<gi::Probes, String> {
        let (low, high) = scene.focus_bounds();
        let layout = gi::Layout::inside(low, high);
        let patches = (self.grid.size.0 * self.grid.size.1) as usize;
        let key = gi::Probes::key(scene.source, &layout, patches);
        let path = gi::Probes::cache_path(key);
        if let Some(probes) = path.as_ref().and_then(|p| std::fs::read(p).ok()).and_then(|b| gi::Probes::from_bytes(&b))
            && probes.layout == layout
            && probes.patches == patches
        {
            return Ok(probes);
        }
        let started = std::time::Instant::now();
        let program = link(gl, glsl, &format!("{}#define BAKE 1\n", defines), MESH_VERT, LIT_FRAG, "light bake")?;
        let probes = self.bake_probes(gl, program, scene, layout, patches).and_then(|first| {
            self.gi = Some(make_gi(gl, glsl, defines, &first)?);
            self.update_gi(gl, scene);
            let second = self.bake_probes(gl, program, scene, layout, patches);
            if let Some(gi) = self.gi.take() {
                delete_gi(gl, gi);
            }
            second
        });
        // SAFETY: see `GlScreen`.
        unsafe { gl.delete_program(program) };
        let probes = probes?;
        eprintln!("[VR] Worked out the light bouncing around the scene ({} probes) in {:.1} s", layout.len(), started.elapsed().as_secs_f32());
        if let Some(path) = path {
            let saved = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, probes.to_bytes()));
            if let Err(e) = saved {
                eprintln!("[VR] Can't keep the scene's light in {}: {}", path.display(), e);
            }
            if let Some(dir) = path.parent() {
                gi::Probes::prune(dir);
            }
        }
        Ok(probes)
    }

    /// Draw what each probe sees around it, a batch at a time side by side
    /// in one picture, read it back and sum it up.
    fn bake_probes(&self, gl: &glow::Context, program: glow::Program, scene: &Scene, layout: gi::Layout, patches: usize) -> Result<gi::Probes, String> {
        const ACROSS: usize = 16;
        const DOWN: usize = 64;
        let face = gi::FACE;
        let (w, h) = ((ACROSS * 6 * face) as i32, (DOWN * face) as i32);
        let directions = gi::texel_directions();
        let mut probes = Vec::with_capacity(layout.len());
        // SAFETY: see `GlScreen`.
        unsafe {
            let framebuffer = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            let mut pictures = Vec::new();
            for i in 0..5 {
                let picture = gl.create_renderbuffer()?;
                gl.bind_renderbuffer(glow::RENDERBUFFER, Some(picture));
                gl.renderbuffer_storage(glow::RENDERBUFFER, glow::RGBA32F, w, h);
                gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0 + i, glow::RENDERBUFFER, Some(picture));
                pictures.push(picture);
            }
            let depth = gl.create_renderbuffer()?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH_COMPONENT24, w, h);
            gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, glow::RENDERBUFFER, Some(depth));
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);
            let attachments: Vec<u32> = (0..5).map(|i| glow::COLOR_ATTACHMENT0 + i).collect();
            gl.draw_buffers(&attachments);
            let complete = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            let mut result = Err("OpenGL can't draw what the scene's probes see".to_string());
            if complete {
                gl.disable(glow::SCISSOR_TEST);
                gl.disable(glow::BLEND);
                gl.disable(glow::CULL_FACE);
                gl.disable(glow::FRAMEBUFFER_SRGB);
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(glow::LESS);
                gl.depth_mask(true);
                gl.color_mask(true, true, true, true);
                gl.use_program(Some(program));
                self.bind_lighting(gl, program, scene, None, true);
                gl.uniform_matrix_4_f32_slice(uniform(gl, program, "u_model").as_ref(), false, &Mat4::IDENTITY.to_cols_array());
                let view_projection = uniform(gl, program, "u_view_projection");
                let backs = uniform(gl, program, "u_backs");
                let locations = MaterialLocations::of(gl, program);
                let indices: Vec<usize> = (0..layout.len()).collect();
                let mut pixels = vec![vec![0u8; (w * h * 16) as usize]; 5];
                for batch in indices.chunks(ACROSS * DOWN) {
                    gl.viewport(0, 0, w, h);
                    gl.clear_buffer_f32_slice(glow::COLOR, 0, &[0.0, 0.0, 0.0, 1.0]);
                    for i in 1..5 {
                        gl.clear_buffer_f32_slice(glow::COLOR, i, &[0.0; 4]);
                    }
                    gl.clear_buffer_f32_slice(glow::DEPTH, 0, &[1.0]);
                    for (slot, &index) in batch.iter().enumerate() {
                        let views = gi::face_views(layout.position(index));
                        for (f, view) in views.iter().enumerate() {
                            let x = ((slot % ACROSS) * 6 + f) * face;
                            let y = slot / ACROSS * face;
                            gl.viewport(x as i32, y as i32, face as i32, face as i32);
                            gl.uniform_matrix_4_f32_slice(view_projection.as_ref(), false, &view.to_cols_array());
                            for mesh in &self.meshes {
                                let material = &scene.materials[mesh.material];
                                if material.alpha == Alpha::Blend {
                                    continue;
                                }
                                // The PC's lights as when it is off.
                                self.bind_material(gl, &locations, material, material.led.is_none());
                                gl.uniform_1_i32(backs.as_ref(), !material.double_sided as i32);
                                gl.bind_vertex_array(Some(mesh.vao));
                                gl.draw_elements(glow::TRIANGLES, mesh.count, glow::UNSIGNED_INT, 0);
                            }
                        }
                    }
                    let rows = (batch.len().div_ceil(ACROSS) * face) as i32;
                    for (i, buffer) in pixels.iter_mut().enumerate() {
                        gl.read_buffer(glow::COLOR_ATTACHMENT0 + i as u32);
                        let size = (w * rows * 16) as usize;
                        let out = glow::PixelPackData::Slice(Some(&mut buffer[..size]));
                        gl.read_pixels(0, 0, w, rows, glow::RGBA, glow::FLOAT, out);
                    }
                    let texel = |picture: &[u8], x: usize, y: usize| -> [f32; 4] {
                        let at = (y * w as usize + x) * 16;
                        std::array::from_fn(|c| f32::from_ne_bytes(picture[at + c * 4..at + c * 4 + 4].try_into().unwrap()))
                    };
                    for slot in 0..batch.len() {
                        let seen = (0..6 * face * face).map(|t| {
                            let (f, j, i) = (t / (face * face), t / face % face, t % face);
                            let x = ((slot % ACROSS) * 6 + f) * face + i;
                            let y = slot / ACROSS * face + j;
                            let [a, b, c] = [2, 3, 4].map(|p| texel(&pixels[p], x, y));
                            let mut patches = [0.0; 12];
                            patches[..4].copy_from_slice(&a);
                            patches[4..8].copy_from_slice(&b);
                            patches[8..].copy_from_slice(&c);
                            gi::Seen { light: texel(&pixels[0], x, y), color: texel(&pixels[1], x, y), patches }
                        });
                        probes.push(gi::integrate(&directions, seen, patches));
                    }
                }
                result = Ok(());
            }
            gl.draw_buffer(glow::COLOR_ATTACHMENT0);
            gl.read_buffer(glow::COLOR_ATTACHMENT0);
            gl.delete_framebuffer(framebuffer);
            for picture in pictures {
                gl.delete_renderbuffer(picture);
            }
            gl.delete_renderbuffer(depth);
            restore(gl);
            result?;
        }
        Ok(gi::Probes { layout, patches, probes })
    }

    /// The probes' light for the screen's picture now, if it changed.
    fn update_gi(&mut self, gl: &glow::Context, scene: &Scene) {
        let strength = self.glow_strength(scene);
        let Some(gi) = &mut self.gi else { return };
        if !gi.stale {
            return;
        }
        gi.stale = false;
        let program = gi.program;
        let [nx, ny, nz] = gi.layout.count.map(|c| c as i32);
        let u = |name: &str| uniform(gl, program, name);
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(gi.framebuffer));
            gl.viewport(0, 0, 6 * nx, ny);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            gl.disable(glow::FRAMEBUFFER_SRGB);
            gl.color_mask(true, true, true, true);
            gl.use_program(Some(program));
            gl.uniform_1_i32(u("u_probes").as_ref(), 0);
            gl.uniform_1_i32(u("u_glow_grid").as_ref(), 1);
            gl.uniform_1_f32(u("u_glow").as_ref(), strength);
            gl.uniform_3_f32_slice(u("u_ambient").as_ref(), &scene.ambient.to_array());
            gl.uniform_1_i32(u("u_sky").as_ref(), scene.sky as i32);
            gl.uniform_3_i32(u("u_count").as_ref(), nx, ny, nz);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.grid.texture));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(gi.probes));
            gl.bind_vertex_array(Some(self.empty));
            let z_at = u("u_z");
            for z in 0..nz {
                gl.framebuffer_texture_layer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, Some(gi.light), 0, z);
                gl.uniform_1_i32(z_at.as_ref(), z);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
            restore(gl);
        }
    }

    /// The largest anisotropic filtering, where OpenGL has it, for the
    /// texture bound.
    pub fn set_anisotropy(&self, gl: &glow::Context) {
        if let Some(most) = self.anisotropy {
            // SAFETY: see `GlScreen`.
            unsafe { gl.tex_parameter_f32(glow::TEXTURE_2D, TEXTURE_MAX_ANISOTROPY, most) };
        }
    }

    fn upload(&mut self, gl: &glow::Context, scene: &Scene) -> Result<(), String> {
        // SAFETY: see `GlScreen`.
        unsafe {
            for image in &scene.images {
                let texture = gl.create_texture()?;
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
                let (w, h) = (image.width as i32, image.height as i32);
                let pixels = glow::PixelUnpackData::Slice(Some(&image.rgba));
                // The colours are sRGB, which sampling turns into linear
                // light.
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::SRGB8_ALPHA8 as i32, w, h, 0, glow::RGBA, glow::UNSIGNED_BYTE, pixels);
                gl.generate_mipmap(glow::TEXTURE_2D);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                self.set_anisotropy(gl);
                self.textures.push(texture);
            }
        }
        for mesh in &scene.meshes {
            self.meshes.push(upload_mesh(gl, mesh)?);
        }
        Ok(())
    }

    /// The target for `format`, made anew if it is of another.
    fn target(&mut self, gl: &glow::Context, format: Format) -> Result<&Target, String> {
        if self.target.as_ref().is_some_and(|t| t.format != format) {
            let old = self.target.take().expect("the target");
            // SAFETY: see `GlScreen`.
            unsafe {
                gl.delete_framebuffer(old.framebuffer);
                gl.delete_framebuffer(old.resolve);
                gl.delete_renderbuffer(old.color);
                gl.delete_renderbuffer(old.depth);
                gl.delete_texture(old.resolved);
            }
        }
        if self.target.is_none() {
            self.target = Some(make_target(gl, format)?);
        }
        Ok(self.target.as_ref().expect("the target"))
    }

    /// Draw `scene` as `view` sees it into the target for `format`, with
    /// `screen` the picture on the screen, the PC's lights as `leds` and
    /// `extras` over it all. An sRGB target is written linear light, which
    /// it encodes; another sRGB values.
    pub fn render(
        &mut self,
        gl: &glow::Context,
        scene: &Scene,
        view: &View,
        format: Format,
        screen: Option<glow::Texture>,
        leds: Leds,
        extras: &[Extra],
    ) -> Result<(), String> {
        // (After a change of the screen's brightness.)
        self.update_gi(gl, scene);
        let ao = self.draw_ao(gl, scene, view, format.size);
        if ao.is_some() {
            self.mark(gl, Pass::Ao);
        }
        let (lit, sky, empty) = (self.lit, self.sky, self.empty);
        let target = self.target(gl, format)?;
        let (framebuffer, resolve) = (target.framebuffer, target.resolve);
        let (w, h) = (format.size.0 as i32, format.size.1 as i32);
        let encode = !format.srgb as i32;
        let u = |program, name: &str| uniform(gl, program, name);
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.viewport(0, 0, w, h);
            if format.srgb {
                gl.enable(glow::FRAMEBUFFER_SRGB);
            }
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.color_mask(true, true, true, true);
            gl.depth_mask(true);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);

            // The sky, at no distance: everything goes over it.
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.use_program(Some(sky));
            let mut rotation = view.view;
            rotation.w_axis = glam::Vec4::W;
            let inverse = (view.projection * rotation).inverse();
            gl.uniform_matrix_4_f32_slice(u(sky, "u_inverse").as_ref(), false, &inverse.to_cols_array());
            gl.uniform_3_f32_slice(u(sky, "u_sun").as_ref(), &scene.sun.to_array());
            gl.uniform_1_i32(u(sky, "u_sky").as_ref(), scene.sky as i32);
            gl.uniform_1_i32(u(sky, "u_encode").as_ref(), encode);
            gl.bind_vertex_array(Some(empty));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);

            // The depth of the opaque meshes, then their colour where they
            // are in front.
            let vp = view.view_projection();
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            gl.color_mask(false, false, false, false);
            self.draw_depth(gl, scene, &vp);
            gl.color_mask(true, true, true, true);
            gl.depth_mask(false);
            gl.depth_func(glow::LEQUAL);
            gl.use_program(Some(lit));
            gl.uniform_matrix_4_f32_slice(u(lit, "u_view_projection").as_ref(), false, &vp.to_cols_array());
            let model = u(lit, "u_model");
            gl.uniform_matrix_4_f32_slice(model.as_ref(), false, &Mat4::IDENTITY.to_cols_array());
            self.bind_lighting(gl, lit, scene, screen, format.srgb);
            gl.uniform_3_f32_slice(u(lit, "u_eye").as_ref(), &view.eye().to_array());
            gl.uniform_1_f32(u(lit, "u_fog").as_ref(), if scene.sky { 0.02 } else { 0.0 });
            let has_ao = u(lit, "u_has_ao");
            gl.uniform_1_i32(has_ao.as_ref(), ao.is_some() as i32);
            gl.uniform_1_i32(u(lit, "u_ao").as_ref(), 6);
            gl.uniform_2_f32_slice(u(lit, "u_view_size").as_ref(), &[w as f32, h as f32]);
            gl.active_texture(glow::TEXTURE6);
            gl.bind_texture(glow::TEXTURE_2D, ao);

            let locations = MaterialLocations::of(gl, lit);
            let MaterialLocations { shading, base_color, has_base, emissive, has_emissive, cutoff } = &locations;
            // The opaque meshes, then those seen through.
            for blended in [false, true] {
                if blended {
                    gl.enable(glow::BLEND);
                    gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                    gl.uniform_1_i32(has_ao.as_ref(), 0);
                }
                for mesh in &self.meshes {
                    let material = &scene.materials[mesh.material];
                    if (material.alpha == Alpha::Blend) != blended {
                        continue;
                    }
                    let glow = material.led.is_none_or(|led| super::scene::lit(leds, led));
                    self.bind_material(gl, &locations, material, glow);
                    if material.double_sided {
                        gl.disable(glow::CULL_FACE);
                    } else {
                        gl.enable(glow::CULL_FACE);
                        gl.cull_face(glow::BACK);
                    }
                    gl.bind_vertex_array(Some(mesh.vao));
                    gl.draw_elements(glow::TRIANGLES, mesh.count, glow::UNSIGNED_INT, 0);
                }
            }

            // The extras, unlit, seen through where they are.
            gl.uniform_1_i32(shading.as_ref(), 1);
            gl.uniform_3_f32_slice(emissive.as_ref(), &[0.0; 3]);
            gl.uniform_1_f32(cutoff.as_ref(), -1.0);
            gl.uniform_1_i32(has_base.as_ref(), 0);
            gl.uniform_1_i32(has_emissive.as_ref(), 0);
            gl.disable(glow::CULL_FACE);
            gl.bind_vertex_array(Some(self.cube.vao));
            for extra in extras {
                gl.uniform_matrix_4_f32_slice(model.as_ref(), false, &extra.model.to_cols_array());
                gl.uniform_4_f32_slice(base_color.as_ref(), &extra.color);
                gl.draw_elements(glow::TRIANGLES, self.cube.count, glow::UNSIGNED_INT, 0);
            }

            // The samples resolved into the texture.
            gl.disable(glow::FRAMEBUFFER_SRGB);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(framebuffer));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(resolve));
            gl.blit_framebuffer(0, 0, w, h, 0, 0, w, h, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            restore(gl);
        }
        self.mark(gl, Pass::Scene);
        Ok(())
    }

    /// Copy the last view drawn into the `x, y, width, height` of
    /// `framebuffer` (None for the window's; y from the bottom), scaled
    /// smoothly if it is of another size.
    pub fn copy_to(&self, gl: &glow::Context, framebuffer: Option<glow::Framebuffer>, (x, y, w, h): (i32, i32, i32, i32)) {
        let Some(target) = &self.target else { return };
        let (sw, sh) = (target.format.size.0 as i32, target.format.size.1 as i32);
        let filter = if (sw, sh) == (w, h) { glow::NEAREST } else { glow::LINEAR };
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(target.resolve));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, framebuffer);
            gl.blit_framebuffer(0, 0, sw, sh, x, y, x + w, y + h, glow::COLOR_BUFFER_BIT, filter);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }
}

/// A mesh's vertices and indices in buffers of OpenGL's.
fn upload_mesh(gl: &glow::Context, mesh: &super::scene::Mesh) -> Result<GpuMesh, String> {
    let vertices: Vec<u8> = mesh
        .vertices
        .iter()
        .flat_map(|v| v.position.into_iter().chain(v.normal).chain(v.uv))
        .flat_map(f32::to_ne_bytes)
        .collect();
    let indices: Vec<u8> = mesh.indices.iter().flat_map(|i| i.to_ne_bytes()).collect();
    // SAFETY: see `GlScreen`.
    unsafe {
        let vao = gl.create_vertex_array()?;
        gl.bind_vertex_array(Some(vao));
        let vbo = gl.create_buffer()?;
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &vertices, glow::STATIC_DRAW);
        let ebo = gl.create_buffer()?;
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
        gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, &indices, glow::STATIC_DRAW);
        let stride = 8 * 4;
        for (index, size, offset) in [(0, 3, 0), (1, 3, 12), (2, 2, 24)] {
            gl.enable_vertex_attrib_array(index);
            gl.vertex_attrib_pointer_f32(index, size, glow::FLOAT, false, stride, offset);
        }
        gl.bind_vertex_array(None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        Ok(GpuMesh { vao, buffers: [vbo, ebo], count: mesh.indices.len() as i32, material: mesh.material })
    }
}

/// Leave OpenGL as the flat picture's drawing expects it.
fn restore(gl: &glow::Context) {
    // SAFETY: see `GlScreen`.
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
        gl.disable(glow::BLEND);
        gl.disable(glow::FRAMEBUFFER_SRGB);
        gl.depth_mask(true);
        gl.bind_vertex_array(None);
        gl.use_program(None);
        gl.active_texture(glow::TEXTURE6);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.active_texture(glow::TEXTURE5);
        gl.bind_texture(glow::TEXTURE_3D, None);
        gl.active_texture(glow::TEXTURE4);
        gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
        for unit in [3, 2, 1, 0] {
            gl.active_texture(glow::TEXTURE0 + unit);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
    }
}

/// The textures and program for `probes`' light, which is yet to be
/// worked out (`Gi::stale`).
fn make_gi(gl: &glow::Context, glsl: Glsl, defines: &str, probes: &gi::Probes) -> Result<Gi, String> {
    let program = link(gl, glsl, defines, FULLSCREEN_VERT, GI_UPDATE_FRAG, "bounced light")?;
    let [nx, ny, nz] = probes.layout.count.map(|c| c as i32);
    let width = gi::Probes::row_texels(probes.patches) as i32;
    let texels: Vec<u8> = probes.texels().into_iter().flat_map(f32::to_ne_bytes).collect();
    // SAFETY: see `GlScreen`.
    unsafe {
        let made = (|| {
            let sums = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(sums));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            let pixels = glow::PixelUnpackData::Slice(Some(&texels));
            let rows = probes.probes.len() as i32;
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA32F as i32, width, rows, 0, glow::RGBA, glow::FLOAT, pixels);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            let light = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_3D, Some(light));
            let none = glow::PixelUnpackData::Slice(None);
            gl.tex_image_3d(glow::TEXTURE_3D, 0, glow::RGBA16F as i32, 6 * nx, ny, nz, 0, glow::RGBA, glow::HALF_FLOAT, none);
            for (key, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_R, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_3D, key, value as i32);
            }
            let framebuffer = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_layer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, Some(light), 0, 0);
            let complete = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_3D, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            if !complete {
                gl.delete_framebuffer(framebuffer);
                gl.delete_texture(light);
                gl.delete_texture(sums);
                return Err("OpenGL can't draw the bounced light".to_string());
            }
            Ok(Gi { layout: probes.layout, probes: sums, light, framebuffer, program, stale: true })
        })();
        if made.is_err() {
            gl.delete_program(program);
        }
        made
    }
}

fn delete_gi(gl: &glow::Context, gi: Gi) {
    // SAFETY: see `GlScreen`.
    unsafe {
        gl.delete_framebuffer(gi.framebuffer);
        gl.delete_texture(gi.light);
        gl.delete_texture(gi.probes);
        gl.delete_program(gi.program);
    }
}

/// The patches of the screen's light, `size` of them across and down,
/// black.
fn make_grid(gl: &glow::Context, size: (i32, i32)) -> Result<Grid, String> {
    // SAFETY: see `GlScreen`.
    unsafe {
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        // A column more, for the whole picture's colour.
        let black = vec![0u8; ((size.0 + 1) * size.1 * 8) as usize];
        let pixels = glow::PixelUnpackData::Slice(Some(&black));
        gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA16F as i32, size.0 + 1, size.1, 0, glow::RGBA, glow::HALF_FLOAT, pixels);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
        let framebuffer = gl.create_framebuffer()?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
        let complete = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        if !complete {
            return Err("OpenGL can't make the screen's light".into());
        }
        Ok(Grid { framebuffer, texture, size, empty: true })
    }
}

fn make_target(gl: &glow::Context, format: Format) -> Result<Target, String> {
    let color_format = if format.srgb { glow::SRGB8_ALPHA8 } else { glow::RGBA8 };
    let (w, h) = (format.size.0 as i32, format.size.1 as i32);
    // SAFETY: see `GlScreen`.
    unsafe {
        let most = gl.get_parameter_i32(glow::MAX_SAMPLES);
        let framebuffer = gl.create_framebuffer()?;
        let color = gl.create_renderbuffer()?;
        let depth = gl.create_renderbuffer()?;
        let mut samples = most.clamp(0, 4);
        loop {
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(color));
            gl.renderbuffer_storage_multisample(glow::RENDERBUFFER, samples, color_format, w, h);
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth));
            gl.renderbuffer_storage_multisample(glow::RENDERBUFFER, samples, glow::DEPTH_COMPONENT24, w, h);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::RENDERBUFFER, Some(color));
            gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, glow::RENDERBUFFER, Some(depth));
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE {
                break;
            }
            if samples == 0 {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                return Err("OpenGL can't make the 3D view's framebuffer".into());
            }
            samples = 0;
        }
        gl.bind_renderbuffer(glow::RENDERBUFFER, None);
        let resolved = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(resolved));
        let none = glow::PixelUnpackData::Slice(None);
        gl.tex_image_2d(glow::TEXTURE_2D, 0, color_format as i32, w, h, 0, glow::RGBA, glow::UNSIGNED_BYTE, none);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        let resolve = gl.create_framebuffer()?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(resolve));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(resolved), 0);
        let complete = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        if !complete {
            return Err("OpenGL can't make the 3D view's resolve framebuffer".into());
        }
        Ok(Target { format, framebuffer, color, depth, resolve, resolved })
    }
}

/// The emulated picture drawn through the look, for the screen: a texture
/// with mipmaps, so that it doesn't shimmer seen from afar or aslant.
pub struct ScreenTarget {
    pub framebuffer: glow::Framebuffer,
    pub texture: glow::Texture,
    pub size: (u32, u32),
}

impl ScreenTarget {
    pub fn new(gl: &glow::Context) -> Result<Self, String> {
        // SAFETY: see `GlScreen`.
        unsafe {
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            let framebuffer = gl.create_framebuffer()?;
            Ok(ScreenTarget { framebuffer, texture, size: (0, 0) })
        }
    }

    /// Make the texture `size` big, if it isn't; bound to the framebuffer
    /// either way. False if OpenGL can't draw into it.
    pub fn resize(&mut self, gl: &glow::Context, size: (u32, u32)) -> bool {
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
            if size != self.size {
                gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
                let none = glow::PixelUnpackData::Slice(None);
                let (w, h) = (size.0 as i32, size.1 as i32);
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w, h, 0, glow::RGBA, glow::UNSIGNED_BYTE, none);
                gl.generate_mipmap(glow::TEXTURE_2D);
                let texture = Some(self.texture);
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, texture, 0);
                self.size = size;
            }
            gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE
        }
    }

    /// After drawing into it: the smaller sizes of the picture.
    pub fn finish(&self, gl: &glow::Context) {
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            gl.generate_mipmap(glow::TEXTURE_2D);
        }
    }

    /// Delete the texture and the framebuffer, with `gl` current.
    pub fn delete(self, gl: &glow::Context) {
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.delete_framebuffer(self.framebuffer);
            gl.delete_texture(self.texture);
        }
    }
}

#[cfg(test)]
mod tests {
    //! The screen's light as lit.frag works it out, in Rust, against
    //! counting it out.

    use glam::Vec3;

    /// lit.frag's `edge`.
    fn edge(a: Vec3, b: Vec3) -> Vec3 {
        let x = a.dot(b);
        let y = x.abs();
        let v = (0.854_398_5 + (0.496_515_5 + 0.014_520_6 * y) * y) / (3.417_594 + (4.161_672_4 + y) * y);
        let theta = if x > 0.0 { v } else { 0.5 / (1.0 - x * x).max(1e-7).sqrt() - v };
        a.cross(b) * theta
    }

    /// lit.frag's `patch_near`, for a rectangle from `origin` along
    /// `across` and `down`.
    fn near(p: Vec3, n: Vec3, origin: Vec3, across: Vec3, down: Vec3) -> f32 {
        let corners = [origin, origin + across, origin + across + down, origin + down].map(|c| (c - p).normalize());
        let sum: Vec3 = (0..4).map(|i| edge(corners[i], corners[(i + 1) % 4])).sum();
        n.dot(sum).max(0.0)
    }

    /// lit.frag's `patch_far`.
    fn far(p: Vec3, n: Vec3, origin: Vec3, across: Vec3, down: Vec3) -> f32 {
        let to = origin + (across + down) * 0.5 - p;
        let d2 = to.length_squared();
        let l = to / d2.sqrt();
        let out_of = across.cross(down).normalize() * -1.0;
        across.length() * down.length() / std::f32::consts::PI * n.dot(l).max(0.0) * (-out_of.dot(l)).max(0.0) / d2
    }

    /// The form factor counted out over a grid of points on the rectangle.
    fn counted(p: Vec3, n: Vec3, origin: Vec3, across: Vec3, down: Vec3) -> f32 {
        let out_of = -across.cross(down).normalize();
        let steps = 400;
        let area = across.length() * down.length() / (steps * steps) as f32;
        let mut sum = 0.0;
        for j in 0..steps {
            for i in 0..steps {
                let q = origin + across * ((i as f32 + 0.5) / steps as f32) + down * ((j as f32 + 0.5) / steps as f32);
                let to = q - p;
                let d2 = to.length_squared();
                let l = to / d2.sqrt();
                sum += n.dot(l).max(0.0) * (-out_of.dot(l)).max(0.0) / (std::f32::consts::PI * d2) * area;
            }
        }
        sum
    }

    /// The test room's screen: 1.6 by 1.2 m, facing +z.
    fn screen() -> (Vec3, Vec3, Vec3) {
        (Vec3::new(-0.8, 2.0, -2.5), Vec3::new(1.6, 0.0, 0.0), Vec3::new(0.0, -1.2, 0.0))
    }

    #[test]
    fn the_screen_lights_what_is_in_front_of_it_as_counted() {
        let (origin, across, down) = screen();
        for (p, n) in [
            // The viewer, facing it; the floor in front; the side of a
            // bezel square to the glass at its edge; under its corner.
            (Vec3::new(0.0, 1.2, 0.0), Vec3::NEG_Z),
            (Vec3::new(0.0, 0.0, -1.5), Vec3::Y),
            (Vec3::new(0.85, 1.4, -2.45), Vec3::NEG_X),
            (Vec3::new(-0.7, 0.75, -2.4), Vec3::new(0.3, 1.0, 0.2).normalize()),
        ] {
            let (exact, count) = (near(p, n, origin, across, down), counted(p, n, origin, across, down));
            assert!((exact - count).abs() < 0.002 + count * 0.01, "{:?}: {} counted {}", p, exact, count);
        }
    }

    #[test]
    fn nothing_behind_the_screen_is_lit_by_it() {
        let (origin, across, down) = screen();
        assert_eq!(near(Vec3::new(0.0, 1.4, -3.0), Vec3::Z, origin, across, down), 0.0);
    }

    #[test]
    fn far_from_a_patch_it_is_a_small_light() {
        // A twelfth of the screen, a diagonal of the whole from the
        // screen's middle (where lit.frag starts taking it as small).
        let (origin, across, down) = screen();
        let (across, down) = (across / 4.0, down / 3.0);
        let diagonal = (across * 4.0 + down * 3.0).length();
        for (p, n) in [
            (Vec3::new(0.0, 1.4, -2.5 + diagonal), Vec3::NEG_Z),
            (Vec3::new(0.0, 1.4 - diagonal * 0.7, -2.5 + diagonal * 0.7), Vec3::new(0.0, 0.6, -0.8)),
        ] {
            let (small, exact) = (far(p, n, origin, across, down), near(p, n, origin, across, down));
            assert!((small - exact).abs() < exact * 0.05, "{:?}: {} exactly {}", p, small, exact);
        }
    }
}
