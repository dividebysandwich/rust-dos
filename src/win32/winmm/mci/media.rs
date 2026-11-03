//! The files the digital video devices (video.rs) play: AVI (avi.rs),
//! which AVIVideo opens, as Windows 98's MCIAVI does, and AVI and MPEG-1
//! (mpeg.rs), which MPEGVideo opens, as ActiveMovie's MCIQTZ does.

use super::{avi, mpeg};
use crate::win32::dsound::device::Format;

/// A file's streams, as far as the device needs them.
#[derive(Default)]
pub struct Media {
    /// The pictures' size and how many there are, and their rate in
    /// frames a second as a fraction.
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub rate: (u32, u32),
    pub video: Kind,
    /// The audio's format and samples, if it has audio.
    pub audio: Option<(Format, Vec<u8>)>,
}

/// The video stream, in the file's own format.
#[derive(Default)]
pub enum Kind {
    #[default]
    None,
    Mpeg(mpeg::Video),
    Avi(avi::Video),
}

impl std::fmt::Debug for Media {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.video {
            Kind::None => "no video",
            Kind::Mpeg(_) => "MPEG-1",
            Kind::Avi(_) => "AVI",
        };
        write!(
            f,
            "Media({kind}, {}x{}, {} frames)",
            self.width, self.height, self.frames
        )
    }
}

impl Media {
    /// Reads the file `d`, for MPEGVideo (`mpeg`) or AVIVideo. None when
    /// the device doesn't play it.
    pub fn parse(d: &[u8], mpeg: bool) -> Option<Media> {
        if d.starts_with(b"RIFF") {
            avi::parse(d)
        } else if mpeg {
            mpeg::parse(d).ok()
        } else {
            None
        }
    }

    /// The length in milliseconds: the video's, or the audio's without
    /// video.
    pub fn length_ms(&self) -> u32 {
        if self.frames > 0 {
            self.frame_ms(self.frames)
        } else if let Some((f, data)) = &self.audio {
            (data.len() as u64 * 1000 / f.bytes_per_second().max(1) as u64) as u32
        } else {
            0
        }
    }

    /// When the frame `n` starts, in milliseconds.
    pub fn frame_ms(&self, n: u32) -> u32 {
        let (num, den) = self.rate;
        (n as u64 * den as u64 * 1000 / num.max(1) as u64) as u32
    }

    /// The frame shown at `ms` milliseconds.
    pub fn frame_at(&self, ms: u64) -> u32 {
        let (num, den) = self.rate;
        let n = ms * num as u64 / (den.max(1) as u64 * 1000);
        n.min(self.frames.saturating_sub(1) as u64) as u32
    }
}

/// A file's picture decoder, made for its format when the first frame
/// is asked for. A copy is empty, as one after a restore is.
#[derive(Default)]
pub enum Pictures {
    #[default]
    None,
    Mpeg(Box<mpeg::Pictures>),
    Avi(avi::Pictures),
}

impl Clone for Pictures {
    fn clone(&self) -> Pictures {
        Pictures::None
    }
}

impl std::fmt::Debug for Pictures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Pictures::None => f.write_str("Pictures(None)"),
            Pictures::Mpeg(p) => p.fmt(f),
            Pictures::Avi(p) => p.fmt(f),
        }
    }
}

impl Pictures {
    /// The frame `n` of `media` as rows of 24-bit pixels from the top,
    /// `stride` bytes each, in a DIB's order: blue, green, red.
    pub fn rows(&mut self, media: &Media, n: u32, stride: usize) -> Option<Vec<u8>> {
        match &media.video {
            Kind::None => None,
            Kind::Mpeg(v) => {
                if !matches!(self, Pictures::Mpeg(_)) {
                    *self = Pictures::Mpeg(Box::default());
                }
                let Pictures::Mpeg(p) = self else {
                    return None;
                };
                p.frame(v, n).map(|f| mpeg::bgr_rows(f, stride))
            }
            Kind::Avi(v) => {
                if !matches!(self, Pictures::Avi(_)) {
                    *self = Pictures::Avi(avi::Pictures::default());
                }
                let Pictures::Avi(p) = self else {
                    return None;
                };
                p.frame(v, (media.width, media.height), n)
                    .map(<[u8]>::to_vec)
            }
        }
    }
}
