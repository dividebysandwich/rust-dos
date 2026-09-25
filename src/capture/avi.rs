//! Video recordings with sound: an AVI file of ZMBV video (see zmbv.rs) at
//! 60 frames a second of emulated time and 16-bit stereo sound at 44.1 kHz,
//! interleaved as the machine makes them, and an index. The encoding runs
//! on a thread of its own.

use super::zmbv;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

/// Frames a second.
pub const FPS: u32 = 60;
const AUDIO_RATE: u32 = crate::opl::RATE;
/// Where AVI 1.0's 32-bit sizes and offsets stop being safe.
const LIMIT: u64 = 1 << 30;
/// The size of the headers before the 'movi' list, which `finish` writes
/// again with the counts.
const HEADERS: u64 = 12 + 8 + 4 + (8 + 56) + (12 + (8 + 56) + (8 + 40)) + (12 + (8 + 56) + (8 + 16));

/// An AVI file being written: the video stream 0 (`00dc` chunks) and the
/// sound stream 1 (`01wb`).
pub struct AviWriter {
    file: BufWriter<File>,
    width: u32,
    height: u32,
    /// The index: chunk id, keyframe, offset from the 'movi' list's type,
    /// size.
    index: Vec<([u8; 4], bool, u32, u32)>,
    /// Where the 'movi' list starts, and the bytes written into it.
    movi: u64,
    written: u64,
    frames: u32,
    /// Sound frames (a left and a right sample each).
    sound: u64,
}

impl AviWriter {
    pub fn create(path: &Path, width: u32, height: u32) -> Result<Self, String> {
        let file = File::create(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        let mut avi = Self {
            file: BufWriter::new(file),
            width,
            height,
            index: Vec::new(),
            movi: HEADERS,
            written: 0,
            frames: 0,
            sound: 0,
        };
        avi.headers().map_err(|e| e.to_string())?;
        avi.file.write_all(b"LIST\0\0\0\0movi").map_err(|e| e.to_string())?;
        Ok(avi)
    }

    /// Whether the file has grown as big as AVI 1.0 files safely get.
    pub fn full(&self) -> bool {
        self.movi + self.written >= LIMIT
    }

    /// The RIFF header, the main header and the two streams' headers.
    fn headers(&mut self) -> std::io::Result<()> {
        let (width, height, frames) = (self.width, self.height, self.frames);
        let riff = self.movi + 12 + self.written + 8 + 16 * self.index.len() as u64 - 8;
        let mut h = Vec::with_capacity(HEADERS as usize);
        let u32s = |h: &mut Vec<u8>, values: &[u32]| values.iter().for_each(|v| h.extend(v.to_le_bytes()));
        h.extend(b"RIFF");
        u32s(&mut h, &[riff as u32]);
        h.extend(b"AVI LIST");
        u32s(&mut h, &[(HEADERS - 20) as u32]);
        h.extend(b"hdrlavih");
        // Microseconds a frame, the most bytes a second, padding, flags
        // (an index, interleaved), frames, initial frames, streams,
        // buffer size, width, height, 4 reserved.
        u32s(&mut h, &[56, 1_000_000 / FPS, 0, 0, 0x110, frames, 0, 2, 0, width, height, 0, 0, 0, 0]);

        h.extend(b"LIST");
        u32s(&mut h, &[4 + 8 + 56 + 8 + 40]);
        h.extend(b"strlstrh");
        u32s(&mut h, &[56]);
        h.extend(b"vidsZMBV");
        // Flags, priority and language, initial frames, scale, rate,
        // start, length, buffer size, quality, sample size.
        u32s(&mut h, &[0, 0, 0, 1, FPS, 0, frames, 0, u32::MAX, 0]);
        h.extend([0, 0, 0, 0]);
        h.extend((width as u16).to_le_bytes());
        h.extend((height as u16).to_le_bytes());
        h.extend(b"strf");
        // A BITMAPINFOHEADER: size, width, height, planes and bits,
        // compression, image size, resolution, colours.
        u32s(&mut h, &[40, 40, width, height, 1 | 32 << 16]);
        h.extend(b"ZMBV");
        u32s(&mut h, &[width * height * 4, 0, 0, 0, 0]);

        h.extend(b"LIST");
        u32s(&mut h, &[4 + 8 + 56 + 8 + 16]);
        h.extend(b"strlstrh");
        u32s(&mut h, &[56]);
        h.extend(b"auds\0\0\0\0");
        // Flags, priority and language, initial frames, scale (a frame's
        // bytes), rate (bytes a second), start, length (frames), buffer
        // size, quality, sample size.
        u32s(&mut h, &[0, 0, 0, 4, AUDIO_RATE * 4, 0, self.sound as u32, 0, u32::MAX, 4]);
        u32s(&mut h, &[0, 0]);
        h.extend(b"strf");
        // A WAVEFORMAT: PCM, 2 channels, rate, bytes a second, block,
        // bits.
        u32s(&mut h, &[16, 1 | 2 << 16, AUDIO_RATE, AUDIO_RATE * 4, 4 | 16 << 16]);
        debug_assert_eq!(h.len() as u64, HEADERS);

        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&h)?;
        Ok(())
    }

    fn chunk(&mut self, id: &[u8; 4], data: &[u8], keyframe: bool) -> Result<(), String> {
        let offset = 4 + self.written as u32;
        let mut bytes = Vec::with_capacity(data.len() + 9);
        bytes.extend(id);
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(data);
        if data.len() % 2 == 1 {
            bytes.push(0);
        }
        self.file.write_all(&bytes).map_err(|e| e.to_string())?;
        self.written += bytes.len() as u64;
        self.index.push((*id, keyframe, offset, data.len() as u32));
        Ok(())
    }

    /// Add a video frame's ZMBV data.
    pub fn video(&mut self, data: &[u8], keyframe: bool) -> Result<(), String> {
        self.frames += 1;
        self.chunk(b"00dc", data, keyframe)
    }

    /// Add interleaved stereo samples.
    pub fn sound(&mut self, samples: &[i16]) -> Result<(), String> {
        if samples.is_empty() {
            return Ok(());
        }
        self.sound += samples.len() as u64 / 2;
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        self.chunk(b"01wb", &bytes, true)
    }

    /// Write the index and the sizes and counts, and close the file.
    /// Returns the frames written.
    pub fn finish(mut self) -> Result<u32, String> {
        let mut index = Vec::with_capacity(8 + 16 * self.index.len());
        index.extend(b"idx1");
        index.extend((16 * self.index.len() as u32).to_le_bytes());
        for (id, keyframe, offset, size) in &self.index {
            index.extend(id);
            index.extend((if *keyframe { 0x10u32 } else { 0 }).to_le_bytes());
            index.extend(offset.to_le_bytes());
            index.extend(size.to_le_bytes());
        }
        let io = |e: std::io::Error| e.to_string();
        self.file.write_all(&index).map_err(io)?;
        self.headers().map_err(io)?;
        self.file.seek(SeekFrom::Start(self.movi + 4)).map_err(io)?;
        self.file.write_all(&(4 + self.written as u32).to_le_bytes()).map_err(io)?;
        self.file.flush().map_err(io)?;
        Ok(self.frames)
    }
}

/// What the recording thread gets: a frame and the sound since the last,
/// or the end.
enum Work {
    Frame { rgb: Option<Vec<u8>>, sound: Vec<i16> },
    Stop,
}

/// A video recording, encoding and writing on its own thread. The frames
/// come at 60 a second of emulated time, the sound as the machine makes it.
pub struct VideoRecorder {
    work: Sender<Work>,
    thread: Option<JoinHandle<Result<u32, String>>>,
    width: usize,
    height: usize,
    /// Emulated time at the start, and the frames sent since.
    start_ns: u64,
    frames: u64,
    /// Sound waiting for the next frame.
    carry: Vec<i16>,
}

impl VideoRecorder {
    /// Start recording to `path` at the size of `width` x `height`, at
    /// emulated time `now_ns`.
    pub fn start(path: &Path, width: usize, height: usize, now_ns: u64) -> Result<Self, String> {
        let mut avi = AviWriter::create(path, width as u32, height as u32)?;
        let (work, jobs): (Sender<Work>, Receiver<Work>) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut encoder = zmbv::Encoder::new(width, height);
            let mut last = None;
            while let Ok(Work::Frame { rgb, sound }) = jobs.recv() {
                avi.sound(&sound)?;
                // No new picture: the last one again.
                let rgb = match rgb {
                    Some(rgb) => last.insert(rgb),
                    None => last.get_or_insert_with(|| vec![0; width * height * 3]),
                };
                let (data, keyframe) = encoder.encode(rgb);
                avi.video(&data, keyframe)?;
                if avi.full() {
                    break;
                }
            }
            avi.finish()
        });
        Ok(Self { work, thread: Some(thread), width, height, start_ns: now_ns, frames: 0, carry: Vec::new() })
    }

    /// Record the frames due by emulated time `now_ns`, `frame` the first of
    /// them and repeats after it, with the sound made since the last call.
    /// Sound made before a frame is due goes with the next one. Returns
    /// false once the recording has stopped (the file is full, or writing
    /// failed).
    pub fn record(&mut self, frame: &crate::video::Frame, sound: Vec<i16>, now_ns: u64) -> bool {
        self.carry.extend(sound);
        let due = (now_ns.saturating_sub(self.start_ns) as u128 * FPS as u128 / 1_000_000_000) as u64;
        let mut rgb = (self.frames < due).then(|| zmbv::fitted(frame, self.width, self.height).into_owned());
        while self.frames < due {
            self.frames += 1;
            let work = Work::Frame { rgb: rgb.take(), sound: std::mem::take(&mut self.carry) };
            if self.work.send(work).is_err() {
                return false;
            }
        }
        !self.thread.as_ref().is_some_and(|t| t.is_finished())
    }

    /// Stop recording and finish the file. Returns the frames written.
    pub fn stop(mut self) -> Result<u32, String> {
        let _ = self.work.send(Work::Stop);
        match self.thread.take().map(|t| t.join()) {
            Some(Ok(result)) => result,
            _ => Err("the recording thread failed".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::Frame;

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn a_recording_is_an_indexed_avi() {
        let path = std::env::temp_dir().join(format!("rust-dos-avi-{}.avi", std::process::id()));
        let mut recorder = VideoRecorder::start(&path, 32, 16, 0).unwrap();
        let mut frame = Frame::new(32, 16);
        // A second at 60 frames a second, in steps of a sixtieth, with the
        // sound of each.
        for i in 1..=60u64 {
            frame.rgb[0] = i as u8;
            assert!(recorder.record(&frame, vec![i as i16; 1470], i * 1_000_000_000 / 60));
        }
        assert_eq!(recorder.stop().unwrap(), 60);
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32_at(&bytes, 4) as usize, bytes.len() - 8);
        assert_eq!(&bytes[8..12], b"AVI ");
        assert_eq!((&bytes[24..28], u32_at(&bytes, 28)), (&b"avih"[..], 56));
        assert_eq!(u32_at(&bytes, 32 + 16), 60, "total frames");
        let movi = HEADERS as usize;
        assert_eq!(&bytes[movi..movi + 4], b"LIST");
        assert_eq!(&bytes[movi + 8..movi + 12], b"movi");
        let idx1 = movi + 8 + u32_at(&bytes, movi + 4) as usize;
        assert_eq!(&bytes[idx1..idx1 + 4], b"idx1");
        let entries = u32_at(&bytes, idx1 + 4) as usize / 16;
        let video = (0..entries).filter(|e| &bytes[idx1 + 8 + e * 16..][..4] == b"00dc").count();
        assert_eq!(video, 60);
        assert!(entries > video, "and sound chunks");
        // Each entry points at its chunk.
        for e in 0..entries {
            let entry = idx1 + 8 + e * 16;
            let at = movi + 8 + u32_at(&bytes, entry + 8) as usize;
            assert_eq!(&bytes[at..at + 4], &bytes[entry..entry + 4]);
            assert_eq!(u32_at(&bytes, at + 4), u32_at(&bytes, entry + 12));
        }
        // The first video chunk is a keyframe, and the sound's length.
        let first_video = (0..entries).map(|e| idx1 + 8 + e * 16).find(|&e| &bytes[e..e + 4] == b"00dc").unwrap();
        assert_eq!(u32_at(&bytes, first_video + 4), 0x10);
        let audio_strh = 12 + 8 + 4 + (8 + 56) + (12 + (8 + 56) + (8 + 40)) + 12;
        assert_eq!(&bytes[audio_strh..audio_strh + 4], b"strh");
        assert_eq!(u32_at(&bytes, audio_strh + 8 + 32), 60 * 735, "sound frames");
    }
}
