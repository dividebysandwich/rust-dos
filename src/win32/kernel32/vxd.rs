//! VxDCall0 to VxDCall8: how Windows 98's KERNEL32 and the programs that
//! know its ordinals call a VxD's service from level 3. The first
//! argument is the service, the VxD's id in its high word and the
//! service's number in the low one; the others go to the service.
//!
//! On Windows 98 they trap through INT 30h into VMM, which calls the
//! service. The built-in Windows has no VxDs, so it has no services yet,
//! and a call of one ends the program, as Windows 98 does on a call of a
//! service no VxD provides ("invalid VxD dynamic link call").

use crate::cpu::Cpu;

/// VxDCallN(service, N arguments).
pub fn vxd_call(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let service = args[0];
    crate::win32::fatal(
        cpu,
        &format!(
            "an invalid VxD call: VxD {:04X}h's service {:04X}h, which the \
             built-in Windows doesn't have",
            service >> 16,
            service & 0xFFFF
        ),
        None,
    );
    None
}
