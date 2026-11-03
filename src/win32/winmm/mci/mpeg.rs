//! MPEG-1 files, as the MPEGVideo device (video.rs) plays them: a system
//! stream (ISO/IEC 11172-1) of packs with a video and an audio stream, or
//! a video or audio elementary stream alone. The `oxideav-mpeg12video`
//! crate decodes the pictures and `oxideav-mp2` the Layer II audio.
//!
//! The audio is decoded whole when the file opens, to be played as one
//! sound. The pictures are decoded as they are shown: a decoder goes on
//! from the picture it showed last, and a seek starts it again at the
//! group of pictures before the one the frame is in, since the first B
//! pictures of an open group predict from the group before.
//!
//! The crate's own sequence driver decodes a whole stream at once and
//! refuses a B picture that comes before two anchors, which a closed
//! group's first ones do: they predict backward only. So this drives its
//! per-picture functions itself, with the one anchor in the place of
//! both for those.

use super::media::{Kind, Media};
use crate::win32::dsound::device::Format;
use oxideav_core::{CodecId, CodecParameters, Frame, Packet, TimeBase};
use oxideav_mpeg12video::frame_assembly::FrameBuffer;
use oxideav_mpeg12video::inter_reconstruction::ReferenceFrames;
use oxideav_mpeg12video::mpeg1_picture::{
    Mpeg1InterParams, Mpeg1PictureParams, decode_mpeg1_inter_picture, decode_mpeg1_intra_picture,
};
use oxideav_mpeg12video::picture_header::{Mpeg2PictureHeader, PictureCodingType};
use oxideav_mpeg12video::quant_matrix_extension::QuantiserMatrixPayload;
use oxideav_mpeg12video::sequence_header::Mpeg2SequenceHeader;

/// The start codes.
const PICTURE: u8 = 0x00;
const SEQUENCE_HEADER: u8 = 0xB3;
const EXTENSION: u8 = 0xB5;
const SEQUENCE_END: u8 = 0xB7;
const GROUP: u8 = 0xB8;
const PACK: u8 = 0xBA;
const SYSTEM_HEADER: u8 = 0xBB;
const END: u8 = 0xB9;

/// The largest picture decoded: MPEG-1's constrained parameters are
/// 768 by 576, and nothing bigger is wanted of a 1990s file.
const MAX_SIZE: u32 = 1024;

/// A file's video elementary stream, and where its groups of pictures
/// start in it, with the display index of each one's first frame.
#[derive(Default)]
pub struct Video {
    data: Vec<u8>,
    groups: Vec<(usize, u32)>,
}

/// Why a file doesn't open: it isn't MPEG-1 (or is MPEG-2, which
/// ActiveMovie's decoder doesn't take), or has no stream it plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invalid;

/// Whether `d` has a start code at `i`.
fn code_at(d: &[u8], i: usize) -> Option<u8> {
    (d.get(i..i + 3)? == [0, 0, 1]).then(|| d[i + 3])
}

/// The next start code from `i`, and where it is.
fn next_code(d: &[u8], i: usize) -> Option<(usize, u8)> {
    let at = d.get(i..)?.windows(3).position(|w| w == [0, 0, 1])? + i;
    Some((at, *d.get(at + 3)?))
}

/// The video and audio elementary streams of a system stream: the
/// payloads of its first video (0xE0..0xEF) and audio (0xC0..0xDF)
/// streams' packets, in order.
fn demux(d: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let (mut video, mut audio) = (Vec::new(), Vec::new());
    let (mut video_id, mut audio_id) = (None, None);
    let mut i = 0;
    while let Some((at, code)) = next_code(d, i) {
        match code {
            PACK => {
                // MPEG-1's pack header is 12 bytes; MPEG-2's has '01'
                // first and stuffing.
                i = match d.get(at + 4) {
                    Some(b) if b >> 6 == 1 => {
                        at + 14 + d.get(at + 13).map_or(0, |b| b & 7) as usize
                    }
                    _ => at + 12,
                };
            }
            END => i = at + 4,
            SYSTEM_HEADER.. => {
                let Some(len) = d.get(at + 4..at + 6) else {
                    break;
                };
                let start = at + 6;
                let end = (start + u16::from_be_bytes([len[0], len[1]]) as usize).min(d.len());
                let wanted = match code {
                    0xE0..=0xEF => *video_id.get_or_insert(code) == code,
                    0xC0..=0xDF => *audio_id.get_or_insert(code) == code,
                    _ => false,
                };
                if wanted && let Some(payload) = payload(&d[start..end]) {
                    if code >= 0xE0 {
                        video.extend_from_slice(payload);
                    } else {
                        audio.extend_from_slice(payload);
                    }
                }
                i = end;
            }
            _ => i = at + 3,
        }
    }
    (video, audio)
}

/// The data of an MPEG-1 packet, after its stuffing, its buffer size
/// and its time stamps.
fn payload(p: &[u8]) -> Option<&[u8]> {
    let mut j = p.iter().position(|&b| b != 0xFF)?;
    if p[j] >> 6 == 1 {
        j += 2;
    }
    j += match p.get(j)? >> 4 {
        2 => 5,
        3 => 10,
        _ => 1,
    };
    p.get(j..)
}

/// The rates of frame_rate_code 1 to 8, in frames a second.
const RATES: [(u32, u32); 8] = [
    (24000, 1001),
    (24, 1),
    (25, 1),
    (30000, 1001),
    (30, 1),
    (50, 1),
    (60000, 1001),
    (60, 1),
];

/// The geometry and quantiser matrices of the sequence header at the
/// start of `region`.
fn sequence(region: &[u8]) -> Option<(Mpeg1PictureParams, u8)> {
    let h = Mpeg2SequenceHeader::parse(region).ok()?;
    let raster = |zz: [u8; 64]| QuantiserMatrixPayload { bytes: zz }.to_matrix();
    let params = Mpeg1PictureParams {
        width: h.width as usize,
        height: h.height as usize,
        intra_quant: h
            .intra_quant
            .map(raster)
            .unwrap_or(oxideav_mpeg12video::dequantize::DEFAULT_INTRA_QUANT),
        non_intra_quant: h.non_intra_quant.map(raster).unwrap_or([[16; 8]; 8]),
    };
    Some((params, h.frame_rate_code))
}

/// Reads the file `d`.
pub fn parse(d: &[u8]) -> Result<Media, Invalid> {
    let (video, audio) = match code_at(d, 0) {
        Some(PACK) => demux(d),
        Some(SEQUENCE_HEADER) => (d.to_vec(), Vec::new()),
        _ if d.len() >= 2 && d[0] == 0xFF && d[1] & 0xE0 == 0xE0 => (Vec::new(), d.to_vec()),
        _ => return Err(Invalid),
    };
    let mut media = Media {
        audio: decode_audio(&audio),
        ..Media::default()
    };
    if !video.is_empty() {
        index(&mut media, video)?;
    }
    if media.frames == 0 && media.audio.is_none() {
        return Err(Invalid);
    }
    Ok(media)
}

/// Finds the video's size, rate, groups and pictures.
fn index(media: &mut Media, data: Vec<u8>) -> Result<(), Invalid> {
    let (at, _) = next_code(&data, 0).ok_or(Invalid)?;
    if code_at(&data, at) != Some(SEQUENCE_HEADER) {
        return Err(Invalid);
    }
    let (params, rate) = sequence(&data[at..]).ok_or(Invalid)?;
    let (width, height) = (params.width as u32, params.height as u32);
    if !(1..=MAX_SIZE).contains(&width) || !(1..=MAX_SIZE).contains(&height) {
        return Err(Invalid);
    }
    let rate = *RATES.get((rate as usize).wrapping_sub(1)).ok_or(Invalid)?;
    let mut groups = Vec::new();
    let mut pictures = 0;
    let mut i = at + 4;
    let mut first = true;
    while let Some((at, code)) = next_code(&data, i) {
        match code {
            // A sequence extension makes it MPEG-2.
            EXTENSION if first => return Err(Invalid),
            GROUP => groups.push((at, pictures)),
            PICTURE => {
                if groups.is_empty() {
                    groups.push((at, 0));
                }
                pictures += 1;
            }
            _ => {}
        }
        first = false;
        i = at + 4;
    }
    media.width = width;
    media.height = height;
    media.rate = rate;
    media.frames = pictures;
    media.video = Kind::Mpeg(Video { data, groups });
    Ok(())
}

/// The samples of the Layer II audio `a`, which a frame after another
/// carries: none for audio of another layer, or with no frame.
fn decode_audio(a: &[u8]) -> Option<(Format, Vec<u8>)> {
    const KBPS: [u32; 16] = [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 0,
    ];
    const RATES: [u32; 4] = [44100, 48000, 32000, 0];
    let mut decoder = None;
    let mut format = None;
    let mut pcm = Vec::new();
    let mut i = 0;
    while i + 4 <= a.len() {
        // A frame header: the sync word, MPEG-1, Layer II.
        if a[i] != 0xFF || a[i + 1] & 0xFE != 0xFC {
            i += 1;
            continue;
        }
        let kbps = KBPS[(a[i + 2] >> 4) as usize];
        let rate = RATES[((a[i + 2] >> 2) & 3) as usize];
        if kbps == 0 || rate == 0 {
            i += 1;
            continue;
        }
        let len = (144 * kbps * 1000 / rate) as usize + ((a[i + 2] >> 1) & 1) as usize;
        if i + len > a.len() {
            break;
        }
        let channels = if a[i + 3] >> 6 == 3 { 1 } else { 2 };
        let f = *format.get_or_insert(Format::pcm(rate, 16, channels));
        if f.rate != rate || f.channels != channels {
            // The stream changed; what came first is what plays.
            break;
        }
        let d = decoder.get_or_insert_with(|| {
            let mut params = CodecParameters::audio(CodecId::new(oxideav_mp2::CODEC_ID_STR));
            params.channels = Some(channels);
            params.sample_rate = Some(rate);
            oxideav_mp2::make_decoder(&params).ok()
        });
        let Some(d) = d else {
            return None;
        };
        let packet = Packet::new(0, TimeBase::new(1, rate as i64), a[i..i + len].to_vec());
        if d.send_packet(&packet).is_ok() {
            while let Ok(Frame::Audio(frame)) = d.receive_frame() {
                for s in 0..frame.samples as usize {
                    for c in 0..channels as usize {
                        let plane = frame.data.get(c).or(frame.data.first());
                        let sample = plane.and_then(|p| p.get(s * 2..s * 2 + 2));
                        pcm.extend_from_slice(sample.unwrap_or(&[0, 0]));
                    }
                }
            }
        }
        i += len;
    }
    Some((format?, pcm))
}

/// A picture decoder, which goes on from where it got to. A copy is
/// empty, as one after a restore is: it starts again at the next frame
/// it is asked for.
#[derive(Default)]
pub struct Pictures {
    /// Where the next start code is looked for in the video, and the
    /// display index of the current group's first frame.
    at: usize,
    base: u32,
    params: Option<Mpeg1PictureParams>,
    /// The older and the newer anchor, and the newer one's display
    /// index while it waits to be shown after the B pictures before it.
    past: Option<FrameBuffer>,
    future: Option<FrameBuffer>,
    held: Option<u32>,
    /// The last frame shown, and its index.
    shown: Option<(u32, FrameBuffer)>,
    /// Whether the decoder has started.
    started: bool,
}

impl Clone for Pictures {
    fn clone(&self) -> Pictures {
        Pictures::default()
    }
}

impl std::fmt::Debug for Pictures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pictures({:?})", self.shown.as_ref().map(|s| s.0))
    }
}

impl Pictures {
    /// The frame `n` of `video`, or the nearest one before it that
    /// decodes.
    pub fn frame(&mut self, video: &Video, n: u32) -> Option<&FrameBuffer> {
        let group_of = |n: u32| video.groups.partition_point(|g| g.1 <= n).saturating_sub(1);
        let group = group_of(n);
        let current = self.shown.as_ref().map(|s| s.0);
        // Back, or on past the next group: start again.
        let restart = match current {
            _ if !self.started => true,
            Some(c) => c > n || group > group_of(c) + 1,
            None => false,
        };
        if restart {
            self.restart(video, group.saturating_sub(1));
        }
        while self.shown.as_ref().is_none_or(|s| s.0 < n) {
            if !self.step(video) {
                break;
            }
        }
        self.shown.as_ref().map(|s| &s.1)
    }

    fn restart(&mut self, video: &Video, group: usize) {
        let (at, base) = video.groups.get(group).copied().unwrap_or((0, 0));
        *self = Pictures {
            at,
            base,
            started: true,
            // The sequence header before the group, which every group
            // in the files MCIQTZ plays has; the first one otherwise.
            params: sequence_before(&video.data, at),
            ..Pictures::default()
        };
    }

    /// Decodes the next picture. False at the end of the stream.
    fn step(&mut self, video: &Video) -> bool {
        let v = &video.data;
        loop {
            let Some((at, code)) = next_code(v, self.at) else {
                // The last anchor shows at the end.
                if let (Some(n), Some(f)) = (self.held.take(), self.future.clone()) {
                    self.shown = Some((n, f));
                    return true;
                }
                return false;
            };
            self.at = at + 4;
            match code {
                SEQUENCE_HEADER => {
                    if let Some((params, _)) = sequence(&v[at..]) {
                        self.params = Some(params);
                    }
                }
                GROUP => {
                    let i = video.groups.partition_point(|g| g.0 < at);
                    if let Some(&(_, base)) = video.groups.get(i) {
                        self.base = base;
                    }
                }
                PICTURE => {
                    // The picture runs to the next boundary, whose start
                    // code ends its last slice.
                    let next = next_boundary(v, at + 4);
                    self.at = next.unwrap_or(v.len());
                    let end = next.map_or(v.len(), |n| (n + 4).min(v.len()));
                    if self.picture(&v[at..end]) {
                        return true;
                    }
                }
                _ => {}
            }
        }
    }

    /// Decodes the picture `region`. True when a frame is shown.
    fn picture(&mut self, region: &[u8]) -> bool {
        let Some(params) = self.params else {
            return false;
        };
        let Ok(header) = Mpeg2PictureHeader::parse(region) else {
            return false;
        };
        let index = self.base + header.temporal_reference as u32;
        let inter = Mpeg1InterParams {
            base: params,
            picture_coding_type: header.picture_coding_type,
            forward_f_code: header.fwd_f_code.unwrap_or(1),
            full_pel_forward_vector: header.full_pel_forward_vector.unwrap_or(false),
            backward_f_code: header.bwd_f_code.unwrap_or(1),
            full_pel_backward_vector: header.full_pel_backward_vector.unwrap_or(false),
        };
        match header.picture_coding_type {
            PictureCodingType::Intra | PictureCodingType::Predictive => {
                let frame = if header.picture_coding_type == PictureCodingType::Intra {
                    decode_mpeg1_intra_picture(region, &params).map(|r| r.0)
                } else if let Some(f) = &self.future {
                    decode_mpeg1_inter_picture(region, &inter, ReferenceFrames::forward_only(f))
                        .map(|r| r.0)
                } else {
                    return false;
                };
                let Ok(frame) = frame else {
                    return false;
                };
                let shows = match (self.held.take(), self.future.take()) {
                    (Some(n), Some(f)) => {
                        self.shown = Some((n, f.clone()));
                        self.past = Some(f);
                        true
                    }
                    (_, f) => {
                        self.past = f;
                        false
                    }
                };
                self.future = Some(frame);
                self.held = Some(index);
                shows
            }
            PictureCodingType::Bidirectional => {
                let Some(future) = &self.future else {
                    return false;
                };
                let past = self.past.as_ref().unwrap_or(future);
                let refs = ReferenceFrames::bidirectional(past, future);
                match decode_mpeg1_inter_picture(region, &inter, refs) {
                    Ok((f, _)) => {
                        self.shown = Some((index, f));
                        true
                    }
                    Err(_) => false,
                }
            }
            _ => false,
        }
    }
}

/// The sequence header nearest before `at` in the video.
fn sequence_before(v: &[u8], at: usize) -> Option<Mpeg1PictureParams> {
    let start = v[..at.min(v.len())]
        .windows(4)
        .rposition(|w| w == [0, 0, 1, SEQUENCE_HEADER])?;
    sequence(&v[start..]).map(|s| s.0)
}

/// The next picture's boundary: a picture, group, sequence header or
/// sequence end start code.
fn next_boundary(v: &[u8], i: usize) -> Option<usize> {
    let mut i = i;
    while let Some((at, code)) = next_code(v, i) {
        if matches!(code, PICTURE | GROUP | SEQUENCE_HEADER | SEQUENCE_END) {
            return Some(at);
        }
        i = at + 3;
    }
    None
}

/// A frame as rows of 24-bit pixels from the top, `stride` bytes each,
/// in a DIB's order: blue, green, red.
pub fn bgr_rows(frame: &FrameBuffer, stride: usize) -> Vec<u8> {
    let (w, h) = (frame.width, frame.height);
    let (yw, cw) = (frame.y.width(), frame.cb.width());
    let (y, cb, cr) = (frame.y.samples(), frame.cb.samples(), frame.cr.samples());
    let mut out = vec![0; stride * h];
    for row in 0..h {
        let line = &mut out[row * stride..];
        for col in 0..w {
            let c = (row / 2) * cw + col / 2;
            let px = crate::win32::vfw::codec::bgr(y[row * yw + col], cb[c], cr[c]);
            line[col * 3..col * 3 + 3].copy_from_slice(&px);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxideav_core::AudioFrame;
    use oxideav_mpeg12video::mpeg1_encoder::encode_mpeg1_display_order_sequence;
    use oxideav_mpeg12video::mpeg1_stream_writer::Mpeg1SequenceParams;
    use oxideav_mpeg12video::sequence_extension::ChromaFormat;

    /// The colours of the test's frames, as Y, Cb and Cr: red, then blue.
    const RED: (u8, u8, u8) = (81, 90, 240);
    const BLUE: (u8, u8, u8) = (41, 240, 110);

    fn flat(w: usize, h: usize, (y, cb, cr): (u8, u8, u8)) -> FrameBuffer {
        let mut f = FrameBuffer::new(w, h, ChromaFormat::Yuv420);
        for (plane, v) in [(&mut f.y, y), (&mut f.cb, cb), (&mut f.cr, cr)] {
            for row in 0..plane.height() {
                for col in 0..plane.width() {
                    plane.put_sample(col, row, v);
                }
            }
        }
        f
    }

    /// 12 frames of 32 by 32 at 25 a second, 6 red and 6 blue, in two
    /// groups of I, P and B pictures.
    fn video() -> Vec<u8> {
        let frames: Vec<FrameBuffer> = (0..12)
            .map(|n| flat(32, 32, if n < 6 { RED } else { BLUE }))
            .collect();
        let seq = Mpeg1SequenceParams {
            horizontal_size: 32,
            vertical_size: 32,
            picture_rate_code: 3,
            ..Default::default()
        };
        encode_mpeg1_display_order_sequence(&frames, 2, 2, &seq, 4, 1, 1).unwrap()
    }

    /// Half a second of a 1 kHz tone in Layer II, 44100 Hz stereo.
    fn audio() -> Vec<u8> {
        let mut params = CodecParameters::audio(CodecId::new(oxideav_mp2::CODEC_ID_STR));
        params.sample_rate = Some(44100);
        params.channels = Some(2);
        params.bit_rate = Some(192_000);
        let mut e = oxideav_mp2::make_encoder(&params).unwrap();
        let n = 22050;
        let plane: Vec<u8> = (0..n)
            .flat_map(|i| {
                let t = i as f64 / 44100.0;
                let v = (t * 1000.0 * std::f64::consts::TAU).sin() * 8000.0;
                (v as i16).to_le_bytes()
            })
            .collect();
        let frame = Frame::Audio(AudioFrame {
            samples: n as u32,
            pts: None,
            data: vec![plane.clone(), plane],
        });
        e.send_frame(&frame).unwrap();
        e.flush().unwrap();
        let mut out = Vec::new();
        while let Ok(p) = e.receive_packet() {
            out.extend_from_slice(&p.data);
        }
        out
    }

    /// A system stream of packs, each with a packet of `video`'s and one
    /// of `audio`'s.
    fn system(video: &[u8], audio: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let (mut v, mut a) = (video.chunks(1000), audio.chunks(1000));
        loop {
            let (cv, ca) = (v.next(), a.next());
            if cv.is_none() && ca.is_none() {
                break;
            }
            // A pack header: SCR and mux rate, their marker bits set.
            out.extend_from_slice(&[0, 0, 1, PACK, 0x21, 0, 1, 0, 1, 0x80, 0x01, 0x01]);
            for (id, chunk) in [(0xE0, cv), (0xC0, ca)] {
                let Some(chunk) = chunk else { continue };
                // No time stamps: the 0x0F byte.
                let len = (chunk.len() + 1) as u16;
                out.extend_from_slice(&[0, 0, 1, id]);
                out.extend_from_slice(&len.to_be_bytes());
                out.push(0x0F);
                out.extend_from_slice(chunk);
            }
        }
        out.extend_from_slice(&[0, 0, 1, END]);
        out
    }

    /// The colour of a frame's middle, as Y, Cb and Cr.
    fn colour(f: &FrameBuffer) -> (u8, u8, u8) {
        let at = |p: &oxideav_mpeg12video::frame_assembly::Plane, x, y| p.get(x, y).unwrap();
        (at(&f.y, 16, 16), at(&f.cb, 8, 8), at(&f.cr, 8, 8))
    }

    fn stream(m: &Media) -> &Video {
        match &m.video {
            Kind::Mpeg(v) => v,
            _ => panic!("not MPEG"),
        }
    }

    fn near(a: (u8, u8, u8), b: (u8, u8, u8)) -> bool {
        let d = |x: u8, y: u8| (x as i32 - y as i32).abs() <= 3;
        d(a.0, b.0) && d(a.1, b.1) && d(a.2, b.2)
    }

    #[test]
    fn a_system_stream_has_its_pictures_and_its_sound() {
        let m = parse(&system(&video(), &audio())).unwrap();
        assert_eq!((m.width, m.height, m.frames), (32, 32, 12));
        assert_eq!(m.rate, (25, 1));
        assert_eq!(stream(&m).groups.len(), 2);
        assert_eq!(m.length_ms(), 480);
        assert_eq!(m.frame_at(239), 5);
        assert_eq!(m.frame_at(240), 6);
        let (format, samples) = m.audio.clone().unwrap();
        assert_eq!((format.rate, format.channels, format.bits), (44100, 2, 16));
        // Whole frames of 1152 samples, the last one padded.
        assert_eq!(samples.len(), 20 * 1152 * 4);
        let loud = samples
            .chunks(2)
            .map(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs() as u32)
            .max()
            .unwrap();
        assert!((6000..10000).contains(&loud), "{loud}");
    }

    #[test]
    fn frames_decode_in_order_and_after_a_seek_back() {
        let m = parse(&video()).unwrap();
        assert!(m.audio.is_none());
        let mut p = Pictures::default();
        for n in [0, 3, 5, 6, 11, 2, 9, 4] {
            let want = if n < 6 { RED } else { BLUE };
            let got = colour(p.frame(stream(&m), n).unwrap());
            assert!(near(got, want), "frame {n}: {got:?}");
        }
    }

    #[test]
    fn other_files_and_mpeg_2_are_refused() {
        assert_eq!(parse(b"RIFF....AVI ").unwrap_err(), Invalid);
        // A sequence extension after the header makes it MPEG-2.
        let mut v = video();
        let at = v.windows(4).position(|w| w == [0, 0, 1, GROUP]).unwrap();
        v.splice(at..at, [0, 0, 1, EXTENSION, 0x14, 0x8A, 0, 1, 0, 0]);
        assert_eq!(parse(&v).unwrap_err(), Invalid);
    }
}
