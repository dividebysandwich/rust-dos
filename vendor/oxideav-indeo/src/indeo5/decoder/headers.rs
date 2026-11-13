//! Picture start, GOP header, frame header and band header (spec 01 and
//! spec 02).

use super::Iv50Error;
use super::bits::Bits;
use super::huff::read_descriptor;

/// Picture sizes of ids 0 to 14 (spec 02, 1.3), as (width, height).
const PICTURE_SIZES: [(u32, u32); 15] = [
    (640, 480),
    (320, 240),
    (160, 120),
    (704, 480),
    (352, 240),
    (352, 288),
    (176, 144),
    (240, 180),
    (640, 240),
    (704, 240),
    (80, 60),
    (88, 72),
    (0, 0),
    (0, 0),
    (0, 0),
];

/// The picture start fields (spec 01, "Picture start").
#[derive(Debug, Clone, Copy)]
pub(crate) struct PictureStart {
    pub frame_type: u32,
    pub frame_number: u32,
}

/// Read the picture start: a 5-bit start code 0x1f, a 3-bit frame type
/// and an 8-bit frame number.
pub(crate) fn read_picture_start(r: &mut Bits) -> Result<PictureStart, Iv50Error> {
    let code = r.read(5);
    let frame_type = r.read(3);
    let frame_number = r.read(8);
    if code != 0x1f || frame_type > 4 {
        return Err(Iv50Error::BadPictureStart);
    }
    Ok(PictureStart {
        frame_type,
        frame_number,
    })
}

/// One band's entry of the GOP header's band info (spec 02, 1.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BandInfo {
    /// Motion vectors in half samples.
    pub half_pel: bool,
    /// Macroblock size in samples.
    pub mb_size: u32,
    /// Block size in samples.
    pub blk_size: u32,
    /// Transform type, the band class of spec 02, 1.4 (06, 2.1).
    pub xform: u32,
}

/// The GOP header (spec 02, section 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GopHeader {
    pub flags: u32,
    pub luma_bands: usize,
    pub chroma_bands: usize,
    pub width: u32,
    pub height: u32,
    /// Band info for the luma bands, then the chroma bands.
    pub luma_info: Vec<BandInfo>,
    pub chroma_info: Vec<BandInfo>,
    /// gop_flags bit 3: frames carry a transparency trailer.
    pub transparency: bool,
    /// gop_flags bit 2: the stream has intra frames only.
    pub intra_only: bool,
}

impl GopHeader {
    /// Chroma plane size, a quarter of the luma size (spec 02, 1.3).
    pub fn chroma_size(&self) -> (u32, u32) {
        ((self.width + 3) >> 2, (self.height + 3) >> 2)
    }
}

/// Read the 6-bit band info of one band (spec 02, 1.4). `p` is 0 for
/// luma and 1 for chroma.
fn read_band_info(r: &mut Bits, index: usize, p: u32) -> Result<BandInfo, Iv50Error> {
    let v = r.read(6);
    let s = (v >> 1) & 3;
    let class = if v >= 8 {
        // A 2-bit field follows that must be 0; the DLL stops the
        // process otherwise.
        if r.read(2) != 0 {
            return Err(Iv50Error::BadHeader("band info guard bits"));
        }
        ((v >> 4) & 3) + 4 * p
    } else {
        index as u32 + 4 * p
    };
    Ok(BandInfo {
        half_pel: v & 1 != 0,
        mb_size: [16, 8, 8, 4][s as usize],
        blk_size: [8, 8, 4, 4][s as usize],
        xform: class,
    })
}

/// Read the GOP header that follows the picture start of a type 0
/// frame, up to and including the align before the frame header.
pub(crate) fn read_gop(r: &mut Bits) -> Result<GopHeader, Iv50Error> {
    let flags = r.read(8);
    if flags & 1 != 0 {
        // GOP header size: the DLL skips it.
        r.read(16);
    }
    if flags & 0x20 != 0 {
        // Access key (spec 02, section 6): a locked stream needs a key
        // that this decoder has no way to receive.
        return Err(Iv50Error::Unsupported("locked stream (gop_flags bit 5)"));
    }
    if flags & 0x40 != 0 {
        // Several tiles: how a chroma band's tile size follows from the
        // luma tile is unclear (impl-questions.md, Q3).
        return Err(Iv50Error::Unsupported("multiple tiles (gop_flags bit 6)"));
    }
    if flags & 2 != 0 {
        // Half-size chroma: spec 08 gives the window scaling for
        // quarter-size chroma only (impl-questions.md, Q4).
        return Err(Iv50Error::Unsupported("half-size chroma (gop_flags bit 1)"));
    }
    let sub = r.read(3);
    if !matches!(sub, 0 | 1 | 2 | 6) {
        return Err(Iv50Error::BadHeader("subsampling code"));
    }
    if sub & 2 != 0 {
        // Codes 2 and 6: seven luma bands (and four chroma bands for 6).
        // The DLL leaves a seven-band plane unwritten (spec 08, 8.7).
        return Err(Iv50Error::Unsupported(
            "seven-band luma or four-band chroma (subsampling codes 2 and 6)",
        ));
    }
    let luma_bands = ((sub & 3) * 3 + 1) as usize;
    let chroma_bands = ((sub >> 2) * 3 + 1) as usize;
    let size_id = r.read(4);
    let (width, height) = if size_id == 15 {
        let v = r.read(26);
        (v >> 13, v & 0x1fff)
    } else {
        PICTURE_SIZES[size_id as usize]
    };
    if width == 0 || height == 0 {
        return Err(Iv50Error::Unsupported("picture size 0x0"));
    }
    let mut luma_info = Vec::with_capacity(luma_bands);
    for b in 0..luma_bands {
        luma_info.push(read_band_info(r, b, 0)?);
    }
    let mut chroma_info = Vec::with_capacity(chroma_bands);
    for b in 0..chroma_bands {
        chroma_info.push(read_band_info(r, b, 1)?);
    }
    let transparency = flags & 8 != 0;
    if transparency {
        let t = r.read(4);
        if t & 7 != 0 {
            return Err(Iv50Error::BadHeader("transparency flags"));
        }
        if t & 8 != 0 {
            // Transparency colour: only the application uses it.
            r.read(24);
        }
    }
    r.align();
    let version_word = r.read(24);
    let version = (version_word >> 19) & 0xf;
    if version != 2 {
        // Other versions make the DLL draw a "version" picture over the
        // output (spec 02, 1.7; spec 08, 8.10).
        return Err(Iv50Error::Unsupported("bitstream version other than 2"));
    }
    if version_word & (1 << 23) != 0 {
        loop {
            let w = r.read(16);
            if w & 0x8000 == 0 || r.overrun() {
                break;
            }
        }
    }
    r.align();
    Ok(GopHeader {
        flags,
        luma_bands,
        chroma_bands,
        width,
        height,
        luma_info,
        chroma_info,
        transparency,
        intra_only: flags & 4 != 0,
    })
}

/// The frame header (spec 02, section 2).
#[derive(Debug, Clone)]
pub(crate) struct FrameHeader {
    pub flags: u32,
    /// Macroblock Huffman descriptor (preset or custom).
    pub mb_rows: Vec<u8>,
}

/// Read the frame header, up to and including its final align.
pub(crate) fn read_frame_header(r: &mut Bits) -> Result<FrameHeader, Iv50Error> {
    let flags = r.read(8);
    if flags & 1 != 0 {
        // Frame size: only used for a check this decoder skips (2.2).
        r.read(24);
    }
    if flags & 0x10 != 0 {
        // Frame checksum: read only by the access key check.
        r.read(16);
    }
    if flags & 0x20 != 0 {
        // Frame extension: chunks of an 8-bit length and that many
        // bytes, ended by a length of 0.
        loop {
            let n = r.read(8);
            if n == 0 || r.overrun() {
                break;
            }
            r.skip(n * 8);
        }
    }
    let mut mb_rows = super::huff::MB_PRESETS[7].to_vec();
    if flags & 0x40 != 0 {
        let sel = r.read(3);
        mb_rows = if sel == 7 {
            read_descriptor(r)
        } else {
            super::huff::MB_PRESETS[sel as usize].to_vec()
        };
    }
    // Plane filter bits (2.4); see impl-questions.md.
    r.read(3);
    r.align();
    if r.overrun() {
        return Err(Iv50Error::Truncated);
    }
    Ok(FrameHeader { flags, mb_rows })
}

/// The band header (spec 02, section 3).
#[derive(Debug, Clone)]
pub(crate) struct BandHeader {
    pub flags: u32,
    /// Byte length of the whole band, if the frame carries band sizes.
    pub size: Option<u32>,
    /// Correction pairs (spec 04, 3.5).
    pub corrections: Vec<(u8, u8)>,
    /// Run/value table slot.
    pub rv_table: usize,
    /// Block Huffman descriptor.
    pub blk_rows: Vec<u8>,
    /// Global quantiser.
    pub gq: u32,
}

impl BandHeader {
    pub fn empty(&self) -> bool {
        self.flags & 1 != 0
    }
    pub fn inherit(&self) -> bool {
        self.flags & 2 != 0
    }
    pub fn has_delta(&self) -> bool {
        self.flags & 4 != 0
    }
    /// The band reads quantiser deltas of its own (bit 2 set, bit 3
    /// clear).
    pub fn reads_delta(&self) -> bool {
        self.flags & 0xc == 4
    }
}

/// Read a band header starting at the reader's (byte-aligned) position,
/// up to and including the align before the first tile.
pub(crate) fn read_band_header(r: &mut Bits, frame_flags: u32) -> Result<BandHeader, Iv50Error> {
    let flags = r.read(8);
    let mut h = BandHeader {
        flags,
        size: None,
        corrections: Vec::new(),
        rv_table: 8,
        blk_rows: super::huff::BLOCK_PRESETS[7].to_vec(),
        gq: 0,
    };
    if flags & 1 != 0 {
        // Empty band: the header is this one byte.
        return Ok(h);
    }
    if frame_flags & 0x80 != 0 {
        h.size = Some(r.read(24));
    }
    if flags & 0x10 != 0 {
        let n = r.read(8);
        if n > 61 {
            return Err(Iv50Error::BadHeader("correction count above 61"));
        }
        for _ in 0..n {
            let a = r.read(8) as u8;
            let b = r.read(8) as u8;
            h.corrections.push((a, b));
        }
    }
    if flags & 0x40 != 0 {
        h.rv_table = r.read(3) as usize;
    }
    if flags & 0x80 != 0 {
        let sel = r.read(3);
        h.blk_rows = if sel == 7 {
            read_descriptor(r)
        } else {
            super::huff::BLOCK_PRESETS[sel as usize].to_vec()
        };
    }
    if r.bit() {
        // Band checksum: no reader in the DLL.
        r.read(16);
    }
    h.gq = r.read(5);
    if flags & 0x20 != 0 {
        // Band extension: align, then chunks of an 8-bit length, that
        // many bytes and a bit that says whether another chunk follows.
        r.align();
        loop {
            let n = r.read(8);
            r.skip(n * 8);
            if !r.bit() || r.overrun() {
                break;
            }
        }
    }
    r.align();
    if r.overrun() {
        return Err(Iv50Error::Truncated);
    }
    Ok(h)
}

/// Transparency rectangles of the frame trailer (spec 02, section 5;
/// spec 08, 8.9). Returns the rectangles as (x, y, w, h).
pub(crate) fn read_transparency_rects(r: &mut Bits, frame_flags: u32) -> Vec<(u32, u32, u32, u32)> {
    if frame_flags & 1 != 0 {
        r.read(24);
    }
    let n = r.read(3);
    (0..n)
        .map(|_| {
            let x = r.read(16);
            let y = r.read(16);
            let w = r.read(16);
            let h = r.read(16);
            (x, y, w, h)
        })
        .collect()
}
