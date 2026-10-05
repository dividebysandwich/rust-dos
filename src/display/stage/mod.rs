//! The picture on a screen in a 3D scene (`[vr]`): the window shows the
//! scene through a camera flown with the mouse (`mode=desktop`), or a VR
//! headset shows it through OpenXR and the window the left eye's view
//! (`mode=headset`).
//!
//! The picture is drawn through the look into a texture with mipmaps
//! whenever it changes (`ScreenTarget`), which the scene's screen shows;
//! the scene itself is drawn every frame, since the viewer moves.

mod camera;
mod pick;
mod render;
mod scene;
#[cfg(xr)]
mod xr;

use crate::video::shader::Glsl;
use camera::FlyCamera;
use glam::{Mat4, Vec2};
use glow::HasContext;
use render::{Format, Gpu, ScreenTarget, View};
use rust_dos::vr::{VrMode, VrSettings};
use scene::Scene;

pub struct Stage {
    scene: Scene,
    gpu: Gpu,
    screen: ScreenTarget,
    /// Whether the screen has a picture yet.
    has_picture: bool,
    camera: FlyCamera,
    #[cfg(xr)]
    xr: Option<xr::Xr>,
    /// What the window showed last: the view's view-projection and where
    /// it was in the drawable (x, y from the top, width, height), for the
    /// mouse.
    shown: Option<(Mat4, (f32, f32, f32, f32))>,
}

/// Width and height.
type Size = (u32, u32);

impl Stage {
    /// The scene of `settings`, or the test room, ready to draw; and why
    /// the scene or the headset the settings ask for isn't there, if it
    /// isn't. A headset draws with the OpenGL context current, the
    /// window's.
    pub fn new(gl: &glow::Context, glsl: Glsl, settings: &VrSettings) -> Result<(Self, Vec<String>), String> {
        let mut notes = Vec::new();
        let scene = match &settings.scene {
            Some(path) => Scene::load(path).unwrap_or_else(|e| {
                notes.push(format!("[VR] The scene can't be shown ({}); the test room is", e));
                Scene::test_room()
            }),
            None => Scene::test_room(),
        };
        let gpu = Gpu::new(gl, glsl, &scene)?;
        let screen = ScreenTarget::new(gl)?;
        // SAFETY: see `GlScreen`.
        unsafe { gl.bind_texture(glow::TEXTURE_2D, Some(screen.texture)) };
        gpu.set_anisotropy(gl);
        let camera = FlyCamera::at(scene.spawn);
        #[allow(unused_mut)]
        let mut stage =
            Stage { scene, gpu, screen, has_picture: false, camera, #[cfg(xr)] xr: None, shown: None };
        if settings.mode == VrMode::Headset {
            #[cfg(xr)]
            match xr::Xr::new(gl) {
                Ok(xr) => {
                    notes.push(format!("[VR] {}", xr.describe()));
                    stage.xr = Some(xr);
                }
                Err(e) => notes.push(format!("[VR] No headset ({}); the scene is shown in the window", e)),
            }
            #[cfg(not(xr))]
            notes.push("[VR] This build has no OpenXR; the scene is shown in the window".to_string());
        }
        Ok((stage, notes))
    }

    /// The size the picture is drawn at for the screen, for a picture of
    /// `display` proportions: twice it, so that scanlines and masks keep
    /// their shape, but from 1024 to 2048 across, in the screen's shape.
    pub fn screen_size(&self, display: Size) -> Size {
        let width = (display.0 * 2).clamp(1024, 2048);
        let height = (width as f32 / self.scene.screen.aspect()).round().clamp(1.0, 4096.0) as u32;
        (width, height)
    }

    /// Bind the screen's framebuffer at `size` to draw the picture into.
    /// False if it can't be.
    pub fn begin_screen(&mut self, gl: &glow::Context, size: Size) -> bool {
        self.screen.resize(gl, size)
    }

    /// The picture is drawn into the screen.
    pub fn end_screen(&mut self, gl: &glow::Context) {
        self.screen.finish(gl);
        self.has_picture = true;
    }

    /// Draw the scene for the window, whose drawable is `drawable` big, and
    /// for the headset if it is running. The window is to be swapped
    /// after.
    pub fn render(&mut self, gl: &glow::Context, drawable: Size) {
        let screen = self.has_picture.then_some(self.screen.texture);
        #[cfg(xr)]
        if let Some(xr) = &mut self.xr
            && xr.frame_pending()
        {
            let (gpu, scene) = (&mut self.gpu, &self.scene);
            let mut mirror = None;
            let result = xr.render(gl, scene.spawn, |eye, view, size, srgb, framebuffer| {
                let format = Format { size, srgb };
                if let Err(e) = gpu.render(gl, scene, &view, format, screen) {
                    eprintln!("[VR] {}", e);
                    return;
                }
                let full = (0, 0, size.0 as i32, size.1 as i32);
                gpu.copy_to(gl, Some(framebuffer), full);
                // The left eye's view in the window too.
                if eye == 0 {
                    let (x, y, w, h) = super::letterbox(drawable, size);
                    clear_window(gl, drawable);
                    let bottom = drawable.1 as i32 - y as i32 - h as i32;
                    gpu.copy_to(gl, None, (x as i32, bottom, w as i32, h as i32));
                    mirror = Some((view.view_projection(), (x as f32, y as f32, w as f32, h as f32)));
                }
            });
            match result {
                Ok(true) => {
                    self.shown = mirror;
                    return;
                }
                Ok(false) => {}
                Err(e) => eprintln!("[VR] {}", e),
            }
        }
        if drawable.0 == 0 || drawable.1 == 0 {
            return;
        }
        let aspect = drawable.0 as f32 / drawable.1 as f32;
        let view = View { view: self.camera.view(), projection: self.camera.projection(aspect) };
        let format = Format { size: drawable, srgb: false };
        if let Err(e) = self.gpu.render(gl, &self.scene, &view, format, screen) {
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
        if let Some(xr) = &mut self.xr {
            xr.recenter();
        }
    }

    /// Whether a headset is attached, running or not.
    pub fn has_headset(&self) -> bool {
        #[cfg(xr)]
        return self.xr.is_some();
        #[cfg(not(xr))]
        false
    }

    /// Handle the headset's events; what happened worth saying.
    pub fn poll(&mut self) -> Vec<String> {
        #[cfg(xr)]
        if let Some(xr) = &mut self.xr {
            let (notes, lost) = xr.poll();
            if lost {
                self.xr = None;
            }
            return notes;
        }
        Vec::new()
    }

    /// Wait for the headset's next frame, if it is showing the scene: then
    /// the headset paces the frames, each the period returned.
    pub fn wait_frame(&mut self) -> Option<std::time::Duration> {
        #[cfg(xr)]
        if let Some(xr) = &mut self.xr {
            return xr.wait_frame();
        }
        None
    }
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
