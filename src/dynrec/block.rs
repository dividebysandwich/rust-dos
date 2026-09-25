//! Blocks: which instructions from CS:EIP on make up a block, and what the
//! translated code and the execution loop need to know about them.
//!
//! A block holds exactly the instructions the interpreter would fetch
//! through its code window after the first one: all in one page, none
//! starting in the page's last 15 bytes or less than 15 bytes before the
//! CS limit (see `exec::CodeWindow`). It stops before an instruction the
//! interpreter must run itself (an invalid encoding, which may be an
//! emulator service trap, and HLT), and after one that may change what
//! the execution loop checks between instructions: whether an interrupt
//! can be delivered, the mode, CS, SS, the page tables or CR0, I/O ports
//! (and with them the timer deadline), or where execution goes on.

use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, InstructionInfoFactory, Mnemonic, OpAccess, Register};

use crate::bus::GEN_SHIFT;
use crate::exec::At;
use crate::instructions::{Handler, handler};

/// Bytes of a page from the last place an instruction may start in the
/// code window on (`exec::PAGE_TAIL`).
const PAGE_TAIL: u32 = 15;

/// A block of guest code. The translated code reads `gen_sum` and passes
/// the block to the helpers, so it lives at a fixed address (boxed) for
/// as long as the code can run.
#[repr(C)]
pub struct BlockData {
    /// Sum of the code generations of the 64-byte chunks the block's bytes
    /// are in, when the bytes were last found unchanged.
    pub gen_sum: u32,
    /// First and last chunk.
    pub chunk_first: u32,
    pub chunk_last: u32,
    /// Physical address and length of the block's bytes.
    pub phys: u32,
    pub len: u32,
    /// The instructions, their EIPs and handlers, and whether each may
    /// write memory (and with it the block's own bytes).
    pub instrs: Box<[Instruction]>,
    pub eips: Box<[u32]>,
    pub handlers: Box<[Handler]>,
    pub writes: Box<[bool]>,
    /// The block's bytes as they were translated.
    pub bytes: Box<[u8]>,
    /// The CS limit the block needs: the interpreter fetches every one of
    /// its instructions through the code window only if the limit is at
    /// least this.
    pub limit_need: u32,
    /// Where the block's linkable exits (to a known EIP in its page) jump:
    /// the translated code of the block there, or else the exit's stub in
    /// `stubs`, which returns to the execution loop to have it linked.
    pub links: [usize; 2],
    pub stubs: [usize; 2],
    /// The block's index in the translator's table.
    pub id: u32,
}

impl BlockData {
    /// Decode a block from the instruction at `at`, which is in the code
    /// window, with at most `max` instructions. None if no block can start
    /// there: the interpreter runs that instruction itself.
    pub fn build(at: &At, ram: &[u8], page_gen: &[u32], max: usize) -> Option<BlockData> {
        // Linear and physical addresses are the same within a page.
        let page_off = at.phys_ip as u32 & 0xFFF;
        let page_phys = at.phys_ip - page_off as usize;
        let page = &ram[page_phys..page_phys + 0x1000];
        let mut decoder = Decoder::new(if at.code32 { 32 } else { 16 }, page, DecoderOptions::NONE);
        let mut info = InstructionInfoFactory::new();
        let (mut instrs, mut eips, mut writes) = (Vec::new(), Vec::new(), Vec::new());
        let (mut off, mut eip) = (page_off, at.eip);
        while off <= 0x1000 - 16 && eip as u64 + PAGE_TAIL as u64 - 1 <= at.cs_limit as u64 {
            decoder.set_position(off as usize).unwrap();
            decoder.set_ip(eip as u64);
            let instr = decoder.decode();
            if instr.is_invalid() || instr.mnemonic() == Mnemonic::Hlt {
                break;
            }
            let ends = ends_block(&instr);
            writes.push(!ends && writes_memory(&mut info, &instr));
            instrs.push(instr);
            eips.push(eip);
            off += instr.len() as u32;
            eip = eip.wrapping_add(instr.len() as u32);
            if ends || instrs.len() >= max {
                break;
            }
        }
        if instrs.is_empty() {
            return None;
        }
        let phys = at.phys_ip as u32;
        let len = off - page_off;
        let (chunk_first, chunk_last) = (phys >> GEN_SHIFT, (phys + len - 1) >> GEN_SHIFT);
        let mut data = BlockData {
            gen_sum: 0,
            chunk_first,
            chunk_last,
            phys,
            len,
            handlers: instrs.iter().map(handler).collect(),
            // (The code window has checked this doesn't overflow.)
            limit_need: *eips.last().unwrap() + PAGE_TAIL - 1,
            links: [0; 2],
            stubs: [0; 2],
            id: 0,
            instrs: instrs.into_boxed_slice(),
            eips: eips.into_boxed_slice(),
            writes: writes.into_boxed_slice(),
            bytes: ram[phys as usize..(phys + len) as usize].into(),
        };
        data.gen_sum = data.gens_now(page_gen);
        Some(data)
    }

    /// Instructions in the block.
    pub fn count(&self) -> usize {
        self.instrs.len()
    }

    /// The sum of the chunks' generations now.
    pub fn gens_now(&self, page_gen: &[u32]) -> u32 {
        page_gen[self.chunk_first as usize..=self.chunk_last as usize]
            .iter()
            .fold(0u32, |sum, g| sum.wrapping_add(*g))
    }

    /// Where instruction `ix` starts in the block's bytes.
    pub fn offset(&self, ix: usize) -> usize {
        self.eips[ix].wrapping_sub(self.eips[0]) as usize
    }

    /// Physical address of instruction `ix`.
    pub fn phys_of(&self, ix: usize) -> usize {
        self.phys as usize + self.offset(ix)
    }

    /// Whether `eip` is in the block's page (linearly, which is also
    /// physically): translated code links an exit to a block there.
    pub fn in_page(&self, eip: u32) -> bool {
        let at = (self.phys & 0xFFF) as i64 + eip.wrapping_sub(self.eips[0]) as i32 as i64;
        (0..0x1000).contains(&at)
    }

    /// The physical address of `eip`, in the block's page.
    pub fn phys_in_page(&self, eip: u32) -> u32 {
        self.phys.wrapping_add(eip.wrapping_sub(self.eips[0]))
    }

    /// Whether the block's bytes from instruction `ix` on (all of them for
    /// 0) are still what was translated.
    pub fn unchanged_from(&self, ram: &[u8], ix: usize) -> bool {
        if ix >= self.count() {
            return true;
        }
        let from = self.offset(ix);
        let phys = self.phys as usize;
        ram[phys + from..phys + self.len as usize] == self.bytes[from..]
    }
}

/// Whether the block ends after `instr`: it transfers control, or it may
/// change something the execution loop checks between instructions.
pub fn ends_block(instr: &Instruction) -> bool {
    use Mnemonic::*;
    if instr.flow_control() != FlowControl::Next {
        return true;
    }
    match instr.mnemonic() {
        // Port I/O: the devices, their interrupts and the timer deadline.
        In | Out | Insb | Insw | Insd | Outsb | Outsw | Outsd => true,
        // IF and the interrupt shadow.
        Sti | Popf | Popfd | Iret | Iretd => true,
        // SS (the interrupt shadow and the stack's size).
        Lss => true,
        Mov | Pop if instr.op0_register() == Register::SS => true,
        // CR0 and the TLB, the descriptor tables and the task register.
        Mov if instr.op0_register().is_cr() || instr.op0_register().is_dr() || instr.op0_register().is_tr() => true,
        Lmsw | Clts | Invlpg | Lgdt | Lidt | Lldt | Ltr => true,
        _ => false,
    }
}

/// Whether `instr` may write memory.
fn writes_memory(info: &mut InstructionInfoFactory, instr: &Instruction) -> bool {
    info.info(instr).used_memory().iter().any(|m| {
        matches!(m.access(), OpAccess::Write | OpAccess::CondWrite | OpAccess::ReadWrite | OpAccess::ReadCondWrite)
    })
}
