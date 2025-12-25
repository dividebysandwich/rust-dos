use crate::cpu::Cpu;
use crate::video::print_string;

pub fn handle(cpu: &mut Cpu) {
    cpu.bus.log_string("[CPU] EXCEPTION: Divide by Zero (INT 0).");
    print_string(cpu, "Divide overflow\r\n");
    cpu.state = crate::cpu::CpuState::RebootShell;
}