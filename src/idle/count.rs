//! Passes that differ in counts (`Phase::Count`).
//!
//! A loop may come back to its head with everything as it was but for a
//! few counts: bytes of RAM or a general register that every pass moves on
//! by the same steps. MFC's idle loop counts its passes in EDI and hands
//! the count to OnIdle on the stack, which compares it with 0 and 1 and
//! does the same as long as it is above them (#1308); MS-DOS 7 counts
//! down a word in RAM between its keyboard polls. The passes then run the
//! same instructions until a count comes to a value that some instruction
//! tells apart from the one before.
//!
//! One pass is recorded from a visit of the head. The bytes whose values
//! come from the counts (the tainted cells, of RAM or of the registers)
//! are followed through it, from those that differed between two visits
//! (the seeds). An instruction may use them only as this module models
//! it:
//!
//! - MOV, PUSH and POP copy them, between registers and RAM;
//! - INC, DEC, ADD and SUB move them on, by a constant or by a value the
//!   counts don't reach;
//! - CMP and TEST compare them with such a value, and set flags.
//!
//! Anything else that reads a tainted cell ends the proof: a count used
//! as an address, an argument of a built-in service, or the operand of
//! any other instruction may make the passes differ in more than the
//! counts. A write of a value that doesn't come from the counts makes a
//! cell ordinary again. The pass is proven when the model, run from the
//! seeds' values at its start, finds in every step the values the CPU
//! found, and leaves the cells as the CPU left them, with no cell tainted
//! that wasn't a seed (else the pass is recorded again with those seeds,
//! up to `ROUNDS` times). The passes after it then run the same steps,
//! and the same instructions, for as long as each comparison sets the
//! flags read after it as it did in the recorded pass (`counted_passes`).
//!
//! The built-in Windows' services take their arguments on the stack and
//! keep EBX, ESI, EDI and EBP (stdcall), so a count may stay in one of
//! those across a service that says it changes nothing (`pure_call`).
//! The RAM it reads must hold no count: its arguments, and what it names
//! beyond them (`reads_arguments`, `reads_also`), or else everything but
//! the free stack below its caller's stack top.

use std::collections::BTreeSet;
use std::ops::Range;

use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpAccess, OpKind, Register};

use super::{
    Access, CHUNK, FACTORY, PASS_LIMIT, Phase, REP_SPAN, STACK_REACH, Visit, code_at, copy_blocks,
    entries_into, finish, pages, physical_into, present, rflags, segment_runs,
};
use crate::cpu::Cpu;

/// A byte the proof follows: of RAM, by its physical address, or of a
/// general register (`reg_cell`).
pub(super) type Cell = u64;

/// The first register cell: EAX's low byte, then the other bytes and
/// registers in the CPU's order.
const REG_CELLS: Cell = 1 << 40;
/// Bytes of RAM that may hold counts at the head.
const SEED_BYTES: usize = 64;
/// Passes recorded from one start, each with the cells the one before
/// found the counts in.
const ROUNDS: u8 = 3;
/// Bytes below the stack's top that a service reads nothing of.
const FREE_STACK: u32 = 0x1_0000;

/// The general registers, in the CPU's order.
pub(super) const GPR32: [Register; 8] = [
    Register::EAX,
    Register::ECX,
    Register::EDX,
    Register::EBX,
    Register::ESP,
    Register::EBP,
    Register::ESI,
    Register::EDI,
];
const ESP: usize = 4;

/// The first cell of general register `index`.
fn reg_cell(index: usize) -> Cell {
    REG_CELLS + 4 * index as Cell
}

/// The cells of `reg` if it is a general register: the first and how
/// many.
fn reg_cells(reg: Register) -> Option<(Cell, u8)> {
    let n = reg as usize;
    let al = Register::AL as usize;
    let ah = Register::AH as usize;
    let (index, byte, size) = if reg.is_gpr32() {
        (n - Register::EAX as usize, 0, 4)
    } else if reg.is_gpr16() {
        (n - Register::AX as usize, 0, 2)
    } else if (al..al + 4).contains(&n) {
        (n - al, 0, 1)
    } else if (ah..ah + 4).contains(&n) {
        (n - ah, 1, 1)
    } else {
        return None;
    };
    (index < 8).then(|| (reg_cell(index) + byte, size))
}

/// The cells of `seeds` in general registers, as a mask of registers.
pub(super) fn registers_of(seeds: &[Cell]) -> u8 {
    seeds
        .iter()
        .filter(|&&c| c >= REG_CELLS)
        .fold(0, |m, &c| m | 1 << ((c - REG_CELLS) / 4))
}

/// The cells of the registers in `mask`.
pub(super) fn register_cells(mask: u8) -> impl Iterator<Item = Cell> {
    (0..8)
        .filter(move |i| mask & 1 << i != 0)
        .flat_map(|i| reg_cell(i)..reg_cell(i) + 4)
}

/// The value of cell `cell` in the machine as it is.
fn value_now(cpu: &Cpu, cell: Cell) -> u8 {
    if cell >= REG_CELLS {
        let at = (cell - REG_CELLS) as usize;
        (cpu.reg(GPR32[at / 4]) >> (8 * (at % 4))) as u8
    } else {
        cpu.bus.peek_8(cell as usize)
    }
}

/// Of the registers `gpr`.
fn reg_byte(gpr: &[u32; 8], cell: Cell) -> u8 {
    let at = (cell - REG_CELLS) as usize;
    (gpr[at / 4] >> (8 * (at % 4))) as u8
}

/// Set cell `cell` of the machine to `value`.
pub(super) fn set_cell(cpu: &mut Cpu, cell: Cell, value: u8) {
    if cell >= REG_CELLS {
        let at = (cell - REG_CELLS) as usize;
        let reg = GPR32[at / 4];
        let shift = 8 * (at % 4);
        let old = cpu.reg(reg);
        cpu.set_reg(reg, old & !(0xFF << shift) | (value as u32) << shift);
    } else {
        cpu.bus.write_8(cell as usize, value);
    }
}

/// What an instruction the model follows does with its operands.
///
/// MUL and DIV have a second destination, `hi`: MUL puts the product's
/// upper half there (AH, DX or EDX), and DIV takes the dividend's upper
/// half from it and leaves the remainder there. ADC and SBB take the carry
/// the recorded pass had, which the passes after it have too: a count
/// that set it sets it as it did there (`Model::checks`).
#[derive(Clone, Copy, PartialEq, Debug)]
enum CountOp {
    Mov,
    Inc,
    Dec,
    Add,
    Sub,
    Adc,
    Sbb,
    Neg,
    Not,
    And,
    Or,
    Xor,
    Mul,
    Div,
    Cmp,
    Test,
}

/// What a step of the model left: the destination's new value (None
/// where it keeps it), `hi`'s, and the flags, of those it sets.
type Outcome = (Option<u32>, Option<u32>, u32);

impl CountOp {
    /// What it leaves for the operands' values, or None where it raises
    /// an exception (a DIV by 0 or whose quotient doesn't fit), which the
    /// pass recorded didn't.
    fn run(self, size: u8, dst: u32, src: u32, hi: u32, carry: bool) -> Option<Outcome> {
        use crate::cpu::alu;
        let m = alu::size_mask(size);
        let (dst, src, hi) = (dst & m, src & m, hi & m);
        let value = |(r, f): (u32, u32)| Some((Some(r), None, f));
        match self {
            CountOp::Mov => Some((Some(src), None, 0)),
            CountOp::Inc => {
                let (r, f) = alu::add(size, dst, 1, false);
                Some((Some(r), None, f & !alu::CF))
            }
            CountOp::Dec => {
                let (r, f) = alu::sub(size, dst, 1, false);
                Some((Some(r), None, f & !alu::CF))
            }
            CountOp::Add => value(alu::add(size, dst, src, false)),
            CountOp::Sub => value(alu::sub(size, dst, src, false)),
            CountOp::Adc => value(alu::add(size, dst, src, carry)),
            CountOp::Sbb => value(alu::sub(size, dst, src, carry)),
            CountOp::Neg => value(alu::sub(size, 0, dst, false)),
            CountOp::Not => Some((Some(!dst & m), None, 0)),
            CountOp::And => Some((Some(dst & src), None, alu::logic(size, dst & src))),
            CountOp::Or => Some((Some(dst | src), None, alu::logic(size, dst | src))),
            CountOp::Xor => Some((Some(dst ^ src), None, alu::logic(size, dst ^ src))),
            CountOp::Mul => {
                // As `instructions::arith::mul`: CF and OF tell whether
                // the upper half is in use.
                let bits = 8 * size as u32;
                let product = dst as u64 * src as u64;
                let upper = (product >> bits) as u32;
                let flags = if upper != 0 { alu::CF | alu::OF } else { 0 };
                Some((Some(product as u32 & m), Some(upper), flags))
            }
            CountOp::Div => {
                let bits = 8 * size as u32;
                let dividend = (hi as u64) << bits | dst as u64;
                if src == 0 || dividend / src as u64 > m as u64 {
                    return None;
                }
                let (q, r) = (
                    (dividend / src as u64) as u32,
                    (dividend % src as u64) as u32,
                );
                let flags = crate::instructions::arith::division_flags(q, r, bits);
                Some((Some(q), Some(r), flags))
            }
            CountOp::Cmp => Some((None, None, alu::sub(size, dst, src, false).1)),
            CountOp::Test => Some((None, None, alu::logic(size, dst & src))),
        }
    }

    /// The flags it sets.
    fn sets(self) -> u32 {
        use crate::cpu::alu::{ARITH, CF, OF};
        match self {
            CountOp::Mov | CountOp::Not => 0,
            CountOp::Inc | CountOp::Dec => ARITH & !CF,
            CountOp::Mul => CF | OF,
            _ => ARITH,
        }
    }

    fn reads_dst(self) -> bool {
        self != CountOp::Mov
    }

    fn writes_dst(self) -> bool {
        !matches!(self, CountOp::Cmp | CountOp::Test)
    }

    /// Whether it has a second destination (`hi`), and reads it.
    fn has_hi(self) -> bool {
        matches!(self, CountOp::Mul | CountOp::Div)
    }

    fn reads_hi(self) -> bool {
        self == CountOp::Div
    }
}

/// Where an operand is.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Place {
    Reg(Cell),
    Ram(usize),
    Imm(u32),
}

impl Place {
    /// Its cells, for an operand of `size` bytes.
    fn cells(self, size: u8) -> Option<Range<Cell>> {
        match self {
            Place::Reg(c) => Some(c..c + size as Cell),
            Place::Ram(a) => Some(a as Cell..a as Cell + size as Cell),
            Place::Imm(_) => None,
        }
    }

    /// Its value in the machine as it is.
    fn value(self, cpu: &Cpu, size: u8) -> u32 {
        match self {
            Place::Imm(v) => v & crate::cpu::alu::size_mask(size),
            _ => {
                let cells = self.cells(size).unwrap_or(0..0);
                cells
                    .enumerate()
                    .fold(0, |v, (i, c)| v | (value_now(cpu, c) as u32) << (8 * i))
            }
        }
    }
}

/// An instruction the model can follow, with its operands: `hi` of MUL
/// and DIV (`CountOp`), and the carry ADC and SBB found.
struct Operation {
    op: CountOp,
    size: u8,
    dst: Place,
    src: Place,
    hi: Option<Place>,
    carry: bool,
}

impl Operation {
    /// Whether it reads a cell holding a count.
    fn reads_count(&self, pass: &Pass) -> bool {
        let any = |p: Option<Place>| {
            p.and_then(|p| p.cells(self.size))
                .is_some_and(|c| pass.any(&c))
        };
        any(Some(self.src))
            || self.op.reads_dst() && any(Some(self.dst))
            || self.op.reads_hi() && any(self.hi)
    }
}

/// An operand as the pass found it: its first cell (none for a constant)
/// and its value then, which its bytes that hold no count keep in every
/// pass.
#[derive(Clone, Copy)]
struct Operand {
    cell: Option<Cell>,
    fixed: u32,
}

/// An instruction of the pass that read a count.
#[derive(Clone, Copy)]
struct Counting {
    op: CountOp,
    size: u8,
    dst: Operand,
    src: Operand,
    /// MUL's and DIV's second destination (`CountOp`), a constant 0 for
    /// the others.
    hi: Operand,
    carry: bool,
    /// The instruction's place in the pass.
    at: usize,
}

/// What the pass did to the cells, in order.
enum Step {
    Op(Counting),
    /// A value that doesn't come from the counts was written to the cell.
    Kill(Cell),
    /// The built-in Windows' clock in milliseconds was read into the four
    /// cells from `cell` (EAX), `offset` instructions after the pass
    /// started, and was `value`: the passes after it read it at the same
    /// place in them, each the value the clock has then (`Model::run`).
    Clock {
        cell: Cell,
        offset: u64,
        value: u32,
    },
}

/// The pass `Phase::Count` records.
#[derive(Default)]
pub(super) struct Pass {
    /// The cells that may hold counts at the head, sorted.
    seeds: Vec<Cell>,
    /// The cells holding counts as the pass runs.
    tainted: BTreeSet<Cell>,
    steps: Vec<Step>,
    /// The arithmetic flags each instruction reads, and those it sets
    /// whatever they were.
    flags: Vec<(u32, u32)>,
    /// The RAM the instruction running wrote: from, to, and whether
    /// exactly those bytes.
    pending: Vec<(usize, usize, bool)>,
    /// The cells of RAM the instruction running sets from the counts,
    /// tainted once its writes are settled.
    deferred: Vec<Cell>,
    /// The instruction running is the trap of a Win16 stub in 16-bit
    /// code (`after_service`).
    stub16: bool,
}

impl Pass {
    pub(super) fn new(mut seeds: Vec<Cell>) -> Pass {
        seeds.sort_unstable();
        seeds.dedup();
        Pass {
            tainted: seeds.iter().copied().collect(),
            seeds,
            ..Pass::default()
        }
    }

    /// Whether the pass follows a read of the clock (`Step::Clock`).
    pub(super) fn reads_clock(&self) -> bool {
        self.steps.iter().any(|s| matches!(s, Step::Clock { .. }))
    }

    /// The general registers that may hold counts at the head.
    pub(super) fn registers(&self) -> u8 {
        registers_of(&self.seeds)
    }

    /// RAM `start..end` was written, `exact`ly those bytes or within
    /// them. False when the instruction writes too much to follow.
    pub(super) fn wrote(&mut self, start: usize, end: usize, exact: bool) -> bool {
        self.pending.push((start, end, exact));
        self.pending.len() <= 4 * PASS_LIMIT
    }

    fn any(&self, cells: &Range<Cell>) -> bool {
        self.tainted.range(cells.clone()).next().is_some()
    }

    /// Cells `cells` now hold values that don't come from the counts.
    fn kill(&mut self, cells: Range<Cell>) {
        let hit: Vec<Cell> = self.tainted.range(cells).copied().collect();
        for c in hit {
            self.tainted.remove(&c);
            self.steps.push(Step::Kill(c));
        }
    }
}

/// The writes of the instruction before: those of values that don't come
/// from the counts make their cells ordinary, then the cells it set from
/// them hold counts. A write of RAM that isn't known byte for byte over a
/// count ends the proof.
fn settle(cpu: &mut Cpu) {
    let o = &mut cpu.bus.observe;
    let Phase::Count { pass, .. } = &mut o.phase else {
        return;
    };
    if pass.pending.is_empty() && pass.deferred.is_empty() {
        return;
    }
    let mut pending = std::mem::take(&mut pass.pending);
    let mut deferred = std::mem::take(&mut pass.deferred);
    let mut inexact = false;
    for &(start, end, exact) in &pending {
        let hit: Vec<Cell> = pass
            .tainted
            .range(start as Cell..end as Cell)
            .filter(|c| !deferred.contains(c))
            .copied()
            .collect();
        inexact |= !exact && !hit.is_empty();
        for c in hit {
            pass.tainted.remove(&c);
            pass.steps.push(Step::Kill(c));
        }
    }
    pass.tainted.extend(deferred.drain(..));
    pending.clear();
    (pass.pending, pass.deferred) = (pending, deferred);
    if inexact {
        o.opaque_because("counter");
    }
}

/// The linear address of memory operand `operand` of `instr`.
fn linear(cpu: &Cpu, instr: &Instruction, operand: u32) -> Option<u32> {
    let seg_base =
        |r: Register| crate::cpu::Seg::from_register(r).map(|s| cpu.seg_cache(s).base as u64);
    instr
        .virtual_address(operand, 0, |r, _, _| seg_base(r).or(Some(cpu.reg(r) as u64)))
        .map(|a| a as u32)
}

/// The RAM of the `size` bytes at linear `lin`, if one run of it.
fn ram_at(cpu: &Cpu, lin: u32, size: u8, write: bool) -> Option<Place> {
    let mut run: Option<Access> = None;
    for piece in pages(cpu, lin, size as u64, write) {
        let piece = piece?;
        match &mut run {
            None => run = Some(piece),
            Some(r) if r.start + r.len == piece.start => r.len += piece.len,
            Some(_) => return None,
        }
    }
    run.map(|r| Place::Ram(r.start))
}

/// Operand `i` of `instr`, of `size` bytes.
fn place(cpu: &Cpu, instr: &Instruction, i: u32, size: u8, write: bool) -> Option<Place> {
    match instr.op_kind(i) {
        OpKind::Register => reg_cells(instr.op_register(i)).map(|(c, _)| Place::Reg(c)),
        OpKind::Memory => ram_at(cpu, linear(cpu, instr, i)?, size, write),
        OpKind::Immediate8
        | OpKind::Immediate16
        | OpKind::Immediate32
        | OpKind::Immediate8to16
        | OpKind::Immediate8to32 => Some(Place::Imm(instr.immediate(i) as u32)),
        _ => None,
    }
}

/// The size of operand `i` of `instr`.
fn size_of(instr: &Instruction, i: u32) -> Option<u8> {
    match instr.op_kind(i) {
        OpKind::Register => reg_cells(instr.op_register(i)).map(|(_, s)| s),
        OpKind::Memory => Some(instr.memory_size().size() as u8),
        _ => None,
    }
}

/// The `size` bytes `offset` from the stack's top.
fn stack(cpu: &Cpu, offset: i32, size: u8, write: bool) -> Option<Place> {
    let ss = cpu.seg_cache(crate::cpu::Seg::SS);
    let lin = if ss.attr & crate::cpu::ATTR_DB != 0 {
        ss.base.wrapping_add(cpu.esp().wrapping_add(offset as u32))
    } else {
        let off = (cpu.sp() as u32).wrapping_add(offset as u32) & 0xFFFF;
        if off + size as u32 > 0x1_0000 {
            return None;
        }
        ss.base.wrapping_add(off)
    };
    ram_at(cpu, lin, size, write)
}

/// What `instr` does, if it is one the model follows.
fn operation(cpu: &Cpu, instr: &Instruction) -> Option<Operation> {
    if instr.has_lock_prefix() || instr.is_string_instruction() {
        return None;
    }
    let mnemonic = instr.mnemonic();
    let op = match (mnemonic, instr.op_count()) {
        (Mnemonic::Mov | Mnemonic::Push | Mnemonic::Pop, _) => CountOp::Mov,
        (Mnemonic::Inc, 1) => CountOp::Inc,
        (Mnemonic::Dec, 1) => CountOp::Dec,
        (Mnemonic::Neg, 1) => CountOp::Neg,
        (Mnemonic::Not, 1) => CountOp::Not,
        (Mnemonic::Mul, 1) => CountOp::Mul,
        (Mnemonic::Div, 1) => CountOp::Div,
        (Mnemonic::Add, 2) => CountOp::Add,
        (Mnemonic::Sub, 2) => CountOp::Sub,
        (Mnemonic::Adc, 2) => CountOp::Adc,
        (Mnemonic::Sbb, 2) => CountOp::Sbb,
        (Mnemonic::And, 2) => CountOp::And,
        (Mnemonic::Or, 2) => CountOp::Or,
        (Mnemonic::Xor, 2) => CountOp::Xor,
        (Mnemonic::Cmp, 2) => CountOp::Cmp,
        (Mnemonic::Test, 2) => CountOp::Test,
        _ => return None,
    };
    let carry = cpu.get_cpu_flag(crate::cpu::CpuFlags::CF);
    if op.has_hi() {
        // AL, AX or EAX by the operand's size, and the upper half in AH,
        // DX or EDX.
        let size = size_of(instr, 0)?;
        let hi = if size == 1 {
            reg_cell(0) + 1
        } else {
            reg_cell(2)
        };
        let src = place(cpu, instr, 0, size, false)?;
        return Some(Operation {
            op,
            size,
            dst: Place::Reg(reg_cell(0)),
            src,
            hi: Some(Place::Reg(hi)),
            carry,
        });
    }
    let (size, dst, src) = match (mnemonic, instr.op_count()) {
        (Mnemonic::Push, 1) => {
            let size = u8::try_from(-instr.stack_pointer_increment()).ok()?;
            (size, stack(cpu, -(size as i32), size, true)?, place(cpu, instr, 0, size, false)?)
        }
        (Mnemonic::Pop, 1) => {
            let size = u8::try_from(instr.stack_pointer_increment()).ok()?;
            // POP to memory addresses it with the stack's top after it.
            if instr.op0_kind() == OpKind::Memory {
                return None;
            }
            (size, place(cpu, instr, 0, size, true)?, stack(cpu, 0, size, false)?)
        }
        (Mnemonic::Push | Mnemonic::Pop, _) => return None,
        (_, 1) => {
            let size = size_of(instr, 0)?;
            (size, place(cpu, instr, 0, size, true)?, Place::Imm(0))
        }
        (_, 2) => {
            let size = size_of(instr, 0)?;
            if size_of(instr, 1).is_some_and(|s| s != size) {
                return None;
            }
            (size, place(cpu, instr, 0, size, true)?, place(cpu, instr, 1, size, false)?)
        }
        _ => return None,
    };
    if !matches!(size, 1 | 2 | 4) || matches!(dst, Place::Imm(_)) {
        return None;
    }
    Some(Operation {
        op,
        size,
        dst,
        src,
        hi: None,
        carry,
    })
}

/// What `record` takes from an instruction's bytes alone, kept by its
/// place for the proofs after (`Observe::decoded`): a loop runs the same
/// instructions in each pass, and a loop proven again at every timer
/// event or millisecond records the same pass each time.
pub(super) struct Decoded {
    /// Its linear address, EIP and code size.
    lin: u32,
    eip: u32,
    bitness: u32,
    /// The bytes it was decoded from, and how many of them it depends on:
    /// its length, or all 15 where it doesn't decode (a trap) or runs past
    /// the bytes that could be read. A place read again uses it only where
    /// that many could be read and they are the same.
    bytes: [u8; 15],
    known: usize,
    instr: Instruction,
    used: Vec<iced_x86::UsedMemory>,
    regs: Vec<iced_x86::UsedRegister>,
}

/// Places of `Observe::decoded`.
pub(super) const DECODED: usize = 4096;

/// The instruction at linear address `ip`, with `bytes` there, of which
/// `mapped` could be read: from `Observe::decoded` where it has the same
/// instruction, else decoded and kept there.
fn decode(
    cpu: &mut Cpu,
    ip: u32,
    bitness: u32,
    bytes: &[u8; 15],
    mapped: usize,
) -> std::sync::Arc<Decoded> {
    let eip = cpu.eip();
    let slot = (ip.wrapping_mul(0x9E37_79B1) >> 20) as usize % DECODED;
    let cache = &mut cpu.bus.observe.decoded;
    if let Some(Some(d)) = cache.get(slot)
        && (d.lin, d.eip, d.bitness) == (ip, eip, bitness)
        && d.known <= mapped
        && d.bytes[..d.known] == bytes[..d.known]
    {
        return d.clone();
    }
    let instr = Decoder::with_ip(bitness, bytes, eip as u64, DecoderOptions::NONE).decode();
    let (used, regs) = FACTORY.with_borrow_mut(|f| {
        let info = f.info(&instr);
        (info.used_memory().to_vec(), info.used_registers().to_vec())
    });
    // An instruction decoded from fewer bytes than it needs (or that
    // doesn't decode) depends on all 15, the zeros past `mapped` too:
    // with more of them there later, it may decode as another.
    let known = if instr.is_invalid() || instr.len() > mapped {
        bytes.len()
    } else {
        instr.len()
    };
    let d = std::sync::Arc::new(Decoded {
        lin: ip,
        eip,
        bitness,
        bytes: *bytes,
        known,
        instr,
        used,
        regs,
    });
    if cache.is_empty() {
        cache.resize(DECODED, None);
    }
    cache[slot] = Some(d.clone());
    d
}

/// During `Phase::Count`, the instruction about to run: what it does to
/// the flags, and to the cells holding counts.
pub(super) fn record(cpu: &mut Cpu) {
    use crate::cpu::alu::ARITH;
    settle(cpu);
    let paged = cpu.cr0 & crate::cpu::CR0_PG != 0;
    let cs = cpu.seg_cache(crate::cpu::Seg::CS);
    let bitness = if cs.attr & crate::cpu::ATTR_DB != 0 { 32 } else { 16 };
    let ip = cs.base.wrapping_add(cpu.eip());
    let (bytes, mapped) = code_at(cpu, ip);
    let decoded = decode(cpu, ip, bitness, &bytes, mapped);
    let instr = decoded.instr;
    // An instruction that runs into a page that isn't there faults, and
    // the fault ends the proof: its bytes aren't known before.
    if instr.len() > mapped || instr.is_invalid() && mapped < bytes.len() {
        return cpu.bus.observe.fail("paging");
    }
    // The page table entries that find its bytes and the RAM it reaches.
    let (mut reads, mut tables) = std::mem::take(&mut cpu.bus.observe.scratch);
    reads.clear();
    tables.clear();
    entries_into(cpu, ip, instr.len() as u64, &mut tables);
    let trap = instr.is_invalid() && bytes[0] == 0xFE;
    // A service of the built-in Windows called from 32-bit code, which
    // takes its arguments on the stack (`after_service`), or the stub of
    // a Win16 system module's function called from the program's 16-bit
    // code, which takes them on the 16-bit stack (`idle::arguments16`).
    let service16 =
        trap && bytes[1] == crate::win32::TRAP && bitness == 16 && crate::win16::in_16bit_code(cpu);
    let service = trap && bytes[1] == crate::win32::TRAP && bitness == 32 || service16;
    // The BIOS's and DOS's services read memory of their own (the BIOS
    // data area, DOS's data), which with paging on is where the page
    // tables put it, not where the window below looks.
    if paged && trap && bytes[1] != crate::win32::TRAP {
        return cpu.bus.observe.fail("count service");
    }
    let mnemonic = instr.mnemonic();
    // An emulator service trap (which decodes as nothing), and what pushes
    // the flags or goes through the interrupt table: all the flags may be
    // read, and RAM near the stack and the table reached.
    let opaque = instr.is_invalid()
        || matches!(
            mnemonic,
            Mnemonic::Int
                | Mnemonic::Int1
                | Mnemonic::Int3
                | Mnemonic::Into
                | Mnemonic::Iret
                | Mnemonic::Iretd
                | Mnemonic::Pushf
                | Mnemonic::Pushfd
        );
    // (The count is in CX or ECX by the address size.)
    let addr32 = instr.op_kinds().any(|k| {
        matches!(k, OpKind::MemorySegESI | OpKind::MemorySegEDI | OpKind::MemoryESEDI)
    });
    let rep_count = |cpu: &Cpu| if addr32 { cpu.ecx() } else { cpu.cx() as u32 };
    let repeated =
        instr.is_string_instruction() && (instr.has_rep_prefix() || instr.has_repne_prefix());
    // A service of the built-in Windows reads no register but ESP, the
    // flags neither (`pure_call`): what one leaves in them comes from what
    // it read, as a written register's value does.
    let flags = if service {
        (0, 0)
    } else if opaque {
        (u32::MAX, 0)
    } else {
        let read = rflags(instr.rflags_read());
        let mut kill = rflags(instr.rflags_written() | instr.rflags_cleared() | instr.rflags_set());
        // A shift by a count of 0 and a REP of none leave the flags.
        let shift = matches!(
            mnemonic,
            Mnemonic::Rol
                | Mnemonic::Ror
                | Mnemonic::Rcl
                | Mnemonic::Rcr
                | Mnemonic::Shl
                | Mnemonic::Sal
                | Mnemonic::Shr
                | Mnemonic::Sar
                | Mnemonic::Shld
                | Mnemonic::Shrd
        );
        let count = match instr.op_kinds().last() {
            Some(OpKind::Immediate8) => instr.immediate8() as u32,
            _ => cpu.cx() as u32 & 0xFF,
        };
        if shift && count & 0x1F == 0 || repeated && rep_count(cpu) == 0 {
            kill = 0;
        }
        // AF is undefined after AND, OR, XOR and TEST; the CPU clears it
        // (`alu::logic`), so what an instruction before left there is
        // gone.
        let logic = matches!(
            mnemonic,
            Mnemonic::And | Mnemonic::Or | Mnemonic::Xor | Mnemonic::Test
        );
        if logic {
            kill |= rflags(instr.rflags_undefined());
        }
        (read & ARITH, kill & ARITH)
    };
    // The RAM it reads. What it writes the bus reports as it writes it
    // (`Pass::wrote`).
    let (used, regs) = (&decoded.used, &decoded.regs);
    for m in used {
        let write = matches!(m.access(), OpAccess::Write | OpAccess::CondWrite);
        let seg_base = |r: Register| {
            crate::cpu::Seg::from_register(r).map(|s| cpu.seg_cache(s).base as u64)
        };
        let Some(addr) = m.virtual_address(0, |r, _, _| seg_base(r).or(Some(cpu.reg(r) as u64)))
        else {
            return cpu.bus.observe.fail("count");
        };
        let size = m.memory_size().size().max(1) as u64;
        // The linear bytes it reaches: for a REP, its elements from the
        // one at `addr` on, up or down. 16-bit offsets wrap in their
        // segment, which may make them two runs.
        let runs = if repeated {
            let span = rep_count(cpu) as u64 * size;
            let down = cpu.get_cpu_flag(crate::cpu::CpuFlags::DF);
            match m.address_size() {
                iced_x86::CodeSize::Code16 => {
                    let base = seg_base(m.segment()).unwrap_or(0) as u32;
                    let off = (addr as u32).wrapping_sub(base) & 0xFFFF;
                    let span = span.min(0x1_0000) as u32;
                    let first = if down {
                        (off + size as u32).wrapping_sub(span) & 0xFFFF
                    } else {
                        off
                    };
                    segment_runs(base, first, span)
                }
                iced_x86::CodeSize::Code32 if span <= REP_SPAN => {
                    let back = span.saturating_sub(size) as u32;
                    let start = if down {
                        (addr as u32).wrapping_sub(back)
                    } else {
                        addr as u32
                    };
                    [(start, span), (0, 0)]
                }
                _ => return cpu.bus.observe.fail("count"),
            }
        } else {
            [(addr as u32, size), (0, 0)]
        };
        for (lin, len) in runs {
            let from = reads.len();
            if !physical_into(cpu, lin, len, write, &mut reads) {
                return cpu.bus.observe.fail("paging");
            }
            if write {
                reads.truncate(from);
            }
            entries_into(cpu, lin, len, &mut tables);
        }
    }
    if opaque && !service {
        // The 64 bytes either side of the stack's top (a 16-bit stack's
        // offsets wrap at 64K), the interrupt table and the BIOS's data
        // area, and in protected mode the IDT.
        let ss = cpu.seg_cache(crate::cpu::Seg::SS);
        let runs = if ss.attr & crate::cpu::ATTR_DB != 0 {
            [
                (
                    ss.base.wrapping_add(cpu.esp().wrapping_sub(STACK_REACH)),
                    2 * STACK_REACH as u64,
                ),
                (0, 0),
            ]
        } else {
            let off = (cpu.sp() as u32).wrapping_sub(STACK_REACH) & 0xFFFF;
            segment_runs(ss.base, off, 2 * STACK_REACH)
        };
        for (lin, len) in runs {
            reads.extend(present(cpu, lin, len, false));
            entries_into(cpu, lin, len, &mut tables);
        }
        reads.push(Access { start: 0, len: 0x400 });
        if cpu.pe() {
            let idt = cpu.idtr;
            reads.extend(present(cpu, idt.base, idt.limit as u64 + 1, false));
            entries_into(cpu, idt.base, idt.limit as u64 + 1, &mut tables);
        }
    }
    reads.extend_from_slice(&tables);
    let operation = if opaque { None } else { operation(cpu, &instr) };
    // XOR and SUB of a register with itself read nothing of it.
    let zeroes = matches!(mnemonic, Mnemonic::Xor | Mnemonic::Sub)
        && instr.op_count() == 2
        && instr.op0_kind() == OpKind::Register
        && instr.op1_kind() == OpKind::Register
        && instr.op0_register() == instr.op1_register();
    let values = operation.as_ref().map(|m| {
        let hi = m.hi.map_or(0, |h| h.value(cpu, m.size));
        (m.dst.value(cpu, m.size), m.src.value(cpu, m.size), hi)
    });
    let o = &mut cpu.bus.observe;
    let Phase::Count { pass, .. } = &mut o.phase else {
        return;
    };
    if pass.flags.len() >= PASS_LIMIT {
        return o.fail("pass");
    }
    let at = pass.flags.len();
    pass.flags.push(flags);
    pass.stub16 = service16;
    // The instruction counts if it reads a count, as the model follows. (A
    // register XORed or subtracted from itself is 0, whatever it held.)
    let counting = operation.filter(|m| !zeroes && m.reads_count(pass));
    let covered: Vec<Range<Cell>> = counting
        .iter()
        .flat_map(|m| [Some(m.dst), Some(m.src), m.hi].map(|p| p.and_then(|p| p.cells(m.size))))
        .flatten()
        .collect();
    let is_covered = |c: Cell| covered.iter().any(|r| r.contains(&c));
    let mut failed = None;
    if opaque {
        // A service keeps EBX, ESI, EDI and EBP, sets EAX to what it
        // returns, and reads no register but ESP; it may change ECX and
        // EDX (`after_service`). What a Win16 function leaves in the
        // registers depends on whether it calls 32-bit code, which
        // `after_service` looks at. Anything else may read any.
        let open = if service16 || service {
            0..0
        } else {
            reg_cell(0)..reg_cell(8)
        };
        if pass.any(&open) {
            failed = Some(if service { "count service" } else { "counter" });
        }
    } else {
        for u in regs {
            let Some((c, size)) = reg_cells(u.register()) else {
                continue;
            };
            let cells = c..c + size as Cell;
            if is_covered(c) {
                continue;
            }
            let reads = !zeroes
                && matches!(
                    u.access(),
                    OpAccess::Read
                        | OpAccess::CondRead
                        | OpAccess::ReadWrite
                        | OpAccess::ReadCondWrite
                        | OpAccess::CondWrite
                );
            if reads && pass.any(&cells) {
                failed = Some("counter");
            } else if matches!(u.access(), OpAccess::Write | OpAccess::ReadWrite) {
                pass.kill(cells);
            }
        }
    }
    for a in &reads {
        let range = a.start as Cell..(a.start + a.len) as Cell;
        if pass.tainted.range(range).any(|&c| !is_covered(c)) {
            failed = Some("counter");
        }
    }
    o.scratch = (reads, tables);
    if let Some(why) = failed {
        return o.fail(why);
    }
    let (Some(m), Some((dst_value, src_value, hi_value))) = (counting, values) else {
        return;
    };
    // A count may not move the stack.
    if m.dst == Place::Reg(reg_cell(ESP)) {
        return o.fail("counter");
    }
    let operand = |p: Place, fixed: u32| Operand {
        cell: p.cells(m.size).map(|c| c.start),
        fixed,
    };
    pass.steps.push(Step::Op(Counting {
        op: m.op,
        size: m.size,
        dst: operand(m.dst, dst_value),
        src: operand(m.src, src_value),
        hi: operand(m.hi.unwrap_or(Place::Imm(0)), hi_value),
        carry: m.carry,
        at,
    }));
    let written = [m.op.writes_dst().then_some(m.dst), m.hi];
    for place in written.into_iter().flatten() {
        let cells = place.cells(m.size).unwrap_or(0..0);
        match place {
            Place::Reg(_) => pass.tainted.extend(cells),
            _ => pass.deferred.extend(cells),
        }
    }
}

/// A step of the model, its cells by their places in its list.
enum Run {
    Op {
        op: CountOp,
        size: u8,
        dst: (Option<usize>, u32),
        src: (Option<usize>, u32),
        hi: (Option<usize>, u32),
        carry: bool,
    },
    Kill(usize),
    Clock {
        slot: usize,
        offset: u64,
    },
}

/// The recorded pass as the model runs it: each cell it follows has a
/// slot, which holds the cell's value while it holds a count.
pub(super) struct Model {
    cells: Vec<Cell>,
    seeds: Vec<usize>,
    runs: Vec<Run>,
    /// For each operation, the flags it sets that something reads before
    /// they are set again, those flags as the recorded pass set them, and
    /// the flags it sets that the head finds. A pass may leave the head
    /// other flags than the one before it, as a SUB of the time before a
    /// call of the built-in Windows does: the passes skipped leave those
    /// of the last (`counted_passes`).
    checks: Vec<(u32, u32, u32)>,
}

type State = Vec<Option<u8>>;

impl Model {
    fn new(pass: &Pass) -> Model {
        let mut cells: Vec<Cell> = pass.seeds.clone();
        for step in &pass.steps {
            match step {
                Step::Op(c) => {
                    for o in [c.dst, c.src, c.hi] {
                        cells.extend(o.cell.map(|s| s..s + c.size as Cell).into_iter().flatten());
                    }
                }
                Step::Kill(cell) => cells.push(*cell),
                Step::Clock { cell, .. } => cells.extend(*cell..cell + 4),
            }
        }
        cells.sort_unstable();
        cells.dedup();
        let slot = |c: Cell| cells.binary_search(&c).unwrap_or(0);
        let runs = pass
            .steps
            .iter()
            .map(|step| match step {
                Step::Op(c) => Run::Op {
                    op: c.op,
                    size: c.size,
                    dst: (c.dst.cell.map(slot), c.dst.fixed),
                    src: (c.src.cell.map(slot), c.src.fixed),
                    hi: (c.hi.cell.map(slot), c.hi.fixed),
                    carry: c.carry,
                },
                Step::Kill(cell) => Run::Kill(slot(*cell)),
                &Step::Clock { cell, offset, .. } => Run::Clock {
                    slot: slot(cell),
                    offset,
                },
            })
            .collect();
        let seeds = pass.seeds.iter().map(|&c| slot(c)).collect();
        Model {
            cells,
            seeds,
            runs,
            checks: Vec::new(),
        }
    }

    /// The state with the seeds' values `value` gives.
    fn seeded(&self, value: impl Fn(Cell) -> u8) -> State {
        let mut state = vec![None; self.cells.len()];
        for &s in &self.seeds {
            state[s] = Some(value(self.cells[s]));
        }
        state
    }

    /// Run the pass on `state`, with `clock(offset)` the milliseconds the
    /// built-in Windows' clock has `offset` instructions after the pass
    /// started, calling `each` with every operation's index, the
    /// destination's, source's and `hi`'s values it found and the flags it
    /// set; stops where `each` returns false, or where an operation raises
    /// an exception. Whether it ran to the end.
    fn run(
        &self,
        state: &mut State,
        clock: impl Fn(u64) -> u32,
        mut each: impl FnMut(usize, [u32; 3], u32) -> bool,
    ) -> bool {
        let read = |state: &State, (slot, fixed): (Option<usize>, u32), size: u8| {
            (0..size as usize).fold(0u32, |v, i| {
                let byte = slot
                    .and_then(|s| state[s + i])
                    .unwrap_or((fixed >> (8 * i)) as u8);
                v | (byte as u32) << (8 * i)
            })
        };
        let write = |state: &mut State, slot: Option<usize>, value: Option<u32>, size: u8| {
            if let (Some(value), Some(slot)) = (value, slot) {
                for i in 0..size as usize {
                    state[slot + i] = Some((value >> (8 * i)) as u8);
                }
            }
        };
        let mut index = 0;
        for run in &self.runs {
            match *run {
                Run::Kill(s) => state[s] = None,
                Run::Clock { slot, offset } => write(state, Some(slot), Some(clock(offset)), 4),
                Run::Op {
                    op,
                    size,
                    dst,
                    src,
                    hi,
                    carry,
                } => {
                    let found = [dst, src, hi].map(|o| read(state, o, size));
                    let [d, s, h] = found;
                    let Some((after, upper, flags)) = op.run(size, d, s, h, carry) else {
                        return false;
                    };
                    write(state, dst.0, after, size);
                    write(state, hi.0, upper, size);
                    if !each(index, found, flags) {
                        return false;
                    }
                    index += 1;
                }
            }
        }
        true
    }
}

/// At the visit after the pass `Phase::Count` recorded, with the
/// registers of its base but for the counts: the model must find in every
/// step what the CPU found, and leave the cells as they are now, with the
/// counts in no cells but the seeds. Then the passes after it run the same
/// as long as each comparison sets the flags read after it as it did in
/// the recorded pass (`counted_passes`).
pub(super) fn count(cpu: &mut Cpu, visit: Visit) {
    settle(cpu);
    let chunks = cpu.bus.observe.take_chunks();
    let o = &mut cpu.bus.observe;
    let Phase::Count {
        head,
        base,
        copies,
        pass,
        until,
        rounds,
        ..
    } = std::mem::take(&mut o.phase)
    else {
        return;
    };
    // The head stays known, so `fail` backs off and `finish` remembers it.
    o.phase = Phase::Armed { head, until };
    let fail = |cpu: &mut Cpu, why| {
        cpu.bus.observe.on = true;
        cpu.bus.observe.fail(why)
    };
    if let Some(why) = cpu.bus.observe.failed.get() {
        return fail(cpu, why);
    }
    let ram = cpu.bus.ram();
    let mut differ: BTreeSet<Cell> = BTreeSet::new();
    for &c in &chunks {
        let Ok(i) = copies.binary_search_by_key(&(c as usize), |(k, _)| *k) else {
            return fail(cpu, "count blocks");
        };
        let start = c as usize * CHUNK;
        differ.extend(
            (0..CHUNK)
                .filter(|&b| ram[start + b] != copies[i].1[b])
                .map(|b| (start + b) as Cell),
        );
    }
    let (from, to) = (&base.key.regs.gpr, &visit.key.regs.gpr);
    differ.extend(register_cells(0xFF).filter(|&c| reg_byte(from, c) != reg_byte(to, c)));
    let at_base = |cell: Cell| {
        if cell >= REG_CELLS {
            return reg_byte(from, cell);
        }
        let a = cell as usize;
        match copies.binary_search_by_key(&(a / CHUNK), |(k, _)| *k) {
            Ok(i) => copies[i].1[a % CHUNK],
            Err(_) => ram[a],
        }
    };
    let now = |cell: Cell| {
        if cell >= REG_CELLS {
            reg_byte(to, cell)
        } else {
            ram[cell as usize]
        }
    };
    // The pass as the model makes it, from the seeds at the base: what each
    // step found must be what the CPU found.
    let mut model = Model::new(&pass);
    let mut state = model.seeded(at_base);
    let ops: Vec<&Counting> = pass
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Op(c) => Some(c),
            Step::Kill(_) | Step::Clock { .. } => None,
        })
        .collect();
    // The clock as the pass read it, which the model reads from the
    // emulated clock at the same places.
    let clock = |offset: u64| clock_at(cpu, base.icount + offset);
    let clocked = pass.steps.iter().all(|s| match *s {
        Step::Clock { offset, value, .. } => clock(offset) == value,
        _ => true,
    });
    let mask = |size: u8| crate::cpu::alu::size_mask(size);
    let mut seen = Vec::with_capacity(ops.len());
    let same = clocked
        && model.run(&mut state, clock, |i, found, flags| {
            let op = ops[i];
            seen.push(flags);
            let fixed = [op.dst.fixed, op.src.fixed, op.hi.fixed];
            found
                .iter()
                .zip(fixed)
                .all(|(&v, f)| v == f & mask(op.size))
        });
    let ends_as_now = state
        .iter()
        .zip(&model.cells)
        .all(|(v, &c)| v.is_none_or(|v| v == now(c)));
    if !same || !ends_as_now {
        return fail(cpu, "count model");
    }
    let end: Vec<Cell> = state
        .iter()
        .zip(&model.cells)
        .filter(|(v, _)| v.is_some())
        .map(|(_, &c)| c)
        .collect();
    // Counts the seeds didn't have, or bytes that changed outside them:
    // the pass is recorded again with them among the seeds.
    let grows = end.iter().any(|c| pass.seeds.binary_search(c).is_err())
        || differ.iter().any(|c| end.binary_search(c).is_err());
    if grows {
        let mut seeds: Vec<Cell> = pass.seeds.clone();
        seeds.extend(end);
        seeds.extend(differ);
        seeds.sort_unstable();
        seeds.dedup();
        let in_ram = seeds.iter().filter(|&&c| c < REG_CELLS).count();
        if rounds + 1 >= ROUNDS || in_ram > SEED_BYTES || registers_of(&seeds) & 1 << ESP != 0 {
            return fail(cpu, "count rounds");
        }
        let blocks: Vec<usize> = copies
            .iter()
            .map(|(k, _)| *k)
            .chain(chunks.iter().map(|&c| c as usize))
            .collect();
        let copies = copy_blocks(cpu.bus.ram(), blocks);
        let o = &mut cpu.bus.observe;
        o.restart_ports();
        o.on = true;
        let pass = Pass::new(seeds);
        o.phase = Phase::Count {
            head,
            base: Box::new(visit),
            copies,
            loose: pass.registers(),
            pass: Box::new(pass),
            until,
            rounds: rounds + 1,
        };
        return;
    }
    // The flags of each operation that something reads before they are
    // set again, in this pass or, for those still set at its end, in the
    // next one, and those that reach the head.
    model.checks = ops
        .iter()
        .zip(&seen)
        .map(|(op, &flags)| {
            let mut left = op.op.sets();
            let mut live = 0;
            for &(read, kill) in &pass.flags[op.at + 1..] {
                live |= read & left;
                left &= !kill;
            }
            let out = left;
            for &(read, kill) in &pass.flags[..=op.at] {
                live |= read & left;
                left &= !kill;
            }
            (live, flags & live, out)
        })
        .collect();
    cpu.bus.observe.on = true;
    cpu.bus.observe.found = Some(super::Shape {
        head,
        seeds: pass.seeds.clone(),
        blocks: copies.iter().map(|(k, _)| *k).collect(),
    });
    finish(
        cpu,
        &visit,
        base.icount,
        base.executed,
        base.calls,
        Some(Box::new(model)),
    );
}

/// The milliseconds the built-in Windows' clock reads at instruction count
/// `icount`, as `idle::tick_count` reads them.
fn clock_at(cpu: &Cpu, icount: u64) -> u32 {
    (cpu.bus.clock.micros_at(icount) / 1000) as u32
}

/// How many of the next passes, up to `most`, run as the recorded one:
/// those whose operations set the flags read after them as it did, the
/// first starting at instruction count `from` and each `period` after the
/// one before. Also the cells holding counts after them, and the flags
/// the last leaves the head where they come from a count: a mask and
/// their values.
pub(super) fn counted_passes(
    cpu: &Cpu,
    model: &Model,
    most: u64,
    from: u64,
    period: u64,
) -> (u64, Vec<(Cell, u8)>, (u32, u32)) {
    let mut state = model.seeded(|c| value_now(cpu, c));
    let mut done = state.clone();
    let mut passes = 0;
    let mut head_flags = (0, 0);
    while passes < most {
        let start = from + passes * period;
        let clock = |offset: u64| clock_at(cpu, start + offset);
        let mut left = head_flags;
        let same = model.run(&mut state, clock, |i, _, flags| {
            let (live, seen, out) = model.checks[i];
            left = (left.0 | out, left.1 & !out | flags & out);
            flags & live == seen
        });
        if !same {
            break;
        }
        passes += 1;
        done.clone_from(&state);
        head_flags = left;
    }
    let values = done
        .iter()
        .zip(&model.cells)
        .filter_map(|(v, &c)| v.map(|v| (c, v)))
        .collect();
    (passes, values, head_flags)
}

/// The service running read the general registers (`reads_registers`):
/// a count in one of them would go into what it wrote, which the pass
/// doesn't model, so it ends the proof.
pub(super) fn registers_read(cpu: &mut Cpu) {
    let o = &cpu.bus.observe;
    let Phase::Count { pass, .. } = &o.phase else {
        return;
    };
    if pass.any(&(reg_cell(0)..reg_cell(8))) {
        o.opaque_because("count service");
    }
}

/// After a service the pass followed (`after_service`): it kept the
/// registers that hold counts, and read no RAM that does. A service that
/// read no RAM but its arguments (`reads_arguments`) names what it read
/// beyond them (`reads_also`); one that may have read any reads nothing
/// of the free stack below its caller's top.
///
/// The built-in Windows' clock read by a function that returns it
/// (`idle::tick_count`) becomes a count in EAX, which the passes after it
/// read anew (`Step::Clock`). A Win16 stub that called 32-bit code may
/// leave no count in a register: the registers 16-bit code goes on with
/// are those it had there, but for DX:AX, which Win16Return sets
/// (`idle::resumed16`), from a count in EAX where it holds one.
pub(super) fn after_service(cpu: &mut Cpu) {
    // A count in EAX is gone where the function returned a value there,
    // and may be anything where it didn't.
    let eax = reg_cell(0)..reg_cell(1);
    let all = reg_cell(0)..reg_cell(8);
    // DX:AX, which a Win16 function returns.
    let dx = reg_cell(2)..reg_cell(2) + 2;
    let returns = |c: &Cell| (eax.start..eax.start + 2).contains(c) || dx.contains(c);
    let value = cpu.eax();
    let o = &mut cpu.bus.observe;
    let returned = o.service_returned;
    let clock = o.service_clock.take();
    let resumed = o.resumed16.take();
    let regs = o.service_regs;
    let Phase::Count { pass, base, .. } = &mut o.phase else {
        return;
    };
    let base = base.icount;
    let stub16 = pass.stub16;
    let mut clocked = false;
    let mut moved = false;
    match clock {
        Some((ms, at, _)) if returned && value == ms => {
            pass.steps.push(Step::Clock {
                cell: eax.start,
                offset: at - base,
                value,
            });
            pass.tainted.extend(eax.clone());
            clocked = true;
        }
        _ if returned => pass.kill(eax.clone()),
        _ => {}
    }
    if let Some(as_is) = resumed {
        if pass.any(&eax) {
            if !as_is {
                return cpu.bus.observe.opaque_because("count service");
            }
            // AX keeps EAX's low half; DX takes its high half.
            let at = pass.flags.len().saturating_sub(1);
            pass.steps.push(Step::Op(Counting {
                op: CountOp::Mov,
                size: 2,
                dst: Operand {
                    cell: Some(dx.start),
                    fixed: regs[2] & 0xFFFF,
                },
                src: Operand {
                    cell: Some(eax.start + 2),
                    fixed: regs[0] >> 16,
                },
                hi: Operand {
                    cell: None,
                    fixed: 0,
                },
                carry: false,
                at,
            }));
            pass.tainted.extend(dx.clone());
            moved = true;
        }
        // The rest is the 16-bit context's, which held no count.
        let gone: Vec<Cell> = pass
            .tainted
            .range(all.clone())
            .copied()
            .filter(|c| !(moved && returns(c)))
            .collect();
        for c in gone {
            pass.kill(c..c + 1);
        }
    }
    // A Win16 function keeps SI, DI and BP. One that calls 32-bit code
    // (`builtin::auto_thunk`) gives 16-bit code back the registers it had
    // at the call, but for DX:AX, which it returns, and the 32-bit code
    // starts with registers of its own; one that doesn't may leave
    // anything in AX, BX, CX and DX.
    let thunked = stub16 && !crate::win16::in_16bit_code(cpu);
    let o = &mut cpu.bus.observe;
    o.clock_modelled |= clocked;
    let Phase::Count { pass, .. } = &mut o.phase else {
        return;
    };
    if thunked {
        if pass.tainted.range(all.clone()).any(|c| !returns(c)) {
            return o.opaque_because("count service");
        }
        pass.kill(all.clone());
    }
    let o = &cpu.bus.observe;
    let Phase::Count { pass, .. } = &o.phase else {
        return;
    };
    let changed = if resumed.is_some() || thunked {
        false
    } else if stub16 {
        pass.any(&(reg_cell(0)..reg_cell(4)))
            || (4..8).any(|i| {
                let cells = reg_cell(i)..reg_cell(i) + 4;
                pass.any(&cells) && cpu.reg(GPR32[i]) != o.service_regs[i]
            })
    } else {
        !clocked && pass.any(&eax)
            || (3..8).any(|i| {
                let cells = reg_cell(i)..reg_cell(i) + 4;
                pass.any(&cells) && cpu.reg(GPR32[i]) != o.service_regs[i]
            })
    };
    // A count in ECX or EDX that a 32-bit service changed is gone: the
    // service didn't read it. One it left as it was still holds it.
    let plain32 = !stub16 && !thunked && resumed.is_none();
    let scratch: Vec<Range<Cell>> = (1..3)
        .filter(|&i| plain32 && cpu.reg(GPR32[i]) != o.service_regs[i])
        .map(|i| reg_cell(i)..reg_cell(i) + 4)
        .collect();
    if let Phase::Count { pass, .. } = &mut cpu.bus.observe.phase {
        for cells in scratch {
            pass.kill(cells);
        }
    }
    if clock.is_some() && !clocked {
        // A read of the clock the passes can't follow: they hold up to its
        // next millisecond.
        let next = clock.map_or(0, |(_, _, next)| next);
        let o = &mut cpu.bus.observe;
        o.holds_until = Some(o.holds_until.map_or(next, |h| h.min(next)));
    }
    let o = &cpu.bus.observe;
    let Phase::Count { pass, .. } = &o.phase else {
        return;
    };
    let ram: Vec<usize> = pass.tainted.range(..REG_CELLS).map(|&c| c as usize).collect();
    let top = o.service_top;
    let reads_ok = ram.is_empty()
        || if o.arguments {
            let args = o.service_args.unwrap_or((top, STACK_REACH));
            o.service_reads.iter().chain([&args]).all(|&(lin, len)| {
                present(cpu, lin, len as u64, false)
                    .iter()
                    .all(|a| !ram.iter().any(|&c| a.covers(c)))
            })
        } else if o.service_flat {
            let low = top.saturating_sub(FREE_STACK);
            let free = present(cpu, low, (top - low) as u64, false);
            ram.iter().all(|&c| free.iter().any(|a| a.covers(c)))
        } else {
            false
        };
    if changed || !reads_ok {
        cpu.bus.observe.opaque_because("count service");
    }
}
