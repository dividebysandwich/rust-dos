//! The head and the puck as the VFX1 reports them: signed 16-bit angles,
//! 65536 to a turn, positive yaw to the left, pitch up and roll to the
//! right, made from the host's rotations (quaternions in OpenXR's axes: x
//! right, y up, -z ahead).

/// A rotation as a unit quaternion: w, x, y, z.
pub type Quat = [f64; 4];

pub const IDENTITY: Quat = [1.0, 0.0, 0.0, 0.0];

/// How far pitch and roll go when steering makes them larger, so that they
/// never jump to the other end: 170 degrees.
const STEER_LIMIT: f64 = 170.0 / 360.0 * 65536.0;

/// The frame interval steering makes the head's movement as large as.
const STEER_INTERVAL_MS: f64 = 1000.0 / 30.0;

pub fn multiply(a: Quat, b: Quat) -> Quat {
    [
        a[0] * b[0] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3],
        a[0] * b[1] + a[1] * b[0] + a[2] * b[3] - a[3] * b[2],
        a[0] * b[2] - a[1] * b[3] + a[2] * b[0] + a[3] * b[1],
        a[0] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[0],
    ]
}

pub fn conjugate(q: Quat) -> Quat {
    [q[0], -q[1], -q[2], -q[3]]
}

fn normalize(q: Quat) -> Quat {
    let n = q.iter().map(|c| c * c).sum::<f64>().sqrt();
    if n > 0.0 { q.map(|c| c / n) } else { IDENTITY }
}

/// An angle in radians as the VFX1's units.
pub fn units(radians: f64) -> i16 {
    let units = (radians / std::f64::consts::TAU * 65536.0).round() as i64;
    units.rem_euclid(65536) as u16 as i16
}

/// Yaw, pitch and roll in radians of a rotation, applied in that order:
/// q = Ry(yaw) Rx(pitch) Rz(-roll).
pub fn angles(q: Quat) -> [f64; 3] {
    let [w, x, y, z] = normalize(q);
    let m02 = 2.0 * (x * z + w * y);
    let m22 = 1.0 - 2.0 * (x * x + y * y);
    let m12 = 2.0 * (y * z - w * x);
    let m10 = 2.0 * (x * y + w * z);
    let m11 = 1.0 - 2.0 * (x * x + z * z);
    [
        m02.atan2(m22),
        (-m12).clamp(-1.0, 1.0).asin(),
        -m10.atan2(m11),
    ]
}

/// The head's angles in the VFX1's units, of `head` as turned from `zero`.
pub fn head(zero: Quat, head: Quat) -> [i16; 3] {
    angles(multiply(conjugate(zero), head)).map(units)
}

/// Pitch and roll in radians against gravity, whichever way the rotation
/// faces, as the CyberPuck's two inclinometers have them.
pub fn tilt(q: Quat) -> [f64; 2] {
    let [w, x, y, z] = normalize(q);
    // Up of the local -z (ahead) and +x (right) axes.
    let ahead = 2.0 * (w * x - y * z);
    let right = 2.0 * (x * y + w * z);
    [
        ahead.clamp(-1.0, 1.0).asin(),
        -right.clamp(-1.0, 1.0).asin(),
    ]
}

/// The puck's pitch and roll in the VFX1's units, from where it was held
/// at `zero`: the puck level however the controller was held.
pub fn puck(zero: [f64; 2], q: Quat) -> [i16; 2] {
    let [pitch, roll] = tilt(q);
    [units(pitch - zero[0]), units(roll - zero[1])]
}

/// The head's movement between polls made larger as the game polls more
/// often than 30 times a second: Descent takes the head's change from one
/// frame to the next as a rate to turn the ship at, so at 60 frames a
/// second, where each frame's change is half as large, the same turn of
/// the head would turn it half as fast.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Steer {
    primed: bool,
    out: [f64; 3],
    last: [i16; 3],
    last_ms: f64,
    interval_ms: f64,
}

impl Steer {
    pub fn update(&mut self, raw: [i16; 3], now_ms: f64) -> [i16; 3] {
        if !self.primed {
            *self = Self {
                primed: true,
                out: raw.map(f64::from),
                last: raw,
                last_ms: now_ms,
                interval_ms: STEER_INTERVAL_MS,
            };
            return raw;
        }
        let dt = now_ms - self.last_ms;
        if dt > 0.0 && dt < 500.0 {
            self.interval_ms += (dt - self.interval_ms) * 0.1;
        }
        self.last_ms = now_ms;
        let gain = (STEER_INTERVAL_MS / self.interval_ms.max(1.0)).clamp(1.0, 10.0);
        for (i, raw) in raw.into_iter().enumerate() {
            self.out[i] += f64::from(raw.wrapping_sub(self.last[i])) * gain;
            self.last[i] = raw;
        }
        self.out[0] = (self.out[0] + 32768.0).rem_euclid(65536.0) - 32768.0;
        for out in &mut self.out[1..] {
            *out = out.clamp(-STEER_LIMIT, STEER_LIMIT);
        }
        self.out.map(|v| v.round() as i32 as i16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_PI_4;

    /// A turn of `angle` about `axis`.
    fn about(axis: [f64; 3], angle: f64) -> Quat {
        let (s, c) = (angle / 2.0).sin_cos();
        [c, axis[0] * s, axis[1] * s, axis[2] * s]
    }

    #[test]
    fn head_signs() {
        let eighth = 8192;
        // Turned left (about +y), looking up (about +x), tilted right
        // (clockwise as seen from behind: about -z).
        assert_eq!(
            head(IDENTITY, about([0.0, 1.0, 0.0], FRAC_PI_4)),
            [eighth, 0, 0]
        );
        assert_eq!(
            head(IDENTITY, about([1.0, 0.0, 0.0], FRAC_PI_4)),
            [0, eighth, 0]
        );
        assert_eq!(
            head(IDENTITY, about([0.0, 0.0, -1.0], FRAC_PI_4)),
            [0, 0, eighth]
        );
        // Yaw first: turned left, then up, reads both.
        let q = multiply(
            about([0.0, 1.0, 0.0], FRAC_PI_4),
            about([1.0, 0.0, 0.0], 0.3),
        );
        assert_eq!(head(IDENTITY, q), [eighth, units(0.3), 0]);
        // From a zero turned right, facing ahead is turned left of it.
        assert_eq!(
            head(about([0.0, 1.0, 0.0], -FRAC_PI_4), IDENTITY),
            [eighth, 0, 0]
        );
        assert_eq!(units(std::f64::consts::PI), -32768);
    }

    #[test]
    fn puck_tilt_ignores_yaw() {
        let turned = about([0.0, 1.0, 0.0], 1.0);
        let up = multiply(turned, about([1.0, 0.0, 0.0], 0.5));
        assert_eq!(puck([0.0; 2], up), [units(0.5), 0]);
        let right = multiply(turned, about([0.0, 0.0, -1.0], 0.5));
        assert_eq!(puck([0.0; 2], right), [0, units(0.5)]);
    }

    #[test]
    fn steering_follows_the_frame_rate() {
        let mut steer = Steer::default();
        assert_eq!(steer.update([0; 3], 0.0), [0; 3]);
        // At 30 polls a second the head goes as it is.
        let mut t = 0.0;
        for i in 1..=10 {
            t += STEER_INTERVAL_MS;
            assert_eq!(steer.update([i * 10, 0, 0], t), [i * 10, 0, 0]);
        }
        // Polled at 60 a second for a while, a turn counts twice.
        for _ in 0..100 {
            t += STEER_INTERVAL_MS / 2.0;
            steer.update([100, 0, 0], t);
        }
        t += STEER_INTERVAL_MS / 2.0;
        let out = steer.update([200, 0, 0], t);
        assert!((295..=300).contains(&out[0]), "{:?}", out);
    }
}
