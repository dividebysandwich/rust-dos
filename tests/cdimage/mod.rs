//! Small CD images for the tests, made on the fly: an ISO 9660 file system
//! with a few files, stored as 2048-byte or raw 2352-byte sectors, audio
//! tracks with a known waveform, and the CUE sheets that tie them together.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

pub const SECTOR: usize = 2048;
pub const RAW: usize = 2352;

/// A fresh directory for one test's images.
pub fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from("target/test_cdrom").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(&dir).unwrap()
}

/// A file for `iso`: its path on the disc ("SUB\\FILE.TXT"), contents, and
/// whether it is hidden.
pub struct IsoFile<'a> {
    pub path: &'a str,
    pub data: &'a [u8],
    pub hidden: bool,
}

pub fn file<'a>(path: &'a str, data: &'a [u8]) -> IsoFile<'a> {
    IsoFile { path, data, hidden: false }
}

/// 31 October 1995, 12:00:00.
const DATE: [u8; 7] = [95, 10, 31, 12, 0, 0, 0];

fn both16(v: u16) -> [u8; 4] {
    let (le, be) = (v.to_le_bytes(), v.to_be_bytes());
    [le[0], le[1], be[0], be[1]]
}

fn both32(v: u32) -> [u8; 8] {
    let (le, be) = (v.to_le_bytes(), v.to_be_bytes());
    [le[0], le[1], le[2], le[3], be[0], be[1], be[2], be[3]]
}

fn record(name: &[u8], lba: u32, size: u32, flags: u8) -> Vec<u8> {
    let mut r = vec![0u8; 33];
    r[2..10].copy_from_slice(&both32(lba));
    r[10..18].copy_from_slice(&both32(size));
    r[18..25].copy_from_slice(&DATE);
    r[25] = flags;
    r[28..32].copy_from_slice(&both16(1));
    r[32] = name.len() as u8;
    r.extend_from_slice(name);
    if r.len() % 2 == 1 {
        r.push(0);
    }
    r[0] = r.len() as u8;
    r
}

fn parent(path: &str) -> &str {
    path.rsplit_once('\\').map_or("", |(p, _)| p)
}

fn leaf(path: &str) -> &str {
    path.rsplit_once('\\').map_or(path, |(_, l)| l)
}

/// An ISO 9660 image in 2048-byte sectors: the volume descriptors at 16
/// and 17, one sector per directory from 18 on, then the files.
pub fn iso(label: &str, files: &[IsoFile]) -> Vec<u8> {
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    dirs.insert(String::new());
    for f in files {
        let mut p = parent(f.path);
        while !p.is_empty() {
            dirs.insert(p.to_string());
            p = parent(p);
        }
    }
    let dirs: Vec<String> = dirs.into_iter().collect();
    let dir_lba = |d: &str| 18 + dirs.iter().position(|x| x == d).unwrap() as u32;
    let mut next = 18 + dirs.len() as u32;
    let mut extents = Vec::new();
    for f in files {
        extents.push(next);
        next += f.data.len().div_ceil(SECTOR).max(1) as u32;
    }
    let total = next as usize;
    let mut image = vec![0u8; total * SECTOR];

    // Directories: ".", "..", then the entries in name order.
    for d in &dirs {
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        for sub in dirs.iter().filter(|s| !s.is_empty() && parent(s) == d) {
            entries.push((leaf(sub).to_string(), record(leaf(sub).as_bytes(), dir_lba(sub), SECTOR as u32, 0x02)));
        }
        for (f, &lba) in files.iter().zip(&extents) {
            if parent(f.path) == d {
                let name = format!("{};1", leaf(f.path));
                let flags = if f.hidden { 0x01 } else { 0x00 };
                entries.push((leaf(f.path).to_string(), record(name.as_bytes(), lba, f.data.len() as u32, flags)));
            }
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut sector = record(&[0], dir_lba(d), SECTOR as u32, 0x02);
        sector.extend(record(&[1], dir_lba(parent(d)), SECTOR as u32, 0x02));
        for (_, r) in entries {
            sector.extend(r);
        }
        let at = dir_lba(d) as usize * SECTOR;
        image[at..at + sector.len()].copy_from_slice(&sector);
    }
    for (f, &lba) in files.iter().zip(&extents) {
        let at = lba as usize * SECTOR;
        image[at..at + f.data.len()].copy_from_slice(f.data);
    }

    let pvd = &mut image[16 * SECTOR..17 * SECTOR];
    pvd[0] = 1;
    pvd[1..6].copy_from_slice(b"CD001");
    pvd[6] = 1;
    pvd[40..72].fill(b' ');
    pvd[40..40 + label.len()].copy_from_slice(label.as_bytes());
    pvd[80..88].copy_from_slice(&both32(total as u32));
    pvd[120..124].copy_from_slice(&both16(1));
    pvd[124..128].copy_from_slice(&both16(1));
    pvd[128..132].copy_from_slice(&both16(SECTOR as u16));
    let root = record(&[0], 18, SECTOR as u32, 0x02);
    pvd[156..156 + 34].copy_from_slice(&root[..34]);
    pvd[702..739].fill(b' ');
    pvd[702..717].copy_from_slice(b"COPYRIGHT.TXT;1");
    let end = &mut image[17 * SECTOR..18 * SECTOR];
    end[0] = 0xFF;
    end[1..6].copy_from_slice(b"CD001");
    end[6] = 1;
    image
}

/// 2048-byte sectors as whole Mode 1 sectors, with sync and header.
pub fn mode1_2352(iso: &[u8]) -> Vec<u8> {
    let mut raw = Vec::new();
    for (lba, sector) in iso.chunks(SECTOR).enumerate() {
        let frames = lba as u32 + 150;
        let bcd = |v: u32| ((v / 10) << 4 | v % 10) as u8;
        raw.extend([0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        raw.extend([bcd(frames / 75 / 60), bcd(frames / 75 % 60), bcd(frames % 75), 1]);
        raw.extend(sector);
        raw.extend([0u8; 288]);
    }
    raw
}

/// The sample pair `i` of the test audio: a ramp on the left channel and
/// its negation on the right.
pub fn audio_sample(i: usize) -> (i16, i16) {
    let v = ((i * 37) % 20_000) as i16;
    (v, -v)
}

/// `sectors` sectors of the test audio, 588 sample pairs each.
pub fn audio(sectors: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(sectors * RAW);
    for i in 0..sectors * 588 {
        let (l, r) = audio_sample(i);
        data.extend(l.to_le_bytes());
        data.extend(r.to_le_bytes());
    }
    data
}

/// A disc of a data track (`iso` as Mode 1/2352 in GAME.BIN) and an audio
/// track of `audio_sectors` in its own file, TRACK2.BIN, with a two-second
/// pregap. Returns the CUE sheet's path.
pub fn mixed_disc(dir: &std::path::Path, iso_image: &[u8], audio_sectors: usize) -> PathBuf {
    fs::write(dir.join("GAME.BIN"), mode1_2352(iso_image)).unwrap();
    fs::write(dir.join("Track2.bin"), audio(audio_sectors)).unwrap();
    let cue = dir.join("GAME.CUE");
    fs::write(
        &cue,
        "FILE \"GAME.BIN\" BINARY\r\n  TRACK 01 MODE1/2352\r\n    INDEX 01 00:00:00\r\n\
         FILE \"TRACK2.BIN\" BINARY\r\n  TRACK 02 AUDIO\r\n    PREGAP 00:02:00\r\n    INDEX 01 00:00:00\r\n",
    )
    .unwrap();
    cue
}
