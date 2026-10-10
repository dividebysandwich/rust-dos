//! The Sound Blaster family: the cards' DSP, mixer and DMA (`sb`), from
//! the SB 1.x to the SB16, and the AWE32's wavetable synthesizer, the
//! EMU8000 (`awe32`). The FM synthesizer is the `rust-dos-opl` crate's.

pub mod awe32;
pub mod sb;
