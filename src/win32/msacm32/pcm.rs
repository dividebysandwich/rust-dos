//! The PCM converter built into Windows' MSACM32: it converts PCM to PCM
//! of other channels (mono to stereo and back), bits (8 and 16) and rate.
//! What it writes follows Wine's converter (`dlls/msacm32/pcmconverter.c`):
//! mono becomes stereo by copying each sample to both channels, stereo
//! becomes mono as the sum of its two channels, clipped, and a rate
//! changes by dropping or repeating whole frames, with no filter.

/// A PCM format the converter takes: mono or stereo, 8 or 16 bits a
/// sample, at any rate but 0.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pcm {
    pub channels: u16,
    pub rate: u32,
    pub bits: u16,
}

crate::state_fields!(Pcm {
    channels,
    rate,
    bits
});

impl Pcm {
    /// The format of these fields of WAVEFORMATEX, if the converter
    /// takes it. `align` is nBlockAlign, which has to be a frame's bytes.
    pub fn new(channels: u16, rate: u32, bits: u16, align: u16) -> Option<Pcm> {
        let pcm = Pcm {
            channels,
            rate,
            bits,
        };
        (matches!(channels, 1 | 2)
            && matches!(bits, 8 | 16)
            && rate != 0
            && u32::from(align) == pcm.frame())
        .then_some(pcm)
    }

    /// The bytes of a frame: a sample of each channel.
    pub fn frame(&self) -> u32 {
        u32::from(self.channels) * u32::from(self.bits) / 8
    }

    /// The bytes of a second.
    fn per_second(&self) -> u64 {
        u64::from(self.rate) * u64::from(self.frame())
    }

    /// The frame at `at` of `bytes` as 16-bit samples of `channels`
    /// channels, the first `channels` of the result. A mono sample goes
    /// to both channels of stereo, and stereo becomes mono as the sum of
    /// its channels, clipped. 8-bit samples are unsigned and become their
    /// 16-bit value with the low byte 0.
    fn read(&self, bytes: &[u8], at: usize, channels: u16) -> [i16; 2] {
        let sample = |i: usize| match self.bits {
            8 => (i16::from(bytes[i]) - 128) << 8,
            _ => i16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]),
        };
        let first = at * self.channels as usize;
        match (self.channels, channels) {
            (1, _) => [sample(first); 2],
            (_, 1) => {
                let sum = i32::from(sample(first)) + i32::from(sample(first + 1));
                [sum.clamp(-32768, 32767) as i16, 0]
            }
            _ => [sample(first), sample(first + 1)],
        }
    }

    /// Append the first of `frame`'s samples, as many as this format has
    /// channels. 8 bits keep a sample's high byte.
    fn write(&self, frame: [i16; 2], out: &mut Vec<u8>) {
        for &v in &frame[..self.channels as usize] {
            match self.bits {
                8 => out.push(((v >> 8) as u8) ^ 0x80),
                _ => out.extend_from_slice(&v.to_le_bytes()),
            }
        }
    }
}

/// acmStreamSize from `from` to `to`: the bytes of `to` that `bytes` of
/// `from` convert to, by their bytes a second, rounded up and then down
/// to whole frames of `to`. None if that passes 4 GB. A program's rate
/// can be any 32-bit number, so the product takes 128 bits.
pub fn size(from: Pcm, to: Pcm, bytes: u32) -> Option<u32> {
    let whole = u128::from(bytes - bytes % from.frame());
    let rate = u128::from(from.per_second());
    let converted = (whole * u128::from(to.per_second())).div_ceil(rate);
    let converted = u32::try_from(converted).ok()?;
    Some(converted - converted % to.frame())
}

/// acmStreamConvert from `from` to `to`: convert the whole frames of
/// `source` that fit in `room` bytes. Returns the source's bytes used
/// and what they converted to. At the same rate each frame of the
/// source makes one of the destination. Otherwise, as in Wine, each
/// source frame is written as many times as the rates' ratio says, an
/// error term starting at half the source's rate, and the conversion
/// stops when the destination is full.
pub fn convert(from: Pcm, to: Pcm, source: &[u8], room: u32) -> (u32, Vec<u8>) {
    let frames = source.len() / from.frame() as usize;
    let fit = (room / to.frame()) as usize;
    let mut out = Vec::with_capacity(fit * to.frame() as usize);
    if from.rate == to.rate {
        let n = frames.min(fit);
        for at in 0..n {
            to.write(from.read(source, at, to.channels), &mut out);
        }
        return (n as u32 * from.frame(), out);
    }
    let (source_rate, rate) = (u64::from(from.rate), u64::from(to.rate));
    let mut error = source_rate / 2;
    let mut written = 0;
    let mut used = 0;
    'frames: while used < frames {
        error += rate;
        while error > source_rate {
            if written == fit {
                break 'frames;
            }
            written += 1;
            error -= source_rate;
            to.write(from.read(source, used, to.channels), &mut out);
        }
        used += 1;
    }
    (used as u32 * from.frame(), out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(channels: u16, rate: u32, bits: u16) -> Pcm {
        Pcm::new(channels, rate, bits, channels * bits / 8).unwrap()
    }

    #[test]
    fn takes_mono_or_stereo_of_8_or_16_bits_with_whole_frames() {
        assert!(Pcm::new(2, 22050, 16, 4).is_some());
        assert!(Pcm::new(3, 22050, 16, 6).is_none());
        assert!(Pcm::new(1, 22050, 24, 3).is_none());
        assert!(Pcm::new(1, 0, 8, 1).is_none());
        assert!(Pcm::new(2, 22050, 16, 2).is_none());
    }

    #[test]
    fn stereo_16_to_mono_8_sums_and_clips() {
        let source: Vec<u8> = [1000i16, 2000, 30000, 30000, -32768, -1]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let (used, out) = convert(pcm(2, 11025, 16), pcm(1, 11025, 8), &source, 100);
        assert_eq!(used, 12);
        // 3000 is 0x0BB8, the clipped sums 0x7FFF and -0x8000.
        assert_eq!(out, [0x0B ^ 0x80, 0x7F ^ 0x80, 0x80 ^ 0x80]);
    }

    #[test]
    fn mono_8_to_stereo_16_copies_each_sample() {
        let (used, out) = convert(pcm(1, 8000, 8), pcm(2, 8000, 16), &[0x80, 0xFF, 0x00], 8);
        // Two frames fit in 8 bytes.
        assert_eq!(used, 2);
        assert_eq!(out, [0, 0, 0, 0, 0, 0x7F, 0, 0x7F]);
    }

    #[test]
    fn doubling_the_rate_repeats_each_frame() {
        let (used, out) = convert(pcm(1, 11025, 8), pcm(1, 22050, 8), &[1, 2, 3], 100);
        assert_eq!(used, 3);
        assert_eq!(out, [1, 1, 2, 2, 3, 3]);
    }

    #[test]
    fn halving_the_rate_drops_every_other_frame() {
        let (used, out) = convert(pcm(1, 22050, 8), pcm(1, 11025, 8), &[1, 2, 3, 4], 100);
        assert_eq!(used, 4);
        assert_eq!(out, [2, 4]);
    }

    #[test]
    fn a_full_destination_stops_the_conversion() {
        let (used, out) = convert(pcm(1, 11025, 8), pcm(1, 22050, 8), &[1, 2, 3], 3);
        // The second frame's copy doesn't fit, so it isn't used.
        assert_eq!(used, 1);
        assert_eq!(out, [1, 1, 2]);
    }

    #[test]
    fn sizes_by_the_bytes_of_a_second() {
        let (from, to) = (pcm(2, 44100, 16), pcm(1, 22050, 8));
        assert_eq!(size(from, to, 4000), Some(500));
        assert_eq!(size(to, from, 500), Some(4000));
        // 1006 bytes hold 251 frames, which make 125.5 bytes, rounded up.
        assert_eq!(size(from, to, 1006), Some(126));
        // Up, then down to whole frames of 4 bytes: 16.54 to 17 to 16.
        assert_eq!(size(pcm(1, 8000, 8), pcm(2, 11025, 16), 3), Some(16));
        assert_eq!(size(pcm(1, 11025, 8), pcm(2, 11025, 16), u32::MAX), None);
        // The largest rate and size: past 64 bits before the division.
        let fastest = pcm(2, u32::MAX, 16);
        assert_eq!(size(fastest, fastest, u32::MAX), Some(u32::MAX - 3));
        assert_eq!(size(pcm(1, 1, 8), fastest, u32::MAX), None);
    }
}
