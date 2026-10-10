//! The Roland MT-32 and CM-32L on the MPU-401 (`midisynth=mt32`), played by
//! munt's emulation of them, libmt32emu. rust-dos doesn't link the library:
//! it loads it when the MT-32 is chosen, so the program runs without munt
//! installed, and plays the MT-32 wherever it is. The synthesizer needs the
//! module's ROMs, a control ROM and a PCM ROM of the same model, which
//! munt identifies from their contents whatever the files are called.
//!
//! The model setting is here everywhere; the synthesizer only where there
//! are libraries to load (not in the browser).

#[cfg(not(target_arch = "wasm32"))]
mod munt;
#[cfg(not(target_arch = "wasm32"))]
pub use munt::*;

/// The model the ROMs are for (`mt32model`): the MT-32, or the CM-32L with
/// its extra sound effects. `Auto` takes the CM-32L's ROMs when they are
/// there, as DOSBox Staging does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mt32Model {
    Auto,
    Mt32,
    Cm32l,
}

impl Mt32Model {
    pub const ALL: [Mt32Model; 3] = [Mt32Model::Auto, Mt32Model::Mt32, Mt32Model::Cm32l];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Mt32Model::Auto),
            "mt32" | "mt-32" => Some(Mt32Model::Mt32),
            "cm32l" | "cm-32l" => Some(Mt32Model::Cm32l),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mt32Model::Auto => "auto",
            Mt32Model::Mt32 => "mt32",
            Mt32Model::Cm32l => "cm32l",
        }
    }

    /// As the settings window shows it.
    pub fn describe(self) -> &'static str {
        match self {
            Mt32Model::Auto => "auto (CM-32L, else MT-32)",
            Mt32Model::Mt32 => "MT-32",
            Mt32Model::Cm32l => "CM-32L",
        }
    }
}
