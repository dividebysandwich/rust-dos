//! The sound coming from where the scene's speakers are: the left and
//! right channels from points in the scene (the screen's sides, or the
//! scene's `speaker_left` and `speaker_right`), heard by a listener that
//! turns and moves. From where the scene starts it sounds as without the
//! scene; a gain matrix for each block of samples adds no delay.

use super::super::audio_mix::Mix;
use glam::{Mat4, Vec3};

/// How far a speaker is to the side of `listener` (its -Z ahead, +X
/// right): the sine of its angle, -1 left to 1 right; and how far it is.
fn place(listener: Mat4, speaker: Vec3) -> (f32, f32) {
    let local = listener.inverse().transform_point3(speaker);
    let distance = local.length().max(0.05);
    ((local.x / distance).clamp(-1.0, 1.0), distance)
}

/// The mix for a listener at `listener` of `speakers`, where `start` (the
/// scene's start) hears them as plain stereo.
pub fn mix(listener: Mat4, start: Mat4, speakers: [Vec3; 2]) -> Mix {
    let mut mix = [0.0; 4];
    for (i, &speaker) in speakers.iter().enumerate() {
        let (side, distance) = place(listener, speaker);
        let (start_side, start_distance) = place(start, speaker);
        // Panned so that the start's angle is all the way to its side.
        let width = start_side.abs().max(0.05);
        let pan = (side / width).clamp(-1.0, 1.0);
        let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
        let gain = (start_distance / distance).clamp(0.3, 2.0);
        mix[i] = angle.cos() * gain;
        mix[2 + i] = angle.sin() * gain;
    }
    mix
}

#[cfg(test)]
mod tests {
    use super::super::super::audio_mix::IDENTITY;
    use super::*;

    fn close(a: Mix, b: Mix) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
    }

    #[test]
    fn the_start_hears_plain_stereo_and_turning_moves_it() {
        let start = Mat4::from_translation(Vec3::new(0.0, 1.2, 0.0));
        let speakers = [Vec3::new(-0.8, 1.4, -2.5), Vec3::new(0.8, 1.4, -2.5)];
        assert!(close(mix(start, start, speakers), IDENTITY), "{:?}", mix(start, start, speakers));
        // Turned left a quarter: both are to the right.
        let turned = start * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let m = mix(turned, start, speakers);
        assert!(m[0] < 0.1 && m[1] < 0.1 && m[2] > 0.9 && m[3] > 0.9, "{:?}", m);
        // Turned around: left and right swap.
        let back = start * Mat4::from_rotation_y(std::f32::consts::PI);
        assert!(close(mix(back, start, speakers), [0.0, 1.0, 1.0, 0.0]), "{:?}", mix(back, start, speakers));
        // Closer is louder.
        let near = Mat4::from_translation(Vec3::new(0.0, 1.2, -1.5));
        assert!(mix(near, start, speakers)[0] > 1.5);
    }
}
