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

    /// The swapchains: one for both eyes if `layered`.
    pub fn eyes(&self) -> usize {
        each!(self, frames => frames.eyes())
    }

    /// Both eyes are the layers of one swapchain's images.
    pub fn layered(&self) -> bool {
        match self {
            Frames::Gl(frames) => frames.layered(),
            #[cfg(target_os = "linux")]
            Frames::Egl(frames) => frames.layered(),
            #[cfg(target_os = "linux")]
            Frames::Vulkan(_) => false,
        }
    }

    /// Less than the whole of the images can be drawn (`end`'s area).
    pub fn partial(&self) -> bool {
        match self {
            #[cfg(target_os = "linux")]
            Frames::Vulkan(_) => false,
            _ => true,
        }
    }

    pub fn begin(&mut self) -> xr::Result<()> {
        each!(self, frames => frames.begin())
    }

    /// The texture to draw swapchain `index`'s eye (or eyes) into, and
    /// its size.
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

    /// Send on what the eyes drawn so far still hold back, with `gl`:
    /// before a frame is ended, whether all its eyes were drawn or not.
    pub fn flush(&mut self, gl: &glow::Context) -> Result<(), Failure> {
        match self {
            #[cfg(target_os = "linux")]
            Frames::Vulkan(bridge) => bridge.copy_drawn(gl),
            _ => {
                let _ = gl;
                Ok(())
            }
        }
    }

    /// End the frame: the eyes drawn from `views`, `area` of their images
    /// (where `partial`), or nothing.
    pub fn end(
        &mut self,
        time: xr::Time,
        blend: xr::EnvironmentBlendMode,
        space: &xr::Space,
        views: Option<&[xr::View]>,
        area: (u32, u32),
    ) -> xr::Result<()> {
        match self {
            Frames::Gl(frames) => frames.end(time, blend, space, views, area),
            #[cfg(target_os = "linux")]
            Frames::Egl(frames) => frames.end(time, blend, space, views, area),
            #[cfg(target_os = "linux")]
            Frames::Vulkan(bridge) => bridge.end(time, blend, space, views),
        }
    }

    /// Delete what was made with `gl`, current.
    pub fn delete(&mut self, gl: &glow::Context) {
        each!(self, frames => frames.delete(gl))
    }
}
