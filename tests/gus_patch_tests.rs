//! The General MIDI synthesizer with the Gravis patch set built into
//! rust-dos. `renders_a_midi_file` plays one of the MIDI files of an
//! Ultrasound install, so it is ignored by default:
//!
//!   ULTRASND_DIR=~/Games/DOS/ULTRASND cargo test --release --test gus_patch_tests -- --ignored --nocapture
//!
//! It writes `target/gus_midi.wav` for listening (`GUS_MIDI` picks the
//! file, relative to the MIDI directory).

use rust_dos::gus::builtin;
use rust_dos::gus::patch::{self, PatchBank};
use rust_dos::gus::synth::GusSynth;
use std::path::{Path, PathBuf};

fn ultrasnd_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("ULTRASND_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join("Games/DOS/ULTRASND")))?;
    dir.join("MIDI").is_dir().then_some(dir)
}

fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| {
        p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case(name))
    })
}

#[test]
fn every_patch_parses() {
    let mut count = 0;
    for (path, bytes) in builtin::FILES.iter().filter(|(p, _)| p.ends_with(".PAT")) {
        let p = patch::parse(bytes).unwrap_or_else(|e| panic!("{}: {}", path, e));
        for s in &p.samples {
            assert!(!s.data.is_empty(), "{}", path);
            let mean = s.data.iter().map(|&v| v as f64).sum::<f64>() / s.data.len() as f64;
            assert!(mean.abs() < 4000.0, "{}: DC {}", path, mean);
        }
        count += 1;
    }
    assert!(count > 100, "{} patches", count);
}

/// A Standard MIDI File as (seconds, event bytes) in time order.
fn read_smf(bytes: &[u8]) -> Vec<(f64, Vec<u8>)> {
    let be16 = |b: &[u8]| u16::from_be_bytes([b[0], b[1]]) as usize;
    let be32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
    assert_eq!(&bytes[..4], b"MThd");
    let tracks = be16(&bytes[10..]);
    let division = be16(&bytes[12..]);
    let mut at = 8 + be32(&bytes[4..]);
    // (tick, order, event); tempo changes are events with status FF 51.
    let mut events: Vec<(u64, usize, Vec<u8>)> = Vec::new();
    for _ in 0..tracks {
        assert_eq!(&bytes[at..at + 4], b"MTrk");
        let len = be32(&bytes[at + 4..]);
        let track = &bytes[at + 8..at + 8 + len];
        at += 8 + len;
        let mut i = 0;
        let mut tick = 0u64;
        let mut running = 0u8;
        let varlen = |i: &mut usize| {
            let mut v = 0usize;
            loop {
                let b = track[*i];
                *i += 1;
                v = (v << 7) | (b & 0x7F) as usize;
                if b & 0x80 == 0 {
                    return v;
                }
            }
        };
        while i < track.len() {
            tick += varlen(&mut i) as u64;
            let mut status = track[i];
            if status & 0x80 != 0 {
                i += 1;
            } else {
                status = running;
            }
            match status {
                0xFF => {
                    let kind = track[i];
                    i += 1;
                    let n = varlen(&mut i);
                    if kind == 0x51 {
                        events.push((tick, events.len(), vec![0xFF, 0x51, track[i], track[i + 1], track[i + 2]]));
                    }
                    i += n;
                }
                0xF0 | 0xF7 => {
                    let n = varlen(&mut i);
                    let mut e = vec![0xF0];
                    e.extend_from_slice(&track[i..i + n]);
                    events.push((tick, events.len(), e));
                    i += n;
                }
                _ => {
                    running = status;
                    let n = if matches!(status & 0xF0, 0xC0 | 0xD0) { 1 } else { 2 };
                    let mut e = vec![status];
                    e.extend_from_slice(&track[i..i + n]);
                    events.push((tick, events.len(), e));
                    i += n;
                }
            }
        }
    }
    events.sort_by_key(|(tick, order, _)| (*tick, *order));
    let mut out = Vec::new();
    let (mut last_tick, mut seconds, mut tempo) = (0u64, 0.0f64, 500_000.0f64);
    for (tick, _, e) in events {
        seconds += (tick - last_tick) as f64 * tempo / 1e6 / division as f64;
        last_tick = tick;
        if e[0] == 0xFF {
            tempo = ((e[2] as u32) << 16 | (e[3] as u32) << 8 | e[4] as u32) as f64;
        } else {
            out.push((seconds, e));
        }
    }
    out
}

fn write_wav(path: &Path, samples: &[i16]) {
    let mut f = Vec::with_capacity(44 + samples.len() * 2);
    let data = (samples.len() * 2) as u32;
    f.extend_from_slice(b"RIFF");
    f.extend_from_slice(&(36 + data).to_le_bytes());
    f.extend_from_slice(b"WAVEfmt ");
    f.extend_from_slice(&16u32.to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&2u16.to_le_bytes());
    f.extend_from_slice(&44100u32.to_le_bytes());
    f.extend_from_slice(&(44100u32 * 4).to_le_bytes());
    f.extend_from_slice(&4u16.to_le_bytes());
    f.extend_from_slice(&16u16.to_le_bytes());
    f.extend_from_slice(b"data");
    f.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        f.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, f).unwrap();
}

#[test]
#[ignore]
fn renders_a_midi_file() {
    let Some(dir) = ultrasnd_dir() else { return };
    let midi_dir = dir.join("MIDI");
    let mut synth = GusSynth::new(PatchBank::builtin());
    let name = std::env::var("GUS_MIDI").unwrap_or_else(|_| "HERO.MID".to_string());
    let events = read_smf(&std::fs::read(find(&midi_dir, &name).expect("MIDI file")).unwrap());
    let seconds = events.last().map_or(0.0, |(t, _)| *t).min(60.0) + 2.0;

    let mut out: Vec<i16> = Vec::new();
    let (mut peak, mut clipped, mut sum) = (0.0f32, 0usize, 0.0f64);
    let mut next = events.iter().peekable();
    let frames = (seconds * 44100.0) as usize;
    for frame in 0..frames {
        let t = frame as f64 / 44100.0;
        while let Some((at, e)) = next.peek() {
            if *at > t {
                break;
            }
            if e[0] == 0xF0 {
                synth.sysex(&e[1..]);
            } else {
                synth.message(e[0], e[1], e.get(2).copied().unwrap_or(0));
            }
            next.next();
        }
        let (l, r) = synth.render();
        for v in [l, r] {
            peak = peak.max(v.abs());
            sum += (v as f64) * (v as f64);
            if v.abs() > 32767.0 {
                clipped += 1;
            }
            out.push(v.clamp(-32768.0, 32767.0) as i16);
        }
    }
    let rms = (sum / out.len() as f64).sqrt();
    let wav = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/gus_midi.wav");
    write_wav(&wav, &out);
    println!(
        "{}: {:.1} s, peak {:.0}, RMS {:.0}, clipped {:.3}%, missing {:?} -> {}",
        name,
        seconds,
        peak,
        rms,
        clipped as f64 * 100.0 / out.len() as f64,
        synth.missing_patches(),
        wav.display()
    );
    assert!(rms > 300.0, "RMS {}", rms);
    assert!(synth.missing_patches().is_empty());
}
