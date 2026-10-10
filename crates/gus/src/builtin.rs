//! The Ultrasound software built into rust-dos: the Gravis General MIDI
//! patch set (`MIDI\*.PAT`) and the files that map it (`ULTRASND.INI`,
//! `MIDI\ULTRAMID.INI`, ...), as the Gravis installer leaves them in
//! `C:\ULTRASND`. With the Ultrasound installed they are on a read-only
//! drive of their own, in `\ULTRASND`, and ULTRADIR points there, so
//! programs find the patches without a Gravis installation.
//!
//! The files are in `assets/ultrasnd`; `build.rs` lists them.

/// The files, by path in the Ultrasound directory ("MIDI\ACPIANO.PAT").
pub static FILES: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/ultrasnd.rs"));

/// The directory on the drive that holds them.
pub const DIR: &str = "ULTRASND";

/// Volume label of the drive.
pub const LABEL: &str = "ULTRASND";

/// The file at `path` in the Ultrasound directory.
pub fn file(path: &str) -> Option<&'static [u8]> {
    FILES.iter().find(|(p, _)| p.eq_ignore_ascii_case(path)).map(|&(_, data)| data)
}

#[cfg(test)]
mod tests {
    use crate::patch::PatchBank;

    #[test]
    fn every_patch_the_bank_names_is_there() {
        let mut bank = PatchBank::builtin();
        for program in 0..128 {
            assert!(bank.melodic(program).is_some(), "program {}", program);
        }
        // The General MIDI drum keys.
        for key in 35..=81 {
            assert!(bank.drum(key).is_some(), "drum {}", key);
        }
        assert!(bank.missing.is_empty(), "{:?}", bank.missing);
    }
}
