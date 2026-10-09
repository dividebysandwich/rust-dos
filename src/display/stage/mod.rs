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
use render::{Dest, Format, Gpu, Options, Pass, ScreenTarget, View};
use rust_dos::vr::{ScreenFit, VrMode, VrSettings};
use scene::Scene;
use rust_dos::vr::{Leds, VrGraphics, VrQuality};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

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
    /// What the scene's programs are made for and with.
    glsl: Glsl,
    /// Where the scene is shown: in the window, or in a headset as well.
    mode: VrMode,
    quality: VrQuality,
    /// How the headset's session draws, and its eyes' resolution, which it
    /// is started again for.
    headset_options: (VrGraphics, u32),
    glow: f32,
    /// The scene file shown (None: the test room), and the one being read
    /// on a thread of its own to take its place.
    scene_path: Option<PathBuf>,
    loading: Option<(Option<PathBuf>, Receiver<Result<Scene, String>>)>,
    /// When the window's frame times were last said (RUST_DOS_VR_TIMING).
    timing_since: std::time::Instant,
}

/// How the window's view is drawn: timed with RUST_DOS_VR_TIMING set.
fn window_options() -> Options {
    Options { timed: std::env::var_os("RUST_DOS_VR_TIMING").is_some(), samples: 4, multiview: None }
}

/// Width and height.
type Size = (u32, u32);

/// What has to be done before SDL starts for a headset to work.
#[cfg_attr(not(xr), allow(unused_variables))]
pub fn before_sdl(settings: &VrSettings) {
    #[cfg(xr)]
    headset::before_sdl(settings);
}

/// Write what the OpenXR runtime and OpenGL offer a headset (`--vr-probe`),
/// with an OpenGL context of its own, as the headset's thread has.
#[cfg_attr(not(xr), allow(unused_variables))]
pub fn probe(video: &sdl2::VideoSubsystem, settings: &VrSettings) {
    #[cfg(xr)]
    headset::probe(video, settings);
    #[cfg(not(xr))]
    println!("This build has no OpenXR");
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
        let scene = load(settings.scene.as_deref()).unwrap_or_else(|e| {
            notes.push(format!("[VR] The scene can't be shown ({}); the test room is", e));
            Scene::test_room()
        });
        let scene = Arc::new(scene);
        let mut gpu = Gpu::new(gl, glsl, &scene, settings.quality, window_options())?;
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
            glsl,
            mode: settings.mode,
            quality: settings.quality,
            headset_options: (settings.graphics, settings.resolution),
            glow: glow_of(settings),
            scene_path: settings.scene.clone(),
            loading: None,
            timing_since: std::time::Instant::now(),
        };
        if settings.mode == VrMode::Headset {
            notes.extend(stage.start_headset(settings, window, main));
        }
        Ok((stage, notes))
    }

    /// Show the scene in a headset too; why it isn't, if it can't be.
    #[cfg_attr(not(xr), allow(unused_variables))]
    fn start_headset(
        &mut self,
        settings: &VrSettings,
        window: &sdl2::video::Window,
        main: &sdl2::video::GLContext,
    ) -> Option<String> {
        #[cfg(xr)]
        {
            let textures = [self.screens[0].texture, self.screens[1].texture, self.screens[2].texture];
            match headset::Headset::start(window, main, self.glsl, self.scene.clone(), textures, settings) {
                Ok(headset) => {
                    self.headset = Some(headset);
                    None
                }
                Err(e) => Some(format!("[VR] No headset ({}); the scene is shown in the window", e)),
            }
        }
        #[cfg(not(xr))]
        Some("[VR] This build has no OpenXR; the scene is shown in the window".to_string())
    }

    /// The headset's session over, and the window showing the scene
    /// through its own camera, with `gl` current.
    #[cfg_attr(not(xr), allow(unused_variables))]
    fn stop_headset(&mut self, gl: &glow::Context) {
        #[cfg(xr)]
        if let Some(headset) = self.headset.take() {
            headset.close(gl);
            self.shown = None;
        }
    }

    /// Delete everything it made, the headset's session ended first, with
    /// `gl` current. (A scene still being read is let go of.)
    pub fn close(mut self, gl: &glow::Context) {
        self.stop_headset(gl);
        self.gpu.delete(gl);
        for screen in self.screens {
            screen.delete(gl);
        }
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
        self.gpu.frame_start(gl);
        if let Some(texture) = screen.filter(|_| std::mem::take(&mut self.fresh)) {
            self.gpu.prepare(gl, &self.scene, texture);
        }
        let aspect = drawable.0 as f32 / drawable.1 as f32;
        let view = View { view: self.camera.view(), projection: self.camera.projection(aspect) };
        let format = Format { size: drawable, srgb: false };
        if let Err(e) = self.gpu.render(gl, &self.scene, &[view], Dest::Own(format), drawable, screen, self.leds, &[]) {
            eprintln!("[VR] {}", e);
            clear_window(gl, drawable);
            return;
        }
        self.gpu.copy_to(gl, None, (0, 0, drawable.0 as i32, drawable.1 as i32));
        self.gpu.mark(gl, Pass::Copy);
        self.gpu.frame_end();
        if self.timing_since.elapsed().as_secs() >= 3 {
            self.timing_since = std::time::Instant::now();
            if let Some(line) = self.gpu.timing_report() {
                eprintln!("[VR] {}", line);
            }
        }
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

    /// What the headset's thread has to say (once it is over, the window
    /// shows the scene), and a scene read since, which takes the place of
    /// the one shown, with `gl` current.
    pub fn poll(&mut self, gl: &glow::Context) -> Vec<String> {
        let mut notes = self.poll_scene(gl);
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            let (said, ended) = headset.poll();
            if ended {
                self.headset = None;
            }
            notes.extend(said);
        }
        notes
    }

    /// Read the scene `path` (None: the test room) on a thread of its own,
    /// to show once it is read.
    fn switch_scene(&mut self, path: Option<PathBuf>) {
        let wanted = self.loading.as_ref().map_or(&self.scene_path, |(path, _)| path);
        if *wanted == path {
            return;
        }
        let (done, result) = std::sync::mpsc::channel();
        let reading = path.clone();
        let spawned = std::thread::Builder::new().name("vr-scene".into()).spawn(move || {
            let _ = done.send(load(reading.as_deref()));
        });
        // (Without a thread, the scene stays.)
        if spawned.is_ok() {
            self.loading = Some((path, result));
        }
    }

    /// The scene read, in place of the one shown: in the window, and in
    /// the headset. Kept if it can't be drawn.
    fn poll_scene(&mut self, gl: &glow::Context) -> Vec<String> {
        let Some((path, result)) = &self.loading else { return Vec::new() };
        let read = match result.try_recv() {
            Ok(read) => read,
            Err(TryRecvError::Empty) => return Vec::new(),
            Err(TryRecvError::Disconnected) => Err("reading it stopped".to_string()),
        };
        let path = path.clone();
        self.loading = None;
        let scene = match read.and_then(|scene| Ok((Gpu::new(gl, self.glsl, &scene, self.quality, window_options())?, scene))) {
            Ok((gpu, scene)) => {
                std::mem::replace(&mut self.gpu, gpu).delete(gl);
                self.gpu.set_glow(self.glow);
                Arc::new(scene)
            }
            Err(e) => {
                // Tried again when it is chosen again.
                self.scene_path = path;
                return vec![format!("The scene can't be shown: {}", e)];
            }
        };
        self.scene_path = path;
        self.scene = scene.clone();
        self.camera = FlyCamera::at(scene.spawn);
        self.fresh = self.latest.is_some();
        self.shown = None;
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            headset.set_scene(scene);
        }
        let name = self.scene_path.as_deref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        vec![format!("Showing {}", name.as_deref().unwrap_or("the test room"))]
    }

    /// The PC's lights, as the machine's are.
    pub fn set_leds(&mut self, leds: Leds) {
        self.leds = leds;
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            headset.set_leds(leds);
        }
    }

    /// Take on the `[vr]` settings but for `mode` off, which closes it: the
    /// headset started or stopped, and the lighting worked out again at
    /// another quality, in the window and the headset. What is worth
    /// saying about it.
    pub fn apply(
        &mut self,
        gl: &glow::Context,
        settings: &VrSettings,
        window: &sdl2::video::Window,
        main: &sdl2::video::GLContext,
    ) -> Vec<String> {
        let mut notes = Vec::new();
        let mut restart = false;
        if settings.quality != self.quality {
            match Gpu::new(gl, self.glsl, &self.scene, settings.quality, window_options()) {
                Ok(gpu) => {
                    std::mem::replace(&mut self.gpu, gpu).delete(gl);
                    self.quality = settings.quality;
                    self.fresh = self.latest.is_some();
                    // The headset's thread lights with its own, made as it starts.
                    restart = self.has_headset();
                }
                Err(e) => notes.push(format!("[VR] The lighting can't be changed: {}", e)),
            }
        }
        if (settings.graphics, settings.resolution) != self.headset_options {
            self.headset_options = (settings.graphics, settings.resolution);
            restart |= self.has_headset();
        }
        if settings.mode != self.mode || restart {
            self.stop_headset(gl);
            if settings.mode == VrMode::Headset {
                notes.extend(self.start_headset(settings, window, main));
            }
            self.mode = settings.mode;
        }
        self.fit = settings.screen_fit;
        self.glow = glow_of(settings);
        self.gpu.set_glow(self.glow);
        self.switch_scene(settings.scene.clone());
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            headset.set_controllers(settings.controllers);
            headset.set_glow(glow_of(settings));
            headset.set_placement(headset::Placement::of(settings));
        }
        notes
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

/// The scene of the file `path`, or the test room.
fn load(path: Option<&std::path::Path>) -> Result<Scene, String> {
    match path {
        Some(path) => Scene::load(path),
        None => Ok(Scene::test_room()),
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
