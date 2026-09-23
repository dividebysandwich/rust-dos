//! BIOS handler of INT 06h, invalid opcode: log the instruction and skip
//! it, so a program that runs an instruction the emulated CPU doesn't have
//! keeps going. This is what the emulator did before it raised #UD.

use iced_x86::{Decoder, DecoderOptions};

use crate::cpu::Cpu;

pub fn handle(cpu: &mut Cpu) {
    // The exception frame: IP of the faulting instruction, CS, FLAGS.
    let frame = cpu.get_physical_addr(cpu.ss(), cpu.sp());
    let ip = cpu.bus.read_16(frame);
    let cs = cpu.bus.read_16(frame + 2);

    let start = cpu.get_physical_addr(cs, ip);
    let bytes: Vec<u8> = (0..15).map(|i| cpu.bus.read_8((start + i) & 0xFFFFF)).collect();
    let mut decoder = Decoder::with_ip(16, &bytes, ip as u64, DecoderOptions::NONE);
    let instr = decoder.decode();
    let len = instr.len().max(1);

    let hex: Vec<String> = bytes[..len].iter().map(|b| format!("{:02X}", b)).collect();
    cpu.bus.log_string(&format!(
        "[CPU] Invalid opcode at {:04X}:{:04X}: {} ({}), skipped",
        cs,
        ip,
        instr,
        hex.join(" ")
    ));
    cpu.bus.write_16(frame, ip.wrapping_add(len as u16));
}
