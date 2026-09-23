//! Ultrasound patches (`.PAT`, "GF1PATCH110"), the instrument files of the
//! Gravis MIDI patch set, and the bank that maps General MIDI programs and
//! drum keys to them (`ULTRASND.INI`).
//!
//! A patch holds one or more samples, each covering a range of notes, with
//! its loop, its root frequency and a six-point volume envelope in the
//! GF1's volume ramp units.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const MODE_16BIT: u8 = 0x01;
pub const MODE_UNSIGNED: u8 = 0x02;
pub const MODE_LOOP: u8 = 0x04;
pub const MODE_BIDIRECTIONAL: u8 = 0x08;
pub const MODE_REVERSE: u8 = 0x10;
pub const MODE_SUSTAIN: u8 = 0x20;
pub const MODE_ENVELOPE: u8 = 0x40;

const HEADER: usize = 129;
const INSTRUMENT: usize = 63;
const LAYER: usize = 47;
const SAMPLE: usize = 96;

/// One sample of a patch, converted to signed 16-bit.
#[derive(Debug, Clone)]
pub struct PatchSample {
    pub data: Vec<i16>,
    /// Loop points in samples, with fractions.
    pub loop_start: f64,
    pub loop_end: f64,
    pub rate: u32,
    /// Note range and the frequency the sample plays at unchanged, in mHz.
    pub low_freq: u32,
    pub high_freq: u32,
    pub root_freq: u32,
    /// Pan position, 0 (left) to 15.
    pub balance: u8,
    pub env_rate: [u8; 6],
    pub env_offset: [u8; 6],
    /// Sweep, rate and depth.
    pub tremolo: [u8; 3],
    pub vibrato: [u8; 3],
    pub modes: u8,
    /// Keyboard scaling: the note that plays at the root frequency, and
    /// how far pitch follows the key (1024 = a semitone a key, 0 = fixed).
    pub scale_freq: i16,
    pub scale_factor: u16,
}

#[derive(Debug, Clone)]
pub struct Patch {
    pub samples: Vec<PatchSample>,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Parse a patch file.
pub fn parse(bytes: &[u8]) -> Result<Patch, String> {
    if bytes.len() < HEADER + INSTRUMENT + LAYER || !bytes.starts_with(b"GF1PATCH1") {
        return Err("not a GF1 patch".to_string());
    }
    let count = bytes[HEADER + INSTRUMENT + 6] as usize;
    let mut at = HEADER + INSTRUMENT + LAYER;
    let mut samples = Vec::with_capacity(count);
    for _ in 0..count {
        let h = bytes.get(at..at + SAMPLE).ok_or("truncated sample header")?;
        let size = u32_at(h, 8) as usize;
        let raw = bytes.get(at + SAMPLE..at + SAMPLE + size).ok_or("truncated sample data")?;
        samples.push(convert(h, raw));
        at += SAMPLE + size;
    }
    if samples.is_empty() {
        return Err("patch has no samples".to_string());
    }
    Ok(Patch { samples })
}

fn convert(h: &[u8], raw: &[u8]) -> PatchSample {
    let fractions = h[7];
    let mut modes = h[55];
    let bits16 = modes & MODE_16BIT != 0;
    let unsigned = modes & MODE_UNSIGNED != 0;
    let mut data: Vec<i16> = if bits16 {
        raw.as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                let v = u16::from_le_bytes([c[0], c[1]]);
                (if unsigned { v ^ 0x8000 } else { v }) as i16
            })
            .collect()
    } else {
        raw.iter()
            .map(|&b| ((if unsigned { b ^ 0x80 } else { b }) as i8 as i16) << 8)
            .collect()
    };
    let unit = if bits16 { 2.0 } else { 1.0 };
    let mut loop_start = u32_at(h, 12) as f64 / unit + (fractions & 0x0F) as f64 / 16.0;
    let mut loop_end = u32_at(h, 16) as f64 / unit + (fractions >> 4) as f64 / 16.0;
    let len = data.len() as f64;
    if modes & MODE_REVERSE != 0 {
        data.reverse();
        (loop_start, loop_end) = (len - loop_end, len - loop_start);
        modes &= !MODE_REVERSE;
    }
    loop_end = loop_end.clamp(0.0, len);
    loop_start = loop_start.clamp(0.0, loop_end);

    let mut env_rate = [0; 6];
    env_rate.copy_from_slice(&h[37..43]);
    let mut env_offset = [0; 6];
    env_offset.copy_from_slice(&h[43..49]);

    // TiMidity's reading of the envelope flags, which suits the Gravis
    // set: looped samples sustain; unlooped ones play out without an
    // envelope; odd envelopes (all rates at maximum, a high final level)
    // and envelopes without sustain are dropped.
    if modes & MODE_LOOP != 0 {
        modes |= MODE_SUSTAIN;
    }
    if modes & (MODE_LOOP | MODE_BIDIRECTIONAL) == 0 || loop_end <= loop_start {
        modes &= !(MODE_SUSTAIN | MODE_ENVELOPE | MODE_LOOP | MODE_BIDIRECTIONAL);
    } else if env_rate.iter().all(|&r| r == 63) || env_offset[5] >= 100 || modes & MODE_SUSTAIN == 0 {
        modes &= !MODE_ENVELOPE;
    }

    PatchSample {
        data,
        loop_start,
        loop_end,
        rate: u16_at(h, 20) as u32,
        low_freq: u32_at(h, 22),
        high_freq: u32_at(h, 26),
        root_freq: u32_at(h, 30).max(1),
        balance: h[36] & 0x0F,
        env_rate,
        env_offset,
        tremolo: [h[49], h[50], h[51]],
        vibrato: [h[52], h[53], h[54]],
        modes,
        scale_freq: u16_at(h, 56) as i16,
        scale_factor: u16_at(h, 58),
    }
}

impl Patch {
    /// The sample for a note of `freq` mHz: the one whose range holds it,
    /// else the one with the nearest root.
    pub fn sample_for(&self, freq: u32) -> &PatchSample {
        self.samples
            .iter()
            .find(|s| s.low_freq <= freq && freq <= s.high_freq)
            .unwrap_or_else(|| {
                self.samples
                    .iter()
                    .min_by_key(|s| (s.root_freq as i64 - freq as i64).unsigned_abs())
                    .expect("a patch has samples")
            })
    }
}

/// The patch set: General MIDI programs and drum keys mapped to patch
/// files, loaded when first played.
pub struct PatchBank {
    /// Patch files by lower-case name without extension.
    files: HashMap<String, PathBuf>,
    melodic: Vec<Option<String>>,
    drums: Vec<Option<String>>,
    cache: HashMap<String, Option<Arc<Patch>>>,
    /// Patches the bank names but that could not be loaded.
    pub missing: Vec<String>,
}

impl PatchBank {
    /// A bank from the text of `ULTRASND.INI`, with the patches in `dir`.
    pub fn from_ini(ini: &str, dir: &Path) -> Self {
        let mut files = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_pat = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pat"));
                if let (true, Some(stem)) = (is_pat, path.file_stem().and_then(|s| s.to_str())) {
                    files.insert(stem.to_ascii_lowercase(), path);
                }
            }
        }
        let section = |names: &[&str]| -> Vec<Option<String>> {
            let mut map = vec![None; 128];
            for name in names {
                let entries = ini_section(ini, name);
                if entries.is_empty() {
                    continue;
                }
                for (key, value) in entries {
                    if let Ok(n) = key.parse::<usize>()
                        && n < 128
                        && !value.is_empty()
                    {
                        map[n] = Some(value.to_ascii_lowercase());
                    }
                }
                break;
            }
            map
        };
        Self {
            files,
            melodic: section(&["Melodic Bank 0", "Melodic Patches"]),
            drums: section(&["Drum Bank 0", "Drum Patches"]),
            cache: HashMap::new(),
            missing: Vec::new(),
        }
    }

    /// The bank of the Ultrasound software in DOS directory `ultradir`
    /// (ULTRADIR) on the mounted drives: `ULTRASND.INI` there, and the
    /// patches in the directory it names or else in its MIDI directory.
    /// Also returns the host directory of the patches.
    pub fn from_dos_dir(disk: &crate::disk::DiskController, ultradir: &str) -> Result<(Self, PathBuf), String> {
        let dir = ultradir.trim_end_matches('\\');
        let ini_path = disk
            .resolve_path(&format!("{}\\ULTRASND.INI", dir))
            .filter(|p| p.is_file())
            .ok_or_else(|| format!("no {}\\ULTRASND.INI", dir))?;
        let ini = std::fs::read(&ini_path).map_err(|e| format!("{}: {}", ini_path.display(), e))?;
        let ini = String::from_utf8_lossy(&ini);
        let patches = Self::patch_dir(&ini)
            .and_then(|d| disk.resolve_path(d.trim_end_matches('\\')))
            .filter(|p| p.is_dir())
            .or_else(|| disk.resolve_path(&format!("{}\\MIDI", dir)).filter(|p| p.is_dir()))
            .ok_or_else(|| format!("no patch directory for {}", ini_path.display()))?;
        let bank = Self::from_ini(&ini, &patches);
        if bank.file_count() == 0 {
            return Err(format!("no patches in {}", patches.display()));
        }
        Ok((bank, patches))
    }

    /// The DOS directory `ULTRASND.INI` puts the patches in, if it says.
    pub fn patch_dir(ini: &str) -> Option<String> {
        ["Melodic Bank 0", "Ultrasound"].iter().find_map(|section| {
            ini_section(ini, section)
                .into_iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("PatchDir"))
                .map(|(_, v)| v)
                .filter(|v| !v.is_empty())
        })
    }

    /// Number of patch files found.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn melodic(&mut self, program: u8) -> Option<Arc<Patch>> {
        let name = self.melodic.get(program as usize)?.clone()?;
        self.load(&name)
    }

    pub fn drum(&mut self, key: u8) -> Option<Arc<Patch>> {
        let name = self.drums.get(key as usize)?.clone()?;
        self.load(&name)
    }

    fn load(&mut self, name: &str) -> Option<Arc<Patch>> {
        if let Some(patch) = self.cache.get(name) {
            return patch.clone();
        }
        let patch = self
            .files
            .get(name)
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| parse(&bytes).ok())
            .map(Arc::new);
        if patch.is_none() {
            self.missing.push(name.to_string());
        }
        self.cache.insert(name.to_string(), patch.clone());
        patch
    }
}

/// The `key=value` lines of an INI section.
fn ini_section(ini: &str, name: &str) -> Vec<(String, String)> {
    let mut inside = false;
    let mut entries = Vec::new();
    for line in ini.lines() {
        let line = line.trim();
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            inside = section.trim().eq_ignore_ascii_case(name);
            continue;
        }
        if inside
            && !line.starts_with('#')
            && let Some((k, v)) = line.split_once('=')
        {
            entries.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    entries
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A test sample: data, loop start, loop end, modes, root, low and
    /// high frequency (mHz).
    pub(crate) type Sample = (Vec<i16>, u32, u32, u8, u32, u32, u32);

    /// A patch file with one 16-bit unsigned sample per entry of `samples`.
    pub(crate) fn build(samples: &[Sample]) -> Vec<u8> {
        let mut f = vec![0u8; HEADER + INSTRUMENT + LAYER];
        f[..12].copy_from_slice(b"GF1PATCH110\0");
        f[HEADER + INSTRUMENT + 6] = samples.len() as u8;
        for (data, ls, le, modes, root, low, high) in samples {
            let mut h = [0u8; SAMPLE];
            h[8..12].copy_from_slice(&(data.len() as u32 * 2).to_le_bytes());
            h[12..16].copy_from_slice(&(ls * 2).to_le_bytes());
            h[16..20].copy_from_slice(&(le * 2).to_le_bytes());
            h[20..22].copy_from_slice(&44100u16.to_le_bytes());
            h[22..26].copy_from_slice(&low.to_le_bytes());
            h[26..30].copy_from_slice(&high.to_le_bytes());
            h[30..34].copy_from_slice(&root.to_le_bytes());
            h[36] = 7;
            // Fast attack to full, hold, fast release.
            h[37..43].copy_from_slice(&[63, 63, 63, 20, 63, 63]);
            h[43..49].copy_from_slice(&[250, 250, 250, 10, 10, 10]);
            h[55] = modes | MODE_16BIT | MODE_UNSIGNED;
            h[56..58].copy_from_slice(&60i16.to_le_bytes());
            h[58..60].copy_from_slice(&1024u16.to_le_bytes());
            f.extend_from_slice(&h);
            for &s in data {
                f.extend_from_slice(&((s as u16) ^ 0x8000).to_le_bytes());
            }
        }
        f
    }

    pub(crate) fn sine(len: usize, period: usize) -> Vec<i16> {
        (0..len)
            .map(|i| ((i as f64 / period as f64 * std::f64::consts::TAU).sin() * 20000.0) as i16)
            .collect()
    }

    #[test]
    fn parses_and_converts_unsigned_samples() {
        let file = build(&[(sine(100, 100), 0, 100, MODE_LOOP | MODE_ENVELOPE, 440_000, 0, 1_000_000)]);
        let patch = parse(&file).unwrap();
        let s = &patch.samples[0];
        assert_eq!(s.data.len(), 100);
        assert_eq!(s.data[25], sine(100, 100)[25]);
        assert_eq!((s.loop_start, s.loop_end), (0.0, 100.0));
        // Looped: sustained, envelope kept.
        assert_eq!(s.modes & (MODE_SUSTAIN | MODE_ENVELOPE | MODE_LOOP), MODE_SUSTAIN | MODE_ENVELOPE | MODE_LOOP);
    }

    #[test]
    fn unlooped_samples_lose_the_envelope() {
        let file = build(&[(sine(10, 10), 0, 0, MODE_ENVELOPE | MODE_SUSTAIN, 440_000, 0, 1_000_000)]);
        let s = &parse(&file).unwrap().samples[0];
        assert_eq!(s.modes & (MODE_SUSTAIN | MODE_ENVELOPE), 0);
    }

    #[test]
    fn reversed_samples_are_turned_around() {
        let data: Vec<i16> = (0..10).map(|i| i * 100).collect();
        let file = build(&[(data, 2, 6, MODE_REVERSE | MODE_LOOP, 440_000, 0, 1_000_000)]);
        let s = &parse(&file).unwrap().samples[0];
        assert_eq!(s.data[0], 900);
        assert_eq!((s.loop_start, s.loop_end), (4.0, 8.0));
    }

    #[test]
    fn picks_the_sample_for_a_note() {
        let file = build(&[
            (sine(10, 10), 0, 10, MODE_LOOP, 200_000, 0, 300_000),
            (sine(10, 10), 0, 10, MODE_LOOP, 500_000, 300_001, 800_000),
        ]);
        let patch = parse(&file).unwrap();
        assert_eq!(patch.sample_for(440_000).root_freq, 500_000);
        assert_eq!(patch.sample_for(100_000).root_freq, 200_000);
        assert_eq!(patch.sample_for(5_000_000).root_freq, 500_000);
    }

    #[test]
    fn bank_sections() {
        let ini = "[Ultrasound]\nPatchDir=C:\\ULTRASND\\MIDI\\\n[Melodic Bank 0]\nPatchDir=D:\\PAT\\\n0=acpiano\n1=britepno\n[Drum Bank 0]\n35=kick1\n[Melodic Patches]\n0=other\n";
        let bank = PatchBank::from_ini(ini, Path::new("/nonexistent"));
        assert_eq!(bank.melodic[0].as_deref(), Some("acpiano"));
        assert_eq!(bank.melodic[1].as_deref(), Some("britepno"));
        assert_eq!(bank.drums[35].as_deref(), Some("kick1"));
        assert_eq!(PatchBank::patch_dir(ini).as_deref(), Some("D:\\PAT\\"));
    }
}
