//! The x86-64 code generator.
//!
//! Registers while translated code runs: RBX the CPU, R12 the context
//! (`JitCtx`), R13 RAM, R14 RAM's code generations, R15 set when a store
//! hit the block's later bytes; R8-R11 hold the operations' temporaries
//! (`uop::T`), and RAX, RCX, RDX, RSI and RDI are scratch. The guest's
//! registers and flags stay in the CPU. Translated code calls Rust with the
//! System V convention, which Rust offers on every x86-64 host.

// dynasm converts the registers it is given at run time with `into`.
#![allow(clippy::useless_conversion)]

use dynasmrt::x64::X64Relocation;
use dynasmrt::{DynamicLabel, DynasmApi, DynasmLabelApi, VecAssembler, dynasm};
use iced_x86::ConditionCode;

use super::block::{BlockData, LINKS, RETURN_LINK};
use super::helpers::*;
use super::uop::*;
use crate::cpu::Seg;
use crate::cpu::alu::ShiftOp;
use crate::cpu::layout;

type Asm = VecAssembler<X64Relocation>;

const ICOUNT: i32 = layout::ICOUNT as i32;
const DEADLINE: i32 = layout::DEADLINE as i32;
const EXECUTED: i32 = layout::EXECUTED as i32;
const EIP: i32 = layout::EIP as i32;
const FLAGS: i32 = layout::FLAGS as i32;
const CR0: i32 = layout::CR0 as i32;
const A20: i32 = layout::A20_MASK as i32;
const CPL: i32 = layout::CPL as i32;

/// Flag bits.
const CF: u32 = 0x001;
const PF: u32 = 0x004;
const AF: u32 = 0x010;
const ZF: u32 = 0x040;
const SF: u32 = 0x080;
const OF: u32 = 0x800;
const ARITH: u32 = CF | PF | AF | ZF | SF | OF;
const SZP: u32 = SF | ZF | PF;

/// Where the RAM below the video memory ends, and extended memory starts.
const VIDEO: u32 = 0xA0000;
const EXTENDED: u32 = 0x10_0000;

/// The way in from Rust: `enter(cpu, ctx, code)` saves the registers Rust
/// expects kept, sets up the fixed ones and jumps to `code`; translated
/// code leaves through `exit` with the exit code in EAX and its block in
/// RDX, which it stores in the context before returning the code.
pub struct Trampoline {
    pub bytes: Vec<u8>,
    pub enter: usize,
    pub exit: usize,
}

pub type Enter = unsafe extern "sysv64" fn(*mut crate::cpu::Cpu, *mut JitCtx, *const u8) -> u64;

pub fn trampoline() -> Trampoline {
    let mut ops = Asm::new(0);
    let enter = ops.offset().0;
    dynasm!(ops
        ; .arch x64
        ; push rbx
        ; push rbp
        ; push r12
        ; push r13
        ; push r14
        ; push r15
        // Calls from translated code need RSP 16-byte aligned.
        ; sub rsp, 8
        ; mov rbx, rdi
        ; mov r12, rsi
        ; mov r13, QWORD [r12 + CTX_RAM]
        ; mov r14, QWORD [r12 + CTX_PAGE_GEN]
        ; jmp rdx
    );
    let exit = ops.offset().0;
    dynasm!(ops
        ; .arch x64
        ; mov QWORD [r12 + CTX_EXIT_DATA], rdx
        ; add rsp, 8
        ; pop r15
        ; pop r14
        ; pop r13
        ; pop r12
        ; pop rbp
        ; pop rbx
        ; ret
    );
    Trampoline { bytes: ops.finalize().expect("trampoline"), enter, exit }
}

/// Host register of a temporary.
fn r(t: T) -> u8 {
    8 + t.0
}

fn gpr_offset(g: Gpr) -> i32 {
    (layout::GPR + g.index as usize * 4 + g.high as usize) as i32
}

fn seg_field(seg: Seg, field: usize) -> i32 {
    (layout::SEG + seg as usize * layout::SEG_SIZE + field) as i32
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
    data_ptr: i64,
    /// Per instruction: where it leaves the block with the exit code in
    /// EAX (counting it as executed but not done), and the instruction
    /// count brought up to date for it.
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
/// EIP in the block's page go through its links.
///
/// The prologue checks that the block fits before the timer deadline and
/// that its bytes' chunks haven't been written since it was translated
/// (or if they have, that its bytes are still the same). Before each
/// handler call the instruction count is brought up to date, as devices
/// read the time from it; the count of executed instructions is only
/// brought up to date where the code returns.
pub fn block(data: &BlockData, items: &[Option<Vec<Uop>>], link: bool) -> Code {
    let mut ops = Asm::new(0);
    let n = data.count();
    let fail = (0..n).map(|_| ops.new_dynamic_label()).collect();
    let (tail, deadline, revalidate, body) =
        (ops.new_dynamic_label(), ops.new_dynamic_label(), ops.new_dynamic_label(), ops.new_dynamic_label());
    let limit = ops.new_dynamic_label();
    let mut g = Gen {
        ops,
        data,
        data_ptr: data as *const BlockData as i64,
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
                    let d = ix as i32 - synced;
                    dynasm!(g.ops ; .arch x64 ; add QWORD [rbx + ICOUNT], d);
                    synced = ix as i32;
                }
                g.synced[ix] = synced;
                g.fallback(ix as i32);
            }
            Some(uops) => {
                g.synced[ix] = synced;
                for uop in uops {
                    g.uop(uop);
                }
                if uops.iter().any(|u| matches!(u, Uop::Store { .. })) {
                    // A store hit the rest of the block: leave after this
                    // instruction.
                    let smc = g.ops.new_dynamic_label();
                    let next = data.eips[ix].wrapping_add(data.instrs[ix].len() as u32) as i32;
                    let fail = g.fail[ix];
                    let skip = g.ops.new_dynamic_label();
                    dynasm!(g.ops
                        ; .arch x64
                        ; test r15d, r15d
                        ; jz =>skip
                        ; =>smc
                        ; mov DWORD [rbx + EIP], next
                        ; mov eax, EXIT_SMC as i32
                        ; jmp =>fail
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
    dynasm!(g.ops ; .arch x64 ; =>tail);
    g.leave(None, 0, false);
    g.epilogue();
    let stubs = g.stubs.map(|s| s.map(|l| g.ops.labels().resolve_dynamic(l).expect("stub").0));
    Code { bytes: g.ops.finalize().expect("block"), stubs }
}

impl Gen<'_> {
    fn prologue(&mut self, items: &[Option<Vec<Uop>>]) {
        let data = self.data;
        let n = data.count() as i32;
        let (deadline, revalidate, body) = (self.deadline, self.revalidate, self.body);
        let data_ptr = self.data_ptr;
        let limit = self.limit;
        dynasm!(self.ops
            ; .arch x64
            ; mov rax, QWORD [rbx + ICOUNT]
            ; add rax, n
            ; cmp rax, QWORD [rbx + DEADLINE]
            ; ja =>deadline
            ; cmp DWORD [rbx + seg_field(Seg::CS, layout::SEG_LIMIT)], data.limit_need as i32
            ; jb =>limit
            ; mov rdx, QWORD data_ptr
            ; mov eax, DWORD [r14 + (data.chunk_first * 4) as i32]
        );
        for chunk in data.chunk_first + 1..=data.chunk_last {
            dynasm!(self.ops ; .arch x64 ; add eax, DWORD [r14 + (chunk * 4) as i32]);
        }
        dynasm!(self.ops
            ; .arch x64
            ; cmp eax, DWORD [rdx + DATA_GEN_SUM]
            ; jne =>revalidate
            ; =>body
        );
        if items.iter().flatten().flatten().any(|u| matches!(u, Uop::Store { .. })) {
            dynasm!(self.ops ; .arch x64 ; xor r15d, r15d);
        }
    }

    /// Run instruction `ix` through its handler.
    fn fallback(&mut self, ix: i32) {
        let data_ptr = self.data_ptr;
        let fail = self.fail[ix as usize];
        dynasm!(self.ops
            ; .arch x64
            ; mov rdi, rbx
            ; mov rsi, r12
            ; mov rdx, QWORD data_ptr
            ; mov ecx, ix
            ; call QWORD [r12 + CTX_FALLBACK]
            ; test eax, eax
            ; jnz =>fail
        );
    }

    /// The stubs that leave the block from an instruction, and the slow
    /// paths.
    fn epilogue(&mut self) {
        let data_ptr = self.data_ptr;
        // The prologue's ways out: nothing ran.
        let (deadline, revalidate, limit, body) = (self.deadline, self.revalidate, self.limit, self.body);
        dynasm!(self.ops
            ; .arch x64
            ; =>deadline
            ; mov eax, EXIT_DEADLINE as i32
            ; mov rdx, QWORD data_ptr
            ; jmp QWORD [r12 + CTX_EXIT]
            ; =>limit
            ; mov eax, EXIT_LIMIT as i32
            ; mov rdx, QWORD data_ptr
            ; jmp QWORD [r12 + CTX_EXIT]
            ; =>revalidate
            ; mov rdi, rbx
            ; mov rsi, r12
            ; call QWORD [r12 + CTX_REVALIDATE]
            ; test eax, eax
            ; jz =>body
            ; mov eax, EXIT_STALE as i32
            ; mov rdx, QWORD data_ptr
            ; jmp QWORD [r12 + CTX_EXIT]
        );
        // The links' stubs: back to the execution loop to be linked.
        for (k, stub) in self.stubs.iter().enumerate() {
            if let Some(stub) = *stub {
                dynasm!(self.ops
                    ; .arch x64
                    ; =>stub
                    ; mov eax, (EXIT_UNLINKED | (k as u32) << 8) as i32
                    ; jmp QWORD [r12 + CTX_EXIT]
                );
            }
        }
        for ix in 0..self.data.count() {
            // Instruction ix stopped the block: it counts as executed (the
            // interpreter counts it before running it) but not in the
            // instruction count, which the execution loop adds once it has
            // dealt with it.
            let (label, d, e) = (self.fail[ix], ix as i32 - self.synced[ix], ix as i32 + 1);
            dynasm!(self.ops
                ; .arch x64
                ; =>label
                ; add QWORD [rbx + ICOUNT], d
                ; add QWORD [rbx + EXECUTED], e
                ; or eax, (ix as i32) << 8
                ; mov rdx, QWORD data_ptr
                ; jmp QWORD [r12 + CTX_EXIT]
            );
        }
        for slow in std::mem::take(&mut self.slow) {
            match slow {
                Slow::MemRef { at, back, t, desc, fail } => {
                    dynasm!(self.ops
                        ; .arch x64
                        ; =>at
                        ; push r8
                        ; push r9
                        ; push r10
                        ; push r11
                        ; mov rdi, rbx
                        ; mov rsi, r12
                        ; mov edx, Rd(r(t))
                        ; mov ecx, desc as i32
                        ; call QWORD [r12 + CTX_MEMREF]
                        ; pop r11
                        ; pop r10
                        ; pop r9
                        ; pop r8
                        ; test rax, rax
                        ; js >fault
                        ; mov Rd(r(t)), eax
                        ; jmp =>back
                        ; fault:
                        ; mov eax, EXIT_FAULT as i32
                        ; jmp =>fail
                    );
                }
                Slow::Load { at, back, dst, m } => {
                    dynasm!(self.ops
                        ; .arch x64
                        ; =>at
                        ; push r8
                        ; push r9
                        ; push r10
                        ; push r11
                        ; mov rdi, rbx
                        ; mov rsi, r12
                        ; mov edx, Rd(r(m))
                        ; and edx, 3
                        ; call QWORD [r12 + CTX_READ]
                        ; pop r11
                        ; pop r10
                        ; pop r9
                        ; pop r8
                        ; mov Rd(r(dst)), eax
                        ; jmp =>back
                    );
                }
                Slow::Store { at, back, m, src, lo, hi } => {
                    dynasm!(self.ops
                        ; .arch x64
                        ; =>at
                        ; push r8
                        ; push r9
                        ; push r10
                        ; push r11
                        ; mov DWORD [r12 + CTX_SMC_LO], lo as i32
                        ; mov DWORD [r12 + CTX_SMC_HI], hi as i32
                        ; mov rdi, rbx
                        ; mov rsi, r12
                        ; mov edx, Rd(r(m))
                        ; and edx, 3
                        ; mov ecx, Rd(r(src))
                        ; call QWORD [r12 + CTX_WRITE]
                        ; pop r11
                        ; pop r10
                        ; pop r9
                        ; pop r8
                        ; or r15d, eax
                        ; jmp =>back
                    );
                }
            }
        }
    }

    /// Instruction ix's #GP(0) exit.
    fn gp0(&mut self) -> DynamicLabel {
        let label = self.ops.new_dynamic_label();
        let fail = self.fail[self.ix];
        let skip = self.ops.new_dynamic_label();
        dynasm!(self.ops
            ; .arch x64
            ; jmp =>skip
            ; =>label
            ; mov eax, EXIT_GP0 as i32
            ; jmp =>fail
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
                let (t, off) = (r(t), gpr_offset(g));
                match g.size {
                    4 => dynasm!(self.ops ; .arch x64 ; mov Rd(t), DWORD [rbx + off]),
                    2 => dynasm!(self.ops ; .arch x64 ; movzx Rd(t), WORD [rbx + off]),
                    _ => dynasm!(self.ops ; .arch x64 ; movzx Rd(t), BYTE [rbx + off]),
                }
            }
            Uop::Set { r: g, t } => {
                let (t, off) = (r(t), gpr_offset(g));
                match g.size {
                    4 => dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + off], Rd(t)),
                    2 => dynasm!(self.ops ; .arch x64 ; mov WORD [rbx + off], Rw(t)),
                    _ => dynasm!(self.ops ; .arch x64 ; mov BYTE [rbx + off], Rb(t)),
                }
            }
            Uop::Const { t, v } => dynasm!(self.ops ; .arch x64 ; mov Rd(r(t)), v as i32),
            Uop::Copy { dst, src } => dynasm!(self.ops ; .arch x64 ; mov Rd(r(dst)), Rd(r(src))),
            Uop::AddConst { t, v, size } => {
                let t = r(t);
                dynasm!(self.ops ; .arch x64 ; add Rd(t), v as i32);
                if size == 2 {
                    dynasm!(self.ops ; .arch x64 ; movzx Rd(t), Rw(t));
                }
            }
            Uop::SarConst { t, count } => dynasm!(self.ops ; .arch x64 ; sar Rd(r(t)), count as i8),
            Uop::Extend { t, from, signed } => {
                let t = r(t);
                match (from, signed) {
                    (1, false) => dynasm!(self.ops ; .arch x64 ; movzx Rd(t), Rb(t)),
                    (1, true) => dynasm!(self.ops ; .arch x64 ; movsx Rd(t), Rb(t)),
                    (_, false) => dynasm!(self.ops ; .arch x64 ; movzx Rd(t), Rw(t)),
                    (_, true) => dynasm!(self.ops ; .arch x64 ; movsx Rd(t), Rw(t)),
                }
            }
            Uop::Ea { t, base, index, scale, disp, a32 } => {
                let t = r(t);
                dynasm!(self.ops ; .arch x64 ; mov Rd(t), disp as i32);
                if let Some(b) = base {
                    self.load_eax(b);
                    dynasm!(self.ops ; .arch x64 ; add Rd(t), eax);
                }
                if let Some(i) = index {
                    self.load_eax(i);
                    if scale > 1 {
                        dynasm!(self.ops ; .arch x64 ; shl eax, scale.trailing_zeros() as i8);
                    }
                    dynasm!(self.ops ; .arch x64 ; add Rd(t), eax);
                }
                if !a32 {
                    dynasm!(self.ops ; .arch x64 ; movzx Rd(t), Rw(t));
                }
            }
            Uop::MemRef { t, seg, size, write, slot } => self.memref(t, seg, size, write, slot),
            Uop::Load { dst, m, size } => {
                let (at, back) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
                let (d, m_) = (r(dst), r(m));
                dynasm!(self.ops ; .arch x64 ; cmp Rd(m_), SLOW as i32 ; jae =>at);
                match size {
                    1 => dynasm!(self.ops ; .arch x64 ; movzx Rd(d), BYTE [r13 + Rq(m_)]),
                    2 => dynasm!(self.ops ; .arch x64 ; movzx Rd(d), WORD [r13 + Rq(m_)]),
                    _ => dynasm!(self.ops ; .arch x64 ; mov Rd(d), DWORD [r13 + Rq(m_)]),
                }
                dynasm!(self.ops ; .arch x64 ; =>back);
                self.slow.push(Slow::Load { at, back, dst, m });
            }
            Uop::Store { m, src, size } => {
                let (at, back) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
                let (m_, s) = (r(m), r(src));
                dynasm!(self.ops ; .arch x64 ; cmp Rd(m_), SLOW as i32 ; jae =>at);
                match size {
                    1 => dynasm!(self.ops ; .arch x64 ; mov BYTE [r13 + Rq(m_)], Rb(s)),
                    2 => dynasm!(self.ops ; .arch x64 ; mov WORD [r13 + Rq(m_)], Rw(s)),
                    _ => dynasm!(self.ops ; .arch x64 ; mov DWORD [r13 + Rq(m_)], Rd(s)),
                }
                // The code generations of the first and last byte's chunks,
                // as the bus's writes bump them.
                dynasm!(self.ops
                    ; .arch x64
                    ; mov eax, Rd(m_)
                    ; shr eax, crate::bus::GEN_SHIFT as i8
                    ; add DWORD [r14 + rax * 4], 1
                );
                if size > 1 {
                    dynasm!(self.ops
                        ; .arch x64
                        ; lea eax, [Rq(m_) + (size - 1) as i32]
                        ; shr eax, crate::bus::GEN_SHIFT as i8
                        ; add DWORD [r14 + rax * 4], 1
                    );
                }
                let (lo, hi) = self.rest();
                if lo < hi {
                    dynasm!(self.ops
                        ; .arch x64
                        ; cmp Rd(m_), hi as i32
                        ; jae =>back
                        ; lea eax, [Rq(m_) + size as i32]
                        ; cmp eax, lo as i32
                        ; jbe =>back
                        ; mov r15d, 1
                    );
                }
                dynasm!(self.ops ; .arch x64 ; =>back);
                self.slow.push(Slow::Store { at, back, m, src, lo, hi });
            }
            Uop::Alu { op, size, a, b } => self.alu(op, size, a, b),
            Uop::Unary { op, size, t } => self.unary(op, size, t),
            Uop::Shift { op, size, t, count } => self.shift(op, size, t, count),
            Uop::DoubleShift { left, size, dst, src, count } => self.double_shift(left, size, dst, src, count),
            Uop::Imul { size, a, b } => {
                let a = r(a);
                match (size, b) {
                    (2, Src::T(b)) => dynasm!(self.ops ; .arch x64 ; imul Rw(a), Rw(r(b))),
                    (_, Src::T(b)) => dynasm!(self.ops ; .arch x64 ; imul Rd(a), Rd(r(b))),
                    (2, Src::Imm(v)) => dynasm!(self.ops ; .arch x64 ; imul Rw(a), Rw(a), v as i16),
                    (_, Src::Imm(v)) => dynasm!(self.ops ; .arch x64 ; imul Rd(a), Rd(a), v as i32),
                }
                // CF and OF: the product doesn't fit.
                self.host_flags();
                self.merge(CF | OF, CF | OF);
            }
            Uop::MulWide { signed, size, t } => self.mul_wide(signed, size, t),
            Uop::Flag { mask, set } => match set {
                Some(true) => dynasm!(self.ops ; .arch x64 ; or DWORD [rbx + FLAGS], mask as i32),
                Some(false) => dynasm!(self.ops ; .arch x64 ; and DWORD [rbx + FLAGS], !mask as i32),
                None => dynasm!(self.ops ; .arch x64 ; xor DWORD [rbx + FLAGS], mask as i32),
            },
            Uop::CheckLimit { src } => {
                self.value_eax(src);
                let gp = self.gp0();
                dynasm!(self.ops ; .arch x64 ; cmp eax, DWORD [rbx + seg_field(Seg::CS, layout::SEG_LIMIT)] ; ja =>gp);
            }
            Uop::Exit { eip: Src::Imm(target) } => self.leave(Some(target), 0, true),
            Uop::Exit { eip: Src::T(t) } => {
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + EIP], Rd(r(t)));
                if self.link {
                    // A return: through its link if it goes where the link
                    // was made to.
                    self.counts();
                    self.guarded(RETURN_LINK, Some(t));
                } else {
                    let tail = self.tail;
                    dynasm!(self.ops ; .arch x64 ; jmp =>tail);
                }
            }
            Uop::ExitIf { cond, taken, next, commit } => self.exit_if(cond, taken, next, commit),
        }
    }

    /// EAX = the register, zero-extended.
    fn load_eax(&mut self, g: Gpr) {
        let off = gpr_offset(g);
        match g.size {
            4 => dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + off]),
            2 => dynasm!(self.ops ; .arch x64 ; movzx eax, WORD [rbx + off]),
            _ => dynasm!(self.ops ; .arch x64 ; movzx eax, BYTE [rbx + off]),
        }
    }

    fn value_eax(&mut self, src: Src) {
        match src {
            Src::T(t) => dynasm!(self.ops ; .arch x64 ; mov eax, Rd(r(t))),
            Src::Imm(v) => dynasm!(self.ops ; .arch x64 ; mov eax, v as i32),
        }
    }

    /// Check the operand at seg:t as `Cpu::mem_ref` does, inline for plain
    /// RAM in one page with paging off, and leave its handle in t.
    fn memref(&mut self, t: T, seg: Seg, size: u8, write: bool, slot: u8) {
        let (at, back) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        let t_ = r(t);
        let need = if write { layout::RIGHT_WRITE } else { layout::RIGHT_READ };
        let (lo, hi, rights, base) = (
            seg_field(seg, layout::SEG_LO),
            seg_field(seg, layout::SEG_HI),
            seg_field(seg, layout::SEG_RIGHTS),
            seg_field(seg, layout::SEG_BASE),
        );
        let last = size as i32 - 1;
        let tag = (if write { layout::TLB_WRITE_TAG } else { layout::TLB_READ_TAG }) as i32;
        dynasm!(self.ops
            ; .arch x64
            // The segment's limit and type, as `seg_linear` checks them.
            ; lea ecx, [Rq(t_) + last]
            ; cmp ecx, Rd(t_)
            ; jb =>at
            ; cmp Rd(t_), DWORD [rbx + lo]
            ; jb =>at
            ; cmp ecx, DWORD [rbx + hi]
            ; ja =>at
            ; test BYTE [rbx + rights], need as i8
            ; jz =>at
            ; mov eax, Rd(t_)
            ; add eax, DWORD [rbx + base]
            // Paging on (CR0.PG is the sign bit): through the TLB.
            ; cmp DWORD [rbx + CR0], 0
            ; jl >paging
            ; and eax, DWORD [rbx + A20]
            ; jmp >physical
            // The entry of the page (linear address >> 12) in the set of
            // the privilege level: its tag must be the page + 1.
            ; paging:
            ; mov ecx, eax
            ; shr ecx, 12
            ; mov edx, ecx
            ; and edx, (layout::TLB_SET - 1) as i32
            ; cmp BYTE [rbx + CPL], 3
            ; jne >supervisor
            ; add edx, layout::TLB_SET as i32
            ; supervisor:
            ; imul edx, edx, layout::TLB_ENTRY_SIZE as i32
            ; add rdx, QWORD [r12 + CTX_TLB]
            ; inc ecx
            ; cmp ecx, DWORD [rdx + tag]
            ; jne =>at
            ; and eax, 0xFFF
            ; or eax, DWORD [rdx + layout::TLB_PHYS as i32]
            ; and eax, DWORD [rbx + A20]
            // Within a page, in plain RAM.
            ; physical:
            ; mov ecx, eax
            ; and ecx, 0xFFF
            ; cmp ecx, 0x1000 - size as i32
            ; ja =>at
            ; lea rcx, [rax + size as i32]
            ; cmp rcx, VIDEO as i32
            ; jbe >ram
            ; cmp eax, EXTENDED as i32
            ; jb =>at
            ; cmp rcx, QWORD [r12 + CTX_RAM_LEN]
            ; ja =>at
            ; ram:
            ; mov Rd(t_), eax
            ; =>back
        );
        let desc = memref_desc(seg, size, write, slot);
        self.slow.push(Slow::MemRef { at, back, t, desc, fail: self.fail[self.ix] });
    }

    /// Merge the host's flags (in EAX, from PUSHF) into the guest's: the
    /// bits in `mask` become those of EAX & `bits`.
    fn merge(&mut self, mask: u32, bits: u32) {
        dynasm!(self.ops
            ; .arch x64
            ; and eax, bits as i32
            ; mov ecx, DWORD [rbx + FLAGS]
            ; and ecx, !mask as i32
            ; or ecx, eax
            ; mov DWORD [rbx + FLAGS], ecx
        );
    }

    fn host_flags(&mut self) {
        dynasm!(self.ops ; .arch x64 ; pushfq ; pop rax);
    }

    fn alu(&mut self, op: AluOp, size: u8, a: T, b: Src) {
        let a = r(a);
        if matches!(op, AluOp::Adc | AluOp::Sbb) {
            // The guest's carry in.
            dynasm!(self.ops ; .arch x64 ; bt DWORD [rbx + FLAGS], 0);
        }
        macro_rules! op {
            ($m:ident) => {
                match (size, b) {
                    (1, Src::T(b)) => dynasm!(self.ops ; .arch x64 ; $m Rb(a), Rb(r(b))),
                    (2, Src::T(b)) => dynasm!(self.ops ; .arch x64 ; $m Rw(a), Rw(r(b))),
                    (_, Src::T(b)) => dynasm!(self.ops ; .arch x64 ; $m Rd(a), Rd(r(b))),
                    (1, Src::Imm(v)) => dynasm!(self.ops ; .arch x64 ; $m Rb(a), BYTE v as i8),
                    (2, Src::Imm(v)) => dynasm!(self.ops ; .arch x64 ; $m Rw(a), WORD v as i16),
                    (_, Src::Imm(v)) => dynasm!(self.ops ; .arch x64 ; $m Rd(a), DWORD v as i32),
                }
            };
        }
        match op {
            AluOp::Add => op!(add),
            AluOp::Or => op!(or),
            AluOp::Adc => op!(adc),
            AluOp::Sbb => op!(sbb),
            AluOp::And => op!(and),
            AluOp::Sub => op!(sub),
            AluOp::Xor => op!(xor),
            AluOp::Cmp => op!(cmp),
            AluOp::Test => op!(test),
        }
        self.host_flags();
        match op {
            // The logic operations clear CF, OF and AF.
            AluOp::And | AluOp::Or | AluOp::Xor | AluOp::Test => self.merge(ARITH, SZP),
            _ => self.merge(ARITH, ARITH),
        }
    }

    fn unary(&mut self, op: UnOp, size: u8, t: T) {
        let t = r(t);
        macro_rules! op {
            ($m:ident) => {
                match size {
                    1 => dynasm!(self.ops ; .arch x64 ; $m Rb(t)),
                    2 => dynasm!(self.ops ; .arch x64 ; $m Rw(t)),
                    _ => dynasm!(self.ops ; .arch x64 ; $m Rd(t)),
                }
            };
        }
        match op {
            UnOp::Inc => op!(inc),
            UnOp::Dec => op!(dec),
            UnOp::Neg => op!(neg),
            UnOp::Not => {
                op!(not);
                return;
            }
        }
        self.host_flags();
        match op {
            // INC and DEC leave CF.
            UnOp::Inc | UnOp::Dec => self.merge(ARITH & !CF, ARITH & !CF),
            _ => self.merge(ARITH, ARITH),
        }
    }

    /// Shifts and rotates by 1 to width - 1, with the flags `alu_shift`
    /// gives them where the host's are undefined.
    fn shift(&mut self, op: ShiftOp, size: u8, t: T, count: u8) {
        let t = r(t);
        let c = count as i8;
        macro_rules! op {
            ($m:ident) => {
                match size {
                    1 => dynasm!(self.ops ; .arch x64 ; $m Rb(t), c),
                    2 => dynasm!(self.ops ; .arch x64 ; $m Rw(t), c),
                    _ => dynasm!(self.ops ; .arch x64 ; $m Rd(t), c),
                }
            };
        }
        match op {
            ShiftOp::Shl => op!(shl),
            ShiftOp::Shr => op!(shr),
            ShiftOp::Sar => op!(sar),
            ShiftOp::Rol => op!(rol),
            ShiftOp::Ror => op!(ror),
            ShiftOp::Rcl | ShiftOp::Rcr => unreachable!("not translated"),
        }
        self.host_flags();
        let top = size as i8 * 8 - 1;
        match op {
            ShiftOp::Shl => {
                // OF = the result's sign ^ CF; AF set.
                dynasm!(self.ops
                    ; .arch x64
                    ; mov ecx, eax
                    ; shr ecx, 7
                    ; xor ecx, eax
                    ; and ecx, 1
                    ; shl ecx, 11
                    ; and eax, (CF | SZP) as i32
                    ; or eax, ecx
                    ; or eax, AF as i32
                );
                self.merge(ARITH, ARITH);
            }
            ShiftOp::Shr => {
                // OF is the original sign for a count of 1, else 0; AF set.
                let of = if count == 1 { OF } else { 0 };
                dynasm!(self.ops
                    ; .arch x64
                    ; and eax, (CF | SZP | of) as i32
                    ; or eax, AF as i32
                );
                self.merge(ARITH, ARITH);
            }
            // OF clear, AF kept.
            ShiftOp::Sar => self.merge(CF | OF | SZP, CF | SZP),
            ShiftOp::Rol => {
                // OF = the result's top bit ^ CF (its bottom bit).
                dynasm!(self.ops
                    ; .arch x64
                    ; mov ecx, Rd(t)
                    ; shr ecx, top
                    ; xor ecx, eax
                    ; and ecx, 1
                    ; shl ecx, 11
                    ; and eax, CF as i32
                    ; or eax, ecx
                );
                self.merge(CF | OF, CF | OF);
            }
            _ => {
                // ROR: OF = the result's top two bits differ.
                dynasm!(self.ops
                    ; .arch x64
                    ; mov ecx, Rd(t)
                    ; mov edx, ecx
                    ; shr ecx, top
                    ; shr edx, top - 1
                    ; xor ecx, edx
                    ; and ecx, 1
                    ; shl ecx, 11
                    ; and eax, CF as i32
                    ; or eax, ecx
                );
                self.merge(CF | OF, CF | OF);
            }
        }
    }

    /// SHLD and SHRD by 1 to width - 1: the host's result, CF, SF, ZF and
    /// PF; OF as `alu_double_shift` has it; AF kept.
    fn double_shift(&mut self, left: bool, size: u8, dst: T, src: T, count: u8) {
        let (d, s, c) = (r(dst), r(src), count as i8);
        match (left, size) {
            (true, 2) => dynasm!(self.ops ; .arch x64 ; shld Rw(d), Rw(s), c),
            (true, _) => dynasm!(self.ops ; .arch x64 ; shld Rd(d), Rd(s), c),
            (false, 2) => dynasm!(self.ops ; .arch x64 ; shrd Rw(d), Rw(s), c),
            (false, _) => dynasm!(self.ops ; .arch x64 ; shrd Rd(d), Rd(s), c),
        }
        self.host_flags();
        let top = size as i8 * 8 - 1;
        if left {
            // OF = the result's sign ^ CF.
            dynasm!(self.ops
                ; .arch x64
                ; mov ecx, eax
                ; shr ecx, 7
                ; xor ecx, eax
            );
        } else {
            // OF = the result's top two bits differ.
            dynasm!(self.ops
                ; .arch x64
                ; mov ecx, Rd(d)
                ; mov edx, ecx
                ; shr ecx, top
                ; shr edx, top - 1
                ; xor ecx, edx
            );
        }
        dynasm!(self.ops
            ; .arch x64
            ; and ecx, 1
            ; shl ecx, 11
            ; and eax, (CF | SZP) as i32
            ; or eax, ecx
        );
        self.merge(CF | OF | SZP, CF | OF | SZP);
    }

    /// MUL or IMUL of AL, AX or EAX by t into AX, DX:AX or EDX:EAX, with the
    /// host's CF and OF.
    fn mul_wide(&mut self, signed: bool, size: u8, t: T) {
        let t = r(t);
        let (acc, high) = (gpr_offset(Gpr::dword(0)), gpr_offset(Gpr::dword(2)));
        match size {
            1 => dynasm!(self.ops ; .arch x64 ; movzx eax, BYTE [rbx + acc]),
            2 => dynasm!(self.ops ; .arch x64 ; movzx eax, WORD [rbx + acc]),
            _ => dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + acc]),
        }
        match (signed, size) {
            (false, 1) => dynasm!(self.ops ; .arch x64 ; mul Rb(t)),
            (false, 2) => dynasm!(self.ops ; .arch x64 ; mul Rw(t)),
            (false, _) => dynasm!(self.ops ; .arch x64 ; mul Rd(t)),
            (true, 1) => dynasm!(self.ops ; .arch x64 ; imul Rb(t)),
            (true, 2) => dynasm!(self.ops ; .arch x64 ; imul Rw(t)),
            (true, _) => dynasm!(self.ops ; .arch x64 ; imul Rd(t)),
        }
        dynasm!(self.ops ; .arch x64 ; pushfq);
        match size {
            1 => dynasm!(self.ops ; .arch x64 ; mov WORD [rbx + acc], ax),
            2 => dynasm!(self.ops ; .arch x64 ; mov WORD [rbx + acc], ax ; mov WORD [rbx + high], dx),
            _ => dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + acc], eax ; mov DWORD [rbx + high], edx),
        }
        dynasm!(self.ops ; .arch x64 ; pop rax);
        self.merge(CF | OF, CF | OF);
    }

    fn exit_if(&mut self, cond: Cond, taken: u32, next: u32, commit: Option<(Gpr, T)>) {
        let yes = self.ops.new_dynamic_label();
        match cond {
            Cond::Flags(cc) => self.condition(cc, yes),
            Cond::Zero(t) => dynasm!(self.ops ; .arch x64 ; test Rd(r(t)), Rd(r(t)) ; jz =>yes),
            Cond::NonZero(t) => dynasm!(self.ops ; .arch x64 ; test Rd(r(t)), Rd(r(t)) ; jnz =>yes),
            Cond::NonZeroZf(t, zf) => {
                let no = self.ops.new_dynamic_label();
                dynasm!(self.ops
                    ; .arch x64
                    ; test Rd(r(t)), Rd(r(t))
                    ; jz =>no
                    ; test DWORD [rbx + FLAGS], ZF as i32
                );
                if zf {
                    dynasm!(self.ops ; .arch x64 ; jnz =>yes);
                } else {
                    dynasm!(self.ops ; .arch x64 ; jz =>yes);
                }
                dynasm!(self.ops ; .arch x64 ; =>no);
            }
        }
        // Not taken.
        self.commit(commit);
        self.leave(Some(next), 1, true);
        dynasm!(self.ops ; .arch x64 ; =>yes);
        // Taken: the target must be within the CS limit.
        let gp = self.gp0();
        dynasm!(self.ops
            ; .arch x64
            ; mov eax, taken as i32
            ; cmp eax, DWORD [rbx + seg_field(Seg::CS, layout::SEG_LIMIT)]
            ; ja =>gp
        );
        self.commit(commit);
        self.leave(Some(taken), 0, true);
    }

    /// Leave the block after its last instruction, at `eip` if the code
    /// sets it (`set`) or knows it: through link `slot` if that is in the
    /// page, else back to the execution loop. The counts first.
    fn leave(&mut self, eip: Option<u32>, slot: usize, set: bool) {
        if let (Some(eip), true) = (eip, set) {
            dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + EIP], eip as i32);
        }
        self.counts();
        match eip {
            Some(eip) if self.link && self.data.in_page(eip) => {
                self.stubs[slot].get_or_insert_with(|| self.ops.new_dynamic_label());
                dynasm!(self.ops ; .arch x64 ; jmp QWORD [rdx + DATA_LINKS + slot as i32 * 8]);
            }
            Some(_) if self.link => self.guarded(slot, None),
            _ => dynasm!(self.ops ; .arch x64 ; mov eax, EXIT_NEXT as i32 ; jmp QWORD [r12 + CTX_EXIT]),
        }
    }

    /// Bring the counts up to date for leaving the block after its last
    /// instruction, and RDX = the block.
    fn counts(&mut self) {
        let data = self.data;
        let (n, synced) = (data.count() as i32, self.synced[data.count() - 1]);
        let data_ptr = self.data_ptr;
        dynasm!(self.ops
            ; .arch x64
            ; add QWORD [rbx + ICOUNT], n - synced
            ; add QWORD [rbx + EXECUTED], n
            ; mov rdx, QWORD data_ptr
        );
    }

    /// Leave through link `slot` to another page, if fetching its target
    /// goes as when the link was made (its `Guard`): the same EIP for a
    /// return (in `eip`), CS base, A20 gate and paging, and with paging
    /// the target page's translation still in the TLB. Otherwise through
    /// the stub, to the execution loop. RDX is the block.
    fn guarded(&mut self, slot: usize, eip: Option<T>) {
        let stub = *self.stubs[slot].get_or_insert_with(|| self.ops.new_dynamic_label());
        let g = DATA_GUARDS + slot as i32 * GUARD_SIZE;
        if let Some(t) = eip {
            dynasm!(self.ops ; .arch x64 ; cmp Rd(r(t)), DWORD [rdx + g + GUARD_EIP] ; jne =>stub);
        }
        dynasm!(self.ops
            ; .arch x64
            ; mov ecx, DWORD [rbx + seg_field(Seg::CS, layout::SEG_BASE)]
            ; cmp ecx, DWORD [rdx + g + GUARD_CS_BASE]
            ; jne =>stub
            ; mov ecx, DWORD [rbx + A20]
            ; cmp ecx, DWORD [rdx + g + GUARD_A20]
            ; jne =>stub
            ; mov ecx, DWORD [rbx + CR0]
            ; shr ecx, 31
            ; cmp ecx, DWORD [rdx + g + GUARD_PAGING]
            ; jne =>stub
            ; test ecx, ecx
            ; jz >go
            // The TLB entry of the page in the set of the privilege level.
            ; mov ecx, DWORD [rdx + g + GUARD_PAGE]
            ; and ecx, (layout::TLB_SET - 1) as i32
            ; cmp BYTE [rbx + CPL], 3
            ; jne >supervisor
            ; add ecx, layout::TLB_SET as i32
            ; supervisor:
            ; imul ecx, ecx, layout::TLB_ENTRY_SIZE as i32
            ; add rcx, QWORD [r12 + CTX_TLB]
            ; mov esi, DWORD [rdx + g + GUARD_PAGE]
            ; inc esi
            ; cmp esi, DWORD [rcx + layout::TLB_READ_TAG as i32]
            ; jne =>stub
            ; mov esi, DWORD [rcx + layout::TLB_PHYS as i32]
            ; cmp esi, DWORD [rdx + g + GUARD_PHYS]
            ; jne =>stub
            ; go:
            ; jmp QWORD [rdx + DATA_LINKS + slot as i32 * 8]
        );
    }

    fn commit(&mut self, commit: Option<(Gpr, T)>) {
        if let Some((g, t)) = commit {
            self.uop(&Uop::Set { r: g, t });
        }
    }

    /// Jump to `yes` if condition `cc` holds on the guest's flags.
    fn condition(&mut self, cc: ConditionCode, yes: DynamicLabel) {
        use ConditionCode as C;
        let bits = |cc| match cc {
            C::o | C::no => OF,
            C::b | C::ae => CF,
            C::e | C::ne => ZF,
            C::be | C::a => CF | ZF,
            C::s | C::ns => SF,
            _ => PF,
        };
        match cc {
            C::o | C::b | C::e | C::be | C::s | C::p => {
                dynasm!(self.ops ; .arch x64 ; test DWORD [rbx + FLAGS], bits(cc) as i32 ; jnz =>yes)
            }
            C::no | C::ae | C::ne | C::a | C::ns | C::np => {
                dynasm!(self.ops ; .arch x64 ; test DWORD [rbx + FLAGS], bits(cc) as i32 ; jz =>yes)
            }
            C::l | C::ge => {
                // SF != OF: OF moved down to SF's bit.
                dynasm!(self.ops
                    ; .arch x64
                    ; mov eax, DWORD [rbx + FLAGS]
                    ; mov ecx, eax
                    ; shr ecx, 4
                    ; xor ecx, eax
                    ; test ecx, SF as i32
                );
                if cc == C::l {
                    dynasm!(self.ops ; .arch x64 ; jnz =>yes);
                } else {
                    dynasm!(self.ops ; .arch x64 ; jz =>yes);
                }
            }
            _ => {
                // LE: ZF or SF != OF; G: neither.
                dynasm!(self.ops
                    ; .arch x64
                    ; mov eax, DWORD [rbx + FLAGS]
                    ; mov ecx, eax
                    ; shr ecx, 4
                    ; xor ecx, eax
                    ; and ecx, SF as i32
                    ; and eax, ZF as i32
                    ; or eax, ecx
                );
                if cc == C::le {
                    dynasm!(self.ops ; .arch x64 ; jnz =>yes);
                } else {
                    dynasm!(self.ops ; .arch x64 ; jz =>yes);
                }
            }
        }
    }
}
