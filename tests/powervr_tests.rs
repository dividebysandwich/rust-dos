//! The PowerVR PCX2: its place on the PCI bus, its windows, and the
//! render handshake Tomb Raider's PowerVR version uses.

use rust_dos::bus::Bus;
use rust_dos::powervr::{Chip, regs};
use std::path::PathBuf;

const REGS: usize = 0xD100_0000;
const TEXTURES: usize = 0xD140_0000;

/// A machine with a PCX2.
fn bus() -> Bus {
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    bus.configure_powervr(Some(Chip::Pcx2));
    bus
}

fn cfg_read(bus: &mut Bus, device: u32, reg: u32) -> u32 {
    bus.io_write_wide(0xCF8, 0x8000_0000 | device << 11 | reg, 4);
    bus.io_read_wide(0xCFC, 4)
}

fn cfg_write(bus: &mut Bus, device: u32, reg: u32, value: u32) {
    bus.io_write_wide(0xCF8, 0x8000_0000 | device << 11 | reg, 4);
    bus.io_write_wide(0xCFC, value, 4);
}

fn w(bus: &mut Bus, reg: usize, value: u32) {
    bus.write_32(REGS + 4 * reg, value);
}

fn r(bus: &Bus, reg: usize) -> u32 {
    bus.read_32(REGS + 4 * reg)
}

#[test]
fn the_card_is_device_2_of_the_pci_bus() {
    let mut bus = bus();
    assert!(bus.pci_present());
    assert_eq!(cfg_read(&mut bus, 2, 0x00), 0x0046_1033, "NEC PCX2");
    assert_eq!(cfg_read(&mut bus, 2, 0x10), 0xD100_0000, "BAR0, the registers");
    assert_eq!(cfg_read(&mut bus, 2, 0x14), 0xD140_0008, "BAR1, the texture memory, prefetchable");
    assert_eq!(cfg_read(&mut bus, 2, 0x3C) & 0xFFFF, 0x010B, "INTA# on IRQ 11");
    assert_eq!(cfg_read(&mut bus, 0, 0x00), 0xFFFF_FFFF, "no 3dfx card");
}

#[test]
fn the_pci_bios_finds_the_card() {
    let mut cpu = rust_dos::cpu::Cpu::new(PathBuf::from("."));
    cpu.bus.configure_powervr(Some(Chip::Pcx2));
    cpu.set_ax(0xB102);
    cpu.set_cx(0x0046);
    cpu.set_dx(0x1033);
    cpu.set_si(0);
    rust_dos::pci::bios(&mut cpu);
    assert_eq!(cpu.get_reg8(iced_x86::Register::AH), 0);
    assert_eq!(cpu.bx(), 2 << 3, "bus 0, device 2, function 0");
}

#[test]
fn bars_size_and_move() {
    let mut bus = bus();
    cfg_write(&mut bus, 2, 0x10, 0xFFFF_FFFF);
    cfg_write(&mut bus, 2, 0x14, 0xFFFF_FFFF);
    assert_eq!(cfg_read(&mut bus, 2, 0x10), 0xFFFF_0000, "64 KB of registers");
    assert_eq!(cfg_read(&mut bus, 2, 0x14), 0xFFC0_0008, "4 MB of texture memory");
    cfg_write(&mut bus, 2, 0x10, 0xE900_0000);
    cfg_write(&mut bus, 2, 0x14, 0xE940_0000);
    w(&mut bus, regs::FOGCOL, 0); // nowhere now
    bus.write_32(0xE900_0000 + 4 * regs::FOGCOL, 0x0112_3456);
    assert_eq!(bus.read_32(0xE900_0000 + 4 * regs::FOGCOL), 0x0112_3456);
    bus.write_16(0xE940_0002, 0xBEEF);
    assert_eq!(bus.read_32(0xE940_0000), 0xBEEF_0000);
    assert_eq!(bus.read_32(TEXTURES), 0xFFFF_FFFF, "nothing at the old place");
}

#[test]
fn texture_memory_takes_every_access_size() {
    let mut bus = bus();
    bus.write_32(TEXTURES + 0x100, 0x4433_2211);
    bus.write_8(TEXTURES + 0x104, 0x55);
    assert_eq!(bus.read_16(TEXTURES + 0x102), 0x4433);
    assert_eq!(bus.read_8(TEXTURES + 0x104), 0x55);
}

#[test]
fn a_render_ends_and_the_reset_pulse_clears_its_status() {
    let mut bus = bus();
    assert_eq!(r(&bus, regs::INTSTATUS) & regs::END_OF_RENDER, 0);
    w(&mut bus, regs::SOFTRESET, 1);
    w(&mut bus, regs::SOFTRESET, 0);
    w(&mut bus, regs::STARTRENDER, 0);
    assert_ne!(r(&bus, regs::INTSTATUS) & regs::END_OF_RENDER, 0);
    w(&mut bus, regs::SOFTRESET, 1);
    assert_eq!(r(&bus, regs::INTSTATUS) & regs::END_OF_RENDER, 0);
}

#[test]
fn the_end_of_a_render_interrupts_when_unmasked() {
    let mut bus = bus();
    w(&mut bus, regs::INTMASK, regs::END_OF_RENDER);
    w(&mut bus, regs::STARTRENDER, 0);
    assert!(bus.pic.busy(11), "IRQ 11 requested");
}

#[test]
fn state_round_trips() {
    let mut bus = bus();
    w(&mut bus, regs::PACKMODE, 0x12);
    bus.write_32(TEXTURES + 0x3F_FFFC, 0xCAFE_F00D);
    let mut w2 = rust_dos::savestate::Writer::new();
    rust_dos::savestate::State::save(bus.powervr.as_ref().unwrap(), &mut w2);
    let bytes = w2.buf;
    let mut other = self::bus();
    let mut reader = rust_dos::savestate::Reader::new(&bytes);
    rust_dos::savestate::State::load(other.powervr.as_mut().unwrap(), &mut reader).unwrap();
    assert_eq!(r(&other, regs::PACKMODE), 0x12);
    assert_eq!(other.read_32(TEXTURES + 0x3F_FFFC), 0xCAFE_F00D);
}

/// Render a snapshot `RUST_DOS_POWERVR_TRACE` took (`render-N.regs`,
/// `.tex`, `.ram`) into `render-N.png` beside it:
/// `RUST_DOS_POWERVR_SNAPSHOT=dir/render-500 cargo test --release
/// --test powervr_tests -- --ignored snapshot`.
#[test]
#[ignore]
fn renders_a_snapshot() {
    let Ok(base) = std::env::var("RUST_DOS_POWERVR_SNAPSHOT") else { return };
    let read = |ext: &str| std::fs::read(format!("{}.{}", base, ext)).unwrap();
    let regs: Vec<u32> = read("regs").chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let (tex, ram) = (read("tex"), read("ram"));
    let shader = rust_dos::powervr::tsp::Shader::default();
    let start = std::time::Instant::now();
    let rendered = rust_dos::powervr::render::render(&regs, &tex, rust_dos::powervr::render::Memory { ram: &ram }, &shader);
    eprintln!("{} tiles, {} plane-pixels, {:?}", rendered.tiles.len(), rendered.work, start.elapsed());
    let (w, h) = (640usize, 480usize);
    let mut rgb = vec![0u8; w * h * 3];
    for tile in &rendered.tiles {
        for y in 0..tile.height as usize {
            for x in 0..tile.width as usize {
                let (px, py) = (tile.x as usize + x, tile.y as usize + y);
                if px < w && py < h {
                    let c = tile.pixels[y * tile.width as usize + x];
                    rgb[(py * w + px) * 3..][..3].copy_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8]);
                }
            }
        }
    }
    let file = std::fs::File::create(format!("{}.png", base)).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.write_header().unwrap().write_image_data(&rgb).unwrap();
}

/// Where the tests put the parameter space, and the frame buffer.
const PARAMS: u32 = 0x20_0000;
const FRAME: u32 = 0x30_0000;

/// A scene the way the driver lays one out: the TLB over `PARAMS`, the
/// planes at byte 100h of the parameter space, a one-tile object list at
/// 1000h, TSP records in the texture memory; a 32x32 frame buffer in RAM,
/// 565.
struct Scene {
    planes: Vec<[u32; 4]>,
    pointers: Vec<u32>,
}

impl Scene {
    fn new() -> Self {
        Self { planes: Vec::new(), pointers: Vec::new() }
    }

    /// An object of these planes (A, B, C, instruction, tag).
    fn object(&mut self, planes: &[(f32, f32, f32, u32, u32)]) {
        let addr = (0x100 + 16 * self.planes.len() as u32) >> 2;
        for &(a, b, c, instr, tag) in planes {
            self.planes.push([a.to_bits(), b.to_bits(), c.to_bits(), instr | tag << 4]);
        }
        self.pointers.push(addr | (planes.len() as u32) << 19);
    }

    fn render(self, bus: &mut Bus) {
        for page in 0..128 {
            w(bus, regs::TLB + page, (PARAMS >> 12) + 4 * page as u32);
        }
        for (i, plane) in self.planes.iter().enumerate() {
            for (j, word) in plane.iter().enumerate() {
                bus.write_32((PARAMS + 0x100 + 16 * i as u32 + 4 * j as u32) as usize, *word);
            }
        }
        // One 32x32 tile at 0, 0.
        bus.write_32((PARAMS + 0x1000) as usize, 0x4000_0000 | 31 << 5);
        let n = self.pointers.len();
        for (i, p) in self.pointers.iter().enumerate() {
            let last = if i + 1 == n { 0x8000_0000 } else { 0 };
            bus.write_32((PARAMS + 0x1004 + 4 * i as u32) as usize, p | last);
        }
        w(bus, regs::OBJECT_OFFSET, 0x1000 | 1);
        w(bus, regs::PACKMODE, 2);
        w(bus, regs::SOFADDR, FRAME);
        w(bus, regs::LSTRIDE, 64);
        w(bus, regs::SOFTRESET, 1);
        w(bus, regs::SOFTRESET, 0);
        w(bus, regs::STARTRENDER, 0);
    }
}

/// A flat-shaded, unfogged TSP record for tag `tag`.
fn flat(bus: &mut Bus, tag: u32, (r, g, b): (u32, u32, u32)) {
    let at = TEXTURES + 8 * tag as usize;
    bus.write_32(at, 0x2000_0000 | r);
    bus.write_32(at + 4, g << 24 | b << 16);
}

fn pixel(bus: &Bus, x: usize, y: usize) -> u16 {
    bus.read_16(FRAME as usize + y * 64 + x * 2)
}

/// The background, a triangle with corners 4,4, 28,4 and 4,28 in front
/// of it, and the plane that ends the list.
fn triangle_scene(depth: f32) -> Scene {
    let mut scene = Scene::new();
    scene.object(&[(0.0, 0.0, 0.0, 8, 4)]);
    scene.object(&[
        (0.0, 0.0, depth, 8, 6),
        (1.0, 0.0, -4.0, 2, 0),
        (0.0, 1.0, -4.0, 2, 0),
        (-1.0, -1.0, 32.0, 2, 0),
    ]);
    scene.object(&[(0.0, 0.0, -1.0, 8, 0)]);
    scene
}

#[test]
fn a_flat_triangle_over_the_background() {
    let mut bus = bus();
    flat(&mut bus, 4, (0, 0, 255));
    flat(&mut bus, 6, (255, 0, 0));
    triangle_scene(0.5).render(&mut bus);
    assert_ne!(r(&bus, regs::INTSTATUS) & regs::END_OF_RENDER, 0, "the render ended");
    assert_eq!(pixel(&bus, 8, 8), 0xF800, "the triangle, red");
    assert_eq!(pixel(&bus, 2, 2), 0x001F, "the background, blue");
    assert_eq!(pixel(&bus, 30, 30), 0x001F, "past the long edge");
}

#[test]
fn a_triangle_behind_the_background_is_hidden() {
    let mut bus = bus();
    flat(&mut bus, 4, (0, 0, 255));
    flat(&mut bus, 6, (255, 0, 0));
    triangle_scene(-0.5).render(&mut bus);
    assert_eq!(pixel(&bus, 8, 8), 0x001F);
}

#[test]
fn a_translucent_pass_draws_in_front_of_the_opaque_one() {
    let mut bus = bus();
    flat(&mut bus, 4, (0, 0, 248));
    // Red, translucent, global translucency 8 of 16.
    let at = TEXTURES + 8 * 6;
    bus.write_32(at, 0x2000_0000 | 0x400 | 8 << 13 | 248);
    bus.write_32(at + 4, 0);
    let mut scene = Scene::new();
    scene.object(&[(0.0, 0.0, 0.0, 8, 4)]);
    // A translucent pass: its start, then the triangle.
    scene.object(&[(0.0, 0.0, 0.0, 0xF, 0), (0.0, 0.0, 0.0, 6, 0)]);
    scene.object(&[
        (0.0, 0.0, 0.5, 8, 6),
        (1.0, 0.0, -4.0, 2, 0),
        (0.0, 1.0, -4.0, 2, 0),
        (-1.0, -1.0, 32.0, 2, 0),
    ]);
    scene.object(&[(0.0, 0.0, -1.0, 8, 0)]);
    scene.render(&mut bus);
    // Untextured, so the alpha is 0 and global translucency doesn't
    // apply: the triangle covers what is under it.
    assert_eq!(pixel(&bus, 8, 8), 0xF800);
    assert_eq!(pixel(&bus, 2, 2), 0x001F);
}
