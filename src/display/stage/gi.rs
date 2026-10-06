//! Light bounced around the room: probes on a grid over the scene's focus
//! (`Scene::focus_bounds`), each of which looks around itself once, on a
//! cube of small pictures, when the scene is loaded. What a probe sees is
//! summed up for each of the six directions along the axes
//! (an "ambient cube"): the light coming from the lit and glowing surfaces
//! there, how much of the background (the ambient light's way in) shows,
//! and for each patch of the screen how much of its light the surfaces
//! there send on, for a white patch. The screen's colours change with the
//! picture; the rest doesn't, so each picture costs only a sum per probe
//! (`gi_update.frag`).
//!
//! The probes' sums are kept in the user's directory (`vr-cache`), by the
//! scene's contents, and read back the next time.

use glam::{Mat4, Vec3};
use std::path::PathBuf;

/// The probes are about this far apart, in metres; further, if there would
/// be more than `MAX_PROBES`.
const SPACING: f32 = 0.4;
pub const MAX_PROBES: usize = 2048;

/// A probe's cube's faces are this many texels across.
pub const FACE: usize = 16;

/// A probe sees more than this much of the backs of single-sided
/// surfaces: it is inside something, and its light is left out.
const INSIDE: f32 = 0.12;

/// Changed whenever what is baked or how changes, so that older caches
/// aren't read.
const VERSION: u32 = 2;

/// Where the probes are: `count` along each axis from `low`, `step` apart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub low: Vec3,
    pub step: Vec3,
    pub count: [usize; 3],
}

impl Layout {
    /// Probes over the box from `low` to `high`, its corners among them.
    pub fn over(low: Vec3, high: Vec3) -> Self {
        let extent = (high - low).max(Vec3::splat(1e-3));
        let mut spacing = SPACING;
        loop {
            let count = extent.to_array().map(|e| ((e / spacing).ceil() as usize + 1).max(2));
            if count.iter().product::<usize>() <= MAX_PROBES {
                let step = extent / Vec3::new(count[0] as f32 - 1.0, count[1] as f32 - 1.0, count[2] as f32 - 1.0);
                return Layout { low, step, count };
            }
            spacing *= 1.1;
        }
    }

    /// Probes a little inside the box: those on its faces would be in its
    /// walls, or outside them.
    pub fn inside(low: Vec3, high: Vec3) -> Self {
        let inset = ((high - low) * 0.1).min(Vec3::splat(0.2));
        Self::over(low + inset, high - inset)
    }

    pub fn len(&self) -> usize {
        self.count.iter().product()
    }

    /// Probe `index`, counted along x, then y, then z.
    pub fn position(&self, index: usize) -> Vec3 {
        let [nx, ny, _] = self.count;
        let (x, y, z) = (index % nx, index / nx % ny, index / (nx * ny));
        self.low + self.step * Vec3::new(x as f32, y as f32, z as f32)
    }
}

/// The six directions along the axes, in the order the sums are kept:
/// +x, -x, +y, -y, +z, -z.
pub const AXES: [Vec3; 6] = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z];

/// The views of a cube's faces from `at`, a quarter turn wide, along
/// `AXES`.
pub fn face_views(at: Vec3) -> [Mat4; 6] {
    let projection = Mat4::perspective_rh_gl(std::f32::consts::FRAC_PI_2, 1.0, 0.02, 60.0);
    AXES.map(|axis| {
        let up = if axis.y.abs() > 0.5 { Vec3::Z } else { Vec3::Y };
        projection * Mat4::look_to_rh(at, axis, up)
    })
}

/// The direction through each texel of a cube's faces (by face, then rows
/// from the bottom, then columns) and the solid angle it covers.
pub fn texel_directions() -> Vec<(Vec3, f32)> {
    let views = face_views(Vec3::ZERO);
    let mut out = Vec::with_capacity(6 * FACE * FACE);
    for view in views {
        let inverse = view.inverse();
        for j in 0..FACE {
            for i in 0..FACE {
                let x = (i as f32 + 0.5) / FACE as f32 * 2.0 - 1.0;
                let y = (j as f32 + 0.5) / FACE as f32 * 2.0 - 1.0;
                let d = inverse.project_point3(Vec3::new(x, y, 1.0)).normalize();
                let texel = 2.0 / FACE as f32;
                out.push((d, texel * texel / (x * x + y * y + 1.0).powf(1.5)));
            }
        }
    }
    out
}

/// What a probe sees, summed for each of `AXES` with the cosine to it,
/// over π: the light from the surfaces (linear RGB), how much of the
/// background shows, and for each patch of the screen the light the
/// surfaces send on from a white patch (by patch, then axis), with the
/// colour of the surfaces it falls on (`tint`).
#[derive(Clone, Debug, PartialEq)]
pub struct Probe {
    pub light: [[f32; 3]; 6],
    pub sky: [f32; 6],
    pub screen: Vec<[f32; 6]>,
    pub tint: [f32; 3],
    pub valid: bool,
}

/// One texel of what a probe's cube saw, from the five pictures the bake
/// draws: the light leaving the surface and whether it is the background
/// (1), a surface (0) or a single-sided surface's back (-1); its colour
/// and brightness times the screen's light falling on it; and each
/// patch's light falling on it, times its brightness.
#[derive(Clone, Copy, Debug, Default)]
pub struct Seen {
    pub light: [f32; 4],
    pub color: [f32; 4],
    pub patches: [f32; 12],
}

/// A probe from the texels its cube saw, in `texel_directions`' order.
pub fn integrate(directions: &[(Vec3, f32)], seen: impl Iterator<Item = Seen>, patches: usize) -> Probe {
    let mut probe =
        Probe { light: [[0.0; 3]; 6], sky: [0.0; 6], screen: vec![[0.0; 6]; patches], tint: [0.0; 3], valid: true };
    let (mut backs, mut tint, mut weight) = (0.0, Vec3::ZERO, 0.0);
    for (&(d, solid), s) in directions.iter().zip(seen) {
        if s.light[3] < -0.5 {
            backs += solid;
            continue;
        }
        let background = s.light[3] > 0.5;
        tint += Vec3::new(s.color[0], s.color[1], s.color[2]) * solid;
        weight += s.color[3] * solid;
        for (k, axis) in AXES.iter().enumerate() {
            let c = d.dot(*axis).max(0.0) * solid / std::f32::consts::PI;
            if c == 0.0 {
                continue;
            }
            if background {
                probe.sky[k] += c;
                continue;
            }
            for (sum, l) in probe.light[k].iter_mut().zip(s.light) {
                *sum += l * c;
            }
            for (p, screen) in probe.screen.iter_mut().enumerate() {
                screen[k] += s.patches[p] * c;
            }
        }
    }
    probe.valid = backs < INSIDE * 4.0 * std::f32::consts::PI;
    probe.tint = if weight > 1e-6 { (tint / weight).to_array() } else { [1.0; 3] };
    probe
}

/// All the probes of a scene.
#[derive(Clone, Debug, PartialEq)]
pub struct Probes {
    pub layout: Layout,
    pub patches: usize,
    pub probes: Vec<Probe>,
}

impl Probes {
    /// RGBA texels in each probe's row of the texture the update reads:
    /// the light and background for each axis; the patches' light, four to
    /// a texel, for each axis; the tint and whether it counts.
    pub fn row_texels(patches: usize) -> usize {
        6 + 6 * patches.div_ceil(4) + 1
    }

    /// The texture's texels, a row for each probe.
    pub fn texels(&self) -> Vec<f32> {
        let groups = self.patches.div_ceil(4);
        let mut out = Vec::with_capacity(self.probes.len() * Self::row_texels(self.patches) * 4);
        for probe in &self.probes {
            for k in 0..6 {
                out.extend(probe.light[k]);
                out.push(probe.sky[k]);
            }
            for k in 0..6 {
                for g in 0..groups {
                    out.extend((0..4).map(|c| probe.screen.get(g * 4 + c).map_or(0.0, |s| s[k])));
                }
            }
            out.extend(probe.tint);
            out.push(probe.valid as u32 as f32);
        }
        out
    }

    /// The cache file for a scene whose contents and settings hash to
    /// `key`.
    pub fn cache_path(key: u64) -> Option<PathBuf> {
        Some(rust_dos::config::user_dir()?.join("vr-cache").join(format!("{:016x}.bin", key)))
    }

    /// Delete all but the newest few caches in `dir`, so that those of
    /// scenes since changed or gone don't pile up.
    pub fn prune(dir: &std::path::Path) {
        const KEEP: usize = 16;
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut caches: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "bin"))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        caches.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, old) in caches.into_iter().skip(KEEP) {
            let _ = std::fs::remove_file(old);
        }
    }

    /// The key of the cache, from the scene's (`source`) and how the
    /// probes are laid out.
    pub fn key(source: u64, layout: &Layout, patches: usize) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (VERSION, source, patches, FACE, layout.count).hash(&mut h);
        for v in [layout.low, layout.step] {
            v.to_array().map(f32::to_bits).hash(&mut h);
        }
        h.finish()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(b"RDGI");
        out.extend(VERSION.to_le_bytes());
        out.extend((self.patches as u32).to_le_bytes());
        for c in self.layout.count {
            out.extend((c as u32).to_le_bytes());
        }
        for v in self.layout.low.to_array().into_iter().chain(self.layout.step.to_array()) {
            out.extend(v.to_le_bytes());
        }
        for v in self.texels() {
            out.extend(v.to_le_bytes());
        }
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let mut words = bytes.strip_prefix(b"RDGI")?.chunks_exact(4).map(|w| [w[0], w[1], w[2], w[3]]);
        let mut word = || words.next();
        if u32::from_le_bytes(word()?) != VERSION {
            return None;
        }
        let patches = u32::from_le_bytes(word()?) as usize;
        let count = [(); 3].map(|_| word().map(|w| u32::from_le_bytes(w) as usize));
        let count = [count[0]?, count[1]?, count[2]?];
        let mut float = || word().map(f32::from_le_bytes);
        let low = Vec3::new(float()?, float()?, float()?);
        let step = Vec3::new(float()?, float()?, float()?);
        let layout = Layout { low, step, count };
        if layout.len() > MAX_PROBES || patches > 12 {
            return None;
        }
        let groups = patches.div_ceil(4);
        let mut probes = Vec::with_capacity(layout.len());
        for _ in 0..layout.len() {
            let mut light = [[0.0; 3]; 6];
            let mut sky = [0.0; 6];
            for k in 0..6 {
                light[k] = [float()?, float()?, float()?];
                sky[k] = float()?;
            }
            let mut screen = vec![[0.0; 6]; patches];
            for k in 0..6 {
                for g in 0..groups {
                    for c in 0..4 {
                        let v = float()?;
                        if let Some(s) = screen.get_mut(g * 4 + c) {
                            s[k] = v;
                        }
                    }
                }
            }
            let tint = [float()?, float()?, float()?];
            let valid = float()? > 0.5;
            probes.push(Probe { light, sky, screen, tint, valid });
        }
        if float().is_some() {
            return None;
        }
        Some(Probes { layout, patches, probes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_covers_the_box_and_keeps_to_its_size() {
        let layout = Layout::over(Vec3::new(-2.5, 0.0, -2.2), Vec3::new(2.5, 2.6, 2.2));
        assert_eq!(layout.count, [14, 8, 12]);
        assert!(layout.position(0).distance(Vec3::new(-2.5, 0.0, -2.2)) < 1e-5);
        assert!(layout.position(layout.len() - 1).distance(Vec3::new(2.5, 2.6, 2.2)) < 1e-4);
        let p = layout.position(1 + 14 * (2 + 8 * 3));
        assert!(p.distance(layout.low + layout.step * Vec3::new(1.0, 2.0, 3.0)) < 1e-5);
        let big = Layout::over(Vec3::ZERO, Vec3::splat(24.0));
        assert!(big.len() <= MAX_PROBES && big.len() > MAX_PROBES / 2, "{:?}", big.count);
    }

    #[test]
    fn the_cube_covers_the_whole_sphere() {
        let directions = texel_directions();
        let total: f32 = directions.iter().map(|d| d.1).sum();
        assert!((total / (4.0 * std::f32::consts::PI) - 1.0).abs() < 0.002, "{}", total);
        // Face 0 looks along +x, row 0 is its bottom.
        let (d, _) = directions[0];
        assert!(d.x > 0.5 && d.y < 0.0, "{:?}", d);
        let (d, _) = directions[2 * FACE * FACE + FACE * FACE / 2 + FACE / 2];
        assert!(d.y > 0.99, "{:?}", d);
    }

    #[test]
    fn a_probe_under_open_sky_sees_it_all() {
        let directions = texel_directions();
        let seen = directions.iter().map(|_| Seen { light: [0.0, 0.0, 0.0, 1.0], ..Seen::default() });
        let probe = integrate(&directions, seen, 4);
        for k in 0..6 {
            assert!((probe.sky[k] - 1.0).abs() < 0.01, "{:?}", probe.sky);
        }
        assert!(probe.valid);
    }

    #[test]
    fn light_from_below_reaches_what_faces_down() {
        let directions = texel_directions();
        let seen = directions.iter().map(|(d, _)| {
            let below = d.y < 0.0;
            let mut s = Seen { light: [0.5, 0.25, 0.0, 0.0], color: [0.4, 0.2, 0.0, 0.3], ..Seen::default() };
            s.patches[1] = if below { 2.0 } else { 0.0 };
            s
        });
        let probe = integrate(&directions, seen, 2);
        // A uniform surround of 0.5 gives 0.5 in every direction.
        for k in 0..6 {
            assert!((probe.light[k][0] - 0.5).abs() < 0.01, "{:?}", probe.light);
        }
        assert!(probe.screen[1][3] > 1.9 && probe.screen[1][2] < 1e-6, "{:?}", probe.screen);
        assert!(probe.screen[0].iter().all(|&v| v == 0.0));
        assert!((probe.tint[0] - 0.4 / 0.3).abs() < 1e-3);
    }

    #[test]
    fn a_probe_inside_something_doesnt_count() {
        let directions = texel_directions();
        let seen = directions.iter().map(|_| Seen { light: [0.0, 0.0, 0.0, -1.0], ..Seen::default() });
        assert!(!integrate(&directions, seen, 1).valid);
    }

    #[test]
    fn probes_come_back_from_the_cache_as_they_went() {
        let layout = Layout::over(Vec3::ZERO, Vec3::new(0.8, 0.4, 0.4));
        let probes = (0..layout.len())
            .map(|i| Probe {
                light: [[i as f32, 1.0, 2.0]; 6],
                sky: [0.5; 6],
                screen: (0..5).map(|p| [p as f32 * 0.1; 6]).collect(),
                tint: [0.9, 0.8, 0.7],
                valid: i % 2 == 0,
            })
            .collect();
        let probes = Probes { layout, patches: 5, probes };
        assert_eq!(Probes::from_bytes(&probes.to_bytes()), Some(probes.clone()));
        assert_eq!(probes.texels().len(), probes.probes.len() * Probes::row_texels(5) * 4);
        assert!(Probes::from_bytes(&probes.to_bytes()[..100]).is_none());
    }
}
