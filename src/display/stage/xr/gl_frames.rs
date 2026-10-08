//! The eyes' swapchains of a session drawn with OpenGL (through GLX, WGL
//! or EGL): their images are textures of the context current on the
//! headset's thread, drawn into through a framebuffer each.

use super::err;
use glow::HasContext;
use openxr as xr;

/// A graphics binding whose swapchain images are OpenGL textures.
pub trait GlGraphics: xr::Graphics<Format = u32, SwapchainImage = u32> {}

impl<G: xr::Graphics<Format = u32, SwapchainImage = u32>> GlGraphics for G {}

struct Eye<G: GlGraphics> {
    swapchain: xr::Swapchain<G>,
    /// A framebuffer for each of the swapchain's images.
    framebuffers: Vec<glow::Framebuffer>,
    size: (u32, u32),
    /// The image acquired, between `acquire` and `release`.
    acquired: Option<usize>,
}

pub struct GlFrames<G: GlGraphics> {
    eyes: Vec<Eye<G>>,
    stream: xr::FrameStream<G>,
    /// The eyes' swapchains encode linear light as sRGB themselves.
    srgb: bool,
}

/// The format of the eyes' images out of the session's `formats`, and
/// whether it is sRGB.
pub fn pick_format(formats: &[u32]) -> Result<(u32, bool), String> {
    if formats.contains(&glow::SRGB8_ALPHA8) {
        Ok((glow::SRGB8_ALPHA8, true))
    } else if formats.contains(&glow::RGBA8) {
        Ok((glow::RGBA8, false))
    } else {
        Err(format!("the headset takes none of OpenGL's RGBA8 formats ({:x?})", formats))
    }
}

impl<G: GlGraphics> GlFrames<G> {
    /// A swapchain for each of the eyes, `sizes` big, with a framebuffer
    /// for each of its images.
    pub fn new(
        gl: &glow::Context,
        session: &xr::Session<G>,
        stream: xr::FrameStream<G>,
        sizes: &[(u32, u32)],
    ) -> Result<Self, String> {
        let formats = session.enumerate_swapchain_formats().map_err(err("the headset's formats"))?;
        let (format, srgb) = pick_format(&formats)?;
        let mut eyes = Vec::new();
        for &(width, height) in sizes {
            let swapchain = session
                .create_swapchain(&xr::SwapchainCreateInfo {
                    create_flags: xr::SwapchainCreateFlags::EMPTY,
                    usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT | xr::SwapchainUsageFlags::TRANSFER_DST,
                    format,
                    sample_count: 1,
                    width,
                    height,
                    face_count: 1,
                    array_size: 1,
                    mip_count: 1,
                })
                .map_err(err("the headset's swapchain"))?;
            let images = swapchain.enumerate_images().map_err(err("the headset's swapchain"))?;
            let mut framebuffers = Vec::new();
            for image in images {
                let texture = std::num::NonZeroU32::new(image).map(glow::NativeTexture).ok_or("an empty swapchain image")?;
                // SAFETY: see `GlScreen`.
                unsafe {
                    let framebuffer = gl.create_framebuffer()?;
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                    gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
                    let complete = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
                    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                    if !complete {
                        return Err("OpenGL can't draw into the headset's images".into());
                    }
                    framebuffers.push(framebuffer);
                }
            }
            eyes.push(Eye { swapchain, framebuffers, size: (width, height), acquired: None });
        }
        Ok(GlFrames { eyes, stream, srgb })
    }

    pub fn eye_format(&self) -> ((u32, u32), bool) {
        (self.eyes.first().map_or((0, 0), |e| e.size), self.srgb)
    }

    pub fn eyes(&self) -> usize {
        self.eyes.len()
    }

    pub fn begin(&mut self) -> xr::Result<()> {
        self.stream.begin().map(|_| ())
    }

    /// Acquire an image of eye `index` and wait for it: its framebuffer,
    /// and its size.
    pub fn acquire(&mut self, index: usize) -> Result<(glow::Framebuffer, (u32, u32)), (&'static str, xr::sys::Result)> {
        let eye = &mut self.eyes[index];
        let image = eye.swapchain.acquire_image().map_err(|e| ("acquiring the headset's image", e))?;
        if let Err(e) = eye.swapchain.wait_image(xr::Duration::INFINITE) {
            let _ = eye.swapchain.release_image();
            return Err(("waiting for the headset's image", e));
        }
        eye.acquired = Some(image as usize);
        match eye.framebuffers.get(image as usize) {
            Some(&framebuffer) => Ok((framebuffer, eye.size)),
            None => {
                let _ = self.release(index);
                Err(("the headset's image", xr::sys::Result::ERROR_VALIDATION_FAILURE))
            }
        }
    }

    /// Release the image of eye `index` acquired, drawn.
    pub fn release(&mut self, index: usize) -> Result<(), (&'static str, xr::sys::Result)> {
        let eye = &mut self.eyes[index];
        if eye.acquired.take().is_none() {
            return Ok(());
        }
        eye.swapchain.release_image().map_err(|e| ("releasing the headset's image", e))
    }

    /// End the frame: the eyes drawn from `views`, or nothing.
    pub fn end(
        &mut self,
        time: xr::Time,
        blend: xr::EnvironmentBlendMode,
        space: &xr::Space,
        views: Option<&[xr::View]>,
    ) -> xr::Result<()> {
        let Some(views) = views else { return self.stream.end(time, blend, &[]) };
        let projection_views: Vec<_> = views
            .iter()
            .zip(&self.eyes)
            .map(|(view, eye)| {
                let rect = xr::Rect2Di {
                    offset: xr::Offset2Di { x: 0, y: 0 },
                    extent: xr::Extent2Di { width: eye.size.0 as i32, height: eye.size.1 as i32 },
                };
                xr::CompositionLayerProjectionView::new().pose(view.pose).fov(view.fov).sub_image(
                    xr::SwapchainSubImage::new().swapchain(&eye.swapchain).image_array_index(0).image_rect(rect),
                )
            })
            .collect();
        let layer = xr::CompositionLayerProjection::new().space(space).views(&projection_views);
        self.stream.end(time, blend, &[&layer])
    }

    /// Delete the framebuffers, with `gl` current.
    pub fn delete(&mut self, gl: &glow::Context) {
        for eye in &mut self.eyes {
            for framebuffer in eye.framebuffers.drain(..) {
                // SAFETY: see `GlScreen`.
                unsafe { gl.delete_framebuffer(framebuffer) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_is_taken_first() {
        assert_eq!(pick_format(&[glow::RGBA8, glow::SRGB8_ALPHA8]), Ok((glow::SRGB8_ALPHA8, true)));
        assert_eq!(pick_format(&[0x8058 /* RGBA8 */, 0x881A]), Ok((glow::RGBA8, false)));
        assert!(pick_format(&[0x881A /* RGBA16F */]).is_err());
    }
}
