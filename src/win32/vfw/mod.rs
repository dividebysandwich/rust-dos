//! Video for Windows: AVIFIL32.DLL, which reads AVI files, and
//! MSVFW32.DLL, whose DrawDib functions decompress and draw their
//! frames. Games of the late 1990s play their movies with the two:
//! Grom reads its intro with AVIStreamRead and draws each frame with
//! DrawDibDraw. Indeo Video 3 (IV31, IV32), which Windows 98 has a
//! codec for, is decompressed by the `oxideav-indeo` crate; frames of
//! other codecs draw as black.

pub mod avi;
pub mod avifil32;
pub mod msvfw32;

/// What a process has open of the two: its AVI files and its DrawDib
/// handles.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub files: Vec<Option<avifil32::AviFile>>,
    pub drawdibs: Vec<Option<msvfw32::DrawDib>>,
}

crate::state_fields!(State { files, drawdibs });
