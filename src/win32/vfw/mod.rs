//! Video for Windows: AVIFIL32.DLL, which reads AVI files, and
//! MSVFW32.DLL, whose DrawDib functions decompress and draw their
//! frames. Games of the late 1990s play their movies with the two:
//! Grom reads its intro with AVIStreamRead and draws each frame with
//! DrawDibDraw, and ACM 1918 takes its frames from AVIStreamGetFrame.
//! codec.rs has the codecs: Indeo Video 3, 4 and 5, Cinepak and
//! Microsoft Video 1. Biosys decompresses its Video 1 films itself, with
//! ICLocate and ICDecompress, and Outlaw Racers its Cinepak ones.

pub mod avi;
pub mod avifil32;
pub mod cinepak;
pub mod codec;
pub mod indeo4;
pub mod msvfw32;
pub mod msvideo1;

/// What a process has open of the two: its AVI files, their
/// IGetFrames, its DrawDib handles, and the codecs it opened with ICOpen
/// or ICLocate, each by its FourCC in capitals, with the decoder that
/// ICDecompress decodes its frames with and the palette the program gave
/// it with ICM_DECOMPRESS_SET_PALETTE. A save state doesn't keep the
/// decoders, as it doesn't a DrawDib's: after a restore a handle's
/// decoder starts again, and refuses the frames up to the next key
/// frame.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub files: Vec<Option<avifil32::AviFile>>,
    pub frames: Vec<Option<avifil32::GetFrame>>,
    pub drawdibs: Vec<Option<msvfw32::DrawDib>>,
    pub codecs: Vec<Option<u32>>,
    /// By the index of `codecs`: the decoder of the handle there.
    pub decoders: Vec<codec::Codec>,
    /// By the index of `codecs`: the colours, as COLORREFs, of the
    /// palette ICM_DECOMPRESS_SET_PALETTE gave the handle there when
    /// they aren't the codec's own (`msvfw32::set_palette`).
    pub palettes: crate::savestate::Since<{ PALETTES_SAVED }, Vec<Option<Vec<u32>>>>,
}

/// The W32 section's version that saves the palettes the codecs were
/// given (`State::palettes`). An older state's handles load with none,
/// and decode in the codec's own palette.
pub const PALETTES_SAVED: u16 = 122;

crate::state_fields!(State {
    files,
    frames,
    drawdibs,
    codecs,
    palettes
} skip {
    decoders
});

/// The index of the first empty entry of `list`, which a new handle takes
/// as `index + 1`: a closed handle's, or a new one pushed at the end.
fn free_slot<T>(list: &mut Vec<Option<T>>) -> usize {
    let slot = list.iter().position(Option::is_none).unwrap_or(list.len());
    if slot == list.len() {
        list.push(None);
    }
    slot
}
