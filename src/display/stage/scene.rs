//! What the 3D view shows, as plain data: the meshes with their materials
//! and pictures, the lights, the screen the emulated picture goes on,
//! where the viewer starts and the things on the desk to pick up (the
//! game's manual and a joystick). It comes from a glTF file exported from
//! Blender, or is the built-in test room.

use super::keycaps;
use glam::{Mat3, Mat4, Vec2, Vec3};
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// Triangles in the scene's (world) coordinates, all of one material.
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub material: usize,
}

/// A picture, four bytes a pixel, rows from the top.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A picture of the scene's and whether it repeats beyond 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureRef {
    pub image: usize,
    pub repeat: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shading {
    /// Lit by the lights, the sky and the screen's glow.
    Lit,
    /// Its colour as it is: lighting baked into the textures
    /// (KHR_materials_unlit).
    Unlit,
    /// The emulated picture.
    Screen,
    /// The test room's floor: lit, with a faint grid.
    Floor,
}

/// A light on the PC's front, lit as the emulated PC's would be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Led {
    Power,
    /// Lit while the CPU runs fast (`Leds::turbo`).
    Turbo,
    /// Lit while a hard disk is read or written.
    Hdd,
    /// Lit while a floppy disk is.
    Floppy,
    /// The keyboard's lock lights.
    NumLock,
    CapsLock,
    ScrollLock,
}

impl Led {
    /// A light by its mesh's name (`led_hdd`) or custom property's value
    /// (`hdd`).
    fn parse(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        let name = name.strip_prefix("led_").unwrap_or(&name);
        // Blender adds .001 to copies' names.
        let name = name.split('.').next().unwrap_or(name);
        match name {
            "power" => Some(Led::Power),
            "turbo" => Some(Led::Turbo),
            "hdd" | "disk" | "harddisk" => Some(Led::Hdd),
            "floppy" | "fdd" => Some(Led::Floppy),
            "numlock" | "num" => Some(Led::NumLock),
            "capslock" | "caps" => Some(Led::CapsLock),
            "scrolllock" | "scroll" => Some(Led::ScrollLock),
            _ => None,
        }
    }
}

pub use rust_dos::vr::Leds;

/// Whether the light is lit.
pub fn lit(leds: Leds, led: Led) -> bool {
    match led {
        Led::Power => leds.power,
        Led::Turbo => leds.turbo,
        Led::Hdd => leds.hdd,
        Led::Floppy => leds.floppy,
        Led::NumLock => leds.num,
        Led::CapsLock => leds.caps,
        Led::ScrollLock => leds.scroll,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Alpha {
    Opaque,
    /// Cut away below the cutoff.
    Mask(f32),
    Blend,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// Linear RGBA.
    pub base_color: [f32; 4],
    pub base_texture: Option<TextureRef>,
    /// Linear RGB.
    pub emissive: [f32; 3],
    pub emissive_texture: Option<TextureRef>,
    pub shading: Shading,
    pub double_sided: bool,
    pub alpha: Alpha,
    /// The PC's light this is, glowing with `emissive` only while lit.
    pub led: Option<Led>,
    /// The lamp (`Scene::lamps`) this glows with, only while it is on.
    pub lamp: Option<usize>,
    /// A window's glass, which the rain runs down.
    pub window: bool,
}

impl Material {
    /// Whether it casts shadows: not if it is seen through, or is a light
    /// or a backdrop (black, and glowing), or a lamp's glowing bulb (which
    /// would be in the way of its own light).
    pub fn casts_shadow(&self) -> bool {
        let black = self.base_color[..3] == [0.0; 3];
        let glows = self.emissive != [0.0; 3] || self.emissive_texture.is_some();
        self.alpha != Alpha::Blend && self.shading != Shading::Screen && !(black && glows) && self.lamp.is_none()
    }

    pub(super) fn plain(color: [f32; 3], shading: Shading) -> Self {
        Material {
            base_color: [color[0], color[1], color[2], 1.0],
            base_texture: None,
            emissive: [0.0; 3],
            emissive_texture: None,
            shading,
            double_sided: false,
            alpha: Alpha::Opaque,
            led: None,
            lamp: None,
            window: false,
        }
    }

    /// A light of the PC's, glowing `color` when lit.
    pub(super) fn led(led: Led, color: [f32; 3]) -> Self {
        Material {
            emissive: color.map(|c| c * 1.6),
            led: Some(led),
            ..Material::plain(color.map(|c| c * 0.15), Shading::Lit)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LightKind {
    /// Light from `direction`'s opposite, the sun's.
    Directional,
    Point,
    /// A point's light in a cone around `direction`, between the cosines
    /// of the inner and outer angle.
    Spot { inner_cos: f32, outer_cos: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    pub kind: LightKind,
    /// Linear RGB, times the intensity.
    pub color: Vec3,
    pub position: Vec3,
    /// Where the light shines to.
    pub direction: Vec3,
    /// Whether what is in its way casts a shadow (the light's
    /// `rustdos_shadow` custom property).
    pub shadow: bool,
}

/// A lamp of the scene's (a node with `rustdos_lamp`), switched on and off
/// with a hand or a click: its lights, and its meshes that glow.
#[derive(Clone, Debug, PartialEq)]
pub struct Lamp {
    pub name: String,
    /// On as the scene starts (`rustdos_lamp_on`).
    pub on: bool,
    /// Its lights, by their index in `Scene::lights`.
    pub lights: Vec<usize>,
    /// The box around its meshes, where it is switched.
    pub bounds: (Vec3, Vec3),
}

impl Lamp {
    #[cfg(test)]
    pub fn test(on: bool) -> Self {
        Lamp { name: "lamp".into(), on, lights: Vec::new(), bounds: (Vec3::ZERO, Vec3::ONE) }
    }
}

/// The surface the emulated picture is shown on.
#[derive(Clone, Debug, Default)]
pub struct Screen {
    /// Its triangles' corners and their places in the picture, (0, 0) its
    /// top left.
    pub triangles: Vec<[(Vec3, Vec2); 3]>,
    /// The middle, the way it faces and its width and height, in metres:
    /// for the light it gives the room.
    pub center: Vec3,
    pub normal: Vec3,
    pub size: Vec2,
    /// The picture's left to right, on the surface.
    pub right: Vec3,
    /// The scene asks for the picture to be stretched over the whole
    /// screen (`rustdos_screen_fit` = `stretch`), not kept in its shape.
    pub stretch: bool,
}

impl Screen {
    /// Width by height, as the picture is stretched over it.
    pub fn aspect(&self) -> f32 {
        if self.size.y > 0.0 { self.size.x / self.size.y } else { 4.0 / 3.0 }
    }

    /// The picture's bottom to top, on the surface.
    pub fn up(&self) -> Vec3 {
        self.normal.cross(self.right).normalize_or(Vec3::Y)
    }

    /// The picture's top left corner and its whole width and height going
    /// right and down, as vectors.
    pub fn frame(&self) -> (Vec3, Vec3, Vec3) {
        let (across, down) = (self.right * self.size.x, -self.up() * self.size.y);
        (self.center - (across + down) / 2.0, across, down)
    }

    /// Work out the middle, the facing and the size from the triangles.
    fn measure(&mut self) {
        let (mut area, mut center, mut normal, mut size) = (0.0, Vec3::ZERO, Vec3::ZERO, Vec2::ZERO);
        let mut right = Vec3::ZERO;
        for &[(p0, t0), (p1, t1), (p2, t2)] in &self.triangles {
            let cross = (p1 - p0).cross(p2 - p0);
            let a = cross.length() * 0.5;
            if a <= 0.0 {
                continue;
            }
            area += a;
            center += (p0 + p1 + p2) / 3.0 * a;
            normal += cross;
            // How far the surface goes for a whole picture across and down:
            // the derivatives of the position by the picture's u and v.
            let (e1, e2, d1, d2) = (p1 - p0, p2 - p0, t1 - t0, t2 - t0);
            let det = d1.x * d2.y - d2.x * d1.y;
            if det.abs() > 1e-12 {
                let dpdu = (e1 * d2.y - e2 * d1.y) / det;
                let dpdv = (e2 * d1.x - e1 * d2.x) / det;
                size += Vec2::new(dpdu.length(), dpdv.length()) * a;
                right += dpdu * a;
            }
        }
        if area > 0.0 {
            self.center = center / area;
            self.normal = normal.normalize_or_zero();
            self.size = size / area;
            self.right = right.normalize_or(Vec3::X);
        }
    }
}

/// Where the viewer's eyes start and which way they look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spawn {
    pub position: Vec3,
    /// Radians: 0 looks along -Z, more turns left.
    pub yaw: f32,
    pub pitch: f32,
}

impl Spawn {
    fn from_matrix(world: Mat4) -> Self {
        let position = world.transform_point3(Vec3::ZERO);
        let forward = world.transform_vector3(Vec3::NEG_Z).normalize_or(Vec3::NEG_Z);
        Spawn { position, yaw: (-forward.x).atan2(-forward.z), pitch: forward.y.clamp(-1.0, 1.0).asin() }
    }
}

/// Something to pick up (see `props`): the game's manual, a joystick, a
/// polaroid camera, a VFX1 headset to put on (`[vfx1]`), the PC's
/// keyboard, to type on, or anything else of the scene's that is only
/// moved about (`item`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropKind {
    Manual,
    Joystick,
    Camera,
    Vfx1,
    Keyboard,
    Item,
}

impl PropKind {
    /// A prop by its node's name (`manual`, `joystick.001`) or custom
    /// property's value.
    fn parse(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        match name.split('.').next().unwrap_or(&name) {
            "manual" => Some(PropKind::Manual),
            "joystick" => Some(PropKind::Joystick),
            "camera" => Some(PropKind::Camera),
            "vfx1" => Some(PropKind::Vfx1),
            "keyboard" => Some(PropKind::Keyboard),
            "item" => Some(PropKind::Item),
            _ => None,
        }
    }

    /// Whether Rust-DOS has a model of its own for it, for an empty.
    fn built_in(self) -> bool {
        self != PropKind::Item
    }
}

/// Where the eyes are in the frame of Rust-DOS's own VFX1, worn: behind
/// its visor.
pub const VFX1_EYES: Vec3 = Vec3::new(0.0, 0.06, -0.005);

/// A hand takes a prop by its box, which is at least this thick, so that
/// a pencil or a sheet of paper can be taken too.
const THINNEST: f32 = 0.02;

/// What a part of a prop is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartRole {
    Body,
    /// The manual's open pages, which show its pages' pictures: UV (0, 0)
    /// is a page's top left.
    PageLeft,
    PageRight,
    /// A page being turned, both its sides, hinged at the spine: in the
    /// frame of `Prop::hinge`, lying along +X.
    SheetFront,
    SheetBack,
    /// The joystick's stick, in the frame of `Prop::stick`.
    Stick,
    /// The keyboard's caps: a row's together, drawn while none of them is
    /// down, and each key's (`Prop::keys`) on its own.
    KeyRow(usize),
    Key(usize),
}

pub struct PropPart {
    pub role: PartRole,
    pub mesh: Mesh,
}

/// A prop as the scene has it, where it starts.
pub struct Prop {
    pub kind: PropKind,
    /// Its frame in the scene: where it stands, +Y up and -Z away from
    /// the player. Without scale.
    pub pose: Mat4,
    /// Its parts, in its frame (but the stick's and the sheet's).
    pub parts: Vec<PropPart>,
    /// The stick's frame in the prop's: at the pivot, +Y along the stick.
    pub stick: Mat4,
    /// Where a hand holds the stick, in the stick's frame.
    pub knob: Vec3,
    /// The spine, where a page turns about the Z axis, in the prop's frame.
    pub hinge: Vec3,
    /// The box around it in its frame (but the stick's): where a hand
    /// takes it.
    pub bounds: (Vec3, Vec3),
    /// That box as thin as the prop is: what it is to what it falls on.
    pub solid: (Vec3, Vec3),
    /// A camera's lens (looking along its -Z) and the slot its pictures
    /// come out of (along its -Z, +Y the picture's up), in its frame.
    pub lens: Option<Mat4>,
    pub slot: Option<Mat4>,
    /// The keyboard's keys.
    pub keys: Vec<keycaps::Key>,
}

impl Prop {
    fn new(kind: PropKind, pose: Mat4) -> Self {
        Prop {
            kind,
            pose,
            parts: Vec::new(),
            stick: Mat4::IDENTITY,
            knob: Vec3::ZERO,
            hinge: Vec3::ZERO,
            bounds: (Vec3::ZERO, Vec3::ZERO),
            solid: (Vec3::ZERO, Vec3::ZERO),
            lens: None,
            slot: None,
            keys: Vec::new(),
        }
    }

    fn push(&mut self, role: PartRole, mesh: Mesh) {
        self.parts.push(PropPart { role, mesh });
    }

    fn has(&self, role: PartRole) -> bool {
        self.parts.iter().any(|p| p.role == role)
    }

    /// The box around the parts of `role` in their own frame.
    fn bounds_of(&self, role: impl Fn(PartRole) -> bool) -> Option<(Vec3, Vec3)> {
        let points = self.parts.iter().filter(|p| role(p.role)).flat_map(|p| p.mesh.vertices.iter().map(|v| Vec3::from(v.position)));
        points.fold(None, |b, p| Some(b.map_or((p, p), |(low, high): (Vec3, Vec3)| (low.min(p), high.max(p)))))
    }

    /// Work out where it is held, and the sheet a page turns on.
    fn finish(&mut self) {
        use PartRole::{Body, KeyRow, PageLeft, PageRight};
        let parts = |r| matches!(r, Body | PageLeft | PageRight | KeyRow(_));
        let (mut low, mut high) = self.bounds_of(parts).unwrap_or_default();
        if let Some((_, top)) = self.bounds_of(|r| r == PartRole::Stick) {
            // The knob: the top of the stick.
            self.knob = Vec3::new(0.0, (top.y - 0.015).max(0.0), 0.0);
            let base = self.stick.transform_point3(Vec3::ZERO);
            low = low.min(base);
            high = high.max(base);
        }
        self.solid = (low, high);
        // Thickened across, and upwards from what it stands on.
        let thin = (Vec3::splat(THINNEST) - (high - low)).max(Vec3::ZERO);
        let across = Vec3::new(thin.x, 0.0, thin.z) / 2.0;
        self.bounds = (low - across, high + across + Vec3::Y * thin.y);
        if self.kind == PropKind::Camera {
            // Unless the scene says, the lens in the middle of its front
            // (-Z), the slot low under it.
            let (low, high) = self.bounds;
            let front = Vec3::new((low.x + high.x) / 2.0, 0.0, low.z - 0.002);
            self.lens.get_or_insert(Mat4::from_translation(front + Vec3::Y * (low.y + (high.y - low.y) * 0.6)));
            self.slot.get_or_insert(Mat4::from_translation(front + Vec3::Y * (low.y + (high.y - low.y) * 0.12)));
        }
        if let Some((a, b)) = self.bounds_of(|r| r == PartRole::PageRight)
            && !self.has(PartRole::SheetFront)
        {
            // A sheet the size of the right page, a little above it.
            let width = b.x - a.x;
            self.hinge = Vec3::new(a.x, b.y + 0.0006, 0.0);
            let material = self.parts.iter().find(|p| p.role == PartRole::PageRight).map_or(0, |p| p.mesh.material);
            for (role, up) in [(PartRole::SheetFront, 1.0), (PartRole::SheetBack, -1.0)] {
                self.push(role, sheet(width, (a.z, b.z), up, material));
            }
        }
    }
}

pub struct Scene {
    pub meshes: Vec<Mesh>,
    pub materials: Vec<Material>,
    pub images: Vec<Image>,
    pub lights: Vec<Light>,
    /// Light from everywhere, linear RGB.
    pub ambient: Vec3,
    pub screen: Screen,
    pub spawn: Spawn,
    /// Whether the sky is drawn around the scene, which then fades into it
    /// with distance (`haze` a metre).
    pub sky: bool,
    pub haze: f32,
    /// The light that is the sun (or the moon) and goes with the time of
    /// day, by its index in `lights`.
    pub sun_light: Option<usize>,
    /// Where the scene's -Z points, in degrees from north (90 east): the
    /// sun sets that way at 270 (`rustdos_heading`).
    pub heading: f32,
    /// The lamps to switch on and off.
    pub lamps: Vec<Lamp>,
    /// The inside of the room (a box, `rustdos_indoor`), where it doesn't
    /// rain or snow; the middles of its windows' glass.
    pub indoor: Option<(Vec3, Vec3)>,
    pub windows: Vec<Vec3>,
    /// What the lit colours are multiplied by.
    pub exposure: f32,
    /// Where the left and right channels' sound comes from: the scene's
    /// `speaker_left` and `speaker_right`, else the screen's sides.
    pub speakers: [Vec3; 2],
    /// Whether the lights cast shadows (`rustdos_shadows`) and the light
    /// bouncing around the room is worked out (`rustdos_gi`).
    pub shadows: bool,
    pub gi: bool,
    /// Whether nearby things darken the light from all around
    /// (`rustdos_ao`).
    pub ao: bool,
    /// How brightly the screen lights the room, 1 as bright as the picture
    /// is (`rustdos_screen_glow`).
    pub screen_glow: f32,
    /// A hash of where the scene came from, for keeping what is worked out
    /// for it (`gi::Probes::key`).
    pub source: u64,
    /// The light bounced around it, once the first view of it works it
    /// out: shared by the window's and the headset's.
    pub probes: std::sync::OnceLock<Option<super::gi::Probes>>,
    /// The things to pick up, where they start: not in `meshes`, nor in
    /// the lighting worked out for the room, since they move.
    pub props: Vec<Prop>,
}

/// How far from the screen and the viewer's start the lighting is worked
/// out in detail, in metres.
const FOCUS: f32 = 12.0;

/// How brightly the screen lights the room, unless the scene says.
const SCREEN_GLOW: f32 = 5.0;

/// The test room's desk: the middle of its top's top, and the top's size.
const TEST_DESK: Vec3 = Vec3::new(0.0, 0.72, -0.55);
const TEST_DESK_TOP: Vec3 = Vec3::new(1.2, 0.03, 0.6);
/// Where the manual, the joystick, the camera, the VFX1 and the keyboard
/// stand on it, and how far they are turned (the camera's lens and the
/// VFX1's front to the viewer).
const TEST_PROPS: [(PropKind, Vec3, f32); 5] = [
    (PropKind::Manual, Vec3::new(-0.42, 0.72, -0.55), 0.0),
    (PropKind::Joystick, Vec3::new(0.24, 0.72, -0.53), 0.0),
    (PropKind::Camera, Vec3::new(-0.5, 0.72, -0.74), std::f32::consts::PI),
    (PropKind::Vfx1, Vec3::new(0.45, 0.72, -0.61), std::f32::consts::PI),
    (PropKind::Keyboard, Vec3::new(0.0, 0.72, -0.345), 0.0),
];

/// The sunset sky's sun: low, ahead and to the left.
fn sunset_sun() -> Vec3 {
    Vec3::new(-0.45, 0.1, -1.0).normalize()
}

/// Where a scene's -Z points unless it says, in degrees from north: west,
/// so that the sun sets ahead of the viewer, as the sunset of before.
const HEADING: f32 = 270.0;

/// How fast the scene fades into the sky with distance, unless it says.
const HAZE: f32 = 0.02;

/// The light of the sunset sky: its sun, orange, and the blue of the rest.
fn sunset_lights() -> (Light, Vec3) {
    let sun = Light {
        kind: LightKind::Directional,
        color: Vec3::new(1.0, 0.5, 0.22) * 1.6,
        position: Vec3::ZERO,
        direction: -sunset_sun(),
        shadow: true,
    };
    (sun, Vec3::new(0.035, 0.045, 0.09))
}

/// The sRGB colour #rrggbb in linear light.
pub(super) fn srgb(hex: u32) -> [f32; 3] {
    let channel = |shift: u32| {
        let c = ((hex >> shift) & 0xFF) as f32 / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    [channel(16), channel(8), channel(0)]
}

impl Scene {
    /// The screen floating in front of the viewer over a dark floor, under
    /// a dark blue sky with an orange sunset.
    pub fn test_room() -> Self {
        let materials = vec![
            Material::plain(srgb(0x1a1c20), Shading::Floor),
            Material::plain([1.0; 3], Shading::Screen),
            Material::plain(srgb(0x101114), Shading::Lit),
            // The PC: its case, its front's slots and its lights.
            Material::plain(srgb(0x2c2d31), Shading::Lit),
            Material::plain(srgb(0x0b0b0d), Shading::Lit),
            Material::led(Led::Power, [0.1, 1.0, 0.15]),
            Material::led(Led::Turbo, [1.0, 0.65, 0.05]),
            Material::led(Led::Hdd, [1.0, 0.12, 0.05]),
            Material::led(Led::Floppy, [0.1, 1.0, 0.15]),
            // The desk.
            Material::plain(srgb(0x6b4a30), Shading::Lit),
        ];
        let mut meshes = Vec::new();
        // The floor, so big that its edges are lost in the haze.
        let up = [0.0, 1.0, 0.0];
        let corner = |x: f32, z: f32| Vertex { position: [x, 0.0, z], normal: up, uv: [x, z] };
        let edge = 250.0;
        meshes.push(Mesh {
            vertices: vec![corner(-edge, -edge), corner(-edge, edge), corner(edge, edge), corner(edge, -edge)],
            indices: vec![0, 1, 2, 0, 2, 3],
            material: 0,
        });
        // The screen: 4:3, 1.6 m wide, 2.5 m ahead, its middle a little
        // above the eyes.
        let (center, w, h) = (Vec3::new(0.0, 1.4, -2.5), 1.6, 1.2);
        let at = |x: f32, y: f32, u: f32, v: f32| Vertex {
            position: (center + Vec3::new(x, y, 0.0)).to_array(),
            normal: [0.0, 0.0, 1.0],
            uv: [u, v],
        };
        meshes.push(Mesh {
            vertices: vec![
                at(-w / 2.0, h / 2.0, 0.0, 0.0),
                at(-w / 2.0, -h / 2.0, 0.0, 1.0),
                at(w / 2.0, -h / 2.0, 1.0, 1.0),
                at(w / 2.0, h / 2.0, 1.0, 0.0),
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            material: 1,
        });
        // A thin dark slab behind it, its frame.
        let bezel = center + Vec3::new(0.0, 0.0, -0.035);
        meshes.push(cuboid(bezel, Vec3::new(w + 0.08, h + 0.08, 0.06), 2));
        // A small tower floating beside it, its front to the viewer: two
        // drive bays, a floppy drive with its light, and the power, turbo
        // and hard disk lights.
        let tower = Vec3::new(1.2, 1.0, -2.4);
        let (tw, th, td) = (0.2, 0.44, 0.42);
        meshes.push(cuboid(tower, Vec3::new(tw, th, td), 3));
        let front = tower.z + td / 2.0;
        let on_front = |x: f32, y: f32| Vec3::new(tower.x + x, tower.y + y, front);
        for (y, height) in [(0.17, 0.045), (0.115, 0.045)] {
            meshes.push(cuboid(on_front(0.0, y), Vec3::new(0.16, height, 0.004), 4));
        }
        meshes.push(cuboid(on_front(0.0, 0.06), Vec3::new(0.11, 0.028, 0.004), 4));
        meshes.push(cuboid(on_front(0.045, 0.052), Vec3::new(0.008, 0.005, 0.008), 8));
        for (i, material) in [5, 6, 7].into_iter().enumerate() {
            let x = -0.05 + i as f32 * 0.025;
            meshes.push(cuboid(on_front(x, 0.0), Vec3::new(0.008, 0.008, 0.008), material));
        }
        // A desk in front of the viewer, its top at `DESK`'s height, for the
        // manual and the joystick: the top and four legs.
        let (desk, top) = (TEST_DESK, TEST_DESK_TOP);
        meshes.push(cuboid(desk - Vec3::Y * top.y / 2.0, top, 9));
        for (x, z) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let leg = Vec3::new(desk.x + x * (top.x / 2.0 - 0.04), (desk.y - top.y) / 2.0, desk.z + z * (top.z / 2.0 - 0.04));
            meshes.push(cuboid(leg, Vec3::new(0.04, desk.y - top.y, 0.04), 9));
        }
        let mut scene = Scene {
            meshes,
            materials,
            images: Vec::new(),
            lights: Vec::new(),
            ambient: Vec3::ZERO,
            screen: Screen::default(),
            spawn: Spawn { position: Vec3::new(0.0, 1.2, 0.0), yaw: 0.0, pitch: 0.0 },
            sky: true,
            haze: HAZE,
            sun_light: Some(0),
            heading: HEADING,
            lamps: Vec::new(),
            indoor: None,
            windows: Vec::new(),
            exposure: 1.0,
            speakers: [Vec3::ZERO; 2],
            shadows: true,
            gi: true,
            ao: true,
            screen_glow: SCREEN_GLOW,
            source: hash(b"the test room"),
            probes: std::sync::OnceLock::new(),
            props: Vec::new(),
        };
        let (sun, ambient) = sunset_lights();
        scene.lights.push(sun);
        scene.ambient = ambient;
        scene.collect_screen();
        scene.speakers = scene.screen_speakers();
        for (kind, at, yaw) in TEST_PROPS {
            let prop = scene.built_in_prop(kind, Mat4::from_translation(at) * Mat4::from_rotation_y(yaw));
            scene.props.push(prop);
        }
        scene
    }

    fn add_material(&mut self, material: Material) -> usize {
        self.materials.push(material);
        self.materials.len() - 1
    }

    /// Rust-DOS's own model of a prop, standing at `pose`.
    fn built_in_prop(&mut self, kind: PropKind, pose: Mat4) -> Prop {
        let mut prop = Prop::new(kind, pose);
        match kind {
            PropKind::Manual => {
                // An A5 booklet, open: its cover, the pages' edges, and the
                // two pages showing.
                let cover = self.add_material(Material::plain(srgb(0x23325e), Shading::Lit));
                let edges = self.add_material(Material::plain(srgb(0xdedbd0), Shading::Lit));
                let paper = self.add_material(Material::plain(srgb(0xf4f2ec), Shading::Lit));
                let (w, d) = (0.148, 0.21);
                prop.push(PartRole::Body, cuboid(Vec3::new(0.0, 0.0015, 0.0), Vec3::new(2.0 * w + 0.008, 0.003, d + 0.006), cover));
                prop.push(PartRole::Body, cuboid(Vec3::new(0.0, 0.0055, 0.0), Vec3::new(2.0 * w, 0.005, d), edges));
                for (role, x0) in [(PartRole::PageLeft, -w), (PartRole::PageRight, 0.0)] {
                    prop.push(role, page(Vec3::new(x0, 0.0081, -d / 2.0), w, d, paper));
                }
            }
            PropKind::Joystick => {
                // A desktop joystick: a square base with a fire button,
                // and a stick with a red knob on it.
                let black = self.add_material(Material::plain(srgb(0x18181b), Shading::Lit));
                let grey = self.add_material(Material::plain(srgb(0x55575c), Shading::Lit));
                let red = self.add_material(Material::plain(srgb(0xb3161b), Shading::Lit));
                let (side, height) = (0.12, 0.04);
                prop.push(PartRole::Body, cuboid(Vec3::new(0.0, height / 2.0, 0.0), Vec3::new(side, height, side), black));
                prop.push(PartRole::Body, cylinder(Vec3::new(0.0, height, 0.0), 0.022, 0.006, 16, grey));
                prop.push(PartRole::Body, cylinder(Vec3::new(-0.04, height, -0.04), 0.009, 0.006, 12, red));
                prop.stick = Mat4::from_translation(Vec3::new(0.0, height + 0.004, 0.0));
                prop.push(PartRole::Stick, cylinder(Vec3::ZERO, 0.009, 0.13, 12, black));
                prop.push(PartRole::Stick, cylinder(Vec3::new(0.0, 0.11, 0.0), 0.017, 0.04, 16, red));
            }
            PropKind::Camera => {
                // An instant camera: a box, its white front with a rainbow
                // stripe, the lens, the viewfinder on top and a red button.
                let body = self.add_material(Material::plain(srgb(0x2b3238), Shading::Lit));
                let white = self.add_material(Material::plain(srgb(0xe8e6df), Shading::Lit));
                let glass = self.add_material(Material::plain(srgb(0x0c0e12), Shading::Lit));
                let red = self.add_material(Material::plain(srgb(0xc8231c), Shading::Lit));
                let (w, h, d) = (0.11, 0.085, 0.1);
                prop.push(PartRole::Body, cuboid(Vec3::new(0.0, h / 2.0, 0.0), Vec3::new(w, h, d), body));
                prop.push(PartRole::Body, cuboid(Vec3::new(0.0, h * 0.42, -d / 2.0 - 0.003), Vec3::new(w, h * 0.84, 0.006), white));
                for (i, hex) in [0xd9302c, 0xf2a71b, 0x3a9c4a, 0x2a6fb5].into_iter().enumerate() {
                    let stripe = self.add_material(Material::plain(srgb(hex), Shading::Lit));
                    let x = -0.012 + i as f32 * 0.008;
                    prop.push(PartRole::Body, cuboid(Vec3::new(x, h * 0.3, -d / 2.0 - 0.0065), Vec3::new(0.006, h * 0.5, 0.001), stripe));
                }
                prop.push(PartRole::Body, cuboid(Vec3::new(0.0, h * 0.62, -d / 2.0 - 0.012), Vec3::new(0.046, 0.046, 0.016), body));
                prop.push(PartRole::Body, cuboid(Vec3::new(0.0, h * 0.62, -d / 2.0 - 0.0205), Vec3::new(0.03, 0.03, 0.002), glass));
                prop.push(PartRole::Body, cuboid(Vec3::new(-0.03, h + 0.012, 0.01), Vec3::new(0.035, 0.024, 0.07), body));
                prop.push(PartRole::Body, cylinder(Vec3::new(0.038, h, -0.025), 0.007, 0.006, 12, red));
                prop.lens = Some(Mat4::from_translation(Vec3::new(0.0, h * 0.62, -d / 2.0 - 0.022)));
                prop.slot = Some(Mat4::from_translation(Vec3::new(0.0, 0.012, -d / 2.0 - 0.007)));
            }
            PropKind::Vfx1 => self.vfx1_model(&mut prop),
            PropKind::Keyboard => {
                let model = keycaps::model();
                let image = self.images.len();
                self.images.push(model.image);
                let first = self.materials.len();
                for mut material in model.materials {
                    if let Some(texture) = &mut material.base_texture {
                        texture.image = image;
                    }
                    self.materials.push(material);
                }
                for (role, mut mesh) in model.parts {
                    mesh.material += first;
                    prop.push(role, mesh);
                }
                prop.keys = model.keys;
            }
            // The scene's own only.
            PropKind::Item => {}
        }
        prop.finish();
        prop
    }

    /// Forte's VFX1, all matt black: the visor before the eyes, wide and
    /// squared off at the front with the name across it in yellow, which
    /// rises to its hinge above the forehead; the broad shell from there
    /// over the head and down the back, padded inside; a hoop round the
    /// back of the head; and the round headphones on it, with Forte's
    /// mark. The eyes are at `VFX1_EYES` in its frame, worn.
    fn vfx1_model(&mut self, prop: &mut Prop) {
        let shell = self.add_material(Material::plain(srgb(0x26272a), Shading::Lit));
        let foam = self.add_material(Material::plain(srgb(0x0b0b0c), Shading::Lit));
        let leather = self.add_material(Material::plain(srgb(0x151517), Shading::Lit));
        let screw = self.add_material(Material::plain(srgb(0x4a4c50), Shading::Lit));
        let yellow = self.add_material(Material::plain(srgb(0xd9d23c), Shading::Lit));

        // The visor from the side, (z, y) round from the bottom of its
        // back, and how far it reaches either way across there.
        let profile = [
            (-0.015, 0.0, 0.092),
            (-0.135, 0.0, 0.1),
            (-0.122, 0.095, 0.1),
            (-0.075, 0.122, 0.092),
            (-0.022, 0.152, 0.075),
            (0.006, 0.148, 0.075),
        ];
        prop.push(PartRole::Body, loft(&profile, shell));
        for side in [-1.0, 1.0] {
            let at = Mat4::from_translation(Vec3::new(0.075 * side, 0.133, -0.02))
                * Mat4::from_rotation_z(-side * std::f32::consts::FRAC_PI_2);
            prop.push(PartRole::Body, moved(cylinder(Vec3::ZERO, 0.006, 0.004, 12, screw), at));
        }

        // The shell, from the hinge over the head and down the back, and
        // the padding inside it.
        let middle = Vec3::new(0.0, 0.06, 0.09);
        let over = |radius: f32, (from, to): (f32, f32)| -> Vec<(Vec3, Vec3, Vec3)> {
            (0..=16)
                .map(|i| {
                    let a = (from + (to - from) * i as f32 / 16.0).to_radians();
                    let out = Vec3::new(0.0, a.cos(), -a.sin());
                    (middle + out * radius, out, Vec3::X)
                })
                .collect()
        };
        prop.push(PartRole::Body, band(&over(0.13, (55.0, -110.0)), Vec2::new(0.05, 0.011), shell));
        prop.push(PartRole::Body, band(&over(0.112, (50.0, -105.0)), Vec2::new(0.042, 0.007), foam));

        // The hoop round the back of the head, from ear to ear.
        let (rx, rz, y) = (0.098, 0.125, 0.05);
        let hoop: Vec<_> = (0..=20)
            .map(|i| {
                let a = -0.35 + (std::f32::consts::PI + 0.7) * i as f32 / 20.0;
                let at = Vec3::new(rx * a.cos(), y, middle.z + rz * a.sin());
                let out = Vec3::new(a.cos() / rx, 0.0, a.sin() / rz).normalize();
                (at, out, Vec3::Y)
            })
            .collect();
        prop.push(PartRole::Body, band(&hoop, Vec2::new(0.02, 0.008), shell));

        // The headphones on it, each a cushion, a cup and a cap, with
        // Forte's "f" in a ring on the cap.
        for side in [-1.0f32, 1.0] {
            let outward = |x: f32| {
                Mat4::from_translation(Vec3::new(side * x, 0.048, 0.1))
                    * Mat4::from_rotation_z(-side * std::f32::consts::FRAC_PI_2)
            };
            let parts = [
                (0.084, 0.044, 0.02, leather),
                (0.104, 0.046, 0.03, shell),
                (0.134, 0.038, 0.008, shell),
                (0.142, 0.024, 0.0006, yellow),
                (0.1426, 0.021, 0.0006, shell),
            ];
            for (x, radius, depth, material) in parts {
                prop.push(PartRole::Body, moved(cylinder(Vec3::ZERO, radius, depth, 24, material), outward(x)));
            }
            // The cap's face, as seen from beside it: X across, Y up.
            let out = Vec3::X * side;
            let middle = outward(0.1432).transform_point3(Vec3::ZERO);
            let face = Mat4::from_cols(Vec3::Y.cross(out).extend(0.0), Vec3::Y.extend(0.0), out.extend(0.0), middle.extend(1.0));
            let strokes = [[(0.6, 1.0), (0.4, 0.0)], [(0.6, 1.0), (0.9, 0.9)], [(0.25, 0.55), (0.75, 0.55)]];
            for [a, b] in strokes {
                let at = |(u, v): (f32, f32)| Vec2::new(-0.015 + 0.03 * u, -0.017 + 0.034 * v);
                prop.push(PartRole::Body, stroke(face, at(a), at(b), 0.0022, yellow));
            }
        }

        // The name across the visor's front, from its bottom middle,
        // across to the viewer's right and up the front.
        let up = Vec3::new(0.0, 0.095, 0.013).normalize();
        let out = Vec3::NEG_X.cross(up);
        let face = Mat4::from_cols(Vec3::NEG_X.extend(0.0), up.extend(0.0), out.extend(0.0), Vec3::new(0.0, 0.0, -0.135).extend(1.0));
        let mut write = |text: &str, (width, height): (f32, f32), gap: f32, (base, thick): (f32, f32), dots: bool| {
            let advance = |c: char| if c == '1' { width * 0.55 } else { width };
            let dot = if dots { gap * 2.0 + thick } else { 0.0 };
            let total: f32 = text.chars().map(|c| advance(c) + gap + dot).sum::<f32>() - gap - dot;
            let mut left = -total / 2.0;
            for (i, c) in text.chars().enumerate() {
                for &[a, b] in glyph(c) {
                    // Leaning forwards, as Forte's name does.
                    let at = |(u, v): (f32, f32)| Vec2::new(left + u * advance(c) + 0.2 * v * height, base + v * height);
                    prop.push(PartRole::Body, stroke(face, at(a), at(b), thick, yellow));
                }
                left += advance(c) + gap;
                if dots && i + 1 < text.len() {
                    let at = Vec2::new(left, base + height / 2.0);
                    prop.push(PartRole::Body, stroke(face, at, at, thick, yellow));
                    left += dot;
                }
            }
        };
        write("VFX1", (0.021, 0.03), 0.004, (0.045, 0.0045), false);
        write("HEADGEAR", (0.0065, 0.009), 0.0032, (0.022, 0.0014), true);
    }

    /// Rust-DOS's own manual and joystick, for a scene without them:
    /// standing on what is below the viewer's hands, the manual to the
    /// left and the joystick to the right, facing the viewer; in the air
    /// there if nothing is.
    fn add_missing_props(&mut self) {
        let spawn = self.spawn;
        let turn = Mat4::from_rotation_y(spawn.yaw);
        let props = [
            (PropKind::Manual, -0.25, -0.45),
            (PropKind::Joystick, 0.25, -0.45),
            (PropKind::Camera, -0.55, -0.45),
            (PropKind::Vfx1, 0.5, -0.45),
            (PropKind::Keyboard, 0.0, -0.3),
        ];
        for (kind, side, ahead) in props {
            if self.props.iter().any(|p| p.kind == kind) {
                continue;
            }
            let over = spawn.position + turn.transform_vector3(Vec3::new(side, 0.0, ahead));
            let floor = self.surface_below(over, 1.2).unwrap_or(over - Vec3::Y * 0.45);
            // (The camera's lens and the VFX1's front to the viewer.)
            let facing = match kind {
                PropKind::Camera | PropKind::Vfx1 => Mat4::from_rotation_y(std::f32::consts::PI),
                _ => Mat4::IDENTITY,
            };
            let prop = self.built_in_prop(kind, Mat4::from_translation(floor) * turn * facing);
            self.props.push(prop);
        }
    }

    /// The first opaque surface below `point`, at most `reach` down.
    pub fn surface_below(&self, point: Vec3, reach: f32) -> Option<Vec3> {
        let mut nearest: Option<f32> = None;
        for mesh in self.meshes.iter().filter(|m| self.materials[m.material].casts_shadow()) {
            for tri in mesh.indices.as_chunks::<3>().0 {
                let corners = [tri[0], tri[1], tri[2]].map(|i| Vec3::from(mesh.vertices[i as usize].position));
                if let Some((t, _, _)) = super::pick::hit_triangle(point, Vec3::NEG_Y, corners)
                    && t <= reach
                    && nearest.is_none_or(|n| t < n)
                {
                    nearest = Some(t);
                }
            }
        }
        nearest.map(|t| point - Vec3::Y * t)
    }

    /// The box around the meshes, cut down to `FOCUS` around the screen
    /// and the viewer's start: what shadows and bounced light cover.
    pub fn focus_bounds(&self) -> (Vec3, Vec3) {
        let (mut low, mut high) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for mesh in self.meshes.iter().filter(|m| self.materials[m.material].casts_shadow()) {
            for v in &mesh.vertices {
                let p = Vec3::from(v.position);
                low = low.min(p);
                high = high.max(p);
            }
        }
        let middle = (self.screen.center + self.spawn.position) / 2.0;
        let reach = FOCUS + (self.screen.center - self.spawn.position).length() / 2.0;
        let (low, high) = (low.max(middle - reach), high.min(middle + reach));
        if low.cmple(high).all() { (low, high) } else { (middle - 1.0, middle + 1.0) }
    }

    /// The screen's left and right sides, where its sound comes from if
    /// the scene has no speakers.
    fn screen_speakers(&self) -> [Vec3; 2] {
        let half = self.screen.right * self.screen.size.x / 2.0;
        [self.screen.center - half, self.screen.center + half]
    }

    /// The screen's triangles, from the meshes of the screen's material.
    fn collect_screen(&mut self) {
        let mut screen = Screen::default();
        for mesh in &self.meshes {
            if self.materials[mesh.material].shading != Shading::Screen {
                continue;
            }
            for tri in mesh.indices.as_chunks::<3>().0 {
                let corner = |i: u32| {
                    let v = &mesh.vertices[i as usize];
                    (Vec3::from(v.position), Vec2::from(v.uv))
                };
                screen.triangles.push([corner(tri[0]), corner(tri[1]), corner(tri[2])]);
            }
        }
        screen.measure();
        self.screen = screen;
    }

    /// A scene from a glTF file (.glb, or .gltf with its buffers and
    /// pictures beside it or in it).
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        let base = path.parent().unwrap_or(Path::new("."));
        Self::from_gltf(&bytes, base).map_err(|e| format!("{}: {}", path.display(), e))
    }

    fn from_gltf(bytes: &[u8], base: &Path) -> Result<Self, String> {
        let gltf = gltf::Gltf::from_slice(bytes).map_err(|e| e.to_string())?;
        let mut buffers = Vec::new();
        for buffer in gltf.buffers() {
            let data = match buffer.source() {
                gltf::buffer::Source::Bin => gltf.blob.clone().ok_or("the binary chunk is missing")?,
                gltf::buffer::Source::Uri(uri) => read_uri(uri, base)?,
            };
            if data.len() < buffer.length() {
                return Err(format!("buffer {} is shorter than it says", buffer.index()));
            }
            buffers.push(data);
        }
        let mut images = Vec::new();
        for image in gltf.images() {
            let data = match image.source() {
                gltf::image::Source::View { view, .. } => {
                    let buffer = &buffers[view.buffer().index()];
                    buffer.get(view.offset()..view.offset() + view.length()).ok_or("an image is outside its buffer")?.to_vec()
                }
                gltf::image::Source::Uri { uri, .. } => read_uri(uri, base)?,
            };
            images.push(decode_image(&data).map_err(|e| format!("image {}: {}", image.index(), e))?);
        }

        let texture = |info: Option<gltf::texture::Info>| {
            info.map(|info| {
                let texture = info.texture();
                let repeat = texture.sampler().wrap_s() != gltf::texture::WrappingMode::ClampToEdge;
                TextureRef { image: texture.source().index(), repeat }
            })
        };
        let mut materials: Vec<Material> = gltf
            .materials()
            .map(|m| {
                let pbr = m.pbr_metallic_roughness();
                Material {
                    base_color: pbr.base_color_factor(),
                    base_texture: texture(pbr.base_color_texture()),
                    // Blender's emission strength above 1 comes as
                    // KHR_materials_emissive_strength.
                    emissive: m.emissive_factor().map(|c| c * m.emissive_strength().unwrap_or(1.0)),
                    emissive_texture: texture(m.emissive_texture()),
                    shading: if m.unlit() { Shading::Unlit } else { Shading::Lit },
                    double_sided: m.double_sided(),
                    alpha: match m.alpha_mode() {
                        gltf::material::AlphaMode::Opaque => Alpha::Opaque,
                        gltf::material::AlphaMode::Mask => Alpha::Mask(m.alpha_cutoff().unwrap_or(0.5)),
                        gltf::material::AlphaMode::Blend => Alpha::Blend,
                    },
                    led: None,
                    lamp: None,
                    window: false,
                }
            })
            .collect();
        // For primitives without a material, and the screen's.
        let default_material = materials.len();
        materials.push(Material::plain([0.8; 3], Shading::Lit));
        let screen_material = materials.len();
        materials.push(Material { double_sided: true, ..Material::plain([1.0; 3], Shading::Screen) });

        let scene = gltf.default_scene().or_else(|| gltf.scenes().next()).ok_or("there is no scene")?;
        let mut loaded = Scene {
            meshes: Vec::new(),
            materials,
            images,
            lights: Vec::new(),
            ambient: Vec3::ZERO,
            screen: Screen::default(),
            spawn: Spawn { position: Vec3::new(0.0, 1.2, 0.0), yaw: 0.0, pitch: 0.0 },
            sky: extra(scene.extras(), "rustdos_sky").is_none_or(truthy),
            haze: extra(scene.extras(), "rustdos_haze").and_then(|v| v.as_f64()).map_or(HAZE, |h| h.max(0.0) as f32),
            sun_light: None,
            heading: extra(scene.extras(), "rustdos_heading").and_then(|v| v.as_f64()).map_or(HEADING, |h| h as f32),
            lamps: Vec::new(),
            indoor: None,
            windows: Vec::new(),
            exposure: extra(scene.extras(), "rustdos_exposure").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
            speakers: [Vec3::ZERO; 2],
            shadows: extra(scene.extras(), "rustdos_shadows").is_none_or(truthy),
            gi: extra(scene.extras(), "rustdos_gi").is_none_or(truthy),
            ao: extra(scene.extras(), "rustdos_ao").is_none_or(truthy),
            screen_glow: extra(scene.extras(), "rustdos_screen_glow")
                .and_then(|v| v.as_f64())
                .map_or(SCREEN_GLOW, |g| g.max(0.0) as f32),
            source: hash(bytes),
            probes: std::sync::OnceLock::new(),
            props: Vec::new(),
        };
        let mut walk = Walk {
            buffers: &buffers,
            default_material,
            screen_material,
            spawn: None,
            camera: None,
            speakers: [None; 2],
            leds: Vec::new(),
            screen_stretch: false,
            prop_places: Vec::new(),
            lamp: None,
            lamps: Vec::new(),
            lamp_meshes: Vec::new(),
            windows: Vec::new(),
            indoor: None,
        };
        for node in scene.nodes() {
            walk.node(&mut loaded, node, Mat4::IDENTITY, false)?;
        }
        if let Some(spawn) = walk.spawn.or(walk.camera) {
            loaded.spawn = spawn;
        }
        if loaded.meshes.iter().all(|m| m.material != screen_material) {
            return Err("no mesh is the screen: name it \"screen\", or give it the custom property rustdos_screen".into());
        }
        // A scene without lights of its own is lit by the sun, and with a
        // sky the sun lights it too; with lights, the sky (or without one,
        // what fills the room) only adds a little.
        let (sun, ambient) = sunset_lights();
        loaded.ambient = if loaded.lights.is_empty() { ambient } else { ambient * 0.5 };
        if loaded.lights.is_empty() || loaded.sky {
            // First, the light that most wants shadows; the others after.
            loaded.lights.insert(0, sun);
            loaded.lights.truncate(MAX_LIGHTS);
            for lamp in &mut walk.lamps {
                lamp.lights.iter_mut().for_each(|i| *i += 1);
                lamp.lights.retain(|&i| i < MAX_LIGHTS);
            }
            loaded.sun_light = Some(0);
        }
        loaded.lamps = std::mem::take(&mut walk.lamps);
        // A lamp's own copies of the materials that glow, lit or not apart
        // from the rest using them.
        for (mesh, lamp) in std::mem::take(&mut walk.lamp_meshes) {
            let material = &loaded.materials[loaded.meshes[mesh].material];
            if material.emissive == [0.0; 3] && material.emissive_texture.is_none() {
                continue;
            }
            let material = Material { lamp: Some(lamp), ..material.clone() };
            loaded.meshes[mesh].material = loaded.materials.len();
            loaded.materials.push(material);
        }
        for mesh in std::mem::take(&mut walk.windows) {
            let vertices = &loaded.meshes[mesh].vertices;
            let middle = vertices.iter().map(|v| Vec3::from(v.position)).sum::<Vec3>() / vertices.len().max(1) as f32;
            loaded.windows.push(middle);
            let material = Material { window: true, ..loaded.materials[loaded.meshes[mesh].material].clone() };
            loaded.meshes[mesh].material = loaded.materials.len();
            loaded.materials.push(material);
        }
        loaded.indoor = walk.indoor;
        loaded.collect_screen();
        loaded.screen.stretch = walk.screen_stretch;
        let sides = loaded.screen_speakers();
        loaded.speakers = [walk.speakers[0].unwrap_or(sides[0]), walk.speakers[1].unwrap_or(sides[1])];
        // A light's own copy of its material, lit or not apart from the
        // rest using it.
        for (mesh, led) in walk.leds {
            let mut material = loaded.materials[loaded.meshes[mesh].material].clone();
            if material.emissive == [0.0; 3] && material.emissive_texture.is_none() {
                let [r, g, b, _] = material.base_color;
                material.emissive = [r * 1.6, g * 1.6, b * 1.6];
            }
            material.led = Some(led);
            loaded.meshes[mesh].material = loaded.materials.len();
            loaded.materials.push(material);
        }
        // Props placed by an empty of their name: Rust-DOS's model.
        for (kind, pose) in walk.prop_places {
            let prop = loaded.built_in_prop(kind, pose);
            loaded.props.push(prop);
        }
        loaded.add_missing_props();
        Ok(loaded)
    }
}

/// What walking a glTF scene's nodes needs.
struct Walk<'a> {
    buffers: &'a [Vec<u8>],
    default_material: usize,
    screen_material: usize,
    /// The node called `spawn`, and the first camera, where the viewer
    /// starts.
    spawn: Option<Spawn>,
    camera: Option<Spawn>,
    /// The empties called `speaker_left` and `speaker_right`.
    speakers: [Option<Vec3>; 2],
    /// The meshes that are the PC's lights, by their index.
    leds: Vec<(usize, Led)>,
    /// A screen's `rustdos_screen_fit` is `stretch`.
    screen_stretch: bool,
    /// Props that are only an empty: where Rust-DOS's model goes.
    prop_places: Vec<(PropKind, Mat4)>,
    /// The lamp being walked below, the lamps, and the meshes of each.
    lamp: Option<usize>,
    lamps: Vec<Lamp>,
    lamp_meshes: Vec<(usize, usize)>,
    /// The meshes that are a window's glass.
    windows: Vec<usize>,
    /// The inside of the room, from an empty's cube.
    indoor: Option<(Vec3, Vec3)>,
}

/// What a node of a prop is, by its name or `rustdos_prop`.
fn part_role(node: &gltf::Node) -> Option<PartRole> {
    let named = extra(node.extras(), "rustdos_prop").and_then(|v| v.as_str().map(str::to_string)).or_else(|| node.name().map(str::to_string))?;
    let name = named.trim().to_ascii_lowercase();
    match name.split('.').next().unwrap_or(&name) {
        "manual_page_left" => Some(PartRole::PageLeft),
        "manual_page_right" => Some(PartRole::PageRight),
        "joystick_stick" => Some(PartRole::Stick),
        _ => None,
    }
}

/// Where a node is, without its scale.
fn rigid(world: Mat4) -> Mat4 {
    let (_, rotation, position) = world.to_scale_rotation_translation();
    Mat4::from_rotation_translation(rotation, position)
}

/// The lights of a scene are at most this many; the shader has room for
/// no more.
pub const MAX_LIGHTS: usize = 16;

/// A scene's own lights at most: room is left for the sun and a camera's
/// flash.
const SCENE_LIGHTS: usize = MAX_LIGHTS - 2;

impl Walk<'_> {
    fn node(&mut self, scene: &mut Scene, node: gltf::Node, parent: Mat4, in_screen: bool) -> Result<(), String> {
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        let prop = extra(node.extras(), "rustdos_prop")
            .and_then(|v| v.as_str().and_then(PropKind::parse))
            .or_else(|| node.name().and_then(PropKind::parse));
        if let Some(kind) = prop.filter(|_| !in_screen) {
            return self.prop(scene, node, world, kind);
        }
        let named = |name: Option<&str>, wanted: &str| name.is_some_and(|n| n.eq_ignore_ascii_case(wanted));
        if named(node.name(), "spawn") {
            self.spawn = Some(Spawn::from_matrix(world));
        }
        for (i, side) in ["speaker_left", "speaker_right"].into_iter().enumerate() {
            if node.name().is_some_and(|n| n.split('.').next().unwrap_or(n).eq_ignore_ascii_case(side)) {
                self.speakers[i] = Some(world.transform_point3(Vec3::ZERO));
            }
        }
        if node.camera().is_some() && self.camera.is_none() {
            self.camera = Some(Spawn::from_matrix(world));
        }
        if named(node.name(), "indoor") || extra(node.extras(), "rustdos_indoor").is_some_and(truthy) {
            // Blender's cube empty, from -1 to 1.
            let corners = (0..8).map(|i| world.transform_point3(Vec3::new([-1.0, 1.0][i & 1], [-1.0, 1.0][i >> 1 & 1], [-1.0, 1.0][i >> 2])));
            self.indoor = Some(corners.fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), c| (l.min(c), h.max(c))));
        }
        // A lamp and all below it.
        let outer_lamp = self.lamp;
        if self.lamp.is_none()
            && let Some(value) = extra(node.extras(), "rustdos_lamp").filter(|v| v.is_string() || truthy(v.clone()))
        {
            let name = value.as_str().map(str::to_string).or_else(|| node.name().map(str::to_string)).unwrap_or_else(|| "lamp".into());
            let on = extra(node.extras(), "rustdos_lamp_on").is_some_and(truthy);
            self.lamp = Some(self.lamps.len());
            self.lamps.push(Lamp { name, on, lights: Vec::new(), bounds: (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)) });
        }
        if let Some(light) = node.light()
            && scene.lights.len() < SCENE_LIGHTS
        {
            if let Some(lamp) = self.lamp {
                self.lamps[lamp].lights.push(scene.lights.len());
            }
            let color = Vec3::from(light.color()) * light.intensity();
            let kind = match light.kind() {
                gltf::khr_lights_punctual::Kind::Directional => LightKind::Directional,
                gltf::khr_lights_punctual::Kind::Point => LightKind::Point,
                gltf::khr_lights_punctual::Kind::Spot { inner_cone_angle, outer_cone_angle } => {
                    LightKind::Spot { inner_cos: inner_cone_angle.cos(), outer_cos: outer_cone_angle.cos() }
                }
            };
            scene.lights.push(Light {
                kind,
                color,
                position: world.transform_point3(Vec3::ZERO),
                direction: world.transform_vector3(Vec3::NEG_Z).normalize_or(Vec3::NEG_Y),
                shadow: extra(node.extras(), "rustdos_shadow").or_else(|| extra(light.extras(), "rustdos_shadow")).is_none_or(truthy),
            });
        }
        let screen = in_screen
            || named(node.name(), "screen")
            || extra(node.extras(), "rustdos_screen").is_some_and(truthy)
            || node.mesh().is_some_and(|m| named(m.name(), "screen") || extra(m.extras(), "rustdos_screen").is_some_and(truthy));
        if screen {
            let fit = extra(node.extras(), "rustdos_screen_fit")
                .or_else(|| node.mesh().and_then(|m| extra(m.extras(), "rustdos_screen_fit")));
            if let Some(fit) = fit.as_ref().and_then(|v| v.as_str()) {
                self.screen_stretch = fit.trim().eq_ignore_ascii_case("stretch");
            }
        }
        let led = extra(node.extras(), "rustdos_led")
            .and_then(|v| v.as_str().and_then(Led::parse))
            .or_else(|| node.name().filter(|n| n.to_ascii_lowercase().starts_with("led_")).and_then(Led::parse))
            .or_else(|| node.mesh().and_then(|m| m.name()).filter(|n| n.to_ascii_lowercase().starts_with("led_")).and_then(Led::parse));
        let window = !screen
            && (extra(node.extras(), "rustdos_window").is_some_and(truthy)
                || node.name().is_some_and(is_window_name)
                || node.mesh().is_some_and(|m| m.name().is_some_and(is_window_name)));
        if let Some(mesh) = node.mesh() {
            for (mut read, primitive) in self.read_mesh(&mesh, world)? {
                read.material = if screen {
                    self.screen_material
                } else {
                    primitive.material().index().unwrap_or(self.default_material)
                };
                if let Some(led) = led.filter(|_| !screen) {
                    self.leds.push((scene.meshes.len(), led));
                }
                if let Some(lamp) = self.lamp {
                    let bounds = &mut self.lamps[lamp].bounds;
                    for v in &read.vertices {
                        bounds.0 = bounds.0.min(Vec3::from(v.position));
                        bounds.1 = bounds.1.max(Vec3::from(v.position));
                    }
                    self.lamp_meshes.push((scene.meshes.len(), lamp));
                }
                if window {
                    self.windows.push(scene.meshes.len());
                }
                scene.meshes.push(read);
            }
        }
        for child in node.children() {
            self.node(scene, child, world, screen)?;
        }
        if outer_lamp.is_none()
            && let Some(lamp) = self.lamp.take()
        {
            // Without meshes, a small box around its lights.
            let bounds = &mut self.lamps[lamp].bounds;
            if bounds.0.x > bounds.1.x {
                for &i in &self.lamps[lamp].lights.clone() {
                    let at = scene.lights[i].position;
                    let bounds = &mut self.lamps[lamp].bounds;
                    bounds.0 = bounds.0.min(at - 0.1);
                    bounds.1 = bounds.1.max(at + 0.1);
                }
            }
        }
        Ok(())
    }

    /// A prop and what is below it: its meshes in its frame, apart from
    /// the room's. Only an empty, it is where Rust-DOS's model goes.
    fn prop(&mut self, scene: &mut Scene, node: gltf::Node, world: Mat4, kind: PropKind) -> Result<(), String> {
        let pose = rigid(world);
        let mut prop = Prop::new(kind, pose);
        self.prop_node(&mut prop, node, world, pose.inverse(), PartRole::Body)?;
        if prop.parts.is_empty() {
            if kind.built_in() {
                self.prop_places.push((kind, pose));
            }
        } else {
            prop.finish();
            scene.props.push(prop);
        }
        Ok(())
    }

    /// A node of a prop, its meshes taken into the frame `to` takes the
    /// scene's coordinates to: the prop's, or below the stick, the
    /// stick's.
    fn prop_node(&mut self, prop: &mut Prop, node: gltf::Node, world: Mat4, mut to: Mat4, mut role: PartRole) -> Result<(), String> {
        let frame = prop.pose.inverse() * rigid(world);
        match node.name().map(|n| n.split('.').next().unwrap_or(n).to_ascii_lowercase()).as_deref() {
            Some("camera_lens") => prop.lens = Some(frame),
            Some("camera_slot") => prop.slot = Some(frame),
            _ => {}
        }
        match part_role(&node) {
            Some(PartRole::Stick) => {
                let pivot = rigid(world);
                prop.stick = prop.pose.inverse() * pivot;
                to = pivot.inverse();
                role = PartRole::Stick;
            }
            Some(part) => role = part,
            None => {}
        }
        if let Some(mesh) = node.mesh() {
            for (mut read, primitive) in self.read_mesh(&mesh, to * world)? {
                read.material = primitive.material().index().unwrap_or(self.default_material);
                prop.push(role, read);
            }
        }
        for child in node.children() {
            let child_world = world * Mat4::from_cols_array_2d(&child.transform().matrix());
            self.prop_node(prop, child, child_world, to, role)?;
        }
        Ok(())
    }

    /// A mesh's triangle primitives, through `world`, with their
    /// primitives (for the material).
    fn read_mesh<'m>(&self, mesh: &gltf::Mesh<'m>, world: Mat4) -> Result<Vec<(Mesh, gltf::Primitive<'m>)>, String> {
        let mut read = Vec::new();
        {
            let normals = Mat3::from_mat4(world).inverse().transpose();
            for primitive in mesh.primitives() {
                if primitive.mode() != gltf::mesh::Mode::Triangles {
                    continue;
                }
                let reader = primitive.reader(|b| self.buffers.get(b.index()).map(Vec::as_slice));
                let Some(positions) = reader.read_positions() else { continue };
                let positions: Vec<Vec3> = positions.map(|p| world.transform_point3(Vec3::from(p))).collect();
                let indices: Vec<u32> = match reader.read_indices() {
                    Some(indices) => indices.into_u32().collect(),
                    None => (0..positions.len() as u32).collect(),
                };
                if indices.iter().any(|&i| i as usize >= positions.len()) {
                    return Err(format!("mesh {} has indices past its vertices", mesh.index()));
                }
                let mut normal: Vec<Vec3> = match reader.read_normals() {
                    Some(n) => n.map(|n| (normals * Vec3::from(n)).normalize_or_zero()).collect(),
                    None => vec![Vec3::ZERO; positions.len()],
                };
                if normal.len() != positions.len() || normal.iter().all(|n| *n == Vec3::ZERO) {
                    normal = smooth_normals(&positions, &indices);
                }
                let uvs: Vec<[f32; 2]> = match reader.read_tex_coords(0) {
                    Some(uv) => uv.into_f32().collect(),
                    None => Vec::new(),
                };
                let vertices = positions
                    .iter()
                    .zip(&normal)
                    .enumerate()
                    .map(|(i, (p, n))| Vertex {
                        position: p.to_array(),
                        normal: n.to_array(),
                        uv: uvs.get(i).copied().unwrap_or([0.0, 0.0]),
                    })
                    .collect();
                read.push((Mesh { vertices, indices, material: self.default_material }, primitive));
            }
        }
        Ok(read)
    }
}

/// Whether a node or mesh is called a window's glass ("Window glass",
/// "window_glass.001").
fn is_window_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase().replace(['_', ' '], "");
    name.starts_with("windowglass") || name.starts_with("windowpane")
}

/// Normals for a mesh that has none: each vertex's faces', by their area.
fn smooth_normals(positions: &[Vec3], indices: &[u32]) -> Vec<Vec3> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for tri in indices.as_chunks::<3>().0 {
        let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| i as usize);
        let n = (positions[b] - positions[a]).cross(positions[c] - positions[a]);
        for i in [a, b, c] {
            normals[i] += n;
        }
    }
    normals.into_iter().map(|n| n.normalize_or(Vec3::Y)).collect()
}

/// A polaroid picture's size, across and down, in metres; and its picture's,
/// square, from the card's top edge this far.
pub const PRINT: Vec2 = Vec2::new(0.088, 0.107);
pub const PRINT_PICTURE: f32 = 0.079;
pub const PRINT_TOP: f32 = 0.007;

/// A polaroid picture lying face up, its top edge away (-Z), its middle at
/// the origin: the white card and the picture on it (UV (0, 0) at its top
/// left), both in material 0.
pub fn print_meshes() -> (Mesh, Mesh) {
    let card = cuboid(Vec3::new(0.0, 0.0005, 0.0), Vec3::new(PRINT.x, 0.001, PRINT.y), 0);
    let corner = Vec3::new(-PRINT_PICTURE / 2.0, 0.00105, -PRINT.y / 2.0 + PRINT_TOP);
    (card, page(corner, PRINT_PICTURE, PRINT_PICTURE, 0))
}

/// A box of `size` around the origin of the frame `at`, in the material.
fn turned(at: Mat4, size: Vec3, material: usize) -> Mesh {
    moved(cuboid(Vec3::ZERO, size, material), at)
}

/// A mesh moved into the frame `at` (turned and moved only).
fn moved(mut mesh: Mesh, at: Mat4) -> Mesh {
    for vertex in &mut mesh.vertices {
        vertex.position = at.transform_point3(Vec3::from(vertex.position)).to_array();
        vertex.normal = at.transform_vector3(Vec3::from(vertex.normal)).to_array();
    }
    mesh
}

/// A flat face with these corners, in order round it, wound and lit to
/// face away from `inside`.
fn add_face(mesh: &mut Mesh, corners: &[Vec3], inside: Vec3) {
    let middle = corners.iter().sum::<Vec3>() / corners.len() as f32;
    let mut normal = Vec3::ZERO;
    for (i, a) in corners.iter().enumerate() {
        normal += (*a - middle).cross(corners[(i + 1) % corners.len()] - middle);
    }
    let away = normal.dot(middle - inside) >= 0.0;
    let normal = if away { normal } else { -normal }.normalize_or(Vec3::Y);
    let base = mesh.vertices.len() as u32;
    for corner in corners {
        mesh.vertices.push(Vertex { position: corner.to_array(), normal: normal.to_array(), uv: [0.0, 0.0] });
    }
    for i in 1..corners.len() as u32 - 1 {
        mesh.indices.extend(if away { [base, base + i, base + i + 1] } else { [base, base + i + 1, base + i] });
    }
}

/// A solid the same either side of X = 0: its outline from the side,
/// convex, as (z, y) and how far it reaches either way across there.
fn loft(profile: &[(f32, f32, f32)], material: usize) -> Mesh {
    let mut mesh = Mesh { vertices: Vec::new(), indices: Vec::new(), material };
    let at = |&(z, y, x): &(f32, f32, f32), side: f32| Vec3::new(x * side, y, z);
    let inside = profile.iter().map(|&(z, y, _)| Vec3::new(0.0, y, z)).sum::<Vec3>() / profile.len() as f32;
    for side in [-1.0, 1.0] {
        let corners: Vec<_> = profile.iter().map(|p| at(p, side)).collect();
        add_face(&mut mesh, &corners, inside);
    }
    for (i, a) in profile.iter().enumerate() {
        let b = &profile[(i + 1) % profile.len()];
        add_face(&mut mesh, &[at(a, -1.0), at(a, 1.0), at(b, 1.0), at(b, -1.0)], inside);
    }
    mesh
}

/// A band along a path of (middle, outwards, across): `half.x` either
/// way across and `half.y` either way outwards, smooth along it, closed
/// at its ends.
fn band(path: &[(Vec3, Vec3, Vec3)], half: Vec2, material: usize) -> Mesh {
    let mut mesh = Mesh { vertices: Vec::new(), indices: Vec::new(), material };
    let corners = |&(middle, out, across): &(Vec3, Vec3, Vec3)| {
        let (out, across) = (out * half.y, across * half.x);
        [middle + out - across, middle + out + across, middle - out + across, middle - out - across]
    };
    for side in 0..4 {
        let base = mesh.vertices.len() as u32;
        for ring in path {
            let c = corners(ring);
            let (a, b) = (c[side], c[(side + 1) % 4]);
            let normal = ((a + b) / 2.0 - ring.0).normalize_or(Vec3::Y);
            for p in [a, b] {
                mesh.vertices.push(Vertex { position: p.to_array(), normal: normal.to_array(), uv: [0.0, 0.0] });
            }
        }
        for i in 0..path.len() as u32 - 1 {
            let [a, b, c, d] = [0, 1, 3, 2].map(|k| base + 2 * i + k);
            let pos = |k: u32| Vec3::from(mesh.vertices[k as usize].position);
            let facing = (pos(b) - pos(a)).cross(pos(c) - pos(a));
            let out = Vec3::from(mesh.vertices[a as usize].normal);
            mesh.indices.extend(if facing.dot(out) >= 0.0 { [a, b, c, a, c, d] } else { [a, c, b, a, d, c] });
        }
    }
    for (end, next) in [(0, 1), (path.len() - 1, path.len() - 2)] {
        add_face(&mut mesh, &corners(&path[end]), path[next].0);
    }
    mesh
}

/// A painted stroke from `a` to `b` on the face `face` (X across, Y up,
/// Z out of it), `thick` wide, standing a little out of it.
fn stroke(face: Mat4, a: Vec2, b: Vec2, thick: f32, material: usize) -> Mesh {
    let along = b - a;
    let middle = (a + b) / 2.0;
    let at = face * Mat4::from_translation(middle.extend(0.0004)) * Mat4::from_rotation_z(along.y.atan2(along.x));
    turned(at, Vec3::new(along.length() + thick, thick, 0.0012), material)
}

/// A capital's strokes in a box from (0, 0) at its bottom left to (1, 1),
/// for the VFX1's name.
fn glyph(c: char) -> &'static [[(f32, f32); 2]] {
    match c {
        'V' => &[[(0.0, 1.0), (0.5, 0.0)], [(0.5, 0.0), (1.0, 1.0)]],
        'F' => &[[(0.0, 0.0), (0.0, 1.0)], [(0.0, 1.0), (1.0, 1.0)], [(0.0, 0.55), (0.75, 0.55)]],
        'X' => &[[(0.0, 0.0), (1.0, 1.0)], [(0.0, 1.0), (1.0, 0.0)]],
        '1' => &[[(0.65, 0.0), (0.65, 1.0)], [(0.65, 1.0), (0.1, 0.75)]],
        'H' => &[[(0.0, 0.0), (0.0, 1.0)], [(1.0, 0.0), (1.0, 1.0)], [(0.0, 0.5), (1.0, 0.5)]],
        'E' => &[[(0.0, 0.0), (0.0, 1.0)], [(0.0, 1.0), (1.0, 1.0)], [(0.0, 0.5), (0.8, 0.5)], [(0.0, 0.0), (1.0, 0.0)]],
        'A' => &[[(0.0, 0.0), (0.5, 1.0)], [(0.5, 1.0), (1.0, 0.0)], [(0.25, 0.45), (0.75, 0.45)]],
        'D' => &[
            [(0.0, 0.0), (0.0, 1.0)],
            [(0.0, 1.0), (0.6, 1.0)],
            [(0.6, 1.0), (1.0, 0.7)],
            [(1.0, 0.7), (1.0, 0.3)],
            [(1.0, 0.3), (0.6, 0.0)],
            [(0.6, 0.0), (0.0, 0.0)],
        ],
        'G' => &[
            [(1.0, 1.0), (0.0, 1.0)],
            [(0.0, 1.0), (0.0, 0.0)],
            [(0.0, 0.0), (1.0, 0.0)],
            [(1.0, 0.0), (1.0, 0.45)],
            [(1.0, 0.45), (0.55, 0.45)],
        ],
        'R' => &[
            [(0.0, 0.0), (0.0, 1.0)],
            [(0.0, 1.0), (1.0, 1.0)],
            [(1.0, 1.0), (1.0, 0.5)],
            [(1.0, 0.5), (0.0, 0.5)],
            [(0.4, 0.5), (1.0, 0.0)],
        ],
        _ => &[],
    }
}

/// The cube from -0.5 to 0.5, which the extras are drawn with.
pub fn unit_cube() -> Mesh {
    cuboid(Vec3::ZERO, Vec3::ONE, 0)
}

/// A box of `size` around `center`, in the material.
pub(super) fn cuboid(center: Vec3, size: Vec3, material: usize) -> Mesh {
    let half = size / 2.0;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let mut n = Vec3::ZERO;
            n[axis] = sign;
            // Two directions across the face; u x v is the normal, so the
            // corners go round anticlockwise seen from outside.
            let u = [Vec3::Y, Vec3::Z, Vec3::X][axis];
            let v = n.cross(u);
            let base = vertices.len() as u32;
            for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let p = center + (n + u * su + v * sv) * half;
                vertices.push(Vertex { position: p.to_array(), normal: n.to_array(), uv: [0.0, 0.0] });
            }
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    Mesh { vertices, indices, material }
}

/// A page lying face up, `w` across and `d` deep from its top left
/// corner, UV (0, 0) there.
fn page(corner: Vec3, w: f32, d: f32, material: usize) -> Mesh {
    let at = |x: f32, z: f32| Vertex { position: (corner + Vec3::new(x * w, 0.0, z * d)).to_array(), normal: [0.0, 1.0, 0.0], uv: [x, z] };
    Mesh { vertices: vec![at(0.0, 0.0), at(0.0, 1.0), at(1.0, 1.0), at(1.0, 0.0)], indices: vec![0, 1, 2, 0, 2, 3], material }
}

/// A page being turned, `width` long along +X from the hinge and from
/// `z.0` to `z.1`: its front faces +Y, its back (`up` -1) -Y, which lies
/// face up as the left page once turned over.
fn sheet(width: f32, (z0, z1): (f32, f32), up: f32, material: usize) -> Mesh {
    let depth = z1 - z0;
    let at = |x: f32, z: f32| {
        let u = if up > 0.0 { x } else { 1.0 - x };
        Vertex { position: [x * width, 0.0, z0 + z * depth], normal: [0.0, up, 0.0], uv: [u, z] }
    };
    let indices = if up > 0.0 { vec![0, 1, 2, 0, 2, 3] } else { vec![0, 2, 1, 0, 3, 2] };
    Mesh { vertices: vec![at(0.0, 0.0), at(0.0, 1.0), at(1.0, 1.0), at(1.0, 0.0)], indices, material }
}

/// An upright cylinder from `bottom`, closed at both ends.
fn cylinder(bottom: Vec3, radius: f32, height: f32, segments: u32, material: usize) -> Mesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let around = |i: u32| {
        let a = i as f32 / segments as f32 * std::f32::consts::TAU;
        Vec3::new(a.cos(), 0.0, -a.sin())
    };
    for i in 0..segments {
        let base = vertices.len() as u32;
        let (n0, n1) = (around(i), around(i + 1));
        for (n, y) in [(n0, 0.0), (n1, 0.0), (n1, height), (n0, height)] {
            let p = bottom + n * radius + Vec3::Y * y;
            vertices.push(Vertex { position: p.to_array(), normal: n.to_array(), uv: [0.0, 0.0] });
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    for (y, up) in [(0.0, -1.0f32), (height, 1.0)] {
        let center = vertices.len() as u32;
        let normal = [0.0, up, 0.0];
        vertices.push(Vertex { position: (bottom + Vec3::Y * y).to_array(), normal, uv: [0.0, 0.0] });
        for i in 0..segments {
            let p = bottom + around(i) * radius + Vec3::Y * y;
            vertices.push(Vertex { position: p.to_array(), normal, uv: [0.0, 0.0] });
        }
        for i in 0..segments {
            let (a, b) = (center + 1 + i, center + 1 + (i + 1) % segments);
            indices.extend(if up > 0.0 { [center, a, b] } else { [center, b, a] });
        }
    }
    Mesh { vertices, indices, material }
}

/// A hash of some bytes, to tell scenes apart.
fn hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// A key of a glTF object's `extras` (Blender's custom properties).
fn extra(extras: &gltf::json::Extras, key: &str) -> Option<serde_json::Value> {
    let raw = extras.as_ref()?;
    let value: serde_json::Value = serde_json::from_str(raw.get()).ok()?;
    value.get(key).cloned()
}

/// Whether a custom property says yes: Blender writes booleans as true or
/// as 1.
fn truthy(value: serde_json::Value) -> bool {
    match value {
        serde_json::Value::Bool(on) => on,
        serde_json::Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        serde_json::Value::String(s) => matches!(s.to_ascii_lowercase().as_str(), "true" | "yes" | "1" | "on"),
        _ => false,
    }
}

/// The bytes of a buffer or image a glTF file points to: inline as a data
/// URI, or a file beside it.
fn read_uri(uri: &str, base: &Path) -> Result<Vec<u8>, String> {
    use base64::Engine;
    if let Some(rest) = uri.strip_prefix("data:") {
        let (_, data) = rest.split_once(";base64,").ok_or("a data URI isn't base64")?;
        return base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string());
    }
    let path = base.join(percent_decode(uri));
    std::fs::read(&path).map_err(|e| format!("{}: {}", path.display(), e))
}

/// A URI's %xx escapes, as Blender writes spaces in file names.
fn percent_decode(uri: &str) -> String {
    let bytes = uri.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = uri.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A PNG or JPEG picture as RGBA.
fn decode_image(data: &[u8]) -> Result<Image, String> {
    if data.starts_with(b"\x89PNG") {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
        let mut pixels = vec![0; reader.output_buffer_size().ok_or("the picture is too big")?];
        let info = reader.next_frame(&mut pixels).map_err(|e| e.to_string())?;
        pixels.truncate(info.buffer_size());
        let rgba = match info.color_type {
            png::ColorType::Rgba => pixels,
            png::ColorType::Rgb => pixels.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            png::ColorType::GrayscaleAlpha => {
                pixels.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
            }
            png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::Indexed => return Err("an indexed PNG wasn't expanded".into()),
        };
        return Ok(Image { width: info.width, height: info.height, rgba });
    }
    if data.starts_with(&[0xFF, 0xD8]) {
        let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(data));
        let pixels = decoder.decode().map_err(|e| e.to_string())?;
        let info = decoder.info().ok_or("the JPEG has no header")?;
        let rgba = match info.pixel_format {
            jpeg_decoder::PixelFormat::RGB24 => {
                pixels.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect()
            }
            jpeg_decoder::PixelFormat::L8 => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            jpeg_decoder::PixelFormat::L16 => {
                pixels.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], 255]).collect()
            }
            jpeg_decoder::PixelFormat::CMYK32 => pixels
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| {
                    let k = 255 - p[3] as u32;
                    let c = |x: u8| ((255 - x as u32) * k / 255) as u8;
                    [c(p[0]), c(p[1]), c(p[2]), 255]
                })
                .collect(),
        };
        return Ok(Image { width: info.width as u32, height: info.height as u32, rgba });
    }
    Err("only PNG and JPEG pictures are read".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_room_has_one_screen_ahead() {
        let room = Scene::test_room();
        let screens = room.meshes.iter().filter(|m| room.materials[m.material].shading == Shading::Screen).count();
        assert_eq!(screens, 1);
        assert_eq!(room.screen.triangles.len(), 2);
        assert!((room.screen.aspect() - 4.0 / 3.0).abs() < 1e-4);
        assert!((room.screen.size.x - 1.6).abs() < 1e-4);
        // It faces the viewer, who looks at it.
        assert!(room.screen.normal.z > 0.99);
        assert!(room.screen.center.z < room.spawn.position.z);
        // Its sound comes from its sides; the tower has its four lights.
        assert!(room.speakers[0].x < -0.7 && room.speakers[1].x > 0.7);
        for led in [Led::Power, Led::Turbo, Led::Hdd, Led::Floppy] {
            assert!(room.meshes.iter().any(|m| room.materials[m.material].led == Some(led)), "{:?}", led);
        }
    }

    #[test]
    fn boxes_face_outwards() {
        let mesh = cuboid(Vec3::ZERO, Vec3::ONE, 0);
        for tri in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| Vec3::from(mesh.vertices[i as usize].position));
            let n = (b - a).cross(c - a);
            let mid = (a + b + c) / 3.0;
            assert!(n.dot(mid) > 0.0, "{:?}", tri);
        }
    }

    /// A glTF file of a quad called "screen" under a parent that moves it
    /// up, and an empty called "spawn" turned to the left.
    fn quad_gltf() -> String {
        use base64::Engine;
        let mut bin = Vec::new();
        for p in [[-1.0f32, 1.0, 0.0], [-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [1.0, 1.0, 0.0]] {
            for c in p {
                bin.extend(c.to_le_bytes());
            }
        }
        for uv in [[0.0f32, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]] {
            for c in uv {
                bin.extend(c.to_le_bytes());
            }
        }
        for i in [0u16, 1, 2, 0, 2, 3] {
            bin.extend(i.to_le_bytes());
        }
        let data = base64::engine::general_purpose::STANDARD.encode(&bin);
        // A quarter turn about Y: looking along -X.
        let s = std::f32::consts::FRAC_1_SQRT_2;
        format!(
            r#"{{
            "asset": {{"version": "2.0"}},
            "scene": 0,
            "scenes": [{{"nodes": [0, 2, 3, 4]}}],
            "nodes": [
                {{"name": "Monitor", "translation": [0, 2, -3], "children": [1]}},
                {{"name": "Screen", "mesh": 0}},
                {{"name": "spawn", "translation": [0, 1.5, 0], "rotation": [0, {s}, 0, {s}]}},
                {{"name": "speaker_left.001", "translation": [-2, 1, -3]}},
                {{"name": "Light", "mesh": 0, "extras": {{"rustdos_led": "hdd"}}, "translation": [5, 0, 0]}}
            ],
            "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0, "TEXCOORD_0": 1}}, "indices": 2}}]}}],
            "buffers": [{{"byteLength": {len}, "uri": "data:application/octet-stream;base64,{data}"}}],
            "bufferViews": [
                {{"buffer": 0, "byteOffset": 0, "byteLength": 48}},
                {{"buffer": 0, "byteOffset": 48, "byteLength": 32}},
                {{"buffer": 0, "byteOffset": 80, "byteLength": 12}}
            ],
            "accessors": [
                {{"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3", "min": [-1, -1, 0], "max": [1, 1, 0]}},
                {{"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC2"}},
                {{"bufferView": 2, "componentType": 5123, "count": 6, "type": "SCALAR"}}
            ]
        }}"#,
            len = bin.len()
        )
    }

    #[test]
    fn gltf_scenes_find_the_screen_and_the_spawn() {
        let scene = Scene::from_gltf(quad_gltf().as_bytes(), Path::new(".")).unwrap();
        assert_eq!(scene.screen.triangles.len(), 2);
        assert!((scene.screen.center - Vec3::new(0.0, 2.0, -3.0)).length() < 1e-4);
        assert!((scene.screen.size - Vec2::new(2.0, 2.0)).length() < 1e-4);
        // Smooth normals were made for it, facing +Z.
        assert!(scene.meshes[0].vertices.iter().all(|v| v.normal[2] > 0.99));
        assert!((scene.spawn.position - Vec3::new(0.0, 1.5, 0.0)).length() < 1e-4);
        assert!((scene.spawn.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-4, "{}", scene.spawn.yaw);
        // Without lights of its own, the sunset lights it.
        assert_eq!(scene.lights.len(), 1);
        // The left speaker is the scene's, the right the screen's side.
        assert!((scene.speakers[0] - Vec3::new(-2.0, 1.0, -3.0)).length() < 1e-4);
        assert!((scene.speakers[1] - Vec3::new(1.0, 2.0, -3.0)).length() < 1e-4, "{:?}", scene.speakers);
        // The light has a material of its own, which glows.
        let led = &scene.materials[scene.meshes[1].material];
        assert_eq!(led.led, Some(Led::Hdd));
        assert!(led.emissive[0] > 0.0);
        assert_eq!(scene.materials[scene.meshes[0].material].led, None);
        assert!(!scene.screen.stretch);
    }

    #[test]
    fn gltf_screens_can_ask_to_be_filled() {
        let text = quad_gltf().replace(
            r#"{"name": "Screen", "mesh": 0}"#,
            r#"{"name": "Screen", "mesh": 0, "extras": {"rustdos_screen_fit": "Stretch"}}"#,
        );
        assert_ne!(text, quad_gltf());
        let scene = Scene::from_gltf(text.as_bytes(), Path::new(".")).unwrap();
        assert!(scene.screen.stretch);
    }

    #[test]
    fn the_blender_test_room_is_the_built_in_one() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/vr/test-room.glb");
        let blend = Scene::load(&path).unwrap_or_else(|e| panic!("{}", e));
        let built = Scene::test_room();
        assert_eq!(blend.meshes.len(), built.meshes.len());
        let near = |a: Vec3, b: Vec3| (a - b).length() < 1e-4;
        assert!(near(blend.screen.center, built.screen.center), "{:?}", blend.screen.center);
        assert!(near(blend.screen.normal, built.screen.normal));
        assert!(near(blend.screen.right, built.screen.right));
        assert!((blend.screen.size - built.screen.size).length() < 1e-4);
        assert!(near(blend.spawn.position, built.spawn.position) && blend.spawn.yaw.abs() < 1e-4);
        assert!(near(blend.speakers[0], built.speakers[0]) && near(blend.speakers[1], built.speakers[1]));
        // Lit by the same sunset, under the same sky.
        assert_eq!(blend.lights, built.lights);
        assert_eq!(blend.ambient, built.ambient);
        assert!(blend.sky && blend.exposure == 1.0);
        // The same lights on the tower, as bright.
        for led in [Led::Power, Led::Turbo, Led::Hdd, Led::Floppy] {
            let find = |scene: &Scene| {
                let m = scene.meshes.iter().find(|m| scene.materials[m.material].led == Some(led)).expect("the light");
                let center = m.vertices.iter().map(|v| Vec3::from(v.position)).sum::<Vec3>() / m.vertices.len() as f32;
                (center, scene.materials[m.material].emissive)
            };
            let ((a, ea), (b, eb)) = (find(&blend), find(&built));
            assert!(near(a, b), "{:?}: {:?} {:?}", led, a, b);
            assert!(ea.iter().zip(eb).all(|(x, y)| (x - y).abs() < 1e-4), "{:?}: {:?} {:?}", led, ea, eb);
        }
        // The manual and the joystick on the desk, as Rust-DOS's models.
        assert_eq!(blend.props.len(), built.props.len());
        for b in &built.props {
            let a = blend.props.iter().find(|a| a.kind == b.kind).expect("the prop");
            assert!(a.pose.abs_diff_eq(b.pose, 1e-4), "{:?}: {:?} {:?}", a.kind, a.pose, b.pose);
            assert_eq!(a.parts.len(), b.parts.len());
        }
    }

    #[test]
    fn props_stand_on_the_desk_and_have_their_parts() {
        let room = Scene::test_room();
        let kinds: Vec<PropKind> = room.props.iter().map(|p| p.kind).collect();
        use PropKind::{Camera, Joystick, Keyboard, Manual, Vfx1};
        let all = [Manual, Joystick, Camera, Vfx1, Keyboard];
        assert_eq!(kinds, all);
        for prop in &room.props {
            let at = prop.pose.transform_point3(Vec3::ZERO);
            assert_eq!(room.surface_below(at + Vec3::Y * 0.1, 1.0), Some(at), "{:?} on the desk", prop.kind);
            assert!(prop.bounds.0.y >= -1e-4 && prop.bounds.1.y > 0.0);
        }
        let manual = &room.props[0];
        for role in [PartRole::PageLeft, PartRole::PageRight, PartRole::SheetFront, PartRole::SheetBack] {
            assert!(manual.has(role), "{:?}", role);
        }
        // The sheet turns about the spine, between the pages.
        assert!(manual.hinge.x.abs() < 1e-4);
        let joystick = &room.props[1];
        assert!(joystick.has(PartRole::Stick) && joystick.knob.y > 0.1);
        // A scene without them gets them where the hands are, on the floor
        // of the glTF quad's scene (nothing is below).
        let scene = Scene::from_gltf(quad_gltf().as_bytes(), Path::new(".")).unwrap();
        assert_eq!(scene.props.len(), 5);
        // The camera looks at the viewer, from the front of its box.
        let camera = &room.props[2];
        let lens = camera.pose * camera.lens.unwrap();
        assert!(lens.transform_vector3(Vec3::NEG_Z).z > 0.99, "{:?}", lens);
        assert!(camera.lens.unwrap().transform_point3(Vec3::ZERO).z <= camera.bounds.0.z + 0.01);
        // The VFX1's front faces the viewer too, its eyes inside it.
        let vfx1 = &room.props[3];
        assert!(vfx1.pose.transform_vector3(Vec3::NEG_Z).z > 0.99);
        let (low, high) = vfx1.bounds;
        assert!(VFX1_EYES.cmpge(low).all() && VFX1_EYES.cmple(high).all());
        // The keyboard's keys, each on its own and a row's together, on
        // its case, their legends in a picture of the scene's.
        let keyboard = &room.props[4];
        assert_eq!(keyboard.keys.len(), 101);
        assert!(keyboard.has(PartRole::KeyRow(5)) && keyboard.has(PartRole::Key(100)));
        let (low, high) = keyboard.bounds;
        let on = |k: &keycaps::Key| k.center.cmpge(Vec2::new(low.x, low.z)).all() && k.top <= high.y;
        assert!(keyboard.keys.iter().all(on));
        assert!((high.x - low.x - 0.45).abs() < 0.01, "{:?}", keyboard.bounds);
        let cap = keyboard.parts.iter().find(|p| p.role == PartRole::Key(0)).unwrap();
        let caps = cap.mesh.material;
        assert!(room.materials[caps].base_texture.is_some_and(|t| t.image < room.images.len()));
        // Nothing stands in another's way on the desk.
        let footprint = |p: &Prop| {
            let (low, high) = p.solid;
            let corners = [low, high, Vec3::new(low.x, 0.0, high.z), Vec3::new(high.x, 0.0, low.z)];
            let points = corners.map(|c| p.pose.transform_point3(c));
            let low = points.iter().fold(Vec3::MAX, |a, &b| a.min(b));
            let high = points.iter().fold(Vec3::MIN, |a, &b| a.max(b));
            (Vec2::new(low.x, low.z), Vec2::new(high.x, high.z))
        };
        for (i, a) in room.props.iter().enumerate() {
            for b in &room.props[i + 1..] {
                let ((a0, a1), (b0, b1)) = (footprint(a), footprint(b));
                let apart = a1.x <= b0.x || b1.x <= a0.x || a1.y <= b0.y || b1.y <= a0.y;
                assert!(apart, "{:?} {:?}", a.kind, b.kind);
            }
            let (low, high) = footprint(a);
            let (desk0, desk1) = (TEST_DESK - TEST_DESK_TOP / 2.0, TEST_DESK + TEST_DESK_TOP / 2.0);
            let (desk0, desk1) = (Vec2::new(desk0.x, desk0.z), Vec2::new(desk1.x, desk1.z));
            assert!(low.cmpge(desk0).all() && high.cmple(desk1).all(), "{:?}", a.kind);
        }
    }

    #[test]
    fn gltf_props_keep_their_own_meshes() {
        // The screen quad, and a joystick: a base with the quad as its mesh
        // and a stick below it, moved and turned.
        let text = quad_gltf()
            .replace(r#""nodes": [0, 2, 3, 4]"#, r#""nodes": [0, 2, 3, 4, 5]"#)
            .replace(
                r#"{"name": "Light", "mesh": 0, "extras": {"rustdos_led": "hdd"}, "translation": [5, 0, 0]}"#,
                r#"{"name": "Light", "mesh": 0, "extras": {"rustdos_led": "hdd"}, "translation": [5, 0, 0]},
                {"name": "Stick base", "extras": {"rustdos_prop": "joystick"}, "mesh": 0, "translation": [1, 0.7, -0.5], "children": [6]},
                {"name": "joystick_stick", "mesh": 0, "translation": [0, 0.05, 0]}"#,
            );
        let scene = Scene::from_gltf(text.as_bytes(), Path::new(".")).unwrap();
        // Not in the room's meshes; and the manual is Rust-DOS's.
        assert_eq!(scene.meshes.len(), 2);
        let joystick = scene.props.iter().find(|p| p.kind == PropKind::Joystick).unwrap();
        assert!(joystick.pose.transform_point3(Vec3::ZERO).abs_diff_eq(Vec3::new(1.0, 0.7, -0.5), 1e-4));
        assert_eq!(joystick.parts.iter().filter(|p| p.role == PartRole::Body).count(), 1);
        assert_eq!(joystick.parts.iter().filter(|p| p.role == PartRole::Stick).count(), 1);
        assert!(joystick.stick.transform_point3(Vec3::ZERO).abs_diff_eq(Vec3::new(0.0, 0.05, 0.0), 1e-4));
        // The stick's mesh is in its own frame: the quad, from -1 to 1.
        let stick = &joystick.parts.iter().find(|p| p.role == PartRole::Stick).unwrap().mesh;
        assert!(stick.vertices.iter().any(|v| v.position == [1.0, 1.0, 0.0]));
        assert!((joystick.knob.y - 0.985).abs() < 1e-4);
        assert!(scene.props.iter().any(|p| p.kind == PropKind::Manual && p.has(PartRole::PageRight)));
    }

    #[test]
    fn gltf_items_are_props_of_their_own() {
        // A stapler with the quad as its mesh, and an empty item, which is
        // nothing.
        let text = quad_gltf().replace(r#""nodes": [0, 2, 3, 4]"#, r#""nodes": [0, 2, 3, 4, 5, 6]"#).replace(
            r#"{"name": "Light", "mesh": 0, "extras": {"rustdos_led": "hdd"}, "translation": [5, 0, 0]}"#,
            r#"{"name": "Light", "mesh": 0, "extras": {"rustdos_led": "hdd"}, "translation": [5, 0, 0]},
                {"name": "Stapler", "extras": {"rustdos_prop": "item"}, "mesh": 0, "translation": [1, 0.7, -0.5], "scale": [0.05, 0.001, 1]},
                {"name": "item.001"}"#,
        );
        let scene = Scene::from_gltf(text.as_bytes(), Path::new(".")).unwrap();
        let items: Vec<&Prop> = scene.props.iter().filter(|p| p.kind == PropKind::Item).collect();
        assert_eq!(items.len(), 1);
        assert_eq!(scene.meshes.len(), 2);
        // Thin as it is, it is taken by a box of some thickness.
        let (low, high) = items[0].bounds;
        assert!((high - low).min_element() >= THINNEST - 1e-6, "{:?}", items[0].bounds);
        assert!((high.x - low.x - 0.1).abs() < 1e-4);
    }

    #[test]
    fn gltf_scenes_need_a_screen() {
        let text = quad_gltf().replace("\"Screen\"", "\"Panel\"");
        assert!(Scene::from_gltf(text.as_bytes(), Path::new(".")).err().unwrap().contains("screen"));
    }
}
