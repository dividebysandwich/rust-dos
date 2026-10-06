//! What the 3D view shows, as plain data: the meshes with their materials
//! and pictures, the lights, the screen the emulated picture goes on and
//! where the viewer starts. It comes from a glTF file exported from
//! Blender, or is the built-in test room.

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
}

impl Material {
    /// Whether it casts shadows: not if it is seen through, or is a light
    /// or a backdrop (black, and glowing).
    pub fn casts_shadow(&self) -> bool {
        let black = self.base_color[..3] == [0.0; 3];
        let glows = self.emissive != [0.0; 3] || self.emissive_texture.is_some();
        self.alpha != Alpha::Blend && self.shading != Shading::Screen && !(black && glows)
    }

    fn plain(color: [f32; 3], shading: Shading) -> Self {
        Material {
            base_color: [color[0], color[1], color[2], 1.0],
            base_texture: None,
            emissive: [0.0; 3],
            emissive_texture: None,
            shading,
            double_sided: false,
            alpha: Alpha::Opaque,
            led: None,
        }
    }

    /// A light of the PC's, glowing `color` when lit.
    fn led(led: Led, color: [f32; 3]) -> Self {
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

pub struct Scene {
    pub meshes: Vec<Mesh>,
    pub materials: Vec<Material>,
    pub images: Vec<Image>,
    pub lights: Vec<Light>,
    /// Light from everywhere, linear RGB.
    pub ambient: Vec3,
    pub screen: Screen,
    pub spawn: Spawn,
    /// Whether the sunset sky is drawn around the scene, which then fades
    /// into it with distance.
    pub sky: bool,
    /// Towards the sky's sun.
    pub sun: Vec3,
    /// What the lit colours are multiplied by.
    pub exposure: f32,
    /// Where the left and right channels' sound comes from: the scene's
    /// `speaker_left` and `speaker_right`, else the screen's sides.
    pub speakers: [Vec3; 2],
    /// Whether the lights cast shadows (`rustdos_shadows`) and the light
    /// bouncing around the room is worked out (`rustdos_gi`).
    pub shadows: bool,
    pub gi: bool,
    /// How brightly the screen lights the room, 1 as bright as the picture
    /// is (`rustdos_screen_glow`).
    pub screen_glow: f32,
    /// A hash of where the scene came from, for keeping what is worked out
    /// for it (`gi::Probes::key`).
    pub source: u64,
    /// The light bounced around it, once the first view of it works it
    /// out: shared by the window's and the headset's.
    pub probes: std::sync::OnceLock<Option<super::gi::Probes>>,
}

/// How far from the screen and the viewer's start the lighting is worked
/// out in detail, in metres.
const FOCUS: f32 = 12.0;

/// How brightly the screen lights the room, unless the scene says.
const SCREEN_GLOW: f32 = 5.0;

/// The sunset sky's sun: low, ahead and to the left.
fn sunset_sun() -> Vec3 {
    Vec3::new(-0.45, 0.1, -1.0).normalize()
}

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
fn srgb(hex: u32) -> [f32; 3] {
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
        let mut scene = Scene {
            meshes,
            materials,
            images: Vec::new(),
            lights: Vec::new(),
            ambient: Vec3::ZERO,
            screen: Screen::default(),
            spawn: Spawn { position: Vec3::new(0.0, 1.2, 0.0), yaw: 0.0, pitch: 0.0 },
            sky: true,
            sun: sunset_sun(),
            exposure: 1.0,
            speakers: [Vec3::ZERO; 2],
            shadows: true,
            gi: true,
            screen_glow: SCREEN_GLOW,
            source: hash(b"the test room"),
            probes: std::sync::OnceLock::new(),
        };
        let (sun, ambient) = sunset_lights();
        scene.lights.push(sun);
        scene.ambient = ambient;
        scene.collect_screen();
        scene.speakers = scene.screen_speakers();
        scene
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
            for tri in mesh.indices.chunks_exact(3) {
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
            sun: sunset_sun(),
            exposure: extra(scene.extras(), "rustdos_exposure").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
            speakers: [Vec3::ZERO; 2],
            shadows: extra(scene.extras(), "rustdos_shadows").is_none_or(truthy),
            gi: extra(scene.extras(), "rustdos_gi").is_none_or(truthy),
            screen_glow: extra(scene.extras(), "rustdos_screen_glow")
                .and_then(|v| v.as_f64())
                .map_or(SCREEN_GLOW, |g| g.max(0.0) as f32),
            source: hash(bytes),
            probes: std::sync::OnceLock::new(),
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
        // A scene without lights of its own is lit by the sunset; with
        // some, the sky only adds a little.
        let (sun, ambient) = sunset_lights();
        if loaded.lights.is_empty() {
            loaded.lights.push(sun);
            loaded.ambient = ambient;
        } else {
            loaded.ambient = ambient * 0.5;
        }
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
}

/// The lights of a scene are at most this many; the shader has room for
/// no more.
pub const MAX_LIGHTS: usize = 8;

impl Walk<'_> {
    fn node(&mut self, scene: &mut Scene, node: gltf::Node, parent: Mat4, in_screen: bool) -> Result<(), String> {
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
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
        if let Some(light) = node.light()
            && scene.lights.len() < MAX_LIGHTS
        {
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
        if let Some(mesh) = node.mesh() {
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
                let material = if screen {
                    self.screen_material
                } else {
                    primitive.material().index().unwrap_or(self.default_material)
                };
                if let Some(led) = led.filter(|_| !screen) {
                    self.leds.push((scene.meshes.len(), led));
                }
                scene.meshes.push(Mesh { vertices, indices, material });
            }
        }
        for child in node.children() {
            self.node(scene, child, world, screen)?;
        }
        Ok(())
    }
}

/// Normals for a mesh that has none: each vertex's faces', by their area.
fn smooth_normals(positions: &[Vec3], indices: &[u32]) -> Vec<Vec3> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for tri in indices.chunks_exact(3) {
        let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| i as usize);
        let n = (positions[b] - positions[a]).cross(positions[c] - positions[a]);
        for i in [a, b, c] {
            normals[i] += n;
        }
    }
    normals.into_iter().map(|n| n.normalize_or(Vec3::Y)).collect()
}

/// The cube from -0.5 to 0.5, which the extras are drawn with.
pub fn unit_cube() -> Mesh {
    cuboid(Vec3::ZERO, Vec3::ONE, 0)
}

/// A box of `size` around `center`, in the material.
fn cuboid(center: Vec3, size: Vec3, material: usize) -> Mesh {
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
            png::ColorType::Rgb => pixels.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            png::ColorType::GrayscaleAlpha => pixels.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
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
            jpeg_decoder::PixelFormat::RGB24 => pixels.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            jpeg_decoder::PixelFormat::L8 => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            jpeg_decoder::PixelFormat::L16 => pixels.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], 255]).collect(),
            jpeg_decoder::PixelFormat::CMYK32 => pixels
                .chunks_exact(4)
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
        for tri in mesh.indices.chunks_exact(3) {
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
    }

    #[test]
    fn gltf_scenes_need_a_screen() {
        let text = quad_gltf().replace("\"Screen\"", "\"Panel\"");
        assert!(Scene::from_gltf(text.as_bytes(), Path::new(".")).err().unwrap().contains("screen"));
    }
}
