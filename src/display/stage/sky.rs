//! The sky at a time of day: where the sun and the moon are, the colours
//! of the sky and its light, and how much of each of the sun's baked
//! directions (`SUN_KEYS`) the light bounced around the room is made of.
//!
//! The sun goes its way of an equinox (it rises at six in the east and
//! sets at six in the west), as high as the latitude lets it. The scene
//! says which way its compass points (`Scene::heading`). The moon stands
//! opposite the sun.

use glam::Vec3;

/// The sky's colours and lights at a moment, linear RGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyLook {
    /// Towards the sun and the moon.
    pub sun: Vec3,
    pub moon: Vec3,
    /// High up and along the horizon.
    pub zenith: Vec3,
    pub horizon: Vec3,
    /// The glow along the horizon under the sun (sunrise, sunset).
    pub band: Vec3,
    /// The sun's disc, the moon's, and how many stars show (0 to 1).
    pub sun_disc: Vec3,
    pub moon_disc: Vec3,
    pub stars: f32,
    /// How overcast it is (0 to 1), and a lightning flash's brightness.
    pub clouds: f32,
    pub flash: f32,
    /// The light from above that isn't the sun's, and the sun's (or at
    /// night the moon's): from `light_dir`, towards it.
    pub ambient: Vec3,
    pub light: Vec3,
    pub light_dir: Vec3,
}

/// A time of day's sky, `hours` from midnight, at `latitude` degrees
/// north, the scene's -Z pointing `heading` degrees from north (90 east).
pub fn look(hours: f32, latitude: f32, heading: f32) -> SkyLook {
    let sun = sun_direction(hours, latitude, heading);
    let moon = -sun;
    let e = sun.y.clamp(-1.0, 1.0).asin().to_degrees();
    let at = |keys: &[(f32, [f32; 3])]| Vec3::from(lerp_keys(keys, e));
    let zenith = at(&ZENITH);
    let horizon = at(&HORIZON);
    let band = at(&BAND);
    let ambient_sun = at(&AMBIENT);
    let sun_light = at(&SUN_LIGHT);
    // The moon's light, while it is up, and some sky glow under it.
    let moon_up = smoothstep(-1.0, 6.0, moon.y.asin().to_degrees());
    let moon_light = Vec3::new(0.55, 0.66, 1.0) * 0.07 * moon_up;
    let ambient = ambient_sun + Vec3::new(0.004, 0.006, 0.012) * moon_up;
    // The sun lights until it is under; then the moon.
    let (light, light_dir) = if sun.y > 0.0 { (sun_light, sun) } else { (moon_light, moon) };
    SkyLook {
        sun,
        moon,
        zenith,
        horizon,
        band,
        sun_disc: Vec3::new(1.0, 0.55, 0.2).lerp(Vec3::new(1.0, 0.95, 0.85), smoothstep(2.0, 30.0, e)) * smoothstep(-2.0, 0.5, e),
        moon_disc: Vec3::new(0.8, 0.85, 0.95) * smoothstep(-2.0, 1.0, moon.y.asin().to_degrees()),
        stars: smoothstep(-4.0, -12.0, e),
        clouds: 0.0,
        flash: 0.0,
        ambient,
        light,
        light_dir,
    }
}

impl SkyLook {
    /// Under clouds `amount` thick (0 to 1): the sun and the moon dimmed,
    /// the sky grey, no stars.
    pub fn overcast(mut self, amount: f32) -> Self {
        let amount = amount.clamp(0.0, 1.0);
        let grey = |c: Vec3, bright: f32| c.lerp(Vec3::splat(luma(c) * bright), amount);
        self.zenith = grey(self.zenith, 0.9).lerp(grey(self.horizon, 0.8), amount * 0.6);
        self.horizon = grey(self.horizon, 0.8);
        self.band *= 1.0 - amount * 0.85;
        self.sun_disc *= 1.0 - amount;
        self.moon_disc *= 1.0 - amount;
        self.stars *= 1.0 - amount;
        self.light *= 1.0 - amount * 0.85;
        self.ambient = grey(self.ambient, 0.75);
        self.clouds = amount;
        self
    }

    /// A lightning flash `amount` bright (0 to 1) lighting the sky and
    /// what it reaches.
    pub fn with_flash(mut self, amount: f32) -> Self {
        self.flash = amount;
        self.ambient += Vec3::new(0.7, 0.75, 1.0) * amount * 1.5;
        self
    }
}

/// The scene's sunset of before: the sky the test room had, and still has
/// at its time.
pub const SUNSET_HOURS: f32 = 17.43;

/// Towards the sun at `hours`: the equinox's way across the sky.
pub fn sun_direction(hours: f32, latitude: f32, heading: f32) -> Vec3 {
    let h = ((hours - 12.0) * 15.0).to_radians();
    let phi = latitude.to_radians();
    // East, north and up.
    let (east, north, up) = (-h.sin(), -h.cos() * phi.sin(), h.cos() * phi.cos());
    compass(90.0, heading) * east + compass(0.0, heading) * north + Vec3::Y * up
}

/// The direction in the scene of compass bearing `bearing` (degrees from
/// north, 90 east), its -Z pointing `heading`.
fn compass(bearing: f32, heading: f32) -> Vec3 {
    let a = (bearing - heading).to_radians();
    Vec3::new(a.sin(), 0.0, -a.cos())
}

/// The sun's directions the light bounced around the room is worked out
/// for: low all around, and high above.
pub fn sun_keys() -> Vec<Vec3> {
    let mut keys: Vec<Vec3> = (0..6)
        .map(|i| {
            let a = (i as f32 * 60.0).to_radians();
            let e = 15f32.to_radians();
            Vec3::new(a.sin() * e.cos(), e.sin(), -a.cos() * e.cos())
        })
        .collect();
    keys.push(Vec3::new(0.3, 0.95, 0.0).normalize());
    keys
}

/// How much of each of `keys` the light from `towards` is: the nearest
/// ones, more the nearer, adding up to 1.
pub fn key_weights(towards: Vec3, keys: &[Vec3]) -> Vec<f32> {
    let mut d = towards.normalize_or(Vec3::Y);
    // (Under the horizon, as just over it.)
    if d.y < 0.05 {
        d = Vec3::new(d.x, 0.05, d.z).normalize_or(Vec3::Y);
    }
    let mut weights: Vec<f32> = keys.iter().map(|k| (d.dot(*k) - 0.4).max(0.0).powi(3)).collect();
    let sum: f32 = weights.iter().sum();
    if sum <= 0.0 {
        // Far from all: the nearest.
        let nearest = keys.iter().enumerate().max_by(|a, b| d.dot(*a.1).total_cmp(&d.dot(*b.1))).map_or(0, |(i, _)| i);
        weights.iter_mut().enumerate().for_each(|(i, w)| *w = (i == nearest) as u32 as f32);
        return weights;
    }
    weights.iter_mut().for_each(|w| *w /= sum);
    weights
}

/// The colours by the sun's height in degrees: night, dusk, sunset (the
/// test room's sky of before at its height), golden hour, day.
const ZENITH: [(f32, [f32; 3]); 6] = [
    (-14.0, [0.0006, 0.0009, 0.0028]),
    (-6.0, [0.0012, 0.0018, 0.008]),
    (5.7, [0.002, 0.005, 0.03]),
    (12.0, [0.012, 0.035, 0.13]),
    (25.0, [0.04, 0.1, 0.32]),
    (50.0, [0.05, 0.13, 0.4]),
];
const HORIZON: [(f32, [f32; 3]); 6] = [
    (-14.0, [0.0012, 0.0016, 0.0035]),
    (-6.0, [0.004, 0.005, 0.016]),
    (5.7, [0.012, 0.028, 0.1]),
    (12.0, [0.08, 0.12, 0.24]),
    (25.0, [0.22, 0.32, 0.5]),
    (50.0, [0.3, 0.42, 0.6]),
];
const BAND: [(f32, [f32; 3]); 6] = [
    (-14.0, [0.0, 0.0, 0.0]),
    (-6.0, [0.12, 0.03, 0.03]),
    (5.7, [1.0, 0.3, 0.055]),
    (12.0, [0.5, 0.25, 0.08]),
    (25.0, [0.05, 0.04, 0.03]),
    (50.0, [0.0, 0.0, 0.0]),
];
const AMBIENT: [(f32, [f32; 3]); 6] = [
    (-14.0, [0.002, 0.0025, 0.005]),
    (-6.0, [0.006, 0.008, 0.016]),
    (5.7, [0.035, 0.045, 0.09]),
    (12.0, [0.08, 0.1, 0.16]),
    (25.0, [0.2, 0.25, 0.34]),
    (50.0, [0.28, 0.34, 0.44]),
];
const SUN_LIGHT: [(f32, [f32; 3]); 6] = [
    (-14.0, [0.0, 0.0, 0.0]),
    (0.0, [0.0, 0.0, 0.0]),
    (5.7, [1.6, 0.8, 0.352]),
    (12.0, [2.2, 1.5, 0.9]),
    (25.0, [2.8, 2.5, 2.1]),
    (50.0, [3.2, 3.05, 2.85]),
];

/// The colour at `x` between the keys, held past the ends.
fn lerp_keys(keys: &[(f32, [f32; 3])], x: f32) -> [f32; 3] {
    let last = keys.len() - 1;
    if x <= keys[0].0 {
        return keys[0].1;
    }
    if x >= keys[last].0 {
        return keys[last].1;
    }
    let i = keys.windows(2).position(|w| x < w[1].0).unwrap_or(last - 1);
    let (a, b) = (keys[i], keys[i + 1]);
    let t = (x - a.0) / (b.0 - a.0);
    std::array::from_fn(|c| a.1[c] + (b.1[c] - a.1[c]) * t)
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn luma(c: Vec3) -> f32 {
    c.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sun_rises_in_the_east_and_sets_in_the_west() {
        // -Z north: east is +X.
        let rise = sun_direction(6.0, 48.0, 0.0);
        assert!(rise.y.abs() < 1e-4 && rise.x > 0.99, "{:?}", rise);
        let set = sun_direction(18.0, 48.0, 0.0);
        assert!(set.y.abs() < 1e-4 && set.x < -0.99, "{:?}", set);
        // At noon in the south, as high as the latitude lets it.
        let noon = sun_direction(12.0, 48.0, 0.0);
        assert!(noon.z > 0.0 && (noon.y.asin().to_degrees() - 42.0).abs() < 1e-3, "{:?}", noon);
        // Under the horizon at midnight; the moon is up.
        assert!(sun_direction(0.0, 48.0, 0.0).y < -0.5);
        // -Z west: the sunset is straight ahead.
        let ahead = sun_direction(17.9, 48.0, 270.0);
        assert!(ahead.z < -0.99, "{:?}", ahead);
    }

    #[test]
    fn the_test_rooms_sunset_is_kept() {
        let sky = look(SUNSET_HOURS, 48.0, 270.0);
        let e = sky.sun.y.asin().to_degrees();
        assert!((e - 5.7).abs() < 1.5, "{}", e);
        assert!(sky.zenith.abs_diff_eq(Vec3::new(0.002, 0.005, 0.03), 0.01));
    }

    #[test]
    fn the_day_is_bright_and_the_night_dark_with_stars() {
        let day = look(12.0, 48.0, 0.0);
        let night = look(0.0, 48.0, 0.0);
        assert!(day.ambient.y > 20.0 * night.ambient.y);
        assert!(day.light.length() > 3.0 && day.light_dir == day.sun);
        assert_eq!(day.stars, 0.0);
        assert_eq!(night.stars, 1.0);
        // At night, the moon lights a little from where it is.
        assert_eq!(night.light_dir, night.moon);
        assert!(night.light.length() > 0.0 && night.light.length() < 0.2);
        // Clouds take most of the sun.
        let grey = day.overcast(1.0);
        assert!(grey.light.length() < day.light.length() * 0.2 && grey.sun_disc == Vec3::ZERO);
    }

    #[test]
    fn the_keys_weigh_the_nearest_most() {
        let keys = sun_keys();
        for (i, k) in keys.iter().enumerate() {
            let w = key_weights(*k, &keys);
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
            assert!(w[i] > 0.6, "{} {:?}", i, w);
        }
        // Between two low ones, half each.
        let between = (keys[0] + keys[1]).normalize();
        let w = key_weights(between, &keys);
        assert!((w[0] - w[1]).abs() < 1e-4 && w[0] > 0.3, "{:?}", w);
        // Under the horizon, as just over it.
        let w = key_weights(Vec3::new(0.0, -0.5, -1.0), &keys);
        assert!(w[0] > 0.6, "{:?}", w);
    }
}
