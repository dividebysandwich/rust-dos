//! The VR headset on a thread of its own, so that a long batch of the
//! machine's (a recompile, a disk load) never makes the headset miss a
//! frame. The thread has an OpenGL context of its own, sharing textures
//! with the window's, and owns the OpenXR session: it waits for each of the
//! headset's frames, reads the controllers, draws the eyes with the newest
//! picture the main thread finished, and leaves the left eye's view for
//! the window.
//!
//! Pictures go between the threads in rings of three textures: the drawing
//! side draws into one that is neither the newest nor the one the other
//! side reads, then publishes it with a fence the reading side waits for on
//! the GPU before it reads.

use super::controls::{Controllers, VrInput};
use super::render::{Format, Gpu};
use super::scene::{Leds, Scene};
use super::xr::Xr;
use crate::video::shader::Glsl;
use glam::Mat4;
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
}

impl<F, M> Default for Ring<F, M> {
    fn default() -> Self {
        Ring { latest: None, in_use: None }
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
    /// to the reader, and what goes with it.
    pub fn take(&mut self) -> Option<(usize, Option<F>, M)> {
        let (index, fence, meta) = self.latest.as_mut()?;
        self.in_use = Some(*index);
        Some((*index, fence.take(), *meta))
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

/// A pointer for the thread: the window or its context, which SDL makes
/// current there.
struct Raw(*mut std::ffi::c_void);

// SAFETY: SDL makes a context current on any one thread at a time; the
// window outlives the thread (`Headset` is dropped first).
unsafe impl Send for Raw {}

/// The main thread's side of the headset's thread.
pub struct Headset {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// The thread's context, deleted once the thread is over.
    context: Option<sdl2::video::GLContext>,
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
        let attr = window.subsystem().gl_attr();
        attr.set_share_with_current_context(true);
        let context = window.gl_create_context();
        attr.set_share_with_current_context(false);
        let context = context?;
        window.gl_make_current(main)?;
        let shared = Arc::new(Shared { state: Mutex::new(State { controllers, glow, ..State::default() }), stop: AtomicBool::new(false) });
        // SAFETY: the context outlives the thread (`Headset::drop`).
        let (raw_window, raw_context) = (Raw(window.raw().cast()), Raw(unsafe { context.raw() }));
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("vr-headset".into())
            .spawn(move || run(thread_shared, raw_window, raw_context, glsl, scene, screens, quality))
            .map_err(|e| e.to_string())?;
        Ok(Headset { shared, thread: Some(thread), context: Some(context), mirror_framebuffers: Vec::new() })
    }

    /// The screen's texture the headset reads, not to be drawn into.
    pub fn screen_in_use(&self) -> Option<usize> {
        self.shared.lock().screen.in_use()
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
            (state.mirror.take(), state.mirror_textures.clone(), state.mirror_size)
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

impl Drop for Headset {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // The thread let go of it.
        drop(self.context.take());
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
        if let Err(e) = session(&shared, &gl, glsl, &scene, screens, quality) {
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
    scene: &Scene,
    screens: [glow::Texture; 3],
    quality: VrQuality,
) -> Result<(), String> {
    let mut gpu = Gpu::new(gl, glsl, scene, quality)?;
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
        let (taken, leds, mode, recenter, mirror_index, glow) = {
            let mut state = shared.lock();
            let taken = state.screen.take();
            (taken, state.leds, state.controllers, std::mem::take(&mut state.recenter), state.mirror.free(), state.glow)
        };
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
            eprintln!("[VR] {}", e);
            continue;
        }
        let tracking = xr.track(scene.spawn);
        let (input, extras, hold) = controllers.update(mode, &tracking, &scene.screen, std::time::Instant::now());
        if hold {
            xr.recenter();
        }
        let picture = screen.map(|i| screens[i]);
        let mut left = None;
        let drawn = xr.draw(scene.spawn, |eye, view, eye_size, srgb, framebuffer| {
            if let Err(e) = gpu.render(gl, scene, &view, Format { size: eye_size, srgb }, picture, leds, &extras) {
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
    fn rings_never_draw_over_what_is_read_or_newest() {
        let mut ring: Ring<u32, ()> = Ring::default();
        assert_eq!(ring.free(), 0);
        assert_eq!(ring.publish(0, 10, ()), None);
        assert_eq!(ring.free(), 1);
        // A newer one before the reader looked: the old fence goes.
        assert_eq!(ring.publish(1, 11, ()), Some(10));
        // The reader takes the newest and its fence, once.
        assert_eq!(ring.take(), Some((1, Some(11), ())));
        assert_eq!(ring.take(), Some((1, None, ())));
        let next = ring.free();
        assert!(next != 1);
        assert_eq!(ring.publish(next, 12, ()), None);
        // Neither the newest nor the one being read.
        let free = ring.free();
        assert!(free != next && free != 1);
    }
}
