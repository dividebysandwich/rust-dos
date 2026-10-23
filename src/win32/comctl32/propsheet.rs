//! Property sheets and wizards: PropertySheetA,
//! CreatePropertySheetPageA, DestroyPropertySheetPage.
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

/// CreatePropertySheetPage.
pub fn create_property_sheet_page(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// A function Windows 95 has no Unicode version of: it fails.
pub fn fail(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// DestroyPropertySheetPage.
pub fn destroy_property_sheet_page(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// PropertySheet.
pub fn property_sheet(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// A function Windows 95 has no Unicode version of: -1.
pub fn fail_minus_one(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(u32::MAX)
}
