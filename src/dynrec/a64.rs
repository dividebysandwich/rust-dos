//! The AArch64 code generator.
//!
//! Registers while translated code runs: X19 the CPU, X20 the context
//! (`JitCtx`), X21 RAM, X22 RAM's code generations, W23 set when a store hit
//! the block's later bytes, X27 the CPU plus `HI` (the CPU's fields past
//! X19's offsets' reach), W28 the guest's arithmetic flags where the code
//! has changed them (see `Gen::dirty`); W24-W26 hold the operations'
//! temporaries (`uop::T`), which calls keep, and X0-X17 are scratch (X18,
//! the platform's register, is never touched). The guest's registers stay
//! in the CPU. AArch64 has neither the parity nor the auxiliary carry flag,
//! so the code computes the guest's flags the way `cpu::alu` defines them,
//! with a parity table in the context, and only those that are live (see
//! `flags`).

// dynasm converts the registers it is given at run time with `into`, and
// checks the bit field operands it is given with comparisons that are
// constant for constant operands.
#![allow(clippy::useless_conversion, clippy::absurd_extreme_comparisons, clippy::eq_op)]

use dynasmrt::aarch64::Aarch64Relocation;
use dynasmrt::{DynamicLabel, DynasmApi, DynasmLabelApi, VecAssembler, dynasm};
use iced_x86::ConditionCode;

use super::block::{BlockData, LINKS, RETURN_LINK, RETURN_MISS};
use super::helpers::*;
use super::uop::*;
use crate::cpu::Seg;
use crate::cpu::alu::ShiftOp;
use crate::cpu::layout;

type Asm = VecAssembler<Aarch64Relocation>;

/// Flag bits.
const CF: u32 = 0x001;
const PF: u32 = 0x004;
const AF: u32 = 0x010;
const ZF: u32 = 0x040;
const SF: u32 = 0x080;
const OF: u32 = 0x800;
const ARITH: u32 = CF | PF | AF | ZF | SF | OF;
const SZP: u32 = SF | ZF | PF;

/// Where the RAM below the video memory ends, and extended memory starts
/// (both multiples of 4 KB, so they fit a compare's shifted immediate).
const VIDEO: u32 = 0xA0000;
const EXTENDED: u32 = 0x10_0000;

/// The CPU's fields X27 reaches: those from `HI` on.
const HI: usize = {
    let fields = [
        layout::ICOUNT,
        layout::DEADLINE,
        layout::EXECUTED,
        layout::EIP,
        layout::FLAGS,
        layout::CR0,
        layout::CPL,
        layout::A20_MASK,
    ];
    let mut min = usize::MAX;
    let mut i = 0;
    while i < fields.len() {
        if fields[i] < min {
            min = fields[i];
        }
        i += 1;
    }
    min & !0xF
};

const CPU: u8 = 19;
const HIB: u8 = 27;

/// The way in from Rust: `enter(cpu, ctx, code)` saves the registers Rust
/// expects kept, sets up the fixed ones and branches to `code`; translated
/// code leaves through `exit` with the exit code in W0 and its block in
/// X1, which it stores in the context with W28 (the flags, for
/// `EXIT_FLAGS`) before returning the code.
pub struct Trampoline {
    pub bytes: Vec<u8>,
    pub enter: usize,
    pub exit: usize,
}

pub type Enter = unsafe extern "C" fn(*mut crate::cpu::Cpu, *mut JitCtx, *const u8) -> u64;

pub fn trampoline() -> Trampoline {
    let mut ops = Asm::new(0);
    let enter = ops.offset().0;
    dynasm!(ops
        ; .arch aarch64
        ; stp x29, x30, [sp, -96]!
        ; mov x29, sp
        ; stp x19, x20, [sp, 16]
        ; stp x21, x22, [sp, 32]
        ; stp x23, x24, [sp, 48]
        ; stp x25, x26, [sp, 64]
        ; stp x27, x28, [sp, 80]
        ; mov x19, x0
        ; mov x20, x1
        ; ldr x21, [x20, CTX_RAM as u32]
        ; ldr x22, [x20, CTX_PAGE_GEN as u32]
    );
    mov32(&mut ops, 9, HI as u32);
    dynasm!(ops
        ; .arch aarch64
        ; add x27, x19, x9
        ; br x2
    );
    let exit = ops.offset().0;
    dynasm!(ops
        ; .arch aarch64
        ; str x1, [x20, CTX_EXIT_DATA as u32]
        ; str w28, [x20, CTX_FLAGS as u32]
        ; ldp x27, x28, [sp, 80]
        ; ldp x25, x26, [sp, 64]
        ; ldp x23, x24, [sp, 48]
        ; ldp x21, x22, [sp, 32]
        ; ldp x19, x20, [sp, 16]
        ; ldp x29, x30, [sp], 96
        ; ret
    );
    Trampoline { bytes: ops.finalize().expect("trampoline"), enter, exit }
}

/// W`reg` = `v`.
fn mov32(ops: &mut Asm, reg: u8, v: u32) {
    dynasm!(ops ; .arch aarch64 ; movz W(reg), v & 0xFFFF);
    if v >> 16 != 0 {
        dynasm!(ops ; .arch aarch64 ; movk W(reg), v >> 16, lsl 16);
    }
}

/// Host register of a temporary.
fn r(t: T) -> u8 {
    24 + t.0
}

fn gpr_offset(g: Gpr) -> usize {
    layout::GPR + g.index as usize * 4 + g.high as usize
}

fn seg_field(seg: Seg, field: usize) -> usize {
    layout::SEG + seg as usize * layout::SEG_SIZE + field
}

/// A load or store of a CPU field.
#[derive(Clone, Copy)]
enum Access {
    Ldr8,
    Ldr16,
    Ldr32,
    Ldr64,
    Str32,
    Str64,
}

/// Code emitted after the block's instructions, reached from them.
enum Slow {
    /// A memory operand the inline checks didn't take.
    MemRef { at: DynamicLabel, back: DynamicLabel, t: T, desc: u32, fail: DynamicLabel },
    Load { at: DynamicLabel, back: DynamicLabel, dst: T, m: T },
    Store { at: DynamicLabel, back: DynamicLabel, m: T, src: T, lo: u32, hi: u32 },
    /// An instruction's fault with an exit code of its own (#GP(0), #DE).
    Fault { at: DynamicLabel, code: u32, fail: DynamicLabel },
    /// Instruction `ix` runs through its handler after all (`Uop::Bail`),
    /// with the flags in W28 there (`dirty`), and goes on at `end`.
    Bail { at: DynamicLabel, end: DynamicLabel, ix: usize, dirty: bool },
}

struct Gen<'a> {
    ops: Asm,
    data: &'a BlockData,
    /// Where the block's data pointer is, in the literal after the code.
    data_lit: DynamicLabel,
    /// Per instruction: where it stops the block with the exit code in W0
    /// (made where something jumps there), and how far the instruction
    /// count is brought up to date for it. The stops' way out.
    fail: Vec<Option<DynamicLabel>>,
    synced: Vec<i32>,
    fail_tail: DynamicLabel,
    /// Where the instruction being translated ends (made where something
    /// branches there), and per instruction whether the flags are in W28
    /// there.
    end: Option<DynamicLabel>,
    end_dirty: Vec<bool>,
    tail: DynamicLabel,
    /// The prologue's ways out, and where it goes on.
    deadline: DynamicLabel,
    revalidate: DynamicLabel,
    limit: DynamicLabel,
    body: DynamicLabel,
    slow: Vec<Slow>,
    /// Whether exits to a known EIP in the page may be linked, and the
    /// stubs of the links used.
    link: bool,
    stubs: [Option<DynamicLabel>; LINKS],
    /// The way out of a return to none of the places its links lead to.
    return_miss: Option<DynamicLabel>,
    /// The instruction being translated.
    ix: usize,
    /// The flags live after each operation, and after the one being
    /// translated; whether the guest's arithmetic flags are in W28, and
    /// were where each instruction may stop (see `x64::Gen`).
    live: Vec<Vec<u32>>,
    live_after: u32,
    dirty: bool,
    dirty_at: Vec<bool>,
}

/// A translated block's code, where its links' stubs are in it, and how
/// far behind each instruction the instruction count is (`BlockData::lag`).
pub struct Code {
    pub bytes: Vec<u8>,
    pub stubs: [Option<usize>; LINKS],
    pub lag: Box<[u8]>,
}

/// Translate a block: each instruction's operations (`items[ix]`), or a
/// call of its interpreter handler (None). With `link`, exits to a known
/// EIP in the block's page go through its links. See `x64::block`, which
/// this follows.
pub fn block(data: &BlockData, items: &[Option<Vec<Uop>>], link: bool) -> Code {
    let mut ops = Asm::new(0);
    let n = data.count();
    let mut labels = || ops.new_dynamic_label();
    let (data_lit, tail, deadline, revalidate, limit, body) = (labels(), labels(), labels(), labels(), labels(), labels());
    let fail_tail = labels();
    let mut g = Gen {
        ops,
        data,
        data_lit,
        fail: vec![None; n],
        synced: vec![0; n],
        fail_tail,
        end: None,
        end_dirty: vec![false; n],
        tail,
        deadline,
        revalidate,
        limit,
        body,
        slow: Vec::new(),
        link,
        stubs: [None; LINKS],
        return_miss: None,
        ix: 0,
        live: super::flags::live(items),
        live_after: 0,
        dirty: false,
        dirty_at: vec![false; n],
    };
    g.prologue(items);
    let mut synced = 0;
    for (ix, item) in items.iter().enumerate() {
        g.ix = ix;
        match item {
            None => {
                if ix as i32 > synced {
                    g.add_field64(layout::ICOUNT, (ix as i32 - synced) as u32);
                    synced = ix as i32;
                }
                g.synced[ix] = synced;
                g.flags_back();
                g.dirty = false;
                g.check_watched();
                g.fallback(ix as u32);
            }
            Some(uops) => {
                g.synced[ix] = synced;
                g.dirty_at[ix] = g.dirty;
                g.check_watched();
                for (k, uop) in uops.iter().enumerate() {
                    g.live_after = g.live[ix][k];
                    g.uop(uop);
                }
                g.end_dirty[ix] = g.dirty;
                if let Some(end) = g.end.take() {
                    dynasm!(g.ops ; .arch aarch64 ; =>end);
                }
                if uops.iter().any(|u| matches!(u, Uop::Store { .. })) {
                    // A store hit the rest of the block: leave after this
                    // instruction.
                    let next = data.eips[ix].wrapping_add(data.instrs[ix].len() as u32);
                    let (fail, skip) = (g.fail(), g.ops.new_dynamic_label());
                    dynasm!(g.ops ; .arch aarch64 ; cbz w23, =>skip);
                    g.mov32(0, next);
                    g.field(Access::Str32, 0, layout::EIP);
                    g.mov32(0, EXIT_SMC | if g.dirty { EXIT_FLAGS } else { 0 });
                    dynasm!(g.ops
                        ; .arch aarch64
                        ; b =>fail
                        ; =>skip
                    );
                }
            }
        }
    }
    // A block that ran out of length goes on at the next instruction (a
    // handler sets EIP itself).
    let last = n - 1;
    let next = data.eips[last].wrapping_add(data.instrs[last].len() as u32);
    match &items[last] {
        Some(u) if !u.iter().any(|u| matches!(u, Uop::Exit { .. } | Uop::ExitIf { .. })) => g.leave(Some(next), 0, true),
        None if !super::block::ends_block(&data.instrs[last]) => g.leave(Some(next), 0, false),
        _ => {}
    }
    // The end: the counts, and back to the execution loop. The code that
    // branches here has put the flags back.
    dynasm!(g.ops ; .arch aarch64 ; =>tail);
    g.dirty = false;
    g.leave(None, 0, false);
    g.epilogue();
    let data_ptr = data as *const BlockData as u64;
    dynasm!(g.ops
        ; .arch aarch64
        ; .align 8
        ; =>data_lit
        ; .u64 data_ptr
    );
    let stubs = g.stubs.map(|s| s.map(|l| g.ops.labels().resolve_dynamic(l).expect("stub").0));
    let lag = g.synced.iter().enumerate().map(|(ix, &synced)| (ix as i32 - synced) as u8).collect();
    Code { bytes: g.ops.finalize().expect("block"), stubs, lag }
}

impl Gen<'_> {
    fn mov32(&mut self, reg: u8, v: u32) {
        mov32(&mut self.ops, reg, v);
    }

    /// Load or store W/X`reg` from or to the CPU field at `off`: through
    /// X19 or X27 where an offset reaches it, else X9 holds the offset.
    fn field(&mut self, access: Access, reg: u8, off: usize) {
        let (base, rel) = if off < 4096 {
            (CPU, off as u32)
        } else if off >= HI && off - HI < 4096 {
            (HIB, (off - HI) as u32)
        } else {
            self.mov32(9, off as u32);
            match access {
                Access::Ldr8 => dynasm!(self.ops ; .arch aarch64 ; ldrb W(reg), [x19, x9]),
                Access::Ldr16 => dynasm!(self.ops ; .arch aarch64 ; ldrh W(reg), [x19, x9]),
                Access::Ldr32 => dynasm!(self.ops ; .arch aarch64 ; ldr W(reg), [x19, x9]),
                Access::Ldr64 => dynasm!(self.ops ; .arch aarch64 ; ldr X(reg), [x19, x9]),
                Access::Str32 => dynasm!(self.ops ; .arch aarch64 ; str W(reg), [x19, x9]),
                Access::Str64 => dynasm!(self.ops ; .arch aarch64 ; str X(reg), [x19, x9]),
            }
            return;
        };
        match access {
            Access::Ldr8 => dynasm!(self.ops ; .arch aarch64 ; ldrb W(reg), [X(base), rel]),
            Access::Ldr16 => dynasm!(self.ops ; .arch aarch64 ; ldrh W(reg), [X(base), rel]),
            Access::Ldr32 => dynasm!(self.ops ; .arch aarch64 ; ldr W(reg), [X(base), rel]),
            Access::Ldr64 => dynasm!(self.ops ; .arch aarch64 ; ldr X(reg), [X(base), rel]),
            Access::Str32 => dynasm!(self.ops ; .arch aarch64 ; str W(reg), [X(base), rel]),
            Access::Str64 => dynasm!(self.ops ; .arch aarch64 ; str X(reg), [X(base), rel]),
        }
    }

    /// Add `v` to the u64 CPU field at `off`.
    fn add_field64(&mut self, off: usize, v: u32) {
        if v == 0 {
            return;
        }
        self.field(Access::Ldr64, 0, off);
        if v < 4096 {
            dynasm!(self.ops ; .arch aarch64 ; add x0, x0, v);
        } else {
            self.mov32(9, v);
            dynasm!(self.ops ; .arch aarch64 ; add x0, x0, x9);
        }
        self.field(Access::Str64, 0, off);
    }

    /// Where the instruction being translated stops the block, with the
    /// exit code in W0.
    fn fail(&mut self) -> DynamicLabel {
        let ops = &mut self.ops;
        *self.fail[self.ix].get_or_insert_with(|| ops.new_dynamic_label())
    }

    /// Where the instruction being translated ends.
    fn end(&mut self) -> DynamicLabel {
        let ops = &mut self.ops;
        *self.end.get_or_insert_with(|| ops.new_dynamic_label())
    }

    /// X1 = the block's data.
    fn data_x1(&mut self) {
        let lit = self.data_lit;
        dynasm!(self.ops ; .arch aarch64 ; ldr x1, =>lit);
    }

    /// Back to the execution loop with the exit code in W0 and the block in
    /// X1.
    fn exit(&mut self) {
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x16, [x20, CTX_EXIT as u32]
            ; br x16
        );
    }

    fn prologue(&mut self, items: &[Option<Vec<Uop>>]) {
        let data = self.data;
        let n = data.count() as u32;
        let (deadline, revalidate, limit, body) = (self.deadline, self.revalidate, self.limit, self.body);
        self.field(Access::Ldr64, 0, layout::ICOUNT);
        self.field(Access::Ldr64, 1, layout::DEADLINE);
        dynasm!(self.ops
            ; .arch aarch64
            ; add x0, x0, n
            ; cmp x0, x1
            ; b.hi =>deadline
        );
        self.field(Access::Ldr32, 0, seg_field(Seg::CS, layout::SEG_LIMIT));
        self.mov32(1, data.limit_need);
        dynasm!(self.ops
            ; .arch aarch64
            ; cmp w0, w1
            ; b.lo =>limit
        );
        self.mov32(2, data.chunk_first * 4);
        dynasm!(self.ops ; .arch aarch64 ; ldr w0, [x22, x2]);
        for _ in data.chunk_first + 1..=data.chunk_last {
            dynasm!(self.ops
                ; .arch aarch64
                ; add x2, x2, 4
                ; ldr w3, [x22, x2]
                ; add w0, w0, w3
            );
        }
        self.data_x1();
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr w3, [x1, DATA_GEN_SUM as u32]
            ; cmp w0, w3
            ; b.ne =>revalidate
            ; =>body
        );
        if items.iter().flatten().flatten().any(|u| matches!(u, Uop::Store { .. })) {
            dynasm!(self.ops ; .arch aarch64 ; movz w23, 0);
        }
    }

    /// Run instruction `ix` through its handler.
    fn fallback(&mut self, ix: u32) {
        let fail = self.fail();
        let lit = self.data_lit;
        dynasm!(self.ops
            ; .arch aarch64
            ; mov x0, x19
            ; mov x1, x20
            ; ldr x2, =>lit
        );
        self.mov32(3, ix);
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x16, [x20, CTX_FALLBACK as u32]
            ; blr x16
            ; cbnz w0, =>fail
        );
    }

    /// The stubs that leave the block from an instruction, and the slow
    /// paths.
    fn epilogue(&mut self) {
        let (deadline, revalidate, limit, body, lit) =
            (self.deadline, self.revalidate, self.limit, self.body, self.data_lit);
        // The prologue's ways out: nothing ran.
        dynasm!(self.ops ; .arch aarch64 ; =>deadline ; movz w0, EXIT_DEADLINE);
        self.data_x1();
        self.exit();
        dynasm!(self.ops ; .arch aarch64 ; =>limit ; movz w0, EXIT_LIMIT);
        self.data_x1();
        self.exit();
        dynasm!(self.ops
            ; .arch aarch64
            ; =>revalidate
            ; mov x0, x19
            ; mov x1, x20
            ; ldr x2, =>lit
            ; ldr x16, [x20, CTX_REVALIDATE as u32]
            ; blr x16
            ; cbz w0, =>body
            ; movz w0, EXIT_STALE
        );
        self.data_x1();
        self.exit();
        // The links' stubs: back to the execution loop to be linked (X1 is
        // the block, from `leave`).
        let misses = self.return_miss.map(|miss| (RETURN_MISS, miss));
        for (k, stub) in self.stubs.into_iter().enumerate().filter_map(|(k, s)| Some((k, s?))).chain(misses) {
            dynasm!(self.ops ; .arch aarch64 ; =>stub);
            mov32(&mut self.ops, 0, EXIT_UNLINKED | (k as u32) << 8);
            self.exit();
        }
        // An instruction stopped the block: the exit code gets its index,
        // and whether the flags are in W28, and the execution loop counts
        // it (see `BlockData::lag`).
        let fail_tail = self.fail_tail;
        for (ix, label) in self.fail.clone().into_iter().enumerate() {
            if let Some(label) = label {
                dynasm!(self.ops ; .arch aarch64 ; =>label);
                let bits = (ix as u32) << 8 | if self.dirty_at[ix] { EXIT_FLAGS } else { 0 };
                if bits != 0 {
                    self.mov32(9, bits);
                    dynasm!(self.ops ; .arch aarch64 ; orr w0, w0, w9);
                }
                dynasm!(self.ops ; .arch aarch64 ; b =>fail_tail);
            }
        }
        if self.fail.iter().any(Option::is_some) || self.slow.iter().any(|s| matches!(s, Slow::Bail { .. })) {
            dynasm!(self.ops ; .arch aarch64 ; =>fail_tail);
            self.data_x1();
            self.exit();
        }
        for slow in std::mem::take(&mut self.slow) {
            match slow {
                Slow::MemRef { at, back, t, desc, fail } => {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; =>at
                        ; mov x0, x19
                        ; mov x1, x20
                        ; mov w2, W(r(t))
                    );
                    self.mov32(3, desc);
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; ldr x16, [x20, CTX_MEMREF as u32]
                        ; blr x16
                        ; tbnz x0, 63, >fault
                        ; mov W(r(t)), w0
                        ; b =>back
                        ; fault:
                        ; movz w0, EXIT_FAULT
                        ; b =>fail
                    );
                }
                Slow::Load { at, back, dst, m } => {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; =>at
                        ; mov x0, x19
                        ; mov x1, x20
                        ; and w2, W(r(m)), 3
                        ; ldr x16, [x20, CTX_READ as u32]
                        ; blr x16
                        ; mov W(r(dst)), w0
                        ; b =>back
                    );
                }
                Slow::Fault { at, code, fail } => {
                    dynasm!(self.ops ; .arch aarch64 ; =>at ; movz w0, code ; b =>fail);
                }
                Slow::Bail { at, end, ix, dirty } => {
                    // As a handler's call in the block, but with the
                    // instruction count put back after it, as the code on
                    // from `end` has it. A stop leaves the flags the handler
                    // left in the CPU.
                    let lag = (ix as i32 - self.synced[ix]) as u32;
                    dynasm!(self.ops ; .arch aarch64 ; =>at);
                    self.dirty = dirty;
                    self.flags_back();
                    self.add_field64(layout::ICOUNT, lag);
                    let lit = self.data_lit;
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; mov x0, x19
                        ; mov x1, x20
                        ; ldr x2, =>lit
                    );
                    self.mov32(3, ix as u32);
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; ldr x16, [x20, CTX_FALLBACK as u32]
                        ; blr x16
                        ; mov w8, w0
                    );
                    if lag > 0 {
                        self.field(Access::Ldr64, 0, layout::ICOUNT);
                        dynasm!(self.ops ; .arch aarch64 ; sub x0, x0, lag);
                        self.field(Access::Str64, 0, layout::ICOUNT);
                    }
                    dynasm!(self.ops ; .arch aarch64 ; cbnz w8, >stop);
                    if self.end_dirty[ix] {
                        self.field(Access::Ldr32, 28, layout::FLAGS);
                    }
                    let fail_tail = self.fail_tail;
                    dynasm!(self.ops ; .arch aarch64 ; b =>end ; stop:);
                    self.mov32(9, (ix as u32) << 8);
                    dynasm!(self.ops ; .arch aarch64 ; orr w0, w8, w9 ; b =>fail_tail);
                }
                Slow::Store { at, back, m, src, lo, hi } => {
                    dynasm!(self.ops ; .arch aarch64 ; =>at);
                    self.mov32(9, lo);
                    dynasm!(self.ops ; .arch aarch64 ; str w9, [x20, CTX_SMC_LO as u32]);
                    self.mov32(9, hi);
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; str w9, [x20, CTX_SMC_HI as u32]
                        ; mov x0, x19
                        ; mov x1, x20
                        ; and w2, W(r(m)), 3
                        ; mov w3, W(r(src))
                        ; ldr x16, [x20, CTX_WRITE as u32]
                        ; blr x16
                        ; orr w23, w23, w0
                        ; b =>back
                    );
                }
            }
        }
    }

    /// Leave the block before the instruction being translated if any of
    /// its watched bytes differ from what was translated.
    fn check_watched(&mut self) {
        let data = self.data;
        let mut changed = None;
        for w in data.watched_in(self.ix) {
            let at = *changed.get_or_insert_with(|| self.fault_exit(EXIT_WATCHED));
            self.mov32(0, data.phys + w as u32);
            dynasm!(self.ops
                ; .arch aarch64
                ; ldrb w0, [x21, x0]
                ; cmp w0, data.bytes[w] as u32
                ; b.ne =>at
            );
        }
    }

    /// Where the instruction being translated raises a fault that has an
    /// exit code of its own (`EXIT_GP0`, `EXIT_DE`), or leaves the block
    /// before it runs (`EXIT_WATCHED`).
    fn fault_exit(&mut self, code: u32) -> DynamicLabel {
        let at = self.ops.new_dynamic_label();
        let fail = self.fail();
        self.slow.push(Slow::Fault { at, code, fail });
        at
    }

    /// The physical addresses of the block's bytes after the current
    /// instruction, which a store must not change unseen.
    fn rest(&self) -> (u32, u32) {
        let data = self.data;
        if self.ix + 1 >= data.count() {
            return (0, 0);
        }
        (data.phys + data.offset(self.ix + 1) as u32, data.phys + data.len)
    }

    fn uop(&mut self, uop: &Uop) {
        match *uop {
            Uop::Get { t, r: g } => {
                let access = match g.size {
                    4 => Access::Ldr32,
                    2 => Access::Ldr16,
                    _ => Access::Ldr8,
                };
                self.field(access, r(t), gpr_offset(g));
            }
            Uop::Set { r: g, t } => self.set_gpr(g, r(t)),
            Uop::Const { t, v } => self.mov32(r(t), v),
            Uop::Copy { dst, src } => dynasm!(self.ops ; .arch aarch64 ; mov W(r(dst)), W(r(src))),
            Uop::AddConst { t, v, size } => {
                let t = r(t);
                let (plus, minus) = (v, v.wrapping_neg());
                if plus < 4096 {
                    dynasm!(self.ops ; .arch aarch64 ; add WSP(t), WSP(t), plus);
                } else if minus < 4096 {
                    dynasm!(self.ops ; .arch aarch64 ; sub WSP(t), WSP(t), minus);
                } else {
                    self.mov32(9, v);
                    dynasm!(self.ops ; .arch aarch64 ; add W(t), W(t), w9);
                }
                if size == 2 {
                    dynasm!(self.ops ; .arch aarch64 ; uxth W(t), W(t));
                }
            }
            Uop::SarConst { t, count } => dynasm!(self.ops ; .arch aarch64 ; asr W(r(t)), W(r(t)), count as u32),
            Uop::Extend { t, from, signed } => {
                let t = r(t);
                match (from, signed) {
                    (1, false) => dynasm!(self.ops ; .arch aarch64 ; uxtb W(t), W(t)),
                    (1, true) => dynasm!(self.ops ; .arch aarch64 ; sxtb W(t), W(t)),
                    (_, false) => dynasm!(self.ops ; .arch aarch64 ; uxth W(t), W(t)),
                    (_, true) => dynasm!(self.ops ; .arch aarch64 ; sxth W(t), W(t)),
                }
            }
            Uop::Ea { t, base, index, scale, disp, a32 } => {
                let t = r(t);
                self.mov32(t, disp);
                if let Some(b) = base {
                    self.load_w9(b);
                    dynasm!(self.ops ; .arch aarch64 ; add W(t), W(t), w9);
                }
                if let Some(i) = index {
                    self.load_w9(i);
                    let shift = scale.trailing_zeros();
                    dynasm!(self.ops ; .arch aarch64 ; add W(t), W(t), w9, lsl shift);
                }
                if !a32 {
                    dynasm!(self.ops ; .arch aarch64 ; uxth W(t), W(t));
                }
            }
            Uop::MemRef { t, seg, size, write, slot } => self.memref(t, seg, size, write, slot),
            Uop::Load { dst, m, size } => {
                let (at, back) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
                let slow = SLOW.wrapping_neg();
                let (d, m_) = (r(dst), r(m));
                // A handle at or above SLOW (-256) isn't an address.
                dynasm!(self.ops ; .arch aarch64 ; cmn WSP(m_), slow ; b.hs =>at);
                match size {
                    1 => dynasm!(self.ops ; .arch aarch64 ; ldrb W(d), [x21, X(m_)]),
                    2 => dynasm!(self.ops ; .arch aarch64 ; ldrh W(d), [x21, X(m_)]),
                    _ => dynasm!(self.ops ; .arch aarch64 ; ldr W(d), [x21, X(m_)]),
                }
                dynasm!(self.ops ; .arch aarch64 ; =>back);
                self.slow.push(Slow::Load { at, back, dst, m });
            }
            Uop::Store { m, src, size } => {
                let (at, back) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
                let slow = SLOW.wrapping_neg();
                let (last, width) = (size as u32 - 1, size as u32);
                let (m_, s) = (r(m), r(src));
                dynasm!(self.ops ; .arch aarch64 ; cmn WSP(m_), slow ; b.hs =>at);
                match size {
                    1 => dynasm!(self.ops ; .arch aarch64 ; strb W(s), [x21, X(m_)]),
                    2 => dynasm!(self.ops ; .arch aarch64 ; strh W(s), [x21, X(m_)]),
                    _ => dynasm!(self.ops ; .arch aarch64 ; str W(s), [x21, X(m_)]),
                }
                // The code generations of the first and last byte's chunks,
                // as the bus's writes bump them.
                let shift = crate::bus::GEN_SHIFT as u32;
                dynasm!(self.ops
                    ; .arch aarch64
                    ; lsr w1, W(m_), shift
                    ; ldr w2, [x22, x1, lsl 2]
                    ; add w2, w2, 1
                    ; str w2, [x22, x1, lsl 2]
                );
                if size > 1 {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; add w1, WSP(m_), last
                        ; lsr w1, w1, shift
                        ; ldr w2, [x22, x1, lsl 2]
                        ; add w2, w2, 1
                        ; str w2, [x22, x1, lsl 2]
                    );
                }
                let (lo, hi) = self.rest();
                if lo < hi {
                    self.mov32(3, hi);
                    dynasm!(self.ops ; .arch aarch64 ; cmp W(m_), w3 ; b.hs =>back);
                    self.mov32(3, lo);
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; add w1, WSP(m_), width
                        ; cmp w1, w3
                        ; b.ls =>back
                        ; movz w23, 1
                    );
                }
                dynasm!(self.ops ; .arch aarch64 ; =>back);
                self.slow.push(Slow::Store { at, back, m, src, lo, hi });
            }
            Uop::Alu { op, size, a, b } => self.alu(op, size, a, b),
            Uop::Unary { op, size, t } => self.unary(op, size, t),
            Uop::Shift { op, size, t, count } => self.shift(op, size, t, Some(count)),
            Uop::DoubleShift { left, size, dst, src, count } => self.double_shift(left, size, dst, src, Some(count)),
            Uop::ShiftVar { op, size, t, count } => {
                self.var_count(count, super::flags::shift_flags(op));
                self.shift(op, size, t, None);
            }
            Uop::DoubleShiftVar { left, size, dst, src, count } => {
                self.var_count(count, CF | OF | SZP);
                self.double_shift(left, size, dst, src, None);
            }
            Uop::Bail { t, mask } => {
                let at = self.ops.new_dynamic_label();
                dynasm!(self.ops ; .arch aarch64 ; tst W(r(t)), mask ; b.ne =>at);
                let end = self.end();
                self.slow.push(Slow::Bail { at, end, ix: self.ix, dirty: self.dirty });
            }
            Uop::Imul { size, a, b } => self.imul(size, a, b),
            Uop::MulWide { signed, size, t } => self.mul_wide(signed, size, t),
            Uop::DivWide { signed, size, t } => self.div_wide(signed, size, t),
            Uop::SetCond { t, cc } => {
                if self.test_condition(cc, self.dirty) {
                    dynasm!(self.ops ; .arch aarch64 ; cset W(r(t)), ne);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; cset W(r(t)), eq);
                }
            }
            Uop::Flag { mask, set } => {
                // DF is always in the CPU, the arithmetic flags in W28 once
                // the code has changed them.
                let (arith, other) = (mask & ARITH, mask & !ARITH);
                if other != 0 {
                    self.flag_op(other, set, false);
                }
                if arith != 0 && self.live_after & arith != 0 {
                    self.flag_op(arith, set, self.dirty);
                }
            }
            Uop::CheckLimit { src } => {
                self.value_w0(src);
                let gp = self.fault_exit(EXIT_GP0);
                self.field(Access::Ldr32, 1, seg_field(Seg::CS, layout::SEG_LIMIT));
                dynasm!(self.ops ; .arch aarch64 ; cmp w0, w1 ; b.hi =>gp);
            }
            Uop::Exit { eip: Src::Imm(target) } => self.leave(Some(target), 0, true),
            Uop::Exit { eip: Src::T(t) } => {
                self.field(Access::Str32, r(t), layout::EIP);
                self.flags_back();
                if self.link {
                    // A return: through its link to where it goes, if it
                    // has one.
                    self.counts();
                    self.returned(t);
                } else {
                    let tail = self.tail;
                    dynasm!(self.ops ; .arch aarch64 ; b =>tail);
                }
            }
            Uop::ExitIf { cond, taken, next, commit } => self.exit_if(cond, taken, next, commit),
        }
    }

    /// The register = the low bytes of W`reg`. The register's whole word
    /// is written, so that later loads of it are forwarded from one store
    /// (see `x64::Gen::set_ecx`). W7 is changed.
    fn set_gpr(&mut self, g: Gpr, reg: u8) {
        let off = gpr_offset(Gpr::dword(g.index));
        if g.size == 4 {
            self.field(Access::Str32, reg, off);
            return;
        }
        let (lsb, width) = (g.high as u32 * 8, g.size as u32 * 8);
        self.field(Access::Ldr32, 7, off);
        dynasm!(self.ops ; .arch aarch64 ; bfi w7, W(reg), lsb, width);
        self.field(Access::Str32, 7, off);
    }

    /// W9 = the register, zero-extended.
    fn load_w9(&mut self, g: Gpr) {
        let access = match g.size {
            4 => Access::Ldr32,
            2 => Access::Ldr16,
            _ => Access::Ldr8,
        };
        self.field(access, 9, gpr_offset(g));
    }

    fn value_w0(&mut self, src: Src) {
        match src {
            Src::T(t) => dynasm!(self.ops ; .arch aarch64 ; mov w0, W(r(t))),
            Src::Imm(v) => self.mov32(0, v),
        }
    }

    /// Check the operand at seg:t as `Cpu::mem_ref` does, inline for plain
    /// RAM in one page (through the TLB with paging on), and leave its
    /// handle in t.
    fn memref(&mut self, t: T, seg: Seg, size: u8, write: bool, slot: u8) {
        let (at, back) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        let t_ = r(t);
        let need = if write { layout::RIGHT_WRITE } else { layout::RIGHT_READ } as u32;
        let tag = (if write { layout::TLB_WRITE_TAG } else { layout::TLB_READ_TAG }) as u32;
        let size32 = size as u32;
        let last = size32 - 1;
        // The segment's limit and type, as `seg_linear` checks them.
        dynasm!(self.ops
            ; .arch aarch64
            ; add w1, WSP(t_), last
            ; cmp w1, W(t_)
            ; b.lo =>at
        );
        self.field(Access::Ldr32, 2, seg_field(seg, layout::SEG_LO));
        dynasm!(self.ops ; .arch aarch64 ; cmp W(t_), w2 ; b.lo =>at);
        self.field(Access::Ldr32, 2, seg_field(seg, layout::SEG_HI));
        dynasm!(self.ops ; .arch aarch64 ; cmp w1, w2 ; b.hi =>at);
        self.field(Access::Ldr8, 2, seg_field(seg, layout::SEG_RIGHTS));
        dynasm!(self.ops ; .arch aarch64 ; tst w2, need ; b.eq =>at);
        self.field(Access::Ldr32, 2, seg_field(seg, layout::SEG_BASE));
        dynasm!(self.ops ; .arch aarch64 ; add w0, W(t_), w2);
        self.field(Access::Ldr32, 2, layout::CR0);
        dynasm!(self.ops ; .arch aarch64 ; tbnz w2, 31, >paging);
        self.field(Access::Ldr32, 2, layout::A20_MASK);
        dynasm!(self.ops
            ; .arch aarch64
            ; and w0, w0, w2
            ; b >physical
            // The entry of the page (linear address >> 12) in the set of
            // the privilege level: its tag must be the page + 1.
            ; paging:
            ; lsr w1, w0, 12
            ; and w3, w1, (layout::TLB_SET - 1) as u32
        );
        self.field(Access::Ldr8, 2, layout::CPL);
        dynasm!(self.ops
            ; .arch aarch64
            ; cmp w2, 3
            ; b.ne >supervisor
            ; add w3, w3, layout::TLB_SET as u32
            ; supervisor:
        );
        self.mov32(4, layout::TLB_ENTRY_SIZE as u32);
        dynasm!(self.ops
            ; .arch aarch64
            ; umull x3, w3, w4
            ; ldr x6, [x20, CTX_TLB as u32]
            ; add x6, x6, x3
            ; ldr w5, [x6, tag]
            ; add w1, w1, 1
            ; cmp w5, w1
            ; b.ne =>at
            ; ldr w5, [x6, layout::TLB_PHYS as u32]
            ; and w0, w0, 0xFFF
            ; orr w0, w0, w5
        );
        self.field(Access::Ldr32, 2, layout::A20_MASK);
        dynasm!(self.ops
            ; .arch aarch64
            ; and w0, w0, w2
            // Within a page, in plain RAM.
            ; physical:
            ; and w1, w0, 0xFFF
            ; cmp w1, 0x1000 - size32
            ; b.hi =>at
            ; add x1, x0, size32
            ; cmp x1, VIDEO >> 12, lsl 12
            ; b.ls >ram
            ; cmp w0, EXTENDED >> 12, lsl 12
            ; b.lo =>at
            ; ldr x2, [x20, CTX_RAM_LEN as u32]
            ; cmp x1, x2
            ; b.hi =>at
            ; ram:
            ; mov W(t_), w0
            ; =>back
        );
        let desc = memref_desc(seg, size, write, slot);
        let fail = self.fail();
        self.slow.push(Slow::MemRef { at, back, t, desc, fail });
    }

    /// Set (Some(true)), clear or complement the flags in `mask`, in W28
    /// or in the CPU.
    fn flag_op(&mut self, mask: u32, set: Option<bool>, in_w28: bool) {
        let reg = if in_w28 { 28 } else { 0 };
        if !in_w28 {
            self.field(Access::Ldr32, 0, layout::FLAGS);
        }
        self.mov32(1, mask);
        match set {
            Some(true) => dynasm!(self.ops ; .arch aarch64 ; orr W(reg), W(reg), w1),
            Some(false) => dynasm!(self.ops ; .arch aarch64 ; bic W(reg), W(reg), w1),
            None => dynasm!(self.ops ; .arch aarch64 ; eor W(reg), W(reg), w1),
        }
        if !in_w28 {
            self.field(Access::Str32, 0, layout::FLAGS);
        }
    }

    /// Put the guest's arithmetic flags from W28 into the CPU, if they are
    /// there. W6, W8 and W9 are changed.
    fn flags_back(&mut self) {
        if self.dirty {
            self.field(Access::Ldr32, 8, layout::FLAGS);
            self.mov32(9, ARITH);
            dynasm!(self.ops
                ; .arch aarch64
                ; bic w8, w8, w9
                ; and w6, w28, w9
                ; orr w8, w8, w6
            );
            self.field(Access::Str32, 8, layout::FLAGS);
        }
    }

    /// Put the flags `need` from W5 (whose other bits are 0 or dead) into
    /// the guest's flags in W28. The others stay, from the CPU if they
    /// aren't in W28 yet and are live.
    fn merge(&mut self, need: u32) {
        if ARITH & !need & self.live_after == 0 {
            dynasm!(self.ops ; .arch aarch64 ; mov w28, w5);
        } else {
            if !self.dirty {
                self.field(Access::Ldr32, 28, layout::FLAGS);
            }
            self.mov32(9, need);
            dynasm!(self.ops
                ; .arch aarch64
                ; bic w28, w28, w9
                ; and w5, w5, w9
                ; orr w28, w28, w5
            );
        }
        self.dirty = true;
    }

    /// W5 = the flags `need`: those of `regs` (CF, OF, AF) from W1, W2 and
    /// W3, which hold 0 or 1, those of `ones` set, SF, ZF and PF from the
    /// result in W0 (`bits` wide), and the rest 0.
    ///
    /// The bitfield instructions' `lsb` operands are single names
    /// throughout: dynasm pastes a run-time `lsb` into its range check
    /// (`31 - lsb`) as it is, so `bits - 1` there would check `31 - bits -
    /// 1`, which underflows at 32 bits (a panic in debug builds).
    fn flags_w5(&mut self, bits: u32, need: u32, regs: u32, ones: u32) {
        if need == 0 {
            return;
        }
        let sign = bits - 1;
        let mut first = true;
        for (flag, reg, at) in [(CF, 1, 0), (AF, 3, 4), (OF, 2, 11)] {
            if need & regs & flag != 0 {
                self.put_w5(&mut first, reg, at);
            }
        }
        if need & PF != 0 {
            dynasm!(self.ops
                ; .arch aarch64
                ; and w6, w0, 0xFF
                ; add x6, x20, x6
                ; ldrb w6, [x6, CTX_PARITY as u32]
            );
            self.put_w5(&mut first, 6, 0);
        }
        if need & ZF != 0 {
            dynasm!(self.ops ; .arch aarch64 ; cmp w0, 0 ; cset w6, eq);
            self.put_w5(&mut first, 6, 6);
        }
        if need & SF != 0 {
            dynasm!(self.ops ; .arch aarch64 ; ubfx w6, w0, sign, 1);
            self.put_w5(&mut first, 6, 7);
        }
        let ones = need & ones;
        if first {
            self.mov32(5, ones);
        } else if ones != 0 {
            self.mov32(6, ones);
            dynasm!(self.ops ; .arch aarch64 ; orr w5, w5, w6);
        }
    }

    /// W5 = W`reg` << `at`, or W5 |= that after the first.
    fn put_w5(&mut self, first: &mut bool, reg: u8, at: u32) {
        if *first {
            dynasm!(self.ops ; .arch aarch64 ; lsl w5, W(reg), at);
        } else {
            dynasm!(self.ops ; .arch aarch64 ; orr w5, w5, W(reg), lsl at);
        }
        *first = false;
    }

    /// W0 = W0 & the size's mask.
    fn cut(&mut self, size: u8) {
        match size {
            1 => dynasm!(self.ops ; .arch aarch64 ; and w0, w0, 0xFF),
            2 => dynasm!(self.ops ; .arch aarch64 ; and w0, w0, 0xFFFF),
            _ => {}
        }
    }

    /// An addition (`sub` false) or subtraction of W11 from W10 whose full
    /// result is in X0 (a 64-bit sum or difference of the zero-extended
    /// operands): W0 becomes the result, and W5 its flags `need`.
    fn arith_flags(&mut self, size: u8, sub: bool, need: u32) {
        let bits = size as u32 * 8;
        let sign = bits - 1;
        if need & CF != 0 {
            if sub {
                // A borrow makes the 64-bit difference negative.
                dynasm!(self.ops ; .arch aarch64 ; lsr x1, x0, 63);
            } else {
                dynasm!(self.ops ; .arch aarch64 ; lsr x1, x0, bits);
            }
        }
        self.cut(size);
        if need & OF != 0 {
            if sub {
                // OF: the operands' signs differ and the result's is the
                // subtrahend's.
                dynasm!(self.ops ; .arch aarch64 ; eor w2, w10, w11 ; eor w3, w10, w0 ; and w2, w2, w3);
            } else {
                // OF: the operands' signs agree and the result's doesn't.
                dynasm!(self.ops ; .arch aarch64 ; eor w2, w10, w0 ; eor w3, w11, w0 ; and w2, w2, w3);
            }
            dynasm!(self.ops ; .arch aarch64 ; ubfx w2, w2, sign, 1);
        }
        if need & AF != 0 {
            dynasm!(self.ops
                ; .arch aarch64
                ; eor w3, w10, w11
                ; eor w3, w3, w0
                ; ubfx w3, w3, 4, 1
            );
        }
        if need != 0 {
            self.flags_w5(bits, need, CF | OF | AF, 0);
            self.merge(need);
        }
    }

    /// W10 = a, W11 = b.
    fn operands(&mut self, a: T, b: Src) {
        dynasm!(self.ops ; .arch aarch64 ; mov w10, W(r(a)));
        match b {
            Src::T(b) => dynasm!(self.ops ; .arch aarch64 ; mov w11, W(r(b))),
            Src::Imm(v) => self.mov32(11, v),
        }
    }

    /// W12 = the guest's CF.
    fn carry_in(&mut self) {
        if self.dirty {
            dynasm!(self.ops ; .arch aarch64 ; and w12, w28, 1);
        } else {
            self.field(Access::Ldr32, 12, layout::FLAGS);
            dynasm!(self.ops ; .arch aarch64 ; and w12, w12, 1);
        }
    }

    fn alu(&mut self, op: AluOp, size: u8, a: T, b: Src) {
        let need = ARITH & self.live_after;
        self.operands(a, b);
        match op {
            AluOp::Add | AluOp::Adc => {
                if op == AluOp::Adc {
                    self.carry_in();
                }
                dynasm!(self.ops ; .arch aarch64 ; add x0, x10, x11);
                if op == AluOp::Adc {
                    dynasm!(self.ops ; .arch aarch64 ; add x0, x0, x12);
                }
                self.arith_flags(size, false, need);
            }
            AluOp::Sub | AluOp::Sbb | AluOp::Cmp => {
                if op == AluOp::Sbb {
                    self.carry_in();
                }
                dynasm!(self.ops ; .arch aarch64 ; sub x0, x10, x11);
                if op == AluOp::Sbb {
                    dynasm!(self.ops ; .arch aarch64 ; sub x0, x0, x12);
                }
                self.arith_flags(size, true, need);
            }
            AluOp::And | AluOp::Or | AluOp::Xor | AluOp::Test => {
                match op {
                    AluOp::Or => dynasm!(self.ops ; .arch aarch64 ; orr w0, w10, w11),
                    AluOp::Xor => dynasm!(self.ops ; .arch aarch64 ; eor w0, w10, w11),
                    _ => dynasm!(self.ops ; .arch aarch64 ; and w0, w10, w11),
                }
                // SZP; CF, OF and AF clear.
                if need != 0 {
                    self.flags_w5(size as u32 * 8, need, 0, 0);
                    self.merge(need);
                }
            }
        }
        if op.writes() {
            dynasm!(self.ops ; .arch aarch64 ; mov W(r(a)), w0);
        }
    }

    fn unary(&mut self, op: UnOp, size: u8, t: T) {
        match op {
            UnOp::Not => {
                dynasm!(self.ops ; .arch aarch64 ; mvn w0, W(r(t)));
                self.cut(size);
            }
            UnOp::Inc | UnOp::Dec => {
                // As ADD or SUB 1, leaving CF.
                self.operands(t, Src::Imm(1));
                if op == UnOp::Inc {
                    dynasm!(self.ops ; .arch aarch64 ; add x0, x10, x11);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; sub x0, x10, x11);
                }
                self.arith_flags(size, op == UnOp::Dec, ARITH & !CF & self.live_after);
            }
            UnOp::Neg => {
                // 0 - t.
                dynasm!(self.ops ; .arch aarch64 ; mov w11, W(r(t)) ; movz w10, 0 ; sub x0, x10, x11);
                self.arith_flags(size, true, ARITH & self.live_after);
            }
        }
        dynasm!(self.ops ; .arch aarch64 ; mov W(r(t)), w0);
    }

    /// The count of a shift by register `count` (CL) & 31 into W12, and on
    /// to the instruction's end if it is 0: nothing changes then. The
    /// guest's flags are in W28 both ways if the shift's are live.
    fn var_count(&mut self, count: Gpr, set: u32) {
        let end = self.end();
        self.field(Access::Ldr8, 12, gpr_offset(count));
        if set & self.live_after != 0 && !self.dirty {
            self.field(Access::Ldr32, 28, layout::FLAGS);
            self.dirty = true;
        }
        dynasm!(self.ops ; .arch aarch64 ; ands w12, w12, 31 ; b.eq =>end);
    }

    /// W`dst` = W`src` shifted left (`left`) or right by `count`, or by W12
    /// (None), logically.
    fn shift_by(&mut self, left: bool, dst: u8, src: u8, count: Option<u8>) {
        match (left, count) {
            (true, Some(c)) => {
                let c = c as u32;
                dynasm!(self.ops ; .arch aarch64 ; lsl W(dst), W(src), c);
            }
            (false, Some(c)) => {
                let c = c as u32;
                dynasm!(self.ops ; .arch aarch64 ; lsr W(dst), W(src), c);
            }
            (true, None) => dynasm!(self.ops ; .arch aarch64 ; lsl W(dst), W(src), w12),
            (false, None) => dynasm!(self.ops ; .arch aarch64 ; lsr W(dst), W(src), w12),
        }
    }

    /// W1 = bit `bits` - the count of W10 (CF of a shift left), or bit
    /// the count - 1 (of a shift right), for a count below `bits`.
    fn out_bit(&mut self, left: bool, bits: u32, count: Option<u8>) {
        match count {
            Some(c) => {
                let at = if left { bits - c as u32 } else { c as u32 - 1 };
                dynasm!(self.ops ; .arch aarch64 ; ubfx w1, w10, at, 1);
            }
            None => {
                if left {
                    self.mov32(2, bits);
                    dynasm!(self.ops ; .arch aarch64 ; sub w2, w2, w12);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; sub w2, w12, 1);
                }
                dynasm!(self.ops ; .arch aarch64 ; lsr w1, w10, w2 ; and w1, w1, 1);
            }
        }
    }

    /// W`dst` = W`src` shifted left (`left`) or right by `bits` - the count
    /// (`count`, or W12 for None; W3 is changed).
    fn shift_back(&mut self, left: bool, dst: u8, src: u8, bits: u32, count: Option<u8>) {
        match count {
            Some(c) => self.shift_by(left, dst, src, Some((bits - c as u32) as u8)),
            None => {
                self.mov32(3, bits);
                dynasm!(self.ops ; .arch aarch64 ; sub w3, w3, w12);
                if left {
                    dynasm!(self.ops ; .arch aarch64 ; lsl W(dst), W(src), w3);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; lsr W(dst), W(src), w3);
                }
            }
        }
    }

    /// Shifts and rotates by 1 to width - 1, `count` or W12 (None), with
    /// the flags `alu_shift` gives them.
    fn shift(&mut self, op: ShiftOp, size: u8, t: T, count: Option<u8>) {
        let bits = size as u32 * 8;
        let need = super::flags::shift_flags(op) & self.live_after;
        // The sign bit and the one below it.
        let (sign, below) = (bits - 1, bits - 2);
        dynasm!(self.ops ; .arch aarch64 ; mov w10, W(r(t)));
        match op {
            ShiftOp::Shl => {
                // CF: the last bit out; OF: the result's sign ^ CF; AF set.
                self.shift_by(true, 0, 10, count);
                if need & (CF | OF) != 0 {
                    self.out_bit(true, bits, count);
                }
                self.cut(size);
                if need & OF != 0 {
                    dynasm!(self.ops ; .arch aarch64 ; ubfx w2, w0, sign, 1 ; eor w2, w2, w1);
                }
                self.flags_w5(bits, need, CF | OF, AF);
            }
            ShiftOp::Shr => {
                // CF: the last bit out; OF: the result's top two bits
                // differ (its top bit is 0); AF set.
                self.shift_by(false, 0, 10, count);
                if need & CF != 0 {
                    self.out_bit(false, bits, count);
                }
                if need & OF != 0 {
                    dynasm!(self.ops ; .arch aarch64 ; ubfx w2, w0, below, 1);
                }
                self.flags_w5(bits, need, CF | OF, AF);
            }
            ShiftOp::Sar => {
                // OF clear, AF kept.
                match size {
                    1 => dynasm!(self.ops ; .arch aarch64 ; sxtb w10, w10),
                    2 => dynasm!(self.ops ; .arch aarch64 ; sxth w10, w10),
                    _ => {}
                }
                match count {
                    Some(c) => {
                        let c = c as u32;
                        dynasm!(self.ops ; .arch aarch64 ; asr w0, w10, c);
                    }
                    None => dynasm!(self.ops ; .arch aarch64 ; asr w0, w10, w12),
                }
                if need & CF != 0 {
                    self.out_bit(false, bits, count);
                }
                self.cut(size);
                self.flags_w5(bits, need, CF, 0);
            }
            ShiftOp::Rol | ShiftOp::Ror => {
                let left = op == ShiftOp::Rol;
                if size == 4 {
                    match (left, count) {
                        (_, Some(c)) => {
                            let right = if left { 32 - c as u32 } else { c as u32 };
                            dynasm!(self.ops ; .arch aarch64 ; ror w0, w10, right);
                        }
                        (true, None) => dynasm!(self.ops ; .arch aarch64 ; neg w2, w12 ; ror w0, w10, w2),
                        (false, None) => dynasm!(self.ops ; .arch aarch64 ; ror w0, w10, w12),
                    }
                } else {
                    self.shift_by(left, 2, 10, count);
                    self.shift_back(!left, 3, 10, bits, count);
                    dynasm!(self.ops ; .arch aarch64 ; orr w0, w2, w3);
                }
                self.cut(size);
                if left {
                    // CF: the result's bottom bit; OF: its top bit ^ CF.
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; and w1, w0, 1
                        ; ubfx w2, w0, sign, 1
                        ; eor w2, w2, w1
                    );
                } else {
                    // CF: the result's top bit; OF: its top two bits differ.
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; ubfx w1, w0, sign, 1
                        ; ubfx w2, w0, below, 1
                        ; eor w2, w2, w1
                    );
                }
                self.flags_w5(bits, need, CF | OF, 0);
            }
            ShiftOp::Rcl | ShiftOp::Rcr => unreachable!("not translated"),
        }
        if need != 0 {
            self.merge(need);
        }
        dynasm!(self.ops ; .arch aarch64 ; mov W(r(t)), w0);
    }

    /// SHLD and SHRD by 1 to width - 1, `count` or W12 (None), with the
    /// flags `alu_double_shift` gives them (AF kept).
    fn double_shift(&mut self, left: bool, size: u8, dst: T, src: T, count: Option<u8>) {
        let bits = size as u32 * 8;
        let (sign, below) = (bits - 1, bits - 2);
        let need = (CF | OF | SZP) & self.live_after;
        dynasm!(self.ops ; .arch aarch64 ; mov w10, W(r(dst)) ; mov w11, W(r(src)));
        // dest:src shifted left, or src:dest right; CF is the last bit out
        // of dest.
        self.shift_by(left, 2, 10, count);
        self.shift_back(!left, 4, 11, bits, count);
        dynasm!(self.ops ; .arch aarch64 ; orr w0, w2, w4);
        if need & (CF | OF) != 0 {
            self.out_bit(left, bits, count);
        }
        self.cut(size);
        if need != 0 {
            if left {
                // OF = the result's sign ^ CF.
                dynasm!(self.ops ; .arch aarch64 ; ubfx w2, w0, sign, 1 ; eor w2, w2, w1);
            } else {
                // OF = the result's top two bits differ.
                dynasm!(self.ops ; .arch aarch64 ; ubfx w2, w0, sign, 1 ; ubfx w3, w0, below, 1 ; eor w2, w2, w3);
            }
            self.flags_w5(bits, need, CF | OF, 0);
            self.merge(need);
        }
        dynasm!(self.ops ; .arch aarch64 ; mov W(r(dst)), w0);
    }

    /// The two- and three-operand IMUL: a = a * b cut to `size`, CF and OF
    /// when the product doesn't fit.
    fn imul(&mut self, size: u8, a: T, b: Src) {
        let need = (CF | OF) & self.live_after;
        self.operands(a, b);
        if size == 2 {
            // Both 16-bit products fit in 32 bits.
            dynasm!(self.ops ; .arch aarch64 ; sxth w10, w10 ; sxth w11, w11 ; mul w0, w10, w11);
            if need != 0 {
                dynasm!(self.ops ; .arch aarch64 ; sxth w1, w0 ; cmp w1, w0 ; cset w2, ne);
            }
            dynasm!(self.ops ; .arch aarch64 ; and w0, w0, 0xFFFF);
        } else {
            dynasm!(self.ops ; .arch aarch64 ; smull x0, w10, w11);
            if need != 0 {
                dynasm!(self.ops ; .arch aarch64 ; sxtw x1, w0 ; cmp x1, x0 ; cset w2, ne);
            }
        }
        dynasm!(self.ops ; .arch aarch64 ; mov W(r(a)), w0);
        if need != 0 {
            dynasm!(self.ops ; .arch aarch64 ; orr w5, w2, w2, lsl 11);
            self.merge(need);
        }
    }

    /// MUL or IMUL of AL, AX or EAX by t into AX, DX:AX or EDX:EAX: CF and
    /// OF when the upper half is in use (for IMUL, isn't the lower's sign).
    fn mul_wide(&mut self, signed: bool, size: u8, t: T) {
        let need = (CF | OF) & self.live_after;
        let (acc, high) = (gpr_offset(Gpr::dword(0)), gpr_offset(Gpr::dword(2)));
        let access = match size {
            1 => Access::Ldr8,
            2 => Access::Ldr16,
            _ => Access::Ldr32,
        };
        self.field(access, 10, acc);
        dynasm!(self.ops ; .arch aarch64 ; mov w11, W(r(t)));
        match (signed, size) {
            (false, 4) => dynasm!(self.ops ; .arch aarch64 ; umull x0, w10, w11 ; lsr x1, x0, 32 ; cmp x1, 0),
            (true, 4) => dynasm!(self.ops ; .arch aarch64 ; smull x0, w10, w11 ; sxtw x1, w0 ; cmp x1, x0),
            (false, _) => {
                let bits = size as u32 * 8;
                dynasm!(self.ops ; .arch aarch64 ; mul w0, w10, w11 ; lsr w1, w0, bits ; cmp w1, 0);
            }
            (true, 1) => dynasm!(self.ops
                ; .arch aarch64
                ; sxtb w10, w10
                ; sxtb w11, w11
                ; mul w0, w10, w11
                ; sxtb w1, w0
                ; cmp w1, w0
            ),
            (true, _) => dynasm!(self.ops
                ; .arch aarch64
                ; sxth w10, w10
                ; sxth w11, w11
                ; mul w0, w10, w11
                ; sxth w1, w0
                ; cmp w1, w0
            ),
        }
        if need != 0 {
            dynasm!(self.ops ; .arch aarch64 ; cset w2, ne ; orr w5, w2, w2, lsl 11);
        }
        match size {
            1 => self.set_gpr(Gpr::word(0), 0),
            2 => {
                self.set_gpr(Gpr::word(0), 0);
                dynasm!(self.ops ; .arch aarch64 ; lsr w0, w0, 16);
                self.set_gpr(Gpr::word(2), 0);
            }
            _ => {
                self.field(Access::Str32, 0, acc);
                dynasm!(self.ops ; .arch aarch64 ; lsr x0, x0, 32);
                self.field(Access::Str32, 0, high);
            }
        }
        if need != 0 {
            self.merge(need);
        }
    }

    /// DIV or IDIV of AX, DX:AX or EDX:EAX by t, or #DE first where the
    /// quotient doesn't fit. The flags stay.
    fn div_wide(&mut self, signed: bool, size: u8, t: T) {
        let t = r(t);
        let de = self.fault_exit(EXIT_DE);
        let (acc, high) = (gpr_offset(Gpr::dword(0)), gpr_offset(Gpr::dword(2)));
        match (signed, size) {
            // Unsigned, the quotient fits if the dividend's upper half is
            // below the divisor, which a divisor of 0 never is.
            (false, 1) => {
                self.field(Access::Ldr16, 0, acc);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; lsr w1, w0, 8
                    ; cmp w1, W(t)
                    ; b.hs =>de
                    ; udiv w3, w0, W(t)
                    ; msub w4, w3, W(t), w0
                    ; orr w3, w3, w4, lsl 8
                );
                self.set_gpr(Gpr::word(0), 3);
            }
            (false, 2) => {
                self.field(Access::Ldr16, 0, acc);
                self.field(Access::Ldr16, 1, high);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp w1, W(t)
                    ; b.hs =>de
                    ; orr w0, w0, w1, lsl 16
                    ; udiv w3, w0, W(t)
                    ; msub w4, w3, W(t), w0
                );
                self.set_gpr(Gpr::word(0), 3);
                self.set_gpr(Gpr::word(2), 4);
            }
            (false, _) => {
                self.field(Access::Ldr32, 0, acc);
                self.field(Access::Ldr32, 1, high);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cmp w1, W(t)
                    ; b.hs =>de
                    ; orr x0, x0, x1, lsl 32
                    ; mov w2, W(t)
                    ; udiv x3, x0, x2
                    ; msub x4, x3, x2, x0
                );
                self.field(Access::Str32, 3, acc);
                self.field(Access::Str32, 4, high);
            }
            // Signed, the division is at least twice as wide as the
            // guest's (and never traps), and then the quotient must fit.
            (true, 1) => {
                self.field(Access::Ldr16, 0, acc);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; sxtb w2, W(t)
                    ; cbz w2, =>de
                    ; sxth w0, w0
                    ; sdiv w3, w0, w2
                    ; sxtb w5, w3
                    ; cmp w5, w3
                    ; b.ne =>de
                    ; msub w4, w3, w2, w0
                    ; and w3, w3, 0xFF
                    ; orr w3, w3, w4, lsl 8
                );
                self.set_gpr(Gpr::word(0), 3);
            }
            (true, 2) => {
                self.field(Access::Ldr16, 0, acc);
                self.field(Access::Ldr16, 1, high);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; sxth x2, W(t)
                    ; cbz x2, =>de
                    ; orr w0, w0, w1, lsl 16
                    ; sxtw x0, w0
                    ; sdiv x3, x0, x2
                    ; sxth x5, w3
                    ; cmp x5, x3
                    ; b.ne =>de
                    ; msub x4, x3, x2, x0
                );
                self.set_gpr(Gpr::word(0), 3);
                self.set_gpr(Gpr::word(2), 4);
            }
            (true, _) => {
                self.field(Access::Ldr32, 0, acc);
                self.field(Access::Ldr32, 1, high);
                // -2^63 / -1 gives -2^63, which doesn't fit either.
                dynasm!(self.ops
                    ; .arch aarch64
                    ; sxtw x2, W(t)
                    ; cbz x2, =>de
                    ; orr x0, x0, x1, lsl 32
                    ; sdiv x3, x0, x2
                    ; sxtw x5, w3
                    ; cmp x5, x3
                    ; b.ne =>de
                    ; msub x4, x3, x2, x0
                );
                self.field(Access::Str32, 3, acc);
                self.field(Access::Str32, 4, high);
            }
        }
    }

    fn exit_if(&mut self, cond: Cond, taken: u32, next: u32, commit: Option<(Gpr, T)>) {
        let yes = self.ops.new_dynamic_label();
        // The flags go back into the CPU for both ways out; the condition
        // reads them where they were.
        let in_w28 = self.dirty;
        self.flags_back();
        self.dirty = false;
        match cond {
            Cond::Flags(cc) => self.condition(cc, yes, in_w28),
            Cond::Zero(t) => dynasm!(self.ops ; .arch aarch64 ; cbz W(r(t)), =>yes),
            Cond::NonZero(t) => dynasm!(self.ops ; .arch aarch64 ; cbnz W(r(t)), =>yes),
            Cond::NonZeroZf(t, zf) => {
                let no = self.ops.new_dynamic_label();
                dynasm!(self.ops ; .arch aarch64 ; cbz W(r(t)), =>no);
                self.load_flags_w0(in_w28);
                dynasm!(self.ops ; .arch aarch64 ; tst w0, ZF);
                if zf {
                    dynasm!(self.ops ; .arch aarch64 ; b.ne =>yes);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; b.eq =>yes);
                }
                dynasm!(self.ops ; .arch aarch64 ; =>no);
            }
        }
        // Not taken.
        self.commit(commit);
        self.leave(Some(next), 1, true);
        dynasm!(self.ops ; .arch aarch64 ; =>yes);
        // Taken: the target must be within the CS limit.
        let gp = self.fault_exit(EXIT_GP0);
        self.mov32(0, taken);
        self.field(Access::Ldr32, 1, seg_field(Seg::CS, layout::SEG_LIMIT));
        dynasm!(self.ops ; .arch aarch64 ; cmp w0, w1 ; b.hi =>gp);
        self.commit(commit);
        self.leave(Some(taken), 0, true);
    }

    fn commit(&mut self, commit: Option<(Gpr, T)>) {
        if let Some((g, t)) = commit {
            self.uop(&Uop::Set { r: g, t });
        }
    }

    /// Leave the block after its last instruction, at `eip` if the code
    /// sets it (`set`) or knows it: through link `slot` if that is in the
    /// page, else back to the execution loop. The counts first.
    fn leave(&mut self, eip: Option<u32>, slot: usize, set: bool) {
        if let (Some(eip), true) = (eip, set) {
            self.mov32(0, eip);
            self.field(Access::Str32, 0, layout::EIP);
        }
        self.flags_back();
        self.counts();
        match eip {
            Some(eip) if self.link && self.data.in_page(eip) => {
                self.stubs[slot].get_or_insert_with(|| self.ops.new_dynamic_label());
                let at = DATA_LINKS as u32 + slot as u32 * 8;
                dynasm!(self.ops ; .arch aarch64 ; ldr x16, [x1, at] ; br x16);
            }
            Some(_) if self.link => self.guarded(slot),
            _ => {
                dynasm!(self.ops ; .arch aarch64 ; movz w0, EXIT_NEXT);
                self.exit();
            }
        }
    }

    /// Bring the counts up to date for leaving the block after its last
    /// instruction, and X1 = the block.
    fn counts(&mut self) {
        let data = self.data;
        let (n, synced) = (data.count() as i32, self.synced[data.count() - 1]);
        self.add_field64(layout::ICOUNT, (n - synced) as u32);
        self.add_field64(layout::EXECUTED, n as u32);
        self.data_x1();
    }

    /// Leave through the return link to EIP `t`, if the return has one
    /// (see `guarded`), else to the execution loop, to be linked. X1 is
    /// the block.
    fn returned(&mut self, t: T) {
        let miss = *self.return_miss.get_or_insert_with(|| self.ops.new_dynamic_label());
        for slot in RETURN_LINK..LINKS {
            let next = self.ops.new_dynamic_label();
            let g_eip = DATA_GUARDS as u32 + slot as u32 * GUARD_SIZE as u32 + GUARD_EIP as u32;
            dynasm!(self.ops ; .arch aarch64 ; ldr w2, [x1, g_eip] ; cmp W(r(t)), w2 ; b.ne =>next);
            self.guarded(slot);
            dynasm!(self.ops ; .arch aarch64 ; =>next);
        }
        dynasm!(self.ops ; .arch aarch64 ; b =>miss);
    }

    /// Leave through link `slot` to another page, if fetching its target
    /// goes as when the link was made (see `x64::Gen::guarded`), else
    /// through the stub. X1 is the block.
    fn guarded(&mut self, slot: usize) {
        let stub = *self.stubs[slot].get_or_insert_with(|| self.ops.new_dynamic_label());
        let g = DATA_GUARDS as u32 + slot as u32 * GUARD_SIZE as u32;
        let (g_cs, g_a20, g_paging, g_page, g_phys) = (
            g + GUARD_CS_BASE as u32,
            g + GUARD_A20 as u32,
            g + GUARD_PAGING as u32,
            g + GUARD_PAGE as u32,
            g + GUARD_PHYS as u32,
        );
        self.field(Access::Ldr32, 2, seg_field(Seg::CS, layout::SEG_BASE));
        dynasm!(self.ops ; .arch aarch64 ; ldr w3, [x1, g_cs] ; cmp w2, w3 ; b.ne =>stub);
        self.field(Access::Ldr32, 2, layout::A20_MASK);
        dynasm!(self.ops ; .arch aarch64 ; ldr w3, [x1, g_a20] ; cmp w2, w3 ; b.ne =>stub);
        self.field(Access::Ldr32, 2, layout::CR0);
        dynasm!(self.ops
            ; .arch aarch64
            ; lsr w2, w2, 31
            ; ldr w3, [x1, g_paging]
            ; cmp w2, w3
            ; b.ne =>stub
            ; cbz w2, >go
            // The TLB entry of the page in the set of the privilege level.
            ; ldr w4, [x1, g_page]
            ; and w3, w4, (layout::TLB_SET - 1) as u32
        );
        self.field(Access::Ldr8, 2, layout::CPL);
        dynasm!(self.ops
            ; .arch aarch64
            ; cmp w2, 3
            ; b.ne >supervisor
            ; add w3, w3, layout::TLB_SET as u32
            ; supervisor:
        );
        self.mov32(5, layout::TLB_ENTRY_SIZE as u32);
        let at = DATA_LINKS as u32 + slot as u32 * 8;
        dynasm!(self.ops
            ; .arch aarch64
            ; umull x3, w3, w5
            ; ldr x6, [x20, CTX_TLB as u32]
            ; add x6, x6, x3
            ; add w4, w4, 1
            ; ldr w5, [x6, layout::TLB_READ_TAG as u32]
            ; cmp w4, w5
            ; b.ne =>stub
            ; ldr w5, [x6, layout::TLB_PHYS as u32]
            ; ldr w3, [x1, g_phys]
            ; cmp w5, w3
            ; b.ne =>stub
            ; go:
            ; ldr x16, [x1, at]
            ; br x16
        );
    }

    /// W0 = the guest's flags, from W28 (`in_w28`) or the CPU.
    fn load_flags_w0(&mut self, in_w28: bool) {
        if in_w28 {
            dynasm!(self.ops ; .arch aarch64 ; mov w0, w28);
        } else {
            self.field(Access::Ldr32, 0, layout::FLAGS);
        }
    }

    /// Branch to `yes` if condition `cc` holds on the guest's flags, in W28
    /// (`in_w28`) or the CPU.
    fn condition(&mut self, cc: ConditionCode, yes: DynamicLabel, in_w28: bool) {
        if self.test_condition(cc, in_w28) {
            dynasm!(self.ops ; .arch aarch64 ; b.ne =>yes);
        } else {
            dynasm!(self.ops ; .arch aarch64 ; b.eq =>yes);
        }
    }

    /// Test condition `cc` on the guest's flags, in W28 (`in_w28`) or the
    /// CPU: it holds if the host's Z is clear (true) or set (false).
    fn test_condition(&mut self, cc: ConditionCode, in_w28: bool) -> bool {
        use ConditionCode as C;
        self.load_flags_w0(in_w28);
        match cc {
            C::o | C::b | C::e | C::be | C::s | C::p | C::no | C::ae | C::ne | C::a | C::ns | C::np => {
                self.mov32(1, super::flags::cond_flags(cc));
                dynasm!(self.ops ; .arch aarch64 ; tst w0, w1);
                matches!(cc, C::o | C::b | C::e | C::be | C::s | C::p)
            }
            C::l | C::ge => {
                // SF != OF: OF moved down to SF's bit.
                dynasm!(self.ops ; .arch aarch64 ; eor w1, w0, w0, lsr 4 ; tst w1, SF);
                cc == C::l
            }
            _ => {
                // LE: ZF or SF != OF; G: neither.
                dynasm!(self.ops
                    ; .arch aarch64
                    ; eor w1, w0, w0, lsr 4
                    ; and w1, w1, SF
                    ; and w0, w0, ZF
                    ; orr w0, w0, w1
                    ; tst w0, w0
                );
                cc == C::le
            }
        }
    }
}
