use crate::cpu::{Cpu, CpuState};

pub fn handle(cpu: &mut Cpu) {
    // INT 20h: Terminate Program
    // DOS standard behavior: This restores the parent process (the shell).
    // This simply signals the main loop to reload the shell.
    
    cpu.bus.log_string("[INT20] Program Terminated.");
    cpu.state = CpuState::RebootShell;
}