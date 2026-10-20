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
//! point at the screen as the mouse and are a gamepad. The game's manual
//! and a joystick lie on the desk, to be picked up in the headset, and
//! with `[vfx1]` a VFX1 headset to put on, which shows each eye the
//! picture the game draws for it.

mod camera;
mod controls;
#[cfg(xr)]
mod foveation;
mod gi;
#[cfg(xr)]
mod headset;
mod keycaps;
#[cfg(xr)]
mod keyboard;
mod photo;
mod physics;
mod pick;
mod props;
mod render;
mod scene;
mod shadow;
mod sky;
mod spatial;
mod typing;
#[cfg(xr)]
mod visor;
mod world;
#[cfg(xr)]
mod xr;

use crate::video::shader::Glsl;
use camera::FlyCamera;
pub use controls::VrInput;
pub use typing::KeyEvent;
use super::audio_mix::Mix;
use glam::{Mat4, Vec2};
use glow::HasContext;
use render::{Dest, Format, Gpu, Options, Pass, Punch, ScreenTarget, View};
use rust_dos::vr::{ScreenFit, VrMode, VrSettings};
use scene::Scene;
use rust_dos::vr::{Leds, VrGraphics, VrLook};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use world::World;
use std::time::SystemTime;

/// A scene being read: its path, when it was written, and the result.
type SceneLoad = (Option<PathBuf>, Option<SystemTime>, Receiver<Result<Scene, String>>);

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
    /// The settings the window's view is drawn with, worked out for its
    /// graphics chip (`mobile`).
    look: VrLook,
    mobile: bool,
    /// How the headset's session draws, its eyes' resolution and its
    /// refresh rate, and whether the cameras show the keyboard, which it
    /// is started again for.
    headset_options: (VrGraphics, Option<u32>, Option<u32>, bool),
    glow: f32,
    /// The scene file shown (None: the test room) and when it was written,
    /// and the one being read on a thread of its own to take its place.
    /// Written again (an update downloaded in its place), it is read again.
    scene_path: Option<PathBuf>,
    scene_written: Option<SystemTime>,
    loading: Option<SceneLoad>,
    /// When the window's frame times were last said (RUST_DOS_VR_TIMING).
    timing_since: std::time::Instant,
    /// The props as the window shows them (where the scene has them), and
    /// the game's manuals, which they haven't taken yet if `fresh_manuals`.
    props: props::Props,
    manuals: Vec<PathBuf>,
    fresh_manuals: bool,
    /// The lamps, the time of day and the weather, which the headset's
    /// thread and the sound share.
    world: Arc<World>,
    /// Where the camera's pictures go, the cameras clicked to take one,
    /// and what the threads saving them said.
    capture_dir: Option<PathBuf>,
    shots: Vec<usize>,
    photo_notes: Arc<std::sync::Mutex<Vec<String>>>,
    /// The VFX1 on the desk (`[vfx1]`), and its eyes' pictures for the
    /// headset, left and right, three of each as the screen's, and the
    /// newest.
    vfx1: bool,
    eyes: [[glow::Texture; 2]; 3],
    eyes_latest: Option<usize>,
}

/// How the window's view is drawn, as `look` says: timed with
/// RUST_DOS_VR_TIMING set.
fn window_options(look: VrLook) -> Options {
    Options {
        timed: std::env::var_os("RUST_DOS_VR_TIMING").is_some(),
        samples: look.samples,
        multiview: None,
        ambient_occlusion: look.ambient_occlusion,
    }
}

/// What the headset's session is started again for when it changes.
fn headset_options(settings: &VrSettings) -> (VrGraphics, Option<u32>, Option<u32>, bool) {
    (settings.graphics, settings.resolution, settings.refresh, settings.keyboard != rust_dos::vr::VrKeyboard::Off)
}

/// Width and height.
type Size = (u32, u32);

/// Where the window's camera starts in `scene`: at its spawn, or where
/// RUST_DOS_VR_CAMERA ("x,y,z,yaw,pitch", degrees) says, for pictures of
/// the scene from elsewhere.
fn start_camera(scene: &Scene) -> FlyCamera {
    let set: Option<Vec<f32>> = std::env::var("RUST_DOS_VR_CAMERA").ok().map(|v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect());
    match set.as_deref() {
        Some(&[x, y, z, yaw, pitch]) => FlyCamera::at(scene::Spawn {
            position: glam::Vec3::new(x, y, z),
            yaw: yaw.to_radians(),
            pitch: pitch.to_radians(),
        }),
        _ => FlyCamera::at(scene.spawn),
    }
}

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
        let mobile = rust_dos::vr::mobile_gpu(&render::Features::of(gl).renderer);
        let look = settings.look(mobile);
        let mut gpu = Gpu::new(gl, glsl, &scene, look.quality, window_options(look))?;
        gpu.set_glow(glow_of(settings));
        let mut screens = Vec::new();
        for _ in 0..3 {
            let screen = ScreenTarget::new(gl)?;
            // SAFETY: see `GlScreen`.
            unsafe { gl.bind_texture(glow::TEXTURE_2D, Some(screen.texture)) };
            gpu.set_anisotropy(gl);
            screens.push(screen);
        }
        let camera = start_camera(&scene);
        let props = props::Props::new(&scene);
        let world = Arc::new(World::new(settings));
        let eye = || -> Result<glow::Texture, String> {
            // SAFETY: see `GlScreen`.
            unsafe {
                let texture = gl.create_texture()?;
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                for (name, value) in [
                    (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                    (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
                }
                gl.bind_texture(glow::TEXTURE_2D, None);
                Ok(texture)
            }
        };
        let eyes = [[eye()?, eye()?], [eye()?, eye()?], [eye()?, eye()?]];
        world.set_scene(&scene);
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
            look,
            mobile,
            headset_options: headset_options(settings),
            glow: glow_of(settings),
            scene_path: settings.scene.clone(),
            scene_written: written(settings.scene.as_deref()),
            loading: None,
            timing_since: std::time::Instant::now(),
            props,
            manuals: Vec::new(),
            fresh_manuals: false,
            world,
            capture_dir: None,
            shots: Vec::new(),
            photo_notes: Default::default(),
            vfx1: false,
            eyes,
            eyes_latest: None,
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
            let (scene, world) = (self.scene.clone(), self.world.clone());
            let started = headset::Headset::start(
                window, main, self.glsl, scene, textures, self.eyes, settings, world,
            );
            match started {
                Ok(headset) => {
                    headset.set_manuals(self.manuals.clone());
                    headset.set_vfx1(self.vfx1);
                    if let Some(dir) = &self.capture_dir {
                        headset.set_capture_dir(dir.clone());
                    }
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
        self.props.delete(gl);
        self.gpu.delete(gl);
        for screen in self.screens {
            screen.delete(gl);
        }
        for texture in self.eyes.into_iter().flatten() {
            // SAFETY: see `GlScreen`.
            unsafe { gl.delete_texture(texture) };
        }
    }

    /// The VFX1 on the desk (`[vfx1]`), or not.
    pub fn set_vfx1(&mut self, there: bool) {
        if there == self.vfx1 {
            return;
        }
        self.vfx1 = there;
        self.props.set_vfx1(there);
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            headset.set_vfx1(there);
        }
    }

    /// The VFX1's head tracker and puck, while the headset wears it.
    pub fn vfx1(&self) -> Option<rust_dos::vfx1::Tracking> {
        #[cfg(xr)]
        return self.headset.as_ref().and_then(|h| h.vfx1());
        #[cfg(not(xr))]
        None
    }

    /// The pictures the VFX1's eyes see, `width` degrees across, for the
    /// headset to show.
    #[cfg_attr(not(xr), allow(unused_variables))]
    pub fn show_eyes(
        &mut self,
        gl: &glow::Context,
        left: &crate::video::Frame,
        right: &crate::video::Frame,
        width: f32,
    ) {
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            let in_use = headset.eyes_in_use(gl);
            let free = (0..3)
                .find(|&i| Some(i) != self.eyes_latest && Some(i) != in_use)
                .unwrap_or(0);
            for (texture, frame) in self.eyes[free].iter().zip([left, right]) {
                let (w, h) = (frame.width.max(1) as i32, frame.height.max(1) as i32);
                let pixels = (frame.rgba.len() >= (w * h * 4) as usize).then_some(&frame.rgba[..]);
                // SAFETY: see `GlScreen`.
                unsafe {
                    gl.bind_texture(glow::TEXTURE_2D, Some(*texture));
                    gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                    let data = glow::PixelUnpackData::Slice(pixels);
                    let (format, kind) = (glow::RGBA, glow::UNSIGNED_BYTE);
                    let internal = glow::RGBA8 as i32;
                    gl.tex_image_2d(glow::TEXTURE_2D, 0, internal, w, h, 0, format, kind, data);
                    gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
                    gl.bind_texture(glow::TEXTURE_2D, None);
                }
            }
            headset.publish_eyes(gl, free, width);
            self.eyes_latest = Some(free);
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
        let now = std::time::Instant::now();
        self.gpu.set_ambience(gl, &self.scene, self.world.ambience(&self.scene, now));
        if let Some(texture) = screen.filter(|_| std::mem::take(&mut self.fresh)) {
            self.gpu.prepare(gl, &self.scene, texture);
        }
        let aspect = drawable.0 as f32 / drawable.1 as f32;
        let view = View { view: self.camera.view(), projection: self.camera.projection(aspect) };
        let format = Format { size: drawable, srgb: false };
        if std::mem::take(&mut self.fresh_manuals) {
            self.props.set_manuals(gl, self.manuals.clone());
        }
        for camera in std::mem::take(&mut self.shots) {
            let notes = self.photo_notes.clone();
            let said = move |note: String| notes.lock().unwrap_or_else(|e| e.into_inner()).push(note);
            let dir = self.capture_dir.clone();
            photo::take(gl, &mut self.gpu, &self.scene, &mut self.props, &self.world, camera, screen, self.leds, dir, said, now);
        }
        let props = self.props.draws(gl, &self.gpu, &self.scene, now);
        if let Err(e) = self.gpu.render(gl, &self.scene, &[view], Dest::Own(format), drawable, screen, self.leds, &props, &[], Punch::None) {
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

    /// A click of the mouse at a point of the window's drawable: a lamp
    /// there in front of the screen is switched. Whether it was something
    /// of the scene's (and not the screen's).
    pub fn click(&mut self, (x, y): (f32, f32)) -> bool {
        let Some((view_projection, (rx, ry, rw, rh))) = self.shown else { return false };
        let ndc = Vec2::new((x - rx) / rw.max(1.0) * 2.0 - 1.0, 1.0 - (y - ry) / rh.max(1.0) * 2.0);
        let (origin, dir) = pick::ray(view_projection.inverse(), ndc);
        let screen = pick::pick_hit(&self.scene.screen, origin, dir).map(|(t, _)| t);
        let targets = self.props.targets(&self.scene);
        match pick::pick_target(&targets, origin, dir) {
            Some((t, id)) if screen.is_none_or(|s| t < s) => {
                self.act(id);
                true
            }
            _ => false,
        }
    }

    /// The mouse at a point of the window's drawable: a key of the
    /// keyboard there, in front of the screen, shows lit, as a click
    /// would press it. (In the headset, the laser does that.)
    pub fn hover(&mut self, (x, y): (f32, f32)) {
        #[cfg(xr)]
        if self.headset.as_ref().is_some_and(|h| h.running()) {
            return;
        }
        let Some((view_projection, (rx, ry, rw, rh))) = self.shown else { return };
        let ndc = Vec2::new((x - rx) / rw.max(1.0) * 2.0 - 1.0, 1.0 - (y - ry) / rh.max(1.0) * 2.0);
        let (origin, dir) = pick::ray(view_projection.inverse(), ndc);
        let screen = pick::pick_hit(&self.scene.screen, origin, dir).map(|(t, _)| t);
        let targets = self.props.targets(&self.scene);
        let key = match pick::pick_target(&targets, origin, dir) {
            Some((t, pick::TargetId::Key(k))) if screen.is_none_or(|s| t < s) => Some(k),
            _ => None,
        };
        self.props.set_pointed_key(key);
    }

    /// Do what clicking `id` does: switch a lamp, or take a camera's
    /// picture (in the headset, while it shows the scene).
    fn act(&mut self, id: pick::TargetId) {
        match id {
            pick::TargetId::Lamp(i) => self.world.toggle_lamp(i),
            pick::TargetId::Camera(i) => {
                #[cfg(xr)]
                if let Some(headset) = self.headset.as_ref().filter(|h| h.running()) {
                    headset.take_picture(i);
                    return;
                }
                self.shots.push(i);
            }
            pick::TargetId::Key(k) => {
                #[cfg(xr)]
                if let Some(headset) = self.headset.as_ref().filter(|h| h.running()) {
                    headset.tap_key(k);
                    return;
                }
                self.props.tap(k, std::time::Instant::now());
            }
        }
    }

    /// The keys typed on the keyboard in the scene since, for the machine.
    pub fn take_keys(&mut self) -> Vec<KeyEvent> {
        #[allow(unused_mut)]
        let mut keys = self.props.take_key_events(std::time::Instant::now());
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            keys.extend(headset.take_keys());
        }
        keys
    }

    /// Where the camera's pictures go.
    pub fn set_capture_dir(&mut self, dir: &std::path::Path) {
        if self.capture_dir.as_deref() != Some(dir) {
            self.capture_dir = Some(dir.to_path_buf());
            #[cfg(xr)]
            if let Some(headset) = &self.headset {
                headset.set_capture_dir(dir.to_path_buf());
            }
        }
    }

    /// The window's camera, to fly.
    pub fn camera_mut(&mut self) -> &mut FlyCamera {
        &mut self.camera
    }

    /// Back to where the scene starts, and the headset's view centred
    /// where it looks now.
    pub fn recenter(&mut self) {
        self.camera = start_camera(&self.scene);
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            headset.recenter();
        }
    }

    /// Mark the real keyboard with a controller, in the headset: false
    /// without one.
    pub fn mark_keyboard(&mut self) -> bool {
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            headset.mark_keyboard();
            return true;
        }
        false
    }

    /// A key of the host's was pressed (where the fingers are finds the
    /// keyboard).
    pub fn key_pressed(&self) {
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            headset.key_pressed();
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
        notes.append(&mut self.photo_notes.lock().unwrap_or_else(|e| e.into_inner()));
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
        let when = written(path.as_deref());
        let wanted = self.loading.as_ref().map_or((&self.scene_path, self.scene_written), |(path, when, _)| (path, *when));
        if wanted == (&path, when) {
            return;
        }
        let (done, result) = std::sync::mpsc::channel();
        let reading = path.clone();
        let spawned = std::thread::Builder::new().name("vr-scene".into()).spawn(move || {
            let _ = done.send(load(reading.as_deref()));
        });
        // (Without a thread, the scene stays.)
        if spawned.is_ok() {
            self.loading = Some((path, when, result));
        }
    }

    /// The scene read, in place of the one shown: in the window, and in
    /// the headset. Kept if it can't be drawn.
    fn poll_scene(&mut self, gl: &glow::Context) -> Vec<String> {
        let Some((path, when, result)) = &self.loading else { return Vec::new() };
        let read = match result.try_recv() {
            Ok(read) => read,
            Err(TryRecvError::Empty) => return Vec::new(),
            Err(TryRecvError::Disconnected) => Err("reading it stopped".to_string()),
        };
        let (path, when) = (path.clone(), *when);
        self.loading = None;
        let scene = match read.and_then(|scene| Ok((Gpu::new(gl, self.glsl, &scene, self.look.quality, window_options(self.look))?, scene))) {
            Ok((gpu, scene)) => {
                std::mem::replace(&mut self.gpu, gpu).delete(gl);
                self.gpu.set_glow(self.glow);
                Arc::new(scene)
            }
            Err(e) => {
                // Tried again when it is chosen again, or written again.
                self.scene_path = path;
                self.scene_written = when;
                return vec![format!("The scene can't be shown: {}", e)];
            }
        };
        self.scene_path = path;
        self.scene_written = when;
        self.scene = scene.clone();
        self.camera = start_camera(&scene);
        self.props.reset(&scene);
        self.world.set_scene(&scene);
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
        self.props.set_machine_keys(leds.keys);
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            headset.set_leds(leds);
        }
    }

    /// The game's manuals, for the booklet on the desk: none hides it.
    pub fn set_manuals(&mut self, manuals: &[PathBuf]) {
        if manuals == self.manuals.as_slice() {
            return;
        }
        self.manuals = manuals.to_vec();
        self.fresh_manuals = true;
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            headset.set_manuals(self.manuals.clone());
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
        let look = settings.look(self.mobile);
        if look != self.look {
            match Gpu::new(gl, self.glsl, &self.scene, look.quality, window_options(look)) {
                Ok(gpu) => {
                    std::mem::replace(&mut self.gpu, gpu).delete(gl);
                    self.look = look;
                    self.fresh = self.latest.is_some();
                    // The headset's thread lights with its own, made as it starts.
                    restart = self.has_headset();
                }
                Err(e) => notes.push(format!("[VR] The lighting can't be changed: {}", e)),
            }
        }
        if headset_options(settings) != self.headset_options {
            self.headset_options = headset_options(settings);
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
        self.world.apply(settings);
        self.glow = glow_of(settings);
        self.gpu.set_glow(self.glow);
        self.switch_scene(settings.scene.clone());
        #[cfg(xr)]
        if let Some(headset) = &self.headset {
            headset.set_controllers(settings.controllers);
            headset.set_glow(glow_of(settings));
            headset.set_placement(headset::Placement::of(settings));
            headset.set_keyboard(settings.keyboard);
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

    /// The room's own sounds for the listener now: the rain from the
    /// windows' side, the thunder's loudness, and what plays once.
    pub fn room_sound(&self) -> (super::room_sound::Ambience, Vec<super::room_sound::Effect>) {
        let (rain, thunder, effects) = self.world.sound(std::time::Instant::now());
        let listener = self.listener();
        let at = listener.transform_point3(glam::Vec3::ZERO);
        let right = listener.transform_vector3(glam::Vec3::X).normalize_or(glam::Vec3::X);
        let nearest = self.scene.windows.iter().min_by(|a, b| a.distance(at).total_cmp(&b.distance(at)));
        let pan = nearest.map_or(0.0, |w| (*w - at).normalize_or_zero().dot(right));
        let indoor = self.scene.indoor.is_some_and(|(low, high)| at.cmpge(low).all() && at.cmple(high).all());
        (super::room_sound::Ambience { rain, pan, indoor, thunder }, effects)
    }

    /// Where the listener is: the headset's head while it shows the scene,
    /// else the window's camera.
    fn listener(&self) -> Mat4 {
        #[cfg(xr)]
        let head = self.headset.as_ref().and_then(|h| h.input()).and_then(|(_, head)| head);
        #[cfg(not(xr))]
        let head: Option<Mat4> = None;
        head.unwrap_or_else(|| self.camera.view().inverse())
    }

    /// How the sound's channels mix for the listener: the headset's head
    /// while it shows the scene, else the window's camera.
    pub fn audio_mix(&self) -> Mix {
        let listener = self.listener();
        let spawn = self.scene.spawn;
        let start = Mat4::from_translation(spawn.position) * Mat4::from_rotation_y(spawn.yaw);
        spatial::mix(listener, start, self.scene.speakers)
    }
}

/// The scene of the file `path`, or the test room.
/// When the scene file `path` was last written, if it can be said.
fn written(path: Option<&std::path::Path>) -> Option<SystemTime> {
    std::fs::metadata(path?).and_then(|m| m.modified()).ok()
}

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
