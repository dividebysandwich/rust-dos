//! A polaroid camera's picture: the room seen through its lens, with its
//! flash, drawn on the thread that draws the room (the window's or the
//! headset's), which then shows it on a print coming out of the camera; and
//! saved as a PNG file in the capture folder, on a thread of its own.

use super::camera::NEAR;
use super::props::Props;
use super::render::{Gpu, View};
use super::scene::{Leds, Scene};
use super::world::World;
use glam::{Mat4, Vec3};
use std::path::PathBuf;
use std::time::Instant;

/// The picture's size, square, in pixels, and how wide it sees.
const SIZE: u32 = 1024;
const FOV: f32 = 55.0;

/// Take camera `camera`'s picture of `scene` at `now`: flash, draw it, put
/// it on a print coming out of the camera, and save it in `folder` (if
/// there is one), saying where with `said` (on the saving thread).
#[allow(clippy::too_many_arguments)]
pub fn take(
    gl: &glow::Context,
    gpu: &mut Gpu,
    scene: &Scene,
    props: &mut Props,
    world: &World,
    camera: usize,
    screen: Option<glow::Texture>,
    leds: Leds,
    folder: Option<PathBuf>,
    said: impl FnOnce(String) + Send + 'static,
    now: Instant,
) {
    let lens = props.pose(camera) * scene.props[camera].lens.unwrap_or(Mat4::IDENTITY);
    let eye = lens.transform_point3(Vec3::ZERO);
    let ahead = lens.transform_vector3(Vec3::NEG_Z).normalize_or(Vec3::NEG_Z);
    // The flash, a little in front of the lens, lights the room for this
    // picture and for a moment in the views.
    let up = lens.transform_vector3(Vec3::Y).normalize_or(Vec3::Y);
    world.flash(eye + ahead * 0.01 + up * 0.03, ahead);
    gpu.set_ambience(gl, scene, world.ambience(scene, now));
    let up = if ahead.y.abs() > 0.98 { up } else { Vec3::Y };
    let view = View {
        view: Mat4::look_to_rh(eye + ahead * 0.005, ahead, up),
        projection: Mat4::perspective_rh_gl(FOV.to_radians(), 1.0, NEAR, super::camera::FAR),
    };
    // Everything but the camera itself.
    let draws: Vec<_> = props.draws(gl, gpu, scene, now).into_iter().filter(|d| d.prop() != Some(camera)).collect();
    let rgba = match gpu.snapshot(gl, scene, view, SIZE, screen, leds, &draws) {
        Ok(rgba) => rgba,
        Err(e) => {
            said(format!("The camera can't take a picture: {}", e));
            return;
        }
    };
    props.add_print(scene, camera, gpu.page_texture(gl, &rgba, (SIZE, SIZE)), now);
    if let Some(folder) = folder {
        save(rgba, folder, said);
    }
}

/// Save the picture (RGBA, `SIZE` square, rows from the top) as a PNG file
/// in `folder`, on a thread of its own.
fn save(rgba: Vec<u8>, folder: PathBuf, said: impl FnOnce(String) + Send + 'static) {
    let _ = std::thread::Builder::new().name("vr-photo".into()).spawn(move || {
        let rgb: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        let frame = rust_dos::video::Frame::from_rgb(SIZE, SIZE, rgb);
        let saved = rust_dos::capture::capture_path(&folder, "photo", "png").and_then(|path| {
            rust_dos::capture::png::save(&frame, &path)?;
            Ok(path)
        });
        said(match saved {
            Ok(path) => format!("Photo saved to {}", path.display()),
            Err(e) => format!("The photo can't be saved: {}", e),
        });
    });
}
