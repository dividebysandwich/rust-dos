//! The picture on a screen in a 3D scene (`[vr]`): the window shows the
//! scene through a camera flown with the mouse (`mode=desktop`), or a VR
//! headset shows it through OpenXR, on a thread of its own, and the window
//! the left eye's view (`mode=headset`).
//!
//! The picture is drawn through the look into a texture with mipmaps
//! whenever it changes (`ScreenTarget`, three of them for the headset's
//! thread to read one while the next is drawn), which the scene's screen
//! shows; the scene itself is drawn every frame, since the viewer moves.
//! The PC in the scene has its lights lit as the machine's would be, the
//! sound comes from the screen's sides, and the headset's controllers
//! point at the screen as the mouse and are a gamepad.

mod camera;
mod controls;
mod gi;
#[cfg(xr)]
mod headset;
mod pick;
mod render;
mod scene;
mod shadow;
mod spatial;
#[cfg(xr)]
mod xr;

use crate::video::shader::Glsl;
use camera::FlyCamera;
pub use controls::VrInput;
use super::audio_mix::Mix;
use glam::{Mat4, Vec2};
use glow::HasContext;
use render::{Format, Gpu, ScreenTarget, View};
use rust_dos::vr::{ScreenFit, VrMode, VrSettings};
use scene::Scene;
use rust_dos::vr::Leds;
use std::sync::Arc;

pub struct Stage {
    scene: Arc<Scene>,
    gpu: Gpu,
    /// The screen's pictures: the newest finished, and the one being
    /// drawn into while the headset reads another.
    screens: Vec<ScreenTarget>,
    latest: Option<usize>,
    /// The newest picture's light isn't taken yet (`Gpu::prepare`).
    fresh: bool,
    drawing: usize,
    camera: FlyCamera,
    leds: Leds,
    #[cfg(xr)]
    headset: Option<headset::Headset>,
    /// What the window showed last: the view's view-projection and where
    /// it was in the drawable (x, y from the top, width, height), for the
    /// mouse.
    shown: Option<(Mat4, (f32, f32, f32, f32))>,
    /// How the picture fills the screen (the `screen_fit` setting).
    fit: ScreenFit,
}

/// Width and height.
type Size = (u32, u32);

/// What has to be done before SDL starts for a headset to work.
pub fn before_sdl() {
    #[cfg(xr)]
    headset::before_sdl();
}

impl Stage {
    /// The scene of `settings`, or the test room, ready to draw; and why
    /// the scene or the headset the settings ask for isn't there, if it
    /// isn't. The headset's thread gets a context sharing `main`'s, the
    /// window's, textures.
    #[cfg_attr(not(xr), allow(unused_variables))]
    pub fn new(
        gl: &glow::Context,
        glsl: Glsl,
        settings: &VrSettings,
        window: &sdl2::video::Window,
        main: &sdl2::video::GLContext,
    ) -> Result<(Self, Vec<String>), String> {
        let mut notes = Vec::new();
        let scene = match &settings.scene {
            Some(path) => Scene::load(path).unwrap_or_else(|e| {
                notes.push(format!("[VR] The scene can't be shown ({}); the test room is", e));
                Scene::test_room()
            }),
            None => Scene::test_room(),
        };
        let scene = Arc::new(scene);
        let mut gpu = Gpu::new(gl, glsl, &scene, settings.quality)?;
        gpu.set_glow(glow_of(settings));
        let mut screens = Vec::new();
        for _ in 0..3 {
            let screen = ScreenTarget::new(gl)?;
            // SAFETY: see `GlScreen`.
            unsafe { gl.bind_texture(glow::TEXTURE_2D, Some(screen.texture)) };
            gpu.set_anisotropy(gl);
            screens.push(screen);
        }
        let camera = FlyCamera::at(scene.spawn);
        #[allow(unused_mut)]
        let mut stage = Stage {
            scene,
            gpu,
            screens,
            latest: None,
            fresh: false,
            drawing: 0,
            camera,
            leds: Leds::default(),
            #[cfg(xr)]
            headset: None,
            shown: None,
            fit: settings.screen_fit,
        };
        if settings.mode == VrMode::Headset {
            #[cfg(xr)]
            {
                let textures = [stage.screens[0].texture, stage.screens[1].texture, stage.screens[2].texture];
                match headset::Headset::start(window, main, glsl, stage.scene.clone(), textures, settings) {
                    Ok(headset) => stage.headset = Some(headset),
                    Err(e) => notes.push(format!("[VR] No headset ({}); the scene is shown in the window", e)),
                }
            }
            #[cfg(not(xr))]
            notes.push("[VR] This build has no OpenXR; the scene is shown in the window".to_string());
        }
        Ok((stage, notes))
    }

    /// The size the picture is drawn at for the screen, for a picture of
    /// `display` proportions: twice it, so that scanlines and masks keep
    /// their shape, but from 1024 to 2048 across. It has the screen's shape,
    /// so the picture keeps its own, letterboxed; stretched, it has the
    /// picture's, and the screen's UVs stretch it over the whole surface.
    pub fn screen_size(&self, display: Size) -> Size {
        let width = (display.0 * 2).clamp(1024, 2048);
        let aspect = if self.fit.stretches(self.scene.screen.stretch) {
            display.0.max(1) as f32 / display.1.max(1) as f32
        } else {
            self.scene.screen.aspect()
        };
        let height = (width as f32 / aspect).round().clamp(1.0, 4096.0) as u32;
        (width, height)
    }

    /// Bind a screen's framebuffer at `size` to draw the picture into: one
    /// neither the newest nor the headset's. False if it can't be.
    pub fn begin_screen(&mut self, gl: &glow::Context, size: Size) -> bool {
        #[cfg(xr)]
        let in_use = self.headset.as_ref().and_then(|h| h.screen_in_use(gl));
        #[cfg(not(xr))]
        let in_use: Option<usize> = None;
        self.drawing = (0..self.screens.len()).find(|&i| Some(i) != self.latest && Some(i) != in_use).unwrap_or(0);
        self.screens[self.drawing].resize(gl, size)
    }

    /// The picture is drawn into the screen.
    pub fn end_screen(&mut self, gl: &glow::Context) {
        self.screens[self.drawing].finish(gl);
        self.latest = Some(self.drawing);
        self.fresh = true;
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            headset.publish_screen(gl, self.drawing);
        }
    }

    /// Draw the scene for the window, whose drawable is `drawable` big:
    /// the headset's left eye while it shows the scene, else the window's
    /// camera's view. The window is to be swapped after.
    pub fn render(&mut self, gl: &glow::Context, drawable: Size) {
        if drawable.0 == 0 || drawable.1 == 0 {
            return;
        }
        #[cfg(xr)]
        if let Some(headset) = &mut self.headset
            && headset.running()
        {
            if let Some(shown) = headset.draw_mirror(gl, drawable) {
                self.shown = Some(shown);
            }
            // (Until its first frame, the window keeps what it had.)
            return;
        }
        let screen = self.latest.map(|i| self.screens[i].texture);
        if let Some(texture) = screen.filter(|_| std::mem::take(&mut self.fresh)) {
            self.gpu.prepare(gl, texture);
        }
        let aspect = drawable.0 as f32 / drawable.1 as f32;
        let view = View { view: self.camera.view(), projection: self.camera.projection(aspect) };
        let format = Format { size: drawable, srgb: false };
        if let Err(e) = self.gpu.render(gl, &self.scene, &view, format, screen, self.leds, &[]) {
            eprintln!("[VR] {}", e);
            clear_window(gl, drawable);
            return;
        }
        self.gpu.copy_to(gl, None, (0, 0, drawable.0 as i32, drawable.1 as i32));
        self.shown = Some((view.view_projection(), (0.0, 0.0, drawable.0 as f32, drawable.1 as f32)));
    }

    /// The point of the picture (0 to 1 across and down the screen's
    /// texture) under a point of the window's drawable, if the screen is
    /// there.
    pub fn pick(&self, (x, y): (f32, f32)) -> Option<Vec2> {
        let (view_projection, (rx, ry, rw, rh)) = self.shown?;
        let ndc = Vec2::new((x - rx) / rw.max(1.0) * 2.0 - 1.0, 1.0 - (y - ry) / rh.max(1.0) * 2.0);
        let (origin, dir) = pick::ray(view_projection.inverse(), ndc);
        pick::pick(&self.scene.screen, origin, dir)
    }

    /// The window's camera, to fly.
    pub fn camera_mut(&mut self) -> &mut FlyCamera {
        &mut self.camera
    }

    /// Back to where the scene starts, and the headset's view centred
    /// where it looks now.
    pub fn recenter(&mut self) {
        self.camera = FlyCamera::at(self.scene.spawn);
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            headset.recenter();
        }
    }

    /// Whether a headset is attached, running or not.
    pub fn has_headset(&self) -> bool {
        #[cfg(xr)]
        return self.headset.is_some();
        #[cfg(not(xr))]
        false
    }

    /// What the headset's thread has to say; once it is over, the window
    /// shows the scene.
    pub fn poll(&mut self) -> Vec<String> {
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            let (notes, ended) = headset.poll();
            if ended {
                self.headset = None;
            }
            return notes;
        }
        Vec::new()
    }

    /// The PC's lights, as the machine's are.
    pub fn set_leds(&mut self, leds: Leds) {
        self.leds = leds;
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            headset.set_leds(leds);
        }
    }

    /// Take on the `[vr]` settings that change while it runs.
    pub fn apply(&mut self, settings: &VrSettings) {
        self.fit = settings.screen_fit;
        self.gpu.set_glow(glow_of(settings));
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            headset.set_controllers(settings.controllers);
            headset.set_glow(glow_of(settings));
            headset.set_placement(headset::Placement::of(settings));
        }
    }

    /// The controllers' input, while the headset shows the scene.
    pub fn input(&self) -> Option<VrInput> {
        #[cfg(xr)]
        return self.headset.as_ref().and_then(|h| h.input()).map(|(input, _)| input);
        #[cfg(not(xr))]
        None
    }

    /// How the sound's channels mix for the listener: the headset's head
    /// while it shows the scene, else the window's camera.
    pub fn audio_mix(&self) -> Mix {
        #[cfg(xr)]
        let head = self.headset.as_ref().and_then(|h| h.input()).and_then(|(_, head)| head);
        #[cfg(not(xr))]
        let head: Option<Mat4> = None;
        let listener = head.unwrap_or_else(|| self.camera.view().inverse());
        let spawn = self.scene.spawn;
        let start = Mat4::from_translation(spawn.position) * Mat4::from_rotation_y(spawn.yaw);
        spatial::mix(listener, start, self.scene.speakers)
    }
}

/// How brightly the screen lights the scene, times what the scene says.
fn glow_of(settings: &VrSettings) -> f32 {
    settings.screen_glow as f32 / 100.0
}

/// Black over the whole window, for the bars around the headset's view.
fn clear_window(gl: &glow::Context, (w, h): Size) {
    // SAFETY: see `GlScreen`.
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.viewport(0, 0, w as i32, h as i32);
        gl.clear_color(0.0, 0.0, 0.0, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
    }
}
