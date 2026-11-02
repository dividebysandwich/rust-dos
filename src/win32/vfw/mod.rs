//! Video for Windows: AVIFIL32.DLL, which reads AVI files, and
//! MSVFW32.DLL, whose DrawDib functions decompress and draw their
//! frames. Games of the late 1990s play their movies with the two:
//! Grom reads its intro with AVIStreamRead and draws each frame with
//! DrawDibDraw, and ACM 1918 takes its frames from AVIStreamGetFrame.
//! codec.rs has the codecs, Indeo Video 3 and 5.

pub mod avi;
pub mod avifil32;
pub mod codec;
pub mod msvfw32;

/// What a process has open of the two: its AVI files, their
/// IGetFrames and its DrawDib handles.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub files: Vec<Option<avifil32::AviFile>>,
    pub frames: Vec<Option<avifil32::GetFrame>>,
    pub drawdibs: Vec<Option<msvfw32::DrawDib>>,
}

crate::state_fields!(State {
    files,
    frames,
    drawdibs
});
