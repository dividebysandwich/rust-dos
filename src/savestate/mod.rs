//! Save states of the whole emulated machine. The format and the `State`
//! trait every device implements are in the `rust-dos-savestate` crate;
//! here are the machine's own sections, the save slots and rewind.

pub mod disks;
pub mod machine;
pub mod rewind;
pub mod slots;

pub use rust_dos_savestate::*;
