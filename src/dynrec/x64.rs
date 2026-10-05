//! The x86-64 code generator.
//!
//! Registers while translated code runs: RBX the CPU (and in it the TLB's
//! entries), R12 the context (`JitCtx`), R13 RAM, R14 which blocks of RAM
//! hold code (`Bus::code_blocks`), EBP the guest's arithmetic flags where
//! the code has changed them (see `Gen::dirty`); R8-R10 hold the
//! operations' temporaries (`uop::T`), R11, RSI, RDI and R15 the guest
//! registers a block uses most (see `Cache`), and RAX, RCX and RDX are
//! scratch. Translated code calls Rust with the
//! System V convention, which Rust offers on every x86-64 host.

// dynasm converts the registers it is given at run time with `into`.
#![allow(clippy::useless_conversion)]

use dynasmrt::x64::X64Relocation;
use dynasmrt::{DynamicLabel, DynasmApi, DynasmLabelApi, VecAssembler, dynasm};
use iced_x86::ConditionCode;

use super::block::{BlockData, LINKS, RETURN_LINK, RETURN_MISS, SIDE_LINK};
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
const CPL: i32 = layout::CPL as i32;
const FPU_TOP: i32 = layout::fpu::TOP as i32;
const FPU_TAGS: i32 = layout::fpu::TAGS as i32;
const FPU_FLAGS: i32 = layout::fpu::FLAGS as i32;
const FPU_CONTROL: i32 = layout::fpu::CONTROL as i32;
const FPU_F64: i32 = layout::fpu::F64 as i32;
const FPU_X80: i32 = layout::fpu::X80 as i32;
const FPU_STALE: i32 = layout::fpu::STALE as i32;
const FPU_EMPTY: i8 = crate::cpu::FPU_TAG_EMPTY as i8;
const FPU_VALID: i8 = crate::cpu::FPU_TAG_VALID as i8;

/// Flag bits.
const CF: u32 = 0x001;
const PF: u32 = 0x004;
const AF: u32 = 0x010;
const ZF: u32 = 0x040;
const SF: u32 = 0x080;
const OF: u32 = 0x800;
const ARITH: u32 = CF | PF | AF | ZF | SF | OF;
const SZP: u32 = SF | ZF | PF;

/// A TLB entry's size, and its log2.
const TLB_ENTRY: i32 = layout::TLB_ENTRY_SIZE as i32;
const TLB_ENTRY_SHIFT: i8 = 5;
/// Where the TLB's entries are in the CPU.
const TLB: i32 = layout::TLB as i32;
const _: () = assert!(1 << TLB_ENTRY_SHIFT == TLB_ENTRY);

/// Where the RAM below the video memory ends, and extended memory starts.
/// Handles up to this are addresses in RAM (which is far smaller): above
/// it are `SLOW`'s, and those with `DEV_BIT`.
const RAM_HANDLES: i32 = 0x7FFF_FFFF;
const _: () = assert!(crate::config::MAX_MEMSIZE << 20 <= RAM_HANDLES as usize && SLOW > RAM_HANDLES as u32);
const _: () = assert!(DEV_BIT == 32);
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
    /// `fpu_addsub`.
    pub addsub: usize,
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
        ; mov r14, QWORD [r12 + CTX_CODE_BLOCKS]
        ; mov ebp, DWORD [rbx + FLAGS]
        ; mov BYTE [r12 + CTX_SMC], 0
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
    let addsub = ops.offset().0;
    fpu_addsub(&mut ops);
    Trampoline { bytes: ops.finalize().expect("trampoline"), enter, exit, addsub }
}

/// ST(dst) = a + b, or a - b, as `F80::add` and `F80::sub` add their 80
/// bits (`fpu::arithmetic::addsub_st` and `addsub_value`): where both are
/// registers that aren't empty or doubles, not an infinity or a NaN held as
/// a double, and the sum comes to a double's range; a routine of the
/// trampoline's that translated code calls with the physical registers a
/// and b in R8D and R9D (8 for the double in XMM15) and dst | sub << 8 in
/// R10D, which returns EAX = 0, or 1 with nothing changed for the handler's
/// code. RCX, RDX and R8-R10 are changed.
fn fpu_addsub(ops: &mut Asm) {
    let slow = ops.new_dynamic_label();
    dynasm!(ops ; .arch x64 ; push r11 ; push rsi ; mov r11d, r10d);
    // Operand into (sign << 15 | exponent) and the 64-bit mantissa.
    f80_operand(ops, 0, 8, slow);
    f80_operand(ops, 2, 9, slow);
    dynasm!(ops
        ; .arch x64
        ; mov ecx, r11d
        ; and ecx, 0x100
        ; shl ecx, 7
        ; xor edx, ecx
        // Exponents aligned: the smaller operand's mantissa shifted
        // right (to 0 by 64 or more); R10D the larger exponent.
        ; movzx ecx, ax
        ; and ecx, 0x7FFF
        ; movzx esi, dx
        ; and esi, 0x7FFF
        ; mov r10d, esi
        ; cmp ecx, esi
        ; cmova r10d, ecx
        ; neg ecx
        ; add ecx, r10d
        ; neg esi
        ; add esi, r10d
        ; shr r8, cl
        ; cmp ecx, 64
        ; sbb rcx, rcx
        ; and r8, rcx
        ; mov ecx, esi
        ; shr r9, cl
        ; cmp esi, 64
        ; sbb rsi, rsi
        ; and r9, rsi
        // Same signs: the sum, a carry shifted in from the top.
        ; mov ecx, eax
        ; xor ecx, edx
        ; test ecx, 0x8000
        ; jnz >differ
        ; add r8, r9
        ; setc cl
        ; mov rsi, r8
        ; rcr rsi, 1
        ; test cl, cl
        ; cmovnz r8, rsi
        ; movzx ecx, cl
        ; add r10d, ecx
        ; and eax, 0x8000
        ; jmp >result
        // Different signs: the difference, normalized, with the larger
        // one's sign; 0 for none.
        ; differ:
        ; mov rsi, r9
        ; sub rsi, r8
        ; mov rcx, r8
        ; sub rcx, r9
        ; cmovb rcx, rsi
        ; cmovb eax, edx
        ; mov r8, rcx
        ; test r8, r8
        ; jz >nothing
        ; normalize:
        ; bsr rcx, r8
        ; xor ecx, 63
        ; shl r8, cl
        ; sub r10d, ecx
        ; and eax, 0x8000
        ; jmp >result
        ; nothing:
        ; xor eax, eax
        ; xor r8d, r8d
        ; xor r10d, r10d
        // The double `F80::get_f64` makes of it into R9: 0 for exponent
        // 0; outside a double's range (or an exponent that wrapped) the
        // handlers'.
        ; result:
        ; test r10d, r10d
        ; jz >zero
        ; mov ecx, r10d
        ; sub ecx, 16383 - 1023
        ; lea edx, [rcx - 1]
        ; cmp edx, 0x7FD
        ; ja =>slow
        ; mov r9, r8
        ; shr r9, 11
        ; btr r9, 52
        ; shl rcx, 52
        ; or r9, rcx
        ; mov rdx, rax
        ; shl rdx, 48
        ; or r9, rdx
        ; jmp >store
        ; zero:
        ; mov r9, rax
        ; shl r9, 48
        // ST(dst): the 80 bits, their double, and not stale.
        ; store:
        ; or eax, r10d
        ; mov ecx, r11d
        ; and ecx, 7
        ; mov edx, ecx
        ; shl edx, 4
        ; mov QWORD [rbx + rdx + FPU_X80], r8
        ; mov QWORD [rbx + rdx + FPU_X80 + 8], rax
        ; mov QWORD [rbx + rcx * 8 + FPU_F64], r9
        ; mov BYTE [rbx + rcx + FPU_STALE], 0
        ; xor eax, eax
        ; pop rsi
        ; pop r11
        ; ret
        ; =>slow
        ; mov eax, 1
        ; pop rsi
        ; pop r11
        ; ret
    );
}

/// Operand `fpu_addsub` has in R`man`D (a physical register, or 8 for the
/// double in XMM15) into `hi` (sign << 15 | exponent, of RAX or RDX) and
/// R`man` (R8 or R9), as `FpuRegs::get` and `F80::set_f64` make its 80
/// bits; to `slow` for an empty register or a double that is an infinity
/// or a NaN. RCX is changed.
fn f80_operand(ops: &mut Asm, hi: u8, man: u8, slow: DynamicLabel) {
    let (value, stale, loaded, zero) =
        (ops.new_dynamic_label(), ops.new_dynamic_label(), ops.new_dynamic_label(), ops.new_dynamic_label());
    dynasm!(ops
        ; .arch x64
        ; cmp Rd(man), 8
        ; je =>value
        ; cmp BYTE [rbx + Rq(man) + FPU_TAGS], FPU_EMPTY
        ; je =>slow
        ; cmp BYTE [rbx + Rq(man) + FPU_STALE], 0
        ; jne =>stale
        ; mov Rd(hi), Rd(man)
        ; shl Rd(hi), 4
        ; mov Rq(man), QWORD [rbx + Rq(hi) + FPU_X80]
        ; movzx Rd(hi), WORD [rbx + Rq(hi) + FPU_X80 + 8]
        ; jmp =>loaded
        ; =>stale
        ; mov rcx, QWORD [rbx + Rq(man) * 8 + FPU_F64]
        ; jmp >double
        ; =>value
        ; movq rcx, xmm15
        // The double's bits in RCX.
        ; double:
        ; mov Rq(man), rcx
        ; shl Rq(man), 11
        ; bts Rq(man), 63
        ; mov Rq(hi), rcx
        ; shr Rq(hi), 52
        ; mov ecx, Rd(hi)
        ; and ecx, 0x7FF
        ; jz =>zero
        ; cmp ecx, 0x7FF
        ; je =>slow
        ; and Rd(hi), 0x800
        ; shl Rd(hi), 4
        ; lea Rd(hi), [Rq(hi) + rcx + 16383 - 1023]
        ; jmp =>loaded
        ; =>zero
        ; shl Rd(hi), 4
        ; xor Rd(man), Rd(man)
        ; =>loaded
    );
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

/// Host registers that hold guest registers within a block: R11, RSI and
/// RDI, which calls into Rust don't keep (their slow paths save them), and
/// R15, which they do.
const CACHE_HOSTS: [u8; 4] = [11, 6, 7, 15];
/// Scratch registers.
const RAX: u8 = 0;
const RCX: u8 = 1;
const RDX: u8 = 2;

/// The guest registers a block keeps in host registers: the ones its
/// operations use most. An instruction loads those it uses from the CPU as
/// it starts, if they aren't in their host registers yet, so which are
/// there doesn't change within it; they go back into the CPU where the
/// block leaves, before a handler's call (after which they are loaded
/// again), and where an instruction stops it.
#[derive(Clone, Copy, Default)]
struct Cache {
    /// Per guest register (EAX..EDI), its host register, or 0.
    host: [u8; 8],
    /// Bits per guest register: in its host register, and changed there
    /// since.
    loaded: u8,
    dirty: u8,
}

/// The guest registers (bits by index) operation `u` reads or writes.
fn uop_gprs(u: &Uop) -> u8 {
    let bit = |g: Gpr| 1u8 << g.index;
    match *u {
        Uop::Get { r, .. } | Uop::Set { r, .. } => bit(r),
        Uop::Ea { base, index, .. } => base.map_or(0, bit) | index.map_or(0, bit),
        Uop::ShiftVar { count, .. } | Uop::DoubleShiftVar { count, .. } => bit(count),
        Uop::MulWide { size, .. } | Uop::DivWide { size, .. } => {
            if size == 1 {
                1
            } else {
                1 | 1 << 2
            }
        }
        Uop::ExitIf { commit: Some((g, _)), .. } => bit(g),
        Uop::RepStart { count, .. } => bit(count),
        _ => 0,
    }
}

/// Which guest registers a block keeps in host registers (see `Cache`):
/// the most used, if twice or more.
fn choose_cached(items: &[Option<Vec<Uop>>]) -> [u8; 8] {
    let mut uses = [0u32; 8];
    for u in items.iter().flatten().flatten() {
        let gprs = uop_gprs(u);
        for (i, n) in uses.iter_mut().enumerate() {
            *n += (gprs >> i & 1) as u32;
        }
    }
    let mut order: Vec<usize> = (0..8).filter(|&i| uses[i] >= 2).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(uses[i]));
    let mut host = [0u8; 8];
    for (&i, &h) in order.iter().zip(&CACHE_HOSTS) {
        host[i] = h;
    }
    host
}

fn seg_field(seg: Seg, field: usize) -> i32 {
    (layout::SEG + seg as usize * layout::SEG_SIZE + field) as i32
}

/// Code emitted after the block's instructions, reached from them.
enum Slow {
    /// A memory operand the inline checks didn't take.
    MemRef { at: DynamicLabel, back: DynamicLabel, t: T, desc: u32, fail: DynamicLabel },
    Load { at: DynamicLabel, back: DynamicLabel, dst: T, m: T, size: u8 },
    /// Where a memory reference's slow paths come back to before a load or
    /// store that takes its handle as RAM's (`Gen::ram_back`): to `slow`
    /// with one that isn't, else to the RAM access at `ram`.
    Recheck { at: DynamicLabel, m: T, slow: DynamicLabel, ram: DynamicLabel },
    Store { at: DynamicLabel, back: DynamicLabel, m: T, src: T, size: u8, lo: u32, hi: u32 },
    /// A memory operand that isn't plain RAM, with its physical address
    /// in `addr`, or with `tlb` (the TLB's set, and the tag's offset in
    /// an entry) its linear address in EAX: if it is within one page (one
    /// the TLB holds), its handle is that address with `DEV_BIT`; else it
    /// goes on to `slow`, the `MemRef` above.
    Dev { at: DynamicLabel, back: DynamicLabel, slow: DynamicLabel, t: T, addr: u8, size: u8, tlb: Option<(i32, i32)> },
    /// A store into a block of RAM with code (`Bus::code_blocks`): its
    /// generations bumped, and a store into the rest of the block noted.
    CodeStore { at: DynamicLabel, back: DynamicLabel, m: T, src: T, size: u8, lo: u32, hi: u32 },
    /// An instruction's fault with an exit code of its own (#GP(0), #DE).
    Fault { at: DynamicLabel, code: u32, fail: DynamicLabel },
    /// A store hit the rest of the block: it leaves after the instruction,
    /// at `next`, with the flags in EBP (`dirty`) or a recorded
    /// operation's (`lazy`), through `join`.
    Smc { at: DynamicLabel, next: u32, dirty: bool, lazy: bool, join: DynamicLabel },
    /// Link `slot`'s guard (at `g` in the block) didn't find its page in the
    /// TLB: if the link is made, `jit_fetch` looks the page up and the
    /// guard checks again from `back`, else the link's `stub`. RDX is the
    /// block.
    Fetch { at: DynamicLabel, back: DynamicLabel, stub: DynamicLabel, slot: usize, g: i32 },
    /// The instruction is done and stops the block after it with `code`:
    /// EIP on `next`.
    After { at: DynamicLabel, next: u32, code: u32, fail: DynamicLabel },
    /// Instruction `ix` runs through its handler after all (`Uop::Bail`),
    /// with the flags in EBP there (`dirty`), and goes on at `end`.
    /// The cached guest registers in `wb` go back into the CPU before the
    /// handler runs, and those in `reload` are loaded again after.
    /// With `leave`, the block stops after it (an `FpuGuard`'s).
    Bail { at: DynamicLabel, end: DynamicLabel, ix: usize, dirty: bool, wb: u8, reload: u8, leave: bool },
}

/// A conditional jump the block goes on after (`block::SIDE_EXITS`): its
/// way out where it is taken, at `at`, made after the block's code from
/// what was known at the jump: instruction `ix`, the commit of its
/// counter, the cached registers, where the flags are and the segments
/// loaded.
struct Side {
    at: DynamicLabel,
    ix: usize,
    taken: u32,
    commit: Option<(Gpr, T)>,
    cache: Cache,
    dirty: bool,
    loaded_segs: u8,
}

/// A conditional jump to instruction `to` later in the block
/// (`BlockData::target`), where taken: at `at`, made after the block's
/// code, the counts are brought to where they are at `to` but for the
/// instructions it skips, and the cached registers and flags to how the
/// code at `to` has them (`fall`, known once it is translated); then on at
/// `entry`. With what was known at the jump, as for a `Side`.
struct Merge {
    at: DynamicLabel,
    ix: usize,
    to: usize,
    commit: Option<(Gpr, T)>,
    cache: Cache,
    dirty: bool,
    synced: i32,
    fpu_cr0_checked: bool,
    fpu_known: u8,
    /// Set where `to` is translated: where its code starts, and the
    /// counts, cached registers and flags there.
    entry: Option<(DynamicLabel, i32, Cache, bool)>,
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
    /// After a memory reference: its handle, and where its slow paths come
    /// back to, with a handle that may not be RAM's. A load or store of it
    /// right after uses the fast path's RAM address without checking it
    /// (see `Slow::Recheck`); anything else places the label first.
    ram_back: Option<(T, DynamicLabel)>,
    sides: Vec<Side>,
    merges: Vec<Merge>,
    /// Whether exits to a known EIP in the page may be linked, the stubs
    /// of the links used, and their jumps (`Code::sites`).
    link: bool,
    stubs: [Option<DynamicLabel>; LINKS],
    sites: Vec<(u8, u32)>,
    /// The way out of a return to none of the places its links lead to.
    return_miss: Option<DynamicLabel>,
    /// Where the iteration of a REP string instruction starts.
    rep_top: Option<DynamicLabel>,
    /// The offsets temporaries hold where the operations before made
    /// them constants (`Uop::Const`, or `Uop::Ea` of a displacement).
    known: [Option<u32>; 8],
    /// The instruction being translated.
    ix: usize,
    /// The flags live after each operation (`flags::plan`), and after the
    /// one being translated; the operations that record their operands
    /// instead of computing the flags only a fault or a store into the
    /// block needs (and whether the one being translated does), and per
    /// instruction whether the flags as it starts or ends are a recorded
    /// operation's, which the ways out there work out (`lazy_flags`). The
    /// instructions' ways out after that (see `fail`).
    live: Vec<Vec<u32>>,
    live_after: u32,
    record: Vec<Vec<bool>>,
    record_now: bool,
    lazy_start: Vec<bool>,
    lazy_end: Vec<bool>,
    join: Vec<Option<DynamicLabel>>,
    /// What the code is translated for.
    env: super::Env,
    /// The guest's arithmetic flags are in EBP, not yet in the CPU (whose
    /// other flags are right). They go back into the CPU where anything
    /// else may read them: where the block leaves and before a handler.
    /// Per instruction, whether they are in EBP where it may stop, before
    /// it changes them; the execution loop puts them back then
    /// (`EXIT_FLAGS`).
    dirty: bool,
    dirty_at: Vec<bool>,
    /// The guest registers in host registers, and per instruction those
    /// that go back into the CPU where it stops the block.
    cache: Cache,
    wb_at: Vec<u8>,
    /// The segment registers loaded in the block so far (bits by `Seg`):
    /// their accesses are checked as if they weren't flat, and the block's
    /// links are only taken where the segments are flat as the block's
    /// environment has them.
    loaded_segs: u8,
    /// Whether an FPU instruction of the block has checked CR0's EM and TS,
    /// which don't change within it, and the registers ST(i) (bit i) known
    /// not to be empty where the code being generated runs: those an
    /// `FpuGuard` checked or the block pushed. A handler's call forgets
    /// them (it may be an FPU instruction's).
    fpu_cr0_checked: bool,
    fpu_known: u8,
}

/// A translated block's code, where its links' stubs are in it, and how
/// far behind each instruction the instruction count is (`BlockData::lag`).
pub struct Code {
    pub bytes: Vec<u8>,
    pub stubs: [Option<usize>; LINKS],
    /// The links' jumps, which go to their stubs: the link, and the jump's
    /// offset in the code (see `patch_link`).
    pub sites: Vec<(u8, u32)>,
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
/// Blocks go on into the last 15 bytes of their page (see
/// `BlockData::in_tail`), with paging checking the TLB holds the next page.
pub const TAIL: bool = true;
/// It has the operations of segment loads, port I/O and STI.
pub const SYSTEM: bool = true;
/// Whether it has the loads of data segment registers in protected mode.
pub const SEGMENTS: bool = true;
/// And those of FPU instructions.
pub const FPU: bool = true;

/// Point the link jump at `site` (`Code::sites`) at `to`.
pub fn patch_link(mem: &mut super::codemem::CodeMemory, site: *const u8, to: *const u8) {
    let rel = (to as isize - (site as isize + 4)) as i32;
    mem.patch(site, &rel.to_le_bytes());
}

pub fn block(data: &BlockData, items: &[Option<Vec<Uop>>], link: bool, env: super::Env) -> Code {
    let mut ops = Asm::new(0);
    let n = data.count();
    let (tail, deadline, revalidate, body) =
        (ops.new_dynamic_label(), ops.new_dynamic_label(), ops.new_dynamic_label(), ops.new_dynamic_label());
    let (limit, fail_tail) = (ops.new_dynamic_label(), ops.new_dynamic_label());
    let plan = super::flags::plan(items, &data.targets());
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
        ram_back: None,
        sides: Vec::new(),
        merges: Vec::new(),
        link,
        stubs: [None; LINKS],
        sites: Vec::new(),
        return_miss: None,
        rep_top: None,
        known: [None; 8],
        ix: 0,
        live: plan.live,
        live_after: 0,
        record: plan.record,
        record_now: false,
        lazy_start: plan.lazy_start,
        lazy_end: plan.lazy_end,
        join: vec![None; n],
        env,
        // Blocks start with the flags in EBP, from the trampoline or the
        // block before.
        dirty: true,
        dirty_at: vec![false; n],
        cache: Cache { host: choose_cached(items), ..Cache::default() },
        wb_at: vec![0; n],
        loaded_segs: 0,
        fpu_cr0_checked: false,
        fpu_known: 0,
    };
    g.prologue();
    let mut synced = 0;
    for (ix, item) in items.iter().enumerate() {
        g.ix = ix;
        g.merge_here(synced);
        g.known = [None; 8];
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
                // The handler reads and writes the registers in the CPU.
                g.writeback(g.cache.dirty);
                g.cache.dirty = 0;
                g.check_watched();
                g.check_next_page();
                g.fallback(ix as i32);
                g.fpu_known = 0;
                g.cache.loaded = 0;
                if let Some(seg) = super::block::loaded_segment(&data.instrs[ix]) {
                    g.loaded_segs |= 1 << seg as u8;
                }
            }
            Some(uops) => {
                let port = uops.iter().any(|u| matches!(u, Uop::In { .. } | Uop::Out { .. }));
                if port && ix as i32 > synced {
                    // Devices read the time from the instruction count.
                    let d = ix as i32 - synced;
                    dynasm!(g.ops ; .arch x64 ; add QWORD [rbx + ICOUNT], d);
                    synced = ix as i32;
                }
                g.synced[ix] = synced;
                // Operations check what can fault before they change the
                // flags: they are as at the instruction's start there.
                g.dirty_at[ix] = g.dirty;
                let used = uops.iter().fold(0, |m, u| m | uop_gprs(u));
                g.preload(used);
                g.wb_at[ix] = g.cache.loaded;
                g.check_watched();
                g.check_next_page();
                // A register or constant a store takes goes before its
                // memory reference (neither has an effect it could see), so
                // that the store comes right after it (see `Gen::ram_back`).
                let mut order: Vec<usize> = (0..uops.len()).collect();
                for k in 0..uops.len().saturating_sub(2) {
                    if let (Uop::MemRef { t, .. }, Uop::Get { t: v, .. } | Uop::Const { t: v, .. }, Uop::Store { m, src, .. }) =
                        (uops[k], uops[k + 1], uops[k + 2])
                        && m == t
                        && src == v
                        && v != t
                    {
                        order.swap(k, k + 1);
                    }
                }
                for k in order {
                    let uop = &uops[k];
                    g.live_after = g.live[ix][k];
                    g.record_now = g.record[ix][k];
                    if !matches!(*uop, Uop::Load { m, .. } | Uop::Store { m, .. } if g.ram_back.is_some_and(|(t, _)| t == m)) {
                        g.settle_ram();
                    }
                    g.uop(uop);
                }
                g.settle_ram();
                g.end_dirty[ix] = g.dirty;
                if let Some(end) = g.end.take() {
                    dynasm!(g.ops ; .arch x64 ; =>end);
                }
                if port {
                    // The port access asked for the block to stop after it.
                    let next = data.eips[ix].wrapping_add(data.instrs[ix].len() as u32) as i32;
                    let fail = g.fail();
                    let flags = if g.dirty { EXIT_FLAGS } else { 0 } as i32;
                    dynasm!(g.ops
                        ; .arch x64
                        ; movzx eax, BYTE [r12 + CTX_AFTER]
                        ; test eax, eax
                        ; jz >go_on
                        ; mov BYTE [r12 + CTX_AFTER], 0
                        ; mov DWORD [rbx + EIP], next
                        ; or eax, flags
                        ; jmp =>fail
                        ; go_on:
                    );
                }
                if uops.iter().any(|u| matches!(u, Uop::Store { .. })) {
                    // A store hit the rest of the block: leave after this
                    // instruction.
                    let at = g.ops.new_dynamic_label();
                    let next = data.eips[ix].wrapping_add(data.instrs[ix].len() as u32);
                    g.fail();
                    let join = *g.join[ix].get_or_insert_with(|| g.ops.new_dynamic_label());
                    dynasm!(g.ops ; .arch x64 ; cmp BYTE [r12 + CTX_SMC], 0 ; jne =>at);
                    g.slow.push(Slow::Smc { at, next, dirty: g.dirty, lazy: g.lazy_end[ix], join });
                }
            }
        }
    }
    // A block that ran out of length goes on at the next instruction (a
    // handler sets EIP itself).
    let last = n - 1;
    let next = data.eips[last].wrapping_add(data.instrs[last].len() as u32);
    // After one that leaves an interrupt shadow, the execution loop runs
    // the next instruction, which ends it.
    g.link = link && !super::block::shadows(&data.instrs[last]);
    match &items[last] {
        Some(u) if !u.iter().any(|u| matches!(u, Uop::Exit { .. } | Uop::ExitIf { .. } | Uop::FarExit)) => g.leave(Some(next), 0, true),
        None if !super::block::ends_block(&data.instrs[last]) => g.leave(Some(next), 0, false),
        None if g.link && super::block::far_transfer(&data.instrs[last]) => g.far_returned(),
        _ => {}
    }
    g.link = link;
    // The end: the counts, and back to the execution loop. The code that
    // jumps here has put the flags back.
    dynasm!(g.ops ; .arch x64 ; =>tail);
    g.dirty = false;
    g.cache.dirty = 0;
    g.leave(None, 0, false);
    // The conditional jumps the block went on after, where taken.
    for (k, side) in std::mem::take(&mut g.sides).into_iter().enumerate() {
        dynasm!(g.ops ; .arch x64 ; =>side.at);
        (g.ix, g.cache, g.dirty, g.loaded_segs) = (side.ix, side.cache, side.dirty, side.loaded_segs);
        g.taken(side.taken, side.commit, SIDE_LINK + k);
    }
    for merge in std::mem::take(&mut g.merges) {
        g.jump_in(merge);
    }
    g.epilogue();
    let stubs = g.stubs.map(|s| s.map(|l| g.ops.labels().resolve_dynamic(l).expect("stub").0));
    let lag = g.synced.iter().enumerate().map(|(ix, &synced)| (ix as i32 - synced) as u8).collect();
    let sites = std::mem::take(&mut g.sites);
    Code { bytes: g.ops.finalize().expect("block"), stubs, sites, lag }
}

impl Gen<'_> {
    fn prologue(&mut self) {
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
            ; mov rcx, QWORD [r12 + CTX_PAGE_GEN]
            ; mov eax, DWORD [rcx + (data.chunk_first * 4) as i32]
        );
        for chunk in data.chunk_first + 1..=data.chunk_last {
            dynasm!(self.ops ; .arch x64 ; add eax, DWORD [rcx + (chunk * 4) as i32]);
        }
        dynasm!(self.ops
            ; .arch x64
            ; cmp eax, DWORD [rdx + DATA_GEN_SUM]
            ; jne =>revalidate
            ; =>body
        );
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
        if data.live_imm(self.ix) {
            return;
        }
        for w in data.watched_in(self.ix) {
            let at = *changed.get_or_insert_with(|| self.fault_exit(EXIT_WATCHED));
            let phys = (data.phys as usize + w) as i32;
            dynasm!(self.ops ; .arch x64 ; cmp BYTE [r13 + phys], data.bytes[w] as i8 ; jne =>at);
        }
    }

    /// Leave the block before the instruction being translated if it is in
    /// its page's last 15 bytes, paging is on and the TLB doesn't hold the
    /// next page (the linear address of CS:EIP's page + 1000h): the
    /// interpreter's fetch looks that up, and may walk the page tables for
    /// it, for such an instruction. Where the TLB holds it, the lookup
    /// changes nothing.
    fn check_next_page(&mut self) {
        let data = self.data;
        if !data.in_tail(self.ix) || self.env.bits & super::ENV_PAGING == 0 {
            return;
        }
        let at = self.fault_exit(EXIT_NEXT_PAGE);
        let set = if self.env.bits & super::ENV_USER != 0 { layout::TLB_SET as i32 } else { 0 };
        let entry = set * TLB_ENTRY + layout::TLB_READ_TAG as i32;
        dynasm!(self.ops
            ; .arch x64
            ; mov eax, DWORD [rbx + seg_field(Seg::CS, layout::SEG_BASE)]
            ; add eax, data.eips[self.ix] as i32
            ; or eax, 0xFFF
            ; inc eax
            ; shr eax, 12
            ; mov edx, eax
            ; and edx, (layout::TLB_SET - 1) as i32
            ; shl edx, TLB_ENTRY_SHIFT
            ; inc eax
            ; cmp eax, DWORD [rbx + rdx + TLB + entry]
            ; jne =>at
        );
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
            ; mov eax, (EXIT_DEADLINE | EXIT_FLAGS) as i32
            ; mov rdx, QWORD data_ptr
            ; jmp QWORD [r12 + CTX_EXIT]
            ; =>limit
            ; mov eax, (EXIT_LIMIT | EXIT_FLAGS) as i32
            ; mov rdx, QWORD data_ptr
            ; jmp QWORD [r12 + CTX_EXIT]
            ; =>revalidate
            ; mov rdi, rbx
            ; mov rsi, r12
            ; call QWORD [r12 + CTX_REVALIDATE]
            ; test eax, eax
            ; jz =>body
            ; mov eax, (EXIT_STALE | EXIT_FLAGS) as i32
            ; mov rdx, QWORD data_ptr
            ; jmp QWORD [r12 + CTX_EXIT]
        );
        // The links' stubs: back to the execution loop to be linked.
        let misses = self.return_miss.map(|miss| (RETURN_MISS, miss));
        for (k, stub) in self.stubs.iter().copied().enumerate().filter_map(|(k, s)| Some((k, s?))).chain(misses) {
            dynasm!(self.ops
                ; .arch x64
                ; =>stub
                ; mov eax, (EXIT_UNLINKED | EXIT_FLAGS | (k as u32) << 8) as i32
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
                if self.lazy_start[ix] {
                    // The flags as the instruction starts are a recorded
                    // operation's.
                    self.lazy_flags();
                }
                if let Some(join) = self.join[ix] {
                    dynasm!(self.ops ; .arch x64 ; =>join);
                }
                // The cached registers as the instruction left them: it
                // changes none before it may fault.
                self.writeback(self.wb_at[ix]);
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
                        ; push rsi
                        ; push rdi
                        ; mov rdi, rbx
                        ; mov rsi, r12
                        ; mov edx, Rd(r(t))
                        ; mov ecx, desc as i32
                        ; call QWORD [r12 + CTX_MEMREF]
                        ; pop rdi
                        ; pop rsi
                        ; pop r11
                        ; pop r10
                        ; pop r9
                        ; pop r8
                        ; test rax, rax
                        ; js >fault
                        ; mov Rq(r(t)), rax
                        ; jmp =>back
                        ; fault:
                        ; mov eax, EXIT_FAULT as i32
                        ; jmp =>fail
                    );
                }
                Slow::Dev { at, back, slow, t, addr, size, tlb } => {
                    dynasm!(self.ops ; .arch x64 ; =>at);
                    if size > 1 {
                        dynasm!(self.ops
                            ; .arch x64
                            ; mov ecx, Rd(addr)
                            ; and ecx, 0xFFF
                            ; cmp ecx, 0x1000 - size as i32
                            ; ja =>slow
                        );
                    }
                    if let Some((entry, tag)) = tlb {
                        // The page's entry, whose tag must be the page + 1.
                        dynasm!(self.ops
                            ; .arch x64
                            ; mov edx, eax
                            ; shr edx, 12 - TLB_ENTRY_SHIFT
                            ; and edx, ((layout::TLB_SET - 1) << TLB_ENTRY_SHIFT) as i32
                            ; mov ecx, eax
                            ; shr ecx, 12
                            ; inc ecx
                            ; cmp ecx, DWORD [rbx + rdx + TLB + entry + tag]
                            ; jne =>slow
                            ; and eax, 0xFFF
                            ; or eax, DWORD [rbx + rdx + TLB + entry + layout::TLB_PHYS as i32]
                        );
                    }
                    dynasm!(self.ops
                        ; .arch x64
                        ; mov Rd(r(t)), Rd(addr)
                        ; bts Rq(r(t)), 32
                        ; jmp =>back
                    );
                }
                Slow::Recheck { at, m, slow, ram } => {
                    dynasm!(self.ops
                        ; .arch x64
                        ; =>at
                        ; cmp Rq(r(m)), RAM_HANDLES
                        ; ja =>slow
                        ; jmp =>ram
                    );
                }
                Slow::Load { at, back, dst, m, size } => {
                    dynasm!(self.ops
                        ; .arch x64
                        ; =>at
                        ; bt Rq(r(m)), 32
                        ; jnc >generic
                        ; push r8
                        ; push r9
                        ; push r10
                        ; push r11
                        ; push rsi
                        ; push rdi
                        ; mov rdi, rbx
                        ; mov rsi, r12
                        ; mov edx, Rd(r(m))
                        ; mov ecx, size as i32
                        ; call QWORD [r12 + CTX_DEV]
                        ; pop rdi
                        ; pop rsi
                        ; pop r11
                        ; pop r10
                        ; pop r9
                        ; pop r8
                        ; mov Rd(r(dst)), eax
                        ; jmp =>back
                        ; generic:
                        ; push r8
                        ; push r9
                        ; push r10
                        ; push r11
                        ; push rsi
                        ; push rdi
                        ; mov rdi, rbx
                        ; mov rsi, r12
                        ; mov edx, Rd(r(m))
                        ; and edx, 3
                        ; call QWORD [r12 + CTX_READ]
                        ; pop rdi
                        ; pop rsi
                        ; pop r11
                        ; pop r10
                        ; pop r9
                        ; pop r8
                        ; mov Rd(r(dst)), eax
                        ; jmp =>back
                    );
                }
                Slow::Fetch { at, back, stub, slot, g } => {
                    let off = slot as i32 * 8;
                    dynasm!(self.ops
                        ; .arch x64
                        ; =>at
                        ; mov rax, QWORD [rdx + DATA_LINKS + off]
                        ; cmp rax, QWORD [rdx + DATA_STUBS + off]
                        ; je =>stub
                        ; push rdx
                    );
                    self.save_for_call();
                    dynasm!(self.ops
                        ; .arch x64
                        ; mov ecx, DWORD [rdx + g + GUARD_EIP]
                        ; mov eax, ecx
                        ; and eax, 0xFFF
                        ; mov edx, DWORD [rdx + g + GUARD_PAGE]
                        ; shl edx, 12
                        ; or edx, eax
                        ; mov rdi, rbx
                        ; mov rsi, r12
                        ; call QWORD [r12 + CTX_FETCH]
                    );
                    self.restore_after_call();
                    dynasm!(self.ops
                        ; .arch x64
                        ; pop rdx
                        ; test eax, eax
                        ; jz =>stub
                        ; jmp =>back
                    );
                }
                Slow::Smc { at, next, dirty, lazy, join } => {
                    dynasm!(self.ops
                        ; .arch x64
                        ; =>at
                        ; mov BYTE [r12 + CTX_SMC], 0
                        ; mov DWORD [rbx + EIP], next as i32
                        ; mov eax, (EXIT_SMC | if dirty { EXIT_FLAGS } else { 0 }) as i32
                    );
                    if lazy {
                        // The flags after the instruction are a recorded
                        // operation's.
                        self.lazy_flags();
                    }
                    dynasm!(self.ops ; .arch x64 ; jmp =>join);
                }
                Slow::Fault { at, code, fail } => {
                    dynasm!(self.ops
                        ; .arch x64
                        ; =>at
                        ; mov eax, code as i32
                        ; jmp =>fail
                    );
                }
                Slow::After { at, next, code, fail } => {
                    dynasm!(self.ops
                        ; .arch x64
                        ; =>at
                        ; mov DWORD [rbx + EIP], next as i32
                        ; mov eax, code as i32
                        ; jmp =>fail
                    );
                }
                Slow::Bail { at, end, ix, dirty, wb, reload, leave } => {
                    // As a handler's call in the block, but with the
                    // instruction count put back after it, as the code on
                    // from `end` has it. A stop leaves the flags and
                    // registers the handler left in the CPU.
                    let lag = ix as i32 - self.synced[ix];
                    dynasm!(self.ops ; .arch x64 ; =>at);
                    self.writeback(wb);
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
                    if leave {
                        // The instruction is done: the execution loop goes on.
                        dynasm!(self.ops ; .arch x64 ; mov eax, EXIT_AFTER as i32 ; jmp >stop);
                    }
                    if self.end_dirty[ix] {
                        dynasm!(self.ops ; .arch x64 ; mov ebp, DWORD [rbx + FLAGS]);
                    }
                    self.load_cached(reload);
                    let fail_tail = self.fail_tail;
                    dynasm!(self.ops
                        ; .arch x64
                        ; jmp =>end
                        ; stop:
                        ; or eax, (ix as i32) << 8
                        ; jmp =>fail_tail
                    );
                }
                Slow::CodeStore { at, back, m, src, size, lo, hi } => {
                    // The code generations of the first and last byte's
                    // blocks, as the bus's writes bump them, and a store into
                    // the block's later bytes noted.
                    let m_ = r(m);
                    dynasm!(self.ops
                        ; .arch x64
                        ; =>at
                    );
                    self.store_ram(m, src, size);
                    dynasm!(self.ops
                        ; .arch x64
                        ; mov rdx, QWORD [r12 + CTX_PAGE_GEN]
                        ; mov eax, Rd(m_)
                        ; shr eax, crate::bus::GEN_SHIFT as i8
                        ; add DWORD [rdx + rax * 4], 1
                    );
                    if size > 1 {
                        dynasm!(self.ops
                            ; .arch x64
                            ; lea eax, [Rq(m_) + (size - 1) as i32]
                            ; shr eax, crate::bus::GEN_SHIFT as i8
                            ; add DWORD [rdx + rax * 4], 1
                        );
                    }
                    if lo < hi {
                        dynasm!(self.ops
                            ; .arch x64
                            ; cmp Rd(m_), hi as i32
                            ; jae =>back
                            ; lea eax, [Rq(m_) + size as i32]
                            ; cmp eax, lo as i32
                            ; jbe =>back
                            ; mov BYTE [r12 + CTX_SMC], 1
                        );
                    }
                    dynasm!(self.ops ; .arch x64 ; jmp =>back);
                }
                Slow::Store { at, back, m, src, size, lo, hi } => {
                    dynasm!(self.ops ; .arch x64 ; =>at);
                    self.vga_store(back, m, src, size);
                    dynasm!(self.ops
                        ; .arch x64
                        ; push r8
                        ; push r9
                        ; push r10
                        ; push r11
                        ; push rsi
                        ; push rdi
                        ; mov DWORD [r12 + CTX_SMC_LO], lo as i32
                        ; mov DWORD [r12 + CTX_SMC_HI], hi as i32
                        ; mov ecx, Rd(r(src))
                        ; mov edx, Rd(r(m))
                        ; mov rdi, rbx
                        ; mov rsi, r12
                        ; bt Rq(r(m)), 32
                        ; jnc >generic
                        ; mov r8d, size as i32
                        ; call QWORD [r12 + CTX_DEV + 8]
                        ; jmp >called
                        ; generic:
                        ; and edx, 3
                        ; call QWORD [r12 + CTX_WRITE]
                        ; called:
                        ; pop rdi
                        ; pop rsi
                        ; pop r11
                        ; pop r10
                        ; pop r9
                        ; pop r8
                        ; or BYTE [r12 + CTX_SMC], al
                        ; jmp =>back
                    );
                }
            }
        }
    }

    /// In real mode, unless loading segment register `seg` changes only its
    /// selector and base (`Cpu::load_seg_real` once it is a data-like
    /// segment) and not whether it is flat (a segment with a 4 GB limit,
    /// as HIMEM leaves them, with base 0: the block's environment has
    /// that), run the instruction through its handler (and with `leave`,
    /// stop the block after it). The selector is in `sel` (or not known
    /// yet: then no segment with a 4 GB limit).
    fn real_load(&mut self, seg: Seg, sel: Option<T>, leave: bool) {
        let at = self.ops.new_dynamic_label();
        dynasm!(self.ops
            ; .arch x64
            ; cmp BYTE [rbx + seg_field(seg, layout::SEG_ATTR)], layout::AR_DATA_RW as i8
            ; jne =>at
            ; cmp BYTE [rbx + seg_field(seg, layout::SEG_RIGHTS)], (layout::RIGHT_READ | layout::RIGHT_WRITE) as i8
            ; jne =>at
            ; cmp DWORD [rbx + seg_field(seg, layout::SEG_LO)], 0
            ; jne =>at
            ; cmp DWORD [rbx + seg_field(seg, layout::SEG_HI)], -1
        );
        match sel {
            None => dynasm!(self.ops ; .arch x64 ; je =>at),
            Some(t) => dynasm!(self.ops
                ; .arch x64
                ; jne >plain
                ; cmp DWORD [rbx + seg_field(seg, layout::SEG_BASE)], 0
                ; je =>at
                ; test Rw(r(t)), Rw(r(t))
                ; jz =>at
                ; plain:
            ),
        }
        let end = self.end();
        let (wb, reload) = (self.cache.dirty, self.cache.loaded);
        self.slow.push(Slow::Bail { at, end, ix: self.ix, dirty: self.dirty, wb, reload, leave });
    }

    /// A store of `src` through handle `m` that isn't to plain RAM: if it is
    /// to the VGA's graphics window, with writes there plain ones into the
    /// planes (`JitCtx::vga_ok`), write each plane the map mask selects
    /// and count it in the bus's activity as `Activity::video_write` does,
    /// and go on at `back`; else (and where the write would start a burst
    /// of its own) fall through, to `jit_dev_write`.
    fn vga_store(&mut self, back: DynamicLabel, m: T, src: T, size: u8) {
        let s = size as i32;
        dynasm!(self.ops
            ; .arch x64
            ; bt Rq(r(m)), DEV_BIT as i8
            ; jnc >slow
            ; cmp BYTE [r12 + CTX_VGA_OK], 0
            ; je >slow
            ; mov eax, Rd(r(m))
            ; sub eax, 0xA0000
            ; cmp eax, 0x10000 - s
            ; ja >slow
            ; mov rcx, QWORD [rbx + layout::ICOUNT as i32]
            ; mov rdx, rcx
            ; sub rdx, QWORD [rbx + layout::LAST_WRITE as i32]
            ; cmp rdx, layout::BURST_GAP as i32
            ; ja >slow
            ; add QWORD [rbx + layout::VIDEO_BYTES as i32], s
            ; add QWORD [rbx + layout::BURST as i32], s
            ; mov QWORD [rbx + layout::LAST_WRITE as i32], rcx
            ; mov BYTE [r12 + CTX_VGA_WROTE], 1
        );
        for p in 0..4 {
            dynasm!(self.ops ; .arch x64 ; mov rdx, QWORD [r12 + CTX_VGA_PLANES + p * 8]);
            match size {
                1 => dynasm!(self.ops ; .arch x64 ; mov BYTE [rdx + rax], Rb(r(src))),
                2 => dynasm!(self.ops ; .arch x64 ; mov WORD [rdx + rax], Rw(r(src))),
                _ => dynasm!(self.ops ; .arch x64 ; mov DWORD [rdx + rax], Rd(r(src))),
            }
        }
        dynasm!(self.ops ; .arch x64 ; jmp =>back ; slow:);
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
        // What is known to be constant survives only the operations that
        // can't change it.
        let known = std::mem::take(&mut self.known);
        match *uop {
            Uop::Get { t, .. } | Uop::Const { t, .. } | Uop::Ea { t, .. } => {
                self.known = known;
                self.known[t.0 as usize] = match *uop {
                    Uop::Const { v, .. } => Some(v),
                    Uop::Ea { base: None, index: None, disp, a32, .. } => Some(if a32 { disp } else { disp & 0xFFFF }),
                    _ => None,
                };
            }
            _ => {}
        }
        match *uop {
            Uop::Get { t, r: g } => self.get_into(r(t), g),
            Uop::Set { r: g, t } => self.set_from(g, r(t), RAX),
            Uop::Const { t, v } => dynasm!(self.ops ; .arch x64 ; mov Rd(r(t)), v as i32),
            Uop::LoadCode { t, phys, size, signed } => {
                let (t, at) = (r(t), phys as i32);
                match (size, signed) {
                    (1, false) => dynasm!(self.ops ; .arch x64 ; movzx Rd(t), BYTE [r13 + at]),
                    (1, true) => dynasm!(self.ops ; .arch x64 ; movsx Rd(t), BYTE [r13 + at]),
                    (2, false) => dynasm!(self.ops ; .arch x64 ; movzx Rd(t), WORD [r13 + at]),
                    (2, true) => dynasm!(self.ops ; .arch x64 ; movsx Rd(t), WORD [r13 + at]),
                    _ => dynasm!(self.ops ; .arch x64 ; mov Rd(t), DWORD [r13 + at]),
                }
            }
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
                match base {
                    Some(b) => {
                        let h = match self.cached(b.index) {
                            Some(h) if b.size == 4 => h,
                            _ => {
                                self.get_into(RAX, b);
                                RAX
                            }
                        };
                        dynasm!(self.ops ; .arch x64 ; lea Rd(t), [Rq(h) + disp as i32]);
                    }
                    None => dynasm!(self.ops ; .arch x64 ; mov Rd(t), disp as i32),
                }
                if let Some(i) = index {
                    match self.cached(i.index) {
                        Some(h) if i.size == 4 => match scale {
                            1 => dynasm!(self.ops ; .arch x64 ; lea Rd(t), [Rq(t) + Rq(h)]),
                            2 => dynasm!(self.ops ; .arch x64 ; lea Rd(t), [Rq(t) + Rq(h) * 2]),
                            4 => dynasm!(self.ops ; .arch x64 ; lea Rd(t), [Rq(t) + Rq(h) * 4]),
                            _ => dynasm!(self.ops ; .arch x64 ; lea Rd(t), [Rq(t) + Rq(h) * 8]),
                        },
                        _ => {
                            self.get_into(RAX, i);
                            if scale > 1 {
                                dynasm!(self.ops ; .arch x64 ; shl eax, scale.trailing_zeros() as i8);
                            }
                            dynasm!(self.ops ; .arch x64 ; add Rd(t), eax);
                        }
                    }
                }
                if !a32 {
                    dynasm!(self.ops ; .arch x64 ; movzx Rd(t), Rw(t));
                }
            }
            Uop::MemRef { t, seg, size, write, slot } => self.memref(t, seg, size, write, slot, known[t.0 as usize]),
            Uop::Load { dst, m, size } => {
                let (at, back) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
                let (d, m_) = (r(dst), r(m));
                // (A handle that isn't RAM's has a high bit set, see `SLOW`
                // and `DEV_BIT`.)
                self.ram_or(m, at);
                match size {
                    1 => dynasm!(self.ops ; .arch x64 ; movzx Rd(d), BYTE [r13 + Rq(m_)]),
                    2 => dynasm!(self.ops ; .arch x64 ; movzx Rd(d), WORD [r13 + Rq(m_)]),
                    _ => dynasm!(self.ops ; .arch x64 ; mov Rd(d), DWORD [r13 + Rq(m_)]),
                }
                dynasm!(self.ops ; .arch x64 ; =>back);
                self.slow.push(Slow::Load { at, back, dst, m, size });
            }
            Uop::Store { m, src, size } => {
                let (at, back) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
                let m_ = r(m);
                // RAM of a block without code is written as it is, without
                // bumping its generation, which nothing reads (see
                // `Bus::code_blocks`).
                let code = self.ops.new_dynamic_label();
                self.ram_or(m, at);
                dynasm!(self.ops
                    ; .arch x64
                    ; mov ecx, Rd(m_)
                    ; shr ecx, crate::bus::GEN_SHIFT as i8
                    ; cmp BYTE [r14 + rcx], 0
                    ; jne =>code
                );
                self.store_ram(m, src, size);
                dynasm!(self.ops ; .arch x64 ; =>back);
                let (lo, hi) = self.rest();
                self.slow.push(Slow::Store { at, back, m, src, size, lo, hi });
                self.slow.push(Slow::CodeStore { at: code, back, m, src, size, lo, hi });
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
                let (wb, reload) = (self.cache.dirty, self.cache.loaded);
                self.slow.push(Slow::Bail { at, end, ix: self.ix, dirty: self.dirty, wb, reload, leave: false });
            }
            Uop::BailUnlessRam { t } => {
                let at = self.ops.new_dynamic_label();
                dynasm!(self.ops ; .arch x64 ; cmp Rq(r(t)), RAM_HANDLES ; ja =>at);
                let end = self.end();
                let (wb, reload) = (self.cache.dirty, self.cache.loaded);
                self.slow.push(Slow::Bail { at, end, ix: self.ix, dirty: self.dirty, wb, reload, leave: false });
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
                    self.host_carry_overflow(RAX);
                    self.merge(CF | OF, CF | OF);
                }
            }
            Uop::MulWide { signed, size, t } => self.mul_wide(signed, size, t),
            Uop::DivWide { signed, size, t } => self.div_wide(signed, size, t),
            Uop::BitOp { op, size, t, bit } => {
                let bits = size as i32 * 8;
                match bit {
                    Src::Imm(v) => dynasm!(self.ops ; .arch x64 ; mov ecx, v as i32),
                    Src::T(b) => dynasm!(self.ops ; .arch x64 ; mov ecx, Rd(r(b)) ; and ecx, bits - 1),
                }
                if self.wanted(CF | OF) {
                    // CF is bit 0 of the value rotated right by the bit, OF
                    // whether its top two bits differ (0 or 3 plus 1 has
                    // bit 1 clear, 1 or 2 plus 1 set).
                    dynasm!(self.ops ; .arch x64 ; mov eax, Rd(r(t)));
                    if size == 2 {
                        dynasm!(self.ops ; .arch x64 ; ror ax, cl);
                    } else {
                        dynasm!(self.ops ; .arch x64 ; ror eax, cl);
                    }
                    dynasm!(self.ops
                        ; .arch x64
                        ; mov edx, eax
                        ; shr edx, (bits - 2) as i8
                        ; and edx, 3
                        ; inc edx
                        ; and edx, 2
                        ; shl edx, 10
                        ; and eax, 1
                        ; or eax, edx
                    );
                    self.merge(CF | OF, CF | OF);
                }
                let t = r(t);
                match (op, size) {
                    (BitKind::Test, _) => {}
                    (BitKind::Set, 2) => dynasm!(self.ops ; .arch x64 ; bts Rw(t), cx),
                    (BitKind::Set, _) => dynasm!(self.ops ; .arch x64 ; bts Rd(t), ecx),
                    (BitKind::Reset, 2) => dynasm!(self.ops ; .arch x64 ; btr Rw(t), cx),
                    (BitKind::Reset, _) => dynasm!(self.ops ; .arch x64 ; btr Rd(t), ecx),
                    (BitKind::Complement, 2) => dynasm!(self.ops ; .arch x64 ; btc Rw(t), cx),
                    (BitKind::Complement, _) => dynasm!(self.ops ; .arch x64 ; btc Rd(t), ecx),
                }
            }
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
            Uop::CheckIopl => {
                let (gp, ok) = (self.fault_exit(EXIT_GP0), self.ops.new_dynamic_label());
                dynasm!(self.ops
                    ; .arch x64
                    ; test BYTE [rbx + CR0], crate::cpu::CR0_PE as i8
                    ; jz =>ok
                    ; movzx eax, BYTE [rbx + CPL]
                    ; mov ecx, DWORD [rbx + FLAGS]
                    ; shr ecx, 12
                    ; and ecx, 3
                    ; cmp eax, ecx
                    ; ja =>gp
                    ; =>ok
                );
            }
            Uop::GetSeg { t, seg } => {
                dynasm!(self.ops ; .arch x64 ; movzx Rd(r(t)), WORD [rbx + seg_field(seg, layout::SEG_SELECTOR)]);
            }
            Uop::CheckV86Iopl => {
                let gp = self.fault_exit(EXIT_GP0);
                dynasm!(self.ops
                    ; .arch x64
                    ; mov eax, DWORD [rbx + FLAGS]
                    ; test eax, 0x2_0000
                    ; jz >ok
                    ; and eax, 0x3000
                    ; cmp eax, 0x3000
                    ; jne =>gp
                    ; ok:
                );
            }
            Uop::GetFlags { t, size } => {
                let t = r(t);
                dynasm!(self.ops ; .arch x64 ; mov Rd(t), DWORD [rbx + FLAGS]);
                if self.dirty {
                    dynasm!(self.ops
                        ; .arch x64
                        ; and Rd(t), !ARITH as i32
                        ; mov eax, ebp
                        ; and eax, ARITH as i32
                        ; or Rd(t), eax
                    );
                }
                if size == 2 {
                    dynasm!(self.ops ; .arch x64 ; movzx Rd(t), Rw(t));
                } else {
                    dynasm!(self.ops ; .arch x64 ; and Rd(t), !0x3_0000);
                }
            }
            Uop::CsReal => self.real_load(Seg::CS, None, false),
            Uop::LoadCsReal { t } => {
                let data_ptr = self.data_ptr;
                dynasm!(self.ops
                    ; .arch x64
                    ; mov rax, QWORD data_ptr
                    ; mov QWORD [r12 + CTX_FAR_BLOCK], rax
                    ; mov eax, DWORD [rbx + seg_field(Seg::CS, layout::SEG_BASE)]
                    ; mov DWORD [r12 + CTX_FAR_BASE], eax
                    ; movzx eax, Rw(r(t))
                    ; mov WORD [rbx + seg_field(Seg::CS, layout::SEG_SELECTOR)], ax
                    ; shl eax, 4
                    ; mov DWORD [rbx + seg_field(Seg::CS, layout::SEG_BASE)], eax
                );
            }
            Uop::FarRetPm { sel, off } => {
                // The block and CS base it ran under, as `jit_fallback`
                // notes them for a far transfer.
                let data_ptr = self.data_ptr;
                let (bail, fail) = (self.ops.new_dynamic_label(), self.fail());
                dynasm!(self.ops
                    ; .arch x64
                    ; mov rax, QWORD data_ptr
                    ; mov QWORD [r12 + CTX_FAR_BLOCK], rax
                    ; mov eax, DWORD [rbx + seg_field(Seg::CS, layout::SEG_BASE)]
                    ; mov DWORD [r12 + CTX_FAR_BASE], eax
                );
                self.save_for_call();
                dynasm!(self.ops
                    ; .arch x64
                    ; mov edx, Rd(r(off))
                    ; mov ecx, Rd(r(sel))
                    ; mov rdi, rbx
                    ; mov rsi, r12
                    ; call QWORD [r12 + CTX_FAR_RET]
                );
                self.restore_after_call();
                dynasm!(self.ops
                    ; .arch x64
                    ; test eax, eax
                    ; jz >far_done
                    ; cmp eax, FAR_BAIL as i32
                    ; je =>bail
                    ; jmp =>fail
                    ; far_done:
                );
                let end = self.end();
                let (wb, reload) = (self.cache.dirty, self.cache.loaded);
                self.slow.push(Slow::Bail { at: bail, end, ix: self.ix, dirty: self.dirty, wb, reload, leave: true });
            }
            Uop::FarCallCheck { sel, off } => {
                let (bail, fail) = (self.ops.new_dynamic_label(), self.fail());
                self.far_call(Some((sel, off)));
                dynasm!(self.ops
                    ; .arch x64
                    ; test eax, eax
                    ; jz >far_done
                    ; cmp eax, FAR_BAIL as i32
                    ; je =>bail
                    ; jmp =>fail
                    ; far_done:
                );
                let end = self.end();
                let (wb, reload) = (self.cache.dirty, self.cache.loaded);
                self.slow.push(Slow::Bail { at: bail, end, ix: self.ix, dirty: self.dirty, wb, reload, leave: true });
            }
            Uop::FarCallLoad => {
                let fail = self.fail();
                let data_ptr = self.data_ptr;
                dynasm!(self.ops
                    ; .arch x64
                    ; mov rax, QWORD data_ptr
                    ; mov QWORD [r12 + CTX_FAR_BLOCK], rax
                    ; mov eax, DWORD [rbx + seg_field(Seg::CS, layout::SEG_BASE)]
                    ; mov DWORD [r12 + CTX_FAR_BASE], eax
                );
                self.far_call(None);
                dynasm!(self.ops ; .arch x64 ; test eax, eax ; jnz =>fail);
            }
            Uop::FarExit => {
                // (EIP is the one `jit_far_ret` loaded.)
                let (stop, fail) = (self.ops.new_dynamic_label(), self.fail());
                self.writeback(self.cache.dirty);
                dynasm!(self.ops ; .arch x64 ; cmp BYTE [r12 + CTX_AFTER], 0 ; jne =>stop);
                if self.link {
                    self.far_returned();
                } else {
                    self.flags_back();
                    let tail = self.tail;
                    dynasm!(self.ops ; .arch x64 ; jmp =>tail);
                }
                dynasm!(self.ops
                    ; .arch x64
                    ; =>stop
                    ; mov BYTE [r12 + CTX_AFTER], 0
                    ; mov eax, EXIT_AFTER as i32
                    ; jmp =>fail
                );
            }
            Uop::Spill { t, slot } => {
                dynasm!(self.ops ; .arch x64 ; mov DWORD [r12 + CTX_SCRATCH + slot as i32 * 4], Rd(r(t)));
            }
            Uop::Unspill { t, slot } => {
                dynasm!(self.ops ; .arch x64 ; mov Rd(r(t)), DWORD [r12 + CTX_SCRATCH + slot as i32 * 4]);
            }
            Uop::CheckLimit { src } => {
                self.value_eax(src);
                let gp = self.fault_exit(EXIT_GP0);
                dynasm!(self.ops ; .arch x64 ; cmp eax, DWORD [rbx + seg_field(Seg::CS, layout::SEG_LIMIT)] ; ja =>gp);
            }
            Uop::Exit { eip: Src::Imm(target) } => self.leave(Some(target), 0, true),
            Uop::Exit { eip: Src::T(t) } => {
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + EIP], Rd(r(t)));
                self.writeback(self.cache.dirty);
                if self.link {
                    // A return or indirect call: through its link to
                    // where it goes, if it has one.
                    self.flags_ebp();
                    self.counts();
                    self.check_flat(t);
                    self.returned(t, false);
                } else {
                    self.flags_back();
                    let tail = self.tail;
                    dynasm!(self.ops ; .arch x64 ; jmp =>tail);
                }
            }
            Uop::ExitIf { cond, taken, next, commit } => self.exit_if(cond, taken, next, commit),
            Uop::LoadSeg { seg, t } if self.env.bits & super::ENV_REAL != 0 => {
                // As `Cpu::load_seg_real` loads it once it is a plain
                // segment (which it stays): its handler runs the
                // instruction otherwise, and the block stops after it.
                self.real_load(seg, Some(t), true);
                dynasm!(self.ops
                    ; .arch x64
                    ; movzx eax, Rw(r(t))
                    ; mov WORD [rbx + seg_field(seg, layout::SEG_SELECTOR)], ax
                    ; shl eax, 4
                    ; mov DWORD [rbx + seg_field(seg, layout::SEG_BASE)], eax
                );
            }
            Uop::LoadSeg { seg, t } => {
                let fail = self.fail();
                self.save_for_call();
                dynasm!(self.ops
                    ; .arch x64
                    ; mov ecx, Rd(r(t))
                    ; mov edx, seg as i32
                    ; mov rsi, r12
                    ; mov rdi, rbx
                    ; call QWORD [r12 + CTX_LOAD_SEG]
                );
                self.restore_after_call();
                dynasm!(self.ops ; .arch x64 ; test eax, eax ; jnz =>fail);
                // Its accesses from here on, as if it weren't flat.
                self.loaded_segs |= 1 << seg as u8;
            }
            Uop::In { size, port, t } => self.port_io(false, size, port, t),
            Uop::Out { size, port, t } => self.port_io(true, size, port, t),
            Uop::RepStart { t, count, max } => {
                const TF: i32 = 0x100;
                const DF: i32 = 0x400;
                self.get_into(r(t), count);
                let at = self.ops.new_dynamic_label();
                dynasm!(self.ops
                    ; .arch x64
                    ; test DWORD [rbx + FLAGS], TF | DF
                    ; jnz =>at
                    ; cmp Rd(r(t)), max as i32
                    ; ja =>at
                );
                let end = self.end();
                let (wb, reload) = (self.cache.dirty, self.cache.loaded);
                self.slow.push(Slow::Bail { at, end, ix: self.ix, dirty: self.dirty, wb, reload, leave: false });
                let top = self.ops.new_dynamic_label();
                dynasm!(self.ops
                    ; .arch x64
                    ; test Rd(r(t)), Rd(r(t))
                    ; jz =>end
                    ; =>top
                );
                self.rep_top = Some(top);
            }
            Uop::RepEnd { t } => {
                let top = self.rep_top.take().expect("RepStart before RepEnd");
                dynasm!(self.ops ; .arch x64 ; test Rd(r(t)), Rd(r(t)) ; jnz =>top);
            }
            Uop::Forward => {
                const DF: i32 = 0x400;
                let at = self.ops.new_dynamic_label();
                dynasm!(self.ops ; .arch x64 ; test DWORD [rbx + FLAGS], DF ; jnz =>at);
                let end = self.end();
                let (wb, reload) = (self.cache.dirty, self.cache.loaded);
                self.slow.push(Slow::Bail { at, end, ix: self.ix, dirty: self.dirty, wb, reload, leave: false });
            }
            Uop::FpuGuard { valid } => self.fpu_guard(valid),
            Uop::FGet { x, i } => {
                self.fpu_phys(RAX, i);
                dynasm!(self.ops ; .arch x64 ; movsd Rx(x.0), QWORD [rbx + rax * 8 + FPU_F64]);
            }
            Uop::FSet { i, x, canon } => {
                if canon {
                    self.fpu_canon(x.0);
                }
                self.fpu_phys(RAX, i);
                dynasm!(self.ops
                    ; .arch x64
                    ; movsd QWORD [rbx + rax * 8 + FPU_F64], Rx(x.0)
                    ; mov BYTE [rbx + rax + FPU_STALE], 1
                );
            }
            Uop::FPush { x, canon } => {
                self.fpu_known = self.fpu_known << 1 | 1;
                if canon {
                    self.fpu_canon(x.0);
                }
                dynasm!(self.ops
                    ; .arch x64
                    ; mov eax, DWORD [rbx + FPU_TOP]
                    ; dec eax
                    ; and eax, 7
                    ; mov QWORD [rbx + FPU_TOP], rax
                    ; movsd QWORD [rbx + rax * 8 + FPU_F64], Rx(x.0)
                    ; mov BYTE [rbx + rax + FPU_STALE], 1
                    ; mov BYTE [rbx + rax + FPU_TAGS], FPU_VALID
                );
            }
            Uop::FPop { n } => {
                self.fpu_known >>= n;
                dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + FPU_TOP]);
                for _ in 0..n {
                    dynasm!(self.ops
                        ; .arch x64
                        ; mov BYTE [rbx + rax + FPU_TAGS], FPU_EMPTY
                        ; inc eax
                        ; and eax, 7
                    );
                }
                dynasm!(self.ops ; .arch x64 ; mov QWORD [rbx + FPU_TOP], rax);
            }
            Uop::FCopy { dst, src } => {
                // RCX the source's physical number, RDX the destination's.
                dynasm!(self.ops
                    ; .arch x64
                    ; mov eax, DWORD [rbx + FPU_TOP]
                    ; lea ecx, [rax + src as i32]
                    ; and ecx, 7
                );
                if dst.is_none() {
                    self.fpu_known = self.fpu_known << 1 | 1;
                }
                match dst {
                    Some(dst) => dynasm!(self.ops ; .arch x64 ; lea edx, [rax + dst as i32] ; and edx, 7),
                    None => dynasm!(self.ops
                        ; .arch x64
                        ; lea edx, [rax - 1]
                        ; and edx, 7
                        ; mov QWORD [rbx + FPU_TOP], rdx
                        ; mov BYTE [rbx + rdx + FPU_TAGS], FPU_VALID
                    ),
                }
                dynasm!(self.ops
                    ; .arch x64
                    ; movsd xmm2, QWORD [rbx + rcx * 8 + FPU_F64]
                    ; movsd QWORD [rbx + rdx * 8 + FPU_F64], xmm2
                    ; movzx eax, BYTE [rbx + rcx + FPU_STALE]
                    ; mov BYTE [rbx + rdx + FPU_STALE], al
                    ; shl ecx, 4
                    ; shl edx, 4
                    ; movdqu xmm2, OWORD [rbx + rcx + FPU_X80]
                    ; movdqu OWORD [rbx + rdx + FPU_X80], xmm2
                );
            }
            Uop::FXch { i } => {
                const C1: i16 = 0x200;
                // RAX ST(0)'s physical number, RCX ST(i)'s.
                dynasm!(self.ops
                    ; .arch x64
                    ; mov eax, DWORD [rbx + FPU_TOP]
                    ; lea ecx, [rax + i as i32]
                    ; and ecx, 7
                    ; movsd xmm2, QWORD [rbx + rax * 8 + FPU_F64]
                    ; movsd xmm3, QWORD [rbx + rcx * 8 + FPU_F64]
                    ; movsd QWORD [rbx + rax * 8 + FPU_F64], xmm3
                    ; movsd QWORD [rbx + rcx * 8 + FPU_F64], xmm2
                    ; movzx edx, BYTE [rbx + rax + FPU_STALE]
                    ; shl edx, 8
                    ; mov dl, BYTE [rbx + rcx + FPU_STALE]
                    ; mov BYTE [rbx + rax + FPU_STALE], dl
                    ; mov BYTE [rbx + rcx + FPU_STALE], dh
                    ; shl eax, 4
                    ; shl ecx, 4
                    ; movdqu xmm2, OWORD [rbx + rax + FPU_X80]
                    ; movdqu xmm3, OWORD [rbx + rcx + FPU_X80]
                    ; movdqu OWORD [rbx + rax + FPU_X80], xmm3
                    ; movdqu OWORD [rbx + rcx + FPU_X80], xmm2
                    ; and WORD [rbx + FPU_FLAGS], !C1
                );
            }
            Uop::FFromT { x, t, kind } => match kind {
                FKind::Single => dynasm!(self.ops ; .arch x64 ; movd Rx(x.0), Rd(r(t)) ; cvtss2sd Rx(x.0), Rx(x.0)),
                FKind::Int => dynasm!(self.ops ; .arch x64 ; pxor Rx(x.0), Rx(x.0) ; cvtsi2sd Rx(x.0), Rd(r(t))),
            },
            Uop::FToX80 { t, part } => {
                // From the 80 bits where they aren't stale, else the
                // handlers'.
                dynasm!(self.ops
                    ; .arch x64
                    ; mov eax, DWORD [rbx + FPU_TOP]
                    ; cmp BYTE [rbx + rax + FPU_STALE], 0
                    ; jne >x80_stale
                    ; shl eax, 4
                );
                if part == 2 {
                    dynasm!(self.ops ; .arch x64 ; movzx Rd(r(t)), WORD [rbx + rax + FPU_X80 + 8]);
                } else {
                    dynasm!(self.ops ; .arch x64 ; mov Rd(r(t)), DWORD [rbx + rax + FPU_X80 + part as i32 * 4]);
                }
                dynasm!(self.ops ; .arch x64 ; jmp >x80_done ; x80_stale:);
                self.fpu_op(3 << 13 | part as i32, None);
                dynasm!(self.ops ; .arch x64 ; mov Rd(r(t)), eax ; x80_done:);
            }
            Uop::FToSingle { t, x } => {
                dynasm!(self.ops ; .arch x64 ; cvtsd2ss xmm2, Rx(x.0) ; movd Rd(r(t)), xmm2);
            }
            Uop::FLoad64 { x, m, canon } => {
                dynasm!(self.ops ; .arch x64 ; movsd Rx(x.0), QWORD [r13 + Rq(r(m))]);
                if canon {
                    self.fpu_canon(x.0);
                }
            }
            Uop::FToHalf { t, x, high } => {
                dynasm!(self.ops ; .arch x64 ; movq rax, Rx(x.0));
                if high {
                    dynasm!(self.ops ; .arch x64 ; shr rax, 32);
                }
                dynasm!(self.ops ; .arch x64 ; mov Rd(r(t)), eax);
            }
            Uop::FToInt { t, x, size } => self.fpu_to_int(t, x, size),
            Uop::FMul { a, b } => dynasm!(self.ops ; .arch x64 ; mulsd Rx(a.0), Rx(b.0)),
            Uop::FDiv { i, num, den, ze } => {
                dynasm!(self.ops
                    ; .arch x64
                    ; xorpd xmm2, xmm2
                    ; ucomisd Rx(den.0), xmm2
                    ; jne >fdiv_go
                    ; jp >fdiv_go
                );
                self.save_for_call();
                dynasm!(self.ops
                    ; .arch x64
                    ; mov edx, i as i32 | (ze as i32) << 8
                    ; mov rsi, r12
                    ; mov rdi, rbx
                    ; call QWORD [r12 + CTX_FPU + 24]
                );
                self.restore_after_call();
                dynasm!(self.ops
                    ; .arch x64
                    ; jmp >fdiv_done
                    ; fdiv_go:
                    ; movsd xmm2, Rx(num.0)
                    ; divsd xmm2, Rx(den.0)
                );
                self.fpu_canon(2);
                self.fpu_phys(RAX, i);
                dynasm!(self.ops
                    ; .arch x64
                    ; movsd QWORD [rbx + rax * 8 + FPU_F64], xmm2
                    ; mov BYTE [rbx + rax + FPU_STALE], 1
                    ; fdiv_done:
                );
            }
            Uop::FAddSt { dst, a, b, sub } => {
                self.fpu_addsub([a, b, dst], sub, None);
                dynasm!(self.ops ; .arch x64 ; test eax, eax ; jz >addsub_done);
                let desc = dst as i32 | (a as i32) << 4 | (b as i32) << 8 | (sub as i32) << 12;
                self.fpu_op(desc, None);
                dynasm!(self.ops ; .arch x64 ; addsub_done:);
            }
            Uop::FChs => self.fpu_op(1 << 13, None),
            Uop::FLoad80 { m } => self.fpu_op(2 << 13, Some(m)),
            Uop::FAddValue { kind, x } => {
                use crate::instructions::fpu::arithmetic::{ADD_VALUE, SUB_VALUE};
                // 8: the double in X.
                let (a, b, sub) = match kind {
                    ADD_VALUE => (0, 8, false),
                    SUB_VALUE => (0, 8, true),
                    _ => (8, 0, true),
                };
                self.fpu_addsub([a, b, 0], sub, Some(x));
                dynasm!(self.ops ; .arch x64 ; test eax, eax ; jz >addsub_done);
                self.save_for_call();
                if x.0 != 0 {
                    dynasm!(self.ops ; .arch x64 ; movsd xmm0, Rx(x.0));
                }
                dynasm!(self.ops
                    ; .arch x64
                    ; mov edx, kind as i32
                    ; mov rsi, r12
                    ; mov rdi, rbx
                    ; call QWORD [r12 + CTX_FPU + 8]
                );
                self.restore_after_call();
                dynasm!(self.ops ; .arch x64 ; addsub_done:);
            }
            Uop::FCom { a, b } => {
                // C0, C2 and C3 are where CF, PF and ZF are in AH's flags.
                const C0_C2_C3: i16 = 0x4500;
                dynasm!(self.ops
                    ; .arch x64
                    ; and WORD [rbx + FPU_FLAGS], !C0_C2_C3
                    ; ucomisd Rx(a.0), Rx(b.0)
                    ; mov eax, 0
                    ; mov ecx, 0
                    ; mov edx, 0
                    ; setb al
                    ; setp cl
                    ; sete dl
                    ; shl eax, 8
                    ; shl ecx, 10
                    ; shl edx, 14
                    ; or eax, ecx
                    ; or eax, edx
                    ; or WORD [rbx + FPU_FLAGS], ax
                );
            }
            Uop::FStatus { t } => {
                let t = r(t);
                dynasm!(self.ops
                    ; .arch x64
                    ; movzx Rd(t), WORD [rbx + FPU_FLAGS]
                    ; and Rd(t), !0x3800
                    ; mov eax, DWORD [rbx + FPU_TOP]
                    ; and eax, 7
                    ; shl eax, 11
                    ; or Rd(t), eax
                );
            }
            Uop::FGetControl { t } => dynasm!(self.ops ; .arch x64 ; movzx Rd(r(t)), WORD [rbx + FPU_CONTROL]),
            Uop::FSetControl { t } => dynasm!(self.ops ; .arch x64 ; mov WORD [rbx + FPU_CONTROL], Rw(r(t))),
            Uop::Sti => {
                const IF: i32 = 0x200;
                let data = self.data;
                let next = data.eips[self.ix].wrapping_add(data.instrs[self.ix].len() as u32);
                let at = self.ops.new_dynamic_label();
                let fail = self.fail();
                let code = EXIT_AFTER | if self.dirty { EXIT_FLAGS } else { 0 };
                self.slow.push(Slow::After { at, next, code, fail });
                // Interrupts are recognized after the next instruction: with
                // one waiting, the execution loop runs that; else the shadow
                // ends where the next instruction runs, in the block.
                dynasm!(self.ops
                    ; .arch x64
                    ; mov eax, DWORD [rbx + FLAGS]
                    ; or DWORD [rbx + FLAGS], IF
                    ; test eax, IF
                    ; jnz >was_set
                    ; mov BYTE [rbx + layout::IRQ_SHADOW as i32], 1
                    ; was_set:
                    ; cmp BYTE [rbx + layout::IRQ_READY as i32], 0
                    ; jne =>at
                );
                if self.ix + 1 < data.count() {
                    dynasm!(self.ops ; .arch x64 ; mov BYTE [rbx + layout::IRQ_SHADOW as i32], 0);
                }
            }
        }
    }

    /// `reg` (RAX or RCX) = ST(i)'s physical number.
    fn fpu_phys(&mut self, reg: u8, i: u8) {
        dynasm!(self.ops ; .arch x64 ; mov Rd(reg), DWORD [rbx + FPU_TOP]);
        if i != 0 {
            dynasm!(self.ops ; .arch x64 ; add Rd(reg), i as i32 ; and Rd(reg), 7);
        }
    }

    /// `Uop::FpuGuard`: the instruction's handler runs it after all where
    /// CR0 has EM or TS set, or one of the registers in `valid` is empty.
    fn fpu_guard(&mut self, valid: u8) {
        const EM_TS: i8 = 0x0C;
        // CR0 doesn't change within a block, and the registers the block
        // found or made valid stay so (`fpu_known`): checked once.
        let tags = valid & !self.fpu_known;
        if self.fpu_cr0_checked && tags == 0 {
            return;
        }
        let at = self.ops.new_dynamic_label();
        if !self.fpu_cr0_checked {
            dynasm!(self.ops ; .arch x64 ; test BYTE [rbx + CR0], EM_TS ; jnz =>at);
        }
        if tags != 0 {
            dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + FPU_TOP]);
        }
        for i in (0..8).filter(|i| tags >> i & 1 != 0) {
            if i == 0 {
                dynasm!(self.ops ; .arch x64 ; cmp BYTE [rbx + rax + FPU_TAGS], FPU_EMPTY ; je =>at);
            } else {
                dynasm!(self.ops
                    ; .arch x64
                    ; lea ecx, [rax + i]
                    ; and ecx, 7
                    ; cmp BYTE [rbx + rcx + FPU_TAGS], FPU_EMPTY
                    ; je =>at
                );
            }
        }
        self.fpu_cr0_checked = true;
        self.fpu_known |= valid;
        // The handler runs the instruction then, and the block stops after
        // it: what the code after it knows of the registers holds only
        // where this one's operations ran.
        let end = self.end();
        let (wb, reload) = (self.cache.dirty, self.cache.loaded);
        self.slow.push(Slow::Bail { at, end, ix: self.ix, dirty: self.dirty, wb, reload, leave: true });
    }

    /// Make the double in XMM register `x` what an FPU register holds
    /// (`f80::canon_f64`): a denormal 0, a NaN quiet.
    fn fpu_canon(&mut self, x: u8) {
        dynasm!(self.ops
            ; .arch x64
            ; movq rcx, Rx(x)
            ; mov rdx, rcx
            ; shr rdx, 52
            ; and edx, 0x7FF
            ; dec edx
            ; cmp edx, 0x7FE
            ; jb >canon_done
            ; inc edx
            ; jnz >canon_nan
            // A zero or a denormal: the sign alone.
            ; shr rcx, 63
            ; shl rcx, 63
            ; jmp >canon_fix
            ; canon_nan:
            ; mov rdx, rcx
            ; shl rdx, 12
            ; jz >canon_done
            ; bts rcx, 51
            ; canon_fix:
            ; movq Rx(x), rcx
            ; canon_done:
        );
    }

    /// Call `jit_far_call`: its checks of `sel:off`, or (None) its load.
    fn far_call(&mut self, target: Option<(T, T)>) {
        self.save_for_call();
        match target {
            Some((sel, off)) => dynasm!(self.ops
                ; .arch x64
                ; mov edx, Rd(r(off))
                ; mov ecx, Rd(r(sel))
                ; xor r8d, r8d
            ),
            None => dynasm!(self.ops ; .arch x64 ; mov r8d, 1),
        }
        dynasm!(self.ops
            ; .arch x64
            ; mov rdi, rbx
            ; mov rsi, r12
            ; call QWORD [r12 + CTX_FAR_CALL]
        );
        self.restore_after_call();
    }

    /// Call the trampoline's `fpu_addsub` for ST(dst) = ST(a) + ST(b) (or
    /// minus, with `sub`), of `regs` = [a, b, dst] (8 for the double in X):
    /// EAX is 0 where it did, else the handler's call is to follow.
    fn fpu_addsub(&mut self, regs: [u8; 3], sub: bool, x: Option<X>) {
        if let Some(x) = x {
            dynasm!(self.ops ; .arch x64 ; movq xmm15, Rx(x.0));
        }
        dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + FPU_TOP]);
        for (reg, i) in [8u8, 9, 10].into_iter().zip(regs) {
            match i {
                8 => dynasm!(self.ops ; .arch x64 ; mov Rd(reg), 8),
                0 => dynasm!(self.ops ; .arch x64 ; mov Rd(reg), eax),
                _ => dynasm!(self.ops ; .arch x64 ; lea Rd(reg), [rax + i as i32] ; and Rd(reg), 7),
            }
        }
        if sub {
            dynasm!(self.ops ; .arch x64 ; or r10d, 0x100);
        }
        dynasm!(self.ops ; .arch x64 ; call QWORD [r12 + CTX_FPU + 32]);
    }

    /// Call `jit_fpu_addsub_st` with `desc` and the handle in `arg`.
    fn fpu_op(&mut self, desc: i32, arg: Option<T>) {
        self.save_for_call();
        if let Some(t) = arg {
            dynasm!(self.ops ; .arch x64 ; mov ecx, Rd(r(t)));
        }
        dynasm!(self.ops
            ; .arch x64
            ; mov edx, desc
            ; mov rsi, r12
            ; mov rdi, rbx
            ; call QWORD [r12 + CTX_FPU]
        );
        self.restore_after_call();
    }

    /// `Uop::FToInt`: the conversion inline where the control word rounds
    /// to nearest or chops and the result fits, else through `jit_fpu_to_int`.
    fn fpu_to_int(&mut self, t: T, x: X, size: u8) {
        let t_ = r(t);
        dynasm!(self.ops
            ; .arch x64
            ; movzx eax, WORD [rbx + FPU_CONTROL]
            ; and eax, 0xC00
            ; jnz >toint_other
            ; cvtsd2si Rd(t_), Rx(x.0)
            ; jmp >toint_check
            ; toint_other:
            ; cmp eax, 0xC00
            ; jne >toint_slow
            ; cvttsd2si Rd(t_), Rx(x.0)
            ; toint_check:
        );
        if size == 2 {
            dynasm!(self.ops ; .arch x64 ; movsx eax, Rw(t_) ; cmp eax, Rd(t_) ; je >toint_done);
        } else {
            dynasm!(self.ops ; .arch x64 ; cmp Rd(t_), i32::MIN ; jne >toint_done);
        }
        dynasm!(self.ops ; .arch x64 ; toint_slow:);
        self.save_for_call();
        if x.0 != 0 {
            dynasm!(self.ops ; .arch x64 ; movsd xmm0, Rx(x.0));
        }
        dynasm!(self.ops
            ; .arch x64
            ; mov edx, size as i32
            ; mov rsi, r12
            ; mov rdi, rbx
            ; call QWORD [r12 + CTX_FPU + 16]
        );
        self.restore_after_call();
        dynasm!(self.ops ; .arch x64 ; mov Rd(t_), eax ; toint_done:);
    }

    /// Keep the temporaries and the cached guest registers around a call
    /// into Rust, which doesn't keep them (16 bytes aligned).
    fn save_for_call(&mut self) {
        dynasm!(self.ops
            ; .arch x64
            ; push r8
            ; push r9
            ; push r10
            ; push r11
            ; push rsi
            ; push rdi
        );
    }

    fn restore_after_call(&mut self) {
        dynasm!(self.ops
            ; .arch x64
            ; pop rdi
            ; pop rsi
            ; pop r11
            ; pop r10
            ; pop r9
            ; pop r8
        );
    }

    /// IN (into t) or OUT (of t) of `size` bytes through `jit_port`, the
    /// instruction count brought up to date before the instruction.
    fn port_io(&mut self, out: bool, size: u8, port: Src, t: T) {
        let fail = self.fail();
        let desc = out as i32 | (size as i32) << 1 | (self.ix as i32) << 8;
        let data_ptr = self.data_ptr;
        self.save_for_call();
        match port {
            Src::Imm(p) => dynasm!(self.ops ; .arch x64 ; mov eax, p as i32),
            Src::T(p) => dynasm!(self.ops ; .arch x64 ; mov eax, Rd(r(p))),
        }
        if out {
            dynasm!(self.ops ; .arch x64 ; mov r9d, Rd(r(t)));
        }
        dynasm!(self.ops
            ; .arch x64
            ; mov r8d, eax
            ; mov ecx, desc
            ; mov rdx, QWORD data_ptr
            ; mov rsi, r12
            ; mov rdi, rbx
            ; call QWORD [r12 + CTX_PORT]
        );
        self.restore_after_call();
        dynasm!(self.ops
            ; .arch x64
            ; mov rcx, rax
            ; shr rcx, 32
            ; jz >done
            ; mov eax, ecx
            ; jmp =>fail
            ; done:
        );
        if !out {
            dynasm!(self.ops ; .arch x64 ; mov Rd(r(t)), eax);
        }
    }

    /// Store `size` bytes of src into RAM at handle m.
    fn store_ram(&mut self, m: T, src: T, size: u8) {
        let (m_, s) = (r(m), r(src));
        match size {
            1 => dynasm!(self.ops ; .arch x64 ; mov BYTE [r13 + Rq(m_)], Rb(s)),
            2 => dynasm!(self.ops ; .arch x64 ; mov WORD [r13 + Rq(m_)], Rw(s)),
            _ => dynasm!(self.ops ; .arch x64 ; mov DWORD [r13 + Rq(m_)], Rd(s)),
        }
    }

    /// The host register guest register `index` is kept in, if any.
    fn cached(&self, index: u8) -> Option<u8> {
        let h = self.cache.host[index as usize];
        (h != 0).then_some(h)
    }

    /// Load the cached guest registers in `mask` that aren't in their host
    /// registers yet.
    fn preload(&mut self, mask: u8) {
        let missing = mask & !self.cache.loaded;
        self.load_cached(missing);
        self.cache.loaded |= missing;
    }

    /// Load the cached guest registers in `mask` from the CPU.
    fn load_cached(&mut self, mask: u8) {
        for i in 0..8u8 {
            if let Some(h) = self.cached(i).filter(|_| mask >> i & 1 != 0) {
                dynasm!(self.ops ; .arch x64 ; mov Rd(h), DWORD [rbx + gpr_offset(Gpr::dword(i))]);
            }
        }
    }

    /// Put the cached guest registers in `mask` back into the CPU.
    fn writeback(&mut self, mask: u8) {
        for i in 0..8u8 {
            if let Some(h) = self.cached(i).filter(|_| mask >> i & 1 != 0) {
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + gpr_offset(Gpr::dword(i))], Rd(h));
            }
        }
    }

    /// Host register `dst` = guest register `g`, zero-extended.
    fn get_into(&mut self, dst: u8, g: Gpr) {
        if let Some(h) = self.cached(g.index) {
            debug_assert!(self.cache.loaded >> g.index & 1 != 0);
            match (g.size, g.high) {
                (4, _) => dynasm!(self.ops ; .arch x64 ; mov Rd(dst), Rd(h)),
                (2, _) => dynasm!(self.ops ; .arch x64 ; movzx Rd(dst), Rw(h)),
                (_, false) => dynasm!(self.ops ; .arch x64 ; movzx Rd(dst), Rb(h)),
                (_, true) => dynasm!(self.ops ; .arch x64 ; movzx Rd(dst), Rw(h) ; shr Rd(dst), 8),
            }
            return;
        }
        let off = gpr_offset(g);
        match g.size {
            4 => dynasm!(self.ops ; .arch x64 ; mov Rd(dst), DWORD [rbx + off]),
            2 => dynasm!(self.ops ; .arch x64 ; movzx Rd(dst), WORD [rbx + off]),
            _ => dynasm!(self.ops ; .arch x64 ; movzx Rd(dst), BYTE [rbx + off]),
        }
    }

    /// Guest register `g` = the low bytes of host register `src` (the rest
    /// of its slot stays), with `scratch` (another register) changed. In
    /// the CPU, a word or low byte is stored as wide as it is (faster than
    /// merging it into the dword, though a dword load of it then waits for
    /// the store: measured).
    fn set_from(&mut self, g: Gpr, src: u8, scratch: u8) {
        if let Some(h) = self.cached(g.index) {
            debug_assert!(self.cache.loaded >> g.index & 1 != 0);
            self.cache.dirty |= 1 << g.index;
            match (g.size, g.high) {
                (4, _) => dynasm!(self.ops ; .arch x64 ; mov Rd(h), Rd(src)),
                (2, _) => dynasm!(self.ops ; .arch x64 ; mov Rw(h), Rw(src)),
                (_, false) => dynasm!(self.ops ; .arch x64 ; mov Rb(h), Rb(src)),
                (_, true) => dynasm!(self.ops ; .arch x64 ; ror Rd(h), 8 ; mov Rb(h), Rb(src) ; rol Rd(h), 8),
            }
            return;
        }
        let off = gpr_offset(Gpr::dword(g.index));
        match (g.size, g.high) {
            (4, _) => dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + off], Rd(src)),
            (2, _) => dynasm!(self.ops ; .arch x64 ; mov WORD [rbx + off], Rw(src)),
            (_, false) => dynasm!(self.ops ; .arch x64 ; mov BYTE [rbx + off], Rb(src)),
            (_, true) => dynasm!(self.ops
                ; .arch x64
                ; mov Rd(scratch), DWORD [rbx + off]
                ; ror Rd(scratch), 8
                ; mov Rb(scratch), Rb(src)
                ; rol Rd(scratch), 8
                ; mov DWORD [rbx + off], Rd(scratch)
            ),
        }
    }

    fn value_eax(&mut self, src: Src) {
        match src {
            Src::T(t) => dynasm!(self.ops ; .arch x64 ; mov eax, Rd(r(t))),
            Src::Imm(v) => dynasm!(self.ops ; .arch x64 ; mov eax, v as i32),
        }
    }

    /// Check the operand at seg:t as `Cpu::mem_ref` does, inline for plain
    /// RAM, through the TLB with paging on, and leave its handle in t. The
    /// code is translated for paging, the A20 gate, CPL and the flat
    /// segments as the block's `Env` has them.
    fn memref(&mut self, t: T, seg: Seg, size: u8, write: bool, slot: u8, known: Option<u32>) {
        let (at, back) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        // Where an operand that isn't plain RAM goes (`Slow::Dev`).
        let dev = self.ops.new_dynamic_label();
        let t_ = r(t);
        let bits = self.env.bits;
        let (paging, a20) = (bits & super::ENV_PAGING != 0, bits & super::ENV_A20 != 0);
        let unloaded = self.loaded_segs >> seg as u8 & 1 == 0;
        let flat = bits & super::ENV_FLAT << seg as u32 != 0 && unloaded;
        let plain = bits & super::ENV_PLAIN << seg as u32 != 0 && unloaded;
        let big = bits & super::ENV_BIG << seg as u32 != 0 && unloaded;
        let last = size as i32 - 1;
        // The linear address: a flat segment's is the offset, whose wrapping
        // around past a dword the check for the end of RAM below catches (it
        // takes it to `jit_memref`, which faults). Otherwise the segment's
        // limit and type, as `seg_linear` checks them (a byte is its own
        // last byte, which can't wrap around), and its base; a plain
        // segment's offsets start at 0, and it may be read and written.
        let addr = if flat && !paging && a20 {
            t_
        } else {
            if flat {
                dynasm!(self.ops ; .arch x64 ; mov eax, Rd(t_));
            } else {
                let need = if write { layout::RIGHT_WRITE } else { layout::RIGHT_READ };
                let (lo, hi, rights, base) = (
                    seg_field(seg, layout::SEG_LO),
                    seg_field(seg, layout::SEG_HI),
                    seg_field(seg, layout::SEG_RIGHTS),
                    seg_field(seg, layout::SEG_BASE),
                );
                // (A constant offset's last byte, where it doesn't wrap.)
                let end = known.and_then(|v| v.checked_add(last as u32));
                if let (true, Some(end)) = (size > 1, end) {
                    if !plain {
                        dynasm!(self.ops ; .arch x64 ; cmp Rd(t_), DWORD [rbx + lo] ; jb =>at);
                    }
                    if !big {
                        dynasm!(self.ops ; .arch x64 ; cmp DWORD [rbx + hi], end as i32 ; jb =>at);
                    }
                } else if size > 1 {
                    dynasm!(self.ops
                        ; .arch x64
                        ; lea ecx, [Rq(t_) + last]
                        ; cmp ecx, Rd(t_)
                        ; jb =>at
                    );
                    if !plain {
                        dynasm!(self.ops ; .arch x64 ; cmp Rd(t_), DWORD [rbx + lo] ; jb =>at);
                    }
                    if !big {
                        dynasm!(self.ops ; .arch x64 ; cmp ecx, DWORD [rbx + hi] ; ja =>at);
                    }
                } else if !big {
                    // (Every offset is in a big segment's limits.)
                    if !plain {
                        dynasm!(self.ops ; .arch x64 ; cmp Rd(t_), DWORD [rbx + lo] ; jb =>at);
                    }
                    dynasm!(self.ops ; .arch x64 ; cmp Rd(t_), DWORD [rbx + hi] ; ja =>at);
                }
                if !plain {
                    dynasm!(self.ops ; .arch x64 ; test BYTE [rbx + rights], need as i8 ; jz =>at);
                }
                dynasm!(self.ops
                    ; .arch x64
                    ; mov eax, Rd(t_)
                    ; add eax, DWORD [rbx + base]
                );
            }
            0
        };
        let tag = (if write { layout::TLB_WRITE_TAG } else { layout::TLB_READ_TAG }) as i32;
        let set = if bits & super::ENV_USER != 0 { layout::TLB_SET as i32 } else { 0 };
        let entry = set * TLB_ENTRY;
        if paging && a20 {
            // The TLB entry of the first byte's page (linear address >> 12)
            // in the set of the privilege level, whose tag for this code
            // must be the last byte's page: an operand in two pages misses,
            // as the entry can't hold the next page, and so does a page that
            // isn't plain RAM. The entry's delta takes the address to RAM.
            let jit_tag = (if write { layout::TLB_JIT_WRITE } else { layout::TLB_JIT_READ }) as i32;
            if size > 1 {
                dynasm!(self.ops ; .arch x64 ; lea ecx, [rax + last]);
            } else {
                dynasm!(self.ops ; .arch x64 ; mov ecx, eax);
            }
            dynasm!(self.ops
                ; .arch x64
                ; and ecx, !0xFFF
                ; mov edx, eax
                ; shr edx, 12 - TLB_ENTRY_SHIFT
                ; and edx, ((layout::TLB_SET - 1) << TLB_ENTRY_SHIFT) as i32
                ; cmp ecx, DWORD [rbx + rdx + TLB + entry + jit_tag]
                ; jne =>dev
                ; add eax, DWORD [rbx + rdx + TLB + entry + layout::TLB_JIT_DELTA as i32]
                ; mov Rd(t_), eax
            );
            self.ram_back = Some((t, back));
            let desc = memref_desc(seg, size, write, slot);
            let fail = self.fail();
            self.slow.push(Slow::Dev { at: dev, back, slow: at, t, addr: RAX, size, tlb: Some((entry, tag)) });
            self.slow.push(Slow::MemRef { at, back, t, desc, fail });
            return;
        }
        if paging || !a20 {
            // An operand in two pages takes two translations (or with the
            // A20 gate closed, may wrap around at a megabyte).
            if size > 1 {
                dynasm!(self.ops
                    ; .arch x64
                    ; mov ecx, eax
                    ; and ecx, 0xFFF
                    ; cmp ecx, 0x1000 - size as i32
                    ; ja =>at
                );
            }
            if paging {
                // The page's entry (linear address >> 12) in the TLB's set of
                // the privilege level, whose tag must be the page + 1.
                dynasm!(self.ops
                    ; .arch x64
                    ; mov edx, eax
                    ; shr edx, 12 - TLB_ENTRY_SHIFT
                    ; and edx, ((layout::TLB_SET - 1) << TLB_ENTRY_SHIFT) as i32
                    ; mov ecx, eax
                    ; shr ecx, 12
                    ; inc ecx
                    ; cmp ecx, DWORD [rbx + rdx + TLB + entry + tag]
                    ; jne =>at
                    ; and eax, 0xFFF
                    ; or eax, DWORD [rbx + rdx + TLB + entry + layout::TLB_PHYS as i32]
                );
            }
            if !a20 {
                dynasm!(self.ops ; .arch x64 ; and eax, !0x10_0000);
            }
        }
        // In plain RAM: none of its bytes in the video memory and ROMs from
        // A0000h to FFFFFh, and not past the end of RAM. (With paging off
        // and the A20 gate open, an operand in two pages of RAM is too: they
        // are next to each other.) A flat segment's constant offset is known
        // to be.
        let in_ram = |v: u32| {
            let end = v as u64 + last as u64;
            (end < VIDEO as u64 || v >= EXTENDED) && v <= self.env.ram_len.wrapping_sub(size as u32) && size as u32 <= self.env.ram_len
        };
        let extended = self.env.ram_len.checked_sub(EXTENDED + size as u32);
        if addr == t_ && known.is_some_and(in_ram) {
        } else if let (Some(limit), true) = (extended, bits & 1 != 0) {
            // 32-bit code's data is mostly in extended memory: looked at
            // first (the address's last byte below the video memory, in 64
            // bits, the other way into RAM).
            dynasm!(self.ops
                ; .arch x64
                ; lea ecx, [Rq(addr) - EXTENDED as i32]
                ; cmp ecx, limit as i32
                ; jbe >in_ram
                ; lea rcx, [Rq(addr) + last]
                ; cmp rcx, VIDEO as i32
                ; jae =>dev
                ; in_ram:
            );
        } else {
            dynasm!(self.ops
                ; .arch x64
                ; lea ecx, [Rq(addr) + last - VIDEO as i32]
                ; cmp ecx, (EXTENDED - VIDEO) as i32 + last
                ; jb =>dev
                ; cmp Rd(addr), self.env.ram_len.wrapping_sub(size as u32) as i32
                ; ja =>dev
            );
        }
        if addr != t_ {
            dynasm!(self.ops ; .arch x64 ; mov Rd(t_), eax);
        }
        self.ram_back = Some((t, back));
        let desc = memref_desc(seg, size, write, slot);
        let fail = self.fail();
        self.slow.push(Slow::Dev { at: dev, back, slow: at, t, addr, size, tlb: None });
        self.slow.push(Slow::MemRef { at, back, t, desc, fail });
    }

    /// Where a memory reference's slow paths come back to, if it is still
    /// pending (`ram_back`).
    fn settle_ram(&mut self) {
        if let Some((_, back)) = self.ram_back.take() {
            dynasm!(self.ops ; .arch x64 ; =>back);
        }
    }

    /// Go to `slow` unless handle `m` is RAM's: right after the memory
    /// reference that made it, only where its slow paths made it.
    fn ram_or(&mut self, m: T, slow: DynamicLabel) {
        match self.ram_back.take() {
            Some((t, back)) if t == m => {
                let ram = self.ops.new_dynamic_label();
                dynasm!(self.ops ; .arch x64 ; =>ram);
                self.slow.push(Slow::Recheck { at: back, m, slow, ram });
            }
            pending => {
                self.ram_back = pending;
                self.settle_ram();
                dynasm!(self.ops ; .arch x64 ; cmp Rq(r(m)), RAM_HANDLES ; ja =>slow);
            }
        }
    }

    /// Record an operation (`flags::LAZY_*`) on host register `a` and `b`
    /// for `jit_lazy_flags`, before it changes them.
    fn record_lazy(&mut self, kind: u32, size: u8, a: u8, b: Src) {
        dynasm!(self.ops
            ; .arch x64
            ; mov DWORD [r12 + CTX_LAZY], (kind | (size as u32) << 8) as i32
            ; mov DWORD [r12 + CTX_LAZY + 4], Rd(a)
        );
        match b {
            Src::T(b) => dynasm!(self.ops ; .arch x64 ; mov DWORD [r12 + CTX_LAZY + 8], Rd(r(b))),
            Src::Imm(v) => dynasm!(self.ops ; .arch x64 ; mov DWORD [r12 + CTX_LAZY + 8], v as i32),
        }
    }

    /// EBP = the flags of the recorded operation, from `jit_lazy_flags`,
    /// keeping the exit code in EAX, with `EXIT_FLAGS` set.
    fn lazy_flags(&mut self) {
        dynasm!(self.ops ; .arch x64 ; mov DWORD [r12 + CTX_LAZY_CODE], eax);
        self.save_for_call();
        dynasm!(self.ops
            ; .arch x64
            ; mov rdi, rbx
            ; mov rsi, r12
            ; call QWORD [r12 + CTX_LAZY_FN]
        );
        self.restore_after_call();
        dynasm!(self.ops
            ; .arch x64
            ; mov ebp, eax
            ; mov eax, DWORD [r12 + CTX_LAZY_CODE]
            ; or eax, EXIT_FLAGS as i32
        );
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

    /// The guest's arithmetic flags into EBP, if they aren't there, as
    /// blocks start with them: for a link to another block.
    fn flags_ebp(&mut self) {
        if !self.dirty {
            dynasm!(self.ops ; .arch x64 ; mov ebp, DWORD [rbx + FLAGS]);
            self.dirty = true;
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

    /// Merge the host's flags (in EAX, see `host_flags`) into the guest's
    /// in EBP: the bits in `mask` become those of EAX & `bits`. The others
    /// stay, from the CPU if they aren't in EBP yet and are live.
    fn merge(&mut self, mask: u32, bits: u32) {
        self.merge_from(RAX, mask, bits);
    }

    /// `merge` from host register `reg`.
    fn merge_from(&mut self, reg: u8, mask: u32, bits: u32) {
        dynasm!(self.ops ; .arch x64 ; and Rd(reg), bits as i32);
        if ARITH & !mask & self.live_after == 0 {
            dynasm!(self.ops ; .arch x64 ; mov ebp, Rd(reg));
        } else {
            if !self.dirty {
                dynasm!(self.ops ; .arch x64 ; mov ebp, DWORD [rbx + FLAGS]);
            }
            dynasm!(self.ops
                ; .arch x64
                ; and ebp, !mask as i32
                ; or ebp, Rd(reg)
            );
        }
        self.dirty = true;
    }

    /// EAX = the host's SF, ZF, AF, PF and CF, where the guest's go (LAHF;
    /// with PUSHF, which takes several times as long, OF too, where the
    /// host has no LAHF in 64-bit mode). Callers work out OF themselves.
    fn host_flags(&mut self) {
        if has_lahf() {
            dynasm!(self.ops ; .arch x64 ; lahf ; movzx eax, ah);
        } else {
            dynasm!(self.ops ; .arch x64 ; pushfq ; pop rax);
        }
    }

    /// `reg` = CF and OF, both set where the host's OF is (a product that
    /// doesn't fit, where they are the same).
    fn host_carry_overflow(&mut self, reg: u8) {
        dynasm!(self.ops
            ; .arch x64
            ; seto Rb(reg)
            ; movzx Rd(reg), Rb(reg)
            ; neg Rd(reg)
        );
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
        if self.record_now {
            let kind = Uop::Alu { op, size, a: T0, b }.lazy_kind().expect("a recorded operation");
            self.record_lazy(kind, size, a, b);
        }
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
        if self.record_now {
            self.record_lazy(super::flags::LAZY_NEG, size, t, Src::Imm(0));
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
        self.get_into(RCX, count);
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
        if matches!(op, ShiftOp::Rcl | ShiftOp::Rcr) {
            // (By 1, see `translate::rotate_carry`.)
            self.carry_in();
        }
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
            ShiftOp::Rcl => op!(rcl),
            ShiftOp::Rcr => op!(rcr),
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
                    Some(c) if c > 1 => {
                        dynasm!(self.ops ; .arch x64 ; and eax, (CF | SZP) as i32);
                    }
                    _ => dynasm!(self.ops
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
            ShiftOp::Rcl => {
                // OF = the result's top bit ^ CF.
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
                // ROR and RCR: OF = the result's top two bits differ.
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
        self.get_into(RAX, Gpr { index: 0, high: false, size });
        match (signed, size) {
            (false, 1) => dynasm!(self.ops ; .arch x64 ; mul Rb(t)),
            (false, 2) => dynasm!(self.ops ; .arch x64 ; mul Rw(t)),
            (false, _) => dynasm!(self.ops ; .arch x64 ; mul Rd(t)),
            (true, 1) => dynasm!(self.ops ; .arch x64 ; imul Rb(t)),
            (true, 2) => dynasm!(self.ops ; .arch x64 ; imul Rw(t)),
            (true, _) => dynasm!(self.ops ; .arch x64 ; imul Rd(t)),
        }
        if self.wanted(CF | OF) {
            // Into EBP before the product goes into the registers, which
            // changes the host's flags.
            self.host_carry_overflow(RCX);
            self.merge_from(RCX, CF | OF, CF | OF);
        }
        match size {
            1 => self.set_from(Gpr::word(0), RAX, RCX),
            2 => {
                self.set_from(Gpr::word(0), RAX, RCX);
                self.set_from(Gpr::word(2), RDX, RCX);
            }
            _ => {
                self.set_from(Gpr::dword(0), RAX, RCX);
                self.set_from(Gpr::dword(2), RDX, RCX);
            }
        }
    }


    /// DIV or IDIV of AX, DX:AX or EDX:EAX by t, or #DE first where the
    /// quotient doesn't fit (the host's division would fault too, so it
    /// only runs where it can't). The flags are a 486's (`division_flags`).
    fn div_wide(&mut self, signed: bool, size: u8, t: T) {
        self.divide(signed, size, t);
        if self.wanted(ARITH) {
            self.division_flags(size, t);
        }
    }

    /// The flags DIV and IDIV leave (instructions/arith.rs
    /// `division_flags`), from the quotient and remainder they left: ZF
    /// where the remainder is 0 and the quotient odd, CF where the
    /// remainder's low two bits are 1 or 2, PF where the two have the same
    /// parity, and AF, SF and OF clear. The divisor's t is changed.
    fn division_flags(&mut self, size: u8, t: T) {
        let t = r(t);
        let (quotient, remainder) = match size {
            1 => (Gpr { index: 0, high: false, size: 1 }, Gpr { index: 0, high: true, size: 1 }),
            2 => (Gpr::word(0), Gpr::word(2)),
            _ => (Gpr::dword(0), Gpr::dword(2)),
        };
        self.get_into(RCX, quotient);
        self.get_into(RDX, remainder);
        dynasm!(self.ops
            ; .arch x64
            ; xor eax, eax
            ; test edx, edx
            ; setz al
            ; and eax, ecx
            ; and eax, 1
            ; shl eax, 6
            // CF: (remainder & 3) - 1 below 2.
            ; mov Rd(t), edx
            ; and Rd(t), 3
            ; dec Rd(t)
            ; cmp Rd(t), 2
            ; adc eax, 0
            // PF: the parity of remainder ^ quotient, folded to a byte.
            ; xor edx, ecx
        );
        if size == 4 {
            dynasm!(self.ops ; .arch x64 ; mov Rd(t), edx ; shr Rd(t), 16 ; xor edx, Rd(t));
        }
        if size >= 2 {
            dynasm!(self.ops ; .arch x64 ; mov Rd(t), edx ; shr Rd(t), 8 ; xor edx, Rd(t));
        }
        dynasm!(self.ops
            ; .arch x64
            ; test edx, 0xFF
            ; setp dl
            ; movzx edx, dl
            ; shl edx, 2
            ; or eax, edx
        );
        self.merge(ARITH, ARITH);
    }


    fn divide(&mut self, signed: bool, size: u8, t: T) {
        let t = r(t);
        let de = self.fault_exit(EXIT_DE);
        match (signed, size) {
            // Unsigned, the quotient fits if the dividend's upper half is
            // below the divisor, which a divisor of 0 never is.
            (false, 1) => {
                self.get_into(RAX, Gpr::word(0));
                dynasm!(self.ops
                    ; .arch x64
                    ; movzx ecx, ah
                    ; cmp ecx, Rd(t)
                    ; jae =>de
                    ; div Rb(t)
                );
                self.set_from(Gpr::word(0), RAX, RCX);
            }
            (false, 2) => {
                self.get_into(RAX, Gpr::word(0));
                self.get_into(RDX, Gpr::word(2));
                dynasm!(self.ops
                    ; .arch x64
                    ; cmp edx, Rd(t)
                    ; jae =>de
                    ; div Rw(t)
                );
                self.set_from(Gpr::word(0), RAX, RCX);
                self.set_from(Gpr::word(2), RDX, RCX);
            }
            (false, _) => {
                self.get_into(RAX, Gpr::dword(0));
                self.get_into(RDX, Gpr::dword(2));
                dynasm!(self.ops
                    ; .arch x64
                    ; cmp edx, Rd(t)
                    ; jae =>de
                    ; div Rd(t)
                );
                self.set_from(Gpr::dword(0), RAX, RCX);
                self.set_from(Gpr::dword(2), RDX, RCX);
            }
            // Signed, the division is twice as wide as the guest's, where
            // no quotient overflows, and then the quotient must fit.
            (true, 1) => {
                dynasm!(self.ops
                    ; .arch x64
                    ; movsx ecx, Rb(t)
                    ; test ecx, ecx
                    ; jz =>de
                );
                self.get_into(RAX, Gpr::word(0));
                dynasm!(self.ops
                    ; .arch x64
                    ; movsx eax, ax
                    ; cdq
                    ; idiv ecx
                    ; movsx ecx, al
                    ; cmp ecx, eax
                    ; jne =>de
                    ; mov ah, dl
                );
                self.set_from(Gpr::word(0), RAX, RCX);
            }
            (true, 2) => {
                dynasm!(self.ops
                    ; .arch x64
                    ; movsx rcx, Rw(t)
                    ; test ecx, ecx
                    ; jz =>de
                );
                self.get_into(RAX, Gpr::word(0));
                self.get_into(RDX, Gpr::word(2));
                dynasm!(self.ops
                    ; .arch x64
                    ; shl edx, 16
                    ; or eax, edx
                    ; movsxd rax, eax
                    ; cqo
                    ; idiv rcx
                    ; movsx rcx, ax
                    ; cmp rcx, rax
                    ; jne =>de
                );
                self.set_from(Gpr::word(0), RAX, RCX);
                self.set_from(Gpr::word(2), RDX, RCX);
            }
            (true, _) => {
                dynasm!(self.ops
                    ; .arch x64
                    ; movsxd rcx, Rd(t)
                    ; test rcx, rcx
                    ; jz =>de
                );
                self.get_into(RAX, Gpr::dword(0));
                self.get_into(RDX, Gpr::dword(2));
                dynasm!(self.ops
                    ; .arch x64
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
                    ; movsxd rcx, eax
                    ; cmp rcx, rax
                    ; jne =>de
                );
                self.set_from(Gpr::dword(0), RAX, RCX);
                self.set_from(Gpr::dword(2), RDX, RCX);
            }
        }
    }


    fn exit_if(&mut self, cond: Cond, taken: u32, next: u32, commit: Option<(Gpr, T)>) {
        let yes = self.ops.new_dynamic_label();
        // The condition reads the flags where they are.
        let ebp = self.dirty;
        // A jump the block goes on after leaves where taken, after the
        // block's code.
        let side = self.ix + 1 < self.data.count();
        if let Some(to) = self.data.target(self.ix) {
            self.merges.push(Merge {
                at: yes,
                ix: self.ix,
                to,
                commit,
                cache: self.cache,
                dirty: self.dirty,
                synced: self.synced[self.ix],
                fpu_cr0_checked: self.fpu_cr0_checked,
                fpu_known: self.fpu_known,
                entry: None,
            });
        } else if side {
            self.sides.push(Side {
                at: yes,
                ix: self.ix,
                taken,
                commit,
                cache: self.cache,
                dirty: self.dirty,
                loaded_segs: self.loaded_segs,
            });
        }
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
        if side {
            self.commit(commit);
            return;
        }
        let (cache, dirty) = (self.cache, self.dirty);
        self.commit(commit);
        self.leave(Some(next), 1, true);
        (self.cache, self.dirty) = (cache, dirty);
        dynasm!(self.ops ; .arch x64 ; =>yes);
        self.taken(taken, commit, 0);
    }

    /// Where instruction `ix` starts, which jumps in the block may go to:
    /// note how the code has the counts (`synced`), the cached registers
    /// and the flags there for them, and know only what holds both ways.
    fn merge_here(&mut self, synced: i32) {
        let ix = self.ix;
        if !self.merges.iter().any(|m| m.to == ix) {
            return;
        }
        let entry = self.ops.new_dynamic_label();
        dynasm!(self.ops ; .arch x64 ; =>entry);
        for m in self.merges.iter_mut().filter(|m| m.to == ix) {
            m.entry = Some((entry, synced, self.cache, self.dirty));
            self.fpu_cr0_checked &= m.fpu_cr0_checked;
            self.fpu_known &= m.fpu_known;
        }
    }

    /// A taken jump to an instruction later in the block (see `Merge`).
    fn jump_in(&mut self, m: Merge) {
        let (entry, synced, fall, fall_dirty) = m.entry.expect("jump target translated");
        dynasm!(self.ops ; .arch x64 ; =>m.at);
        (self.ix, self.cache, self.dirty) = (m.ix, m.cache, m.dirty);
        self.commit(m.commit);
        let skipped = (m.to - m.ix - 1) as i32;
        let behind = synced - m.synced - skipped;
        if behind != 0 {
            dynasm!(self.ops ; .arch x64 ; add QWORD [rbx + ICOUNT], behind);
        }
        if skipped != 0 {
            dynasm!(self.ops ; .arch x64 ; sub QWORD [rbx + EXECUTED], skipped);
        }
        self.writeback(self.cache.dirty & !fall.dirty);
        self.load_cached(fall.loaded & !self.cache.loaded);
        match (self.dirty, fall_dirty) {
            (true, false) => self.flags_back(),
            (false, true) => dynasm!(self.ops ; .arch x64 ; mov ebp, DWORD [rbx + FLAGS]),
            _ => {}
        }
        dynasm!(self.ops ; .arch x64 ; jmp =>entry);
    }

    /// Leave for the target `taken` of a conditional jump through link
    /// `slot`, after `commit`: the target must be within the CS limit.
    fn taken(&mut self, taken: u32, commit: Option<(Gpr, T)>, slot: usize) {
        let gp = self.fault_exit(EXIT_GP0);
        dynasm!(self.ops
            ; .arch x64
            ; mov eax, taken as i32
            ; cmp eax, DWORD [rbx + seg_field(Seg::CS, layout::SEG_LIMIT)]
            ; ja =>gp
        );
        self.commit(commit);
        self.leave(Some(taken), slot, true);
    }

    /// Leave the block after its last instruction, at `eip` if the code
    /// sets it (`set`) or knows it: through link `slot` if that is in the
    /// page, else back to the execution loop. The counts first.
    fn leave(&mut self, eip: Option<u32>, slot: usize, set: bool) {
        if let (Some(eip), true) = (eip, set) {
            dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + EIP], eip as i32);
        }
        self.writeback(self.cache.dirty);
        let linked = self.link && eip.is_some();
        if linked {
            self.flags_ebp();
        } else {
            self.flags_back();
        }
        self.counts();
        if let (true, Some(eip)) = (linked, eip) {
            dynasm!(self.ops ; .arch x64 ; mov Rd(r(T0)), eip as i32);
            self.check_flat(T0);
        }
        match eip {
            Some(eip) if self.link && self.data.in_page(eip) => self.link_jump(slot),
            Some(_) if self.link => self.guarded(slot),
            _ => dynasm!(self.ops ; .arch x64 ; mov eax, EXIT_NEXT as i32 ; jmp QWORD [r12 + CTX_EXIT]),
        }
    }

    /// After a segment load in the block, where the segments aren't flat
    /// as the block's environment has them, don't take the link to EIP
    /// `t`, whose blocks were translated for it, but those a return takes
    /// in the mode the segments are in now (`returned` after a far
    /// transfer). RDX is the block.
    fn check_flat(&mut self, t: T) {
        if self.loaded_segs == 0 {
            return;
        }
        let same = self.ops.new_dynamic_label();
        dynasm!(self.ops
            ; .arch x64
            ; mov eax, DWORD [r12 + CTX_FLAT]
            ; cmp eax, (self.env.bits & super::ENV_FLAT_ALL) as i32
            ; je =>same
            ; mov ecx, DWORD [r12 + CTX_MODE]
            ; and ecx, !super::ENV_FLAT_ALL as i32
            ; or ecx, eax
            ; mov DWORD [r12 + CTX_MODE], ecx
        );
        self.returned(t, true);
        dynasm!(self.ops ; .arch x64 ; =>same);
    }

    /// Bring the counts up to date for leaving the block after the
    /// instruction being translated (the last but at a conditional jump the
    /// block goes on after), and RDX = the block.
    fn counts(&mut self) {
        let (n, synced) = (self.ix as i32 + 1, self.synced[self.ix]);
        let data_ptr = self.data_ptr;
        dynasm!(self.ops
            ; .arch x64
            ; add QWORD [rbx + ICOUNT], n - synced
            ; add QWORD [rbx + EXECUTED], n
            ; mov rdx, QWORD data_ptr
        );
    }

    /// After a far transfer its handler ran (`block::far_transfer`), where
    /// it went: through the links a return takes. (`jit_fallback` stopped
    /// the block where the code can't go on.)
    fn far_returned(&mut self) {
        dynasm!(self.ops ; .arch x64 ; mov Rd(r(T0)), DWORD [rbx + EIP]);
        self.flags_ebp();
        self.counts();
        self.returned(T0, true);
    }

    /// Leave through the return (or indirect call) link to EIP `t`, if
    /// there is one (see `guarded`), else to the execution loop, to be
    /// linked. RDX is the block. After a far transfer (`far`) the blocks
    /// may be another mode's, which the context has (`far_goes_on`).
    fn returned(&mut self, t: T, far: bool) {
        let miss = *self.return_miss.get_or_insert_with(|| self.ops.new_dynamic_label());
        for slot in RETURN_LINK..LINKS {
            let next = self.ops.new_dynamic_label();
            let g = DATA_GUARDS + slot as i32 * GUARD_SIZE;
            dynasm!(self.ops ; .arch x64 ; cmp Rd(r(t)), DWORD [rdx + g + GUARD_EIP] ; jne =>next);
            if far {
                dynasm!(self.ops
                    ; .arch x64
                    ; mov ecx, DWORD [r12 + CTX_MODE]
                    ; cmp ecx, DWORD [rdx + g + GUARD_MODE]
                    ; jne =>next
                );
            }
            self.guarded(slot);
            dynasm!(self.ops ; .arch x64 ; =>next);
        }
        // The engine's place for the EIP (see `Return`), made in this
        // block's mode, with its guard checked as a link's.
        let g = RETURN_GUARD;
        dynasm!(self.ops
            ; .arch x64
            ; mov ecx, Rd(r(t))
            ; and ecx, (1 << super::block::RETURN_BITS) - 1
            ; lea ecx, [rcx + rcx * 4]
            ; mov rax, QWORD [r12 + CTX_RETURNS]
            ; lea rcx, [rax + rcx * 8]
            ; cmp Rd(r(t)), DWORD [rcx + g + GUARD_EIP]
            ; jne =>miss
        );
        if far {
            dynasm!(self.ops
                ; .arch x64
                ; mov eax, DWORD [r12 + CTX_MODE]
                ; cmp DWORD [rcx + RETURN_MODE], eax
                ; jne =>miss
            );
        } else {
            dynasm!(self.ops ; .arch x64 ; cmp DWORD [rcx + RETURN_MODE], self.env.bits as i32 ; jne =>miss);
        }
        dynasm!(self.ops
            ; .arch x64
            ; mov eax, DWORD [rbx + seg_field(Seg::CS, layout::SEG_BASE)]
            ; cmp eax, DWORD [rcx + g + GUARD_CS_BASE]
            ; jne =>miss
        );
        if self.env.bits & super::ENV_PAGING != 0 {
            // (RDX is the block again on the way out.)
            let set = if self.env.bits & super::ENV_USER != 0 { layout::TLB_SET as i32 } else { 0 };
            let entry = set * TLB_ENTRY;
            let data_ptr = self.data_ptr;
            dynasm!(self.ops
                ; .arch x64
                ; mov edx, DWORD [rcx + g + GUARD_PAGE]
                ; mov eax, edx
                ; and edx, (layout::TLB_SET - 1) as i32
                ; shl edx, TLB_ENTRY_SHIFT
                ; inc eax
                ; cmp eax, DWORD [rbx + rdx + TLB + entry + layout::TLB_READ_TAG as i32]
                ; jne >not
                ; mov eax, DWORD [rbx + rdx + TLB + entry + layout::TLB_PHYS as i32]
                ; cmp eax, DWORD [rcx + g + GUARD_PHYS]
                ; jne >not
                ; jmp QWORD [rcx + RETURN_CODE]
                ; not:
                ; mov rdx, QWORD data_ptr
                ; jmp =>miss
            );
        } else {
            dynasm!(self.ops ; .arch x64 ; jmp QWORD [rcx + RETURN_CODE]);
        }
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
        );
        // The A20 gate and paging are as the link was made: the block runs
        // only in the environment it was translated for, and its links were
        // made in it.
        if self.env.bits & super::ENV_PAGING != 0 {
            // The TLB entry of the page in the set of the privilege level.
            let set = if self.env.bits & super::ENV_USER != 0 { layout::TLB_SET as i32 } else { 0 };
            let entry = set * TLB_ENTRY;
            let (fetch, back) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
            self.slow.push(Slow::Fetch { at: fetch, back, stub, slot, g });
            dynasm!(self.ops
                ; .arch x64
                ; =>back
                ; mov ecx, DWORD [rdx + g + GUARD_PAGE]
                ; mov eax, ecx
                ; and ecx, (layout::TLB_SET - 1) as i32
                ; shl ecx, TLB_ENTRY_SHIFT
                ; inc eax
                ; cmp eax, DWORD [rbx + rcx + TLB + entry + layout::TLB_READ_TAG as i32]
                ; jne =>fetch
                ; mov eax, DWORD [rbx + rcx + TLB + entry + layout::TLB_PHYS as i32]
                ; cmp eax, DWORD [rdx + g + GUARD_PHYS]
                ; jne =>stub
            );
        }
        self.link_jump(slot);
    }

    /// The jump of link `slot`: to its stub, until the engine makes it a
    /// link (`patch_link`). A return's links are made again for each place
    /// it goes to in turn: they jump where the block's `links` says
    /// (RDX is the block), as rewriting code that just ran is slow.
    fn link_jump(&mut self, slot: usize) {
        let stub = *self.stubs[slot].get_or_insert_with(|| self.ops.new_dynamic_label());
        if slot >= RETURN_LINK {
            dynasm!(self.ops ; .arch x64 ; jmp QWORD [rdx + DATA_LINKS + slot as i32 * 8]);
            return;
        }
        dynasm!(self.ops ; .arch x64 ; jmp =>stub);
        self.sites.push((slot as u8, self.ops.offset().0 as u32 - 4));
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
