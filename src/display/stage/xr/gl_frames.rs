//! The eyes' swapchains of a session drawn with OpenGL (through GLX, WGL
//! or EGL): their images are textures of the context current on the
//! headset's thread, drawn into through a framebuffer each.

use super::err;
use openxr as xr;

/// A graphics binding whose swapchain images are OpenGL textures.
pub trait GlGraphics: xr::Graphics<Format = u32, SwapchainImage = u32> {}

impl<G: xr::Graphics<Format = u32, SwapchainImage = u32>> GlGraphics for G {}

/// An eye's image acquired: its texture and size; or what failed.
pub type Acquired = Result<(glow::Texture, (u32, u32)), Failure>;

/// What failed, and how.
pub type Failure = (&'static str, xr::sys::Result);

struct Eye<G: GlGraphics> {
    swapchain: xr::Swapchain<G>,
    /// The swapchain's images.
    images: Vec<glow::Texture>,
    size: (u32, u32),
    /// The image acquired, between `acquire` and `release`.
    acquired: Option<usize>,
}

pub struct GlFrames<G: GlGraphics> {
    /// A swapchain for each eye, or one of two layers for both.
    eyes: Vec<Eye<G>>,
    layered: bool,
    stream: xr::FrameStream<G>,
    /// The eyes' swapchains encode linear light as sRGB themselves.
    srgb: bool,
    /// The formats the session offered.
    formats: Vec<u32>,
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
    /// A swapchain for each of the eyes, `sizes` big; or with `layered`,
    /// one of a layer for each, where the eyes are the same size.
    pub fn new(session: &xr::Session<G>, stream: xr::FrameStream<G>, sizes: &[(u32, u32)], layered: bool) -> Result<Self, String> {
        let formats = session.enumerate_swapchain_formats().map_err(err("the headset's formats"))?;
        let (format, srgb) = pick_format(&formats)?;
        let layered = layered && sizes.len() == 2 && sizes[0] == sizes[1];
        let chains: Vec<((u32, u32), u32)> = if layered { vec![(sizes[0], 2)] } else { sizes.iter().map(|&s| (s, 1)).collect() };
        let mut eyes = Vec::new();
        for ((width, height), array_size) in chains {
            let swapchain = session
                .create_swapchain(&xr::SwapchainCreateInfo {
                    create_flags: xr::SwapchainCreateFlags::EMPTY,
                    usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT | xr::SwapchainUsageFlags::TRANSFER_DST,
                    format,
                    sample_count: 1,
                    width,
                    height,
                    face_count: 1,
                    array_size,
                    mip_count: 1,
                })
                .map_err(err("the headset's swapchain"))?;
            let images = swapchain.enumerate_images().map_err(err("the headset's swapchain"))?;
            let images = images
                .into_iter()
                .map(|image| std::num::NonZeroU32::new(image).map(glow::NativeTexture).ok_or("an empty swapchain image"))
                .collect::<Result<Vec<_>, _>>()?;
            eyes.push(Eye { swapchain, images, size: (width, height), acquired: None });
        }
        Ok(GlFrames { eyes, layered, stream, srgb, formats })
    }

    pub fn eye_format(&self) -> ((u32, u32), bool) {
        (self.eyes.first().map_or((0, 0), |e| e.size), self.srgb)
    }

    pub fn formats(&self) -> &[u32] {
        &self.formats
    }

    /// The swapchains: one for both eyes if `layered`.
    pub fn eyes(&self) -> usize {
        self.eyes.len()
    }

    pub fn layered(&self) -> bool {
        self.layered
    }

    pub fn begin(&mut self) -> xr::Result<()> {
        self.stream.begin().map(|_| ())
    }

    /// Acquire an image of swapchain `index` and wait for it: its texture,
    /// and its size.
    pub fn acquire(&mut self, index: usize) -> Acquired {
        let eye = &mut self.eyes[index];
        let image = eye.swapchain.acquire_image().map_err(|e| ("acquiring the headset's image", e))?;
        if let Err(e) = eye.swapchain.wait_image(xr::Duration::INFINITE) {
            let _ = eye.swapchain.release_image();
            return Err(("waiting for the headset's image", e));
        }
        eye.acquired = Some(image as usize);
        match eye.images.get(image as usize) {
            Some(&texture) => Ok((texture, eye.size)),
            None => {
                let _ = self.release(index);
                Err(("the headset's image", xr::sys::Result::ERROR_VALIDATION_FAILURE))
            }
        }
    }

    /// Release the image of swapchain `index` acquired, drawn.
    pub fn release(&mut self, index: usize) -> Result<(), Failure> {
        let eye = &mut self.eyes[index];
        if eye.acquired.take().is_none() {
            return Ok(());
        }
        eye.swapchain.release_image().map_err(|e| ("releasing the headset's image", e))
    }

    /// End the frame: the eyes drawn from `views`, `area` of their images
    /// from the bottom left; or nothing.
    pub fn end(
        &mut self,
        time: xr::Time,
        blend: xr::EnvironmentBlendMode,
        space: &xr::Space,
        views: Option<&[xr::View]>,
        area: (u32, u32),
    ) -> xr::Result<()> {
        let Some(views) = views else { return self.stream.end(time, blend, &[]) };
        let layered = self.layered;
        let projection_views: Vec<_> = views
            .iter()
            .enumerate()
            .filter_map(|(i, view)| {
                let (eye, layer) = if layered { (self.eyes.first()?, i as u32) } else { (self.eyes.get(i)?, 0) };
                let rect = xr::Rect2Di {
                    offset: xr::Offset2Di { x: 0, y: 0 },
                    extent: xr::Extent2Di { width: area.0.min(eye.size.0) as i32, height: area.1.min(eye.size.1) as i32 },
                };
                Some(xr::CompositionLayerProjectionView::new().pose(view.pose).fov(view.fov).sub_image(
                    xr::SwapchainSubImage::new().swapchain(&eye.swapchain).image_array_index(layer).image_rect(rect),
                ))
            })
            .collect();
        let layer = xr::CompositionLayerProjection::new().space(space).views(&projection_views);
        self.stream.end(time, blend, &[&layer])
    }

    /// Nothing of OpenGL's to delete: the runtime's textures go with the
    /// swapchains.
    pub fn delete(&mut self, _gl: &glow::Context) {}
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
