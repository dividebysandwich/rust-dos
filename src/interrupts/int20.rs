use crate::cpu::Cpu;

/// INT 20h: Terminate Program, with exit code 0: back to the parent
/// process, or to the shell.
pub fn handle(cpu: &mut Cpu) {
    cpu.bus.log_string("[INT20] Program Terminated.");
    if cpu.terminate(0) {
        cpu.bus.log_string("[INT20] Returning to Parent Process");
    } else {
        cpu.bus.log_string("[INT20] No Parent. Rebooting Shell...");
    }
}
