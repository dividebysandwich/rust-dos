//! Save state files, as the slots keep them: `RDOSSTAT`, the format, a
//! header saying what was saved, when, and of which machine (JSON), a
//! small picture of the screen (PNG), and the machine's state (see
//! machine.rs), deflated.
//!
//! The desktop keeps a game's slots in `states/<game>` beside the
//! configuration file, and those of the machine without a game in
//! `states/dos`; the web page keeps the same bytes in its browser storage.

use crate::config::Settings;
use crate::cpu::Cpu;
use crate::video::Frame;
use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const MAGIC: &[u8; 8] = b"RDOSSTAT";
/// The version of the file's layout (not of the machine's state, whose
/// sections have their own).
pub const FORMAT: u16 = 1;
/// The slots a game has.
pub const SLOTS: u8 = 9;
/// The size of the picture of the screen.
pub const THUMBNAIL: (u32, u32) = (160, 100);

/// What a saved state is of.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Header {
    /// The rust-dos that saved it.
    pub version: String,
    /// When, in local time: `YYYY-MM-DD HH:MM:SS`.
    pub saved: String,
    /// The game that was launched from its profile, if one was: its id
    /// and name.
    pub game: Option<String>,
    pub game_name: Option<String>,
    /// The program that ran, as DOS started it (`KEEN4E.EXE`); empty at
    /// the prompt.
    pub program: String,
    /// The machine's hardware settings, as configuration text (see
    /// `machine_text`).
    pub machine: String,
    /// Its memory in MB, which the machine loading it must have.
    pub memsize: usize,
    /// Its emulated time.
    pub emulated_ns: u64,
}

/// A slot file: the header, the picture and the state.
pub fn encode(header: &Header, thumbnail: &[u8], state: &[u8]) -> Vec<u8> {
    let json = serde_json::to_vec(header).expect("the header is JSON");
    let mut out = Vec::with_capacity(state.len() / 4);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT.to_le_bytes());
    for part in [&json[..], thumbnail] {
        out.extend_from_slice(&(part.len() as u32).to_le_bytes());
        out.extend_from_slice(part);
    }
    let mut deflate = DeflateEncoder::new(out, Compression::fast());
    deflate.write_all(state).expect("writing to memory");
    deflate.finish().expect("writing to memory")
}

/// The header and the picture of a slot file, and where its state starts.
pub fn read_header(data: &[u8]) -> Result<(Header, &[u8], usize), String> {
    let bad = || "not a rust-dos save state".to_string();
    if data.get(..8) != Some(&MAGIC[..]) {
        return Err(bad());
    }
    let format = u16::from_le_bytes(data.get(8..10).ok_or_else(bad)?.try_into().unwrap());
    if format != FORMAT {
        return Err(format!("a save state of a newer rust-dos (format {})", format));
    }
    let mut at = 10;
    let mut part = || -> Result<&[u8], String> {
        let len = u32::from_le_bytes(data.get(at..at + 4).ok_or_else(bad)?.try_into().unwrap()) as usize;
        let bytes = data.get(at + 4..at + 4 + len).ok_or_else(bad)?;
        at += 4 + len;
        Ok(bytes)
    };
    let json = part()?;
    let thumbnail = part()?;
    let header = serde_json::from_slice(json).map_err(|e| format!("a damaged save state: {}", e))?;
    Ok((header, thumbnail, at))
}

/// The header and the machine's state of a slot file.
pub fn decode(data: &[u8]) -> Result<(Header, Vec<u8>), String> {
    let (header, _, at) = read_header(data)?;
    let mut state = Vec::with_capacity(header.memsize << 20);
    DeflateDecoder::new(&data[at..]).read_to_end(&mut state).map_err(|e| format!("a damaged save state: {}", e))?;
    Ok((header, state))
}

/// The settings of the hardware a state is of, as configuration text:
/// the processor, the memory, the sound, the display adapter and its
/// monitor, and the speed. What differs from the defaults is written.
pub fn machine_text(settings: &Settings) -> String {
    let hardware = with_machine(&Settings::default(), settings);
    crate::config::update_text("", &Settings::default(), &hardware, &[], None)
}

/// `settings` with the hardware settings `text` (see `machine_text`) has.
pub fn machine_settings(text: &str, settings: &Settings) -> Settings {
    let config = crate::config::parse(text, Path::new("."), None);
    with_machine(settings, &Settings::from_config(&config))
}

/// `base` with the hardware settings of `from`.
fn with_machine(base: &Settings, from: &Settings) -> Settings {
    Settings {
        machine: from.machine,
        monochrome: from.monochrome,
        cycles: from.cycles,
        cpu: from.cpu,
        memsize: from.memsize,
        ems: from.ems,
        umb: from.umb,
        sound: from.sound.clone(),
        ..base.clone()
    }
}

/// A small picture of `frame`, as PNG.
pub fn thumbnail(frame: &Frame) -> Vec<u8> {
    let (w, h) = THUMBNAIL;
    let mut small = Frame::new(w, h);
    if frame.width > 0 && frame.height > 0 {
        // Each pixel the average of the frame's pixels it covers.
        for y in 0..h {
            let (y0, y1) = (y * frame.height / h, ((y + 1) * frame.height / h).max(y * frame.height / h + 1));
            for x in 0..w {
                let (x0, x1) = (x * frame.width / w, ((x + 1) * frame.width / w).max(x * frame.width / w + 1));
                let mut sum = [0u32; 3];
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let i = ((sy * frame.width + sx) * 3) as usize;
                        for (total, &value) in sum.iter_mut().zip(&frame.rgb[i..i + 3]) {
                            *total += value as u32;
                        }
                    }
                }
                let n = (y1 - y0) * (x1 - x0);
                let o = ((y * w + x) * 3) as usize;
                for (pixel, total) in small.rgb[o..o + 3].iter_mut().zip(sum) {
                    *pixel = (total / n) as u8;
                }
            }
        }
    }
    crate::capture::png::encode(&small).unwrap_or_default()
}

/// The header of a state saved now of `cpu`, with `settings`, while the
/// game `game` (id and name) plays.
pub fn header(cpu: &Cpu, settings: &Settings, game: Option<(&str, &str)>) -> Header {
    Header {
        version: env!("CARGO_PKG_VERSION").to_string(),
        saved: crate::hosttime::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        game: game.map(|(id, _)| id.to_string()),
        game_name: game.map(|(_, name)| name.to_string()),
        program: cpu.program.clone(),
        machine: machine_text(settings),
        memsize: cpu.bus.ram().len() >> 20,
        emulated_ns: cpu.bus.clock.now_ns(),
    }
}

/// Why a state of `header` can't be loaded into a machine with `memsize`
/// MB, if it can't: memory can't change size while the machine runs.
pub fn refusal(header: &Header, memsize: usize) -> Option<String> {
    (header.memsize != memsize).then(|| {
        format!("it is of a machine with {} MB of memory, this one has {} MB (memsize)", header.memsize, memsize)
    })
}

/// The folder of the slots of `game`, or of the machine without a game,
/// in `states`.
pub fn slot_dir(states: &Path, game: Option<&str>) -> PathBuf {
    states.join(game.unwrap_or("dos"))
}

/// The file of slot `slot` (1 to `SLOTS`) in `dir`.
pub fn slot_path(dir: &Path, slot: u8) -> PathBuf {
    dir.join(format!("slot{}.state", slot))
}

/// The header and picture of each filled slot in `dir`.
pub fn list(dir: &Path) -> Vec<(u8, Header, Vec<u8>)> {
    (1..=SLOTS)
        .filter_map(|slot| {
            let data = std::fs::read(slot_path(dir, slot)).ok()?;
            let (header, thumbnail, _) = read_header(&data).ok()?;
            Some((slot, header, thumbnail.to_vec()))
        })
        .collect()
}

/// The header of the slot file at `path`, if there is one: read from the
/// start of the file, without its state.
pub fn read_file_header(path: &Path) -> Option<Header> {
    let mut start = Vec::new();
    std::fs::File::open(path).ok()?.take(1 << 20).read_to_end(&mut start).ok()?;
    read_header(&start).ok().map(|(header, _, _)| header)
}

/// Write a slot file, replacing the old one in one step.
pub fn write_file(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    }
    let partial = path.with_extension("partial");
    std::fs::write(&partial, data).map_err(|e| format!("{}: {}", partial.display(), e))?;
    std::fs::rename(&partial, path).map_err(|e| format!("{}: {}", path.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::CpuModel;
    use crate::video::adapter::Adapter;

    #[test]
    fn a_slot_file_reads_back() {
        let header = Header { version: "1".into(), program: "KEEN4E.EXE".into(), memsize: 16, ..Default::default() };
        let state: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let data = encode(&header, b"picture", &state);
        let (read, picture, _) = read_header(&data).unwrap();
        assert_eq!((read, picture), (header.clone(), &b"picture"[..]));
        assert_eq!(decode(&data).unwrap(), (header, state));
        assert!(decode(b"RDOSSTAT\x02\x00").unwrap_err().contains("newer"));
        assert!(decode(b"something else").is_err());
    }

    #[test]
    fn the_hardware_settings_come_back() {
        let mut settings = Settings { machine: Adapter::Cga, cpu: CpuModel::I386, memsize: 8, ems: false, ..Default::default() };
        settings.sound.gus.enabled = false;
        settings.scale = 3;
        let text = machine_text(&settings);
        let current = Settings { scale: 2, ..Default::default() };
        let loaded = machine_settings(&text, &current);
        assert_eq!((loaded.machine, loaded.cpu, loaded.memsize, loaded.ems), (Adapter::Cga, CpuModel::I386, 8, false));
        assert_eq!(loaded.sound, settings.sound);
        assert_eq!(loaded.scale, 2, "the display's settings stay");
    }

    #[test]
    fn a_thumbnail_is_the_frame_made_small() {
        let mut frame = Frame::new(640, 400);
        frame.rgb[..640 * 200 * 3].fill(0xFF);
        let png = thumbnail(&frame);
        let decoder = png::Decoder::new(std::io::Cursor::new(png));
        let mut reader = decoder.read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut pixels).unwrap();
        assert_eq!((reader.info().width, reader.info().height), THUMBNAIL);
        assert_eq!((pixels[0], pixels[pixels.len() - 1]), (0xFF, 0));
    }
}
