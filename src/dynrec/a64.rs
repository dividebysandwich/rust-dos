//! The AArch64 code generator.
//!
//! Registers while translated code runs: X19 the CPU, X20 the context
//! (`JitCtx`), X21 RAM, X22 RAM's code generations, W23 set when a store hit
//! the block's later bytes, X27 the CPU plus `HI` (the CPU's fields past
//! X19's offsets' reach); W24-W26 hold the operations' temporaries
//! (`uop::T`), which calls keep, and X0-X17 are scratch (X18, the
//! platform's register, is never touched). The guest's registers and flags
//! stay in the CPU. AArch64 has neither the parity nor the auxiliary carry
//! flag, so the code computes the guest's flags the way `cpu::alu` defines
//! them, with a parity table in the context.

// dynasm converts the registers it is given at run time with `into`, and
// checks the bit field operands it is given with comparisons that are
// constant for constant operands.
#![allow(clippy::useless_conversion, clippy::absurd_extreme_comparisons, clippy::eq_op)]

use dynasmrt::aarch64::Aarch64Relocation;
use dynasmrt::{DynamicLabel, DynasmApi, DynasmLabelApi, VecAssembler, dynasm};
use iced_x86::ConditionCode;

use super::block::{BlockData, LINKS, RETURN_LINK};
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
/// X1, which it stores in the context before returning the code.
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
}

struct Gen<'a> {
    ops: Asm,
    data: &'a BlockData,
    /// Where the block's data pointer is, in the literal after the code.
    data_lit: DynamicLabel,
    /// Per instruction: where it leaves the block with the exit code in W0
    /// (counting it as executed but not done), and the instruction count
    /// brought up to date for it.
    fail: Vec<DynamicLabel>,
    synced: Vec<i32>,
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
    /// The instruction being translated.
    ix: usize,
}

/// A translated block's code, and where its links' stubs are in it.
pub struct Code {
    pub bytes: Vec<u8>,
    pub stubs: [Option<usize>; LINKS],
}

/// Translate a block: each instruction's operations (`items[ix]`), or a
/// call of its interpreter handler (None). With `link`, exits to a known
/// EIP in the block's page go through its links. See `x64::block`, which
/// this follows.
pub fn block(data: &BlockData, items: &[Option<Vec<Uop>>], link: bool) -> Code {
    let mut ops = Asm::new(0);
    let n = data.count();
    let fail = (0..n).map(|_| ops.new_dynamic_label()).collect();
    let mut labels = || ops.new_dynamic_label();
    let (data_lit, tail, deadline, revalidate, limit, body) = (labels(), labels(), labels(), labels(), labels(), labels());
    let mut g = Gen {
        ops,
        data,
        data_lit,
        fail,
        synced: vec![0; n],
        tail,
        deadline,
        revalidate,
        limit,
        body,
        slow: Vec::new(),
        link,
        stubs: [None; LINKS],
        ix: 0,
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
                g.fallback(ix as u32);
            }
            Some(uops) => {
                g.synced[ix] = synced;
                for uop in uops {
                    g.uop(uop);
                }
                if uops.iter().any(|u| matches!(u, Uop::Store { .. })) {
                    // A store hit the rest of the block: leave after this
                    // instruction.
                    let next = data.eips[ix].wrapping_add(data.instrs[ix].len() as u32);
                    let (fail, skip) = (g.fail[ix], g.ops.new_dynamic_label());
                    dynasm!(g.ops ; .arch aarch64 ; cbz w23, =>skip);
                    g.mov32(0, next);
                    g.field(Access::Str32, 0, layout::EIP);
                    dynasm!(g.ops
                        ; .arch aarch64
                        ; movz w0, EXIT_SMC
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
    // The end: the counts, and back to the execution loop.
    dynasm!(g.ops ; .arch aarch64 ; =>tail);
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
    Code { bytes: g.ops.finalize().expect("block"), stubs }
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
        let fail = self.fail[ix as usize];
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
        for (k, stub) in self.stubs.into_iter().enumerate() {
            if let Some(stub) = stub {
                dynasm!(self.ops ; .arch aarch64 ; =>stub);
                mov32(&mut self.ops, 0, EXIT_UNLINKED | (k as u32) << 8);
                self.exit();
            }
        }
        for ix in 0..self.data.count() {
            // Instruction ix stopped the block: it counts as executed but
            // not in the instruction count (see `x64`).
            let label = self.fail[ix];
            dynasm!(self.ops ; .arch aarch64 ; =>label ; mov w8, w0);
            self.add_field64(layout::ICOUNT, (ix as i32 - self.synced[ix]) as u32);
            self.add_field64(layout::EXECUTED, ix as u32 + 1);
            self.mov32(9, (ix as u32) << 8);
            dynasm!(self.ops ; .arch aarch64 ; orr w0, w8, w9);
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

    /// Instruction ix's #GP(0) exit.
    fn gp0(&mut self) -> DynamicLabel {
        let (label, skip, fail) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label(), self.fail[self.ix]);
        dynasm!(self.ops
            ; .arch aarch64
            ; b =>skip
            ; =>label
            ; movz w0, EXIT_GP0
            ; b =>fail
            ; =>skip
        );
        label
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
            Uop::Shift { op, size, t, count } => self.shift(op, size, t, count),
            Uop::DoubleShift { left, size, dst, src, count } => self.double_shift(left, size, dst, src, count),
            Uop::Imul { size, a, b } => self.imul(size, a, b),
            Uop::MulWide { signed, size, t } => self.mul_wide(signed, size, t),
            Uop::Flag { mask, set } => {
                self.field(Access::Ldr32, 0, layout::FLAGS);
                self.mov32(1, mask);
                match set {
                    Some(true) => dynasm!(self.ops ; .arch aarch64 ; orr w0, w0, w1),
                    Some(false) => dynasm!(self.ops ; .arch aarch64 ; bic w0, w0, w1),
                    None => dynasm!(self.ops ; .arch aarch64 ; eor w0, w0, w1),
                }
                self.field(Access::Str32, 0, layout::FLAGS);
            }
            Uop::CheckLimit { src } => {
                self.value_w0(src);
                let gp = self.gp0();
                self.field(Access::Ldr32, 1, seg_field(Seg::CS, layout::SEG_LIMIT));
                dynasm!(self.ops ; .arch aarch64 ; cmp w0, w1 ; b.hi =>gp);
            }
            Uop::Exit { eip: Src::Imm(target) } => self.leave(Some(target), 0, true),
            Uop::Exit { eip: Src::T(t) } => {
                self.field(Access::Str32, r(t), layout::EIP);
                if self.link {
                    // A return: through its link if it goes where the link
                    // was made to.
                    self.counts();
                    self.guarded(RETURN_LINK, Some(t));
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
        self.slow.push(Slow::MemRef { at, back, t, desc, fail: self.fail[self.ix] });
    }

    /// SF, ZF and PF of the result in W0 (`bits` wide) into W5.
    ///
    /// The bitfield instructions' `lsb` operands are single names
    /// throughout: dynasm pastes a run-time `lsb` into its range check
    /// (`31 - lsb`) as it is, so `bits - 1` there would check `31 - bits -
    /// 1`, which underflows at 32 bits (a panic in debug builds).
    fn szp(&mut self, bits: u32) {
        let sign = bits - 1;
        dynasm!(self.ops
            ; .arch aarch64
            ; and w6, w0, 0xFF
            ; add x6, x20, x6
            ; ldrb w5, [x6, CTX_PARITY as u32]
            ; cmp w0, 0
            ; cset w6, eq
            ; orr w5, w5, w6, lsl 6
            ; ubfx w6, w0, sign, 1
            ; orr w5, w5, w6, lsl 7
        );
    }

    /// Put the bits of W5 in `mask` into the guest's flags.
    fn merge(&mut self, mask: u32) {
        self.field(Access::Ldr32, 8, layout::FLAGS);
        self.mov32(9, mask);
        dynasm!(self.ops
            ; .arch aarch64
            ; bic w8, w8, w9
            ; and w5, w5, w9
            ; orr w8, w8, w5
        );
        self.field(Access::Str32, 8, layout::FLAGS);
    }

    /// W0 = W0 & the size's mask.
    fn cut(&mut self, size: u8) {
        match size {
            1 => dynasm!(self.ops ; .arch aarch64 ; and w0, w0, 0xFF),
            2 => dynasm!(self.ops ; .arch aarch64 ; and w0, w0, 0xFFFF),
            _ => {}
        }
    }

    /// The flags of an addition (`sub` false) or subtraction of W11 and
    /// W10 whose full result is in X0 (a 64-bit sum or difference of the
    /// zero-extended operands), into W5; W0 becomes the result.
    fn arith_flags(&mut self, size: u8, sub: bool) {
        let bits = size as u32 * 8;
        let sign = bits - 1;
        if sub {
            // A borrow makes the 64-bit difference negative.
            dynasm!(self.ops ; .arch aarch64 ; lsr x1, x0, 63);
        } else {
            dynasm!(self.ops ; .arch aarch64 ; lsr x1, x0, bits);
        }
        self.cut(size);
        if sub {
            // OF: the operands' signs differ and the result's is the
            // subtrahend's.
            dynasm!(self.ops ; .arch aarch64 ; eor w2, w10, w11 ; eor w3, w10, w0 ; and w2, w2, w3);
        } else {
            // OF: the operands' signs agree and the result's doesn't.
            dynasm!(self.ops ; .arch aarch64 ; eor w2, w10, w0 ; eor w3, w11, w0 ; and w2, w2, w3);
        }
        dynasm!(self.ops
            ; .arch aarch64
            ; ubfx w2, w2, sign, 1
            ; eor w3, w10, w11
            ; eor w3, w3, w0
            ; ubfx w3, w3, 4, 1
        );
        self.szp(bits);
        dynasm!(self.ops
            ; .arch aarch64
            ; orr w5, w5, w1
            ; orr w5, w5, w3, lsl 4
            ; orr w5, w5, w2, lsl 11
        );
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
        self.field(Access::Ldr32, 12, layout::FLAGS);
        dynasm!(self.ops ; .arch aarch64 ; and w12, w12, 1);
    }

    fn alu(&mut self, op: AluOp, size: u8, a: T, b: Src) {
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
                self.arith_flags(size, false);
                self.merge(ARITH);
            }
            AluOp::Sub | AluOp::Sbb | AluOp::Cmp => {
                if op == AluOp::Sbb {
                    self.carry_in();
                }
                dynasm!(self.ops ; .arch aarch64 ; sub x0, x10, x11);
                if op == AluOp::Sbb {
                    dynasm!(self.ops ; .arch aarch64 ; sub x0, x0, x12);
                }
                self.arith_flags(size, true);
                self.merge(ARITH);
            }
            AluOp::And | AluOp::Or | AluOp::Xor | AluOp::Test => {
                match op {
                    AluOp::Or => dynasm!(self.ops ; .arch aarch64 ; orr w0, w10, w11),
                    AluOp::Xor => dynasm!(self.ops ; .arch aarch64 ; eor w0, w10, w11),
                    _ => dynasm!(self.ops ; .arch aarch64 ; and w0, w10, w11),
                }
                // SZP; CF, OF and AF clear.
                self.szp(size as u32 * 8);
                self.merge(ARITH);
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
                self.arith_flags(size, op == UnOp::Dec);
                self.merge(ARITH & !CF);
            }
            UnOp::Neg => {
                // 0 - t.
                dynasm!(self.ops ; .arch aarch64 ; mov w11, W(r(t)) ; movz w10, 0 ; sub x0, x10, x11);
                self.arith_flags(size, true);
                self.merge(ARITH);
            }
        }
        dynasm!(self.ops ; .arch aarch64 ; mov W(r(t)), w0);
    }

    /// Shifts and rotates by 1 to width - 1, with the flags `alu_shift`
    /// gives them.
    fn shift(&mut self, op: ShiftOp, size: u8, t: T, count: u8) {
        let bits = size as u32 * 8;
        let c = count as u32;
        // The sign bit, the one below it, and the last bit a shift left or
        // right moves out.
        let (sign, below) = (bits - 1, bits - 2);
        let (out_left, out_right) = (bits - c, c - 1);
        dynasm!(self.ops ; .arch aarch64 ; mov w10, W(r(t)));
        match op {
            ShiftOp::Shl => {
                // CF: the last bit out; OF: the result's sign ^ CF; AF set.
                dynasm!(self.ops
                    ; .arch aarch64
                    ; lsl w0, w10, c
                    ; ubfx w1, w10, out_left, 1
                );
                self.cut(size);
                dynasm!(self.ops ; .arch aarch64 ; ubfx w2, w0, sign, 1 ; eor w2, w2, w1);
                self.szp(bits);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; orr w5, w5, w1
                    ; orr w5, w5, AF
                    ; orr w5, w5, w2, lsl 11
                );
                self.merge(ARITH);
            }
            ShiftOp::Shr => {
                // CF: the last bit out; OF: the result's top two bits
                // differ (its top bit is 0); AF set.
                dynasm!(self.ops
                    ; .arch aarch64
                    ; lsr w0, w10, c
                    ; ubfx w1, w10, out_right, 1
                    ; ubfx w2, w0, below, 1
                );
                self.szp(bits);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; orr w5, w5, w1
                    ; orr w5, w5, AF
                    ; orr w5, w5, w2, lsl 11
                );
                self.merge(ARITH);
            }
            ShiftOp::Sar => {
                // OF clear, AF kept.
                match size {
                    1 => dynasm!(self.ops ; .arch aarch64 ; sxtb w10, w10),
                    2 => dynasm!(self.ops ; .arch aarch64 ; sxth w10, w10),
                    _ => {}
                }
                dynasm!(self.ops
                    ; .arch aarch64
                    ; asr w0, w10, c
                    ; ubfx w1, w10, out_right, 1
                );
                self.cut(size);
                self.szp(bits);
                dynasm!(self.ops ; .arch aarch64 ; orr w5, w5, w1);
                self.merge(CF | OF | SZP);
            }
            ShiftOp::Rol | ShiftOp::Ror => {
                let left = op == ShiftOp::Rol;
                if size == 4 {
                    let right = if left { 32 - c } else { c };
                    dynasm!(self.ops ; .arch aarch64 ; ror w0, w10, right);
                } else if left {
                    dynasm!(self.ops ; .arch aarch64 ; lsl w2, w10, c ; lsr w3, w10, bits - c ; orr w0, w2, w3);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; lsr w2, w10, c ; lsl w3, w10, bits - c ; orr w0, w2, w3);
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
                dynasm!(self.ops ; .arch aarch64 ; orr w5, w1, w2, lsl 11);
                self.merge(CF | OF);
            }
            ShiftOp::Rcl | ShiftOp::Rcr => unreachable!("not translated"),
        }
        dynasm!(self.ops ; .arch aarch64 ; mov W(r(t)), w0);
    }

    /// SHLD and SHRD by 1 to width - 1, with the flags
    /// `alu_double_shift` gives them (AF kept).
    fn double_shift(&mut self, left: bool, size: u8, dst: T, src: T, count: u8) {
        let bits = size as u32 * 8;
        let c = count as u32;
        let back = bits - c;
        let (sign, below, out_right) = (bits - 1, bits - 2, c - 1);
        dynasm!(self.ops ; .arch aarch64 ; mov w10, W(r(dst)) ; mov w11, W(r(src)));
        if left {
            // dest:src shifted left; CF is the last bit out of dest.
            dynasm!(self.ops
                ; .arch aarch64
                ; lsl w2, w10, c
                ; lsr w3, w11, back
                ; orr w0, w2, w3
                ; ubfx w1, w10, back, 1
            );
        } else {
            // src:dest shifted right; CF is the last bit out of dest.
            dynasm!(self.ops
                ; .arch aarch64
                ; lsr w2, w10, c
                ; lsl w3, w11, back
                ; orr w0, w2, w3
                ; ubfx w1, w10, out_right, 1
            );
        }
        self.cut(size);
        if left {
            // OF = the result's sign ^ CF.
            dynasm!(self.ops ; .arch aarch64 ; ubfx w2, w0, sign, 1 ; eor w2, w2, w1);
        } else {
            // OF = the result's top two bits differ.
            dynasm!(self.ops ; .arch aarch64 ; ubfx w2, w0, sign, 1 ; ubfx w3, w0, below, 1 ; eor w2, w2, w3);
        }
        self.szp(bits);
        dynasm!(self.ops ; .arch aarch64 ; orr w5, w5, w1 ; orr w5, w5, w2, lsl 11);
        self.merge(CF | OF | SZP);
        dynasm!(self.ops ; .arch aarch64 ; mov W(r(dst)), w0);
    }

    /// The two- and three-operand IMUL: a = a * b cut to `size`, CF and OF
    /// when the product doesn't fit.
    fn imul(&mut self, size: u8, a: T, b: Src) {
        self.operands(a, b);
        if size == 2 {
            // Both 16-bit products fit in 32 bits.
            dynasm!(self.ops
                ; .arch aarch64
                ; sxth w10, w10
                ; sxth w11, w11
                ; mul w0, w10, w11
                ; sxth w1, w0
                ; cmp w1, w0
                ; cset w2, ne
                ; and w0, w0, 0xFFFF
            );
        } else {
            dynasm!(self.ops
                ; .arch aarch64
                ; smull x0, w10, w11
                ; sxtw x1, w0
                ; cmp x1, x0
                ; cset w2, ne
            );
        }
        dynasm!(self.ops
            ; .arch aarch64
            ; orr w5, w2, w2, lsl 11
            ; mov W(r(a)), w0
        );
        self.merge(CF | OF);
    }

    /// MUL or IMUL of AL, AX or EAX by t into AX, DX:AX or EDX:EAX: CF and
    /// OF when the upper half is in use (for IMUL, isn't the lower's sign).
    fn mul_wide(&mut self, signed: bool, size: u8, t: T) {
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
        dynasm!(self.ops ; .arch aarch64 ; cset w2, ne ; orr w5, w2, w2, lsl 11);
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
        self.merge(CF | OF);
    }

    fn exit_if(&mut self, cond: Cond, taken: u32, next: u32, commit: Option<(Gpr, T)>) {
        let yes = self.ops.new_dynamic_label();
        match cond {
            Cond::Flags(cc) => self.condition(cc, yes),
            Cond::Zero(t) => dynasm!(self.ops ; .arch aarch64 ; cbz W(r(t)), =>yes),
            Cond::NonZero(t) => dynasm!(self.ops ; .arch aarch64 ; cbnz W(r(t)), =>yes),
            Cond::NonZeroZf(t, zf) => {
                let no = self.ops.new_dynamic_label();
                dynasm!(self.ops ; .arch aarch64 ; cbz W(r(t)), =>no);
                self.field(Access::Ldr32, 0, layout::FLAGS);
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
        let gp = self.gp0();
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
        self.counts();
        match eip {
            Some(eip) if self.link && self.data.in_page(eip) => {
                self.stubs[slot].get_or_insert_with(|| self.ops.new_dynamic_label());
                let at = DATA_LINKS as u32 + slot as u32 * 8;
                dynasm!(self.ops ; .arch aarch64 ; ldr x16, [x1, at] ; br x16);
            }
            Some(_) if self.link => self.guarded(slot, None),
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

    /// Leave through link `slot` to another page, if fetching its target
    /// goes as when the link was made (see `x64::Gen::guarded`), else
    /// through the stub. X1 is the block.
    fn guarded(&mut self, slot: usize, eip: Option<T>) {
        let stub = *self.stubs[slot].get_or_insert_with(|| self.ops.new_dynamic_label());
        let g = DATA_GUARDS as u32 + slot as u32 * GUARD_SIZE as u32;
        let (g_eip, g_cs, g_a20, g_paging, g_page, g_phys) = (
            g + GUARD_EIP as u32,
            g + GUARD_CS_BASE as u32,
            g + GUARD_A20 as u32,
            g + GUARD_PAGING as u32,
            g + GUARD_PAGE as u32,
            g + GUARD_PHYS as u32,
        );
        if let Some(t) = eip {
            dynasm!(self.ops ; .arch aarch64 ; ldr w2, [x1, g_eip] ; cmp W(r(t)), w2 ; b.ne =>stub);
        }
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

    /// Branch to `yes` if condition `cc` holds on the guest's flags.
    fn condition(&mut self, cc: ConditionCode, yes: DynamicLabel) {
        use ConditionCode as C;
        self.field(Access::Ldr32, 0, layout::FLAGS);
        let bits = |cc| match cc {
            C::o | C::no => OF,
            C::b | C::ae => CF,
            C::e | C::ne => ZF,
            C::be | C::a => CF | ZF,
            C::s | C::ns => SF,
            _ => PF,
        };
        match cc {
            C::o | C::b | C::e | C::be | C::s | C::p | C::no | C::ae | C::ne | C::a | C::ns | C::np => {
                self.mov32(1, bits(cc));
                dynasm!(self.ops ; .arch aarch64 ; tst w0, w1);
                if matches!(cc, C::o | C::b | C::e | C::be | C::s | C::p) {
                    dynasm!(self.ops ; .arch aarch64 ; b.ne =>yes);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; b.eq =>yes);
                }
            }
            C::l | C::ge => {
                // SF != OF: OF moved down to SF's bit.
                dynasm!(self.ops ; .arch aarch64 ; eor w1, w0, w0, lsr 4 ; tst w1, SF);
                if cc == C::l {
                    dynasm!(self.ops ; .arch aarch64 ; b.ne =>yes);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; b.eq =>yes);
                }
            }
            _ => {
                // LE: ZF or SF != OF; G: neither.
                dynasm!(self.ops
                    ; .arch aarch64
                    ; eor w1, w0, w0, lsr 4
                    ; and w1, w1, SF
                    ; and w0, w0, ZF
                    ; orr w0, w0, w1
                );
                if cc == C::le {
                    dynasm!(self.ops ; .arch aarch64 ; cbnz w0, =>yes);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; cbz w0, =>yes);
                }
            }
        }
    }
}
