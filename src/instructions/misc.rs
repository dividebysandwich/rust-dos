use iced_x86::{Instruction, Mnemonic};
use crate::cpu::{Cpu, FLAG_CF};
use crate::interrupts;

pub fn handle(cpu: &mut Cpu, instr: &Instruction) {
    match instr.mnemonic() {
        Mnemonic::Int => {
            let int_num = instr.immediate8();
            interrupts::handle_interrupt(cpu, int_num);
        },
        Mnemonic::Stc => cpu.set_flag(FLAG_CF, true),
        Mnemonic::Clc => cpu.set_flag(FLAG_CF, false),
        Mnemonic::Std => cpu.set_dflag(true),
        Mnemonic::Cld => cpu.set_dflag(false),
        Mnemonic::Cmc => {
            let cf = cpu.get_flag(FLAG_CF);
            cpu.set_flag(FLAG_CF, !cf);
        }
        Mnemonic::Sti => { /* Enable Interrupts */ },
        Mnemonic::Cli => { /* Disable Interrupts */ },
        Mnemonic::Wait => { /* Wait for Interrupt */ },
        Mnemonic::Nop => { /* No Operation */ },
        Mnemonic::Hlt => { cpu.bus.log_string("CPU Halted"); },
        _ => {}
    }
}