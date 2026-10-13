//! Where the real keyboard is, for the headset's cameras to show it
//! through the scene (`[vr] keyboard`): on the desk below the screen at
//! first, then where the fingertips are as keys are pressed (with the
//! hands tracked), or where it was marked with a controller: the front edge
//! drawn with the trigger held, then the back edge clicked.
//!
//! What is found and marked is kept where it is in the room (`anchor`, the
//! runtime's stage or its own seated space, recentring moves neither), in
//! real metres, and saved for the next time.

use super::controls::{Hand, Tracking};
use super::render::{Extra, Punch, Window};
use super::scene::Scene;
use glam::{Mat4, Vec2, Vec3};
use rust_dos::vr::VrKeyboard;
use std::path::PathBuf;

/// A keyboard's size where nothing else is known, across and deep, in
/// metres: a full-size one with its keys' margins.
const TYPICAL: Vec2 = Vec2::new(0.46, 0.16);
/// The least a keyboard found or marked is taken to be.
const LEAST: Vec2 = Vec2::new(0.30, 0.10);
/// How far before the seat it is at first, in real metres, toward the
/// screen; how far below the head where there is no desk; and how far over
/// the desk its keys are.
const AHEAD: f32 = 0.40;
const BELOW_HEAD: f32 = 0.45;
const KEYS_HIGH: f32 = 0.03;
/// How far the window fades out around the keyboard.
const FEATHER: f32 = 0.03;
/// What is found from typing is grown by this much to each side, and
/// before and behind (the fingertips are on the keys' middles).
const MARGIN: Vec2 = Vec2::new(0.02, 0.03);
/// The fingertips at so many presses are kept, and so many over so wide
/// a spread are needed before the keyboard is taken to be where they are.
const SAMPLES: usize = 256;
const SAMPLES_NEEDED: usize = 30;
const SPREAD_NEEDED: f32 = 0.15;
/// A fingertip this near its hand's controller is the controller's hand
/// made up by the runtime, not a finger typing.
const NEAR_CONTROLLER: f32 = 0.15;
/// How much of the way to where it was found the window goes each second.
const EASE: f32 = 3.0;
/// What is found is saved again after so many new presses.
const SAVE_EVERY: usize = 32;
/// While marking, the real world shows this much over everything.
const MARKING_SEEN: f32 = 0.85;
/// The front edge is at least this long to count.
const LEAST_EDGE: f32 = 0.15;

/// A keyboard in the room: its middle, which way its keys' rows go (level),
/// and its size across and deep, in real metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub center: Vec3,
    pub across: Vec3,
    pub size: Vec2,
}

impl Area {
    /// Its frame from its own metres (x across, y up, z deep) into those of
    /// `to`, which takes real metres where they go.
    fn frame(&self, to: Mat4) -> Mat4 {
        let across = Vec3::new(self.across.x, 0.0, self.across.z).normalize_or(Vec3::X);
        let deep = across.cross(Vec3::Y);
        to * Mat4::from_cols(across.extend(0.0), Vec3::Y.extend(0.0), deep.extend(0.0), self.center.extend(1.0))
    }

    fn window(&self, to: Mat4) -> Window {
        Window { frame: self.frame(to), size: self.size, feather: FEATHER }
    }

    fn lerp(&self, to: &Area, t: f32) -> Area {
        let across = self.across.lerp(to.across, t).normalize_or(to.across);
        Area { center: self.center.lerp(to.center, t), across, size: self.size.lerp(to.size, t) }
    }

    /// As a line of the file: x y z, the way across (x z), width and depth.
    fn line(&self) -> String {
        let (c, a, s) = (self.center, self.across, self.size);
        format!("{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}", c.x, c.y, c.z, a.x, a.z, s.x, s.y)
    }

    fn parse(text: &str) -> Option<Area> {
        let n: Vec<f32> = text.split_whitespace().map(str::parse).collect::<Result<_, _>>().ok()?;
        let [x, y, z, ax, az, w, d] = n[..] else { return None };
        let across = Vec3::new(ax, 0.0, az).try_normalize()?;
        let size = Vec2::new(w, d);
        (size.min_element() > 0.0 && size.max_element() < 2.0).then_some(Area { center: Vec3::new(x, y, z), across, size })
    }
}

/// Where the keyboard is at first: on the desk under the screen, `AHEAD`
/// of the seat toward the screen, in the scene (`world` takes the headset's
/// real metres there, its origin the seat). In real metres from the seat
/// along the scene: its frame is in the scene, scaled as `world` is.
pub fn default_area(scene: &Scene, world: Mat4) -> (Mat4, Vec2) {
    let seat = world.transform_point3(Vec3::ZERO);
    let scale = world.x_axis.truncate().length().max(1e-3);
    let toward = Vec3::new(scene.screen.center.x - seat.x, 0.0, scene.screen.center.z - seat.z).normalize_or(Vec3::NEG_Z);
    let over = seat + toward * AHEAD * scale;
    let top = scene
        .surface_below(over, 1.2 * scale)
        .map_or(over.y - BELOW_HEAD * scale, |desk| desk.y + KEYS_HIGH * scale);
    // Its rows level with the screen's, or square to the way to it.
    let right = Vec3::new(scene.screen.right.x, 0.0, scene.screen.right.z).try_normalize().unwrap_or(toward.cross(Vec3::Y));
    let deep = right.cross(Vec3::Y);
    let frame = Mat4::from_cols(
        (right * scale).extend(0.0),
        (Vec3::Y * scale).extend(0.0),
        (deep * scale).extend(0.0),
        Vec3::new(over.x, top, over.z).extend(1.0),
    );
    (frame, TYPICAL)
}

/// The keyboard where the fingertips pressed keys (`tips`, in real metres
/// of the anchor's space), its rows along `across`: None until there are
/// enough, spread wide enough.
pub fn fit(tips: &[Vec3], across: Vec3) -> Option<Area> {
    if tips.len() < SAMPLES_NEEDED {
        return None;
    }
    let across = Vec3::new(across.x, 0.0, across.z).try_normalize()?;
    let deep = across.cross(Vec3::Y);
    let mut us: Vec<f32> = tips.iter().map(|p| p.dot(across)).collect();
    let mut vs: Vec<f32> = tips.iter().map(|p| p.dot(deep)).collect();
    let mut ys: Vec<f32> = tips.iter().map(|p| p.y).collect();
    for list in [&mut us, &mut vs, &mut ys] {
        list.sort_by(f32::total_cmp);
    }
    // (Leaving out the odd press far from the rest.)
    let at = |list: &[f32], f: f32| list[((list.len() - 1) as f32 * f).round() as usize];
    let (u0, u1, v0, v1) = (at(&us, 0.05), at(&us, 0.95), at(&vs, 0.05), at(&vs, 0.95));
    if u1 - u0 < SPREAD_NEEDED {
        return None;
    }
    let size = (Vec2::new(u1 - u0, v1 - v0) + MARGIN * 2.0).max(LEAST);
    let (u, v) = ((u0 + u1) / 2.0, (v0 + v1) / 2.0);
    let center = across * u + deep * v + Vec3::Y * at(&ys, 0.5);
    Some(Area { center, across, size })
}

/// The keyboard marked by its front edge from `a` to `b` and its back edge
/// through `back` (the controller's tip at each), where the edge is long
/// enough.
pub fn marked(a: Vec3, b: Vec3, back: Vec3) -> Option<Area> {
    let edge = Vec3::new(b.x - a.x, 0.0, b.z - a.z);
    if edge.length() < LEAST_EDGE {
        return None;
    }
    let across = edge.normalize();
    let deep = across.cross(Vec3::Y);
    let depth = (back - a).dot(deep);
    let middle = (a + b) / 2.0;
    let center = middle + deep * depth / 2.0;
    let size = Vec2::new(edge.length(), depth.abs()).max(Vec2::new(LEAST_EDGE, 0.05));
    Some(Area { center: Vec3::new(center.x, (a.y + b.y) / 2.0, center.z), across, size })
}

/// Marking with a controller.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Marking {
    Off,
    /// Waiting for the trigger at one end of the front edge.
    Start,
    /// The trigger held since `from`, on `hand`.
    Edge { hand: usize, from: Vec3 },
    /// The front edge drawn; waiting for the trigger at the back edge.
    Back { hand: usize, a: Vec3, b: Vec3 },
}

/// The keyboard's place, found and marked, and saved in `path`.
pub struct Keyboard {
    mode: VrKeyboard,
    /// The space the found and marked places are kept in.
    anchor_name: &'static str,
    path: Option<PathBuf>,
    /// Where it is at first, for a scene and a seat.
    default: Option<(usize, Mat4, (Mat4, Vec2))>,
    learned: Option<Area>,
    /// What the window shows now, easing toward `learned`.
    shown: Option<Area>,
    marked: Option<Area>,
    tips: Vec<Vec3>,
    since_saved: usize,
    presses: u32,
    marking: Marking,
    triggers: [bool; 2],
    squeezes: [bool; 2],
    /// Where the window was last said to be, in the log.
    said: Option<&'static str>,
}

impl Keyboard {
    /// The places kept in `anchor_name`'s space, saved in `path`.
    pub fn new(mode: VrKeyboard, anchor_name: &'static str, path: Option<PathBuf>) -> Self {
        let mut keyboard = Keyboard {
            mode,
            anchor_name,
            path,
            default: None,
            learned: None,
            shown: None,
            marked: None,
            tips: Vec::new(),
            since_saved: 0,
            presses: 0,
            marking: Marking::Off,
            triggers: [false; 2],
            squeezes: [false; 2],
            said: None,
        };
        keyboard.load();
        keyboard
    }

    pub fn set_mode(&mut self, mode: VrKeyboard) {
        self.mode = mode;
    }

    /// Start marking the keyboard with a controller.
    pub fn mark(&mut self) {
        if self.mode != VrKeyboard::Off {
            self.marking = Marking::Start;
        }
    }

    /// Whether the controllers are marking: they do nothing else then.
    pub fn marking(&self) -> bool {
        self.marking != Marking::Off
    }

    /// The `presses` of the host's keys so far, the head and hands as
    /// `tracking` has them in `scene` (whose seat `world` is), at `dt`
    /// seconds since the last frame: where the real world shows through,
    /// and the boxes that show the marking.
    pub fn update(&mut self, scene: &Scene, world: Mat4, tracking: &Tracking, presses: u32, dt: f32) -> (Vec<Window>, Option<f32>, Vec<Extra>) {
        let pressed = presses != self.presses;
        self.presses = presses;
        if self.mode == VrKeyboard::Off {
            return (Vec::new(), None, Vec::new());
        }
        // (What is kept in the room needs to know where the room is.)
        let mut extras = Vec::new();
        let mut kept = None;
        if let Some(anchor) = tracking.anchor {
            let from_scene = anchor.inverse();
            let across = from_scene.transform_vector3(scene.screen.right);
            if pressed && !self.marking() {
                self.learn(tracking, from_scene, across);
            }
            if let Some(learned) = self.learned {
                let t = (EASE * dt).clamp(0.0, 1.0);
                self.shown = Some(self.shown.map_or(learned, |shown| shown.lerp(&learned, t)));
            }
            extras = self.update_marking(tracking, from_scene);
            let area = match self.mode {
                VrKeyboard::Marked => self.marked.or(self.shown),
                _ => self.shown,
            };
            kept = area.map(|area| area.window(anchor));
        }
        if self.marking() {
            return (Vec::new(), Some(MARKING_SEEN), extras);
        }
        let source = match (kept.is_some(), self.mode) {
            (false, _) => "on the desk below the screen",
            (true, VrKeyboard::Marked) if self.marked.is_some() => "where it was marked",
            (true, _) => "where the fingers typed",
        };
        if self.said != Some(source) {
            self.said = Some(source);
            super::xr::log::line(format!("The keyboard is shown {}", source));
        }
        let window = kept.unwrap_or_else(|| {
            let key = (scene as *const Scene as usize, world);
            if self.default.is_none_or(|(s, w, _)| (s, w) != key) {
                self.default = Some((key.0, key.1, default_area(scene, world)));
            }
            let (_, _, (frame, size)) = self.default.expect("just made");
            Window { frame, size, feather: FEATHER }
        });
        (vec![window], None, extras)
    }

    /// The fingertip lowest at a press, where hands are tracked: into the
    /// fit, and the place found from them saved now and then.
    fn learn(&mut self, tracking: &Tracking, from_scene: Mat4, across: Vec3) {
        let head = tracking.head.map(|h| from_scene.transform_point3(h.transform_point3(Vec3::ZERO)));
        let lowest = (0..2)
            .filter_map(|i| {
                let tips = tracking.tips[i]?;
                let grip = tracking.hands[i].map(|h: Hand| h.grip.transform_point3(Vec3::ZERO));
                // (The runtime's hand round a controller held.)
                if grip.is_some_and(|g| tips.iter().any(|t| t.distance(g) < NEAR_CONTROLLER)) {
                    return None;
                }
                tips.iter().map(|&t| from_scene.transform_point3(t)).min_by(|a, b| a.y.total_cmp(&b.y))
            })
            .min_by(|a, b| a.y.total_cmp(&b.y));
        let Some(tip) = lowest else { return };
        // Not on a keyboard in front of the head at all.
        if head.is_some_and(|h| h.y - tip.y < 0.15 || Vec2::new(h.x - tip.x, h.z - tip.z).length() > 1.0) {
            return;
        }
        if self.tips.len() == SAMPLES {
            self.tips.remove(0);
        }
        self.tips.push(tip);
        if let Some(found) = fit(&self.tips, across) {
            self.learned = Some(found);
            self.since_saved += 1;
            if self.since_saved >= SAVE_EVERY {
                self.since_saved = 0;
                self.save();
            }
        }
    }

    /// Marking with the controllers' tips: the outline drawn as it goes.
    fn update_marking(&mut self, tracking: &Tracking, from_scene: Mat4) -> Vec<Extra> {
        let hands = tracking.hands;
        let tip = |i: usize| hands[i].map(|h| from_scene.transform_point3(h.aim.transform_point3(Vec3::ZERO)));
        let pulled: Vec<usize> = (0..2).filter(|&i| hands[i].is_some_and(|h| h.select) && !self.triggers[i]).collect();
        let squeezed = (0..2).any(|i| hands[i].is_some_and(|h| h.squeeze) && !self.squeezes[i]);
        for i in 0..2 {
            self.triggers[i] = hands[i].is_some_and(|h| h.select);
            self.squeezes[i] = hands[i].is_some_and(|h| h.squeeze);
        }
        if squeezed && self.marking() {
            self.marking = Marking::Off;
            return Vec::new();
        }
        let to_scene = from_scene.inverse();
        let mut extras = Vec::new();
        match self.marking {
            Marking::Off => {}
            Marking::Start => {
                if let Some(&hand) = pulled.first()
                    && let Some(from) = tip(hand)
                {
                    self.marking = Marking::Edge { hand, from };
                }
            }
            Marking::Edge { hand, from } => {
                let Some(to) = tip(hand) else { return extras };
                extras.extend(outline(&[from, to], to_scene));
                if !self.triggers[hand] {
                    let long = Vec2::new(to.x - from.x, to.z - from.z).length() >= LEAST_EDGE;
                    self.marking = if long { Marking::Back { hand, a: from, b: to } } else { Marking::Start };
                }
            }
            Marking::Back { hand, a, b } => {
                let back = tip(hand).unwrap_or(b);
                if let Some(area) = marked(a, b, back) {
                    extras.extend(outline(&corners(&area), to_scene));
                    if pulled.contains(&hand) {
                        self.marked = Some(area);
                        self.mode = VrKeyboard::Marked;
                        self.marking = Marking::Off;
                        self.save();
                    }
                }
            }
        }
        extras
    }

    /// Read the places saved for this space: lines of `space source
    /// area`.
    fn load(&mut self) {
        let Some(text) = self.path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) else { return };
        for line in text.lines() {
            let mut words = line.splitn(3, ' ');
            let (Some(space), Some(source), Some(area)) = (words.next(), words.next(), words.next()) else { continue };
            if space != self.anchor_name {
                continue;
            }
            match (source, Area::parse(area)) {
                ("marked", Some(area)) => self.marked = Some(area),
                ("learned", Some(area)) => {
                    self.learned = Some(area);
                    self.shown = Some(area);
                }
                _ => {}
            }
        }
    }

    /// Save the places, keeping those of other spaces.
    fn save(&self) {
        let Some(path) = &self.path else { return };
        let old = std::fs::read_to_string(path).unwrap_or_default();
        let mut lines: Vec<String> = old
            .lines()
            .filter(|l| !l.starts_with(&format!("{} ", self.anchor_name)) && !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        for (source, area) in [("marked", self.marked), ("learned", self.learned)] {
            if let Some(area) = area {
                lines.push(format!("{} {} {}", self.anchor_name, source, area.line()));
            }
        }
        if let Err(e) = std::fs::write(path, lines.join("\n") + "\n") {
            super::xr::log::line(format!("The keyboard's place can't be saved in {}: {}", path.display(), e));
        }
    }

    /// What the renderer is to let through.
    pub fn punch<'a>(windows: &'a [Window], whole: Option<f32>) -> Punch<'a> {
        match whole {
            Some(much) => Punch::Whole(much),
            None => Punch::Windows(windows),
        }
    }
}

/// The corners of `area`, going round.
fn corners(area: &Area) -> [Vec3; 4] {
    let frame = area.frame(Mat4::IDENTITY);
    let (w, d) = (area.size.x / 2.0, area.size.y / 2.0);
    [(-w, -d), (w, -d), (w, d), (-w, d)].map(|(x, z)| frame.transform_point3(Vec3::new(x, 0.0, z)))
}

/// Thin bars from point to point (a closed loop for more than two), in the
/// anchor's space, into the scene through `to_scene`.
fn outline(points: &[Vec3], to_scene: Mat4) -> Vec<Extra> {
    let n = points.len();
    let edges = if n > 2 { n } else { n - 1 };
    (0..edges)
        .filter_map(|i| {
            let (a, b) = (to_scene.transform_point3(points[i]), to_scene.transform_point3(points[(i + 1) % n]));
            let along = b - a;
            let length = along.length();
            if length < 1e-4 {
                return None;
            }
            let rotation = glam::Quat::from_rotation_arc(Vec3::Z, along / length);
            let model = Mat4::from_rotation_translation(rotation, (a + b) / 2.0) * Mat4::from_scale(Vec3::new(0.004, 0.004, length));
            Some(Extra { model, color: [0.4, 1.4, 0.6, 0.9] })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn typing_finds_the_keyboard_and_ignores_stray_presses() {
        // A keyboard 40 x 12 cm, its middle at (0.1, 0.75, -0.4), rows
        // along x; presses all over its keys, and two far off.
        let mut tips = Vec::new();
        for i in 0..60 {
            let u = (i % 10) as f32 / 9.0 - 0.5;
            let v = (i / 10) as f32 / 5.0 - 0.5;
            tips.push(Vec3::new(0.1 + u * 0.36, 0.75, -0.4 + v * 0.07));
        }
        tips.push(Vec3::new(2.0, 0.2, 3.0));
        tips.push(Vec3::new(-2.0, 1.5, -3.0));
        let area = fit(&tips, Vec3::X).expect("enough presses");
        assert!(near(area.center, Vec3::new(0.1, 0.75, -0.4)), "{:?}", area);
        assert!((area.size.x - (0.36 + 0.04)).abs() < 0.05, "{:?}", area);
        assert!((LEAST.y..0.14).contains(&area.size.y), "{:?}", area);
        // Too few, or all on a few keys, isn't enough.
        assert_eq!(fit(&tips[..10], Vec3::X), None);
        assert_eq!(fit(&[Vec3::new(0.0, 0.7, -0.4); 40], Vec3::X), None);
    }

    #[test]
    fn marking_takes_the_front_edge_and_the_back() {
        let (a, b) = (Vec3::new(-0.2, 0.74, -0.3), Vec3::new(0.24, 0.76, -0.3));
        // The back edge 15 cm further away (-z).
        let area = marked(a, b, Vec3::new(0.0, 0.8, -0.45)).expect("long enough");
        assert!(near(area.center, Vec3::new(0.02, 0.75, -0.375)), "{:?}", area);
        assert!((area.size - Vec2::new(0.44, 0.15)).length() < 1e-3, "{:?}", area);
        let c = corners(&area);
        assert!(c.iter().all(|p| (p.y - 0.75).abs() < 1e-4));
        assert!(marked(a, a + Vec3::new(0.05, 0.0, 0.0), Vec3::ZERO).is_none());
    }

    #[test]
    fn places_are_saved_as_text_and_read_back() {
        let area = Area { center: Vec3::new(0.1, 0.7, -0.4), across: Vec3::new(0.6, 0.0, 0.8), size: Vec2::new(0.45, 0.15) };
        let back = Area::parse(&area.line()).expect("read back");
        assert!(near(back.center, area.center) && near(back.across, area.across));
        assert!((back.size - area.size).length() < 1e-3);
        assert_eq!(Area::parse("1 2 3"), None);
        assert_eq!(Area::parse("0 0 0 0 0 0.4 0.1"), None);
    }

    #[test]
    fn the_default_is_on_the_test_desk_toward_the_screen() {
        let scene = Scene::test_room();
        let world = Mat4::from_translation(scene.spawn.position) * Mat4::from_rotation_y(scene.spawn.yaw);
        let (frame, size) = default_area(&scene, world);
        let at = frame.transform_point3(Vec3::ZERO);
        let seat = scene.spawn.position;
        let toward = (scene.screen.center - seat) * Vec3::new(1.0, 0.0, 1.0);
        assert!((at - seat).dot(toward.normalize()) > 0.3, "{:?}", at);
        assert!(at.y < seat.y - 0.2, "{:?}", at);
        assert_eq!(size, TYPICAL);
    }
}
