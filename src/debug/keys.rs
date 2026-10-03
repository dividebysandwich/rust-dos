//! Remote input's keys are the PC keys of `keyboard::KEYS`, and text-mode
//! screen dumps' characters those of code page 437.

pub use rust_dos::keyboard::{PcKey, lookup, names, MOD_ALT, MOD_CTRL, MOD_LSHIFT, MOD_RSHIFT};
pub use rust_dos::video::CP437;
