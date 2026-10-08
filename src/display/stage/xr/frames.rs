//! The eyes' swapchains and the frames of the session, whatever it draws
//! with: each eye is drawn with OpenGL into a framebuffer either way.

use super::gl_frames::{Acquired, Failure, GlFrames};
use openxr as xr;

pub enum Frames {
    /// OpenGL through GLX or WGL.
    Gl(GlFrames<xr::OpenGL>),
    /// OpenGL through EGL.
    #[cfg(target_os = "linux")]
    Egl(GlFrames<super::egl::Egl>),
    /// OpenGL into images Vulkan copies into the runtime's.
    #[cfg(target_os = "linux")]
    Vulkan(Box<super::vulkan::Bridge>),
}

/// The same call on whichever frames there are.
macro_rules! each {
    ($frames:expr, $f:ident => $call:expr) => {
        match $frames {
            Frames::Gl($f) => $call,
            #[cfg(target_os = "linux")]
            Frames::Egl($f) => $call,
            #[cfg(target_os = "linux")]
            Frames::Vulkan($f) => $call,
        }
    };
}

impl Frames {
    /// The size of an eye's image, and whether it is sRGB.
    pub fn eye_format(&self) -> ((u32, u32), bool) {
        each!(self, frames => frames.eye_format())
    }

    /// The formats the session offered, and whether they are Vulkan's
    /// (else OpenGL's).
    pub fn formats(&self) -> (&[u32], bool) {
        match self {
            Frames::Gl(frames) => (frames.formats(), false),
            #[cfg(target_os = "linux")]
            Frames::Egl(frames) => (frames.formats(), false),
            #[cfg(target_os = "linux")]
            Frames::Vulkan(bridge) => (bridge.formats(), true),
        }
    }

    pub fn eyes(&self) -> usize {
        each!(self, frames => frames.eyes())
    }

    pub fn begin(&mut self) -> xr::Result<()> {
        each!(self, frames => frames.begin())
    }

    /// The framebuffer to draw eye `index` into, and its size.
    pub fn acquire(&mut self, index: usize) -> Acquired {
        each!(self, frames => frames.acquire(index))
    }

    /// Eye `index` is drawn, with `gl`.
    pub fn release(&mut self, gl: &glow::Context, index: usize) -> Result<(), Failure> {
        match self {
            Frames::Gl(frames) => frames.release(index),
            #[cfg(target_os = "linux")]
            Frames::Egl(frames) => frames.release(index),
            #[cfg(target_os = "linux")]
            Frames::Vulkan(bridge) => bridge.release(gl, index),
        }
    }

    /// End the frame: the eyes drawn from `views`, or nothing.
    pub fn end(
        &mut self,
        time: xr::Time,
        blend: xr::EnvironmentBlendMode,
        space: &xr::Space,
        views: Option<&[xr::View]>,
    ) -> xr::Result<()> {
        each!(self, frames => frames.end(time, blend, space, views))
    }

    /// Delete what was made with `gl`, current.
    pub fn delete(&mut self, gl: &glow::Context) {
        each!(self, frames => frames.delete(gl))
    }
}
