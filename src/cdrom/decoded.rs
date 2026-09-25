//! Audio tracks of CD images in compressed files (Ogg Vorbis, FLAC, MP3, or
//! WAVE files that aren't CD audio already), as GOG's CUE sheets and those
//! of many others have them. The file is decoded as the track is read, to
//! the 16-bit stereo samples at 44.1 kHz of a CD's audio: a whole disc
//! would be most of a gigabyte decoded.
//!
//! Reads follow on from where the last ended as the CD player plays; a
//! read further back, or far ahead, seeks in the file first.

use std::collections::VecDeque;
use std::fs::File;
use std::path::{Path, PathBuf};

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Timestamp;

/// The CD's sample rate.
const CD_RATE: u64 = 44_100;

/// How far ahead (in the file's frames) a read may be and still be decoded
/// to instead of seeked to: about two seconds.
const DECODE_AHEAD: u64 = 2 * CD_RATE;

pub struct DecodedTrack {
    path: PathBuf,
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    /// The file's sample rate and channels.
    rate: u64,
    channels: usize,
    /// The track's time base: a timestamp's seconds are `ts * numer / denom`.
    time_base: (u64, u64),
    /// The file's frames at 44.1 kHz.
    frames: u64,
    /// Decoded frames of the file, from its frame `start` on.
    decoded: VecDeque<[i16; 2]>,
    start: u64,
    /// The file has no more frames after `decoded`.
    ended: bool,
    /// Scratch space for the decoder's interleaved samples.
    samples: Vec<i16>,
}

/// An audio file opened for decoding: its reader, decoder and audio track,
/// and what the track says about itself.
struct Opened {
    reader: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    rate: u64,
    channels: usize,
    time_base: (u64, u64),
    num_frames: Option<u64>,
}

fn open_file(path: &Path) -> Result<Opened, String> {
    let error = |e: Error| format!("{}: {}", path.display(), e);
    let file = File::open(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    let stream = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let reader = symphonia::default::get_probe()
        .probe(&hint, stream, FormatOptions::default(), MetadataOptions::default())
        .map_err(error)?;
    let track = reader.default_track(TrackType::Audio).ok_or_else(|| format!("{}: no audio in it", path.display()))?;
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).cloned();
    let params = params.ok_or_else(|| format!("{}: an audio codec that isn't known", path.display()))?;
    let rate = params.sample_rate.ok_or_else(|| format!("{}: no sample rate", path.display()))? as u64;
    let channels = params.channels.as_ref().map_or(2, |c| c.count()).max(1);
    let time_base = track.time_base.map_or((1, rate), |tb| (tb.numer.get() as u64, tb.denom.get() as u64));
    let (track_id, num_frames) = (track.id, track.num_frames);
    let decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(error)?;
    Ok(Opened { reader, decoder, track_id, rate, channels, time_base, num_frames })
}

impl DecodedTrack {
    /// Open an audio file and find how long it is.
    pub fn open(path: &Path) -> Result<Self, String> {
        let opened = open_file(path)?;
        let num_frames = opened.num_frames;
        let mut decoded = DecodedTrack {
            path: path.to_path_buf(),
            reader: opened.reader,
            decoder: opened.decoder,
            track_id: opened.track_id,
            rate: opened.rate,
            channels: opened.channels,
            time_base: opened.time_base,
            frames: 0,
            decoded: VecDeque::new(),
            start: 0,
            ended: false,
            samples: Vec::new(),
        };
        let source_frames = match num_frames {
            Some(n) => n,
            None => decoded.count_frames()?,
        };
        decoded.frames = (source_frames * CD_RATE).div_ceil(decoded.rate);
        Ok(decoded)
    }

    /// The bytes of 16-bit stereo samples at 44.1 kHz the file decodes to.
    pub fn byte_len(&self) -> u64 {
        self.frames * 4
    }

    /// The file's frames, from the durations of its packets, for files
    /// that don't say (an MP3 without a header saying so): back at the
    /// start afterwards.
    fn count_frames(&mut self) -> Result<u64, String> {
        let mut duration = 0u64;
        while let Ok(Some(packet)) = self.reader.next_packet() {
            if packet.track_id == self.track_id {
                duration += packet.dur.get();
            }
        }
        self.seek_file(0)?;
        Ok(self.frames_of(duration))
    }

    /// A number of the track's time base units in frames of the file.
    fn frames_of(&self, ts: u64) -> u64 {
        (ts as u128 * self.time_base.0 as u128 * self.rate as u128 / self.time_base.1 as u128) as u64
    }

    /// A timestamp (which may be before the start) in frames of the file.
    fn frames_of_ts(&self, ts: i64) -> i64 {
        (ts as i128 * self.time_base.0 as i128 * self.rate as i128 / self.time_base.1 as i128) as i64
    }

    /// Start decoding the file again at (or before) its frame `frame`. The
    /// file is opened afresh: the readers don't all seek back reliably
    /// once they have read on. A file that can't seek there is decoded
    /// from its start.
    fn seek_file(&mut self, frame: u64) -> Result<(), String> {
        let opened = open_file(&self.path)?;
        self.reader = opened.reader;
        self.decoder = opened.decoder;
        if frame > 0 {
            let ts = (frame as u128 * self.time_base.1 as u128 / (self.time_base.0 as u128 * self.rate as u128)) as i64;
            let to = SeekTo::Timestamp { ts: Timestamp::new(ts), track_id: self.track_id };
            if self.reader.seek(SeekMode::Accurate, to).is_err() {
                self.reader = open_file(&self.path)?.reader;
            }
        }
        // Where the decoded frames are comes with them (`decode_more`).
        self.decoded.clear();
        self.ended = false;
        Ok(())
    }

    /// Decode the next packet onto `decoded`; false at the end of the file.
    fn decode_more(&mut self) -> bool {
        loop {
            let packet = match self.reader.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) | Err(_) => {
                    self.ended = true;
                    return false;
                }
            };
            if packet.track_id != self.track_id {
                continue;
            }
            // A packet's frames end where its time does: fewer than its
            // duration are those at the start of a file, or after a seek
            // before the decoder is under way.
            let end = packet.pts.get() + packet.dur.get() as i64;
            match self.decoder.decode(&packet) {
                Ok(buffer) => {
                    self.samples.clear();
                    buffer.copy_to_vec_interleaved::<i16>(&mut self.samples);
                }
                // A damaged packet is left out.
                Err(Error::DecodeError(_)) => continue,
                Err(_) => {
                    self.ended = true;
                    return false;
                }
            }
            let channels = self.channels;
            let count = (self.samples.len() / channels) as i64;
            let first = self.frames_of_ts(end) - count;
            // Frames before the file's start are none of it.
            let skip = (-first).max(0) as usize;
            if self.decoded.is_empty() {
                self.start = first.max(0) as u64;
            }
            self.decoded.extend(self.samples.chunks_exact(channels).skip(skip).map(|frame| match frame {
                [mono] => [*mono, *mono],
                [left, right, ..] => [*left, *right],
                [] => [0, 0],
            }));
            return true;
        }
    }

    /// The file's frame `frame`: silence past its end.
    fn source_frame(&mut self, frame: u64) -> [i16; 2] {
        let decoded_end = self.start + self.decoded.len() as u64;
        if frame < self.start || (frame > decoded_end + DECODE_AHEAD && !self.ended) {
            let _ = self.seek_file(frame);
        }
        loop {
            if !self.decoded.is_empty() && frame < self.start + self.decoded.len() as u64 {
                let Some(at) = frame.checked_sub(self.start) else { return [0, 0] };
                // What is behind is let go (but the frame before, which the
                // resampling may look at again); a CD player reads on.
                let behind = at.saturating_sub(1) as usize;
                self.decoded.drain(..behind);
                self.start += behind as u64;
                return self.decoded[(frame - self.start) as usize];
            }
            if self.ended || !self.decode_more() {
                return [0, 0];
            }
        }
    }

    /// Fill `buf` with the little-endian 16-bit stereo samples at 44.1 kHz
    /// from byte `at` on (which `byte_len` has the length of).
    pub fn read_at(&mut self, at: u64, buf: &mut [u8]) {
        let mut frame = [0u8; 4];
        let mut current = None;
        for (i, out) in buf.iter_mut().enumerate() {
            let byte = at + i as u64;
            let n = byte / 4;
            if current != Some(n) {
                current = Some(n);
                let [left, right] = if n < self.frames { self.output_frame(n) } else { [0, 0] };
                frame[..2].copy_from_slice(&left.to_le_bytes());
                frame[2..].copy_from_slice(&right.to_le_bytes());
            }
            *out = frame[(byte % 4) as usize];
        }
    }

    /// Frame `n` at 44.1 kHz: the file's frame there, or between the two
    /// around it (linear interpolation) at other rates.
    fn output_frame(&mut self, n: u64) -> [i16; 2] {
        if self.rate == CD_RATE {
            return self.source_frame(n);
        }
        let position = n as u128 * self.rate as u128;
        let (index, fraction) = ((position / CD_RATE as u128) as u64, (position % CD_RATE as u128) as i64);
        let a = self.source_frame(index);
        let b = self.source_frame(index + 1);
        let mix = |a: i16, b: i16| (a as i64 + (b as i64 - a as i64) * fraction / CD_RATE as i64) as i16;
        [mix(a[0], b[0]), mix(a[1], b[1])]
    }
}
