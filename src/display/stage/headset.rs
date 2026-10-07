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
use super::render::{Format, Gpu};
use super::scene::{Leds, Scene, Spawn};
use super::xr::Xr;
use crate::video::shader::Glsl;
use glam::{Mat4, Vec3};
use glow::HasContext;
use rust_dos::vr::{VrControllers, VrQuality, VrSettings};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Make Xlib safe for the headset's thread's OpenGL, before SDL opens the
/// display: its GLX calls go through the window's X connection too.
pub fn before_sdl() {
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

    fn note(&self, note: String) {
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
        let (controllers, quality, glow) = (settings.controllers, settings.quality, settings.screen_glow as f32 / 100.0);
        // A window of its own, with the same pixel format as the main
        // window's (the same attributes), never shown.
        let video = window.subsystem();
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
        let shared = Arc::new(Shared { state: Mutex::new(State { controllers, glow, placement: Placement::of(settings), ..State::default() }), stop: AtomicBool::new(false) });
        // SAFETY: the context outlives the thread (`Headset::drop`).
        let (raw_window, raw_context) = (Raw(hidden.raw().cast()), Raw(unsafe { context.raw() }));
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("vr-headset".into())
            .spawn(move || run(thread_shared, raw_window, raw_context, glsl, scene, screens, quality))
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
fn run(
    shared: Arc<Shared>,
    window: Raw,
    context: Raw,
    glsl: Glsl,
    scene: Arc<Scene>,
    screens: [glow::Texture; 3],
    quality: VrQuality,
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
        if let Err(e) = session(&shared, &gl, glsl, scene, screens, quality) {
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
    quality: VrQuality,
) -> Result<(), String> {
    let mut gpu = Gpu::new(gl, glsl, &scene, quality)?;
    let mut xr = Xr::new(gl).map_err(|e| format!("No headset ({}); the scene is shown in the window", e))?;
    shared.note(xr.describe().to_string());
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
    while !shared.stop.load(Ordering::Relaxed) {
        // Another scene, with the screen's picture lighting it: the old one
        // stays if it can't be drawn.
        let next = shared.lock().scene.take();
        if let Some(next) = next {
            match Gpu::new(gl, glsl, &next, quality) {
                Ok(made) => {
                    std::mem::replace(&mut gpu, made).delete(gl);
                    if let Some(index) = screen {
                        gpu.prepare(gl, screens[index]);
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
        if let Some((index, fence, ())) = taken {
            if let Some(Fence(fence)) = fence {
                // SAFETY: see `GlScreen`.
                unsafe {
                    gl.wait_sync(fence, 0, glow::TIMEOUT_IGNORED);
                    gl.delete_sync(fence);
                }
            }
            screen = Some(index);
            gpu.prepare(gl, screens[index]);
        }
        if recenter {
            xr.recenter();
        }
        if let Err(e) = xr.begin() {
            // (A failure the session can't go on from ends it at `poll`.)
            eprintln!("[VR] {}", e);
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
        let drawn = xr.draw(world, |eye, view, eye_size, srgb, framebuffer| {
            if let Err(e) = gpu.render(gl, &scene, &view, Format { size: eye_size, srgb }, picture, leds, &extras) {
                eprintln!("[VR] {}", e);
                return;
            }
            gpu.copy_to(gl, Some(framebuffer), (0, 0, eye_size.0 as i32, eye_size.1 as i32));
            if eye == 0 {
                let (w, h) = (size.0 as i32, size.1 as i32);
                gpu.copy_to(gl, Some(mirror[mirror_index].1), (0, 0, w, h));
                // SAFETY: see `GlScreen`.
                unsafe {
                    if let Ok(fence) = gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) {
                        gl.flush();
                        left = Some((fence, view.view_projection()));
                    }
                }
            }
        });
        if let Err(e) = drawn {
            eprintln!("[VR] {}", e);
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
    Ok(())
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
