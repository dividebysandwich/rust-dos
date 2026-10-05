//! The AArch64 code generator.
//!
//! Registers while translated code runs: X19 the CPU, X20 the context
//! (`JitCtx`), X21 RAM, X22 RAM's code generations, W23 set when a store hit
//! the block's later bytes, X27 the CPU plus `HI` and X29 the CPU plus
//! `REGS` (the CPU's fields past X19's offsets' reach), W28 the guest's arithmetic flags where the code
//! has changed them (see `Gen::dirty`); W24-W26 hold the operations'
//! temporaries (`uop::T`), which calls keep, D8 and D9 the FPU
//! instructions' doubles (`uop::X`), which calls keep too, and X0-X17 and
//! D16-D17 are scratch (X18, the platform's register, is never touched). The guest's registers stay
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

use super::block::{BlockData, LINKS, RETURN_BITS, RETURN_LINK, RETURN_MISS, SIDE_LINK};
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

/// The bits of a memory operand's handle that aren't those of an address in
/// RAM (see `x64::RAM_HANDLES`).
const NOT_RAM: u64 = 0xFFFF_FFFF_8000_0000;
const _: () = assert!(crate::config::MAX_MEMSIZE << 20 <= 0x8000_0000 && SLOW as u64 & NOT_RAM != 0);

/// Where the RAM below the video memory ends, and extended memory starts
/// (both multiples of 4 KB, so they fit a compare's shifted immediate).
const VIDEO: u32 = 0xA0000;
const EXTENDED: u32 = 0x10_0000;

/// The lowest of `fields`, rounded down to 16 bytes.
const fn lowest(fields: &[usize]) -> usize {
    let mut min = usize::MAX;
    let mut i = 0;
    while i < fields.len() {
        if fields[i] < min {
            min = fields[i];
        }
        i += 1;
    }
    min & !0xF
}

/// The CPU's fields X27 reaches: those of the bus from `HI` on.
const HI: usize = lowest(&[layout::ICOUNT, layout::DEADLINE, layout::A20_MASK]);

/// And those X29 reaches, from `REGS` on: the CPU's own registers (the bus
/// is between them and `HI`, too far for one).
const REGS: usize = lowest(&[
    layout::GPR,
    layout::SEG,
    layout::EXECUTED,
    layout::EIP,
    layout::FLAGS,
    layout::CR0,
    layout::CPL,
    layout::fpu::TOP,
    layout::fpu::TAGS,
    layout::fpu::F64,
    layout::fpu::X80,
    layout::fpu::STALE,
]);

const CPU: u8 = 19;
const HIB: u8 = 27;
const REGSB: u8 = 29;
const _: () = assert!(layout::fpu::X80 + 128 - REGS < 4096 && layout::fpu::TAGS - REGS < 4096);

/// The way in from Rust: `enter(cpu, ctx, code)` saves the registers Rust
/// expects kept, sets up the fixed ones and branches to `code`; translated
/// code leaves through `exit` with the exit code in W0 and its block in
/// X1, which it stores in the context with W28 (the flags, for
/// `EXIT_FLAGS`) before returning the code.
pub struct Trampoline {
    pub bytes: Vec<u8>,
    pub enter: usize,
    pub exit: usize,
    /// `fpu_addsub`.
    pub addsub: usize,
}

pub type Enter = unsafe extern "C" fn(*mut crate::cpu::Cpu, *mut JitCtx, *const u8) -> u64;

pub fn trampoline() -> Trampoline {
    let mut ops = Asm::new(0);
    let enter = ops.offset().0;
    dynasm!(ops
        ; .arch aarch64
        ; stp x29, x30, [sp, -112]!
        ; mov x29, sp
        ; stp x19, x20, [sp, 16]
        ; stp x21, x22, [sp, 32]
        ; stp x23, x24, [sp, 48]
        ; stp x25, x26, [sp, 64]
        ; stp x27, x28, [sp, 80]
        ; stp d8, d9, [sp, 96]
        ; mov x19, x0
        ; mov x20, x1
        ; ldr x21, [x20, CTX_RAM as u32]
        ; ldr x22, [x20, CTX_PAGE_GEN as u32]
    );
    mov32(&mut ops, 9, HI as u32);
    dynasm!(ops ; .arch aarch64 ; add x27, x19, x9);
    mov32(&mut ops, 9, REGS as u32);
    dynasm!(ops
        ; .arch aarch64
        ; add x29, x19, x9
        ; br x2
    );
    let exit = ops.offset().0;
    dynasm!(ops
        ; .arch aarch64
        ; str x1, [x20, CTX_EXIT_DATA as u32]
        ; str w28, [x20, CTX_FLAGS as u32]
        ; ldp d8, d9, [sp, 96]
        ; ldp x27, x28, [sp, 80]
        ; ldp x25, x26, [sp, 64]
        ; ldp x23, x24, [sp, 48]
        ; ldp x21, x22, [sp, 32]
        ; ldp x19, x20, [sp, 16]
        ; ldp x29, x30, [sp], 112
        ; ret
    );
    let addsub = ops.offset().0;
    fpu_addsub(&mut ops);
    Trampoline { bytes: ops.finalize().expect("trampoline"), enter, exit, addsub }
}

/// ST(dst) = a + b, or a - b with W3 set, as `F80::add` and `F80::sub`
/// add their 80 bits, where x86-64's `Gen::fpu_addsub` does; a routine of
/// the trampoline's that translated code calls with the physical registers
/// a, b and dst in W0-W2 (8 for the double in D16), which returns W0 = 0,
/// or 1 with nothing changed for the handler's code. X0-X15 are changed.
fn fpu_addsub(ops: &mut Asm) {
    let slow = ops.new_dynamic_label();
    dynasm!(ops ; .arch aarch64 ; mov w8, w0 ; mov w9, w1 ; mov w15, w2 ; mov w17, w3);
    for (reg, off) in [(11, layout::fpu::X80), (12, layout::fpu::F64), (13, layout::fpu::STALE), (14, layout::fpu::TAGS)] {
        dynasm!(ops ; .arch aarch64 ; add XSP(reg), x29, (off - REGS) as u32);
    }
    // Operand into (sign << 15 | exponent) and the 64-bit mantissa.
    f80_operand(ops, 8, 0, 2, slow);
    f80_operand(ops, 9, 1, 3, slow);
    dynasm!(ops
        ; .arch aarch64
        ; cbz w17, >signed
        ; eor w1, w1, 0x8000
        ; signed:
        // Exponents aligned: the smaller operand's mantissa shifted
        // right (to 0 by 64 or more); W10 the larger exponent.
        ; and w4, w0, 0x7FFF
        ; and w10, w1, 0x7FFF
        ; subs w5, w4, w10
        ; csel w10, w4, w10, hs
        ; cneg w5, w5, lo
        ; csel w6, w5, wzr, lo
        ; csel w7, w5, wzr, hs
        ; lsr x2, x2, x6
        ; cmp w6, 64
        ; csel x2, xzr, x2, hs
        ; lsr x3, x3, x7
        ; cmp w7, 64
        ; csel x3, xzr, x3, hs
        // Same signs: the sum, a carry shifted in from the top.
        ; eor w5, w0, w1
        ; tbnz w5, 15, >differ
        ; adds x2, x2, x3
        ; lsr x6, x2, 1
        ; orr x6, x6, 1 << 63
        ; csel x2, x6, x2, hs
        ; cinc w10, w10, hs
        ; and w0, w0, 0x8000
        ; b >result
        // Different signs: the difference, normalized, with the larger
        // one's sign; 0 for none.
        ; differ:
        ; subs x5, x2, x3
        ; sub x6, x3, x2
        ; csel x2, x5, x6, hs
        ; csel w0, w0, w1, hs
        ; cbz x2, >nothing
        ; clz x5, x2
        ; lsl x2, x2, x5
        ; sub w10, w10, w5
        ; and w0, w0, 0x8000
        ; b >result
        ; nothing:
        ; mov w0, wzr
        ; mov w10, wzr
        // The double `F80::get_f64` makes of it into X7: 0 for exponent
        // 0; outside a double's range (or an exponent that wrapped) the
        // handlers'.
        ; result:
        ; cbz w10, >zero
        ; sub w5, w10, (16383 - 1023) >> 12, lsl 12
        ; sub w5, w5, (16383 - 1023) & 0xFFF
        ; sub w6, w5, 1
        ; cmp w6, 0x7FD
        ; b.hi =>slow
        ; ubfx x7, x2, 11, 52
        ; orr x7, x7, x5, lsl 52
        ; orr x7, x7, x0, lsl 48
        ; b >store
        ; zero:
        ; lsl x7, x0, 48
        // ST(dst): the 80 bits, their double, and not stale.
        ; store:
        ; orr w0, w0, w10
        ; add x6, x11, x15, lsl 4
        ; str x2, [x6]
        ; str x0, [x6, 8]
        ; str x7, [x12, x15, lsl 3]
        ; strb wzr, [x13, x15]
        ; mov w0, wzr
        ; ret
        ; =>slow
        ; movz w0, 1
        ; ret
    );
}

/// Operand W`idx` of `fpu_addsub` (a physical register, or 8 for D16) into
/// W`hi` (sign << 15 | exponent) and X`man`, as `FpuRegs::get` and
/// `F80::set_f64` make its 80 bits; to `slow` for an empty register or a
/// double that is an infinity or a NaN. X6 and X7 are changed.
fn f80_operand(ops: &mut Asm, idx: u8, hi: u8, man: u8, slow: DynamicLabel) {
    let (value, stale, loaded, zero) =
        (ops.new_dynamic_label(), ops.new_dynamic_label(), ops.new_dynamic_label(), ops.new_dynamic_label());
    dynasm!(ops
        ; .arch aarch64
        ; cmp WSP(idx), 8
        ; b.eq =>value
        ; ldrb w6, [x14, X(idx)]
        ; cmp w6, FPU_EMPTY
        ; b.eq =>slow
        ; ldrb w6, [x13, X(idx)]
        ; cbnz w6, =>stale
        ; add x6, x11, X(idx), lsl 4
        ; ldr X(man), [x6]
        ; ldrh W(hi), [x6, 8]
        ; b =>loaded
        ; =>stale
        ; ldr x7, [x12, X(idx), lsl 3]
        ; b >double
        ; =>value
        ; fmov x7, d16
        // The double's bits in X7.
        ; double:
        ; ubfx X(hi), x7, 52, 11
        ; cbz W(hi), =>zero
        ; cmp WSP(hi), 0x7FF
        ; b.eq =>slow
        ; add WSP(hi), WSP(hi), (16383 - 1023) >> 12, lsl 12
        ; add WSP(hi), WSP(hi), (16383 - 1023) & 0xFFF
        ; lsr x6, x7, 63
        ; orr W(hi), W(hi), w6, lsl 15
        ; lsl X(man), x7, 11
        ; orr XSP(man), X(man), 1 << 63
        ; b =>loaded
        ; =>zero
        ; lsr X(hi), x7, 63
        ; lsl W(hi), W(hi), 15
        ; mov X(man), xzr
        ; =>loaded
    );
}

/// W`reg` = `v`.
fn mov32(ops: &mut Asm, reg: u8, v: u32) {
    dynasm!(ops ; .arch aarch64 ; movz W(reg), v & 0xFFFF);
    if v >> 16 != 0 {
        dynasm!(ops ; .arch aarch64 ; movk W(reg), v >> 16, lsl 16);
    }
}

/// The system register NZCV's encoding, for MRS.
const NZCV: u32 = 0x5A10;

/// FPU register tags.
const FPU_EMPTY: u32 = crate::cpu::FPU_TAG_EMPTY as u32;
const FPU_VALID: u32 = crate::cpu::FPU_TAG_VALID as u32;

/// Host register (D8 on) of an FPU instruction's double.
fn d(x: X) -> u8 {
    8 + x.0
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
    Str8,
    Str16,
    Str32,
    Str64,
}

/// Code emitted after the block's instructions, reached from them.
enum Slow {
    /// A memory operand the inline checks didn't take.
    MemRef { at: DynamicLabel, back: DynamicLabel, t: T, desc: u32, fail: DynamicLabel },
    Load { at: DynamicLabel, back: DynamicLabel, dst: T, m: T, size: u8 },
    /// Where a memory reference's slow paths come back to before a load or
    /// store that takes its handle as RAM's: to `slow` with one that isn't,
    /// else to the RAM access at `ram`.
    Recheck { at: DynamicLabel, m: T, slow: DynamicLabel, ram: DynamicLabel },
    Store { at: DynamicLabel, back: DynamicLabel, m: T, src: T, size: u8, lo: u32, hi: u32 },
    /// A store into a block of RAM with code: the code generations bumped
    /// and a store into the block's later bytes (`lo..hi`) noted.
    CodeStore { at: DynamicLabel, back: DynamicLabel, m: T, src: T, size: u8, lo: u32, hi: u32 },
    /// An instruction's fault with an exit code of its own (#GP(0), #DE).
    Fault { at: DynamicLabel, code: u32, fail: DynamicLabel },
    /// The instruction is done, and the block stops after it, at `next`,
    /// with exit `code` (STI letting an interrupt through).
    After { at: DynamicLabel, next: u32, code: u32, fail: DynamicLabel },
    /// Instruction `ix` runs through its handler after all (`Uop::Bail`),
    /// with the flags in W28 there (`dirty`), and goes on at `end`.
    /// With `leave`, the block stops after it (`Uop::FpuGuard`).
    Bail { at: DynamicLabel, end: DynamicLabel, ix: usize, dirty: bool, leave: bool },
    /// A store hit the rest of the block: it leaves after the instruction,
    /// at `next`, with the flags in W28 (`dirty`) or a recorded
    /// operation's (`lazy`), through `join`.
    Smc { at: DynamicLabel, next: u32, dirty: bool, lazy: bool, join: DynamicLabel },
    /// Link `slot`'s guard (at `g` in the block) didn't find its page in the
    /// TLB: if the link is made, `jit_fetch` looks the page up and the
    /// guard checks again from `back`, else the link's `stub`. X1 is the
    /// block.
    Fetch { at: DynamicLabel, back: DynamicLabel, stub: DynamicLabel, slot: usize, g: u32 },
    /// A memory operand of `size` bytes at physical address `addr` (a
    /// register) that isn't plain RAM: within a page, its handle is that
    /// address with `DEV_BIT`; else it goes on to `slow`, the `MemRef`.
    Dev { at: DynamicLabel, back: DynamicLabel, slow: DynamicLabel, t: T, addr: u8, size: u8 },
}

/// A conditional jump the block goes on after (`block::SIDE_EXITS`): its
/// way out where it is taken, at `at`, made after the block's code from
/// what was known at the jump: instruction `ix`, the commit of its
/// counter, where the flags are and the segments loaded.
struct Side {
    at: DynamicLabel,
    ix: usize,
    taken: u32,
    commit: Option<(Gpr, T)>,
    dirty: bool,
    loaded_segs: u8,
}

/// A conditional jump to instruction `to` later in the block
/// (`BlockData::target`), where taken: at `at`, made after the block's
/// code, the counts are brought to where they are at `to` but for the
/// instructions it skips, and the flags to where the code at `to` has
/// them; then on at `entry`. With what was known at the jump, as for a
/// `Side`.
struct Merge {
    at: DynamicLabel,
    ix: usize,
    to: usize,
    commit: Option<(Gpr, T)>,
    dirty: bool,
    synced: i32,
    fpu_cr0_checked: bool,
    fpu_known: u8,
    /// Set where `to` is translated: where its code starts, the counts
    /// and where the flags are.
    entry: Option<(DynamicLabel, i32, bool)>,
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
    /// After a memory reference: its handle, and where its slow paths come
    /// back to, with a handle that may not be RAM's (see `x64::Gen`).
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
    /// The instruction being translated.
    ix: usize,
    /// The flags live after each operation, and after the one being
    /// translated; whether the guest's arithmetic flags are in W28, and
    /// were where each instruction may stop (see `x64::Gen`).
    live: Vec<Vec<u32>>,
    live_after: u32,
    /// As `x64::Gen` has them: the operations that record their operands
    /// for the flags only a fault or a store into the block needs, and
    /// where the flags are a recorded operation's.
    record: Vec<Vec<bool>>,
    record_now: bool,
    lazy_start: Vec<bool>,
    lazy_end: Vec<bool>,
    join: Vec<Option<DynamicLabel>>,
    dirty: bool,
    dirty_at: Vec<bool>,
    /// What the code is translated for.
    env: super::Env,
    /// Whether CR0 was checked for FPU instructions, and the registers
    /// (ST(i) bit i) known not to be empty, see `x64::Gen::fpu_known`.
    fpu_cr0_checked: bool,
    fpu_known: u8,
    /// The segment registers (bit `Seg`) instructions in the block loaded
    /// so far: their accesses are checked as if they weren't flat.
    loaded_segs: u8,
    /// The top of the REP loop being translated (`Uop::RepStart`).
    rep_top: Option<DynamicLabel>,
    /// The offsets temporaries hold where the operations before made
    /// them constants (`Uop::Const`, or `Uop::Ea` of a displacement).
    known: [Option<u32>; 8],
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

/// Point the link jump at `site` (`Code::sites`), a B, at `to`: the code
/// memory is within its reach (128 MB).
pub fn patch_link(mem: &mut super::codemem::CodeMemory, site: *const u8, to: *const u8) {
    let rel = (to as isize - site as isize) >> 2;
    debug_assert!((-(1 << 25)..1 << 25).contains(&rel));
    let b = 0x1400_0000u32 | (rel as u32 & 0x03FF_FFFF);
    mem.patch(site, &b.to_le_bytes());
}

/// Translate a block: each instruction's operations (`items[ix]`), or a
/// call of its interpreter handler (None). With `link`, exits to a known
/// EIP in the block's page go through its links. See `x64::block`, which
/// this follows.
/// Whether blocks go on into the last 15 bytes of their page (see
/// `BlockData::in_tail`): not in this code generator's.
pub const TAIL: bool = false;
/// Whether it has the operations of port I/O, STI, RCL/RCR by 1 and
/// string loops.
pub const SYSTEM: bool = true;
/// Whether it has the loads of data segment registers in protected mode
/// (through `jit_load_seg`; in real mode it always has them).
pub const SEGMENTS: bool = true;
/// Whether it has those of FPU instructions.
pub const FPU: bool = true;

pub fn block(data: &BlockData, items: &[Option<Vec<Uop>>], link: bool, env: super::Env) -> Code {
    let mut ops = Asm::new(0);
    let n = data.count();
    let mut labels = || ops.new_dynamic_label();
    let (data_lit, tail, deadline, revalidate, limit, body) = (labels(), labels(), labels(), labels(), labels(), labels());
    let fail_tail = labels();
    let plan = super::flags::plan(items, &data.targets());
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
        ram_back: None,
        sides: Vec::new(),
        merges: Vec::new(),
        link,
        stubs: [None; LINKS],
        sites: Vec::new(),
        return_miss: None,
        ix: 0,
        live: plan.live,
        live_after: 0,
        record: plan.record,
        record_now: false,
        lazy_start: plan.lazy_start,
        lazy_end: plan.lazy_end,
        join: vec![None; n],
        dirty: false,
        dirty_at: vec![false; n],
        env,
        fpu_cr0_checked: false,
        fpu_known: 0,
        loaded_segs: 0,
        rep_top: None,
        known: [None; 8],
    };
    g.prologue(items);
    let mut synced = 0;
    for (ix, item) in items.iter().enumerate() {
        g.ix = ix;
        g.merge_here(synced);
        g.known = [None; 8];
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
                g.fpu_known = 0;
                if let Some(seg) = super::block::loaded_segment(&data.instrs[ix]) {
                    g.loaded_segs |= 1 << seg as u8;
                }
            }
            Some(uops) => {
                let port = uops.iter().any(|u| matches!(u, Uop::In { .. } | Uop::Out { .. }));
                if port && ix as i32 > synced {
                    // Devices read the time from the instruction count.
                    g.add_field64(layout::ICOUNT, (ix as i32 - synced) as u32);
                    synced = ix as i32;
                }
                g.synced[ix] = synced;
                g.dirty_at[ix] = g.dirty;
                g.check_watched();
                // A register or constant a store takes goes before its
                // memory reference, as on x86-64.
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
                    dynasm!(g.ops ; .arch aarch64 ; =>end);
                }
                if port {
                    // The port access asked for the block to stop after it.
                    let next = data.eips[ix].wrapping_add(data.instrs[ix].len() as u32);
                    let fail = g.fail();
                    let flags = if g.dirty { EXIT_FLAGS } else { 0 };
                    let go_on = g.ops.new_dynamic_label();
                    dynasm!(g.ops ; .arch aarch64 ; ldrb w0, [x20, CTX_AFTER as u32] ; cbz w0, =>go_on ; strb wzr, [x20, CTX_AFTER as u32]);
                    g.mov32(1, next);
                    g.field(Access::Str32, 1, layout::EIP);
                    if flags != 0 {
                        g.mov32(1, flags);
                        dynasm!(g.ops ; .arch aarch64 ; orr w0, w0, w1);
                    }
                    dynasm!(g.ops ; .arch aarch64 ; b =>fail ; =>go_on);
                }
                if uops.iter().any(|u| matches!(u, Uop::Store { .. })) {
                    // A store hit the rest of the block: leave after this
                    // instruction.
                    let next = data.eips[ix].wrapping_add(data.instrs[ix].len() as u32);
                    g.fail();
                    let join = *g.join[ix].get_or_insert_with(|| g.ops.new_dynamic_label());
                    let at = g.ops.new_dynamic_label();
                    dynasm!(g.ops ; .arch aarch64 ; cbnz w23, =>at);
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
        Some(u) if !u.iter().any(|u| matches!(u, Uop::Exit { .. } | Uop::ExitIf { .. })) => g.leave(Some(next), 0, true),
        None if !super::block::ends_block(&data.instrs[last]) => g.leave(Some(next), 0, false),
        None if g.link && super::block::far_transfer(&data.instrs[last]) => g.far_returned(),
        _ => {}
    }
    g.link = link;
    // The end: the counts, and back to the execution loop. The code that
    // branches here has put the flags back.
    dynasm!(g.ops ; .arch aarch64 ; =>tail);
    g.dirty = false;
    g.leave(None, 0, false);
    // The conditional jumps the block went on after, where taken.
    for (k, side) in std::mem::take(&mut g.sides).into_iter().enumerate() {
        dynasm!(g.ops ; .arch aarch64 ; =>side.at);
        (g.ix, g.dirty, g.loaded_segs) = (side.ix, side.dirty, side.loaded_segs);
        g.taken(side.taken, side.commit, SIDE_LINK + k);
    }
    for merge in std::mem::take(&mut g.merges) {
        g.jump_in(merge);
    }
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
    let sites = std::mem::take(&mut g.sites);
    Code { bytes: g.ops.finalize().expect("block"), stubs, sites, lag }
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
        } else if off >= REGS && off - REGS < 4096 {
            (REGSB, (off - REGS) as u32)
        } else {
            self.mov32(9, off as u32);
            match access {
                Access::Ldr8 => dynasm!(self.ops ; .arch aarch64 ; ldrb W(reg), [x19, x9]),
                Access::Ldr16 => dynasm!(self.ops ; .arch aarch64 ; ldrh W(reg), [x19, x9]),
                Access::Ldr32 => dynasm!(self.ops ; .arch aarch64 ; ldr W(reg), [x19, x9]),
                Access::Ldr64 => dynasm!(self.ops ; .arch aarch64 ; ldr X(reg), [x19, x9]),
                Access::Str8 => dynasm!(self.ops ; .arch aarch64 ; strb W(reg), [x19, x9]),
                Access::Str16 => dynasm!(self.ops ; .arch aarch64 ; strh W(reg), [x19, x9]),
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
            Access::Str8 => dynasm!(self.ops ; .arch aarch64 ; strb W(reg), [X(base), rel]),
            Access::Str16 => dynasm!(self.ops ; .arch aarch64 ; strh W(reg), [X(base), rel]),
            Access::Str32 => dynasm!(self.ops ; .arch aarch64 ; str W(reg), [X(base), rel]),
            Access::Str64 => dynasm!(self.ops ; .arch aarch64 ; str X(reg), [X(base), rel]),
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
        self.field(Access::Ldr8, 0, seg_field(seg, layout::SEG_ATTR));
        dynasm!(self.ops ; .arch aarch64 ; cmp w0, layout::AR_DATA_RW as u32 & 0xFF ; b.ne =>at);
        self.field(Access::Ldr8, 0, seg_field(seg, layout::SEG_RIGHTS));
        dynasm!(self.ops ; .arch aarch64 ; cmp w0, (layout::RIGHT_READ | layout::RIGHT_WRITE) as u32 ; b.ne =>at);
        self.field(Access::Ldr32, 0, seg_field(seg, layout::SEG_LO));
        dynasm!(self.ops ; .arch aarch64 ; cbnz w0, =>at);
        self.field(Access::Ldr32, 0, seg_field(seg, layout::SEG_HI));
        dynasm!(self.ops ; .arch aarch64 ; cmn w0, 1);
        match sel {
            None => dynasm!(self.ops ; .arch aarch64 ; b.eq =>at),
            Some(t) => {
                let plain = self.ops.new_dynamic_label();
                dynasm!(self.ops ; .arch aarch64 ; b.ne =>plain);
                self.field(Access::Ldr32, 0, seg_field(seg, layout::SEG_BASE));
                dynasm!(self.ops
                    ; .arch aarch64
                    ; cbz w0, =>at
                    ; tst W(r(t)), 0xFFFF
                    ; b.eq =>at
                    ; =>plain
                );
            }
        }
        let end = self.end();
        self.slow.push(Slow::Bail { at, end, ix: self.ix, dirty: self.dirty, leave });
    }

    /// A store of `src` through handle `m` that isn't to plain RAM: if it is
    /// to the VGA's graphics window, with writes there plain ones into the
    /// planes (`JitCtx::vga_ok`), write each plane the map mask selects
    /// and count it in the bus's activity as `Activity::video_write` does,
    /// and go on at `back`; else (and where the write would start a burst
    /// of its own) fall through, to `jit_dev_write`.
    fn vga_store(&mut self, back: DynamicLabel, m: T, src: T, size: u8) {
        let s = size as u32;
        dynasm!(self.ops
            ; .arch aarch64
            ; tbz X(r(m)), DEV_BIT as u32, >slow
            ; ldrb w0, [x20, CTX_VGA_OK as u32]
            ; cbz w0, >slow
            ; movz w2, 0xA, lsl 16
            ; sub w1, W(r(m)), w2
            ; movz w2, 0x10000 - s
            ; cmp w1, w2
            ; b.hi >slow
        );
        self.field(Access::Ldr64, 3, layout::ICOUNT);
        self.field(Access::Ldr64, 4, layout::LAST_WRITE);
        dynasm!(self.ops
            ; .arch aarch64
            ; sub x5, x3, x4
            ; movz w6, layout::BURST_GAP as u32
            ; cmp x5, x6
            ; b.hi >slow
        );
        self.field(Access::Ldr64, 5, layout::VIDEO_BYTES);
        dynasm!(self.ops ; .arch aarch64 ; add x5, x5, s);
        self.field(Access::Str64, 5, layout::VIDEO_BYTES);
        self.field(Access::Ldr64, 5, layout::BURST);
        dynasm!(self.ops ; .arch aarch64 ; add x5, x5, s);
        self.field(Access::Str64, 5, layout::BURST);
        self.field(Access::Str64, 3, layout::LAST_WRITE);
        dynasm!(self.ops ; .arch aarch64 ; movz w6, 1 ; strb w6, [x20, CTX_VGA_WROTE as u32]);
        for p in 0..4u32 {
            let at = CTX_VGA_PLANES as u32 + p * 8;
            dynasm!(self.ops ; .arch aarch64 ; ldr x7, [x20, at]);
            match size {
                1 => dynasm!(self.ops ; .arch aarch64 ; strb W(r(src)), [x7, x1]),
                2 => dynasm!(self.ops ; .arch aarch64 ; strh W(r(src)), [x7, x1]),
                _ => dynasm!(self.ops ; .arch aarch64 ; str W(r(src)), [x7, x1]),
            }
        }
        dynasm!(self.ops ; .arch aarch64 ; b =>back ; slow:);
    }

    /// Store the `size` bytes of `src` into RAM at handle `m`.
    fn store_ram(&mut self, m: T, src: T, size: u8) {
        let (m_, s) = (r(m), r(src));
        match size {
            1 => dynasm!(self.ops ; .arch aarch64 ; strb W(s), [x21, X(m_)]),
            2 => dynasm!(self.ops ; .arch aarch64 ; strh W(s), [x21, X(m_)]),
            _ => dynasm!(self.ops ; .arch aarch64 ; str W(s), [x21, X(m_)]),
        }
    }

    /// X`reg` = the address of the CPU's field at `off`.
    fn cpu_addr(&mut self, reg: u8, off: usize) {
        if off < 4096 {
            dynasm!(self.ops ; .arch aarch64 ; add XSP(reg), x19, off as u32);
        } else if off >= REGS && off - REGS < 4096 {
            dynasm!(self.ops ; .arch aarch64 ; add XSP(reg), x29, (off - REGS) as u32);
        } else {
            self.mov32(reg, off as u32);
            dynasm!(self.ops ; .arch aarch64 ; add X(reg), x19, X(reg));
        }
    }

    /// W`reg` = ST(i)'s physical number.
    fn fpu_phys(&mut self, reg: u8, i: u8) {
        self.field(Access::Ldr64, reg, layout::fpu::TOP);
        if i != 0 {
            dynasm!(self.ops ; .arch aarch64 ; add WSP(reg), WSP(reg), i as u32 ; and WSP(reg), W(reg), 7);
        }
    }

    /// Give FPU register W`phys` (a physical number) the double in D`x`,
    /// its 80 bits to be made from it (`f80::FpuRegs`). X2 is changed.
    fn fpu_store(&mut self, phys: u8, x: u8) {
        self.cpu_addr(2, layout::fpu::F64);
        dynasm!(self.ops ; .arch aarch64 ; str D(x), [x2, X(phys), lsl 3]);
        self.cpu_addr(2, layout::fpu::STALE);
        dynasm!(self.ops ; .arch aarch64 ; movz w4, 1 ; strb w4, [x2, X(phys)]);
    }

    /// Tag FPU register W`phys` `tag`. X2 and W4 are changed.
    fn fpu_tag(&mut self, phys: u8, tag: u32) {
        self.cpu_addr(2, layout::fpu::TAGS);
        dynasm!(self.ops ; .arch aarch64 ; movz w4, tag ; strb w4, [x2, X(phys)]);
    }

    /// Copy FPU register W`src` into W`dst` (physical numbers): the double,
    /// whether its 80 bits are stale, and the 80 bits.
    fn fpu_move(&mut self, src: u8, dst: u8) {
        self.cpu_addr(2, layout::fpu::F64);
        dynasm!(self.ops ; .arch aarch64 ; ldr d16, [x2, X(src), lsl 3] ; str d16, [x2, X(dst), lsl 3]);
        self.cpu_addr(2, layout::fpu::STALE);
        dynasm!(self.ops ; .arch aarch64 ; ldrb w4, [x2, X(src)] ; strb w4, [x2, X(dst)]);
        self.cpu_addr(2, layout::fpu::X80);
        dynasm!(self.ops
            ; .arch aarch64
            ; lsl w4, W(src), 4
            ; lsl w5, W(dst), 4
            ; ldr q16, [x2, x4]
            ; str q16, [x2, x5]
        );
    }

    /// Make the double in D`x` what an FPU register holds
    /// (`f80::canon_f64`): a denormal 0, a NaN quiet.
    /// IN or OUT of `size` bytes at port `port` through `jit_port`, as
    /// x86-64's: its value into (or from) t; a fault or panic leaves.
    fn port_io(&mut self, out: bool, size: u8, port: Src, t: T) {
        let fail = self.fail();
        let desc = out as u32 | (size as u32) << 1 | (self.ix as u32) << 8;
        let lit = self.data_lit;
        match port {
            Src::Imm(p) => self.mov32(4, p),
            Src::T(p) => dynasm!(self.ops ; .arch aarch64 ; mov w4, W(r(p))),
        }
        if out {
            dynasm!(self.ops ; .arch aarch64 ; mov w5, W(r(t)));
        }
        dynasm!(self.ops ; .arch aarch64 ; mov x0, x19 ; mov x1, x20 ; ldr x2, =>lit);
        self.mov32(3, desc);
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x16, [x20, CTX_PORT as u32]
            ; blr x16
            ; lsr x9, x0, 32
            ; cbz x9, >done
            ; mov w0, w9
            ; b =>fail
            ; done:
        );
        if !out {
            dynasm!(self.ops ; .arch aarch64 ; mov W(r(t)), w0);
        }
    }

    /// Call the trampoline's `fpu_addsub` for ST(dst) = ST(a) + ST(b) (or
    /// minus, with `sub`), of `regs` = [a, b, dst] (8 for the double in D16):
    /// W0 is 0 where it did, else the handler's call is to follow.
    fn fpu_addsub(&mut self, regs: [u8; 3], sub: bool) {
        self.field(Access::Ldr64, 4, layout::fpu::TOP);
        for (reg, i) in regs.into_iter().enumerate() {
            match i {
                8 => dynasm!(self.ops ; .arch aarch64 ; movz W(reg as u8), 8),
                0 => dynasm!(self.ops ; .arch aarch64 ; mov W(reg as u8), w4),
                _ => dynasm!(self.ops ; .arch aarch64 ; add WSP(reg as u8), w4, i as u32 ; and WSP(reg as u8), W(reg as u8), 7),
            }
        }
        dynasm!(self.ops
            ; .arch aarch64
            ; movz w3, sub as u32
            ; ldr x16, [x20, (CTX_FPU + 32) as u32]
            ; blr x16
        );
    }

    /// Call `jit_fpu_addsub_st` with `desc` and the handle in `arg`.
    fn fpu_op(&mut self, desc: u32, arg: Option<T>) {
        if let Some(t) = arg {
            dynasm!(self.ops ; .arch aarch64 ; mov w3, W(r(t)));
        }
        dynasm!(self.ops ; .arch aarch64 ; mov x0, x19 ; mov x1, x20);
        self.mov32(2, desc);
        dynasm!(self.ops ; .arch aarch64 ; ldr x16, [x20, CTX_FPU as u32] ; blr x16);
    }

    fn fpu_canon(&mut self, x: u8) {
        dynasm!(self.ops
            ; .arch aarch64
            ; fmov x1, D(x)
            ; ubfx x2, x1, 52, 11
            ; sub w2, w2, 1
            ; cmp w2, 0x7FE
            ; b.lo >canon_done
            ; cmn w2, 1
            ; b.ne >canon_nan
            // A zero or a denormal: the sign alone.
            ; and x1, x1, 0x8000_0000_0000_0000
            ; b >canon_fix
            ; canon_nan:
            ; lsl x2, x1, 12
            ; cbz x2, >canon_done
            ; orr x1, x1, 1 << 51
            ; canon_fix:
            ; fmov D(x), x1
            ; canon_done:
        );
    }

    /// `Uop::FpuGuard`: the instruction's handler runs it after all where
    /// CR0 has EM or TS set, or one of the registers in `valid` is empty
    /// (see `x64::Gen::fpu_guard`).
    fn fpu_guard(&mut self, valid: u8) {
        const EM_TS: u32 = 0x0C;
        let tags = valid & !self.fpu_known;
        if self.fpu_cr0_checked && tags == 0 {
            return;
        }
        let at = self.ops.new_dynamic_label();
        if !self.fpu_cr0_checked {
            self.field(Access::Ldr32, 0, layout::CR0);
            dynasm!(self.ops ; .arch aarch64 ; tst w0, EM_TS ; b.ne =>at);
        }
        if tags != 0 {
            self.field(Access::Ldr64, 1, layout::fpu::TOP);
            self.cpu_addr(2, layout::fpu::TAGS);
        }
        for i in (0..8).filter(|i| tags >> i & 1 != 0) {
            dynasm!(self.ops
                ; .arch aarch64
                ; add w3, w1, i
                ; and w3, w3, 7
                ; ldrb w3, [x2, x3]
                ; cmp w3, FPU_EMPTY
                ; b.eq =>at
            );
        }
        self.fpu_cr0_checked = true;
        self.fpu_known |= valid;
        let end = self.end();
        self.slow.push(Slow::Bail { at, end, ix: self.ix, dirty: self.dirty, leave: true });
    }

    /// `Uop::FToInt`: the conversion inline where the control word rounds
    /// to nearest or chops and the result fits, else through
    /// `jit_fpu_to_int`. (AArch64's conversions saturate and make a NaN 0,
    /// where the FPU stores the integer indefinite: those go the slow way.)
    fn fpu_to_int(&mut self, t: T, x: X, size: u8) {
        let (t_, x_) = (r(t), d(x));
        self.field(Access::Ldr16, 0, layout::fpu::CONTROL);
        dynasm!(self.ops
            ; .arch aarch64
            ; fcmp D(x_), D(x_)
            ; b.vs >toint_slow
            ; and w0, w0, 0xC00
            ; cbnz w0, >toint_other
            ; fcvtns W(t_), D(x_)
            ; b >toint_check
            ; toint_other:
            ; cmp w0, 0xC00
            ; b.ne >toint_slow
            ; fcvtzs W(t_), D(x_)
            ; toint_check:
        );
        if size == 2 {
            dynasm!(self.ops ; .arch aarch64 ; cmp WSP(t_), W(t_), sxth ; b.eq >toint_done);
        } else {
            // Saturated (or exactly the limits, which the handler gets right
            // too).
            dynasm!(self.ops
                ; .arch aarch64
                ; movz w1, 0x8000, lsl 16
                ; cmp W(t_), w1
                ; b.eq >toint_slow
                ; sub w1, w1, 1
                ; cmp W(t_), w1
                ; b.ne >toint_done
            );
        }
        dynasm!(self.ops ; .arch aarch64 ; toint_slow: ; fmov d0, D(x_) ; mov x0, x19 ; mov x1, x20);
        self.mov32(2, size as u32);
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x16, [x20, (CTX_FPU + 16) as u32]
            ; blr x16
            ; mov W(t_), w0
            ; toint_done:
        );
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

    fn sub_field64(&mut self, off: usize, v: u32) {
        if v == 0 {
            return;
        }
        self.field(Access::Ldr64, 0, off);
        if v < 4096 {
            dynasm!(self.ops ; .arch aarch64 ; sub x0, x0, v);
        } else {
            self.mov32(9, v);
            dynasm!(self.ops ; .arch aarch64 ; sub x0, x0, x9);
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
                if self.lazy_start[ix] {
                    // The flags as the instruction starts are a recorded
                    // operation's.
                    self.lazy_flags();
                }
                if let Some(join) = self.join[ix] {
                    dynasm!(self.ops ; .arch aarch64 ; =>join);
                }
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
                        ; mov X(r(t)), x0
                        ; b =>back
                        ; fault:
                        ; movz w0, EXIT_FAULT
                        ; b =>fail
                    );
                }
                Slow::Recheck { at, m, slow, ram } => {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; =>at
                        ; tst X(r(m)), NOT_RAM
                        ; b.ne =>slow
                        ; b =>ram
                    );
                }
                Slow::Load { at, back, dst, m, size } => {
                    // Memory that isn't plain RAM, by its physical address
                    // (`DEV_BIT`), or else the operand checked into a slot.
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; =>at
                        ; tbz X(r(m)), DEV_BIT as u32, >generic
                        ; mov x0, x19
                        ; mov x1, x20
                        ; mov w2, W(r(m))
                        ; movz w3, size as u32
                        ; ldr x16, [x20, CTX_DEV as u32]
                        ; blr x16
                        ; mov W(r(dst)), w0
                        ; b =>back
                        ; generic:
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
                Slow::After { at, next, code, fail } => {
                    dynasm!(self.ops ; .arch aarch64 ; =>at);
                    self.mov32(1, next);
                    self.field(Access::Str32, 1, layout::EIP);
                    self.mov32(0, code);
                    dynasm!(self.ops ; .arch aarch64 ; b =>fail);
                }
                Slow::Smc { at, next, dirty, lazy, join } => {
                    dynasm!(self.ops ; .arch aarch64 ; =>at);
                    self.mov32(0, next);
                    self.field(Access::Str32, 0, layout::EIP);
                    self.mov32(0, EXIT_SMC | if dirty { EXIT_FLAGS } else { 0 });
                    if lazy {
                        // The flags after the instruction are a recorded
                        // operation's.
                        self.lazy_flags();
                    }
                    dynasm!(self.ops ; .arch aarch64 ; b =>join);
                }
                Slow::Fetch { at, back, stub, slot, g } => {
                    let off = slot as u32 * 8;
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; =>at
                        ; ldr x0, [x1, DATA_LINKS as u32 + off]
                        ; ldr x2, [x1, DATA_STUBS as u32 + off]
                        ; cmp x0, x2
                        ; b.eq =>stub
                        ; ldr w3, [x1, g + GUARD_EIP as u32]
                        ; and w2, w3, 0xFFF
                        ; ldr w4, [x1, g + GUARD_PAGE as u32]
                        ; orr w2, w2, w4, lsl 12
                        ; mov x0, x19
                        ; mov x1, x20
                        ; ldr x16, [x20, CTX_FETCH as u32]
                        ; blr x16
                    );
                    self.data_x1();
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; cbz w0, =>stub
                        ; b =>back
                    );
                }
                Slow::Dev { at, back, slow, t, addr, size } => {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; =>at
                        ; and w1, W(addr), 0xFFF
                        ; cmp w1, 0x1000 - size as u32
                        ; b.hi =>slow
                        ; orr XSP(r(t)), X(addr), 1u64 << DEV_BIT
                        ; b =>back
                    );
                }
                Slow::Bail { at, end, ix, dirty, leave } => {
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
                    if leave {
                        // The instruction is done: the execution loop goes on.
                        dynasm!(self.ops ; .arch aarch64 ; movz w8, EXIT_AFTER ; b >stop);
                    }
                    if self.end_dirty[ix] {
                        self.field(Access::Ldr32, 28, layout::FLAGS);
                    }
                    let fail_tail = self.fail_tail;
                    dynasm!(self.ops ; .arch aarch64 ; b =>end ; stop:);
                    self.mov32(9, (ix as u32) << 8);
                    dynasm!(self.ops ; .arch aarch64 ; orr w0, w8, w9 ; b =>fail_tail);
                }
                Slow::CodeStore { at, back, m, src, size, lo, hi } => {
                    let m_ = r(m);
                    let (last, width, shift) = (size as u32 - 1, size as u32, crate::bus::GEN_SHIFT as u32);
                    dynasm!(self.ops ; .arch aarch64 ; =>at);
                    self.store_ram(m, src, size);
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
                    dynasm!(self.ops ; .arch aarch64 ; b =>back);
                }
                Slow::Store { at, back, m, src, size, lo, hi } => {
                    dynasm!(self.ops ; .arch aarch64 ; =>at);
                    self.vga_store(back, m, src, size);
                    self.mov32(9, lo);
                    dynasm!(self.ops ; .arch aarch64 ; str w9, [x20, CTX_SMC_LO as u32]);
                    self.mov32(9, hi);
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; str w9, [x20, CTX_SMC_HI as u32]
                        ; tbz X(r(m)), DEV_BIT as u32, >generic
                        ; mov x0, x19
                        ; mov x1, x20
                        ; mov w2, W(r(m))
                        ; mov w3, W(r(src))
                        ; movz w4, size as u32
                        ; ldr x16, [x20, (CTX_DEV + 8) as u32]
                        ; blr x16
                        ; orr w23, w23, w0
                        ; b =>back
                        ; generic:
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
        if data.live_imm(self.ix) {
            return;
        }
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
            Uop::LoadCode { t, phys, size, signed } => {
                let t = r(t);
                self.mov32(t, phys);
                match (size, signed) {
                    (1, false) => dynasm!(self.ops ; .arch aarch64 ; ldrb W(t), [x21, X(t)]),
                    (1, true) => dynasm!(self.ops ; .arch aarch64 ; ldrsb W(t), [x21, X(t)]),
                    (2, false) => dynasm!(self.ops ; .arch aarch64 ; ldrh W(t), [x21, X(t)]),
                    (2, true) => dynasm!(self.ops ; .arch aarch64 ; ldrsh W(t), [x21, X(t)]),
                    _ => dynasm!(self.ops ; .arch aarch64 ; ldr W(t), [x21, X(t)]),
                }
            }
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
            Uop::MemRef { t, seg, size, write, slot } => self.memref(t, seg, size, write, slot, known[t.0 as usize]),
            Uop::Load { dst, m, size } => {
                let (at, back) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
                let (d, m_) = (r(dst), r(m));
                // A handle with bits from 31 up (SLOW and up, `DEV_BIT`) isn't
                // an address in RAM.
                self.ram_or(m, at);
                match size {
                    1 => dynasm!(self.ops ; .arch aarch64 ; ldrb W(d), [x21, X(m_)]),
                    2 => dynasm!(self.ops ; .arch aarch64 ; ldrh W(d), [x21, X(m_)]),
                    _ => dynasm!(self.ops ; .arch aarch64 ; ldr W(d), [x21, X(m_)]),
                }
                dynasm!(self.ops ; .arch aarch64 ; =>back);
                self.slow.push(Slow::Load { at, back, dst, m, size });
            }
            Uop::Store { m, src, size } => {
                let (at, back, code) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
                let m_ = r(m);
                // RAM of a block without code is written as it is, without
                // bumping its generation, which nothing reads (see
                // `Bus::code_blocks`).
                self.ram_or(m, at);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr x3, [x20, CTX_CODE_BLOCKS as u32]
                    ; lsr w1, W(m_), crate::bus::GEN_SHIFT as u32
                    ; ldrb w2, [x3, x1]
                    ; cbnz w2, =>code
                );
                self.store_ram(m, src, size);
                dynasm!(self.ops ; .arch aarch64 ; =>back);
                let (lo, hi) = self.rest();
                self.slow.push(Slow::Store { at, back, m, src, size, lo, hi });
                self.slow.push(Slow::CodeStore { at: code, back, m, size, src, lo, hi });
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
                self.slow.push(Slow::Bail { at, end, ix: self.ix, dirty: self.dirty, leave: false });
            }
            Uop::BailUnlessRam { t } => {
                let at = self.ops.new_dynamic_label();
                dynasm!(self.ops ; .arch aarch64 ; tst X(r(t)), NOT_RAM ; b.ne =>at);
                let end = self.end();
                self.slow.push(Slow::Bail { at, end, ix: self.ix, dirty: self.dirty, leave: false });
            }
            Uop::Imul { size, a, b } => self.imul(size, a, b),
            Uop::BitOp { op, size, t, bit } => {
                let bits = size as u32 * 8;
                match bit {
                    Src::Imm(v) => self.mov32(1, v),
                    Src::T(b) => dynasm!(self.ops ; .arch aarch64 ; and w1, W(r(b)), bits - 1),
                }
                if self.live_after & (CF | OF) != 0 {
                    // CF is bit 0 of the value rotated right by the bit, OF
                    // whether its top two bits differ (0 or 3 plus 1 has
                    // bit 1 clear, 1 or 2 plus 1 set). A word is rotated
                    // in both halves of W2.
                    if size == 2 {
                        dynasm!(self.ops ; .arch aarch64 ; orr w2, W(r(t)), W(r(t)), lsl 16 ; ror w2, w2, w1);
                    } else {
                        dynasm!(self.ops ; .arch aarch64 ; ror w2, W(r(t)), w1);
                    }
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; lsr w4, w2, bits - 2
                        ; and w4, w4, 3
                        ; add w4, w4, 1
                        ; and w4, w4, 2
                        ; and w5, w2, 1
                        ; orr w5, w5, w4, lsl 10
                    );
                    self.merge(CF | OF);
                }
                if op != BitKind::Test {
                    dynasm!(self.ops ; .arch aarch64 ; movz w3, 1 ; lsl w3, w3, w1);
                    match op {
                        BitKind::Set => dynasm!(self.ops ; .arch aarch64 ; orr W(r(t)), W(r(t)), w3),
                        BitKind::Reset => dynasm!(self.ops ; .arch aarch64 ; bic W(r(t)), W(r(t)), w3),
                        _ => dynasm!(self.ops ; .arch aarch64 ; eor W(r(t)), W(r(t)), w3),
                    }
                }
            }
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
            Uop::CheckIopl => {
                let (gp, ok) = (self.fault_exit(EXIT_GP0), self.ops.new_dynamic_label());
                self.field(Access::Ldr32, 0, layout::CR0);
                dynasm!(self.ops ; .arch aarch64 ; tbz w0, 0, =>ok);
                self.field(Access::Ldr8, 1, layout::CPL);
                self.field(Access::Ldr32, 2, layout::FLAGS);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ubfx w2, w2, 12, 2
                    ; cmp w1, w2
                    ; b.hi =>gp
                    ; =>ok
                );
            }
            Uop::GetSeg { t, seg } => self.field(Access::Ldr16, r(t), seg_field(seg, layout::SEG_SELECTOR)),
            Uop::CheckV86Iopl => {
                let (gp, ok) = (self.fault_exit(EXIT_GP0), self.ops.new_dynamic_label());
                self.field(Access::Ldr32, 0, layout::FLAGS);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; tbz w0, 17, =>ok
                    ; ubfx w0, w0, 12, 2
                    ; cmp w0, 3
                    ; b.ne =>gp
                    ; =>ok
                );
            }
            Uop::GetFlags { t, size } => {
                let t_ = r(t);
                self.field(Access::Ldr32, t_, layout::FLAGS);
                if self.dirty {
                    self.mov32(9, ARITH);
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; bic W(t_), W(t_), w9
                        ; and w0, w28, w9
                        ; orr W(t_), W(t_), w0
                    );
                }
                if size == 2 {
                    dynasm!(self.ops ; .arch aarch64 ; and WSP(t_), W(t_), 0xFFFF);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; and WSP(t_), W(t_), !0x3_0000u32);
                }
            }
            Uop::CsReal => self.real_load(Seg::CS, None, false),
            Uop::LoadSeg { seg, t } if self.env.bits & super::ENV_REAL != 0 => {
                // As `Cpu::load_seg_real` loads it once it is a plain
                // segment (which it stays): its handler runs the
                // instruction otherwise, and the block stops after it.
                self.real_load(seg, Some(t), true);
                dynasm!(self.ops ; .arch aarch64 ; and w0, W(r(t)), 0xFFFF);
                self.field(Access::Str16, 0, seg_field(seg, layout::SEG_SELECTOR));
                dynasm!(self.ops ; .arch aarch64 ; lsl w0, w0, 4);
                self.field(Access::Str32, 0, seg_field(seg, layout::SEG_BASE));
            }
            Uop::LoadCsReal { t } => {
                self.data_x1();
                dynasm!(self.ops ; .arch aarch64 ; str x1, [x20, CTX_FAR_BLOCK as u32]);
                self.field(Access::Ldr32, 0, seg_field(Seg::CS, layout::SEG_BASE));
                dynasm!(self.ops
                    ; .arch aarch64
                    ; str w0, [x20, CTX_FAR_BASE as u32]
                    ; and w0, W(r(t)), 0xFFFF
                );
                self.field(Access::Str16, 0, seg_field(Seg::CS, layout::SEG_SELECTOR));
                dynasm!(self.ops ; .arch aarch64 ; lsl w0, w0, 4);
                self.field(Access::Str32, 0, seg_field(Seg::CS, layout::SEG_BASE));
            }
            Uop::Spill { t, slot } => {
                let at = CTX_SCRATCH as u32 + slot as u32 * 4;
                dynasm!(self.ops ; .arch aarch64 ; str W(r(t)), [x20, at]);
            }
            Uop::Unspill { t, slot } => {
                let at = CTX_SCRATCH as u32 + slot as u32 * 4;
                dynasm!(self.ops ; .arch aarch64 ; ldr W(r(t)), [x20, at]);
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
                    // A return or indirect call: through its link to
                    // where it goes, if it has one.
                    self.counts();
                    self.check_flat(t);
                    self.returned(t, false);
                } else {
                    let tail = self.tail;
                    dynasm!(self.ops ; .arch aarch64 ; b =>tail);
                }
            }
            Uop::ExitIf { cond, taken, next, commit } => self.exit_if(cond, taken, next, commit),
            Uop::LoadSeg { seg, t } => {
                // As x86-64's: `jit_load_seg`, and the segment's accesses
                // from here on as if it weren't flat.
                let fail = self.fail();
                dynasm!(self.ops ; .arch aarch64 ; mov w3, W(r(t)) ; mov x0, x19 ; mov x1, x20);
                self.mov32(2, seg as u32);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr x16, [x20, CTX_LOAD_SEG as u32]
                    ; blr x16
                    ; cbnz w0, =>fail
                );
                self.loaded_segs |= 1 << seg as u8;
            }
            Uop::In { size, port, t } => self.port_io(false, size, port, t),
            Uop::Out { size, port, t } => self.port_io(true, size, port, t),
            Uop::Sti => {
                // As x86-64's: interrupts are recognized after the next
                // instruction; with one waiting the execution loop runs
                // that, else the shadow ends where the next instruction
                // runs, in the block.
                let data = self.data;
                let next = data.eips[self.ix].wrapping_add(data.instrs[self.ix].len() as u32);
                let at = self.ops.new_dynamic_label();
                let fail = self.fail();
                let code = EXIT_AFTER | if self.dirty { EXIT_FLAGS } else { 0 };
                self.slow.push(Slow::After { at, next, code, fail });
                self.field(Access::Ldr32, 0, layout::FLAGS);
                dynasm!(self.ops ; .arch aarch64 ; orr w1, w0, 0x200);
                self.field(Access::Str32, 1, layout::FLAGS);
                let was_set = self.ops.new_dynamic_label();
                dynasm!(self.ops ; .arch aarch64 ; tbnz w0, 9, =>was_set ; movz w2, 1);
                self.field(Access::Str8, 2, layout::IRQ_SHADOW);
                dynasm!(self.ops ; .arch aarch64 ; =>was_set);
                self.field(Access::Ldr8, 2, layout::IRQ_READY);
                dynasm!(self.ops ; .arch aarch64 ; cbnz w2, =>at);
                if self.ix + 1 < data.count() {
                    dynasm!(self.ops ; .arch aarch64 ; movz w2, 0);
                    self.field(Access::Str8, 2, layout::IRQ_SHADOW);
                }
            }
            Uop::RepStart { t, count, max } => {
                // As x86-64's: a count up to `max`, forward and not traced,
                // runs here; else the handler.
                let access = if count.size == 4 { Access::Ldr32 } else { Access::Ldr16 };
                self.field(access, r(t), gpr_offset(count));
                let at = self.ops.new_dynamic_label();
                self.field(Access::Ldr32, 0, layout::FLAGS);
                self.mov32(1, max);
                // TF or DF.
                self.mov32(2, 0x500);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; tst w0, w2
                    ; b.ne =>at
                    ; cmp W(r(t)), w1
                    ; b.hi =>at
                );
                let end = self.end();
                self.slow.push(Slow::Bail { at, end, ix: self.ix, dirty: self.dirty, leave: false });
                let top = self.ops.new_dynamic_label();
                dynasm!(self.ops ; .arch aarch64 ; cbz W(r(t)), =>end ; =>top);
                self.rep_top = Some(top);
            }
            Uop::RepEnd { t } => {
                let top = self.rep_top.take().expect("RepStart before RepEnd");
                dynasm!(self.ops ; .arch aarch64 ; cbnz W(r(t)), =>top);
            }
            Uop::Forward => {
                let at = self.ops.new_dynamic_label();
                self.field(Access::Ldr32, 0, layout::FLAGS);
                dynasm!(self.ops ; .arch aarch64 ; tbnz w0, 10, =>at);
                let end = self.end();
                self.slow.push(Slow::Bail { at, end, ix: self.ix, dirty: self.dirty, leave: false });
            }
            Uop::FpuGuard { valid } => self.fpu_guard(valid),
            Uop::FGet { x, i } => {
                self.fpu_phys(1, i);
                self.cpu_addr(2, layout::fpu::F64);
                dynasm!(self.ops ; .arch aarch64 ; ldr D(d(x)), [x2, x1, lsl 3]);
            }
            Uop::FSet { i, x, canon } => {
                if canon {
                    self.fpu_canon(d(x));
                }
                self.fpu_phys(1, i);
                self.fpu_store(1, d(x));
            }
            Uop::FPush { x, canon } => {
                self.fpu_known = self.fpu_known << 1 | 1;
                if canon {
                    self.fpu_canon(d(x));
                }
                self.field(Access::Ldr64, 1, layout::fpu::TOP);
                dynasm!(self.ops ; .arch aarch64 ; sub w1, w1, 1 ; and w1, w1, 7);
                self.field(Access::Str64, 1, layout::fpu::TOP);
                self.fpu_store(1, d(x));
                self.fpu_tag(1, FPU_VALID);
            }
            Uop::FPop { n } => {
                self.fpu_known >>= n;
                self.field(Access::Ldr64, 1, layout::fpu::TOP);
                for _ in 0..n {
                    self.fpu_tag(1, FPU_EMPTY);
                    dynasm!(self.ops ; .arch aarch64 ; add w1, w1, 1 ; and w1, w1, 7);
                }
                self.field(Access::Str64, 1, layout::fpu::TOP);
            }
            Uop::FCopy { dst, src } => {
                // W1 the source's physical number, W3 the destination's.
                self.field(Access::Ldr64, 0, layout::fpu::TOP);
                dynasm!(self.ops ; .arch aarch64 ; add w1, w0, src as u32 ; and w1, w1, 7);
                match dst {
                    Some(dst) => dynasm!(self.ops ; .arch aarch64 ; add w3, w0, dst as u32 ; and w3, w3, 7),
                    None => {
                        self.fpu_known = self.fpu_known << 1 | 1;
                        dynasm!(self.ops ; .arch aarch64 ; sub w3, w0, 1 ; and w3, w3, 7);
                        self.field(Access::Str64, 3, layout::fpu::TOP);
                        self.fpu_tag(3, FPU_VALID);
                    }
                }
                self.fpu_move(1, 3);
            }
            Uop::FXch { i } => {
                const C1: u32 = 0x200;
                // W1 ST(0)'s physical number, W3 ST(i)'s: the registers
                // swap through the scratch ones.
                self.field(Access::Ldr64, 1, layout::fpu::TOP);
                dynasm!(self.ops ; .arch aarch64 ; add w3, w1, i as u32 ; and w3, w3, 7);
                self.cpu_addr(2, layout::fpu::F64);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr d16, [x2, x1, lsl 3]
                    ; ldr d17, [x2, x3, lsl 3]
                    ; str d17, [x2, x1, lsl 3]
                    ; str d16, [x2, x3, lsl 3]
                );
                self.cpu_addr(2, layout::fpu::STALE);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldrb w4, [x2, x1]
                    ; ldrb w5, [x2, x3]
                    ; strb w5, [x2, x1]
                    ; strb w4, [x2, x3]
                );
                self.cpu_addr(2, layout::fpu::X80);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; lsl w4, w1, 4
                    ; lsl w5, w3, 4
                    ; ldr q16, [x2, x4]
                    ; ldr q17, [x2, x5]
                    ; str q17, [x2, x4]
                    ; str q16, [x2, x5]
                );
                self.cpu_addr(2, layout::fpu::FLAGS);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldrh w4, [x2]
                    ; movz w5, C1
                    ; bic w4, w4, w5
                    ; strh w4, [x2]
                );
            }
            Uop::FFromT { x, t, kind } => match kind {
                FKind::Single => dynasm!(self.ops ; .arch aarch64 ; fmov s16, W(r(t)) ; fcvt D(d(x)), s16),
                FKind::Int => dynasm!(self.ops ; .arch aarch64 ; scvtf D(d(x)), W(r(t))),
            },
            Uop::FToX80 { t, part } => {
                // From the 80 bits where they aren't stale, else the
                // handlers'.
                self.fpu_phys(1, 0);
                self.cpu_addr(2, layout::fpu::STALE);
                dynasm!(self.ops ; .arch aarch64 ; ldrb w3, [x2, x1] ; cbnz w3, >x80_stale);
                self.cpu_addr(2, layout::fpu::X80);
                dynasm!(self.ops ; .arch aarch64 ; add x2, x2, x1, lsl 4);
                if part == 2 {
                    dynasm!(self.ops ; .arch aarch64 ; ldrh W(r(t)), [x2, 8]);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; ldr W(r(t)), [x2, part as u32 * 4]);
                }
                dynasm!(self.ops ; .arch aarch64 ; b >x80_done ; x80_stale:);
                self.fpu_op(3 << 13 | part as u32, None);
                dynasm!(self.ops ; .arch aarch64 ; mov W(r(t)), w0 ; x80_done:);
            }
            Uop::FToSingle { t, x } => {
                dynasm!(self.ops ; .arch aarch64 ; fcvt s16, D(d(x)) ; fmov W(r(t)), s16);
            }
            Uop::FLoad64 { x, m, canon } => {
                dynasm!(self.ops ; .arch aarch64 ; ldr D(d(x)), [x21, X(r(m))]);
                if canon {
                    self.fpu_canon(d(x));
                }
            }
            Uop::FToHalf { t, x, high } => {
                dynasm!(self.ops ; .arch aarch64 ; fmov X(r(t)), D(d(x)));
                if high {
                    dynasm!(self.ops ; .arch aarch64 ; lsr X(r(t)), X(r(t)), 32);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; mov W(r(t)), W(r(t)));
                }
            }
            Uop::FToInt { t, x, size } => self.fpu_to_int(t, x, size),
            Uop::FMul { a, b } => dynasm!(self.ops ; .arch aarch64 ; fmul D(d(a)), D(d(a)), D(d(b))),
            Uop::FDiv { i, num, den, ze } => {
                // A division by 0 (not a NaN, which compares unordered) is
                // the handler's.
                dynasm!(self.ops
                    ; .arch aarch64
                    ; fcmp D(d(den)), 0.0
                    ; b.ne >fdiv_go
                    ; mov x0, x19
                    ; mov x1, x20
                );
                self.mov32(2, i as u32 | (ze as u32) << 8);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr x16, [x20, (CTX_FPU + 24) as u32]
                    ; blr x16
                    ; b >fdiv_done
                    ; fdiv_go:
                    ; fdiv d16, D(d(num)), D(d(den))
                );
                self.fpu_canon(16);
                self.fpu_phys(1, i);
                self.fpu_store(1, 16);
                dynasm!(self.ops ; .arch aarch64 ; fdiv_done:);
            }
            Uop::FAddSt { dst, a, b, sub } => {
                self.fpu_addsub([a, b, dst], sub);
                dynasm!(self.ops ; .arch aarch64 ; cbz w0, >addsub_done);
                let desc = dst as u32 | (a as u32) << 4 | (b as u32) << 8 | (sub as u32) << 12;
                self.fpu_op(desc, None);
                dynasm!(self.ops ; .arch aarch64 ; addsub_done:);
            }
            Uop::FChs => self.fpu_op(1 << 13, None),
            Uop::FLoad80 { m } => self.fpu_op(2 << 13, Some(m)),
            Uop::FAddValue { kind, x } => {
                use crate::instructions::fpu::arithmetic::{ADD_VALUE, SUB_VALUE};
                // 8: the double in D16.
                let (a, b, sub) = match kind {
                    ADD_VALUE => (0, 8, false),
                    SUB_VALUE => (0, 8, true),
                    _ => (8, 0, true),
                };
                dynasm!(self.ops ; .arch aarch64 ; fmov d16, D(d(x)));
                self.fpu_addsub([a, b, 0], sub);
                dynasm!(self.ops ; .arch aarch64 ; cbz w0, >addsub_done);
                dynasm!(self.ops ; .arch aarch64 ; fmov d0, D(d(x)) ; mov x0, x19 ; mov x1, x20);
                self.mov32(2, kind);
                dynasm!(self.ops ; .arch aarch64 ; ldr x16, [x20, (CTX_FPU + 8) as u32] ; blr x16 ; addsub_done:);
            }
            Uop::FCom { a, b } => {
                // C0 where less or unordered, C2 where unordered, C3 where
                // equal or unordered (an unordered FCMP sets C and V).
                const C0_C2_C3: u32 = 0x4500;
                dynasm!(self.ops
                    ; .arch aarch64
                    ; fcmp D(d(a)), D(d(b))
                    ; cset w1, lt
                    ; cset w2, vs
                    ; cset w3, eq
                    ; orr w3, w3, w2
                    ; lsl w1, w1, 8
                    ; orr w1, w1, w2, lsl 10
                    ; orr w1, w1, w3, lsl 14
                );
                self.cpu_addr(2, layout::fpu::FLAGS);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldrh w4, [x2]
                    ; movz w5, C0_C2_C3
                    ; bic w4, w4, w5
                    ; orr w4, w4, w1
                    ; strh w4, [x2]
                );
            }
            Uop::FStatus { t } => {
                let t = r(t);
                self.field(Access::Ldr16, t, layout::fpu::FLAGS);
                self.field(Access::Ldr64, 1, layout::fpu::TOP);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; movz w2, 0x3800
                    ; bic W(t), W(t), w2
                    ; and w1, w1, 7
                    ; orr W(t), W(t), w1, lsl 11
                );
            }
            Uop::FGetControl { t } => self.field(Access::Ldr16, r(t), layout::fpu::CONTROL),
            Uop::FSetControl { t } => self.field(Access::Str16, r(t), layout::fpu::CONTROL),
        }
    }

    /// The register = the low bytes of W`reg`, stored as wide as it is (a
    /// Cortex-A76 runs that faster than merging the bytes into the whole
    /// dword).
    fn set_gpr(&mut self, g: Gpr, reg: u8) {
        let access = match g.size {
            4 => Access::Str32,
            2 => Access::Str16,
            _ => Access::Str8,
        };
        self.field(access, reg, gpr_offset(g));
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

    /// Check the operand at seg:t as `Cpu::mem_ref` does, inline within a
    /// page (through the TLB with paging on), and leave its handle in t:
    /// the address in RAM, or the physical address of memory that isn't
    /// plain RAM (`DEV_BIT`). What the block's environment says (flat and
    /// plain segments, paging, CPL, the A20 gate) isn't checked again, as
    /// `x64::Gen::memref` has it.
    /// Where a memory reference's slow paths come back to, if it is still
    /// pending (`ram_back`).
    fn settle_ram(&mut self) {
        if let Some((_, back)) = self.ram_back.take() {
            dynasm!(self.ops ; .arch aarch64 ; =>back);
        }
    }

    /// Go to `slow` unless handle `m` is RAM's: right after the memory
    /// reference that made it, only where its slow paths made it.
    fn ram_or(&mut self, m: T, slow: DynamicLabel) {
        match self.ram_back.take() {
            Some((t, back)) if t == m => {
                let ram = self.ops.new_dynamic_label();
                dynasm!(self.ops ; .arch aarch64 ; =>ram);
                self.slow.push(Slow::Recheck { at: back, m, slow, ram });
            }
            pending => {
                self.ram_back = pending;
                self.settle_ram();
                dynasm!(self.ops ; .arch aarch64 ; tst X(r(m)), NOT_RAM ; b.ne =>slow);
            }
        }
    }

    fn memref(&mut self, t: T, seg: Seg, size: u8, write: bool, slot: u8, known: Option<u32>) {
        let (at, back) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        let t_ = r(t);
        let bits = self.env.bits;
        let (paging, a20) = (bits & super::ENV_PAGING != 0, bits & super::ENV_A20 != 0);
        let unloaded = self.loaded_segs >> seg as u8 & 1 == 0;
        let flat = bits & super::ENV_FLAT << seg as u32 != 0 && unloaded;
        let plain = bits & super::ENV_PLAIN << seg as u32 != 0 && unloaded;
        let big = bits & super::ENV_BIG << seg as u32 != 0 && unloaded;
        let size32 = size as u32;
        let last = size32 - 1;
        // W0 = the linear address: a flat segment's is the offset (a wrap
        // past 4 GB goes past the end of RAM below; with paging off and the
        // A20 gate open, it is the physical address in T itself), else the
        // segment's limit and type are checked as `seg_linear` does, and
        // its base added.
        let addr = if flat && !paging && a20 { t_ } else { 0 };
        if flat {
            if addr == 0 {
                dynasm!(self.ops ; .arch aarch64 ; mov w0, W(t_));
            }
        } else {
            let need = if write { layout::RIGHT_WRITE } else { layout::RIGHT_READ } as u32;
            // W1 the last byte's offset (a constant offset's where it
            // doesn't wrap: known).
            match known.and_then(|v| v.checked_add(last)) {
                Some(end) if !big => self.mov32(1, end),
                Some(_) => {}
                None if size > 1 => {
                    dynasm!(self.ops ; .arch aarch64 ; add w1, WSP(t_), last ; cmp w1, W(t_) ; b.lo =>at);
                }
                None if !big => dynasm!(self.ops ; .arch aarch64 ; mov w1, W(t_)),
                None => {}
            }
            if !plain {
                self.field(Access::Ldr32, 2, seg_field(seg, layout::SEG_LO));
                dynasm!(self.ops ; .arch aarch64 ; cmp W(t_), w2 ; b.lo =>at);
            }
            // (Every offset is in a big segment's limits.)
            if !big {
                self.field(Access::Ldr32, 2, seg_field(seg, layout::SEG_HI));
                dynasm!(self.ops ; .arch aarch64 ; cmp w1, w2 ; b.hi =>at);
            }
            if !plain {
                self.field(Access::Ldr8, 2, seg_field(seg, layout::SEG_RIGHTS));
                dynasm!(self.ops ; .arch aarch64 ; tst w2, need ; b.eq =>at);
            }
            self.field(Access::Ldr32, 2, seg_field(seg, layout::SEG_BASE));
            dynasm!(self.ops ; .arch aarch64 ; add w0, W(t_), w2);
        }
        if paging || !a20 {
            // An operand in two pages takes two translations (or with the
            // A20 gate closed, may wrap around at a megabyte).
            if size > 1 {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; and w1, w0, 0xFFF
                    ; cmp w1, 0x1000 - size32
                    ; b.hi =>at
                );
            }
        }
        if paging {
            // The page's entry (linear address >> 12) in the TLB's set of
            // the privilege level, whose tag must be the page + 1.
            let tag = (if write { layout::TLB_WRITE_TAG } else { layout::TLB_READ_TAG }) as u32;
            let set = if bits & super::ENV_USER != 0 { layout::TLB_SET as u32 } else { 0 };
            dynasm!(self.ops
                ; .arch aarch64
                ; lsr w1, w0, 12
                ; and w3, w1, (layout::TLB_SET - 1) as u32
            );
            if set != 0 {
                dynasm!(self.ops ; .arch aarch64 ; add w3, w3, set);
            }
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
        }
        if !a20 {
            dynasm!(self.ops ; .arch aarch64 ; and w0, w0, !0x10_0000u32);
        }
        // In plain RAM: none of its bytes in the video memory and ROMs from
        // A0000h to FFFFFh, and not past the end of RAM: in extended memory
        // (looked at first in 32-bit code) or below the video memory. (With
        // paging off and the A20 gate open, an operand in two pages of RAM
        // is too: they are next to each other.) Elsewhere within a page,
        // the bus reaches it by its physical address (`Slow::Dev`).
        let dev = self.ops.new_dynamic_label();
        let ram = self.ops.new_dynamic_label();
        let extended = self.env.ram_len.checked_sub(EXTENDED + size32);
        let below = |g: &mut Self| {
            dynasm!(g.ops
                ; .arch aarch64
                ; add x1, XSP(addr), last
                ; cmp x1, VIDEO >> 12, lsl 12
            );
        };
        let above = |g: &mut Self, limit: u32| {
            dynasm!(g.ops ; .arch aarch64 ; sub w1, WSP(addr), EXTENDED >> 12, lsl 12);
            g.mov32(2, limit);
            dynasm!(g.ops ; .arch aarch64 ; cmp w1, w2);
        };
        // (A flat segment's constant offset is known to be.)
        let in_ram = |v: u32| {
            let end = v as u64 + last as u64;
            (end < VIDEO as u64 || v >= EXTENDED) && v <= self.env.ram_len.wrapping_sub(size32) && size32 <= self.env.ram_len
        };
        if !(addr == t_ && known.is_some_and(in_ram)) {
            match extended {
                Some(limit) if self.env.bits & 1 != 0 => {
                    above(self, limit);
                    dynasm!(self.ops ; .arch aarch64 ; b.ls =>ram);
                    below(self);
                    dynasm!(self.ops ; .arch aarch64 ; b.hs =>dev);
                }
                Some(limit) => {
                    below(self);
                    dynasm!(self.ops ; .arch aarch64 ; b.lo =>ram);
                    above(self, limit);
                    dynasm!(self.ops ; .arch aarch64 ; b.hi =>dev);
                }
                None => {
                    below(self);
                    dynasm!(self.ops ; .arch aarch64 ; b.hs =>dev);
                }
            }
        }
        dynasm!(self.ops ; .arch aarch64 ; =>ram);
        if addr != t_ {
            dynasm!(self.ops ; .arch aarch64 ; mov W(t_), w0);
        }
        self.ram_back = Some((t, back));
        let desc = memref_desc(seg, size, write, slot);
        let fail = self.fail();
        self.slow.push(Slow::Dev { at: dev, back, slow: at, t, addr, size });
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
        } else if ones == AF {
            dynasm!(self.ops ; .arch aarch64 ; orr w5, w5, AF);
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

    /// W0 = W10 + W11, or W10 - W11 (`sub`), cut to the size, and its flags
    /// `need` merged into W28. SF, ZF, CF and OF come from the NZCV flags of
    /// ADDS or SUBS of the operands shifted to the top of the register
    /// (where the size's carry and overflow are the register's), through
    /// `JitCtx::szco`; PF from the parity table and AF from the operands'
    /// bit 4, where they are live.
    fn add_sub_flags(&mut self, size: u8, sub: bool, need: u32) {
        let shift = 32 - size as u32 * 8;
        if need == 0 {
            if sub {
                dynasm!(self.ops ; .arch aarch64 ; sub w0, w10, w11);
            } else {
                dynasm!(self.ops ; .arch aarch64 ; add w0, w10, w11);
            }
            self.cut(size);
            return;
        }
        match (shift, sub) {
            (0, false) => dynasm!(self.ops ; .arch aarch64 ; adds w0, w10, w11),
            (0, true) => dynasm!(self.ops ; .arch aarch64 ; subs w0, w10, w11),
            (_, false) => dynasm!(self.ops
                ; .arch aarch64
                ; lsl w12, w10, shift
                ; lsl w13, w11, shift
                ; adds w0, w12, w13
            ),
            (_, true) => dynasm!(self.ops
                ; .arch aarch64
                ; lsl w12, w10, shift
                ; lsl w13, w11, shift
                ; subs w0, w12, w13
            ),
        }
        let table = CTX_SZCO as u32 + if sub { 32 } else { 0 };
        if need & (SF | ZF | CF | OF) != 0 {
            dynasm!(self.ops
                ; .arch aarch64
                ; mrs x5, NZCV
                ; lsr w5, w5, 28
                ; add x6, x20, table
                ; ldrh w5, [x6, x5, lsl 1]
            );
        } else {
            dynasm!(self.ops ; .arch aarch64 ; movz w5, 0);
        }
        if shift != 0 {
            dynasm!(self.ops ; .arch aarch64 ; lsr w0, w0, shift);
        }
        if need & PF != 0 {
            dynasm!(self.ops
                ; .arch aarch64
                ; and w6, w0, 0xFF
                ; add x6, x20, x6
                ; ldrb w6, [x6, CTX_PARITY as u32]
                ; orr w5, w5, w6
            );
        }
        if need & AF != 0 {
            dynasm!(self.ops
                ; .arch aarch64
                ; eor w6, w10, w11
                ; eor w6, w6, w0
                ; and w6, w6, AF
                ; orr w5, w5, w6
            );
        }
        self.merge(need);
    }

    /// Record operation `kind` (`flags::LAZY_*`) on W10 and W11 for
    /// `jit_lazy_flags`. W9 and W12 are changed.
    fn record_lazy(&mut self, kind: u32, size: u8) {
        dynasm!(self.ops ; .arch aarch64 ; add x9, x20, CTX_LAZY as u32);
        self.mov32(12, kind | (size as u32) << 8);
        dynasm!(self.ops
            ; .arch aarch64
            ; stp w12, w10, [x9]
            ; str w11, [x9, 8]
        );
    }

    /// W28 = the flags of the recorded operation, from `jit_lazy_flags`,
    /// keeping the exit code in W0, with `EXIT_FLAGS` set.
    fn lazy_flags(&mut self) {
        dynasm!(self.ops
            ; .arch aarch64
            ; str w0, [x20, CTX_LAZY_CODE as u32]
            ; mov x0, x19
            ; mov x1, x20
            ; ldr x16, [x20, CTX_LAZY_FN as u32]
            ; blr x16
            ; mov w28, w0
            ; ldr w0, [x20, CTX_LAZY_CODE as u32]
            ; orr w0, w0, EXIT_FLAGS
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
        if self.dirty {
            dynasm!(self.ops ; .arch aarch64 ; and w12, w28, 1);
        } else {
            self.field(Access::Ldr32, 12, layout::FLAGS);
            dynasm!(self.ops ; .arch aarch64 ; and w12, w12, 1);
        }
    }

    fn alu(&mut self, op: AluOp, size: u8, a: T, b: Src) {
        let need = ARITH & self.live_after;
        if need == 0 && !self.record_now && !matches!(op, AluOp::Adc | AluOp::Sbb) {
            // No flags wanted: the operation alone, on the temporary.
            if !op.writes() {
                return;
            }
            let a_ = r(a);
            match (op, b) {
                (AluOp::Add, Src::Imm(v)) if v < 4096 => dynasm!(self.ops ; .arch aarch64 ; add WSP(a_), WSP(a_), v),
                (AluOp::Sub, Src::Imm(v)) if v < 4096 => dynasm!(self.ops ; .arch aarch64 ; sub WSP(a_), WSP(a_), v),
                _ => {
                    let b_ = match b {
                        Src::T(b) => r(b),
                        Src::Imm(v) => {
                            self.mov32(11, v);
                            11
                        }
                    };
                    match op {
                        AluOp::Add => dynasm!(self.ops ; .arch aarch64 ; add W(a_), W(a_), W(b_)),
                        AluOp::Sub => dynasm!(self.ops ; .arch aarch64 ; sub W(a_), W(a_), W(b_)),
                        AluOp::And => dynasm!(self.ops ; .arch aarch64 ; and W(a_), W(a_), W(b_)),
                        AluOp::Or => dynasm!(self.ops ; .arch aarch64 ; orr W(a_), W(a_), W(b_)),
                        _ => dynasm!(self.ops ; .arch aarch64 ; eor W(a_), W(a_), W(b_)),
                    }
                }
            }
            // (Zero-extended, as `cut` leaves results.)
            match size {
                1 => dynasm!(self.ops ; .arch aarch64 ; and WSP(a_), W(a_), 0xFF),
                2 => dynasm!(self.ops ; .arch aarch64 ; and WSP(a_), W(a_), 0xFFFF),
                _ => {}
            }
            return;
        }
        self.operands(a, b);
        if self.record_now {
            let kind = Uop::Alu { op, size, a, b }.lazy_kind().expect("a recorded operation");
            self.record_lazy(kind, size);
        }
        match op {
            AluOp::Add => self.add_sub_flags(size, false, need),
            AluOp::Sub | AluOp::Cmp => self.add_sub_flags(size, true, need),
            AluOp::Adc => {
                if op == AluOp::Adc {
                    self.carry_in();
                }
                dynasm!(self.ops ; .arch aarch64 ; add x0, x10, x11);
                if op == AluOp::Adc {
                    dynasm!(self.ops ; .arch aarch64 ; add x0, x0, x12);
                }
                self.arith_flags(size, false, need);
            }
            AluOp::Sbb => {
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
            UnOp::Inc | UnOp::Dec if ARITH & !CF & self.live_after == 0 => {
                // No flags wanted: the operation alone, zero-extended.
                let t_ = r(t);
                if op == UnOp::Inc {
                    dynasm!(self.ops ; .arch aarch64 ; add WSP(t_), WSP(t_), 1);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; sub WSP(t_), WSP(t_), 1);
                }
                match size {
                    1 => dynasm!(self.ops ; .arch aarch64 ; and WSP(t_), W(t_), 0xFF),
                    2 => dynasm!(self.ops ; .arch aarch64 ; and WSP(t_), W(t_), 0xFFFF),
                    _ => {}
                }
                return;
            }
            UnOp::Inc | UnOp::Dec => {
                // As ADD or SUB 1, leaving CF.
                self.operands(t, Src::Imm(1));
                self.add_sub_flags(size, op == UnOp::Dec, ARITH & !CF & self.live_after);
            }
            UnOp::Neg => {
                // 0 - t.
                dynasm!(self.ops ; .arch aarch64 ; mov w11, W(r(t)) ; movz w10, 0);
                if self.record_now {
                    // NEG's operand is the first.
                    dynasm!(self.ops ; .arch aarch64 ; mov w10, w11);
                    self.record_lazy(super::flags::LAZY_NEG, size);
                    dynasm!(self.ops ; .arch aarch64 ; movz w10, 0);
                }
                self.add_sub_flags(size, true, ARITH & self.live_after);
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
            ShiftOp::Rcl | ShiftOp::Rcr => {
                // By 1 (`translate::rotate_carry`): CF in at the far end,
                // the bit out in CF.
                debug_assert!(count == Some(1));
                self.carry_in();
                if op == ShiftOp::Rcl {
                    // OF = the result's top bit ^ CF.
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; ubfx w1, w10, sign, 1
                        ; lsl w0, w10, 1
                        ; orr w0, w0, w12
                    );
                    self.cut(size);
                    dynasm!(self.ops ; .arch aarch64 ; ubfx w2, w0, sign, 1 ; eor w2, w2, w1);
                } else {
                    // OF = the result's top two bits differ.
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; and w1, w10, 1
                        ; lsr w0, w10, 1
                        ; orr w0, w0, w12, lsl sign
                        ; ubfx w2, w0, sign, 1
                        ; ubfx w3, w0, below, 1
                        ; eor w2, w2, w3
                    );
                }
                self.flags_w5(bits, need, CF | OF, 0);
            }
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
    /// quotient doesn't fit. The flags are a 486's (`division_flags`).
    fn div_wide(&mut self, signed: bool, size: u8, t: T) {
        let need = ARITH & self.live_after;
        self.divide(signed, size, t);
        if need != 0 {
            self.division_flags(size, need);
        }
    }

    /// The flags `need` of those DIV and IDIV leave (instructions/arith.rs
    /// `division_flags`), from the quotient and remainder they left: ZF
    /// where the remainder is 0 and the quotient odd, CF where the
    /// remainder's low two bits are 1 or 2, PF where the two have the same
    /// parity, and AF, SF and OF clear.
    fn division_flags(&mut self, size: u8, need: u32) {
        let (acc, high) = (gpr_offset(Gpr::dword(0)), gpr_offset(Gpr::dword(2)));
        match size {
            1 => {
                self.field(Access::Ldr16, 0, acc);
                dynasm!(self.ops ; .arch aarch64 ; lsr w1, w0, 8 ; and w0, w0, 0xFF);
            }
            2 => {
                self.field(Access::Ldr16, 0, acc);
                self.field(Access::Ldr16, 1, high);
            }
            _ => {
                self.field(Access::Ldr32, 0, acc);
                self.field(Access::Ldr32, 1, high);
            }
        }
        dynasm!(self.ops
            ; .arch aarch64
            // ZF: remainder 0 and quotient odd.
            ; cmp w1, 0
            ; cset w5, eq
            ; and w5, w5, w0
            ; and w5, w5, 1
            ; lsl w5, w5, 6
            // CF: (remainder & 3) - 1 below 2.
            ; and w6, w1, 3
            ; sub w6, w6, 1
            ; cmp w6, 2
            ; cset w6, lo
            ; orr w5, w5, w6
            // PF: the parity of remainder ^ quotient, folded to a byte.
            ; eor w6, w0, w1
        );
        if size == 4 {
            dynasm!(self.ops ; .arch aarch64 ; eor w6, w6, w6, lsr 16);
        }
        if size >= 2 {
            dynasm!(self.ops ; .arch aarch64 ; eor w6, w6, w6, lsr 8);
        }
        dynasm!(self.ops
            ; .arch aarch64
            ; and w6, w6, 0xFF
            ; add x6, x20, x6
            ; ldrb w6, [x6, CTX_PARITY as u32]
            ; orr w5, w5, w6
        );
        self.merge(need);
    }

    fn divide(&mut self, signed: bool, size: u8, t: T) {
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
        // A jump the block goes on after leaves where taken, after the
        // block's code, which puts the flags back there. Else they go back
        // into the CPU for both ways out; the condition reads them where
        // they were.
        let in_w28 = self.dirty;
        let side = self.ix + 1 < self.data.count();
        if let Some(to) = self.data.target(self.ix) {
            self.merges.push(Merge {
                at: yes,
                ix: self.ix,
                to,
                commit,
                dirty: self.dirty,
                synced: self.synced[self.ix],
                fpu_cr0_checked: self.fpu_cr0_checked,
                fpu_known: self.fpu_known,
                entry: None,
            });
        } else if side {
            self.sides.push(Side { at: yes, ix: self.ix, taken, commit, dirty: self.dirty, loaded_segs: self.loaded_segs });
        } else {
            self.flags_back();
            self.dirty = false;
        }
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
        if side {
            return;
        }
        self.leave(Some(next), 1, true);
        dynasm!(self.ops ; .arch aarch64 ; =>yes);
        self.taken(taken, commit, 0);
    }

    /// Where instruction `ix` starts, which jumps in the block may go to:
    /// note how the code has the counts (`synced`) and the flags there for
    /// them, and know only what holds both ways.
    fn merge_here(&mut self, synced: i32) {
        let ix = self.ix;
        if !self.merges.iter().any(|m| m.to == ix) {
            return;
        }
        let entry = self.ops.new_dynamic_label();
        dynasm!(self.ops ; .arch aarch64 ; =>entry);
        for m in self.merges.iter_mut().filter(|m| m.to == ix) {
            m.entry = Some((entry, synced, self.dirty));
            self.fpu_cr0_checked &= m.fpu_cr0_checked;
            self.fpu_known &= m.fpu_known;
        }
    }

    /// A taken jump to an instruction later in the block (see `Merge`).
    fn jump_in(&mut self, m: Merge) {
        let (entry, synced, fall_dirty) = m.entry.expect("jump target translated");
        dynasm!(self.ops ; .arch aarch64 ; =>m.at);
        (self.ix, self.dirty) = (m.ix, m.dirty);
        self.commit(m.commit);
        let skipped = (m.to - m.ix - 1) as i32;
        let behind = synced - m.synced - skipped;
        if behind > 0 {
            self.add_field64(layout::ICOUNT, behind as u32);
        } else if behind < 0 {
            self.sub_field64(layout::ICOUNT, -behind as u32);
        }
        self.sub_field64(layout::EXECUTED, skipped as u32);
        match (self.dirty, fall_dirty) {
            (true, false) => self.flags_back(),
            (false, true) => self.field(Access::Ldr32, 28, layout::FLAGS),
            _ => {}
        }
        dynasm!(self.ops ; .arch aarch64 ; b =>entry);
    }

    /// Leave for the target `taken` of a conditional jump through link
    /// `slot`, after `commit`: the target must be within the CS limit.
    fn taken(&mut self, taken: u32, commit: Option<(Gpr, T)>, slot: usize) {
        let gp = self.fault_exit(EXIT_GP0);
        self.mov32(0, taken);
        self.field(Access::Ldr32, 1, seg_field(Seg::CS, layout::SEG_LIMIT));
        dynasm!(self.ops ; .arch aarch64 ; cmp w0, w1 ; b.hi =>gp);
        self.commit(commit);
        self.leave(Some(taken), slot, true);
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
        if let (true, Some(eip)) = (self.link, eip) {
            self.mov32(r(T0), eip);
            self.check_flat(T0);
        }
        match eip {
            Some(eip) if self.link && self.data.in_page(eip) => self.link_jump(slot),
            Some(_) if self.link => self.guarded(slot),
            _ => {
                dynasm!(self.ops ; .arch aarch64 ; movz w0, EXIT_NEXT);
                self.exit();
            }
        }
    }

    /// After a segment load in the block, where the segments aren't flat
    /// as the block's environment has them, the links a return takes to
    /// EIP `t` in the mode they are in now (see `x64::Gen::check_flat`).
    /// X1 is the block.
    fn check_flat(&mut self, t: T) {
        if self.loaded_segs == 0 {
            return;
        }
        let same = self.ops.new_dynamic_label();
        self.mov32(2, self.env.bits & super::ENV_FLAT_ALL);
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr w0, [x20, CTX_FLAT as u32]
            ; cmp w0, w2
            ; b.eq =>same
            ; ldr w3, [x20, CTX_MODE as u32]
        );
        self.mov32(4, !super::ENV_FLAT_ALL);
        dynasm!(self.ops
            ; .arch aarch64
            ; and w3, w3, w4
            ; orr w3, w3, w0
            ; str w3, [x20, CTX_MODE as u32]
        );
        self.returned(t, true);
        dynasm!(self.ops ; .arch aarch64 ; =>same);
    }

    /// Bring the counts up to date for leaving the block after the
    /// instruction being translated (the last but at a conditional jump the
    /// block goes on after), and X1 = the block.
    fn counts(&mut self) {
        let (n, synced) = (self.ix as i32 + 1, self.synced[self.ix]);
        self.add_field64(layout::ICOUNT, (n - synced) as u32);
        self.add_field64(layout::EXECUTED, n as u32);
        self.data_x1();
    }

    /// After a far transfer its handler ran (`block::far_transfer`), where
    /// it went: through the links a return takes. (`jit_fallback` stopped
    /// the block where the code can't go on.)
    fn far_returned(&mut self) {
        self.field(Access::Ldr32, r(T0), layout::EIP);
        self.counts();
        self.returned(T0, true);
    }

    /// Leave through the return (or indirect call) link to EIP `t`, if
    /// there is one (see `guarded`), else to the execution loop, to be
    /// linked. X1 is the block. After a far transfer (`far`) the blocks
    /// may be another mode's, which the context has (see
    /// `x64::Gen::returned`).
    fn returned(&mut self, t: T, far: bool) {
        let miss = *self.return_miss.get_or_insert_with(|| self.ops.new_dynamic_label());
        for slot in RETURN_LINK..LINKS {
            let next = self.ops.new_dynamic_label();
            let g_eip = DATA_GUARDS as u32 + slot as u32 * GUARD_SIZE as u32 + GUARD_EIP as u32;
            dynasm!(self.ops ; .arch aarch64 ; ldr w2, [x1, g_eip] ; cmp W(r(t)), w2 ; b.ne =>next);
            if far {
                let g_mode = DATA_GUARDS as u32 + slot as u32 * GUARD_SIZE as u32 + GUARD_MODE as u32;
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr w2, [x20, CTX_MODE as u32]
                    ; ldr w3, [x1, g_mode]
                    ; cmp w2, w3
                    ; b.ne =>next
                );
            }
            self.guarded(slot);
            dynasm!(self.ops ; .arch aarch64 ; =>next);
        }
        // The engine's place for the EIP (see `Return`), made in this
        // block's mode, with its guard checked as a link's (see
        // `x64::Gen::returned`: the mode has the A20 gate, paging and
        // CPL the guard was made under).
        let g = RETURN_GUARD as u32;
        dynasm!(self.ops
            ; .arch aarch64
            ; and w2, W(r(t)), (1 << RETURN_BITS) - 1
            ; add w2, w2, w2, lsl 2
            ; ldr x3, [x20, CTX_RETURNS as u32]
            ; add x2, x3, x2, lsl 3
            ; ldr w3, [x2, g + GUARD_EIP as u32]
            ; cmp W(r(t)), w3
            ; b.ne =>miss
            ; ldr w3, [x2, RETURN_MODE as u32]
        );
        if far {
            dynasm!(self.ops ; .arch aarch64 ; ldr w4, [x20, CTX_MODE as u32]);
        } else {
            self.mov32(4, self.env.bits);
        }
        dynasm!(self.ops ; .arch aarch64 ; cmp w3, w4 ; b.ne =>miss);
        self.field(Access::Ldr32, 3, seg_field(Seg::CS, layout::SEG_BASE));
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr w4, [x2, g + GUARD_CS_BASE as u32]
            ; cmp w3, w4
            ; b.ne =>miss
        );
        if self.env.bits & super::ENV_PAGING != 0 {
            // The TLB entry of the page in the set of the privilege level.
            let set = if self.env.bits & super::ENV_USER != 0 { layout::TLB_SET as u32 } else { 0 };
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr w4, [x2, g + GUARD_PAGE as u32]
                ; and w3, w4, (layout::TLB_SET - 1) as u32
            );
            if set != 0 {
                dynasm!(self.ops ; .arch aarch64 ; add w3, w3, set);
            }
            self.mov32(5, layout::TLB_ENTRY_SIZE as u32);
            dynasm!(self.ops
                ; .arch aarch64
                ; umull x3, w3, w5
                ; ldr x6, [x20, CTX_TLB as u32]
                ; add x6, x6, x3
                ; add w4, w4, 1
                ; ldr w5, [x6, layout::TLB_READ_TAG as u32]
                ; cmp w4, w5
                ; b.ne =>miss
                ; ldr w5, [x6, layout::TLB_PHYS as u32]
                ; ldr w3, [x2, g + GUARD_PHYS as u32]
                ; cmp w5, w3
                ; b.ne =>miss
            );
        }
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x16, [x2, RETURN_CODE as u32]
            ; br x16
        );
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
        );
        let (fetch, back) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        self.slow.push(Slow::Fetch { at: fetch, back, stub, slot, g });
        dynasm!(self.ops
            ; .arch aarch64
            ; =>back
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
        dynasm!(self.ops
            ; .arch aarch64
            ; umull x3, w3, w5
            ; ldr x6, [x20, CTX_TLB as u32]
            ; add x6, x6, x3
            ; add w4, w4, 1
            ; ldr w5, [x6, layout::TLB_READ_TAG as u32]
            ; cmp w4, w5
            ; b.ne =>fetch
            ; ldr w5, [x6, layout::TLB_PHYS as u32]
            ; ldr w3, [x1, g_phys]
            ; cmp w5, w3
            ; b.ne =>stub
            ; go:
        );
        self.link_jump(slot);
    }

    /// The jump of link `slot`: to its stub, until the engine makes it a
    /// link (`patch_link`). A return's links are made again for each place
    /// it goes to in turn: they jump where the block's `links` says (X1 is
    /// the block), as rewriting code that just ran is slow.
    fn link_jump(&mut self, slot: usize) {
        let stub = *self.stubs[slot].get_or_insert_with(|| self.ops.new_dynamic_label());
        if slot >= RETURN_LINK {
            let at = DATA_LINKS as u32 + slot as u32 * 8;
            dynasm!(self.ops ; .arch aarch64 ; ldr x16, [x1, at] ; br x16);
            return;
        }
        dynasm!(self.ops ; .arch aarch64 ; b =>stub);
        self.sites.push((slot as u8, self.ops.offset().0 as u32 - 4));
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
        // One flag: tested where the flags are.
        let one = |cc| match cc {
            C::o | C::no => Some(OF),
            C::b | C::ae => Some(CF),
            C::e | C::ne => Some(ZF),
            C::s | C::ns => Some(SF),
            C::p | C::np => Some(PF),
            _ => None,
        };
        if let Some(flag) = one(cc) {
            let src = if in_w28 {
                28
            } else {
                self.load_flags_w0(false);
                0
            };
            match flag {
                OF => dynasm!(self.ops ; .arch aarch64 ; tst W(src), OF),
                CF => dynasm!(self.ops ; .arch aarch64 ; tst W(src), CF),
                ZF => dynasm!(self.ops ; .arch aarch64 ; tst W(src), ZF),
                SF => dynasm!(self.ops ; .arch aarch64 ; tst W(src), SF),
                _ => dynasm!(self.ops ; .arch aarch64 ; tst W(src), PF),
            }
            return matches!(cc, C::o | C::b | C::e | C::s | C::p);
        }
        self.load_flags_w0(in_w28);
        match cc {
            C::be | C::a => {
                self.mov32(1, super::flags::cond_flags(cc));
                dynasm!(self.ops ; .arch aarch64 ; tst w0, w1);
                cc == C::be
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
