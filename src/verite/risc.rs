//! The V1000's RISC as the host sees it through the debug registers: held,
//! single-stepped through instructions forced into its instruction
//! register, its program counter and register file read back. Drivers
//! load microcode and start it this way (xf86-video-rendition's
//! `v1krisc.c`), and Rendition's Windows 95 driver checks that the RISC
//! reached the program it started before it uses the card.
//!
//! The microcode itself isn't run: only the forced instructions are
//! carried out, which are the loads, stores, immediates and jumps those
//! drivers build.

/// STATEINDEX's values: what STATEDATA shows.
pub const INDEX_IR: u8 = 0x80;
pub const INDEX_PC: u8 = 0x81;
pub const INDEX_S1: u8 = 0x82;

/// The opcodes of the forced instructions, the top byte.
mod op {
    pub const ADDI: u8 = 0x00;
    pub const ADD: u8 = 0x10;
    pub const ANDN: u8 = 0x12;
    pub const OR: u8 = 0x15;
    pub const ADDIFI: u8 = 0x40;
    pub const ADDSL8: u8 = 0x4B;
    pub const SPRI: u8 = 0x4F;
    pub const JMP: u8 = 0x6C;
    pub const LB: u8 = 0x70;
    pub const LH: u8 = 0x71;
    pub const LW: u8 = 0x72;
    pub const LI: u8 = 0x76;
    pub const LUI: u8 = 0x77;
    pub const SB: u8 = 0x78;
    pub const SH: u8 = 0x79;
    pub const SW: u8 = 0x7A;
}

#[derive(Clone, Debug)]
pub struct Risc {
    /// The instruction register (DEC_IR).
    pub ir: u32,
    pub pc: u32,
    /// A jump's target, taken after the instruction in its delay slot.
    pub target: Option<u32>,
    /// Whether DEBUGREG holds the RISC.
    pub held: bool,
    /// The register file; register 0 reads as zero.
    pub rf: Vec<u32>,
}

impl Default for Risc {
    fn default() -> Self {
        Self { ir: 0, pc: 0, target: None, held: false, rf: vec![0; 256] }
    }
}

crate::state_fields!(Risc { ir, pc, target, held, rf });

impl Risc {
    /// What STATEDATA reads with STATEINDEX at `index`: the instruction
    /// register, the program counter, or the register the instruction's
    /// first source names.
    pub fn state(&self, index: u8) -> Option<u32> {
        match index {
            INDEX_IR => Some(self.ir),
            INDEX_PC => Some(self.pc),
            INDEX_S1 => Some(self.reg(self.ir as u8)),
            _ => None,
        }
    }

    fn reg(&self, r: u8) -> u32 {
        if r == 0 { 0 } else { self.rf[r as usize] }
    }

    fn set(&mut self, r: u8, value: u32) {
        if r != 0 {
            self.rf[r as usize] = value;
        }
    }

    /// The instruction register carried out once, on the card's memory
    /// `vram`, which the RISC sees big-endian. Whether it was one this
    /// knows.
    pub fn step(&mut self, vram: &mut [u8]) -> bool {
        let i = self.ir;
        let (opcode, d, s2, s1) = ((i >> 24) as u8, (i >> 16) as u8, (i >> 8) as u8, i as u8);
        self.pc = self.target.take().unwrap_or(self.pc.wrapping_add(4));
        let at = |base: u32, offset: u8| base.wrapping_add(offset as i8 as u32) as usize % vram.len();
        match opcode {
            op::ADDI => self.set(d, self.reg(s2).wrapping_add(s1 as u32)),
            op::ADD => self.set(d, self.reg(s2).wrapping_add(self.reg(s1))),
            op::ANDN => self.set(d, self.reg(s2) & !self.reg(s1)),
            op::OR => self.set(d, self.reg(s2) | self.reg(s1)),
            op::ADDIFI => self.set(d, self.reg(s2).wrapping_add((s1 as u32) << 16)),
            op::ADDSL8 => self.set(d, self.reg(s2).wrapping_add((s1 as u32) << 8)),
            op::LI => self.set(d, i & 0xFFFF),
            op::LUI => self.set(d, i << 16),
            op::JMP => self.target = Some((i & 0xFF_FFFF) << 2),
            op::SPRI => {}
            op::LB | op::LH | op::LW => {
                let a = at(self.reg(s1), s2);
                let value = match opcode {
                    op::LB => vram[a] as u32,
                    op::LH => u16::from_be_bytes([vram[a], vram[(a + 1) % vram.len()]]) as u32,
                    _ => u32::from_be_bytes(std::array::from_fn(|k| vram[(a + k) % vram.len()])),
                };
                self.set(d, value);
            }
            op::SB | op::SH | op::SW => {
                let a = at(self.reg(s1), d);
                let bytes = self.reg(s2).to_be_bytes();
                let n = match opcode {
                    op::SB => 1,
                    op::SH => 2,
                    _ => 4,
                };
                for (k, &byte) in bytes[4 - n..].iter().enumerate() {
                    vram[(a + k) % vram.len()] = byte;
                }
            }
            _ => return false,
        }
        true
    }

    /// As after a soft reset: at the start of memory.
    pub fn reset(&mut self) {
        self.pc = 0;
        self.target = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn force(risc: &mut Risc, vram: &mut [u8], instr: u32) {
        risc.ir = instr;
        assert!(risc.step(vram));
    }

    // The sequences v1krisc.c's writeRF, risc_writemem and risc_readmem
    // build.
    #[test]
    fn a_word_written_and_read_back_through_forced_instructions() {
        let (mut risc, mut vram) = (Risc::default(), vec![0u8; 0x1000]);
        // RA (254) = 0x800: li; FP (255) = 0x12345678: lui, addsl8, addi.
        force(&mut risc, &mut vram, 0x76FE_0800);
        force(&mut risc, &mut vram, 0x77FF_1234);
        force(&mut risc, &mut vram, 0x4BFF_FF56);
        force(&mut risc, &mut vram, 0x00FF_FF78);
        assert_eq!(risc.rf[255], 0x1234_5678);
        force(&mut risc, &mut vram, 0x7A00_FFFE);
        assert_eq!(&vram[0x800..0x804], &[0x12, 0x34, 0x56, 0x78]);
        // sp (252) = [RA]; then read through S1 with "add zero, zero, sp".
        force(&mut risc, &mut vram, 0x72FC_00FE);
        risc.ir = 0x1000_00FC;
        assert_eq!(risc.state(INDEX_S1), Some(0x1234_5678));
    }

    // v1k_start: the jump, then a nop in its delay slot.
    #[test]
    fn a_forced_jump_moves_the_program_counter_after_its_delay_slot() {
        let (mut risc, mut vram) = (Risc::default(), vec![0u8; 16]);
        force(&mut risc, &mut vram, 0x6C00_0000 | 0x800 >> 2);
        assert_ne!(risc.state(INDEX_PC), Some(0x800));
        force(&mut risc, &mut vram, 0);
        assert_eq!(risc.state(INDEX_PC), Some(0x800));
    }
}
