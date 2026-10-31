//! Source-sized xBRZ work, reused across overlays, captures and resizes.

use super::{Program, compile_sources};
use crate::video::shader::{self, Glsl, XbrzDamage, XbrzPass};
use glow::HasContext;

struct Target {
    texture: glow::Texture,
    framebuffer: glow::Framebuffer,
    size: (u32, u32),
}

impl Target {
    fn new(gl: &glow::Context) -> Result<Self, String> {
        // SAFETY: GlScreen owns the current context and these objects.
        unsafe {
            let texture = gl.create_texture()?;
            let framebuffer = match gl.create_framebuffer() {
                Ok(framebuffer) => framebuffer,
                Err(error) => {
                    gl.delete_texture(texture);
                    return Err(error);
                }
            };
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            for name in [glow::TEXTURE_MIN_FILTER, glow::TEXTURE_MAG_FILTER] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, name, glow::NEAREST as i32);
            }
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::DRAW_FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            Ok(Self {
                texture,
                framebuffer,
                size: (0, 0),
            })
        }
    }

    fn resize(&mut self, gl: &glow::Context, size: (u32, u32)) -> Result<(), String> {
        if self.size == size {
            return Ok(());
        }
        // SAFETY: sizes are checked against the context limit by draw.
        unsafe {
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                size.0 as i32,
                size.1 as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.framebuffer));
            let complete =
                gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            let error = gl.get_error();
            if !complete || error != glow::NO_ERROR {
                return Err("xBRZ framebuffer allocation failed".into());
            }
        }
        self.size = size;
        Ok(())
    }

    fn destroy(self, gl: &glow::Context) {
        // SAFETY: GlScreen still owns the current context and these objects.
        unsafe {
            gl.delete_framebuffer(self.framebuffer);
            gl.delete_texture(self.texture);
        }
    }
}

pub(super) struct XbrzGpu {
    corners: Option<Target>,
    scaled: Option<Target>,
    classify: Program,
    resolve: Program,
    display: Program,
    source_size: (u32, u32),
    source: Option<glow::Texture>,
    corners_valid: bool,
    scaled_valid: bool,
    corner_damage: XbrzDamage,
    scaled_damage: XbrzDamage,
    limit: u32,
    #[cfg(test)]
    pub draws: (usize, usize),
}

impl XbrzGpu {
    pub fn new(gl: &glow::Context, glsl: Glsl) -> Result<Self, String> {
        let mut programs = Vec::new();
        for pass in [XbrzPass::Corners, XbrzPass::Resolve, XbrzPass::Display] {
            match compile_sources(gl, glsl, "xBRZ pass", shader::xbrz_sources(pass, glsl)) {
                Ok(program) => programs.push(program),
                Err(error) => {
                    for program in programs {
                        // SAFETY: these programs belong to the current context.
                        unsafe { gl.delete_program(program.program) };
                    }
                    return Err(error);
                }
            }
        }
        // SAFETY: GlScreen made this context current on this thread.
        let limit = unsafe {
            let mut viewport = [0; 2];
            gl.get_parameter_i32_slice(glow::MAX_VIEWPORT_DIMS, &mut viewport);
            gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE)
                .min(viewport[0])
                .min(viewport[1])
        }
        .max(0) as u32;
        Ok(Self {
            display: programs.pop().unwrap(),
            resolve: programs.pop().unwrap(),
            classify: programs.pop().unwrap(),
            corners: None,
            scaled: None,
            source_size: (0, 0),
            source: None,
            corners_valid: false,
            scaled_valid: false,
            corner_damage: XbrzDamage::default(),
            scaled_damage: XbrzDamage::default(),
            limit,
            #[cfg(test)]
            draws: (0, 0),
        })
    }

    pub fn invalidate(&mut self) {
        self.corners_valid = false;
        self.scaled_valid = false;
        self.corner_damage.clear();
        self.scaled_damage.clear();
    }

    pub fn invalidate_regions(&mut self, damage: &XbrzDamage, size: (u32, u32)) {
        for rect in damage.expanded(size) {
            if self.corners_valid {
                self.corner_damage.add(rect);
            }
            if self.scaled_valid {
                self.scaled_damage.add(rect);
            }
        }
    }

    pub fn supports(&self, size: (u32, u32)) -> bool {
        size.0 < self.limit && size.1 < self.limit
    }

    pub fn destroy(self, gl: &glow::Context) {
        for target in [self.corners, self.scaled].into_iter().flatten() {
            target.destroy(gl);
        }
        for program in [self.classify, self.resolve, self.display] {
            // SAFETY: GlScreen still owns the current context and programs.
            unsafe { gl.delete_program(program.program) };
        }
    }

    pub fn draw(
        &mut self,
        gl: &glow::Context,
        source: glow::Texture,
        size: (u32, u32),
        rect: (i32, i32, u32, u32),
        overlay: Option<glow::Texture>,
    ) -> Result<(), String> {
        let halo = (
            size.0.checked_add(1).ok_or("xBRZ width overflow")?,
            size.1.checked_add(1).ok_or("xBRZ height overflow")?,
        );
        if halo.0 > self.limit || halo.1 > self.limit {
            return Err("xBRZ corner texture exceeds the context limit".into());
        }
        let scaled_size = (size.0.checked_mul(2), size.1.checked_mul(2));
        let scaled_size = match scaled_size {
            (Some(w), Some(h)) if w <= self.limit && h <= self.limit => Some((w, h)),
            _ => None,
        };
        let cached = scaled_size.filter(|&(w, h)| rect.2 >= w && rect.3 >= h);
        let needs_prepare = self.source != Some(source)
            || self.source_size != size
            || !self.corners_valid
            || !self.corner_damage.rects().is_empty()
            || cached.is_some() && (!self.scaled_valid || !self.scaled_damage.rects().is_empty());
        if needs_prepare {
            // SAFETY: all objects belong to GlScreen's current context. The
            // caller bound its empty VAO; each target fits MAX_TEXTURE_SIZE.
            unsafe {
                let framebuffer = gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);
                let mut scissor = [0; 4];
                gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut scissor);
                let states = [glow::BLEND, glow::DITHER, glow::SCISSOR_TEST]
                    .map(|state| (state, gl.is_enabled(state)));
                for &(state, _) in &states {
                    gl.disable(state);
                }
                let prepared = (|| {
                    if self.source != Some(source) {
                        self.source = Some(source);
                        self.invalidate();
                    }
                    if self.source_size != size {
                        if let Some(old) = self.scaled.take() {
                            old.destroy(gl);
                        }
                        self.source_size = size;
                        self.invalidate();
                    }
                    if self.corners.is_none() {
                        self.corners = Some(Target::new(gl)?);
                    }
                    let corners = self.corners.as_mut().unwrap();
                    corners.resize(gl, halo)?;
                    if !self.corners_valid || !self.corner_damage.rects().is_empty() {
                        let damage = self.corners_valid.then_some(&self.corner_damage);
                        draw_regions(gl, &self.classify, source, size, corners, damage, false);
                        self.corner_damage.clear();
                        self.corners_valid = true;
                        #[cfg(test)]
                        {
                            self.draws.0 += 1;
                        }
                    }
                    gl.active_texture(glow::TEXTURE2);
                    gl.bind_texture(glow::TEXTURE_2D, Some(corners.texture));
                    if let Some(scaled_size) = cached {
                        if self.scaled.is_none() {
                            self.scaled = Some(Target::new(gl)?);
                        }
                        let scaled = self.scaled.as_mut().unwrap();
                        scaled.resize(gl, scaled_size)?;
                        if !self.scaled_valid || !self.scaled_damage.rects().is_empty() {
                            let damage = self.scaled_valid.then_some(&self.scaled_damage);
                            draw_regions(gl, &self.resolve, source, size, scaled, damage, true);
                            self.scaled_damage.clear();
                            self.scaled_valid = true;
                            #[cfg(test)]
                            {
                                self.draws.1 += 1;
                            }
                        }
                    }
                    Ok::<_, String>(())
                })();
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, framebuffer);
                gl.scissor(scissor[0], scissor[1], scissor[2], scissor[3]);
                for (state, enabled) in states {
                    if enabled {
                        gl.enable(state);
                    }
                }
                gl.active_texture(glow::TEXTURE0);
                prepared?;
            }
        }
        // SAFETY: preparation validated these textures and restored the
        // caller's framebuffer and raster state before the final draw.
        unsafe {
            let (program, texture) = if cached.is_some() {
                (&self.display, self.scaled.as_ref().unwrap().texture)
            } else {
                (&self.resolve, source)
            };
            if cached.is_none() {
                gl.active_texture(glow::TEXTURE2);
                gl.bind_texture(
                    glow::TEXTURE_2D,
                    Some(self.corners.as_ref().unwrap().texture),
                );
            }
            gl.viewport(rect.0, rect.1, rect.2 as i32, rect.3 as i32);
            gl.use_program(Some(program.program));
            gl.uniform_2_f32(program.source.as_ref(), size.0 as f32, size.1 as f32);
            gl.uniform_1_i32(program.unflipped.as_ref(), 0);
            gl.uniform_1_i32(program.overlay_enabled.as_ref(), overlay.is_some() as i32);
            if let Some(overlay) = overlay {
                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D, Some(overlay));
            }
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }
        Ok(())
    }
}

fn draw_regions(
    gl: &glow::Context,
    program: &Program,
    source: glow::Texture,
    size: (u32, u32),
    target: &Target,
    damage: Option<&XbrzDamage>,
    unflipped: bool,
) {
    // SAFETY: draw owns the current context and restores the scissor box
    // and enable state after both cached passes, including on failure.
    unsafe {
        if let Some(damage) = damage {
            gl.enable(glow::SCISSOR_TEST);
            for &[x, y, w, h] in damage.rects() {
                // Corner storage has an extra top/left kernel; the 2x
                // target stores four output pixels per source pixel.
                let rect = if unflipped {
                    [x * 2, y * 2, w * 2, h * 2]
                } else {
                    [x, y, w + 1, h + 1]
                };
                gl.scissor(
                    rect[0] as i32,
                    rect[1] as i32,
                    rect[2] as i32,
                    rect[3] as i32,
                );
                draw(gl, program, source, size, target, unflipped);
            }
            gl.disable(glow::SCISSOR_TEST);
        } else {
            draw(gl, program, source, size, target, unflipped);
        }
    }
}

fn draw(
    gl: &glow::Context,
    program: &Program,
    source: glow::Texture,
    size: (u32, u32),
    target: &Target,
    unflipped: bool,
) {
    // SAFETY: the caller owns the context, VAO, source and target. Their
    // dimensions were checked before allocating the target storage.
    unsafe {
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(target.framebuffer));
        gl.viewport(0, 0, target.size.0 as i32, target.size.1 as i32);
        gl.use_program(Some(program.program));
        gl.uniform_2_f32(program.source.as_ref(), size.0 as f32, size.1 as f32);
        gl.uniform_1_i32(program.overlay_enabled.as_ref(), 0);
        gl.uniform_1_i32(program.unflipped.as_ref(), unflipped as i32);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(source));
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
    }
}
