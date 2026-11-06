//! The two ADPCM decoders of Windows 98's codecs: Microsoft ADPCM
//! (`msadp32.acm`, format tag 2) and IMA ADPCM (`imaadp32.acm`, tag
//! 11h). Both code a block of samples on its own: a header per channel
//! starts the predictor, and 4-bit codes follow. They are decoded here
//! from the formats' definitions in Microsoft's "Multimedia Standards
//! Update" (1994) and the IMA's "Recommended Practices for Enhancing
//! Digital Audio Compatibility in Multimedia Systems" (1992).

/// WAVE_FORMAT_ADPCM and WAVE_FORMAT_IMA_ADPCM.
pub const MS_ADPCM: u16 = 2;
pub const IMA_ADPCM: u16 = 0x11;

/// How MS ADPCM's step changes with each code.
const MS_ADAPTATION: [i32; 16] = [
    230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230,
];

/// IMA ADPCM's steps and how its index moves with a code's magnitude.
const IMA_STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const IMA_INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// An ADPCM format: what decoding a block needs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Format {
    pub tag: u16,
    pub channels: u16,
    pub block_align: u16,
    pub samples_per_block: u16,
    /// MS ADPCM's coefficient pairs; IMA ADPCM has none.
    pub coefficients: Vec<(i16, i16)>,
}

crate::state_fields!(Format {
    tag,
    channels,
    block_align,
    samples_per_block,
    coefficients
});

impl Format {
    /// The bytes of a block's headers, all channels'.
    fn header(&self) -> usize {
        self.channels as usize * if self.tag == MS_ADPCM { 7 } else { 4 }
    }

    /// The samples of each channel a block of `bytes` bytes holds: its
    /// header's, and two codes a byte after it, which IMA ADPCM takes in
    /// groups of 4 bytes a channel. 0 if it has no more than headers.
    pub fn samples_in(&self, bytes: usize) -> u32 {
        let ch = self.channels as usize;
        let Some(data) = bytes.checked_sub(self.header()).filter(|&d| d > 0) else {
            return 0;
        };
        (if self.tag == MS_ADPCM {
            data * 2 / ch + 2
        } else {
            data / (4 * ch) * 8 + 1
        }) as u32
    }

    /// The samples of each channel a whole block holds.
    pub fn samples_in_block(&self) -> u32 {
        self.samples_in(self.block_align as usize)
    }

    /// Whether the codec decodes this format: one or two channels, a
    /// block longer than its headers, of whole groups of codes, and no
    /// more samples a block than its bytes hold. MS ADPCM also needs its
    /// coefficients.
    pub fn valid(&self) -> bool {
        if !(1..=2).contains(&self.channels) || self.block_align as usize <= self.header() {
            return false;
        }
        // IMA ADPCM's codes come in 4 bytes a channel.
        let data = self.block_align as usize - self.header();
        if self.tag == IMA_ADPCM && !data.is_multiple_of(4 * self.channels as usize) {
            return false;
        }
        if self.tag == MS_ADPCM && self.coefficients.is_empty() {
            return false;
        }
        let first = if self.tag == MS_ADPCM { 2 } else { 1 };
        (first..=self.samples_in_block()).contains(&(self.samples_per_block as u32))
    }

    /// Decode `frames` frames of a block into `out`, the samples of all
    /// channels interleaved. `frames` is at most what the block's bytes
    /// hold (`samples_in`), and at least 1.
    pub fn decode(&self, block: &[u8], frames: usize, out: &mut Vec<i16>) {
        match self.tag {
            MS_ADPCM => self.decode_ms(block, frames, out),
            _ => self.decode_ima(block, frames, out),
        }
    }

    fn decode_ms(&self, block: &[u8], frames: usize, out: &mut Vec<i16>) {
        let ch = self.channels as usize;
        let word = |at: usize| i16::from_le_bytes([block[at], block[at + 1]]) as i32;
        // The header's fields, each for every channel: the predictor,
        // the step, then the last two samples, the later one first.
        let mut coef = [(0i32, 0i32); 2];
        let mut delta = [0i32; 2];
        let mut s1 = [0i32; 2];
        let mut s2 = [0i32; 2];
        for c in 0..ch {
            let index = (block[c] as usize).min(self.coefficients.len() - 1);
            let (c1, c2) = self.coefficients[index];
            coef[c] = (c1 as i32, c2 as i32);
            delta[c] = word(ch + 2 * c);
            s1[c] = word(3 * ch + 2 * c);
            s2[c] = word(5 * ch + 2 * c);
        }
        out.extend((0..ch).map(|c| s2[c] as i16));
        if frames < 2 {
            return;
        }
        out.extend((0..ch).map(|c| s1[c] as i16));
        // The codes, the high nibble first; in stereo the channels take
        // turns.
        let codes = (frames - 2) * ch;
        for n in 0..codes {
            let byte = block[self.header() + n / 2];
            let code = if n % 2 == 0 { byte >> 4 } else { byte & 15 } as i32;
            let c = n % ch;
            let signed = if code >= 8 { code - 16 } else { code };
            let predicted = (s1[c] * coef[c].0 + s2[c] * coef[c].1) >> 8;
            let sample = (predicted + signed * delta[c]).clamp(-32768, 32767);
            s2[c] = s1[c];
            s1[c] = sample;
            delta[c] = ((MS_ADAPTATION[code as usize] * delta[c]) >> 8).max(16);
            out.push(sample as i16);
        }
    }

    fn decode_ima(&self, block: &[u8], frames: usize, out: &mut Vec<i16>) {
        let ch = self.channels as usize;
        let start = out.len();
        out.resize(start + frames * ch, 0);
        for c in 0..ch {
            let mut sample = i16::from_le_bytes([block[4 * c], block[4 * c + 1]]) as i32;
            let mut index = (block[4 * c + 2] as i32).min(88);
            out[start + c] = sample as i16;
            // Each channel's codes come in groups of 4 bytes, 8 samples,
            // the channels' groups in turn, the low nibble first.
            for n in 0..frames - 1 {
                let group = n / 8;
                let at = self.header() + (group * ch + c) * 4 + (n % 8) / 2;
                let code = if n % 2 == 0 {
                    block[at] & 15
                } else {
                    block[at] >> 4
                } as i32;
                let step = IMA_STEPS[index as usize];
                let mut diff = step >> 3;
                if code & 4 != 0 {
                    diff += step;
                }
                if code & 2 != 0 {
                    diff += step >> 1;
                }
                if code & 1 != 0 {
                    diff += step >> 2;
                }
                sample = if code & 8 != 0 {
                    sample - diff
                } else {
                    sample + diff
                }
                .clamp(-32768, 32767);
                index = (index + IMA_INDEX[code as usize & 7]).clamp(0, 88);
                out[start + (n + 1) * ch + c] = sample as i16;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The coefficient pairs of an MS ADPCM format with the standard
    /// table, which every encoder writes, in 8.8 fixed point.
    pub const MS_COEFFICIENTS: [(i16, i16); 7] = [
        (256, 0),
        (512, -256),
        (0, 0),
        (192, 64),
        (240, 0),
        (460, -208),
        (392, -232),
    ];

    fn ms(channels: u16, block_align: u16) -> Format {
        let mut f = Format {
            tag: MS_ADPCM,
            channels,
            block_align,
            samples_per_block: 0,
            coefficients: MS_COEFFICIENTS.to_vec(),
        };
        f.samples_per_block = f.samples_in_block() as u16;
        f
    }

    #[test]
    fn the_formats_of_a_block_hold_the_samples_encoders_give_them() {
        // What Windows' encoders write for 22 kHz: MS ADPCM 512-byte
        // blocks mono, 1024 stereo; IMA ADPCM 512 and 1024.
        assert_eq!(ms(1, 512).samples_in_block(), 1012);
        assert_eq!(ms(2, 1024).samples_in_block(), 1012);
        let ima = |channels, block_align| Format {
            tag: IMA_ADPCM,
            channels,
            block_align,
            ..Format::default()
        };
        assert_eq!(ima(1, 512).samples_in_block(), 1017);
        assert_eq!(ima(2, 1024).samples_in_block(), 1017);
        assert!(ms(2, 1024).valid());
        let mut long = ms(2, 1024);
        long.samples_per_block += 1;
        assert!(!long.valid());
        assert!(!ms(3, 1024).valid());
    }

    #[test]
    fn ms_adpcm_starts_with_the_headers_samples_and_adapts_its_step() {
        // Mono, predictor 0 (1.0, 0): each code adds code * step to the
        // last sample. Step 16, samples 100 then 200 (sample 2 first).
        let mut block = vec![0, 16, 0, 200, 0, 100, 0];
        // Codes 7 then -8 (8), then 0 and 0.
        block.extend([0x78, 0x00]);
        let mut f = ms(1, block.len() as u16);
        f.samples_per_block = 6;
        let mut out = Vec::new();
        f.decode(&block, 6, &mut out);
        // 200 + 7*16 = 312, step 16*614/256 = 38; 312 - 8*38 = 8, step
        // 38*768/256 = 114; then zeros keep the sample.
        assert_eq!(out, [100, 200, 312, 8, 8, 8]);
    }

    #[test]
    fn ms_adpcm_stereo_takes_the_channels_in_turn() {
        // Left: predictor 0, step 16, samples 10, 20. Right: predictor 1
        // (2.0, -1.0), step 32, samples -5 then -10.
        let block = [
            0, 1, 16, 0, 32, 0, 20, 0, 0xF6, 0xFF, 10, 0, 0xFB, 0xFF, 0x11, 0x00,
        ];
        let mut f = ms(2, block.len() as u16);
        f.samples_per_block = 4;
        let mut out = Vec::new();
        f.decode(&block, 4, &mut out);
        // Left: 20 + 16 = 36, then 36 + 0. Right: 2*-10 - -5 = -15, plus
        // 32 = 17; then 2*17 - -10 = 44.
        assert_eq!(out, [10, -5, 20, -10, 36, 17, 36, 44]);
    }

    #[test]
    fn ima_adpcm_decodes_its_groups_of_codes_low_nibble_first() {
        // Mono: sample 1000, index 0 (step 7), then codes 7, 8 and six
        // zeros.
        let block = [0xE8, 0x03, 0, 0, 0x87, 0, 0, 0];
        let f = Format {
            tag: IMA_ADPCM,
            channels: 1,
            block_align: 8,
            samples_per_block: 9,
            coefficients: Vec::new(),
        };
        assert!(f.valid());
        let mut out = Vec::new();
        f.decode(&block, 9, &mut out);
        // Code 7: 7/8 + 7 + 7/2 + 7/4 = 0 + 7 + 3 + 1 = 11, index 8
        // (step 16). Code 8: minus 16/8 = 2, index 7 (step 14). Zeros:
        // plus step/8 each, the index falling: 14/8 = 1, 13/8 = 1,
        // 12/8 = 1, 11/8 = 1, 10/8 = 1, 9/8 = 1.
        assert_eq!(out, [1000, 1011, 1009, 1010, 1011, 1012, 1013, 1014, 1015]);
    }

    #[test]
    fn ima_adpcm_decodes_a_block_as_ffmpeg_does() {
        // A 32-byte block of ffmpeg 7's `adpcm_ima_wav` encoder, from a
        // 440 Hz sine with noise at 22050 Hz, and what its decoder makes
        // of it.
        let block = [
            0xE0, 0x03, 0x00, 0x00, 0xF7, 0x77, 0x7F, 0xF6, 0x96, 0xB0, 0x3A, 0x24, 0x2F, 0xB8,
            0xA3, 0xB1, 0x49, 0x98, 0x7A, 0x9B, 0x04, 0xBA, 0x01, 0x92, 0x90, 0x06, 0x18, 0xD0,
            0xC2, 0xA4, 0xC3, 0xB4,
        ];
        let ffmpeg: [i16; 57] = [
            992, 1003, 973, 1036, 1172, 879, 1510, 2686, 283, 4749, 2924, 3477, -45, -2332, 577,
            3979, 6266, 30, 4487, 3677, -1479, 3208, 165, 1825, -1697, -3069, 673, 170, -1202,
            -3280, 2390, -3283, -5492, 535, 1345, -2338, -7025, -5200, -4647, -2131, -3503, -3088,
            -4222, 244, 852, 299, 1808, 2265, -2308, 735, -4246, 1781, -2271, 2885, -3142, 4152,
            -2711,
        ];
        let f = Format {
            tag: IMA_ADPCM,
            channels: 1,
            block_align: 32,
            samples_per_block: 57,
            coefficients: Vec::new(),
        };
        assert!(f.valid());
        let mut out = Vec::new();
        f.decode(&block, 57, &mut out);
        assert_eq!(out, ffmpeg);
    }
}
