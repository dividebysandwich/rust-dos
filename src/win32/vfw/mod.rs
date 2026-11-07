//! Video for Windows: AVIFIL32.DLL, which reads AVI files, and
//! MSVFW32.DLL, whose DrawDib functions decompress and draw their
//! frames. Games of the late 1990s play their movies with the two:
//! Grom reads its intro with AVIStreamRead and draws each frame with
//! DrawDibDraw, and ACM 1918 takes its frames from AVIStreamGetFrame.
//! codec.rs has the codecs, Indeo Video 3, 4 and 5.

pub mod avi;
pub mod avifil32;
pub mod cinepak;
pub mod codec;
pub mod indeo4;
pub mod msvfw32;

/// What a process has open of the two: its AVI files, their
/// IGetFrames, its DrawDib handles, and the codecs it opened with ICOpen,
/// each by its FourCC in capitals.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub files: Vec<Option<avifil32::AviFile>>,
    pub frames: Vec<Option<avifil32::GetFrame>>,
    pub drawdibs: Vec<Option<msvfw32::DrawDib>>,
    pub codecs: Vec<Option<u32>>,
}

crate::state_fields!(State {
    files,
    frames,
    drawdibs,
    codecs
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
