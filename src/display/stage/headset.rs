//! The VR headset on a thread of its own, so that a long batch of the
//! machine's (a recompile, a disk load) never makes the headset miss a
//! frame. The thread has an OpenGL context of its own, sharing textures
//! with the window's, on a hidden window of its own (so that neither the
//! context nor the device context OpenXR is given is the one the main
//! thread swaps), and owns the OpenXR session from its start to its end:
//! it waits for each of the
//! headset's frames, reads the controllers, draws the eyes with the newest
//! picture the main thread finished, and leaves the left eye's view for
//! the window.
//!
//! Pictures go between the threads in rings of three textures: the drawing
//! side draws into one that is neither the newest nor the one the other
//! side reads, then publishes it with a fence the reading side waits for on
//! the GPU before it reads. Moving on to a newer one, the reading side
//! leaves a fence after its reads of the old one, which the drawing side
//! waits for on the GPU before it draws again. Neither side deletes or
//! reallocates a texture the other could be using.

use super::controls::{Controllers, VrInput};
use super::render::{Dest, Features, Format, Gpu, Multiview, Options, Pass};
use super::scene::{Leds, Scene, Spawn};
use super::xr::{Xr, XrOptions, log};
use crate::video::shader::Glsl;
use glam::{Mat4, Vec3};
use glow::HasContext;
use rust_dos::vr::{VrControllers, VrSettings};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Make Xlib safe for the headset's thread's OpenGL, before SDL opens the
/// display: its GLX calls go through the window's X connection too. With
/// the headset on, tell SDL to make the window's context the kind the
/// session will draw with (`startup_hints`), as the runtime offers.
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
pub fn before_sdl(settings: &VrSettings) {
    #[cfg(target_os = "linux")]
    if settings.mode == rust_dos::vr::VrMode::Headset {
        use rust_dos::vr::{Platform, VrGraphics, startup_hints};
        // (graphics=gl needs nothing asked: X11 it is.)
        let offered = match settings.graphics {
            VrGraphics::Gl => None,
            _ => super::xr::offered().inspect_err(|e| log::line(format!("Before starting: {}", e))).ok(),
        };
        let x11 = std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty());
        let hints = startup_hints(settings.graphics, offered, Platform::Linux { x11 });
        if let Some(offered) = offered {
            log::line(format!("Before starting, the runtime offers {}; SDL is told {:?}", offered.describe(), hints));
        }
        if hints.force_x11 {
            sdl2::hint::set_with_priority("SDL_VIDEODRIVER", "x11", &sdl2::hint::Hint::Override);
        }
        if hints.force_egl {
            sdl2::hint::set_with_priority("SDL_VIDEO_X11_FORCE_EGL", "1", &sdl2::hint::Hint::Override);
        }
    }
    #[cfg(target_os = "linux")]
    // SAFETY: XInitThreads takes nothing, and comes before any other Xlib
    // call of the program's (SDL's come after).
    unsafe {
        if let Ok(xlib) = libloading::Library::new("libX11.so.6")
            && let Ok(init) = xlib.get::<unsafe extern "C" fn() -> std::ffi::c_int>(b"XInitThreads\0")
        {
            init();
            // Xlib stays loaded for SDL.
            std::mem::forget(xlib);
        }
    }
}

/// Write what the OpenXR runtime and OpenGL offer the headset
/// (`--vr-probe`), with a context made as the headset's thread's is, on a
/// hidden window.
pub fn probe(video: &sdl2::VideoSubsystem, settings: &VrSettings) {
    let attr = video.gl_attr();
    attr.set_context_profile(sdl2::video::GLProfile::Core);
    let made = video.window("Rust-DOS VR probe", 64, 64).opengl().hidden().build().map_err(|e| e.to_string()).and_then(|window| {
        let mut context = Err(String::new());
        for newer in NEWER_GL.into_iter().chain([(3, 2)]) {
            attr.set_context_version(newer.0, newer.1);
            context = window.gl_create_context();
            if context.is_ok() {
                break;
            }
        }
        Ok((context?, window))
    });
    let (context, window) = match made {
        Ok(made) => made,
        Err(e) => {
            println!("No OpenGL context for the probe: {}", e);
            return;
        }
    };
    if let Err(e) = window.gl_make_current(&context) {
        println!("No OpenGL context for the probe: {}", e);
        return;
    }
    // SAFETY: the context just made current.
    let gl = unsafe { glow::Context::from_loader_function(|name| video.gl_get_proc_address(name).cast()) };
    super::xr::probe::run(&gl, video.current_video_driver(), settings);
    // (The context goes before its window.)
    drop(context);
    drop(window);
}

/// Where the headset's space is in the scene, from the settings' seat
/// and scale (`VrSettings`): its origin, where the view is centred, is at
/// the scene's `spawn` and the seat's shift, turned the spawn's way and
/// the seat's turn, and its metres are the scale's fraction of the scene's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Metres right, up and back of the spawn, as it faces.
    shift: Vec3,
    /// Radians to the left.
    turn: f32,
    /// How many times bigger the scene looks.
    scale: f32,
}

impl Default for Placement {
    fn default() -> Self {
        Placement::of(&VrSettings::default())
    }
}

impl Placement {
    pub fn of(settings: &VrSettings) -> Self {
        let [right, up, forward] = settings.seat.map(|cm| cm as f32 / 100.0);
        Placement {
            shift: Vec3::new(right, up, -forward),
            turn: (settings.seat_turn as f32).to_radians(),
            scale: settings.scene_scale.max(1) as f32 / 100.0,
        }
    }

    /// The space's matrix in the scene of `spawn`.
    pub fn world(self, spawn: Spawn) -> Mat4 {
        Mat4::from_translation(spawn.position)
            * Mat4::from_rotation_y(spawn.yaw)
            * Mat4::from_translation(self.shift)
            * Mat4::from_rotation_y(self.turn)
            * Mat4::from_scale(Vec3::splat(1.0 / self.scale))
    }
}

/// The OpenGL versions the headset's context is asked for, newest first.
const NEWER_GL: [(u8, u8); 4] = [(4, 6), (4, 5), (4, 3), (4, 0)];

/// A GPU fence, which the other thread's context waits for.
pub struct Fence(glow::Fence);

// SAFETY: a sync object belongs to the share group, which both threads'
// contexts are in; only one side holds each.
unsafe impl Send for Fence {}

/// Three textures, one being drawn, the newest finished, and the one
/// being read.
pub struct Ring<F, M> {
    /// The newest finished: its index, its fence while the reader hasn't
    /// taken it, and what goes with it.
    latest: Option<(usize, Option<F>, M)>,
    in_use: Option<usize>,
    /// The reader's fence after its reads of the texture it read before,
    /// while the writer hasn't waited for it.
    released: Option<F>,
}

impl<F, M> Default for Ring<F, M> {
    fn default() -> Self {
        Ring { latest: None, in_use: None, released: None }
    }
}

impl<F, M: Copy> Ring<F, M> {
    /// The texture to draw into next.
    pub fn free(&self) -> usize {
        let latest = self.latest.as_ref().map(|l| l.0);
        (0..3).find(|&i| Some(i) != latest && Some(i) != self.in_use).expect("one of three is free")
    }

    /// `index` is finished; the fence of the one it replaces, if that was
    /// never read, to delete.
    pub fn publish(&mut self, index: usize, fence: F, meta: M) -> Option<F> {
        self.latest.replace((index, Some(fence), meta)).and_then(|(_, fence, _)| fence)
    }

    /// The newest, to read from now on: its index, its fence if it is new
    /// to the reader, and what goes with it. Leaving another texture for
    /// it, the reader's `release` makes a fence after its reads of that
    /// one, for the writer (`released`); then the fence that replaces is
    /// returned too, to delete.
    pub fn take(&mut self, release: impl FnOnce() -> Option<F>) -> (Option<(usize, Option<F>, M)>, Option<F>) {
        let Some((index, fence, meta)) = self.latest.as_mut() else { return (None, None) };
        let mut stale = None;
        if self.in_use.is_some_and(|old| old != *index)
            && let Some(fence) = release()
        {
            stale = self.released.replace(fence);
        }
        self.in_use = Some(*index);
        (Some((*index, fence.take(), *meta)), stale)
    }

    /// The fence the writer waits for before it draws into a free texture
    /// again, then deletes, if the reader left one since.
    pub fn released(&mut self) -> Option<F> {
        self.released.take()
    }

    pub fn in_use(&self) -> Option<usize> {
        self.in_use
    }
}

/// What the threads share.
#[derive(Default)]
struct State {
    // To the headset.
    screen: Ring<Fence, ()>,
    leds: Leds,
    controllers: VrControllers,
    /// How brightly the screen lights the scene (`Gpu::set_glow`).
    glow: f32,
    placement: Placement,
    /// A scene to show instead, once the thread takes it.
    scene: Option<Arc<Scene>>,
    recenter: bool,
    // From the headset.
    /// The left eye's view, with its view-projection.
    mirror: Ring<Fence, Mat4>,
    mirror_textures: Vec<glow::Texture>,
    mirror_size: (u32, u32),
    input: VrInput,
    head: Option<Mat4>,
    notes: Vec<String>,
    running: bool,
    ended: bool,
}

struct Shared {
    state: Mutex<State>,
    stop: AtomicBool,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Say `note` in the window, and in the log.
    fn note(&self, note: String) {
        log::line(&note);
        self.lock().notes.push(note);
    }
}

/// A pointer for the thread: its hidden window or its context, which SDL
/// makes current there.
struct Raw(*mut std::ffi::c_void);

// SAFETY: SDL makes a context current on any one thread at a time; the
// hidden window and the context outlive the thread (`Headset::drop`).
unsafe impl Send for Raw {}

/// The main thread's side of the headset's thread.
pub struct Headset {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// The thread's context, deleted once the thread is over.
    context: Option<sdl2::video::GLContext>,
    /// The hidden window the thread's context draws with, closed after
    /// the context is deleted.
    window: Option<sdl2::video::Window>,
    /// The main thread's framebuffers of the mirror's textures.
    mirror_framebuffers: Vec<glow::Framebuffer>,
}

impl Headset {
    /// Start the thread, with a context made for it sharing `main`'s
    /// textures, drawing `scene` with the screen's `screens`.
    pub fn start(
        window: &sdl2::video::Window,
        main: &sdl2::video::GLContext,
        glsl: Glsl,
        scene: Arc<Scene>,
        screens: [glow::Texture; 3],
        settings: &VrSettings,
    ) -> Result<Self, String> {
        let (controllers, glow) = (settings.controllers, settings.screen_glow as f32 / 100.0);
        let settings = settings.clone();
        let video = window.subsystem();
        // (Which bindings the window's context allows is found out on the
        // thread, `Xr::new`: GLX under X11, EGL under Wayland or forced.)
        log::line(format!("SDL's video driver is {}", video.current_video_driver()));
        // A window of its own, with the same pixel format as the main
        // window's (the same attributes), never shown.
        let hidden = video.window("Rust-DOS headset", 64, 64).opengl().hidden().build().map_err(|e| e.to_string())?;
        let attr = video.gl_attr();
        window.gl_make_current(main)?;
        // The newest OpenGL there is, in the window's profile: OpenXR
        // runtimes want more than the window's 3.2 (SteamVR 4.3), and
        // call what they want of it whether the context has it or not.
        let version = attr.context_version();
        attr.set_share_with_current_context(true);
        let mut context = Err(String::new());
        for newer in NEWER_GL {
            attr.set_context_version(newer.0, newer.1);
            context = hidden.gl_create_context();
            if context.is_ok() {
                break;
            }
        }
        if context.is_err() {
            attr.set_context_version(version.0, version.1);
            context = hidden.gl_create_context();
        }
        attr.set_context_version(version.0, version.1);
        attr.set_share_with_current_context(false);
        // (Making it made it current here.)
        window.gl_make_current(main)?;
        let context = context?;
        let shared = Arc::new(Shared { state: Mutex::new(State { controllers, glow, placement: Placement::of(&settings), ..State::default() }), stop: AtomicBool::new(false) });
        // SAFETY: the context outlives the thread (`Headset::drop`).
        let (raw_window, raw_context) = (Raw(hidden.raw().cast()), Raw(unsafe { context.raw() }));
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("vr-headset".into())
            .spawn(move || run(thread_shared, raw_window, raw_context, glsl, scene, screens, settings))
            .map_err(|e| e.to_string())?;
        Ok(Headset {
            shared,
            thread: Some(thread),
            context: Some(context),
            window: Some(hidden),
            mirror_framebuffers: Vec::new(),
        })
    }

    /// The screen's texture the headset reads, not to be drawn into; and,
    /// before this returns, `gl` waits on the GPU until the headset's reads
    /// of the ones it read before are done.
    pub fn screen_in_use(&self, gl: &glow::Context) -> Option<usize> {
        let (in_use, released) = {
            let mut state = self.shared.lock();
            (state.screen.in_use(), state.screen.released())
        };
        if let Some(Fence(fence)) = released {
            // SAFETY: see `GlScreen`.
            unsafe {
                gl.wait_sync(fence, 0, glow::TIMEOUT_IGNORED);
                gl.delete_sync(fence);
            }
        }
        in_use
    }

    /// The screen's texture `index` is finished, by the commands sent so
    /// far.
    pub fn publish_screen(&self, gl: &glow::Context, index: usize) {
        // SAFETY: see `GlScreen`.
        let Ok(fence) = (unsafe { gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) }) else { return };
        // SAFETY: see `GlScreen`. The other context sees the commands once
        // they are sent.
        unsafe { gl.flush() };
        let old = self.shared.lock().screen.publish(index, Fence(fence), ());
        if let Some(Fence(old)) = old {
            // SAFETY: see `GlScreen`.
            unsafe { gl.delete_sync(old) };
        }
    }

    pub fn set_leds(&self, leds: Leds) {
        self.shared.lock().leds = leds;
    }

    pub fn set_controllers(&self, controllers: VrControllers) {
        self.shared.lock().controllers = controllers;
    }

    /// Show `scene` from now on.
    pub fn set_scene(&self, scene: Arc<Scene>) {
        self.shared.lock().scene = Some(scene);
    }

    pub fn set_placement(&self, placement: Placement) {
        self.shared.lock().placement = placement;
    }

    pub fn set_glow(&self, glow: f32) {
        self.shared.lock().glow = glow;
    }

    pub fn recenter(&self) {
        self.shared.lock().recenter = true;
    }

    /// Whether the headset shows the scene.
    pub fn running(&self) -> bool {
        self.shared.lock().running
    }

    /// What the thread has to say, and whether it is over.
    pub fn poll(&self) -> (Vec<String>, bool) {
        let mut state = self.shared.lock();
        (std::mem::take(&mut state.notes), state.ended)
    }

    /// The controllers' input and the head, while the headset shows the
    /// scene.
    pub fn input(&self) -> Option<(VrInput, Option<Mat4>)> {
        let state = self.shared.lock();
        state.running.then_some((state.input, state.head))
    }

    /// Draw the headset's left eye into the window (`drawable` big, bound),
    /// letterboxed: its view-projection and where it went (x, y from the
    /// top, width, height). None while the headset shows nothing.
    pub fn draw_mirror(&mut self, gl: &glow::Context, drawable: (u32, u32)) -> Option<(Mat4, (f32, f32, f32, f32))> {
        let (taken, textures, size) = {
            let mut state = self.shared.lock();
            if !state.running {
                return None;
            }
            let (taken, stale) = state.mirror.take(|| release(gl));
            if let Some(Fence(stale)) = stale {
                // SAFETY: see `GlScreen`.
                unsafe { gl.delete_sync(stale) };
            }
            (taken, state.mirror_textures.clone(), state.mirror_size)
        };
        let (index, fence, view_projection) = taken?;
        // SAFETY: see `GlScreen`.
        unsafe {
            if let Some(Fence(fence)) = fence {
                gl.wait_sync(fence, 0, glow::TIMEOUT_IGNORED);
                gl.delete_sync(fence);
            }
            while self.mirror_framebuffers.len() < textures.len() {
                let texture = textures[self.mirror_framebuffers.len()];
                let framebuffer = gl.create_framebuffer().ok()?;
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
                self.mirror_framebuffers.push(framebuffer);
            }
            let (x, y, w, h) = super::super::letterbox(drawable, size);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(0, 0, drawable.0 as i32, drawable.1 as i32);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.mirror_framebuffers[index]));
            let bottom = (drawable.1 - y - h) as i32;
            let (sw, sh) = (size.0 as i32, size.1 as i32);
            let rect = (x as i32, bottom, x as i32 + w as i32, bottom + h as i32);
            gl.blit_framebuffer(0, 0, sw, sh, rect.0, rect.1, rect.2, rect.3, glow::COLOR_BUFFER_BIT, glow::LINEAR);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            Some((view_projection, (x as f32, y as f32, w as f32, h as f32)))
        }
    }
}

impl Headset {
    /// End the session and the thread, and delete the main thread's
    /// framebuffers of the mirror, with `gl` current.
    pub fn close(mut self, gl: &glow::Context) {
        for framebuffer in std::mem::take(&mut self.mirror_framebuffers) {
            // SAFETY: see `GlScreen`.
            unsafe { gl.delete_framebuffer(framebuffer) };
        }
    }
}

/// A fence after the commands `gl` sent so far, sent on so that the other
/// thread's context can wait for it.
fn release(gl: &glow::Context) -> Option<Fence> {
    // SAFETY: see `GlScreen`.
    unsafe {
        let fence = gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0).ok()?;
        gl.flush();
        Some(Fence(fence))
    }
}

/// The thread ends the session and lets go of its context itself, before
/// the context and then its window go; all before the main window and its
/// context, which `GlScreen` drops after the scene.
impl Drop for Headset {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        drop(self.context.take());
        drop(self.window.take());
    }
}

/// The headset's thread.
#[allow(clippy::too_many_arguments)]
fn run(
    shared: Arc<Shared>,
    window: Raw,
    context: Raw,
    glsl: Glsl,
    scene: Arc<Scene>,
    screens: [glow::Texture; 3],
    settings: VrSettings,
) {
    // SAFETY: the window and the context are alive until the thread ends
    // (see `Headset::drop`), and the context is current nowhere else.
    let made = unsafe { sdl2::sys::SDL_GL_MakeCurrent(window.0.cast(), context.0) };
    if made != 0 {
        shared.note("No headset (its thread has no OpenGL context); the scene is shown in the window".into());
    } else {
        // SAFETY: the context just made current.
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                let name = std::ffi::CString::new(name).unwrap_or_default();
                sdl2::sys::SDL_GL_GetProcAddress(name.as_ptr()) as *const _
            })
        };
        if let Err(e) = session(&shared, &gl, glsl, scene, screens, &settings) {
            shared.note(e);
        }
        // SAFETY: as above.
        unsafe {
            gl.finish();
            sdl2::sys::SDL_GL_MakeCurrent(window.0.cast(), std::ptr::null_mut());
        }
    }
    let mut state = shared.lock();
    state.running = false;
    state.ended = true;
}

/// The headset's session, until it ends or the main thread stops it.
fn session(
    shared: &Shared,
    gl: &glow::Context,
    glsl: Glsl,
    mut scene: Arc<Scene>,
    screens: [glow::Texture; 3],
    settings: &VrSettings,
) -> Result<(), String> {
    let features = Features::of(gl);
    // A standalone headset's own graphics chip is gone easier on
    // (RUST_DOS_VR_STANDALONE=1 or 0 says whether it is one, to compare).
    let mobile = match std::env::var("RUST_DOS_VR_STANDALONE").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => rust_dos::vr::mobile_gpu(&features.renderer),
    };
    let look = settings.look(mobile);
    let resolution = settings.headset_resolution(mobile);
    let samples = look.samples;
    // Both eyes in one pass where OpenGL can (and multisample them, if
    // they are); RUST_DOS_VR_MULTIVIEW=0 draws them one by one, to compare.
    let multiview = Multiview::load(&features, |name| {
        let name = std::ffi::CString::new(name).unwrap_or_default();
        // SAFETY: this thread's context is current.
        unsafe { sdl2::sys::SDL_GL_GetProcAddress(name.as_ptr()) as *const _ }
    })
    .filter(|m| samples == 0 || m.multisamples())
    .filter(|_| std::env::var("RUST_DOS_VR_MULTIVIEW").map_or(true, |v| v != "0"));
    let options = XrOptions {
        graphics: settings.graphics,
        resolution: resolution.percent,
        layered: multiview.is_some(),
        refresh: settings.refresh,
        mobile,
    };
    let mut xr = Xr::new(gl, &options).map_err(|e| format!("No headset ({}); the scene is shown in the window", e))?;
    shared.note(xr.describe().to_string());
    log::line(features.describe());
    let gpu_options =
        Options { timed: true, samples, multiview: multiview.filter(|_| xr.layered()), ambient_occlusion: look.ambient_occlusion };
    // As much of the eyes' images drawn as there is time for, where it
    // adapts (and the images can be drawn in part).
    let adaptive = resolution.adaptive.filter(|_| xr.partial());
    let mut percent = adaptive.map_or(100, |(_, _, start)| start);
    xr.set_percent(percent);
    let mut adapted = std::time::Instant::now();
    log::line(format!(
        "{}; lighting {}, ambient occlusion {}, {} samples a pixel, {}; {}",
        if mobile { "A standalone headset's graphics chip" } else { "A PC's graphics chip" },
        look.quality.name(),
        look.ambient_occlusion.map_or("as the lighting", |on| if on { "on" } else { "off" }),
        samples.max(1),
        match adaptive {
            Some((min, max, _)) => format!("eyes {}% of the recommended size, drawn at {} to {}% of that", resolution.percent, min, max),
            None => format!("eyes {}% of the recommended size", resolution.percent),
        },
        if gpu_options.multiview.is_some() { "both eyes drawn at once" } else { "the eyes drawn one by one" }
    ));
    if let Ok(parts) = std::env::var("RUST_DOS_VR_LEAVE_OUT") {
        log::line(format!("Left out of the views, to time them: {}", parts));
    }
    let mut gpu = Gpu::new(gl, glsl, &scene, look.quality, gpu_options)?;
    // The window's view of the left eye, at half its size.
    let ((ew, eh), srgb) = xr.eye_format();
    let size = ((ew / 2).max(1), (eh / 2).max(1));
    let mut mirror = Vec::new();
    // SAFETY: see `GlScreen`: this thread's context is current.
    unsafe {
        let format = if srgb { glow::SRGB8_ALPHA8 } else { glow::RGBA8 };
        for _ in 0..3 {
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            let none = glow::PixelUnpackData::Slice(None);
            let (w, h) = (size.0 as i32, size.1 as i32);
            gl.tex_image_2d(glow::TEXTURE_2D, 0, format as i32, w, h, 0, glow::RGBA, glow::UNSIGNED_BYTE, none);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            let framebuffer = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
            mirror.push((texture, framebuffer));
        }
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.bind_texture(glow::TEXTURE_2D, None);
    }
    {
        let mut state = shared.lock();
        state.mirror_textures = mirror.iter().map(|m| m.0).collect();
        state.mirror_size = size;
    }
    let mut controllers = Controllers::default();
    let mut screen: Option<usize> = None;
    let mut reported = std::time::Instant::now();
    let dump_prefix = std::env::var("RUST_DOS_VR_DUMP").ok();
    let mut failed: Option<String> = None;
    let mut frame = 0u32;
    while !shared.stop.load(Ordering::Relaxed) {
        // Another scene, with the screen's picture lighting it: the old one
        // stays if it can't be drawn.
        let next = shared.lock().scene.take();
        if let Some(next) = next {
            match Gpu::new(gl, glsl, &next, look.quality, gpu_options) {
                Ok(made) => {
                    std::mem::replace(&mut gpu, made).delete(gl);
                    if let Some(index) = screen {
                        gpu.prepare(gl, &next, screens[index]);
                    }
                    scene = next;
                }
                Err(e) => shared.note(format!("The headset can't show the scene: {}", e)),
            }
        }
        let (notes, lost) = xr.poll();
        for note in notes {
            shared.note(note);
        }
        if lost {
            xr.close(gl);
            return Ok(());
        }
        shared.lock().running = xr.running();
        if !xr.wait_frame() {
            std::thread::sleep(std::time::Duration::from_millis(20));
            continue;
        }
        let (taken, leds, mode, recenter, mirror_index, mirror_released, glow, placement) = {
            let mut state = shared.lock();
            let (taken, stale) = state.screen.take(|| release(gl));
            if let Some(Fence(stale)) = stale {
                // SAFETY: see `GlScreen`.
                unsafe { gl.delete_sync(stale) };
            }
            let (mirror_index, mirror_released) = (state.mirror.free(), state.mirror.released());
            (taken, state.leds, state.controllers, std::mem::take(&mut state.recenter), mirror_index, mirror_released, state.glow, state.placement)
        };
        if let Some(Fence(fence)) = mirror_released {
            // SAFETY: see `GlScreen`: the window's reads of the mirror's
            // textures before are done before one is drawn again.
            unsafe {
                gl.wait_sync(fence, 0, glow::TIMEOUT_IGNORED);
                gl.delete_sync(fence);
            }
        }
        gpu.set_glow(glow);
        gpu.frame_start(gl);
        if let Some((index, fence, ())) = taken {
            if let Some(Fence(fence)) = fence {
                // SAFETY: see `GlScreen`.
                unsafe {
                    gl.wait_sync(fence, 0, glow::TIMEOUT_IGNORED);
                    gl.delete_sync(fence);
                }
            }
            screen = Some(index);
            gpu.prepare(gl, &scene, screens[index]);
        }
        if recenter {
            xr.recenter();
        }
        if let Err(e) = xr.begin() {
            // (A failure the session can't go on from ends it at `poll`.)
            log::line(e);
            continue;
        }
        let world = placement.world(scene.spawn);
        let tracking = xr.track(world);
        let (input, extras, hold) = controllers.update(mode, &tracking, &scene.screen, std::time::Instant::now());
        if hold {
            xr.recenter();
        }
        let picture = screen.map(|i| screens[i]);
        let mut left = None;
        let drawn = xr.draw(gl, world, |target, views| {
            let format = Format { size: target.size, srgb: target.srgb };
            let dest = Dest::Image { texture: target.texture, format, layered: target.layered };
            if let Err(e) = gpu.render(gl, &scene, views, dest, target.area, picture, leds, &extras) {
                // (Once, not every frame.)
                if failed.as_ref() != Some(&e) {
                    log::line(&e);
                    failed = Some(e);
                }
                return;
            }
            let (aw, ah) = (target.area.0 as i32, target.area.1 as i32);
            if target.first == 0
                && let Some(read) = gpu.read_framebuffer(gl, target.texture, target.layered.then_some(0))
            {
                let (w, h) = (size.0 as i32, size.1 as i32);
                // SAFETY: see `GlScreen`.
                unsafe {
                    gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(read));
                    gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(mirror[mirror_index].1));
                    gl.blit_framebuffer(0, 0, aw, ah, 0, 0, w, h, glow::COLOR_BUFFER_BIT, glow::LINEAR);
                    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                    if let Ok(fence) = gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) {
                        left = Some((fence, views[0].view_projection()));
                    }
                }
            }
            gpu.mark(gl, Pass::Copy);
            if frame == DUMP_FRAME
                && let Some(prefix) = &dump_prefix
            {
                for i in 0..views.len() {
                    let layer = target.layered.then_some(i as i32);
                    if let Some(read) = gpu.read_framebuffer(gl, target.texture, layer) {
                        dump(gl, read, target.area, &format!("{}-{}.ppm", prefix, target.first + i));
                    }
                }
            }
            // The eyes' commands on their way before their image is
            // released.
            // SAFETY: see `GlScreen`.
            unsafe { gl.flush() };
        });
        gpu.frame_end();
        frame += 1;
        if let Err(e) = drawn {
            log::line(e);
        }
        if let Some((min, max, _)) = adaptive
            && adapted.elapsed() >= ADAPT_EVERY
        {
            adapted = std::time::Instant::now();
            if let Some(ms) = gpu.recent_frame_time() {
                percent = rust_dos::vr::adapt(percent, ms, xr.period_ms(), (min, max));
                xr.set_percent(percent);
            }
        }
        if reported.elapsed() >= log::FrameStats::PERIOD {
            reported = std::time::Instant::now();
            if let Some(line) = gpu.timing_report() {
                match adaptive {
                    Some(_) => log::line(format!("{}; the eyes drawn at {}%", line, percent)),
                    None => log::line(line),
                }
            }
        }
        let mut state = shared.lock();
        if let Some((fence, view_projection)) = left
            && let Some(Fence(old)) = state.mirror.publish(mirror_index, Fence(fence), view_projection)
        {
            // SAFETY: see `GlScreen`.
            unsafe { gl.delete_sync(old) };
        }
        state.input = input;
        state.head = tracking.head;
    }
    xr.close(gl);
    Ok(())
}

/// How often the part of the eyes' images drawn may change.
const ADAPT_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

/// The frame whose eyes RUST_DOS_VR_DUMP writes.
const DUMP_FRAME: u32 = 300;

/// Write the picture in `framebuffer`, `size` big, to `path` as a PPM: the
/// eyes as drawn, to compare the ways of drawing them (RUST_DOS_VR_DUMP=
/// the files' prefix).
fn dump(gl: &glow::Context, framebuffer: glow::Framebuffer, size: (u32, u32), path: &str) {
    let (w, h) = (size.0 as usize, size.1 as usize);
    let mut pixels = vec![0u8; w * h * 4];
    // SAFETY: see `GlScreen`.
    unsafe {
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(framebuffer));
        gl.read_buffer(glow::COLOR_ATTACHMENT0);
        let out = glow::PixelPackData::Slice(Some(&mut pixels));
        gl.read_pixels(0, 0, w as i32, h as i32, glow::RGBA, glow::UNSIGNED_BYTE, out);
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
    }
    let mut ppm = format!("P6\n{} {}\n255\n", w, h).into_bytes();
    for row in pixels.chunks(w * 4).rev() {
        ppm.extend(row.chunks(4).flat_map(|p| [p[0], p[1], p[2]]));
    }
    match std::fs::write(path, ppm) {
        Ok(()) => log::line(format!("Wrote {}", path)),
        Err(e) => log::line(format!("Can't write {}: {}", path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seat_moves_and_turns_as_the_spawn_faces() {
        // A spawn turned to the left (looking along -X).
        let spawn = Spawn { position: Vec3::new(1.0, 1.2, 0.0), yaw: std::f32::consts::FRAC_PI_2, pitch: 0.0 };
        let at = |settings: &VrSettings| Placement::of(settings).world(spawn);
        let near = |a: Vec3, b: Vec3| (a - b).length() < 1e-5;
        assert!(near(at(&VrSettings::default()).transform_point3(Vec3::ZERO), spawn.position));
        // Closer to the screen is ahead (-X); to the right, -Z; up, +Y.
        let seat = VrSettings { seat: [10, 20, 30], ..VrSettings::default() };
        assert!(near(at(&seat).transform_point3(Vec3::ZERO), Vec3::new(1.0 - 0.3, 1.4, -0.1)));
        // Turned a quarter more to the left, ahead is +Z.
        let turned = VrSettings { seat_turn: 90, ..VrSettings::default() };
        assert!(near(at(&turned).transform_vector3(Vec3::NEG_Z), Vec3::Z));
        // At 200%, a real metre is half a metre of the scene, from the seat.
        let big = VrSettings { scene_scale: 200, ..VrSettings::default() };
        assert!(near(at(&big).transform_point3(Vec3::new(0.0, 1.0, 0.0)), spawn.position + Vec3::new(0.0, 0.5, 0.0)));
    }

    #[test]
    fn rings_never_draw_over_what_is_read_or_newest() {
        let mut ring: Ring<u32, ()> = Ring::default();
        assert_eq!(ring.free(), 0);
        assert_eq!(ring.publish(0, 10, ()), None);
        assert_eq!(ring.free(), 1);
        // A newer one before the reader looked: the old fence goes.
        assert_eq!(ring.publish(1, 11, ()), Some(10));
        // The reader takes the newest and its fence, once; reading nothing
        // before, it releases nothing.
        assert_eq!(ring.take(|| Some(90)), (Some((1, Some(11), ())), None));
        assert_eq!(ring.take(|| panic!("the same texture")), (Some((1, None, ())), None));
        assert_eq!(ring.released(), None);
        let next = ring.free();
        assert!(next != 1);
        assert_eq!(ring.publish(next, 12, ()), None);
        // Neither the newest nor the one being read.
        let free = ring.free();
        assert!(free != next && free != 1);
        // Moving on, the reader leaves a fence for the writer.
        assert_eq!(ring.take(|| Some(91)), (Some((next, Some(12), ())), None));
        let after = ring.free();
        assert_eq!(ring.publish(after, 13, ()), None);
        // Moving on again before the writer waited: the newer fence covers
        // the older, which goes.
        assert_eq!(ring.take(|| Some(92)), (Some((after, Some(13), ())), Some(91)));
        assert_eq!(ring.released(), Some(92));
        assert_eq!(ring.released(), None);
    }
}
