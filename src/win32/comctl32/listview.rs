//! The list view (SysListView32).
//!
//! A placeholder: the window procedure draws nothing yet and the
//! functions fail.

use super::script::{Ctx, Op};
use crate::cpu::Cpu;
use crate::win32::user32::defwnd;

/// What the module keeps, in COMCTL32's `State`.
#[derive(Clone, Debug, Default)]
pub struct State {}

crate::state_fields!(State {});

/// The window procedure.
pub fn proc_(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    defwnd::def_window_proc(cpu, args)
}

/// The continuations of its scripts.
pub fn then(_cpu: &mut Cpu, _id: u16, _c: &mut Ctx) -> Vec<Op> {
    Vec::new()
}
