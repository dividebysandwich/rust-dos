//! The most-recently-used lists of the shell and its programs:
//! CreateMRUListA and the functions on them.
//!
//! A placeholder: the window procedure draws nothing yet and the
//! functions fail.

use super::script::{Ctx, Op};
use crate::cpu::Cpu;

/// What the module keeps, in COMCTL32's `State`.
#[derive(Clone, Debug, Default)]
pub struct State {}

crate::state_fields!(State {});

/// The continuations of its scripts.
pub fn then(_cpu: &mut Cpu, _id: u16, _c: &mut Ctx) -> Vec<Op> {
    Vec::new()
}

/// CreateMRUListA.
pub fn create_mru_list(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// FreeMRUList.
pub fn free_mru_list(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// AddMRUStringA.
pub fn add_mru_string(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// EnumMRUListA.
pub fn enum_mru_list(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// FindMRUStringA.
pub fn find_mru_string(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// DelMRUString.
pub fn del_mru_string(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// CreateMRUListLazyA.
pub fn create_mru_list_lazy(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// AddMRUData.
pub fn add_mru_data(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// FindMRUData.
pub fn find_mru_data(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}
