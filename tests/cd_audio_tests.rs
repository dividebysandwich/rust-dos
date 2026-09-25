//! CD images whose audio tracks are compressed files, as GOG's CUE sheets
//! (.ins) have them: Ogg Vorbis, FLAC and MP3 decoded to the CD's 16-bit
//! stereo samples at 44.1 kHz, compared with the same tone as a WAVE file.
//! The files in tests/cdimage/audio hold 0.3 s of 440 Hz on the left and
//! 660 Hz on the right.
#![cfg(feature = "cdaudio")]

use rust_dos::cdrom::image::CdImage;
use std::fs;
use std::path::{Path, PathBuf};

const AUDIO: &str = "tests/cdimage/audio";

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from("target/test_cd_audio").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A CUE sheet `sheet` (named `name`) with the audio file `file` next to it.
fn image(dir: &Path, name: &str, file: &str, keyword: &str) -> CdImage {
    fs::copy(Path::new(AUDIO).join(file), dir.join(file)).unwrap();
    let sheet = format!("FILE \"{}\" {}\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n", file, keyword);
    fs::write(dir.join(name), sheet).unwrap();
    CdImage::open(&dir.join(name)).unwrap()
}

/// Every frame of the disc's first `sectors` sectors.
fn frames(image: &CdImage, sectors: u32) -> Vec<(i16, i16)> {
    let mut out = Vec::new();
    for lba in 0..sectors {
        image.read_audio(lba, &mut out).unwrap();
    }
    out
}

/// The root mean square of the difference, in parts of the full scale.
fn rms_difference(a: &[(i16, i16)], b: &[(i16, i16)]) -> f64 {
    let sum: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| {
            let (l, r) = ((x.0 - y.0) as f64, (x.1 - y.1) as f64);
            l * l + r * r
        })
        .sum();
    (sum / (2 * a.len()) as f64).sqrt() / 32768.0
}

fn reference(dir: &Path) -> Vec<(i16, i16)> {
    let wave = image(dir, "wave.cue", "tone.wav", "WAVE");
    assert_eq!(wave.leadout(), 22, "0.3 s is 22 whole sectors of a WAVE file");
    frames(&wave, 22)
}

#[test]
fn flac_tracks_are_the_same_as_wave_ones() {
    let dir = scratch("flac");
    let reference = reference(&dir);
    // GOG's sheets call their files WAVE whatever they are.
    let flac = image(&dir, "game.ins", "tone.flac", "WAVE");
    assert_eq!(flac.leadout(), 23, "rounded up to whole sectors");
    assert_eq!(frames(&flac, 22), reference);
    let flac = image(&dir, "flac.cue", "tone.flac", "FLAC");
    assert_eq!(frames(&flac, 22), reference);
    // The last sector is padded with silence.
    let mut last = Vec::new();
    flac.read_audio(22, &mut last).unwrap();
    assert_eq!(last[587], (0, 0));
}

#[test]
fn lossy_tracks_sound_like_the_wave_one() {
    let dir = scratch("lossy");
    let reference = reference(&dir);
    let ogg = image(&dir, "ogg.cue", "tone.ogg", "WAVE");
    assert_eq!(ogg.leadout(), 23);
    let difference = rms_difference(&frames(&ogg, 22), &reference);
    assert!(difference < 0.02, "Ogg Vorbis differs by {}", difference);
    let mp3 = image(&dir, "mp3.cue", "tone.mp3", "MP3");
    assert!((22..=24).contains(&mp3.leadout()), "{}", mp3.leadout());
    let difference = rms_difference(&frames(&mp3, 22), &reference);
    assert!(difference < 0.05, "MP3 differs by {}", difference);
}

#[test]
fn other_rates_and_mono_are_resampled() {
    let dir = scratch("resampled");
    let reference = reference(&dir);
    let resampled = image(&dir, "22k.cue", "tone22k.flac", "FLAC");
    assert_eq!(resampled.leadout(), 23);
    let got = frames(&resampled, 22);
    assert!(got.iter().all(|(l, r)| l == r), "mono plays on both sides");
    // The mix of the two tones, as the file has it.
    let mixed: Vec<(i16, i16)> = reference
        .iter()
        .map(|&(l, r)| {
            let mono = ((l as i32 + r as i32) / 2) as i16;
            (mono, mono)
        })
        .collect();
    let difference = rms_difference(&got[100..], &mixed[100..]);
    assert!(difference < 0.05, "resampled differs by {}", difference);
}

#[test]
fn sectors_read_anywhere_are_those_read_in_order() {
    let dir = scratch("seek");
    for (name, file) in [("ogg.cue", "tone.ogg"), ("mp3.cue", "tone.mp3"), ("flac.cue", "tone.flac")] {
        let image = image(&dir, name, file, "WAVE");
        let in_order = frames(&image, 22);
        for lba in [15u32, 3, 20, 0, 9] {
            let mut sector = Vec::new();
            image.read_audio(lba, &mut sector).unwrap();
            let at = lba as usize * 588;
            let difference = rms_difference(&sector, &in_order[at..at + 588]);
            assert!(difference < 0.002, "{}: sector {} differs by {}", file, lba, difference);
        }
    }
}

