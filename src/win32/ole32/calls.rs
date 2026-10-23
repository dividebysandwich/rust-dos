//! OLE32's functions that call into the program: methods of other
//! objects the program gave them (a stream's CopyTo writes to the
//! target's Write; PropVariantClear releases the objects it holds,
//! PropVariantCopy adds a reference to them). They are one task
//! (task.rs), whose first variable says which function it is.

use crate::cpu::Cpu;
use crate::win32::task::{self, Next, Task};

/// The functions, by the first variable.
pub const COPY_TO: u32 = 0;
pub const CALL_ALL: u32 = 1;

/// Start the task with `vars` (the function first) and `scratch` bytes of
/// scratch space.
pub fn start(cpu: &mut Cpu, vars: Vec<u32>, scratch: u32) -> Option<u32> {
    task::start(cpu, task::OLE32, vars, scratch)
}

/// The task's step.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.var(0) {
        COPY_TO => super::stream::copy_to_step(cpu, t, value),
        CALL_ALL => call_all_step(t),
        _ => Next::Done(crate::win32::com::E_INVALIDARG),
    }
}

/// Call the methods at variables 2 on, each with the interface after it,
/// one after another, then return variable 1.
fn call_all_step(t: &mut Task) -> Next {
    let i = 2 + t.step as usize * 2;
    if i + 1 >= t.vars.len() {
        return Next::Done(t.var(1));
    }
    let (method, this) = (t.var(i), t.var(i + 1));
    Next::call(method, &[this], t.step + 1)
}
