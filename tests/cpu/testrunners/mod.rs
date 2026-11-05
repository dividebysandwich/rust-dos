use rust_dos::cpu::Cpu;
use iced_x86::{Decoder, DecoderOptions};

/// Write `code` at CS:IP and step the CPU through it, through the same
/// execution loop the emulator runs, until IP leaves the code, after a
/// HLT, or after 100 steps.
#[allow(dead_code)]
pub fn run_cpu_code(cpu: &mut Cpu, code: &[u8]) {
    let cs_base = (cpu.cs() as u32) << 4;
    let start_ip = cpu.ip() as u32;

    for (i, &byte) in code.iter().enumerate() {
        let phys_addr = (cs_base + start_ip + i as u32) & 0xFFFFF;
        cpu.bus.write_8(phys_addr as usize, byte);
    }

    for _ in 0..100 {
        let current_offset = (cpu.ip() as u32).wrapping_sub(start_ip) as usize;
        if current_offset >= code.len() {
            break;
        }
        let halt = code[current_offset] == 0xF4;
        cpu.step();
        if halt {
            break;
        }
    }
}

#[allow(dead_code)]
pub fn run_fpu_code(cpu: &mut Cpu, code: &[u8]) {
    // Write the code to the CPU's memory at CS:IP
    // This is required because fcom_variants read the raw opcode byte
    let cs_base = (cpu.cs() as u32) << 4;
    let start_ip = cpu.ip() as u32;
    
    for (i, &byte) in code.iter().enumerate() {
        let phys_addr = (cs_base + start_ip + i as u32) & 0xFFFFF;
        cpu.bus.write_8(phys_addr as usize, byte);
    }

    let mut decoder = Decoder::new(16, code, DecoderOptions::NONE);
    let instr = decoder.decode();

    cpu.set_ip((start_ip + instr.len() as u32) as u16);

    rust_dos::instructions::fpu::handle(cpu, &instr);
}