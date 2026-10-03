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

use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, InstructionInfoFactory, Mnemonic, OpAccess};

use crate::bus::GEN_SHIFT;
use crate::exec::At;
use crate::instructions::{Handler, handler};

/// Bytes of a page from the last place an instruction may start in the
/// code window on (`exec::PAGE_TAIL`).
const PAGE_TAIL: u32 = 15;

/// Links a block's exits can have: 0 and 1 to a known EIP (a jump's
/// target, a conditional one's next instruction), and from `RETURN_LINK`
/// on those of an exit to an EIP it only knows as it runs (a return's, an
/// indirect call's), to the last places it went to.
pub const LINKS: usize = RETURN_LINK + RETURN_LINKS;
/// The first link of a return or indirect call, and how many it has: a
/// function called from two places in turn returns to each, and more
/// rarely from more.
pub const RETURN_LINK: usize = 2;
pub const RETURN_LINKS: usize = 4;
/// The index a return or indirect call leaves with that goes to none of
/// the places its links lead to.
pub const RETURN_MISS: usize = LINKS;

/// A byte is watched once blocks have gone stale this often because it
/// changed, each time with at most `POKE_BYTES` of their bytes changed
/// (a poke, not new code over the old).
pub const WATCH_AFTER: u8 = 2;
pub const POKE_BYTES: usize = 4;

/// What a link to another page was made under. Translated code takes it
/// only while fetching its target would go the same way, as the
/// interpreter's instruction fetch would without the link: the same CS
/// base, A20 gate and paging, and with paging the same translation in the
/// TLB. Links within the block's page need none of this (see `block`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Guard {
    /// The target's EIP, which a return's link must see again.
    pub eip: u32,
    pub cs_base: u32,
    pub a20: u32,
    /// 1 with paging on; then the target's linear page number and the
    /// TLB's physical page for it.
    pub paging: u32,
    pub page: u32,
    pub phys: u32,
}

/// log2 of the places returns and indirect calls went to that the
/// engine keeps for all blocks (`Return`).
pub const RETURN_BITS: u32 = 10;

/// A place a return or indirect call went to, in a table for all blocks
/// indexed by its EIP's low bits: the x86-64 code of one whose own links
/// lead elsewhere (a function called from many places in turn) goes there
/// without the execution loop where its guard holds as a link's would,
/// in the same mode (`Key::mode`, which no block has where `mode` is
/// `u32::MAX`).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Return {
    pub guard: Guard,
    pub mode: u32,
    pub code: usize,
}

impl Return {
    pub const NONE: Return = Return {
        guard: Guard { eip: 0, cs_base: 0, a20: 0, paging: 0, page: 0, phys: 0 },
        mode: u32::MAX,
        code: 0,
    };
}

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
    /// Physical address and length of the block's bytes, and the EIP of
    /// its first instruction.
    pub phys: u32,
    pub len: u32,
    pub eip: u32,
    /// The block's index in the translator's table. (These first fields
    /// share a cache line, which the execution loop reads after a block.)
    pub id: u32,
    /// The instructions, their EIPs and handlers, and whether each may
    /// write memory (and with it the block's own bytes).
    pub instrs: Box<[Instruction]>,
    pub eips: Box<[u32]>,
    pub handlers: Box<[Handler]>,
    pub writes: Box<[bool]>,
    /// The block's bytes as they were translated.
    pub bytes: Box<[u8]>,
    /// Offsets in `bytes` of the watched ones, in order: the instructions
    /// they are in check them before they run, and the block stays valid
    /// while only they change.
    pub watched: Box<[u16]>,
    /// Per instruction (empty if none), whether its watched bytes are all
    /// in its immediate, which its translated code reads from RAM rather
    /// than checks (see `translate::live_immediate`).
    pub live_imms: Box<[bool]>,
    /// The CS limit the block needs: the interpreter fetches every one of
    /// its instructions through the code window only if the limit is at
    /// least this.
    pub limit_need: u32,
    /// Where the block's linkable exits jump: the translated code of the
    /// block their EIP leads to, or else the exit's stub in `stubs`, which
    /// returns to the execution loop to have it linked; and for links to
    /// another page, what they were made under.
    pub links: [usize; LINKS],
    pub stubs: [usize; LINKS],
    pub guards: [Guard; LINKS],
    /// Per instruction, how many instructions the instruction count is
    /// behind while it runs: the translated code brings it up to date
    /// only before handlers and where the block ends.
    pub lag: Box<[u8]>,
}

impl BlockData {
    /// Decode a block from the instruction at `at`, which is in the code
    /// window, with at most `max` instructions, and `watch` the changes of
    /// its page's bytes (see `WATCH_AFTER`). With `tail`, the block may go
    /// on into the page's last 15 bytes, with instructions that end in the
    /// page (see `in_tail`). None if no block can start there: the
    /// interpreter runs that instruction itself.
    pub fn build(at: &At, ram: &[u8], page_gen: &[u32], max: usize, watch: Option<&[u8]>, tail: bool) -> Option<BlockData> {
        // Linear and physical addresses are the same within a page.
        let page_off = at.phys_ip as u32 & 0xFFF;
        let page_phys = at.phys_ip - page_off as usize;
        let page = &ram[page_phys..page_phys + 0x1000];
        let mut decoder = Decoder::new(if at.code32 { 32 } else { 16 }, page, DecoderOptions::NONE);
        let mut info = InstructionInfoFactory::new();
        let (mut instrs, mut eips, mut writes, mut watched) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut off, mut eip) = (page_off, at.eip);
        let end = if tail { 0x1000 } else { 0x1000 - PAGE_TAIL };
        while off < end && eip as u64 + PAGE_TAIL as u64 - 1 <= at.cs_limit as u64 {
            decoder.set_position(off as usize).unwrap();
            decoder.set_ip(eip as u64);
            let instr = decoder.decode();
            if instr.is_invalid() || instr.mnemonic() == Mnemonic::Hlt {
                break;
            }
            let ends = ends_block(&instr);
            let bytes = off as usize..off as usize + instr.len();
            if let Some(watch) = watch
                && watch[bytes.clone()].iter().any(|&n| n >= WATCH_AFTER)
            {
                // A RET poked over an instruction ends the blocks translated
                // while it is there: the block stops before it instead, and
                // the interpreter runs whichever is there.
                if ends && watch[off as usize] >= WATCH_AFTER {
                    break;
                }
                watched.extend(bytes.filter(|&i| watch[i] >= WATCH_AFTER).map(|i| (i - page_off as usize) as u16));
            }
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
            eip: at.eip,
            handlers: instrs.iter().map(handler).collect(),
            // (The code window has checked this doesn't overflow.)
            limit_need: *eips.last().unwrap() + PAGE_TAIL - 1,
            links: [0; LINKS],
            stubs: [0; LINKS],
            guards: [Guard::default(); LINKS],
            id: 0,
            lag: Box::new([]),
            instrs: instrs.into_boxed_slice(),
            eips: eips.into_boxed_slice(),
            writes: writes.into_boxed_slice(),
            bytes: ram[phys as usize..(phys + len) as usize].into(),
            watched: watched.into_boxed_slice(),
            live_imms: Box::new([]),
        };
        data.gen_sum = data.gens_now(page_gen);
        Some(data)
    }

    /// Whether instruction `ix` starts in the page's last 15 bytes, where the
    /// interpreter's fetch looks at the next page too (with paging, its
    /// translation), in case the instruction runs on into it. The decoder
    /// found the instruction ends in the page.
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
    pub fn in_tail(&self, ix: usize) -> bool {
        self.phys_of(ix) & 0xFFF >= 0x1000 - PAGE_TAIL as usize
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
        self.eips[ix].wrapping_sub(self.eip) as usize
    }

    /// Physical address of instruction `ix`.
    pub fn phys_of(&self, ix: usize) -> usize {
        self.phys as usize + self.offset(ix)
    }

    /// Whether `eip` is in the block's page (linearly, which is also
    /// physically): translated code links an exit to a block there.
    pub fn in_page(&self, eip: u32) -> bool {
        let at = (self.phys & 0xFFF) as i64 + eip.wrapping_sub(self.eip) as i32 as i64;
        (0..0x1000).contains(&at)
    }

    /// The physical address of `eip`, in the block's page.
    pub fn phys_in_page(&self, eip: u32) -> u32 {
        self.phys.wrapping_add(eip.wrapping_sub(self.eip))
    }

    /// Whether the block's bytes from instruction `ix` on (all of them for
    /// 0) are still what was translated, but for the watched ones, which
    /// their instructions check.
    pub fn unchanged_from(&self, ram: &[u8], ix: usize) -> bool {
        if ix >= self.count() {
            return true;
        }
        let now = &ram[self.phys as usize..][..self.len as usize];
        let mut from = self.offset(ix);
        for &w in &self.watched {
            let w = w as usize;
            if w >= from {
                if now[from..w] != self.bytes[from..w] {
                    return false;
                }
                from = w + 1;
            }
        }
        now[from..] == self.bytes[from..]
    }

    /// Whether instruction `ix` reads its watched immediate from RAM.
    pub fn live_imm(&self, ix: usize) -> bool {
        self.live_imms.get(ix).copied().unwrap_or(false)
    }

    /// Offsets in `bytes` of the watched bytes of instruction `ix`.
    pub fn watched_in(&self, ix: usize) -> impl Iterator<Item = usize> + '_ {
        let range = self.offset(ix)..self.offset(ix) + self.instrs[ix].len();
        self.watched.iter().map(|&w| w as usize).filter(move |w| range.contains(w))
    }

    /// Offsets in `bytes` of the bytes that changed since the block was
    /// translated, if they are few enough to be a poke (`POKE_BYTES`).
    pub fn poked(&self, ram: &[u8]) -> Option<Vec<usize>> {
        let now = &ram[self.phys as usize..][..self.len as usize];
        let mut changed = Vec::new();
        for (i, (a, b)) in now.iter().zip(&self.bytes[..]).enumerate() {
            if a != b {
                if changed.len() == POKE_BYTES {
                    return None;
                }
                changed.push(i);
            }
        }
        (!changed.is_empty()).then_some(changed)
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
        // String port I/O: the devices, their interrupts and the timer
        // deadline. IN, OUT and STI stop the block after them only where
        // they change what the execution loop checks (see
        // `helpers::jit_fallback`).
        Insb | Insw | Insd | Outsb | Outsw | Outsd => true,
        // (POPF, and MOV, POP and LSS into SS, stop the block after them
        // only where they set IF with an interrupt waiting, set TF, or
        // change the stack's width, see `helpers::jit_fallback`.)
        // CR0 and the TLB, the descriptor tables and the task register.
        Mov if instr.op0_register().is_cr() || instr.op0_register().is_dr() || instr.op0_register().is_tr() => true,
        Lmsw | Clts | Invlpg | Lgdt | Lidt | Lldt | Ltr => true,
        _ => false,
    }
}

/// The segment register `instr` loads without ending the block, if any:
/// MOV, POP, LDS, LES, LFS, LGS and LSS. (Far transfers end the block.)
pub fn loaded_segment(instr: &Instruction) -> Option<crate::cpu::Seg> {
    use crate::cpu::Seg;
    match instr.mnemonic() {
        Mnemonic::Mov | Mnemonic::Pop if instr.op0_kind() == iced_x86::OpKind::Register => {
            Seg::from_register(instr.op0_register())
        }
        Mnemonic::Lss => Some(Seg::SS),
        Mnemonic::Lds => Some(Seg::DS),
        Mnemonic::Les => Some(Seg::ES),
        Mnemonic::Lfs => Some(Seg::FS),
        Mnemonic::Lgs => Some(Seg::GS),
        _ => None,
    }
}

/// Whether `instr` is IN or OUT, which may change the devices, their
/// interrupts, the timer deadline, the A20 gate and the reset line.
pub fn port_io(instr: &Instruction) -> bool {
    matches!(instr.mnemonic(), Mnemonic::In | Mnemonic::Out)
}

/// Whether `instr` is a REP string instruction, whose time grows with its
/// count (`instructions::string`).
pub fn repeated_string(instr: &Instruction) -> bool {
    instr.is_string_instruction() && (instr.has_rep_prefix() || instr.has_repne_prefix())
}

/// Whether `instr` may write memory.
fn writes_memory(info: &mut InstructionInfoFactory, instr: &Instruction) -> bool {
    info.info(instr).used_memory().iter().any(|m| {
        matches!(m.access(), OpAccess::Write | OpAccess::CondWrite | OpAccess::ReadWrite | OpAccess::ReadCondWrite)
    })
}
