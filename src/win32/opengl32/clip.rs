//! The user's clip planes: glClipPlane and glGetClipPlane (OpenGL 1.1,
//! section 2.11).
//!
//! A plane is kept in eye coordinates, as glClipPlane gives it through
//! the inverse of the modelview matrix of the time
//! (`matrix::eye_plane`), and a point is inside it where the plane's
//! dot product with the point's eye coordinates is 0 or more. The
//! planes that are on go to the Direct3D device as its user clip
//! planes (`draw::put`), which are in world space: the modelview matrix
//! is the device's world matrix, so its world space is OpenGL's eye
//! space, on the card and in Direct3D's pipeline on the processor
//! alike. Both cut primitives at them as they cut at the view volume.

use super::{GL_INVALID_ENUM, ctx, error, matrix, read_doubles, write_doubles};
use crate::cpu::Cpu;

/// GL_CLIP_PLANE0, the first of the six planes and their enables.
pub const GL_CLIP_PLANE0: u32 = 0x3000;
/// How many there are (GL_MAX_CLIP_PLANES).
pub const PLANES: usize = 6;

/// The clip planes and which of them are on.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Clip {
    /// GL_CLIP_PLANE0 to GL_CLIP_PLANE5, a bit each.
    pub enabled: u8,
    /// The planes in eye coordinates, (0, 0, 0, 0) at first.
    pub planes: [[f32; 4]; PLANES],
}

crate::state_fields!(Clip { enabled, planes });

impl Clip {
    /// The planes that are on, in their order.
    pub fn on(&self) -> Vec<[f32; 4]> {
        (0..PLANES)
            .filter(|i| self.enabled >> i & 1 != 0)
            .map(|i| self.planes[i])
            .collect()
    }
}

/// The plane `plane` names, 0 to 5.
pub fn index(plane: u32) -> Option<usize> {
    let i = plane.checked_sub(GL_CLIP_PLANE0)? as usize;
    (i < PLANES).then_some(i)
}

/// glClipPlane(plane, equation): four doubles.
pub fn clip_plane(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = index(args[0]) else {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    };
    let Some(p) = read_doubles(cpu, args[1], 4) else {
        return Some(0);
    };
    let Some(c) = ctx(cpu) else {
        return Some(0);
    };
    let modelview = *c.modelview.last().expect("a matrix stack is never empty");
    let p = [p[0], p[1], p[2], p[3]].map(|x| x as f32);
    c.clip.planes[i] = matrix::eye_plane(&modelview, p);
    Some(0)
}

/// glGetClipPlane(plane, equation): the plane in eye coordinates, as
/// four doubles.
pub fn get_clip_plane(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = index(args[0]) else {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    };
    let Some(c) = ctx(cpu) else {
        return Some(0);
    };
    let p = c.clip.planes[i].map(f64::from);
    write_doubles(cpu, args[1], &p);
    Some(0)
}
