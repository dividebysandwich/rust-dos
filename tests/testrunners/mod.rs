use rust_dos::cpu::Cpu;
use iced_x86::{Decoder, DecoderOptions, Mnemonic};

#[allow(dead_code)]
pub fn run_cpu_code(cpu: &mut Cpu, code: &[u8]) {
    let cs_base = (cpu.cs as u32) << 4;
    let start_ip = cpu.ip as u32;

    for (i, &byte) in code.iter().enumerate() {
        let phys_addr = (cs_base + start_ip + i as u32) & 0xFFFFF;
        cpu.bus.write_8(phys_addr as usize, byte);
    }

    let mut instructions_left = 100;

    loop {
        if instructions_left == 0 { break; }
        instructions_left -= 1;

        let current_offset = (cpu.ip as u32).wrapping_sub(start_ip) as usize;
        if current_offset >= code.len() { break; }

        let mut decoder = Decoder::new(16, &code[current_offset..], DecoderOptions::NONE);
        decoder.set_ip(cpu.ip as u64);
        
        let instr = decoder.decode();
        
        // Update IP to point to next instruction
        cpu.ip = instr.next_ip() as u16;

        // Execute the instruction
        rust_dos::instructions::execute_instruction(cpu, &instr);

        // Check for HLT *after* execution so the CPU state updates
        if instr.mnemonic() == Mnemonic::Hlt {
            break;
        }
    }
}

#[allow(dead_code)]
pub fn run_fpu_code(cpu: &mut Cpu, code: &[u8]) {
    // Write the code to the CPU's memory at CS:IP
    // This is required because fcom_variants read the raw opcode byte
    let cs_base = (cpu.cs as u32) << 4;
    let start_ip = cpu.ip as u32;
    
    for (i, &byte) in code.iter().enumerate() {
        let phys_addr = (cs_base + start_ip + i as u32) & 0xFFFFF;
        cpu.bus.write_8(phys_addr as usize, byte);
    }

    let mut decoder = Decoder::new(16, code, DecoderOptions::NONE);
    let instr = decoder.decode();

    cpu.ip = (start_ip + instr.len() as u32) as u16;

    rust_dos::instructions::fpu::handle(cpu, &instr);
}