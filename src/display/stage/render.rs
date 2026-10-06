//! The scene drawn with OpenGL: its meshes and pictures uploaded once, and
//! each view drawn multisampled into a target of its own, from which it is
//! copied to the window or a headset's eye.

use super::scene::{Alpha, Leds, Scene, Shading};
use crate::video::shader::Glsl;
use glam::{Mat4, Vec3};
use glow::HasContext;

const COMMON: &str = include_str!("shader/common.glsl");
const MESH_VERT: &str = include_str!("shader/mesh.vert");
const LIT_FRAG: &str = include_str!("shader/lit.frag");
const SKY_FRAG: &str = include_str!("shader/sky.frag");
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

pub struct Gpu {
    lit: glow::Program,
    sky: glow::Program,
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

/// A program of the scene's.
fn link(gl: &glow::Context, glsl: Glsl, vertex: &str, fragment: &str, name: &str) -> Result<glow::Program, String> {
    let preamble = glsl.preamble();
    let sources = [
        (glow::VERTEX_SHADER, format!("{}{}", preamble, vertex)),
        (glow::FRAGMENT_SHADER, format!("{}{}{}", preamble, COMMON, fragment)),
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
    /// Compile the programs and upload `scene`.
    pub fn new(gl: &glow::Context, glsl: Glsl, scene: &Scene) -> Result<Self, String> {
        if glsl == Glsl::Es300 {
            return Err("the 3D view needs desktop OpenGL, not OpenGL ES".into());
        }
        let lit = link(gl, glsl, MESH_VERT, LIT_FRAG, "scene")?;
        let sky = match link(gl, glsl, FULLSCREEN_VERT, SKY_FRAG, "sky") {
            Ok(sky) => sky,
            Err(e) => {
                // SAFETY: see `GlScreen`.
                unsafe { gl.delete_program(lit) };
                return Err(e);
            }
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
        let mut gpu =
            Gpu { lit, sky, empty, meshes: Vec::new(), cube, textures: Vec::new(), target: None, anisotropy };
        gpu.upload(gl, scene)?;
        Ok(gpu)
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

            // The meshes.
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.use_program(Some(lit));
            let vp = view.view_projection();
            gl.uniform_matrix_4_f32_slice(u(lit, "u_view_projection").as_ref(), false, &vp.to_cols_array());
            let model = u(lit, "u_model");
            gl.uniform_matrix_4_f32_slice(model.as_ref(), false, &Mat4::IDENTITY.to_cols_array());
            gl.uniform_3_f32_slice(u(lit, "u_sun").as_ref(), &scene.sun.to_array());
            gl.uniform_1_i32(u(lit, "u_sky").as_ref(), scene.sky as i32);
            gl.uniform_1_i32(u(lit, "u_encode").as_ref(), encode);
            gl.uniform_3_f32_slice(u(lit, "u_ambient").as_ref(), &scene.ambient.to_array());
            gl.uniform_1_f32(u(lit, "u_exposure").as_ref(), scene.exposure);
            gl.uniform_3_f32_slice(u(lit, "u_eye").as_ref(), &view.eye().to_array());
            gl.uniform_1_f32(u(lit, "u_fog").as_ref(), if scene.sky { 0.02 } else { 0.0 });
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
            gl.uniform_1_i32(u(lit, "u_lights").as_ref(), lights.len() as i32);
            if !lights.is_empty() {
                gl.uniform_4_f32_slice(u(lit, "u_light_pos[0]").as_ref(), &pos);
                gl.uniform_3_f32_slice(u(lit, "u_light_color[0]").as_ref(), &color);
                gl.uniform_3_f32_slice(u(lit, "u_light_dir[0]").as_ref(), &dir);
                gl.uniform_2_f32_slice(u(lit, "u_light_cone[0]").as_ref(), &cone);
            }
            gl.uniform_1_i32(u(lit, "u_base").as_ref(), 0);
            gl.uniform_1_i32(u(lit, "u_emissive_map").as_ref(), 1);
            gl.uniform_1_i32(u(lit, "u_screen").as_ref(), 2);
            gl.uniform_1_i32(u(lit, "u_has_screen").as_ref(), screen.is_some() as i32);
            let glow_area = scene.screen.size.x * scene.screen.size.y;
            gl.uniform_3_f32_slice(u(lit, "u_glow_center").as_ref(), &scene.screen.center.to_array());
            gl.uniform_3_f32_slice(u(lit, "u_glow_normal").as_ref(), &scene.screen.normal.to_array());
            gl.uniform_1_f32(u(lit, "u_glow_area").as_ref(), glow_area);
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, screen);

            let (shading, base_color, has_base, emissive, has_emissive, cutoff) = (
                u(lit, "u_shading"),
                u(lit, "u_base_color"),
                u(lit, "u_has_base"),
                u(lit, "u_emissive"),
                u(lit, "u_has_emissive"),
                u(lit, "u_cutoff"),
            );
            // The opaque meshes, then those seen through.
            for blended in [false, true] {
                if blended {
                    gl.enable(glow::BLEND);
                    gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                    gl.depth_mask(false);
                }
                for mesh in &self.meshes {
                    let material = &scene.materials[mesh.material];
                    if (material.alpha == Alpha::Blend) != blended {
                        continue;
                    }
                    let kind = match material.shading {
                        Shading::Lit => 0,
                        Shading::Unlit => 1,
                        Shading::Screen => 2,
                        Shading::Floor => 3,
                    };
                    gl.uniform_1_i32(shading.as_ref(), kind);
                    gl.uniform_4_f32_slice(base_color.as_ref(), &material.base_color);
                    let glow = if material.led.is_none_or(|led| super::scene::lit(leds, led)) { 1.0 } else { 0.0 };
                    gl.uniform_3_f32_slice(emissive.as_ref(), &material.emissive.map(|c| c * glow));
                    let cut = match material.alpha {
                        Alpha::Mask(cut) => cut,
                        _ => -1.0,
                    };
                    gl.uniform_1_f32(cutoff.as_ref(), cut);
                    for (unit, texture, flag) in
                        [(0, material.base_texture, &has_base), (1, material.emissive_texture, &has_emissive)]
                    {
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
        Ok(GpuMesh { vao, count: mesh.indices.len() as i32, material: mesh.material })
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
        for unit in [2, 1, 0] {
            gl.active_texture(glow::TEXTURE0 + unit);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
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
}
