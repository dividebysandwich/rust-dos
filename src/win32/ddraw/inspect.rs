//! What a debugger sees of DirectDraw: its surfaces, copied into plain
//! structs (`snapshot.rs`), and a surface's pixels as a picture.

use super::State;
use super::surface::{self, DDPF_FOURCC, DDPF_PALETTEINDEXED8, KEY_SRC_BLT, MEM_CARD, Surface};
use crate::cpu::Cpu;
use crate::win32::snapshot::{DirectDrawInfo, SurfaceInfo};

/// The names of the color keys' places, in their order.
const KEY_NAMES: [&str; 4] = ["dest_overlay", "dest_blt", "src_overlay", "src_blt"];

/// DirectDraw's display and surfaces as they are.
pub fn info(cpu: &Cpu) -> DirectDrawInfo {
    let s = &cpu.bus.win32.ddraw;
    let surfaces = s
        .objects
        .iter()
        .enumerate()
        .filter_map(|(i, o)| {
            let o = o.as_ref()?;
            let sf = o.surface.as_ref()?;
            Some(surface_info(s, i as u32 + 1, o.pid, &o.ifaces, sf))
        })
        .collect();
    DirectDrawInfo {
        mode: s.display.mode,
        mode_owner: s.display.mode_owner,
        exclusive: s.display.exclusive,
        surfaces,
    }
}

fn surface_info(
    s: &State,
    id: u32,
    pid: u32,
    ifaces: &[super::Iface],
    sf: &Surface,
) -> SurfaceInfo {
    let f = &sf.format;
    SurfaceInfo {
        id,
        pid,
        interfaces: ifaces.iter().map(|i| i.address).collect(),
        width: sf.width,
        height: sf.height,
        pitch: sf.pitch,
        format: (f.flags, f.fourcc, f.bits, [f.r, f.g, f.b, f.a]),
        caps: sf.caps,
        memory: match sf.memory {
            surface::MEM_SYSTEM => "system",
            surface::MEM_VIDEO => "video",
            MEM_CARD => "card",
            _ => "none",
        },
        address: sf.bits,
        locked: sf.locked,
        lost: sf.lost,
        keys: (0..4)
            .filter(|&k| sf.key_set & 1 << k != 0)
            .map(|k| (KEY_NAMES[k], sf.keys[k].0, sf.keys[k].1))
            .collect(),
        palette: id_of(s, sf.palette),
        clipper: id_of(s, sf.clipper),
        root: sf.root,
        attached: sf.attached.iter().map(|&(id, _)| id).collect(),
        version: sf.version,
    }
}

/// The id of the object with the interface at `address`, or 0.
fn id_of(s: &State, address: u32) -> u32 {
    if address == 0 {
        return 0;
    }
    s.objects
        .iter()
        .position(|o| {
            o.as_ref()
                .is_some_and(|o| o.ifaces.iter().any(|i| i.address == address))
        })
        .map_or(0, |i| i as u32 + 1)
}

/// The pixels of surface `id` as RGBA rows. With `keyed`, the pixels its
/// source blit key matches are transparent. A surface on the 3dfx card,
/// in a compressed format or without RGB or palette pixels has no picture
/// here.
///
/// It only looks: a debugger asks for it between two instructions, and
/// the machine has to run on as if it hadn't. So no walk of the page
/// tables that a service does (it sets their accessed bits), and no read
/// of the program's memory to find the palette's interface.
pub fn image(cpu: &Cpu, id: u32, keyed: bool) -> Result<(u32, u32, Vec<u8>), String> {
    let s = &cpu.bus.win32.ddraw;
    let o = s.object(id).ok_or_else(|| format!("no surface {}", id))?;
    let sf = o
        .surface
        .as_ref()
        .ok_or_else(|| format!("no surface {}", id))?;
    if sf.memory == MEM_CARD {
        return Err("on the 3dfx card: /api/screenshot shows the card's picture".into());
    }
    if sf.format.flags & DDPF_FOURCC != 0 {
        return Err("a compressed format".into());
    }
    let paletted = sf.format.flags & DDPF_PALETTEINDEXED8 != 0 || sf.format.bits == 8;
    if !paletted && [sf.format.r, sf.format.g, sf.format.b].contains(&0) {
        return Err("no RGB masks".into());
    }
    let colors = if paletted { palette(s, sf) } else { Vec::new() };
    let bytes = sf.bytes_per_pixel() as usize;
    let row = sf.width as usize * bytes;
    let mut data = vec![0u8; row];
    let key = (keyed && sf.key_set & 1 << KEY_SRC_BLT != 0).then(|| sf.keys[KEY_SRC_BLT]);
    let mut rgba = Vec::with_capacity(sf.width as usize * sf.height as usize * 4);
    for y in 0..sf.height {
        read_row(cpu, o.pid, sf, y * sf.pitch, &mut data)?;
        for pixel in data.chunks_exact(bytes) {
            let mut v = [0u8; 4];
            v[..bytes].copy_from_slice(pixel);
            let value = u32::from_le_bytes(v);
            let [r, g, b] = if paletted {
                colors.get(value as usize).copied().unwrap_or([0; 3])
            } else {
                [
                    channel(value, sf.format.r),
                    channel(value, sf.format.g),
                    channel(value, sf.format.b),
                ]
            };
            let a = if key.is_some_and(|k| super::blt::keyed(value, k, sf)) {
                0
            } else {
                255
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    Ok((sf.width, sf.height, rgba))
}

/// Fill `data` with the surface's pixels at byte `offset`: in system
/// memory through the page tables of its process (`pid`), whether that
/// runs or not, and with the bus's side-effect-free reads.
fn read_row(cpu: &Cpu, pid: u32, sf: &Surface, offset: u32, data: &mut [u8]) -> Result<(), String> {
    match sf.memory {
        surface::MEM_VIDEO => crate::win32::screen::read(cpu, sf.bits + offset, data),
        surface::MEM_SYSTEM => {
            let process = cpu
                .bus
                .win32
                .processes
                .iter()
                .find(|p| p.id == pid)
                .ok_or_else(|| format!("process {:08X} is gone", pid))?;
            let start = sf.bits.wrapping_add(offset);
            let mut done = 0;
            while done < data.len() {
                let lin = start.wrapping_add(done as u32);
                let run = (0x1000 - (lin & 0xFFF) as usize).min(data.len() - done);
                let phys = process
                    .memory
                    .translate(&cpu.bus, lin)
                    .ok_or_else(|| format!("page at {:08X} isn't there", lin))?;
                for (i, byte) in data[done..done + run].iter_mut().enumerate() {
                    *byte = cpu.bus.peek_8(phys as usize + i);
                }
                done += run;
            }
        }
        _ => data.fill(0),
    }
    Ok(())
}

/// The 8-bit value of the channel `mask` of `value`.
fn channel(value: u32, mask: u32) -> u8 {
    let v = ((value & mask) >> mask.trailing_zeros()) as u64;
    let max = (mask >> mask.trailing_zeros()) as u64;
    (v * 255 / max) as u8
}

/// The colors of a palette surface: its own palette's, or else the
/// primary surface's, or else gray levels.
fn palette(s: &State, sf: &Surface) -> Vec<[u8; 3]> {
    let primary = s
        .objects
        .iter()
        .flatten()
        .filter_map(|o| o.surface.as_ref())
        .find(|s| s.is_primary() && s.palette != 0)
        .map_or(0, |s| s.palette);
    let pal = if sf.palette != 0 { sf.palette } else { primary };
    let colors = s
        .object(id_of(s, pal))
        .and_then(|o| o.palette.as_ref())
        .and_then(|p| p.colorrefs());
    match colors {
        Some(c) => c
            .iter()
            .map(|&c| [c as u8, (c >> 8) as u8, (c >> 16) as u8])
            .collect(),
        None => (0..=255).map(|i| [i, i, i]).collect(),
    }
}
