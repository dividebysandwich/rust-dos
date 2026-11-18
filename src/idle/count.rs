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
    code_at, copy_blocks, entries, finish, physical, present, rflags, segment_runs, Access, Phase,
    Visit, CHUNK, FACTORY, PASS_LIMIT, REP_SPAN, STACK_REACH,
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
#[derive(Clone, Copy, PartialEq, Debug)]
enum CountOp {
    Mov,
    Inc,
    Dec,
    Add,
    Sub,
    Cmp,
    Test,
}

impl CountOp {
    /// The destination's new value (None where it keeps it) and the flags,
    /// of those it sets.
    fn run(self, size: u8, dst: u32, src: u32) -> (Option<u32>, u32) {
        use crate::cpu::alu;
        let m = alu::size_mask(size);
        let (dst, src) = (dst & m, src & m);
        match self {
            CountOp::Mov => (Some(src), 0),
            CountOp::Inc => {
                let (r, f) = alu::add(size, dst, 1, false);
                (Some(r), f & !alu::CF)
            }
            CountOp::Dec => {
                let (r, f) = alu::sub(size, dst, 1, false);
                (Some(r), f & !alu::CF)
            }
            CountOp::Add => {
                let (r, f) = alu::add(size, dst, src, false);
                (Some(r), f)
            }
            CountOp::Sub => {
                let (r, f) = alu::sub(size, dst, src, false);
                (Some(r), f)
            }
            CountOp::Cmp => (None, alu::sub(size, dst, src, false).1),
            CountOp::Test => (None, alu::logic(size, dst & src)),
        }
    }

    /// The flags it sets.
    fn sets(self) -> u32 {
        use crate::cpu::alu::{ARITH, CF};
        match self {
            CountOp::Mov => 0,
            CountOp::Inc | CountOp::Dec => ARITH & !CF,
            _ => ARITH,
        }
    }

    fn reads_dst(self) -> bool {
        self != CountOp::Mov
    }

    fn writes_dst(self) -> bool {
        !matches!(self, CountOp::Cmp | CountOp::Test)
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

/// An instruction the model can follow, with its operands.
struct Operation {
    op: CountOp,
    size: u8,
    dst: Place,
    src: Place,
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
    /// The instruction's place in the pass.
    at: usize,
}

/// What the pass did to the cells, in order.
enum Step {
    Op(Counting),
    /// A value that doesn't come from the counts was written to the cell.
    Kill(Cell),
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
    let pending = std::mem::take(&mut pass.pending);
    let deferred = std::mem::take(&mut pass.deferred);
    let mut inexact = false;
    for (start, end, exact) in pending {
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
    pass.tainted.extend(deferred);
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
    match physical(cpu, lin, size as u64, write)?.as_slice() {
        [one] => Some(Place::Ram(one.start)),
        _ => None,
    }
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
        (Mnemonic::Add, 2) => CountOp::Add,
        (Mnemonic::Sub, 2) => CountOp::Sub,
        (Mnemonic::Cmp, 2) => CountOp::Cmp,
        (Mnemonic::Test, 2) => CountOp::Test,
        _ => return None,
    };
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
    Some(Operation { op, size, dst, src })
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
    let instr = Decoder::with_ip(bitness, &bytes, cpu.eip() as u64, DecoderOptions::NONE).decode();
    // An instruction that runs into a page that isn't there faults, and
    // the fault ends the proof: its bytes aren't known before.
    if instr.len() > mapped || instr.is_invalid() && mapped < bytes.len() {
        return cpu.bus.observe.fail("paging");
    }
    // The page table entries that find its bytes and the RAM it reaches.
    let mut tables = entries(cpu, ip, instr.len() as u64);
    let trap = instr.is_invalid() && bytes[0] == 0xFE;
    // A service of the built-in Windows called from 32-bit code, which
    // takes its arguments on the stack (`after_service`).
    let service = trap && bytes[1] == crate::win32::TRAP && bitness == 32;
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
        (read & ARITH, kill & ARITH)
    };
    // The RAM it reads. What it writes the bus reports as it writes it
    // (`Pass::wrote`).
    let mut reads: Vec<Access> = Vec::with_capacity(4);
    let (used, regs) = FACTORY.with_borrow_mut(|f| {
        let info = f.info(&instr);
        (info.used_memory().to_vec(), info.used_registers().to_vec())
    });
    for m in &used {
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
            let Some(found) = physical(cpu, lin, len, write) else {
                return cpu.bus.observe.fail("paging");
            };
            if !write {
                reads.extend(found);
            }
            tables.extend(entries(cpu, lin, len));
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
            tables.extend(entries(cpu, lin, len));
        }
        reads.push(Access { start: 0, len: 0x400 });
        if cpu.pe() {
            let idt = cpu.idtr;
            reads.extend(present(cpu, idt.base, idt.limit as u64 + 1, false));
            tables.extend(entries(cpu, idt.base, idt.limit as u64 + 1));
        }
    }
    reads.extend(tables);
    let operation = if opaque { None } else { operation(cpu, &instr) };
    // XOR and SUB of a register with itself read nothing of it.
    let zeroes = matches!(mnemonic, Mnemonic::Xor | Mnemonic::Sub)
        && instr.op_count() == 2
        && instr.op0_kind() == OpKind::Register
        && instr.op1_kind() == OpKind::Register
        && instr.op0_register() == instr.op1_register();
    let values = operation
        .as_ref()
        .map(|m| (m.dst.value(cpu, m.size), m.src.value(cpu, m.size)));
    let o = &mut cpu.bus.observe;
    let Phase::Count { pass, .. } = &mut o.phase else {
        return;
    };
    if pass.flags.len() >= PASS_LIMIT {
        return o.fail("pass");
    }
    let at = pass.flags.len();
    pass.flags.push(flags);
    // The instruction counts if it reads a count, as the model follows.
    let counting = operation.filter(|m| {
        let src = m.src.cells(m.size).is_some_and(|c| pass.any(&c));
        let dst = m.op.reads_dst() && m.dst.cells(m.size).is_some_and(|c| pass.any(&c));
        src || dst
    });
    let covered: Vec<Range<Cell>> = counting
        .iter()
        .flat_map(|m| [m.dst.cells(m.size), m.src.cells(m.size)])
        .flatten()
        .collect();
    let is_covered = |c: Cell| covered.iter().any(|r| r.contains(&c));
    let mut failed = None;
    if opaque {
        // A service keeps EBX, ESI, EDI and EBP, sets EAX to what it
        // returns, and reads no register but ESP (`after_service`); it may
        // change ECX and EDX. Anything else may read any.
        let open = if service {
            reg_cell(1)..reg_cell(3)
        } else {
            reg_cell(0)..reg_cell(8)
        };
        if pass.any(&open) {
            failed = Some(if service { "count service" } else { "counter" });
        }
    } else {
        for u in &regs {
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
    if let Some(why) = failed {
        return o.fail(why);
    }
    let (Some(m), Some((dst_value, src_value))) = (counting, values) else {
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
        at,
    }));
    if m.op.writes_dst() {
        let cells = m.dst.cells(m.size).unwrap_or(0..0);
        match m.dst {
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
    },
    Kill(usize),
}

/// The recorded pass as the model runs it: each cell it follows has a
/// slot, which holds the cell's value while it holds a count.
pub(super) struct Model {
    cells: Vec<Cell>,
    seeds: Vec<usize>,
    runs: Vec<Run>,
    /// For each operation, the flags it sets that something reads before
    /// they are set again (or at the head, whose flags the passes
    /// compare), and those flags as the recorded pass set them.
    checks: Vec<(u32, u32)>,
}

type State = Vec<Option<u8>>;

impl Model {
    fn new(pass: &Pass) -> Model {
        let mut cells: Vec<Cell> = pass.seeds.clone();
        for step in &pass.steps {
            match step {
                Step::Op(c) => {
                    for o in [c.dst, c.src] {
                        cells.extend(o.cell.map(|s| s..s + c.size as Cell).into_iter().flatten());
                    }
                }
                Step::Kill(cell) => cells.push(*cell),
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
                },
                Step::Kill(cell) => Run::Kill(slot(*cell)),
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

    /// Run the pass on `state`, calling `each` with every operation's
    /// index and the destination's and source's values it found and the
    /// flags it set; stops where `each` returns false. Whether it ran to
    /// the end.
    fn run(&self, state: &mut State, mut each: impl FnMut(usize, u32, u32, u32) -> bool) -> bool {
        let read = |state: &State, (slot, fixed): (Option<usize>, u32), size: u8| {
            (0..size as usize).fold(0u32, |v, i| {
                let byte = slot
                    .and_then(|s| state[s + i])
                    .unwrap_or((fixed >> (8 * i)) as u8);
                v | (byte as u32) << (8 * i)
            })
        };
        let mut index = 0;
        for run in &self.runs {
            match *run {
                Run::Kill(s) => state[s] = None,
                Run::Op { op, size, dst, src } => {
                    let (d, s) = (read(state, dst, size), read(state, src, size));
                    let (after, flags) = op.run(size, d, s);
                    if let (Some(after), Some(slot)) = (after, dst.0) {
                        for i in 0..size as usize {
                            state[slot + i] = Some((after >> (8 * i)) as u8);
                        }
                    }
                    if !each(index, d, s, flags) {
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
            Step::Kill(_) => None,
        })
        .collect();
    let mask = |size: u8| crate::cpu::alu::size_mask(size);
    let mut seen = Vec::with_capacity(ops.len());
    let same = model.run(&mut state, |i, d, s, flags| {
        let op = ops[i];
        seen.push(flags);
        d == op.dst.fixed & mask(op.size) && s == op.src.fixed & mask(op.size)
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
    // set again, or at the head.
    model.checks = ops
        .iter()
        .zip(&seen)
        .map(|(op, &flags)| {
            let mut left = op.op.sets();
            let mut live = 0;
            for &(read, kill) in &pass.flags[op.at + 1..] {
                live |= read & left;
                left &= !kill;
                if left == 0 {
                    break;
                }
            }
            let live = live | left;
            (live, flags & live)
        })
        .collect();
    cpu.bus.observe.on = true;
    finish(
        cpu,
        &visit,
        base.icount,
        base.executed,
        base.calls,
        Some(Box::new(model)),
    );
}

/// How many of the next passes, up to `most`, run as the recorded one:
/// those whose operations set the flags read after them as it did. Also
/// the cells holding counts after them.
pub(super) fn counted_passes(cpu: &Cpu, model: &Model, most: u64) -> (u64, Vec<(Cell, u8)>) {
    let mut state = model.seeded(|c| value_now(cpu, c));
    let mut done = state.clone();
    let mut passes = 0;
    while passes < most {
        let same = model.run(&mut state, |i, _, _, flags| {
            let (live, seen) = model.checks[i];
            flags & live == seen
        });
        if !same {
            break;
        }
        passes += 1;
        done.clone_from(&state);
    }
    let values = done
        .iter()
        .zip(&model.cells)
        .filter_map(|(v, &c)| v.map(|v| (c, v)))
        .collect();
    (passes, values)
}

/// After a service the pass followed (`after_service`): it kept the
/// registers that hold counts, and read no RAM that does. A service that
/// read no RAM but its arguments (`reads_arguments`) names what it read
/// beyond them (`reads_also`); one that may have read any reads nothing
/// of the free stack below its caller's top.
pub(super) fn after_service(cpu: &mut Cpu) {
    // A count in EAX is gone where the function returned a value there,
    // and may be anything where it didn't.
    let eax = reg_cell(0)..reg_cell(1);
    let o = &mut cpu.bus.observe;
    let returned = o.service_returned;
    let Phase::Count { pass, .. } = &mut o.phase else {
        return;
    };
    if returned {
        pass.kill(eax.clone());
    }
    let o = &cpu.bus.observe;
    let Phase::Count { pass, .. } = &o.phase else {
        return;
    };
    let changed = pass.any(&eax)
        || (1..8).any(|i| {
            let cells = reg_cell(i)..reg_cell(i) + 4;
            pass.any(&cells) && cpu.reg(GPR32[i]) != o.service_regs[i]
        });
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
