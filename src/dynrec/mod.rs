//! The dynamic recompiler: translates blocks of guest instructions into
//! host machine code, as DOSBox's dynamic core does. See docs/dynrec.md.

/// Whether this build has a code generator for its host.
pub const AVAILABLE: bool = cfg!(dynrec);

/// The recompiler's state: its translated code and what it knows about it.
#[derive(Default)]
pub struct DynState {}

impl DynState {
    /// Forget all translated code.
    pub fn flush(&mut self) {}
}
