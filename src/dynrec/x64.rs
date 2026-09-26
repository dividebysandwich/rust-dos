//! The x86-64 code generator.
//!
//! Registers while translated code runs: RBX the CPU, R12 the context
//! (`JitCtx`), R13 RAM, R14 RAM's code generations, R15 set when a store
//! hit the block's later bytes, EBP the guest's arithmetic flags where the
//! code has changed them (see `Gen::dirty`); R8-R11 hold the operations'
//! temporaries (`uop::T`), and RAX, RCX, RDX, RSI and RDI are scratch. The
//! guest's registers stay in the CPU. Translated code calls Rust with the
//! System V convention, which Rust offers on every x86-64 host.

// dynasm converts the registers it is given at run time with `into`.
#![allow(clippy::useless_conversion)]

use dynasmrt::x64::X64Relocation;
use dynasmrt::{DynamicLabel, DynasmApi, DynasmLabelApi, VecAssembler, dynasm};
use iced_x86::ConditionCode;

use super::block::{BlockData, LINKS, RETURN_LINK, RETURN_MISS};
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
/// RDX, which it stores in the context with EBP (the flags, for
/// `EXIT_FLAGS`) before returning the code.
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
        ; mov DWORD [r12 + CTX_FLAGS], ebp
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

/// Whether the host has LAHF in 64-bit mode.
fn has_lahf() -> bool {
    static LAHF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    #[allow(unused_unsafe)]
    // SAFETY: CPUID leaf 8000_0001h is there on every x86-64 processor.
    *LAHF.get_or_init(|| unsafe { std::arch::x86_64::__cpuid(0x8000_0001) }.ecx & 1 != 0)
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
    /// A memory operand's linear address in EAX with paging on: its
    /// physical address through the TLB, on to `back`, or to `miss` for
    /// `jit_memref`.
    Paging { at: DynamicLabel, back: DynamicLabel, miss: DynamicLabel, write: bool },
    /// An instruction's fault with an exit code of its own (#GP(0), #DE).
    Fault { at: DynamicLabel, code: u32, fail: DynamicLabel },
    /// Instruction `ix` runs through its handler after all (`Uop::Bail`),
    /// with the flags in EBP there (`dirty`), and goes on at `end`.
    Bail { at: DynamicLabel, end: DynamicLabel, ix: usize, dirty: bool },
}

struct Gen<'a> {
    ops: Asm,
    data: &'a BlockData,
    data_ptr: i64,
    /// Per instruction: where it stops the block with the exit code in EAX
    /// (made where something jumps there), and how far the instruction
    /// count is brought up to date for it. The stops' way out.
    fail: Vec<Option<DynamicLabel>>,
    synced: Vec<i32>,
    fail_tail: DynamicLabel,
    /// Where the instruction being translated ends (made where something
    /// jumps there), and per instruction whether the flags are in EBP
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
    /// The flags live after each operation (`flags::live`), and after the
    /// one being translated.
    live: Vec<Vec<u32>>,
    live_after: u32,
    /// The guest's arithmetic flags are in EBP, not yet in the CPU (whose
    /// other flags are right). They go back into the CPU where anything
    /// else may read them: where the block leaves and before a handler.
    /// Per instruction, whether they are in EBP where it may stop, before
    /// it changes them; the execution loop puts them back then
    /// (`EXIT_FLAGS`).
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
    let (tail, deadline, revalidate, body) =
        (ops.new_dynamic_label(), ops.new_dynamic_label(), ops.new_dynamic_label(), ops.new_dynamic_label());
    let (limit, fail_tail) = (ops.new_dynamic_label(), ops.new_dynamic_label());
    let mut g = Gen {
        ops,
        data,
        data_ptr: data as *const BlockData as i64,
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
                    let d = ix as i32 - synced;
                    dynasm!(g.ops ; .arch x64 ; add QWORD [rbx + ICOUNT], d);
                    synced = ix as i32;
                }
                g.synced[ix] = synced;
                g.flags_back();
                g.dirty = false;
                g.check_watched();
                g.fallback(ix as i32);
            }
            Some(uops) => {
                g.synced[ix] = synced;
                // Operations check what can fault before they change the
                // flags: they are as at the instruction's start there.
                g.dirty_at[ix] = g.dirty;
                g.check_watched();
                for (k, uop) in uops.iter().enumerate() {
                    g.live_after = g.live[ix][k];
                    g.uop(uop);
                }
                g.end_dirty[ix] = g.dirty;
                if let Some(end) = g.end.take() {
                    dynasm!(g.ops ; .arch x64 ; =>end);
                }
                if uops.iter().any(|u| matches!(u, Uop::Store { .. })) {
                    // A store hit the rest of the block: leave after this
                    // instruction.
                    let smc = g.ops.new_dynamic_label();
                    let next = data.eips[ix].wrapping_add(data.instrs[ix].len() as u32) as i32;
                    let fail = g.fail();
                    let skip = g.ops.new_dynamic_label();
                    dynasm!(g.ops
                        ; .arch x64
                        ; test r15d, r15d
                        ; jz =>skip
                        ; =>smc
                        ; mov DWORD [rbx + EIP], next
                        ; mov eax, (EXIT_SMC | if g.dirty { EXIT_FLAGS } else { 0 }) as i32
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
    // The end: the counts, and back to the execution loop. The code that
    // jumps here has put the flags back.
    dynasm!(g.ops ; .arch x64 ; =>tail);
    g.dirty = false;
    g.leave(None, 0, false);
    g.epilogue();
    let stubs = g.stubs.map(|s| s.map(|l| g.ops.labels().resolve_dynamic(l).expect("stub").0));
    let lag = g.synced.iter().enumerate().map(|(ix, &synced)| (ix as i32 - synced) as u8).collect();
    Code { bytes: g.ops.finalize().expect("block"), stubs, lag }
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

    /// Where the instruction being translated stops the block, with the
    /// exit code in EAX.
    fn fail(&mut self) -> DynamicLabel {
        let ops = &mut self.ops;
        *self.fail[self.ix].get_or_insert_with(|| ops.new_dynamic_label())
    }

    /// Where the instruction being translated ends.
    fn end(&mut self) -> DynamicLabel {
        let ops = &mut self.ops;
        *self.end.get_or_insert_with(|| ops.new_dynamic_label())
    }

    /// Leave the block before the instruction being translated if any of
    /// its watched bytes differ from what was translated.
    fn check_watched(&mut self) {
        let data = self.data;
        let mut changed = None;
        for w in data.watched_in(self.ix) {
            let at = *changed.get_or_insert_with(|| self.fault_exit(EXIT_WATCHED));
            let phys = (data.phys as usize + w) as i32;
            dynasm!(self.ops ; .arch x64 ; cmp BYTE [r13 + phys], data.bytes[w] as i8 ; jne =>at);
        }
    }

    /// Run instruction `ix` through its handler.
    fn fallback(&mut self, ix: i32) {
        let data_ptr = self.data_ptr;
        let fail = self.fail();
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
        let misses = self.return_miss.map(|miss| (RETURN_MISS, miss));
        for (k, stub) in self.stubs.iter().copied().enumerate().filter_map(|(k, s)| Some((k, s?))).chain(misses) {
            dynasm!(self.ops
                ; .arch x64
                ; =>stub
                ; mov eax, (EXIT_UNLINKED | (k as u32) << 8) as i32
                ; jmp QWORD [r12 + CTX_EXIT]
            );
        }
        // An instruction stopped the block: the exit code gets its index,
        // and whether the flags are in EBP, and the execution loop counts
        // it (see `BlockData::lag`).
        let fail_tail = self.fail_tail;
        for (ix, label) in self.fail.clone().into_iter().enumerate() {
            if let Some(label) = label {
                dynasm!(self.ops ; .arch x64 ; =>label);
                let bits = (ix as u32) << 8 | if self.dirty_at[ix] { EXIT_FLAGS } else { 0 };
                if bits != 0 {
                    dynasm!(self.ops ; .arch x64 ; or eax, bits as i32);
                }
                dynasm!(self.ops ; .arch x64 ; jmp =>fail_tail);
            }
        }
        if self.fail.iter().any(Option::is_some) || self.slow.iter().any(|s| matches!(s, Slow::Bail { .. })) {
            dynasm!(self.ops
                ; .arch x64
                ; =>fail_tail
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
                Slow::Paging { at, back, miss, write } => {
                    // The entry of the page (linear address >> 12) in the
                    // set of the privilege level: its tag must be the page
                    // + 1.
                    let tag = (if write { layout::TLB_WRITE_TAG } else { layout::TLB_READ_TAG }) as i32;
                    dynasm!(self.ops
                        ; .arch x64
                        ; =>at
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
                        ; jne =>miss
                        ; and eax, 0xFFF
                        ; or eax, DWORD [rdx + layout::TLB_PHYS as i32]
                        ; and eax, DWORD [rbx + A20]
                        ; jmp =>back
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
                Slow::Fault { at, code, fail } => {
                    dynasm!(self.ops
                        ; .arch x64
                        ; =>at
                        ; mov eax, code as i32
                        ; jmp =>fail
                    );
                }
                Slow::Bail { at, end, ix, dirty } => {
                    // As a handler's call in the block, but with the
                    // instruction count put back after it, as the code on
                    // from `end` has it. A stop leaves the flags the handler
                    // left in the CPU.
                    let lag = ix as i32 - self.synced[ix];
                    dynasm!(self.ops ; .arch x64 ; =>at);
                    self.dirty = dirty;
                    self.flags_back();
                    if lag > 0 {
                        dynasm!(self.ops ; .arch x64 ; add QWORD [rbx + ICOUNT], lag);
                    }
                    dynasm!(self.ops
                        ; .arch x64
                        ; mov rdi, rbx
                        ; mov rsi, r12
                        ; mov rdx, QWORD data_ptr
                        ; mov ecx, ix as i32
                        ; call QWORD [r12 + CTX_FALLBACK]
                    );
                    if lag > 0 {
                        dynasm!(self.ops ; .arch x64 ; sub QWORD [rbx + ICOUNT], lag);
                    }
                    dynasm!(self.ops ; .arch x64 ; test eax, eax ; jnz >stop);
                    if self.end_dirty[ix] {
                        dynasm!(self.ops ; .arch x64 ; mov ebp, DWORD [rbx + FLAGS]);
                    }
                    let fail_tail = self.fail_tail;
                    dynasm!(self.ops
                        ; .arch x64
                        ; jmp =>end
                        ; stop:
                        ; or eax, (ix as i32) << 8
                        ; jmp =>fail_tail
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
                let (t, off) = (r(t), gpr_offset(g));
                match g.size {
                    4 => dynasm!(self.ops ; .arch x64 ; mov Rd(t), DWORD [rbx + off]),
                    2 => dynasm!(self.ops ; .arch x64 ; movzx Rd(t), WORD [rbx + off]),
                    _ => dynasm!(self.ops ; .arch x64 ; movzx Rd(t), BYTE [rbx + off]),
                }
            }
            Uop::Set { r: g, t } => {
                dynasm!(self.ops ; .arch x64 ; mov ecx, Rd(r(t)));
                self.set_ecx(g);
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
                dynasm!(self.ops ; .arch x64 ; test Rd(r(t)), mask as i32 ; jnz =>at);
                let end = self.end();
                self.slow.push(Slow::Bail { at, end, ix: self.ix, dirty: self.dirty });
            }
            Uop::Imul { size, a, b } => {
                let a = r(a);
                match (size, b) {
                    (2, Src::T(b)) => dynasm!(self.ops ; .arch x64 ; imul Rw(a), Rw(r(b))),
                    (_, Src::T(b)) => dynasm!(self.ops ; .arch x64 ; imul Rd(a), Rd(r(b))),
                    (2, Src::Imm(v)) => dynasm!(self.ops ; .arch x64 ; imul Rw(a), Rw(a), v as i16),
                    (_, Src::Imm(v)) => dynasm!(self.ops ; .arch x64 ; imul Rd(a), Rd(a), v as i32),
                }
                // CF and OF: the product doesn't fit.
                if self.wanted(CF | OF) {
                    self.host_flags();
                    self.merge(CF | OF, CF | OF);
                }
            }
            Uop::MulWide { signed, size, t } => self.mul_wide(signed, size, t),
            Uop::DivWide { signed, size, t } => self.div_wide(signed, size, t),
            Uop::SetCond { t, cc } => {
                let t = r(t);
                if self.test_condition(cc, self.dirty) {
                    dynasm!(self.ops ; .arch x64 ; setnz Rb(t));
                } else {
                    dynasm!(self.ops ; .arch x64 ; setz Rb(t));
                }
                dynasm!(self.ops ; .arch x64 ; movzx Rd(t), Rb(t));
            }
            Uop::Flag { mask, set } => {
                // DF is always in the CPU, the arithmetic flags in EBP
                // once the code has changed them.
                let (arith, other) = (mask & ARITH, mask & !ARITH);
                if other != 0 {
                    self.flag_op(other, set, false);
                }
                if arith != 0 && self.wanted(arith) {
                    self.flag_op(arith, set, self.dirty);
                }
            }
            Uop::CheckLimit { src } => {
                self.value_eax(src);
                let gp = self.fault_exit(EXIT_GP0);
                dynasm!(self.ops ; .arch x64 ; cmp eax, DWORD [rbx + seg_field(Seg::CS, layout::SEG_LIMIT)] ; ja =>gp);
            }
            Uop::Exit { eip: Src::Imm(target) } => self.leave(Some(target), 0, true),
            Uop::Exit { eip: Src::T(t) } => {
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + EIP], Rd(r(t)));
                self.flags_back();
                if self.link {
                    // A return: through its link to where it goes, if it
                    // has one.
                    self.counts();
                    self.returned(t);
                } else {
                    let tail = self.tail;
                    dynasm!(self.ops ; .arch x64 ; jmp =>tail);
                }
            }
            Uop::ExitIf { cond, taken, next, commit } => self.exit_if(cond, taken, next, commit),
        }
    }

    /// The register = the low bytes of ECX. The register's whole dword is
    /// written, so that later loads of it are forwarded from one store:
    /// a load of a dword written in bytes waits for the stores to finish.
    /// EAX is changed.
    fn set_ecx(&mut self, g: Gpr) {
        let off = gpr_offset(Gpr::dword(g.index));
        match (g.size, g.high) {
            (4, _) => dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + off], ecx),
            (2, _) => dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + off] ; mov ax, cx ; mov DWORD [rbx + off], eax),
            (_, false) => dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + off] ; mov al, cl ; mov DWORD [rbx + off], eax),
            (_, true) => dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + off] ; mov ah, cl ; mov DWORD [rbx + off], eax),
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
    /// RAM in one page, through the TLB with paging on, and leave its
    /// handle in t.
    fn memref(&mut self, t: T, seg: Seg, size: u8, write: bool, slot: u8) {
        let (at, back) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        let (paging, physical) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        let t_ = r(t);
        let need = if write { layout::RIGHT_WRITE } else { layout::RIGHT_READ };
        let (lo, hi, rights, base) = (
            seg_field(seg, layout::SEG_LO),
            seg_field(seg, layout::SEG_HI),
            seg_field(seg, layout::SEG_RIGHTS),
            seg_field(seg, layout::SEG_BASE),
        );
        let last = size as i32 - 1;
        // The segment's limit and type, as `seg_linear` checks them. A byte
        // is its own last byte, which can't wrap around.
        if size > 1 {
            dynasm!(self.ops
                ; .arch x64
                ; lea ecx, [Rq(t_) + last]
                ; cmp ecx, Rd(t_)
                ; jb =>at
                ; cmp Rd(t_), DWORD [rbx + lo]
                ; jb =>at
                ; cmp ecx, DWORD [rbx + hi]
                ; ja =>at
            );
        } else {
            dynasm!(self.ops
                ; .arch x64
                ; cmp Rd(t_), DWORD [rbx + lo]
                ; jb =>at
                ; cmp Rd(t_), DWORD [rbx + hi]
                ; ja =>at
            );
        }
        dynasm!(self.ops
            ; .arch x64
            ; test BYTE [rbx + rights], need as i8
            ; jz =>at
            ; mov eax, Rd(t_)
            ; add eax, DWORD [rbx + base]
            // Paging on (CR0.PG is the sign bit): through the TLB, out of
            // line.
            ; cmp DWORD [rbx + CR0], 0
            ; jl =>paging
            ; and eax, DWORD [rbx + A20]
            ; =>physical
        );
        // Within a page (a byte always is), in plain RAM: not in the video
        // memory and ROMs from A0000h to FFFFFh, whose ends are on page
        // boundaries, and not past the end of RAM.
        if size > 1 {
            dynasm!(self.ops
                ; .arch x64
                ; mov ecx, eax
                ; and ecx, 0xFFF
                ; cmp ecx, 0x1000 - size as i32
                ; ja =>at
            );
        }
        dynasm!(self.ops
            ; .arch x64
            ; lea ecx, [rax - VIDEO as i32]
            ; cmp ecx, (EXTENDED - VIDEO) as i32
            ; jb =>at
            ; lea rcx, [rax + size as i32]
            ; cmp rcx, QWORD [r12 + CTX_RAM_LEN]
            ; ja =>at
            ; mov Rd(t_), eax
            ; =>back
        );
        let desc = memref_desc(seg, size, write, slot);
        let fail = self.fail();
        self.slow.push(Slow::Paging { at: paging, back: physical, miss: at, write });
        self.slow.push(Slow::MemRef { at, back, t, desc, fail });
    }

    /// Whether any of the flags `set` that the operation sets are live.
    fn wanted(&self, set: u32) -> bool {
        set & self.live_after != 0
    }

    /// Put the guest's arithmetic flags from EBP into the CPU, if they are
    /// there. ECX is changed.
    fn flags_back(&mut self) {
        if self.dirty {
            dynasm!(self.ops
                ; .arch x64
                ; mov ecx, DWORD [rbx + FLAGS]
                ; and ecx, !ARITH as i32
                ; and ebp, ARITH as i32
                ; or ecx, ebp
                ; mov DWORD [rbx + FLAGS], ecx
            );
        }
    }

    /// Host CF = the guest's CF.
    fn carry_in(&mut self) {
        if self.dirty {
            dynasm!(self.ops ; .arch x64 ; bt ebp, 0);
        } else {
            dynasm!(self.ops ; .arch x64 ; bt DWORD [rbx + FLAGS], 0);
        }
    }

    /// Set (Some(true)), clear or complement the flags in `mask`, in EBP or
    /// in the CPU.
    fn flag_op(&mut self, mask: u32, set: Option<bool>, ebp: bool) {
        match (set, ebp) {
            (Some(true), false) => dynasm!(self.ops ; .arch x64 ; or DWORD [rbx + FLAGS], mask as i32),
            (Some(false), false) => dynasm!(self.ops ; .arch x64 ; and DWORD [rbx + FLAGS], !mask as i32),
            (None, false) => dynasm!(self.ops ; .arch x64 ; xor DWORD [rbx + FLAGS], mask as i32),
            (Some(true), true) => dynasm!(self.ops ; .arch x64 ; or ebp, mask as i32),
            (Some(false), true) => dynasm!(self.ops ; .arch x64 ; and ebp, !mask as i32),
            (None, true) => dynasm!(self.ops ; .arch x64 ; xor ebp, mask as i32),
        }
    }

    /// Merge the host's flags (in EAX, from PUSHF) into the guest's in EBP:
    /// the bits in `mask` become those of EAX & `bits`. The others stay,
    /// from the CPU if they aren't in EBP yet and are live.
    fn merge(&mut self, mask: u32, bits: u32) {
        dynasm!(self.ops ; .arch x64 ; and eax, bits as i32);
        if ARITH & !mask & self.live_after == 0 {
            dynasm!(self.ops ; .arch x64 ; mov ebp, eax);
        } else {
            if !self.dirty {
                dynasm!(self.ops ; .arch x64 ; mov ebp, DWORD [rbx + FLAGS]);
            }
            dynasm!(self.ops
                ; .arch x64
                ; and ebp, !mask as i32
                ; or ebp, eax
            );
        }
        self.dirty = true;
    }

    fn host_flags(&mut self) {
        dynasm!(self.ops ; .arch x64 ; pushfq ; pop rax);
    }

    /// The host's flags are the guest's arithmetic flags: into EBP. LAHF
    /// has all but OF, which SETO adds where it is live; PUSHF takes
    /// several times as long. (Some early 64-bit Pentium 4s have no LAHF
    /// in 64-bit mode.) RAX and RCX are changed.
    fn host_flags_ebp(&mut self) {
        if !has_lahf() {
            dynasm!(self.ops ; .arch x64 ; pushfq ; pop rbp);
        } else if self.wanted(OF) {
            dynasm!(self.ops
                ; .arch x64
                ; lahf
                ; seto cl
                ; movzx ebp, ah
                ; movzx ecx, cl
                ; shl ecx, 11
                ; or ebp, ecx
            );
        } else {
            dynasm!(self.ops ; .arch x64 ; lahf ; movzx ebp, ah);
        }
        self.dirty = true;
    }

    fn alu(&mut self, op: AluOp, size: u8, a: T, b: Src) {
        let a = r(a);
        if matches!(op, AluOp::Adc | AluOp::Sbb) {
            self.carry_in();
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
        if !self.wanted(ARITH) {
            return;
        }
        self.host_flags_ebp();
        if matches!(op, AluOp::And | AluOp::Or | AluOp::Xor | AluOp::Test) {
            // The logic operations clear AF (and CF and OF, as the host).
            dynasm!(self.ops ; .arch x64 ; and ebp, !AF as i32);
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
        let wanted = op != UnOp::Not && self.wanted(ARITH);
        if wanted && matches!(op, UnOp::Inc | UnOp::Dec) && self.live_after & CF != 0 {
            // INC and DEC leave CF, the host's as well: make it the guest's.
            self.carry_in();
        }
        match op {
            UnOp::Inc => op!(inc),
            UnOp::Dec => op!(dec),
            UnOp::Neg => op!(neg),
            UnOp::Not => op!(not),
        }
        if wanted {
            self.host_flags_ebp();
        }
    }

    /// The count of a shift by register `count` (CL) & 31 into ECX, and on
    /// to the instruction's end if it is 0: nothing changes then. The
    /// guest's flags are in EBP both ways if the shift's are live.
    fn var_count(&mut self, count: Gpr, set: u32) {
        let end = self.end();
        dynasm!(self.ops ; .arch x64 ; movzx ecx, BYTE [rbx + gpr_offset(count)]);
        if self.wanted(set) && !self.dirty {
            dynasm!(self.ops ; .arch x64 ; mov ebp, DWORD [rbx + FLAGS]);
            self.dirty = true;
        }
        dynasm!(self.ops ; .arch x64 ; and ecx, 31 ; jz =>end);
    }

    /// Shifts and rotates by 1 to width - 1, `count` or CL (None, in ECX),
    /// with the flags `alu_shift` gives them where the host's are
    /// undefined.
    fn shift(&mut self, op: ShiftOp, size: u8, t: T, count: Option<u8>) {
        let t = r(t);
        macro_rules! op {
            ($m:ident) => {
                match (size, count) {
                    (1, Some(c)) => dynasm!(self.ops ; .arch x64 ; $m Rb(t), c as i8),
                    (2, Some(c)) => dynasm!(self.ops ; .arch x64 ; $m Rw(t), c as i8),
                    (_, Some(c)) => dynasm!(self.ops ; .arch x64 ; $m Rd(t), c as i8),
                    (1, None) => dynasm!(self.ops ; .arch x64 ; $m Rb(t), cl),
                    (2, None) => dynasm!(self.ops ; .arch x64 ; $m Rw(t), cl),
                    (_, None) => dynasm!(self.ops ; .arch x64 ; $m Rd(t), cl),
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
        if !self.wanted(super::flags::shift_flags(op)) {
            return;
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
                // OF is the original sign for a count of 1, else 0 (the
                // result's bit below its top); AF set.
                match count {
                    Some(c) => {
                        let of = if c == 1 { OF } else { 0 };
                        dynasm!(self.ops ; .arch x64 ; and eax, (CF | SZP | of) as i32);
                    }
                    None => dynasm!(self.ops
                        ; .arch x64
                        ; mov ecx, Rd(t)
                        ; shr ecx, top - 1
                        ; and ecx, 1
                        ; shl ecx, 11
                        ; and eax, (CF | SZP) as i32
                        ; or eax, ecx
                    ),
                }
                dynasm!(self.ops ; .arch x64 ; or eax, AF as i32);
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

    /// SHLD and SHRD by 1 to width - 1, `count` or CL (None, in ECX): the
    /// host's result, CF, SF, ZF and PF; OF as `alu_double_shift` has it;
    /// AF kept.
    fn double_shift(&mut self, left: bool, size: u8, dst: T, src: T, count: Option<u8>) {
        let (d, s) = (r(dst), r(src));
        match (left, size, count) {
            (true, 2, Some(c)) => dynasm!(self.ops ; .arch x64 ; shld Rw(d), Rw(s), c as i8),
            (true, _, Some(c)) => dynasm!(self.ops ; .arch x64 ; shld Rd(d), Rd(s), c as i8),
            (false, 2, Some(c)) => dynasm!(self.ops ; .arch x64 ; shrd Rw(d), Rw(s), c as i8),
            (false, _, Some(c)) => dynasm!(self.ops ; .arch x64 ; shrd Rd(d), Rd(s), c as i8),
            (true, 2, None) => dynasm!(self.ops ; .arch x64 ; shld Rw(d), Rw(s), cl),
            (true, _, None) => dynasm!(self.ops ; .arch x64 ; shld Rd(d), Rd(s), cl),
            (false, 2, None) => dynasm!(self.ops ; .arch x64 ; shrd Rw(d), Rw(s), cl),
            (false, _, None) => dynasm!(self.ops ; .arch x64 ; shrd Rd(d), Rd(s), cl),
        }
        if !self.wanted(CF | OF | SZP) {
            return;
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
        let wanted = self.wanted(CF | OF);
        if wanted {
            dynasm!(self.ops ; .arch x64 ; pushfq);
        }
        match size {
            1 => {
                dynasm!(self.ops ; .arch x64 ; mov ecx, eax);
                self.set_ecx(Gpr::word(0));
            }
            2 => {
                dynasm!(self.ops ; .arch x64 ; mov ecx, eax ; mov esi, edx);
                self.set_ecx(Gpr::word(0));
                dynasm!(self.ops ; .arch x64 ; mov ecx, esi);
                self.set_ecx(Gpr::word(2));
            }
            _ => dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + acc], eax ; mov DWORD [rbx + high], edx),
        }
        if wanted {
            dynasm!(self.ops ; .arch x64 ; pop rax);
            self.merge(CF | OF, CF | OF);
        }
    }

    /// DIV or IDIV of AX, DX:AX or EDX:EAX by t, or #DE first where the
    /// quotient doesn't fit (the host's division would fault too, so it
    /// only runs where it can't). The flags stay.
    fn div_wide(&mut self, signed: bool, size: u8, t: T) {
        let t = r(t);
        let de = self.fault_exit(EXIT_DE);
        let (acc, high) = (gpr_offset(Gpr::dword(0)), gpr_offset(Gpr::dword(2)));
        match (signed, size) {
            // Unsigned, the quotient fits if the dividend's upper half is
            // below the divisor, which a divisor of 0 never is.
            (false, 1) => {
                dynasm!(self.ops
                    ; .arch x64
                    ; movzx eax, WORD [rbx + acc]
                    ; movzx ecx, ah
                    ; cmp ecx, Rd(t)
                    ; jae =>de
                    ; div Rb(t)
                    ; mov ecx, eax
                );
                self.set_ecx(Gpr::word(0));
            }
            (false, 2) => {
                dynasm!(self.ops
                    ; .arch x64
                    ; movzx eax, WORD [rbx + acc]
                    ; movzx edx, WORD [rbx + high]
                    ; cmp edx, Rd(t)
                    ; jae =>de
                    ; div Rw(t)
                    ; mov ecx, eax
                    ; mov esi, edx
                );
                self.set_ecx(Gpr::word(0));
                dynasm!(self.ops ; .arch x64 ; mov ecx, esi);
                self.set_ecx(Gpr::word(2));
            }
            (false, _) => dynasm!(self.ops
                ; .arch x64
                ; mov eax, DWORD [rbx + acc]
                ; mov edx, DWORD [rbx + high]
                ; cmp edx, Rd(t)
                ; jae =>de
                ; div Rd(t)
                ; mov DWORD [rbx + acc], eax
                ; mov DWORD [rbx + high], edx
            ),
            // Signed, the division is twice as wide as the guest's, where
            // no quotient overflows, and then the quotient must fit.
            (true, 1) => {
                dynasm!(self.ops
                    ; .arch x64
                    ; movsx ecx, Rb(t)
                    ; test ecx, ecx
                    ; jz =>de
                    ; movsx eax, WORD [rbx + acc]
                    ; cdq
                    ; idiv ecx
                    ; movsx esi, al
                    ; cmp esi, eax
                    ; jne =>de
                    ; mov ah, dl
                    ; mov ecx, eax
                );
                self.set_ecx(Gpr::word(0));
            }
            (true, 2) => {
                dynasm!(self.ops
                    ; .arch x64
                    ; movsx rcx, Rw(t)
                    ; test ecx, ecx
                    ; jz =>de
                    ; movzx eax, WORD [rbx + acc]
                    ; movzx edx, WORD [rbx + high]
                    ; shl edx, 16
                    ; or eax, edx
                    ; movsxd rax, eax
                    ; cqo
                    ; idiv rcx
                    ; movsx rsi, ax
                    ; cmp rsi, rax
                    ; jne =>de
                    ; mov ecx, eax
                    ; mov esi, edx
                );
                self.set_ecx(Gpr::word(0));
                dynasm!(self.ops ; .arch x64 ; mov ecx, esi);
                self.set_ecx(Gpr::word(2));
            }
            (true, _) => dynasm!(self.ops
                ; .arch x64
                ; movsxd rcx, Rd(t)
                ; test rcx, rcx
                ; jz =>de
                ; mov eax, DWORD [rbx + acc]
                ; mov edx, DWORD [rbx + high]
                ; shl rdx, 32
                ; or rax, rdx
                // The one 64-bit quotient that overflows: -2^63 / -1.
                ; cmp rcx, -1
                ; jne >divide
                ; mov rdx, rax
                ; neg rdx
                ; jo =>de
                ; divide:
                ; cqo
                ; idiv rcx
                ; movsxd rsi, eax
                ; cmp rsi, rax
                ; jne =>de
                ; mov DWORD [rbx + acc], eax
                ; mov DWORD [rbx + high], edx
            ),
        }
    }

    fn exit_if(&mut self, cond: Cond, taken: u32, next: u32, commit: Option<(Gpr, T)>) {
        let yes = self.ops.new_dynamic_label();
        // The flags go back into the CPU for both ways out; the condition
        // reads them where they were.
        let ebp = self.dirty;
        self.flags_back();
        self.dirty = false;
        match cond {
            Cond::Flags(cc) => self.condition(cc, yes, ebp),
            Cond::Zero(t) => dynasm!(self.ops ; .arch x64 ; test Rd(r(t)), Rd(r(t)) ; jz =>yes),
            Cond::NonZero(t) => dynasm!(self.ops ; .arch x64 ; test Rd(r(t)), Rd(r(t)) ; jnz =>yes),
            Cond::NonZeroZf(t, zf) => {
                let no = self.ops.new_dynamic_label();
                dynasm!(self.ops ; .arch x64 ; test Rd(r(t)), Rd(r(t)) ; jz =>no);
                self.load_flags_eax(ebp);
                dynasm!(self.ops ; .arch x64 ; test eax, ZF as i32);
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
        let gp = self.fault_exit(EXIT_GP0);
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
        self.flags_back();
        self.counts();
        match eip {
            Some(eip) if self.link && self.data.in_page(eip) => {
                self.stubs[slot].get_or_insert_with(|| self.ops.new_dynamic_label());
                dynasm!(self.ops ; .arch x64 ; jmp QWORD [rdx + DATA_LINKS + slot as i32 * 8]);
            }
            Some(_) if self.link => self.guarded(slot),
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

    /// Leave through the return link to EIP `t`, if the return has one
    /// (see `guarded`), else to the execution loop, to be linked. RDX is
    /// the block.
    fn returned(&mut self, t: T) {
        let miss = *self.return_miss.get_or_insert_with(|| self.ops.new_dynamic_label());
        for slot in RETURN_LINK..LINKS {
            let next = self.ops.new_dynamic_label();
            let g = DATA_GUARDS + slot as i32 * GUARD_SIZE;
            dynasm!(self.ops ; .arch x64 ; cmp Rd(r(t)), DWORD [rdx + g + GUARD_EIP] ; jne =>next);
            self.guarded(slot);
            dynasm!(self.ops ; .arch x64 ; =>next);
        }
        dynasm!(self.ops ; .arch x64 ; jmp =>miss);
    }

    /// Leave through link `slot` to another page, if fetching its target
    /// goes as when the link was made (its `Guard`): the same CS base, A20
    /// gate and paging, and with paging the target page's translation
    /// still in the TLB (a return's link has checked the EIP). Otherwise
    /// through the stub, to the execution loop. RDX is the block.
    fn guarded(&mut self, slot: usize) {
        let stub = *self.stubs[slot].get_or_insert_with(|| self.ops.new_dynamic_label());
        let g = DATA_GUARDS + slot as i32 * GUARD_SIZE;
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

    /// EAX = the guest's flags, from EBP (`ebp`) or the CPU.
    fn load_flags_eax(&mut self, ebp: bool) {
        if ebp {
            dynasm!(self.ops ; .arch x64 ; mov eax, ebp);
        } else {
            dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + FLAGS]);
        }
    }

    /// Jump to `yes` if condition `cc` holds on the guest's flags, in EBP
    /// (`ebp`) or the CPU.
    fn condition(&mut self, cc: ConditionCode, yes: DynamicLabel, ebp: bool) {
        if self.test_condition(cc, ebp) {
            dynasm!(self.ops ; .arch x64 ; jnz =>yes);
        } else {
            dynasm!(self.ops ; .arch x64 ; jz =>yes);
        }
    }

    /// Test condition `cc` on the guest's flags, in EBP (`ebp`) or the CPU:
    /// it holds if the host's ZF is clear (true) or set (false).
    fn test_condition(&mut self, cc: ConditionCode, ebp: bool) -> bool {
        use ConditionCode as C;
        let bits = super::flags::cond_flags(cc) as i32;
        match cc {
            C::o | C::b | C::e | C::be | C::s | C::p | C::no | C::ae | C::ne | C::a | C::ns | C::np => {
                if ebp {
                    dynasm!(self.ops ; .arch x64 ; test ebp, bits);
                } else {
                    dynasm!(self.ops ; .arch x64 ; test DWORD [rbx + FLAGS], bits);
                }
                matches!(cc, C::o | C::b | C::e | C::be | C::s | C::p)
            }
            C::l | C::ge => {
                // SF != OF: OF moved down to SF's bit.
                self.load_flags_eax(ebp);
                dynasm!(self.ops
                    ; .arch x64
                    ; mov ecx, eax
                    ; shr ecx, 4
                    ; xor ecx, eax
                    ; test ecx, SF as i32
                );
                cc == C::l
            }
            _ => {
                // LE: ZF or SF != OF; G: neither.
                self.load_flags_eax(ebp);
                dynasm!(self.ops
                    ; .arch x64
                    ; mov ecx, eax
                    ; shr ecx, 4
                    ; xor ecx, eax
                    ; and ecx, SF as i32
                    ; and eax, ZF as i32
                    ; or eax, ecx
                );
                cc == C::le
            }
        }
    }
}
