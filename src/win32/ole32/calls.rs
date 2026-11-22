//! OLE32's functions that call into the program: methods of other
//! objects the program gave them (a stream's CopyTo writes to the
//! target's Write; PropVariantClear releases the objects it holds,
//! PropVariantCopy adds a reference to them), the calls between
//! processes (rpc.rs, rpc/ndr.rs) and structured storage's helpers
//! (storage.rs, persist.rs). They are one task (task.rs), whose first
//! variable says which function it is.

use crate::cpu::Cpu;
use crate::win32::task::{self, Next, Task};

/// The functions, by the first variable.
pub const COPY_TO: u32 = 0;
pub const CALL_ALL: u32 = 1;
pub const CLASS: u32 = 2;
pub const MARSHAL: u32 = 3;
pub const UNMARSHAL: u32 = 4;
pub const PROXY: u32 = 5;
pub const STUB: u32 = 6;
pub const ACTIVATE: u32 = 7;
pub const STORAGE_COPY: u32 = 8;
pub const PERSIST: u32 = 9;
pub const NDR_LOAD: u32 = 10;
pub const NDR_SEND: u32 = 11;
pub const NDR_STUB: u32 = 12;
pub const NDR_INVOKE: u32 = 13;

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
        CLASS => super::compobj::class_step(cpu, t, value),
        MARSHAL => super::compobj::marshal_step(cpu, t, value),
        UNMARSHAL => super::compobj::unmarshal_step(cpu, t, value),
        PROXY => super::rpc::proxy_step(cpu, t, value),
        STUB => super::rpc::stub_step(cpu, t, value),
        ACTIVATE => super::rpc::activate_step(cpu, t, value),
        STORAGE_COPY => super::storage::copy_step(cpu, t, value),
        PERSIST => super::persist::step(cpu, t, value),
        NDR_LOAD => super::rpc::ndr::load_step(cpu, t, value),
        NDR_SEND => super::rpc::ndr::proxy_step(cpu, t, value),
        NDR_STUB => super::rpc::ndr::stub_step(cpu, t, value),
        NDR_INVOKE => super::rpc::ndr::call_step(cpu, t, value),
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
