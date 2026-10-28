//! Blocks: which instructions from CS:EIP on make up a block, and what the
//! translated code and the execution loop need to know about them.
//!
//! A block holds exactly the instructions the interpreter would fetch
//! through its code window after the first one: all in one page, none
//! starting in the page's last 15 bytes or less than 15 bytes before the
//! CS limit (see `exec::CodeWindow`). Where the code generator checks the
//! next page as the interpreter's fetch looks it up, it may go on into
//! the last 15 bytes, its last instruction running on into the next page
//! (`BlockData::in_tail`, `BlockData::crosses`). It stops before an
//! instruction the interpreter must run itself (an invalid encoding,
//! which may be an emulator service trap, and HLT), and after one that
//! may change what the execution loop checks between instructions:
//! whether an interrupt can be delivered, the mode, CS, SS, the page
//! tables or CR0, I/O ports (and with them the timer deadline), or where
//! execution goes on.

use iced_x86::{
    Decoder, DecoderError, DecoderOptions, FlowControl, Instruction, InstructionInfoFactory,
    Mnemonic, OpAccess,
};

use crate::bus::GEN_SHIFT;
use crate::exec::At;
use crate::instructions::{Handler, handler};

/// Bytes of a page from the last place an instruction may start in the
/// code window on (`exec::PAGE_TAIL`).
const PAGE_TAIL: u32 = 15;

/// Links a block's exits can have: 0 and 1 to a known EIP (a jump's
/// target, a conditional one's next instruction), from `SIDE_LINK` on
/// those of the conditional jumps the block goes on after (`SIDE_EXITS`)
/// to their targets, and from `RETURN_LINK` on those of an exit to an EIP
/// it only knows as it runs (a return's, an indirect call's), to the last
/// places it went to.
pub const LINKS: usize = RETURN_LINK + RETURN_LINKS;
/// The first link of a conditional jump in the block to a place outside
/// it, and how many a block has: it ends at the jump after them. (One to
/// an instruction later in the block goes there, see `BlockData::target`.)
pub const SIDE_LINK: usize = 2;
pub const SIDE_EXITS: usize = 6;
/// The first link of a return or indirect call, and how many it has: a
/// function called from two places in turn returns to each, and more
/// rarely from more.
pub const RETURN_LINK: usize = SIDE_LINK + SIDE_EXITS;
pub const RETURN_LINKS: usize = 4;
/// The index a return or indirect call leaves with that goes to none of
/// the places its links lead to.
pub const RETURN_MISS: usize = LINKS;

/// A byte is watched once blocks have gone stale this often because it
/// changed, each time with their changed bytes in at most `POKES` runs of
/// at most `POKE_BYTES`: pokes of immediates or addresses. New code over
/// the old changes longer runs.
pub const WATCH_AFTER: u8 = 2;
pub const POKE_BYTES: usize = 4;
pub const POKES: usize = 16;

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
    /// The target's mode (`Key::mode`), which a return after a far
    /// transfer must be in (`helpers::far_goes_on`).
    pub mode: u32,
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
        guard: Guard { eip: 0, cs_base: 0, a20: 0, paging: 0, page: 0, phys: 0, mode: u32::MAX },
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
    /// Physical address and length of the block's bytes in its page, and
    /// the EIP of its first instruction.
    pub phys: u32,
    pub len: u32,
    pub eip: u32,
    /// The block's index in the translator's table. (These first fields
    /// share a cache line, which the execution loop reads after a block.)
    pub id: u32,
    /// Where the block's last instruction runs on into the next page
    /// (`crosses`): the physical address of that page, how many of its
    /// first bytes the instruction has (after the `len` in its own page,
    /// in `bytes`), and the code generation of their chunk when they were
    /// last found unchanged. `next_len` is 0 for the other blocks.
    pub next: u32,
    pub next_len: u32,
    pub next_gen: u32,
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
    /// another page, what they were made under. The jumps are direct ones
    /// in the code (`sites`: the link, and the jump's offset in the
    /// code), which the engine points where `links` says.
    pub links: [usize; LINKS],
    pub stubs: [usize; LINKS],
    pub guards: [Guard; LINKS],
    pub sites: Box<[(u8, u32)]>,
    /// Per instruction, how many instructions the instruction count is
    /// behind while it runs: the translated code brings it up to date
    /// only before handlers and where the block ends.
    pub lag: Box<[u8]>,
}

impl BlockData {
    /// Decode a block from the instruction at `at`, which is in the code
    /// window, with at most `max` instructions, and `watch` the changes of
    /// its page's bytes (see `WATCH_AFTER`). With `tail`, the block may go
    /// on into the page's last 15 bytes (see `in_tail`), and with `next`,
    /// the physical address of the next page, all in `ram`, its last
    /// instruction may run on into that page (see `crosses`). The block
    /// goes on after the conditional jumps that `side` says its code takes,
    /// up to `SIDE_EXITS` of them that leave it. None if no block can start
    /// there: the interpreter runs that instruction itself.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        at: &At,
        ram: &[u8],
        page_gen: &[u32],
        max: usize,
        watch: Option<&[u8]>,
        tail: bool,
        next: Option<u32>,
        side: impl Fn(&Instruction) -> bool,
    ) -> Option<BlockData> {
        // Jumps forward in the page may stay in the block; where too many
        // leave it after all, it ends at the first that is one too many
        // (which may make more of those before it leave). A loop's start
        // starts a block of its own: the block stops before it.
        let mut max = max;
        loop {
            let data = Self::build_up_to(at, ram, page_gen, max, watch, tail, next, &side)?;
            let n = data.count();
            let back = (0..n).filter_map(|ix| {
                let t = jump_target(&data.instrs[ix])?;
                (1..=ix).find(|&j| data.eips[j] == t)
            });
            if let Some(head) = back.min() {
                max = head;
                continue;
            }
            let outside: Vec<usize> =
                (0..n - 1).filter(|&ix| side_exit(&data.instrs[ix]) && data.target(ix).is_none()).collect();
            if outside.len() <= SIDE_EXITS {
                return Some(data);
            }
            max = outside[SIDE_EXITS] + 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_up_to(
        at: &At,
        ram: &[u8],
        page_gen: &[u32],
        max: usize,
        watch: Option<&[u8]>,
        tail: bool,
        next: Option<u32>,
        side: &impl Fn(&Instruction) -> bool,
    ) -> Option<BlockData> {
        let next = next.filter(|_| tail);
        // Linear and physical addresses are the same within a page.
        let page_off = at.phys_ip as u32 & 0xFFF;
        let page_phys = at.phys_ip - page_off as usize;
        let page = &ram[page_phys..page_phys + 0x1000];
        let bitness = if at.code32 { 32 } else { 16 };
        let mut decoder = Decoder::new(bitness, page, DecoderOptions::NONE);
        let mut info = InstructionInfoFactory::new();
        let (mut instrs, mut eips, mut writes, mut watched) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut off, mut eip, mut sides) = (page_off, at.eip, 0);
        let end = if tail { 0x1000 } else { 0x1000 - PAGE_TAIL };
        while off < end && eip as u64 + PAGE_TAIL as u64 - 1 <= at.cs_limit as u64 {
            decoder.set_position(off as usize).unwrap();
            decoder.set_ip(eip as u64);
            let mut instr = decoder.decode();
            let here = 0x1000 - off as usize;
            if let Some(next) = next
                && here < PAGE_TAIL as usize
                && decoder.last_error() == DecoderError::NoMoreBytes
            {
                // It runs on into the next page: its bytes from both.
                let mut buf = [0u8; 2 * PAGE_TAIL as usize];
                buf[..here].copy_from_slice(&page[off as usize..]);
                buf[here..][..PAGE_TAIL as usize]
                    .copy_from_slice(&ram[next as usize..][..PAGE_TAIL as usize]);
                let buf = &buf[..here + PAGE_TAIL as usize];
                instr = Decoder::with_ip(bitness, buf, eip as u64, DecoderOptions::NONE).decode();
            }
            if instr.is_invalid() || instr.mnemonic() == Mnemonic::Hlt {
                break;
            }
            let ends = ends_block(&instr);
            let bytes = off as usize..(off as usize + instr.len()).min(0x1000);
            if let Some(watch) = watch
                && watch[bytes.clone()].iter().any(|&n| n >= WATCH_AFTER)
            {
                // A RET poked over an instruction ends the blocks translated
                // while it is there: the block stops before it instead, and
                // the interpreter runs whichever is there. So does one that
                // runs on into the next page, which has no watched bytes.
                if (ends && watch[off as usize] >= WATCH_AFTER) || instr.len() > here {
                    break;
                }
                watched.extend(bytes.filter(|&i| watch[i] >= WATCH_AFTER).map(|i| (i - page_off as usize) as u16));
            }
            writes.push(!ends && writes_memory(&mut info, &instr));
            instrs.push(instr);
            eips.push(eip);
            off += instr.len() as u32;
            eip = eip.wrapping_add(instr.len() as u32);
            // (A loop's end ends it.)
            let forward = jump_target(&instr).is_some_and(|t| t > eip);
            let goes_on = ends && forward && sides < SIDE_EXITS && side_exit(&instr) && side(&instr);
            if goes_on && !jump_target(&instr).is_some_and(|t| t - eip < 0x1000) {
                sides += 1;
            }
            if (ends && !goes_on) || instrs.len() >= max {
                break;
            }
        }
        if instrs.is_empty() {
            return None;
        }
        let phys = at.phys_ip as u32;
        let len = off.min(0x1000) - page_off;
        let (next, next_len) = (next.unwrap_or(0), off.saturating_sub(0x1000));
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
            sites: Box::new([]),
            id: 0,
            next,
            next_len,
            next_gen: 0,
            lag: Box::new([]),
            instrs: instrs.into_boxed_slice(),
            eips: eips.into_boxed_slice(),
            writes: writes.into_boxed_slice(),
            bytes: [
                &ram[phys as usize..][..len as usize],
                &ram[next as usize..][..next_len as usize],
            ]
            .concat()
            .into(),
            watched: watched.into_boxed_slice(),
            live_imms: Box::new([]),
        };
        data.note_gens(page_gen);
        Some(data)
    }

    /// Whether instruction `ix` starts in the page's last 15 bytes, where the
    /// interpreter's fetch looks at the next page too (with paging, its
    /// translation), in case the instruction runs on into it.
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
    pub fn in_tail(&self, ix: usize) -> bool {
        self.phys_of(ix) & 0xFFF >= 0x1000 - PAGE_TAIL as usize
    }

    /// Whether instruction `ix` runs on into the next page: the block's
    /// last, whose bytes there are at `next`. With paging, its code checks
    /// that the TLB maps the next page there as it did.
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
    pub fn crosses(&self, ix: usize) -> bool {
        self.next_len != 0 && ix + 1 == self.count()
    }

    /// Where conditional jump `ix` goes in the block, if it isn't its last
    /// instruction and its target is the start of a later one: the code
    /// jumps there, without the instructions in between.
    pub fn target(&self, ix: usize) -> Option<usize> {
        if ix + 1 >= self.count() || !side_exit(&self.instrs[ix]) {
            return None;
        }
        let t = jump_target(&self.instrs[ix])?;
        (ix + 1..self.count()).find(|&j| self.eips[j] == t)
    }

    /// Per instruction, whether a conditional jump in the block goes there
    /// (`target`).
    pub fn targets(&self) -> Vec<bool> {
        let mut to = vec![false; self.count()];
        for ix in 0..self.count() {
            if let Some(j) = self.target(ix) {
                to[j] = true;
            }
        }
        to
    }

    /// Instructions in the block.
    pub fn count(&self) -> usize {
        self.instrs.len()
    }

    /// The sum of the chunks' generations now, with that of the chunk of
    /// the bytes in the next page.
    pub fn gens_now(&self, page_gen: &[u32]) -> u32 {
        page_gen[self.chunk_first as usize..=self.chunk_last as usize]
            .iter()
            .fold(self.next_gen_now(page_gen), |sum, g| sum.wrapping_add(*g))
    }

    /// The generation of the chunk of the bytes in the next page now (0
    /// where there are none).
    fn next_gen_now(&self, page_gen: &[u32]) -> u32 {
        if self.next_len == 0 { 0 } else { page_gen[(self.next >> GEN_SHIFT) as usize] }
    }

    /// Note the generations now, the bytes being as translated.
    pub fn note_gens(&mut self, page_gen: &[u32]) {
        self.gen_sum = self.gens_now(page_gen);
        self.next_gen = self.next_gen_now(page_gen);
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
        let len = self.len as usize;
        if ram[self.next as usize..][..self.next_len as usize] != self.bytes[len..] {
            return false;
        }
        let now = &ram[self.phys as usize..][..len];
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
        now[from..] == self.bytes[from..len]
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

    /// Offsets in `bytes` of the bytes in the block's page that changed
    /// since the block was translated, if they are pokes: in at most
    /// `POKES` runs of at most `POKE_BYTES` (SkyNET pokes two or three
    /// immediates into a block before running it, Riverworld the twelve
    /// factors of a transform). A run may skip bytes that stayed as they
    /// were, as the middle byte of a new immediate can, but never has more
    /// than `POKE_BYTES` changed in a row. A change to the bytes of the
    /// last instruction in the next page (`crosses`) counts as one to its
    /// first byte, which has a count of pokes: once that is watched,
    /// blocks stop before it.
    pub fn poked(&self, ram: &[u8]) -> Option<Vec<usize>> {
        let len = self.len as usize;
        let now = &ram[self.phys as usize..][..len];
        let mut changed = Vec::new();
        // The runs so far, where the last one starts, and the bytes
        // changed in a row up to here.
        let (mut runs, mut start, mut row) = (0, 0, 0);
        for (i, (a, b)) in now.iter().zip(&self.bytes[..len]).enumerate() {
            if a == b {
                row = 0;
                continue;
            }
            if runs == 0 || i >= start + POKE_BYTES {
                runs += 1;
                start = i;
            }
            row += 1;
            if row > POKE_BYTES || runs > POKES {
                return None;
            }
            changed.push(i);
        }
        if ram[self.next as usize..][..self.next_len as usize] != self.bytes[len..] {
            let first = self.offset(self.count() - 1);
            if !changed.contains(&first) {
                if runs == POKES {
                    return None;
                }
                changed.push(first);
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
        // CR0 and the TLB, and the task register. (The descriptor tables
        // matter only to what reads them: segment loads, which do so as
        // they run, and interrupts and faults, which the execution loop
        // delivers after the block.)
        Mov if instr.op0_register().is_cr() || instr.op0_register().is_dr() || instr.op0_register().is_tr() => true,
        Lmsw | Clts | Invlpg | Ltr => true,
        // (SYSENTER and SYSEXIT, which change CS, SS and the privilege
        // level, stop it as a call and a return.)
        _ => false,
    }
}

/// Whether the block can go on after `instr`, which ends it, if its code
/// leaves only where the jump is taken (`SIDE_EXITS`): a conditional jump.
/// (The engine checks that it translates it.)
pub fn side_exit(instr: &Instruction) -> bool {
    instr.flow_control() == FlowControl::ConditionalBranch
}

/// The target of a near jump, as its code has it.
fn jump_target(instr: &Instruction) -> Option<u32> {
    match instr.op0_kind() {
        iced_x86::OpKind::NearBranch16 => Some(instr.near_branch16() as u32),
        iced_x86::OpKind::NearBranch32 => Some(instr.near_branch32()),
        _ => None,
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

/// Whether `instr` is a far transfer that its handler runs: a far CALL or
/// JMP, RETF, INT or IRET. In real mode and virtual-8086 mode the block
/// goes on through the links a return takes after it (see
/// `helpers::jit_fallback`).
pub fn far_transfer(instr: &Instruction) -> bool {
    use Mnemonic::*;
    match instr.mnemonic() {
        Retf | Int | Int3 | Into | Iret | Iretd => true,
        Call => instr.is_call_far() || instr.is_call_far_indirect(),
        Jmp => instr.is_jmp_far() || instr.is_jmp_far_indirect(),
        _ => false,
    }
}

/// Whether `instr` may leave an interrupt shadow (STI, a load of SS): the
/// instruction after it ends that.
pub fn shadows(instr: &Instruction) -> bool {
    instr.mnemonic() == Mnemonic::Sti || loaded_segment(instr) == Some(crate::cpu::Seg::SS)
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
