//! The things on the desk to pick up (`Scene::props`): the game's manual,
//! a booklet whose pages turn with the thumbstick of the hand holding it,
//! and a joystick, whose stick, held, follows the hand and is the game
//! port's joystick, and whose base moves it. What is put down settles
//! upright on what is below it.
//!
//! The manual's pages are rendered on a thread of their own, as they are
//! needed, into textures of the context that draws them.

use super::controls::Tracking;
use super::render::{Gpu, PropDraw};
use super::scene::{PartRole, Prop, PropKind, Scene};
use glam::{Mat4, Quat, Vec3};
use glow::HasContext;
use rust_dos::padmap::PadSnapshot;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

/// How far outside a prop a hand still takes it, and from the knob the
/// stick, in metres.
const REACH: f32 = 0.05;
const KNOB_REACH: f32 = 0.07;
/// How far the stick tilts, and the tilt that is the joystick's full
/// deflection (less, so that it is reached at the corners too).
const MAX_TILT: f32 = 22.0 * std::f32::consts::PI / 180.0;
const FULL_TILT: f32 = MAX_TILT * 0.8;
/// How long a page takes to turn, and how long the thumbstick is held to
/// turn another.
const TURN: Duration = Duration::from_millis(350);
const REPEAT: Duration = Duration::from_millis(450);
/// The thumbstick's push that turns a page.
const PUSH: f32 = 0.6;
/// How tall the pages' textures are, in pixels.
const PAGE_HEIGHT: u32 = 1024;
/// How far from the pages shown the textures of others are kept.
const KEEP: usize = 6;

/// What a hand holds.
#[derive(Clone, Copy, Debug)]
enum Hold {
    /// A prop, where it is in the hand's frame.
    Body { prop: usize, offset: Mat4 },
    /// The joystick's stick, the hand where it was in the stick's frame
    /// when it took it.
    Stick { prop: usize, from: Vec3 },
}

/// What the props do to the machine at a frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct PropInput {
    /// The hands holding something, whose buttons and stick are the
    /// prop's.
    pub busy: [bool; 2],
    /// The joystick's stick, held: the game port's joystick.
    pub pad: Option<PadSnapshot>,
}

pub struct Props {
    poses: Vec<Mat4>,
    /// Each prop's stick: the way it points in its frame.
    tilts: Vec<Vec3>,
    holds: [Option<Hold>; 2],
    squeezed: [bool; 2],
    pressed: [bool; 2],
    booklet: Booklet,
}

impl Props {
    pub fn new(scene: &Scene) -> Self {
        let mut props = Props {
            poses: Vec::new(),
            tilts: Vec::new(),
            holds: [None; 2],
            squeezed: [false; 2],
            pressed: [false; 2],
            booklet: Booklet::default(),
        };
        props.reset(scene);
        props
    }

    /// Back where `scene` has them.
    pub fn reset(&mut self, scene: &Scene) {
        self.poses = scene.props.iter().map(|p| p.pose).collect();
        self.tilts = vec![Vec3::Y; scene.props.len()];
        self.holds = [None; 2];
        self.booklet.canvas = scene.props.iter().find(|p| p.kind == PropKind::Manual).map_or((768, PAGE_HEIGHT), page_canvas);
    }

    /// The game's manuals, for the booklet: none hides it.
    pub fn set_manuals(&mut self, gl: &glow::Context, manuals: Vec<PathBuf>) {
        if manuals != self.booklet.manuals {
            self.booklet.forget(gl);
            self.booklet.manuals = manuals;
            self.booklet.current = 0;
            self.booklet.open();
        }
    }

    /// Whether prop `i` is shown: the manual only with manuals to show.
    fn shown(&self, prop: &Prop) -> bool {
        prop.kind != PropKind::Manual || !self.booklet.manuals.is_empty()
    }

    /// Take, move, put down and work the props with the hands of
    /// `tracking`.
    pub fn update(&mut self, scene: &Scene, tracking: &Tracking, now: Instant) -> PropInput {
        let mut input = PropInput::default();
        for (i, hand) in tracking.hands.iter().enumerate() {
            let Some(hand) = hand else {
                // Out of sight: what it held is put down.
                self.release(scene, i);
                self.squeezed[i] = false;
                continue;
            };
            let grip = hand.grip.transform_point3(Vec3::ZERO);
            if hand.squeeze && !self.squeezed[i] && self.holds[i].is_none() {
                self.holds[i] = self.take(scene, grip, hand.grip);
                // Taken from the other hand.
                if let Some(Hold::Body { prop, .. }) = self.holds[i]
                    && let Some(Hold::Body { prop: other, .. }) = self.holds[1 - i]
                    && other == prop
                {
                    self.holds[1 - i] = None;
                }
            }
            if !hand.squeeze {
                self.release(scene, i);
            }
            self.squeezed[i] = hand.squeeze;
            match self.holds[i] {
                Some(Hold::Body { prop, offset }) => {
                    self.poses[prop] = hand.grip * offset;
                    if scene.props[prop].kind == PropKind::Manual {
                        // Its pages turn with the thumbstick, another manual
                        // is the primary button's.
                        self.booklet.steer(hand.stick.x, now);
                        if hand.primary && !self.pressed[i] {
                            self.booklet.next_manual();
                        }
                    }
                }
                Some(Hold::Stick { prop, from }) => {
                    let frame = self.poses[prop] * scene.props[prop].stick;
                    let to = frame.inverse().transform_point3(grip);
                    let tilt = clamp_tilt(Quat::from_rotation_arc(from.normalize_or(Vec3::Y), to.normalize_or(Vec3::Y)) * Vec3::Y);
                    self.tilts[prop] = tilt;
                    // Pushed away is up, as on a joystick; the trigger is the
                    // first button, the primary the second.
                    let axis = |side: f32| (side.atan2(tilt.y) / FULL_TILT).clamp(-1.0, 1.0);
                    let mut pad = PadSnapshot { axes: [axis(tilt.x), axis(tilt.z), 0.0, 0.0], ..PadSnapshot::default() };
                    if hand.select {
                        pad.buttons |= rust_dos::vr::PAD_A;
                    }
                    if hand.primary {
                        pad.buttons |= rust_dos::vr::PAD_B;
                    }
                    input.pad = Some(pad);
                }
                None => {}
            }
            self.pressed[i] = hand.primary;
            input.busy[i] = self.holds[i].is_some();
        }
        input
    }

    /// What hand at `grip` (its frame `hand`) takes: the stick first, then
    /// the nearest prop it is in or by.
    fn take(&self, scene: &Scene, grip: Vec3, hand: Mat4) -> Option<Hold> {
        let mut best: Option<(f32, Hold)> = None;
        for (i, prop) in scene.props.iter().enumerate().filter(|(_, p)| self.shown(p)) {
            let pose = self.poses[i];
            if prop.parts.iter().any(|p| p.role == PartRole::Stick) {
                let frame = pose * prop.stick;
                let knob = frame.transform_point3(self.tilt_matrix(i).transform_point3(prop.knob));
                if grip.distance(knob) < KNOB_REACH {
                    return Some(Hold::Stick { prop: i, from: frame.inverse().transform_point3(grip) });
                }
            }
            let local = pose.inverse().transform_point3(grip);
            let (low, high) = prop.bounds;
            let outside = (low - local).max(local - high).max(Vec3::ZERO).length();
            if outside < REACH && best.is_none_or(|(d, _)| outside < d) {
                best = Some((outside, Hold::Body { prop: i, offset: hand.inverse() * pose }));
            }
        }
        best.map(|(_, hold)| hold)
    }

    /// Hand `i` lets go: a prop settles upright on what is below it; a
    /// stick springs back.
    fn release(&mut self, scene: &Scene, i: usize) {
        match self.holds[i].take() {
            Some(Hold::Body { prop, .. }) => {
                let (_, rotation, position) = self.poses[prop].to_scale_rotation_translation();
                let ahead = rotation * Vec3::NEG_Z;
                let yaw = if ahead.x.abs() + ahead.z.abs() > 1e-4 { (-ahead.x).atan2(-ahead.z) } else { 0.0 };
                let upright = Mat4::from_rotation_y(yaw);
                let lift = Vec3::Y * scene.props[prop].bounds.1.y.max(0.05);
                let at = scene.surface_below(position + lift, 3.0).unwrap_or(position);
                self.poses[prop] = Mat4::from_translation(at) * upright;
            }
            Some(Hold::Stick { prop, .. }) => self.tilts[prop] = Vec3::Y,
            None => {}
        }
    }

    fn tilt_matrix(&self, prop: usize) -> Mat4 {
        Mat4::from_quat(Quat::from_rotation_arc(Vec3::Y, self.tilts[prop]))
    }

    /// The props' parts to draw, the manual's pages rendered as they come.
    pub fn draws(&mut self, gl: &glow::Context, gpu: &Gpu, scene: &Scene, now: Instant) -> Vec<PropDraw> {
        self.booklet.receive(gl, gpu);
        let pages = self.booklet.showing(now);
        let mut draws = Vec::new();
        for (i, prop) in scene.props.iter().enumerate().filter(|(_, p)| self.shown(p)) {
            let pose = self.poses[i];
            for (part, p) in prop.parts.iter().enumerate() {
                let (model, page) = match p.role {
                    PartRole::Body => (pose, None),
                    PartRole::PageLeft => (pose, self.booklet.texture(pages.left)),
                    PartRole::PageRight => (pose, self.booklet.texture(pages.right)),
                    PartRole::SheetFront | PartRole::SheetBack => {
                        let Some((angle, front, back)) = pages.turning else { continue };
                        let page = if p.role == PartRole::SheetFront { front } else { back };
                        (pose * Mat4::from_translation(prop.hinge) * Mat4::from_rotation_z(angle), self.booklet.texture(page))
                    }
                    PartRole::Stick => (pose * prop.stick * self.tilt_matrix(i), None),
                };
                draws.push(PropDraw { prop: i, part, model, page });
            }
        }
        draws
    }

    /// Delete the pages' textures, with `gl` current.
    pub fn delete(mut self, gl: &glow::Context) {
        self.booklet.forget(gl);
    }
}

/// `tilt` no further than `MAX_TILT` from upright.
fn clamp_tilt(tilt: Vec3) -> Vec3 {
    let tilt = tilt.normalize_or(Vec3::Y);
    let angle = tilt.y.clamp(-1.0, 1.0).acos();
    if angle <= MAX_TILT {
        return tilt;
    }
    let across = Vec3::new(tilt.x, 0.0, tilt.z).normalize_or(Vec3::X);
    across * MAX_TILT.sin() + Vec3::Y * MAX_TILT.cos()
}

/// The size of the manual's pages' textures: `PAGE_HEIGHT` tall, as wide as
/// its right page is for that.
fn page_canvas(prop: &Prop) -> (u32, u32) {
    let page = prop.parts.iter().find(|p| p.role == PartRole::PageRight);
    let (low, high) = page
        .map(|p| {
            let points = p.mesh.vertices.iter().map(|v| Vec3::from(v.position));
            points.fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), q| (l.min(q), h.max(q)))
        })
        .unwrap_or((Vec3::ZERO, Vec3::new(0.148, 0.0, 0.21)));
    let (w, d) = ((high.x - low.x).max(0.01), (high.z - low.z).max(0.01));
    (((PAGE_HEIGHT as f32 * w / d).round() as u32).clamp(64, 2048), PAGE_HEIGHT)
}

/// The pages a booklet shows: on the left, on the right, and turning (its
/// angle about the spine, its front and its back).
struct Showing {
    left: usize,
    right: usize,
    turning: Option<(f32, usize, usize)>,
}

/// The manual: which of the game's, its pages and the textures of those.
#[derive(Default)]
struct Booklet {
    manuals: Vec<PathBuf>,
    current: usize,
    /// How many pages it has, once it is opened.
    pages: Option<usize>,
    /// Pages 2k and 2k+1 show.
    spread: usize,
    /// Turning from a spread to another since a time.
    turning: Option<(usize, usize, Instant)>,
    /// The thumbstick's direction held, and when it turns again.
    held: Option<(i32, Instant)>,
    textures: HashMap<usize, glow::Texture>,
    /// Textures of another manual, to delete.
    stale: Vec<glow::Texture>,
    asked: HashSet<usize>,
    canvas: (u32, u32),
    /// Which opening of a manual the pages come from.
    generation: u64,
    worker: Option<Worker>,
}

struct Worker {
    ask: Sender<Ask>,
    done: Receiver<Done>,
}

enum Ask {
    Open(u64, PathBuf),
    Page(u64, usize, (u32, u32)),
}

enum Done {
    Opened(u64, Result<usize, String>),
    Page(u64, usize, Vec<u8>, (u32, u32)),
}

impl Booklet {
    /// Open the current manual, on the thread.
    fn open(&mut self) {
        self.pages = None;
        self.spread = 0;
        self.turning = None;
        self.generation += 1;
        let Some(path) = self.manuals.get(self.current).cloned() else { return };
        if self.worker.is_none() {
            self.worker = start_worker();
        }
        if let Some(worker) = &self.worker {
            let _ = worker.ask.send(Ask::Open(self.generation, path));
        }
    }

    fn next_manual(&mut self) {
        if self.manuals.len() > 1 {
            self.current = (self.current + 1) % self.manuals.len();
            self.textures_stale();
            self.open();
        }
    }

    /// The textures are of another manual: forgotten as they are replaced.
    fn textures_stale(&mut self) {
        self.asked.clear();
        // (Deleted where `gl` is at hand: in `receive`.)
        self.stale.extend(self.textures.drain().map(|(_, t)| t));
    }

    fn forget(&mut self, gl: &glow::Context) {
        for texture in self.textures.drain().map(|(_, t)| t).chain(self.stale.drain(..)) {
            // SAFETY: see `GlScreen`.
            unsafe { gl.delete_texture(texture) };
        }
        self.asked.clear();
    }

    /// The thumbstick of the hand holding it: pushed right, the next pages;
    /// left, the ones before; again while it is held.
    fn steer(&mut self, x: f32, now: Instant) {
        let way = if x > PUSH { 1 } else if x < -PUSH { -1 } else { 0 };
        if way == 0 {
            self.held = None;
            return;
        }
        if self.held.is_some_and(|(held, at)| held == way && now < at) || self.turning.is_some() {
            return;
        }
        self.held = Some((way, now + REPEAT));
        let last = self.pages.map_or(0, |n| n.saturating_sub(1) / 2);
        let to = (self.spread as i64 + way as i64).clamp(0, last as i64) as usize;
        if to != self.spread {
            self.turning = Some((self.spread, to, now));
        }
    }

    /// What shows at `now`; a turn that is over is done.
    fn showing(&mut self, now: Instant) -> Showing {
        if let Some((_, to, since)) = self.turning
            && now.duration_since(since) >= TURN
        {
            self.spread = to;
            self.turning = None;
        }
        let shown = match self.turning {
            Some((from, to, since)) => {
                let t = (now.duration_since(since).as_secs_f32() / TURN.as_secs_f32()).clamp(0.0, 1.0);
                // Eased: quick in the middle.
                let t = t * t * (3.0 - 2.0 * t);
                let low = from.min(to);
                let angle = if to > from { t } else { 1.0 - t } * std::f32::consts::PI;
                Showing { left: 2 * low, right: 2 * low + 3, turning: Some((angle, 2 * low + 1, 2 * low + 2)) }
            }
            None => Showing { left: 2 * self.spread, right: 2 * self.spread + 1, turning: None },
        };
        // Those shown and the next and last pages, the rest let go.
        let around = 2 * self.spread;
        let wanted: Vec<usize> = (around.saturating_sub(2)..around + 6).collect();
        for page in wanted {
            self.ask(page);
        }
        shown
    }

    fn ask(&mut self, page: usize) {
        if self.pages.is_none_or(|n| page >= n) || !self.asked.insert(page) {
            return;
        }
        if let Some(worker) = &self.worker {
            let _ = worker.ask.send(Ask::Page(self.generation, page, self.canvas));
        }
    }

    fn texture(&self, page: usize) -> Option<glow::Texture> {
        self.textures.get(&page).copied()
    }

    /// Take what the thread rendered.
    fn receive(&mut self, gl: &glow::Context, gpu: &Gpu) {
        for texture in self.stale.drain(..) {
            // SAFETY: see `GlScreen`.
            unsafe { gl.delete_texture(texture) };
        }
        let Some(worker) = &self.worker else { return };
        let mut done = Vec::new();
        while let Ok(d) = worker.done.try_recv() {
            done.push(d);
        }
        for d in done {
            match d {
                Done::Opened(generation, pages) if generation == self.generation => match pages {
                    Ok(n) => self.pages = Some(n),
                    Err(e) => {
                        eprintln!("[VR] The manual can't be shown: {}", e);
                        self.pages = Some(0);
                    }
                },
                Done::Page(generation, page, rgba, size) if generation == self.generation => {
                    if let Some(texture) = gpu.page_texture(gl, &rgba, size)
                        && let Some(old) = self.textures.insert(page, texture)
                    {
                        // SAFETY: see `GlScreen`.
                        unsafe { gl.delete_texture(old) };
                    }
                }
                _ => {}
            }
        }
        // Pages far from those shown go.
        let around = 2 * self.spread;
        let far: Vec<usize> = self.textures.keys().copied().filter(|&p| p + KEEP < around || p > around + KEEP + 2).collect();
        for page in far {
            if let Some(texture) = self.textures.remove(&page) {
                self.asked.remove(&page);
                // SAFETY: see `GlScreen`.
                unsafe { gl.delete_texture(texture) };
            }
        }
    }
}

/// The thread rendering a manual's pages, as they are asked for.
fn start_worker() -> Option<Worker> {
    let (ask, asked) = std::sync::mpsc::channel::<Ask>();
    let (send, done) = std::sync::mpsc::channel::<Done>();
    std::thread::Builder::new()
        .name("vr-manual".into())
        .spawn(move || {
            let mut document: Option<(u64, rust_dos::manuals::Document)> = None;
            while let Ok(ask) = asked.recv() {
                match ask {
                    Ask::Open(generation, path) => {
                        let opened = rust_dos::manuals::Document::open(&path);
                        let pages = opened.as_ref().map(|d| d.pages()).map_err(|e| e.clone());
                        document = opened.ok().map(|d| (generation, d));
                        if send.send(Done::Opened(generation, pages)).is_err() {
                            return;
                        }
                    }
                    Ask::Page(generation, page, canvas) => {
                        let Some((_, document)) = document.as_ref().filter(|(g, _)| *g == generation) else { continue };
                        let rgba = render_page(document, page, canvas);
                        if send.send(Done::Page(generation, page, rgba, canvas)).is_err() {
                            return;
                        }
                    }
                }
            }
        })
        .ok()?;
    Some(Worker { ask, done })
}

/// Page `page` in the middle of a white `canvas`, as big as fits, RGBA.
fn render_page(document: &rust_dos::manuals::Document, page: usize, (cw, ch): (u32, u32)) -> Vec<u8> {
    let mut rgba = vec![255u8; (cw * ch * 4) as usize];
    let (pw, ph) = document.page_size(page);
    if pw <= 0.0 || ph <= 0.0 {
        return rgba;
    }
    let scale = (cw as f32 / pw).min(ch as f32 / ph);
    let (w, h) = (((pw * scale) as u32).clamp(1, cw), ((ph * scale) as u32).clamp(1, ch));
    let frame = document.render(page, w, h);
    let (fw, fh) = (frame.width.min(cw), frame.height.min(ch));
    let (x0, y0) = ((cw - fw) / 2, (ch - fh) / 2);
    for y in 0..fh {
        for x in 0..fw {
            let from = ((y * frame.width + x) * 3) as usize;
            let to = (((y0 + y) * cw + x0 + x) * 4) as usize;
            rgba[to..to + 3].copy_from_slice(&frame.rgb[from..from + 3]);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::super::controls::Hand;
    use super::*;

    fn hand_at(p: Vec3, squeeze: bool) -> Hand {
        let grip = Mat4::from_translation(p);
        Hand { aim: grip, grip, squeeze, ..Hand::default() }
    }

    fn tracking(right: Hand) -> Tracking {
        Tracking { head: None, hands: [None, Some(right)] }
    }

    #[test]
    fn the_joystick_is_taken_by_its_stick_and_follows_the_hand() {
        let room = Scene::test_room();
        let mut props = Props::new(&room);
        let now = Instant::now();
        let j = room.props.iter().position(|p| p.kind == PropKind::Joystick).unwrap();
        let prop = &room.props[j];
        let knob = (prop.pose * prop.stick).transform_point3(prop.knob);
        let input = props.update(&room, &tracking(hand_at(knob, true)), now);
        assert!(input.busy[1]);
        let pad = input.pad.unwrap();
        assert!(pad.axes[0].abs() < 1e-3 && pad.axes[1].abs() < 1e-3, "{:?}", pad.axes);
        // Pushed away (-Z) and to the right: up and right, at full tilt.
        let input = props.update(&room, &tracking(hand_at(knob + Vec3::new(0.08, 0.0, -0.08), true)), now);
        let pad = input.pad.unwrap();
        assert!(pad.axes[0] > 0.85 && pad.axes[1] < -0.85, "{:?}", pad.axes);
        // Let go, it stands up again and isn't the joystick any more.
        let input = props.update(&room, &tracking(hand_at(knob, false)), now);
        assert!(input.pad.is_none() && !input.busy[1]);
        assert_eq!(props.tilts[j], Vec3::Y);
    }

    #[test]
    fn props_put_down_stand_on_what_is_below() {
        let room = Scene::test_room();
        let mut props = Props::new(&room);
        let now = Instant::now();
        let j = room.props.iter().position(|p| p.kind == PropKind::Joystick).unwrap();
        let base = room.props[j].pose.transform_point3(Vec3::new(0.0, 0.02, 0.0));
        props.update(&room, &tracking(hand_at(base, true)), now);
        // Carried over the desk's other side, and turned a little.
        let moved = base + Vec3::new(-0.4, 0.3, -0.1);
        let mut hand = hand_at(moved, true);
        hand.grip *= Mat4::from_rotation_x(0.3) * Mat4::from_rotation_y(0.5);
        props.update(&room, &tracking(hand), now);
        props.update(&room, &tracking(hand_at(moved, false)), now);
        let (_, rotation, at) = props.poses[j].to_scale_rotation_translation();
        assert!((at.y - 0.72).abs() < 1e-3, "on the desk: {:?}", at);
        assert!((at.x - (base.x - 0.4)).abs() < 0.05);
        assert!((rotation * Vec3::Y).dot(Vec3::Y) > 0.9999, "upright");
    }

    #[test]
    fn pages_turn_with_the_thumbstick() {
        let mut booklet = Booklet { pages: Some(9), ..Booklet::default() };
        let now = Instant::now();
        booklet.steer(-1.0, now);
        assert!(booklet.turning.is_none(), "not before the first");
        booklet.steer(1.0, now);
        let showing = booklet.showing(now + TURN / 2);
        let (angle, front, back) = showing.turning.unwrap();
        assert!((angle - std::f32::consts::FRAC_PI_2).abs() < 0.1);
        assert_eq!((showing.left, front, back, showing.right), (0, 1, 2, 3));
        // Held, it doesn't turn again until a while later.
        booklet.steer(1.0, now + TURN);
        assert_eq!(booklet.showing(now + TURN).left, 2);
        booklet.steer(1.0, now + REPEAT);
        assert_eq!(booklet.turning.map(|t| t.1), Some(2));
        let after = now + REPEAT + TURN;
        assert_eq!(booklet.showing(after).right, 5);
        // Never past the last pages (8 alone on the left).
        for i in 0..10 {
            let at = after + (REPEAT + TURN) * (i + 1);
            booklet.steer(1.0, at);
            booklet.showing(at + TURN);
        }
        assert_eq!(booklet.spread, 4);
    }

    #[test]
    fn the_tilt_stops_at_full_deflection() {
        let tilt = clamp_tilt(Vec3::new(1.0, 0.1, 0.0));
        assert!((tilt.y.acos() - MAX_TILT).abs() < 1e-4);
        assert_eq!(clamp_tilt(Vec3::Y), Vec3::Y);
    }
}
