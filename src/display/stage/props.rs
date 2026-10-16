//! The things on the desk to pick up (`Scene::props`): the game's manual,
//! a booklet whose pages turn with the thumbstick of the hand holding it,
//! a joystick, whose stick, held, tilts as the hand does and is the game
//! port's joystick, and whose base moves it, and a polaroid camera, which
//! takes a picture when its trigger is pulled: the picture comes out of
//! its slot and develops, to be taken and put down too. What is let go
//! falls (`physics`), thrown as the hand moved. The VFX1 headset
//! (`[vfx1]`), let go at the face, is worn, and gripped at the head taken
//! off again.
//!
//! The manual's pages are rendered on a thread of their own, as they are
//! needed, into textures of the context that draws them.

use super::controls::{Hand, Tracking};
use super::physics::{self, Body, Colliders, Moved, Obstacle, Shape};
use super::pick::{Target, TargetId};
use super::render::{Gpu, PropDraw};
use super::scene::{PRINT, PartRole, Prop, PropKind, Scene, VFX1_EYES};
use glam::{Mat4, Quat, Vec3};
use glow::HasContext;
use rust_dos::padmap::PadSnapshot;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

/// How far outside a prop a hand (its grip, or the tip of the controller
/// drawn for it) still takes it, and from the knob the stick, in metres.
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
/// How long a picture takes to come out of the camera, and to develop.
const EJECT: Duration = Duration::from_millis(1300);
const DEVELOP: Duration = Duration::from_secs(10);
/// The pictures kept at most: the oldest goes.
const PRINTS: usize = 8;
/// How tall the pages' textures are, in pixels.
const PAGE_HEIGHT: u32 = 1024;
/// How far from the pages shown the textures of others are kept.
const KEEP: usize = 6;
/// How far back the hands' moves are kept, for how fast a thing let go was
/// thrown.
const THROW: Duration = Duration::from_millis(80);
/// How near another thing has to be to one taken to fall, then, too.
const NEAR: f32 = 0.03;
/// How thick a picture is to the simulation.
const PRINT_THICKNESS: f32 = 0.001;
/// How near the eyes the VFX1's have to be when it is let go for it to be
/// worn, and a hand to the head to take it off, in metres.
const PUT_ON: f32 = 0.2;
const TAKE_OFF: f32 = 0.25;

/// What a hand holds.
#[derive(Clone, Copy, Debug)]
enum Hold {
    /// A prop, where it is in the hand's frame.
    Body { prop: usize, offset: Mat4 },
    /// The joystick's stick, the way the hand was turned in the stick's
    /// frame when it took it: upright. Only the hand's turn from that
    /// tilts the stick, not where it moves.
    Stick { prop: usize, neutral: Quat },
    /// A camera's picture (`Props::prints`), where it is in the hand's frame.
    Print { print: usize, offset: Mat4 },
}

/// A picture a camera took: where it is, or the camera it is coming out
/// of; when it was taken, and its picture.
struct Print {
    pose: Mat4,
    camera: Option<usize>,
    taken: Instant,
    picture: Option<glow::Texture>,
    /// Falling, let go.
    body: Option<Body>,
}

/// What the props do to the machine at a frame.
#[derive(Clone, Debug, Default)]
pub struct PropInput {
    /// The hands holding something, whose buttons and stick are the
    /// prop's.
    pub busy: [bool; 2],
    /// The joystick's stick, held: the game port's joystick.
    pub pad: Option<PadSnapshot>,
    /// What a hand touched and pulled its trigger at: a lamp to switch.
    pub touched: Vec<TargetId>,
    /// The cameras (by their prop) whose trigger was pulled: a picture to
    /// take.
    pub shots: Vec<usize>,
    /// The VFX1 is worn.
    pub worn: bool,
}

pub struct Props {
    poses: Vec<Mat4>,
    /// Each prop's stick: the way it points in its frame.
    tilts: Vec<Vec3>,
    holds: [Option<Hold>; 2],
    squeezed: [bool; 2],
    pressed: [bool; 2],
    /// Each hand's trigger, pulled before.
    pulled: [bool; 2],
    booklet: Booklet,
    prints: Vec<Print>,
    /// Pictures' textures to delete where `gl` is at hand.
    stale: Vec<glow::Texture>,
    /// Each prop, falling.
    falling: Vec<Option<Body>>,
    colliders: Colliders,
    /// The hands' grips of late, and when.
    moves: [Vec<(Instant, Mat4)>; 2],
    /// Up to when the falling has been simulated.
    simulated: Option<Instant>,
    /// The VFX1 is there (`[vfx1]`), and worn; and the head, at the
    /// frame.
    vfx1: bool,
    worn: bool,
    head: Option<Mat4>,
}

impl Props {
    pub fn new(scene: &Scene) -> Self {
        let mut props = Props {
            poses: Vec::new(),
            tilts: Vec::new(),
            holds: [None; 2],
            squeezed: [false; 2],
            pressed: [false; 2],
            pulled: [false; 2],
            booklet: Booklet::default(),
            prints: Vec::new(),
            stale: Vec::new(),
            falling: Vec::new(),
            colliders: Colliders::default(),
            moves: [Vec::new(), Vec::new()],
            simulated: None,
            vfx1: false,
            worn: false,
            head: None,
        };
        props.reset(scene);
        props
    }

    /// Back where `scene` has them.
    pub fn reset(&mut self, scene: &Scene) {
        self.poses = scene.props.iter().map(|p| p.pose).collect();
        self.tilts = vec![Vec3::Y; scene.props.len()];
        self.holds = [None; 2];
        self.falling = vec![None; scene.props.len()];
        self.colliders = Colliders::new(scene);
        self.moves = [Vec::new(), Vec::new()];
        self.worn = false;
        self.stale.extend(self.prints.drain(..).filter_map(|p| p.picture));
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

    /// Whether prop `i` is shown: the manual only with manuals to show,
    /// the VFX1 while there is one and it isn't worn.
    fn shown(&self, prop: &Prop) -> bool {
        match prop.kind {
            PropKind::Manual => !self.booklet.manuals.is_empty(),
            PropKind::Vfx1 => self.vfx1 && !self.worn,
            _ => true,
        }
    }

    /// The VFX1 there (`[vfx1]`) or not: taken away, it is taken off too.
    pub fn set_vfx1(&mut self, there: bool) {
        self.vfx1 = there;
        if !there {
            self.worn = false;
            for hold in &mut self.holds {
                if matches!(hold, Some(Hold::Body { .. })) {
                    *hold = None;
                }
            }
        }
    }

    /// Put the VFX1 on, as though it had been (`RUST_DOS_VR_WEAR`).
    pub fn wear(&mut self) {
        self.worn = self.vfx1;
    }

    /// Whether the VFX1 let go now at `pose` is worn: its eyes near the
    /// head's, and it facing the way the head does.
    fn on_head(&self, pose: Mat4) -> bool {
        let Some(head) = self.head else { return false };
        let eyes = pose.transform_point3(VFX1_EYES);
        let facing = pose.transform_vector3(Vec3::NEG_Z).normalize_or_zero();
        let looking = head.transform_vector3(Vec3::NEG_Z).normalize_or_zero();
        eyes.distance(head.transform_point3(Vec3::ZERO)) < PUT_ON && facing.dot(looking) > 0.0
    }

    /// The worn VFX1 taken off by `hand` gripping at the head.
    fn take_off(&mut self, scene: &Scene, hand: &Hand) -> Option<Hold> {
        let head = self.head.filter(|_| self.worn)?;
        let grip = hand.grip.transform_point3(Vec3::ZERO);
        if grip.distance(head.transform_point3(Vec3::ZERO)) > TAKE_OFF {
            return None;
        }
        let prop = scene.props.iter().position(|p| p.kind == PropKind::Vfx1)?;
        self.worn = false;
        self.falling[prop] = None;
        // Where it was on the head.
        self.poses[prop] = head * Mat4::from_translation(-VFX1_EYES);
        Some(Hold::Body { prop, offset: hand.grip.inverse() * self.poses[prop] })
    }

    /// Take, move, put down and work the props with the hands of
    /// `tracking`.
    pub fn update(&mut self, scene: &Scene, tracking: &Tracking, now: Instant) -> PropInput {
        let mut input = PropInput::default();
        self.head = tracking.head;
        self.fall(scene, now);
        for (i, hand) in tracking.hands.iter().enumerate() {
            let Some(hand) = hand else {
                // Out of sight: what it held drops.
                self.moves[i].clear();
                self.release(scene, i);
                self.squeezed[i] = false;
                continue;
            };
            self.moves[i].retain(|&(at, _)| now.saturating_duration_since(at) <= THROW);
            self.moves[i].push((now, hand.grip));
            let pulled = std::mem::replace(&mut self.pulled[i], hand.select);
            if hand.squeeze && !self.squeezed[i] && self.holds[i].is_none() {
                self.holds[i] = match self.take_off(scene, hand) {
                    Some(hold) => Some(hold),
                    None => self.take(scene, hand, now),
                };
                // Thrown as it moved since.
                self.moves[i] = vec![(now, hand.grip)];
                // Taken from the other hand.
                if let Some(Hold::Body { prop, .. }) = self.holds[i]
                    && let Some(Hold::Body { prop: other, .. }) = self.holds[1 - i]
                    && other == prop
                {
                    self.holds[1 - i] = None;
                }
                if let Some(Hold::Print { print, .. }) = self.holds[i]
                    && let Some(Hold::Print { print: other, .. }) = self.holds[1 - i]
                    && other == print
                {
                    self.holds[1 - i] = None;
                }
            }
            if !hand.squeeze {
                self.release(scene, i);
            }
            self.squeezed[i] = hand.squeeze;
            match self.holds[i] {
                Some(Hold::Print { print, offset }) => self.prints[print].pose = hand.grip * offset,
                Some(Hold::Body { prop, offset }) => {
                    self.poses[prop] = hand.grip * offset;
                    // A camera's trigger takes a picture.
                    if scene.props[prop].kind == PropKind::Camera && hand.select && !pulled {
                        input.shots.push(prop);
                    }
                    if scene.props[prop].kind == PropKind::Manual {
                        // Its pages turn with the thumbstick, another manual
                        // is the primary button's.
                        self.booklet.steer(hand.stick.x, now);
                        if hand.primary && !self.pressed[i] {
                            self.booklet.next_manual();
                        }
                    }
                }
                Some(Hold::Stick { prop, neutral }) => {
                    // The hand's turn since it took the stick, in the
                    // stick's frame, turns the stick from upright; turned
                    // about the stick, it stays put.
                    let turned = self.stick_turn(scene, prop, hand) * neutral.inverse();
                    let tilt = clamp_tilt(turned * Vec3::Y);
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
            // A free hand's trigger pulled at a lamp it touches switches it
            // (and doesn't point the laser).
            if hand.select && !pulled && self.holds[i].is_none() {
                let points = reaching(hand);
                let near = |low: Vec3, high: Vec3| points.iter().any(|&p| (low - p).max(p - high).max(Vec3::ZERO).length() < REACH);
                if let Some(target) = self.targets(scene).into_iter().find(|t| near(t.low, t.high)) {
                    input.touched.push(target.id);
                    input.busy[i] = true;
                }
            }
        }
        input.worn = self.worn;
        input
    }

    /// What can be clicked with the laser or the mouse, or touched: the
    /// lamps; and the cameras, where they are.
    pub fn targets(&self, scene: &Scene) -> Vec<Target> {
        let lamps = scene.lamps.iter().enumerate().map(|(i, lamp)| Target { id: TargetId::Lamp(i), low: lamp.bounds.0, high: lamp.bounds.1 });
        let cameras = scene.props.iter().enumerate().filter(|(_, p)| p.kind == PropKind::Camera).map(|(i, prop)| {
            let (low, high) = prop.bounds;
            let corners = (0..8).map(|c| self.poses[i].transform_point3(Vec3::new([low.x, high.x][c & 1], [low.y, high.y][c >> 1 & 1], [low.z, high.z][c >> 2])));
            let (low, high) = corners.fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), p| (l.min(p), h.max(p)));
            Target { id: TargetId::Camera(i), low, high }
        });
        lamps.chain(cameras).collect()
    }

    /// Where prop `i` is now.
    pub fn pose(&self, i: usize) -> Mat4 {
        self.poses[i]
    }

    /// Camera `camera` took `picture`: it comes out of its slot, pushing
    /// out the one before, which falls; the oldest of too many goes.
    pub fn add_print(&mut self, scene: &Scene, camera: usize, picture: Option<glow::Texture>, now: Instant) {
        for i in 0..self.prints.len() {
            if self.prints[i].camera == Some(camera) {
                self.prints[i].pose = self.print_pose(scene, i, now);
                self.prints[i].camera = None;
                self.drop_print(i);
            }
        }
        if self.prints.len() >= PRINTS {
            // (Not one held: those keep their index.)
            if let Some(oldest) = (0..self.prints.len()).find(|&i| !self.holds.iter().any(|h| matches!(h, Some(Hold::Print { print, .. }) if *print == i))) {
                let gone = self.prints.remove(oldest);
                self.stale.extend(gone.picture);
                for hold in self.holds.iter_mut().flatten() {
                    if let Hold::Print { print, .. } = hold
                        && *print > oldest
                    {
                        *print -= 1;
                    }
                }
            }
        }
        self.prints.push(Print { pose: Mat4::IDENTITY, camera: Some(camera), taken: now, picture, body: None });
    }

    /// Where picture `i` is at `now`: coming out of its camera's slot, or
    /// where it was put.
    fn print_pose(&self, scene: &Scene, i: usize, now: Instant) -> Mat4 {
        let print = &self.prints[i];
        let Some(camera) = print.camera else { return print.pose };
        let slot = scene.props[camera].slot.unwrap_or(Mat4::IDENTITY);
        let t = (now.saturating_duration_since(print.taken).as_secs_f32() / EJECT.as_secs_f32()).clamp(0.0, 1.0);
        let out = t * t * (3.0 - 2.0 * t) * 0.85;
        // Lying flat, its top first, out along the slot's -Z.
        self.poses[camera] * slot * Mat4::from_translation(Vec3::new(0.0, 0.0, PRINT.y / 2.0 - out * PRINT.y))
    }

    /// Picture `i`, out of its camera, falls.
    fn drop_print(&mut self, i: usize) {
        self.prints[i].body = Some(Body::new(Self::print_shape(), self.prints[i].pose, Vec3::ZERO, Vec3::ZERO));
    }

    /// What `hand` takes with its grip or its controller's tip: the stick
    /// first, then the nearest prop or picture it is in or by.
    fn take(&mut self, scene: &Scene, hand: &Hand, now: Instant) -> Option<Hold> {
        let points = reaching(hand);
        let mut stick: Option<(f32, Hold)> = None;
        let mut best: Option<(f32, Hold)> = None;
        for (i, prop) in scene.props.iter().enumerate().filter(|(_, p)| self.shown(p)) {
            let pose = self.poses[i];
            for &point in &points {
                if prop.parts.iter().any(|p| p.role == PartRole::Stick) {
                    let frame = pose * prop.stick;
                    let knob = frame.transform_point3(self.tilt_matrix(i).transform_point3(prop.knob));
                    let distance = point.distance(knob);
                    if distance < KNOB_REACH && stick.is_none_or(|(d, _)| distance < d) {
                        stick = Some((distance, Hold::Stick { prop: i, neutral: self.stick_turn(scene, i, hand) }));
                    }
                }
                let local = pose.inverse().transform_point3(point);
                let (low, high) = prop.bounds;
                let outside = (low - local).max(local - high).max(Vec3::ZERO).length();
                if outside < REACH && best.is_none_or(|(d, _)| outside < d) {
                    best = Some((outside, Hold::Body { prop: i, offset: hand.grip.inverse() * pose }));
                }
            }
        }
        let half = Vec3::new(PRINT.x / 2.0, 0.003, PRINT.y / 2.0);
        for i in 0..self.prints.len() {
            let pose = self.print_pose(scene, i, now);
            for &point in &points {
                let local = pose.inverse().transform_point3(point);
                let outside = (local.abs() - half).max(Vec3::ZERO).length();
                if outside < REACH && best.is_none_or(|(d, _)| outside < d) {
                    best = Some((outside, Hold::Print { print: i, offset: hand.grip.inverse() * pose }));
                }
            }
        }
        let hold = stick.or(best).map(|(_, hold)| hold);
        match hold {
            Some(Hold::Print { print, .. }) => {
                // Out of the camera (or the air), into the hand.
                self.prints[print].pose = self.print_pose(scene, print, now);
                self.prints[print].camera = None;
                self.prints[print].body = None;
            }
            Some(Hold::Body { prop, .. }) => {
                self.falling[prop] = None;
                // What it held up falls.
                let (low, high) = self.world_box(self.poses[prop], scene.props[prop].solid);
                self.wake(scene, prop, low - Vec3::splat(NEAR), high + Vec3::splat(NEAR));
            }
            _ => {}
        }
        hold
    }

    /// The way `hand` is turned in the frame of prop `prop`'s stick.
    fn stick_turn(&self, scene: &Scene, prop: usize, hand: &Hand) -> Quat {
        let (_, frame, _) = (self.poses[prop] * scene.props[prop].stick).to_scale_rotation_translation();
        let (_, grip, _) = hand.grip.to_scale_rotation_translation();
        (frame.inverse() * grip).normalize()
    }

    /// The box around `bounds` at `pose`, in the scene.
    fn world_box(&self, pose: Mat4, (low, high): (Vec3, Vec3)) -> (Vec3, Vec3) {
        let corners = (0..8).map(|c| pose.transform_point3(Vec3::new([low.x, high.x][c & 1], [low.y, high.y][c >> 1 & 1], [low.z, high.z][c >> 2])));
        corners.fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), p| (l.min(p), h.max(p)))
    }

    fn held(&self, prop: usize) -> bool {
        self.holds.iter().any(|h| matches!(h, Some(Hold::Body { prop: p, .. } | Hold::Stick { prop: p, .. }) if *p == prop))
    }

    fn print_held(&self, print: usize) -> bool {
        self.holds.iter().any(|h| matches!(h, Some(Hold::Print { print: p, .. }) if *p == print))
    }

    fn print_shape() -> Shape {
        let half = Vec3::new(PRINT.x / 2.0, PRINT_THICKNESS / 2.0, PRINT.y / 2.0);
        Shape::of_box(-half, half)
    }

    /// What lies still (not held, nor prop `taken`) in the box from `low`
    /// to `high` falls, should nothing be under it any more.
    fn wake(&mut self, scene: &Scene, taken: usize, low: Vec3, high: Vec3) {
        let overlaps = |(l, h): (Vec3, Vec3)| l.cmple(high).all() && h.cmpge(low).all();
        for i in 0..scene.props.len() {
            if i != taken && self.falling[i].is_none() && !self.held(i) && self.shown(&scene.props[i]) {
                let solid = scene.props[i].solid;
                if overlaps(self.world_box(self.poses[i], solid)) {
                    self.falling[i] = Some(Body::new(Shape::of_box(solid.0, solid.1), self.poses[i], Vec3::ZERO, Vec3::ZERO));
                }
            }
        }
        for i in 0..self.prints.len() {
            let print = &self.prints[i];
            if print.camera.is_none() && print.body.is_none() && !self.print_held(i) {
                let shape = Self::print_shape();
                if overlaps(self.world_box(print.pose, (shape.middle - shape.half, shape.middle + shape.half))) {
                    self.prints[i].body = Some(Body::new(shape, print.pose, Vec3::ZERO, Vec3::ZERO));
                }
            }
        }
    }

    /// How fast hand `hand` moved, of late, what it holds at `offset` in
    /// its frame (`middle` in that thing's frame).
    fn throw(&self, hand: usize, offset: Mat4, middle: Vec3) -> (Vec3, Vec3) {
        let moves = &self.moves[hand];
        let (Some(&(t0, from)), Some(&(t1, to))) = (moves.first(), moves.last()) else { return (Vec3::ZERO, Vec3::ZERO) };
        physics::motion(from * offset, to * offset, middle, t1.saturating_duration_since(t0).as_secs_f32())
    }

    /// The boxes of the props shown but `except`, in the way of what falls.
    fn obstacles(&self, scene: &Scene, except: Option<usize>) -> Vec<Obstacle> {
        let shown = scene.props.iter().enumerate().filter(|&(i, p)| Some(i) != except && self.shown(p));
        shown.map(|(i, p)| Obstacle { pose: self.poses[i], low: p.solid.0, high: p.solid.1 }).collect()
    }

    /// What falls, simulated up to `now`.
    fn fall(&mut self, scene: &Scene, now: Instant) {
        let since = self.simulated.replace(now).map_or(0.0, |at| now.saturating_duration_since(at).as_secs_f32());
        let falling = self.falling.iter().any(Option::is_some) || self.prints.iter().any(|p| p.body.is_some());
        if !falling || since <= 0.0 {
            return;
        }
        let steps = (since.min(physics::MAX_LAG) / physics::STEP).ceil() as usize;
        for i in 0..self.falling.len() {
            let Some(mut body) = self.falling[i] else { continue };
            let obstacles = self.obstacles(scene, Some(i));
            let mut moved = Moved::Moving;
            for _ in 0..steps {
                moved = body.step(&self.colliders, &obstacles, physics::STEP);
                if moved != Moved::Moving {
                    break;
                }
            }
            match moved {
                // Lost, it is back where it started.
                Moved::Lost => self.poses[i] = scene.props[i].pose,
                _ => self.poses[i] = body.pose(),
            }
            self.falling[i] = (moved == Moved::Moving).then_some(body);
        }
        let obstacles = self.obstacles(scene, None);
        for print in &mut self.prints {
            let Some(mut body) = print.body else { continue };
            let mut moved = Moved::Moving;
            for _ in 0..steps {
                moved = body.step(&self.colliders, &obstacles, physics::STEP);
                if moved != Moved::Moving {
                    break;
                }
            }
            if moved != Moved::Lost {
                print.pose = body.pose();
            }
            print.body = (moved == Moved::Moving).then_some(body);
        }
    }

    /// Hand `i` lets go: a prop or a picture falls, thrown as the hand
    /// moved; a stick springs back.
    fn release(&mut self, scene: &Scene, i: usize) {
        match self.holds[i].take() {
            Some(Hold::Body { prop, .. })
                if scene.props[prop].kind == PropKind::Vfx1 && self.on_head(self.poses[prop]) =>
            {
                self.worn = true;
                self.falling[prop] = None;
            }
            Some(Hold::Body { prop, offset }) => {
                let shape = Shape::of_box(scene.props[prop].solid.0, scene.props[prop].solid.1);
                let (velocity, spin) = self.throw(i, offset, shape.middle);
                self.falling[prop] = Some(Body::new(shape, self.poses[prop], velocity, spin));
            }
            Some(Hold::Stick { prop, .. }) => self.tilts[prop] = Vec3::Y,
            Some(Hold::Print { print, offset }) => {
                let (velocity, spin) = self.throw(i, offset, Vec3::ZERO);
                self.prints[print].body = Some(Body::new(Self::print_shape(), self.prints[print].pose, velocity, spin));
            }
            None => {}
        }
    }

    fn tilt_matrix(&self, prop: usize) -> Mat4 {
        Mat4::from_quat(Quat::from_rotation_arc(Vec3::Y, self.tilts[prop]))
    }

    /// The props' parts to draw, the manual's pages rendered as they come.
    pub fn draws(&mut self, gl: &glow::Context, gpu: &Gpu, scene: &Scene, now: Instant) -> Vec<PropDraw> {
        self.booklet.receive(gl, gpu);
        for texture in self.stale.drain(..) {
            // SAFETY: see `GlScreen`.
            unsafe { gl.delete_texture(texture) };
        }
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
                draws.push(PropDraw::Part { prop: i, part, model, page });
            }
        }
        for i in 0..self.prints.len() {
            let print = &self.prints[i];
            let developing = now.saturating_duration_since(print.taken).saturating_sub(EJECT);
            let develop = developing.as_secs_f32() / DEVELOP.as_secs_f32();
            draws.push(PropDraw::Print { model: self.print_pose(scene, i, now), picture: print.picture, develop });
        }
        draws
    }

    /// Whether prop `prop` is left out of what is drawn.
    #[cfg(test)]
    fn draws_nothing_of(&self, scene: &Scene, prop: usize) -> bool {
        !self.shown(&scene.props[prop])
    }

    /// Delete the pages' textures, with `gl` current.
    pub fn delete(mut self, gl: &glow::Context) {
        self.booklet.forget(gl);
        for texture in self.stale.drain(..).chain(self.prints.drain(..).filter_map(|p| p.picture)) {
            // SAFETY: see `GlScreen`.
            unsafe { gl.delete_texture(texture) };
        }
    }
}

/// Where a hand takes things: its grip, and the tip of the controller
/// drawn for it (see `controls`), which is where it points from.
fn reaching(hand: &Hand) -> [Vec3; 2] {
    [hand.grip.transform_point3(Vec3::ZERO), hand.aim.transform_point3(Vec3::ZERO)]
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
    use super::*;

    fn hand_at(p: Vec3, squeeze: bool) -> Hand {
        let grip = Mat4::from_translation(p);
        Hand { aim: grip, grip, squeeze, ..Hand::default() }
    }

    fn tracking(right: Hand) -> Tracking {
        Tracking { head: None, hands: [None, Some(right)], ..Tracking::default() }
    }

    /// `hand` turned about where it is by `turn`.
    fn turned(mut hand: Hand, turn: Quat) -> Hand {
        let at = hand.grip.transform_point3(Vec3::ZERO);
        let about = |m: Mat4| Mat4::from_translation(at) * Mat4::from_quat(turn) * Mat4::from_translation(-at) * m;
        hand.grip = about(hand.grip);
        hand.aim = about(hand.aim);
        hand
    }

    /// Simulated from `now` for `seconds`, the hands as `tracking` has them.
    fn run(props: &mut Props, room: &Scene, tracking: &Tracking, now: Instant, seconds: f32) -> Instant {
        let mut at = now;
        for _ in 0..(seconds * 90.0) as usize {
            at += Duration::from_secs_f32(1.0 / 90.0);
            props.update(room, tracking, at);
        }
        at
    }

    #[test]
    fn the_joystick_is_taken_by_its_stick_and_tilts_as_the_hand_does() {
        let room = Scene::test_room();
        let mut props = Props::new(&room);
        let now = Instant::now();
        let j = room.props.iter().position(|p| p.kind == PropKind::Joystick).unwrap();
        let prop = &room.props[j];
        let knob = (prop.pose * prop.stick).transform_point3(prop.knob);
        // Taken by a hand turned some way: that is upright.
        let hand = turned(hand_at(knob, true), Quat::from_rotation_x(0.4) * Quat::from_rotation_y(0.7));
        let input = props.update(&room, &tracking(hand), now);
        assert!(input.busy[1]);
        let pad = input.pad.unwrap();
        assert!(pad.axes[0].abs() < 1e-3 && pad.axes[1].abs() < 1e-3, "{:?}", pad.axes);
        // Moved, not turned, it stays upright.
        let mut moved = hand;
        moved.grip = Mat4::from_translation(Vec3::new(0.08, 0.0, -0.08)) * moved.grip;
        let pad = props.update(&room, &tracking(moved), now).pad.unwrap();
        assert!(pad.axes[0].abs() < 1e-3 && pad.axes[1].abs() < 1e-3, "{:?}", pad.axes);
        // Turned about the stick, too.
        let pad = props.update(&room, &tracking(turned(hand, Quat::from_rotation_y(0.5))), now).pad.unwrap();
        assert!(pad.axes[0].abs() < 1e-3 && pad.axes[1].abs() < 1e-3, "{:?}", pad.axes);
        // Tipped away (its top to -Z) and rolled right: up and right, at
        // full tilt.
        let tipped = turned(hand, Quat::from_rotation_z(-0.3) * Quat::from_rotation_x(-0.3));
        let input = props.update(&room, &tracking(tipped), now);
        let pad = input.pad.unwrap();
        assert!(pad.axes[0] > 0.85 && pad.axes[1] < -0.85, "{:?}", pad.axes);
        let knob = (prop.pose * prop.stick).transform_point3(prop.knob);
        // Let go, it stands up again and isn't the joystick any more.
        let input = props.update(&room, &tracking(hand_at(knob, false)), now);
        assert!(input.pad.is_none() && !input.busy[1]);
        assert_eq!(props.tilts[j], Vec3::Y);
    }

    #[test]
    fn the_controllers_tip_takes_things_too() {
        let room = Scene::test_room();
        let mut props = Props::new(&room);
        let now = Instant::now();
        let j = room.props.iter().position(|p| p.kind == PropKind::Joystick).unwrap();
        let prop = &room.props[j];
        let knob = (prop.pose * prop.stick).transform_point3(prop.knob);
        // The grip well behind the knob, the tip on it.
        let mut hand = hand_at(knob + Vec3::new(0.0, 0.0, 0.2), true);
        hand.aim = Mat4::from_translation(knob);
        let input = props.update(&room, &tracking(hand), now);
        let pad = input.pad.unwrap();
        assert!(pad.axes[0].abs() < 1e-3 && pad.axes[1].abs() < 1e-3, "{:?}", pad.axes);
        // Turning the hand tilts it.
        let pad = props.update(&room, &tracking(turned(hand, Quat::from_rotation_x(-0.4))), now).pad.unwrap();
        assert!(pad.axes[0].abs() < 1e-3 && pad.axes[1] < -0.85, "{:?}", pad.axes);
    }

    #[test]
    fn props_let_go_fall_onto_what_is_below() {
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
        // (Held still a while: not thrown.)
        let now = run(&mut props, &room, &tracking(hand), now, 0.2);
        hand.squeeze = false;
        let later = run(&mut props, &room, &tracking(hand), now, 3.0);
        assert!(props.falling[j].is_none(), "lies still");
        let (_, rotation, at) = props.poses[j].to_scale_rotation_translation();
        assert!((at.y - 0.72).abs() < 0.005, "on the desk: {:?}", at);
        assert!((at.x - (base.x - 0.4)).abs() < 0.08);
        assert!((rotation * Vec3::Y).dot(Vec3::Y) > 0.99, "upright");
        // Thrown off the desk, it is on the floor.
        let at = props.poses[j].transform_point3(Vec3::new(0.0, 0.02, 0.0));
        let now = later;
        props.update(&room, &tracking(hand_at(at, true)), now);
        let throw = Vec3::new(0.0, 0.1, 2.0) / 90.0;
        let mut at_frame = now;
        for k in 1..=4 {
            at_frame += Duration::from_secs_f32(1.0 / 90.0);
            props.update(&room, &tracking(hand_at(at + throw * k as f32, true)), at_frame);
        }
        run(&mut props, &room, &tracking(hand_at(at + throw * 4.0, false)), at_frame, 4.0);
        let landed = props.poses[j].transform_point3(Vec3::ZERO);
        assert!(landed.y < 0.1, "on the floor: {:?}", landed);
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
    fn a_lamp_is_switched_by_touching_it() {
        let mut room = Scene::test_room();
        let mut lamp = super::super::scene::Lamp::test(false);
        lamp.bounds = (Vec3::new(0.5, 0.72, -0.6), Vec3::new(0.6, 1.0, -0.5));
        room.lamps.push(lamp);
        let mut props = Props::new(&room);
        let now = Instant::now();
        let mut hand = hand_at(Vec3::new(0.55, 1.02, -0.55), false);
        assert!(props.update(&room, &tracking(hand), now).touched.is_empty());
        hand.select = true;
        let input = props.update(&room, &tracking(hand), now);
        assert_eq!(input.touched, [TargetId::Lamp(0)]);
        assert!(input.busy[1]);
        assert!(props.update(&room, &tracking(hand), now).touched.is_empty(), "once a pull");
    }

    #[test]
    fn the_camera_takes_pictures_that_come_out_and_are_taken() {
        let room = Scene::test_room();
        let mut props = Props::new(&room);
        let now = Instant::now();
        let c = room.props.iter().position(|p| p.kind == PropKind::Camera).unwrap();
        let body = room.props[c].pose.transform_point3(Vec3::new(0.0, 0.04, 0.0));
        let mut hand = hand_at(body, true);
        props.update(&room, &tracking(hand), now);
        hand.select = true;
        assert_eq!(props.update(&room, &tracking(hand), now).shots, [c]);
        assert!(props.update(&room, &tracking(hand), now).shots.is_empty(), "once a pull");
        props.add_print(&room, c, None, now);
        // It comes out of the slot, along the lens's way.
        let slot = (room.props[c].pose * room.props[c].slot.unwrap()).transform_point3(Vec3::ZERO);
        let lens = (room.props[c].pose * room.props[c].lens.unwrap()).transform_vector3(Vec3::NEG_Z);
        let out = props.print_pose(&room, 0, now + EJECT).transform_point3(Vec3::ZERO);
        assert!((out - slot).dot(lens) > PRINT.y * 0.2, "{:?} {:?}", out, slot);
        // Put the camera down; another hand takes the picture by its middle.
        props.update(&room, &tracking(hand_at(body, false)), now);
        let later = now + EJECT * 2;
        let at = props.print_pose(&room, 0, later).transform_point3(Vec3::ZERO);
        props.update(&room, &tracking(hand_at(at, true)), later);
        assert!(matches!(props.holds[1], Some(Hold::Print { print: 0, .. })));
        // Let go over the desk, it lies on it.
        let over = Vec3::new(0.0, 1.0, -0.5);
        let later = run(&mut props, &room, &tracking(hand_at(over, true)), later, 0.2);
        run(&mut props, &room, &tracking(hand_at(over, false)), later, 3.0);
        assert!((props.prints[0].pose.transform_point3(Vec3::ZERO).y - 0.72).abs() < 0.003);
    }

    #[test]
    fn the_vfx1_is_put_on_at_the_face_and_taken_off_at_the_head() {
        let room = Scene::test_room();
        let mut props = Props::new(&room);
        let v = room.props.iter().position(|p| p.kind == PropKind::Vfx1).unwrap();
        let body = room.props[v].pose.transform_point3(Vec3::new(0.0, 0.05, -0.07));
        let head = Mat4::from_translation(Vec3::new(0.0, 1.2, 0.0));
        let with_head = |hand: Hand| Tracking { head: Some(head), ..tracking(hand) };
        let now = Instant::now();
        // Not there, it can't be taken.
        props.update(&room, &with_head(hand_at(body, true)), now);
        assert!(props.holds[1].is_none());
        props.set_vfx1(true);
        props.update(&room, &with_head(hand_at(body, false)), now);
        props.update(&room, &with_head(hand_at(body, true)), now);
        assert!(matches!(props.holds[1], Some(Hold::Body { prop, .. }) if prop == v));
        // Brought to the face facing the way the head does (it stands on
        // the desk facing the viewer), and let go: worn.
        let Some(Hold::Body { offset, .. }) = props.holds[1] else { unreachable!() };
        let worn = head * Mat4::from_translation(-VFX1_EYES);
        let mut hand = hand_at(Vec3::ZERO, true);
        hand.grip = worn * offset.inverse();
        hand.aim = hand.grip;
        props.update(&room, &with_head(hand), now);
        hand.squeeze = false;
        assert!(props.update(&room, &with_head(hand), now).worn);
        assert!(props.draws_nothing_of(&room, v));
        // Gripped far from the head, nothing; at the head, it comes off
        // into the hand.
        let far = Vec3::new(0.0, 0.8, -0.5);
        assert!(props.update(&room, &with_head(hand_at(far, true)), now).worn);
        props.update(&room, &with_head(hand_at(far, false)), now);
        let at_head = Vec3::new(0.1, 1.25, 0.0);
        let input = props.update(&room, &with_head(hand_at(at_head, true)), now);
        assert!(!input.worn && input.busy[1]);
        // Let go away from the face, it falls.
        let away = hand_at(Vec3::new(0.3, 1.0, -0.5), true);
        props.update(&room, &with_head(away), now);
        let input = props.update(&room, &with_head(Hand { squeeze: false, ..away }), now);
        assert!(!input.worn && props.falling[v].is_some());
    }

    #[test]
    fn the_tilt_stops_at_full_deflection() {
        let tilt = clamp_tilt(Vec3::new(1.0, 0.1, 0.0));
        assert!((tilt.y.acos() - MAX_TILT).abs() < 1e-4);
        assert_eq!(clamp_tilt(Vec3::Y), Vec3::Y);
    }
}
