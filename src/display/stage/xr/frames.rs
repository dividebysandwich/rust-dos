//! The eyes' swapchains and the frames of the session, whatever it draws
//! with: each eye is drawn with OpenGL into a framebuffer either way.

use super::gl_frames::GlFrames;
use openxr as xr;

pub enum Frames {
    /// OpenGL through GLX or WGL.
    Gl(GlFrames<xr::OpenGL>),
}

impl Frames {
    /// The size of an eye's image, and whether it is sRGB.
    pub fn eye_format(&self) -> ((u32, u32), bool) {
        match self {
            Frames::Gl(frames) => frames.eye_format(),
        }
    }

    pub fn eyes(&self) -> usize {
        match self {
            Frames::Gl(frames) => frames.eyes(),
        }
    }

    pub fn begin(&mut self) -> xr::Result<()> {
        match self {
            Frames::Gl(frames) => frames.begin(),
        }
    }

    /// The framebuffer to draw eye `index` into, and its size.
    pub fn acquire(&mut self, index: usize) -> Result<(glow::Framebuffer, (u32, u32)), (&'static str, xr::sys::Result)> {
        match self {
            Frames::Gl(frames) => frames.acquire(index),
        }
    }

    /// Eye `index` is drawn.
    pub fn release(&mut self, index: usize) -> Result<(), (&'static str, xr::sys::Result)> {
        match self {
            Frames::Gl(frames) => frames.release(index),
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
        match self {
            Frames::Gl(frames) => frames.end(time, blend, space, views),
        }
    }

    /// Delete what was made with `gl`, current.
    pub fn delete(&mut self, gl: &glow::Context) {
        match self {
            Frames::Gl(frames) => frames.delete(gl),
        }
    }
}
