//! The headset's cameras seen through the scene where its pictures are
//! transparent: the runtime's ALPHA_BLEND environment (the Steam Frame,
//! Varjo's XR headsets), or XR_FB_passthrough's layer under the scene's
//! (a Quest through Link). The eyes' images are premultiplied: where their
//! alpha is 0, only the cameras are seen.

use super::log;
use openxr as xr;
use xr::sys::Handle as _;

pub enum Passthrough {
    /// The environment blend mode shows the cameras.
    Blend,
    /// A layer of the cameras' pictures under the eyes'.
    Fb(FbCameras),
}

/// XR_FB_passthrough's feature and layer, running from when they are made
/// until they are dropped (before the session). Made through the raw calls:
/// the crate's need the session's graphics type, which is erased here.
pub struct FbCameras {
    fp: xr::raw::PassthroughFB,
    feature: xr::sys::PassthroughFB,
    layer: xr::sys::PassthroughLayerFB,
}

impl FbCameras {
    fn new(session: &xr::Session<xr::AnyGraphics>) -> Result<FbCameras, xr::sys::Result> {
        let fp = *session.instance().exts().fb_passthrough.as_ref().ok_or(xr::sys::Result::ERROR_EXTENSION_NOT_PRESENT)?;
        let running = xr::PassthroughFlagsFB::IS_RUNNING_AT_CREATION;
        let info = xr::sys::PassthroughCreateInfoFB {
            ty: xr::sys::PassthroughCreateInfoFB::TYPE,
            next: std::ptr::null(),
            flags: running,
        };
        let mut feature = xr::sys::PassthroughFB::NULL;
        // SAFETY: the session is alive and has the extension enabled (its
        // functions were loaded); the info is complete.
        let made = unsafe { (fp.create_passthrough)(session.as_raw(), &info, &mut feature) };
        if made.into_raw() < 0 {
            return Err(made);
        }
        let info = xr::sys::PassthroughLayerCreateInfoFB {
            ty: xr::sys::PassthroughLayerCreateInfoFB::TYPE,
            next: std::ptr::null(),
            passthrough: feature,
            flags: running,
            purpose: xr::PassthroughLayerPurposeFB::RECONSTRUCTION,
        };
        let mut layer = xr::sys::PassthroughLayerFB::NULL;
        // SAFETY: as above, with the feature just made.
        let made = unsafe { (fp.create_passthrough_layer)(session.as_raw(), &info, &mut layer) };
        if made.into_raw() < 0 {
            // SAFETY: made above, used nowhere else.
            unsafe { (fp.destroy_passthrough)(feature) };
            return Err(made);
        }
        Ok(FbCameras { fp, feature, layer })
    }
}

impl Drop for FbCameras {
    fn drop(&mut self) {
        // SAFETY: made by `new`, the session still alive (`Xr` drops this
        // first), and not in a frame being composed any more.
        unsafe {
            (self.fp.destroy_passthrough_layer)(self.layer);
            (self.fp.destroy_passthrough)(self.feature);
        }
    }
}

impl Passthrough {
    /// How the session shows the cameras out of what it offers: the
    /// `modes` of blending with the world, and `fb` (XR_FB_passthrough,
    /// enabled); with the blend mode to end frames with. None where it
    /// can't.
    pub fn open(
        session: &xr::Session<xr::AnyGraphics>,
        modes: &[xr::EnvironmentBlendMode],
        fb: bool,
    ) -> Option<(Passthrough, xr::EnvironmentBlendMode)> {
        if modes.contains(&xr::EnvironmentBlendMode::ALPHA_BLEND) {
            log::line("Passthrough: the runtime blends the eyes' alpha with the cameras");
            return Some((Passthrough::Blend, xr::EnvironmentBlendMode::ALPHA_BLEND));
        }
        if fb {
            match FbCameras::new(session) {
                Ok(cameras) => {
                    log::line("Passthrough: XR_FB_passthrough's layer under the eyes'");
                    return Some((Passthrough::Fb(cameras), xr::EnvironmentBlendMode::OPAQUE));
                }
                Err(e) => log::line(format!("Passthrough: XR_FB_passthrough can't be used: {}", e)),
            }
        }
        log::line(format!("No passthrough: the runtime blends with {:?}, and has no XR_FB_passthrough", modes));
        None
    }
}

/// End a frame with the eyes' `projection` layer: over the cameras with
/// `passthrough`, its alpha counting.
pub fn end<G: xr::Graphics>(
    stream: &mut xr::FrameStream<G>,
    time: xr::Time,
    blend: xr::EnvironmentBlendMode,
    projection: xr::CompositionLayerProjection<'_, G>,
    passthrough: Option<&Passthrough>,
) -> xr::Result<()> {
    match passthrough {
        None => stream.end(time, blend, &[&projection]),
        Some(Passthrough::Blend) => {
            let projection = projection.layer_flags(xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA);
            stream.end(time, blend, &[&projection])
        }
        Some(Passthrough::Fb(fb)) => {
            let cameras = xr::sys::CompositionLayerPassthroughFB {
                ty: xr::sys::CompositionLayerPassthroughFB::TYPE,
                next: std::ptr::null(),
                flags: xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA,
                space: xr::sys::Space::NULL,
                layer_handle: fb.layer,
            };
            // SAFETY: a layer is read through its header, which the raw
            // struct starts with, as the crate's own layers are (the base
            // is a transparent wrapper of the header).
            let cameras: &xr::CompositionLayerBase<'_, G> = unsafe { &*(&cameras as *const _ as *const _) };
            let projection = projection.layer_flags(xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA);
            stream.end(time, blend, &[cameras, &projection])
        }
    }
}
