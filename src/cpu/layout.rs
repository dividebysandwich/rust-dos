//! Where the dynamic recompiler's generated code finds the CPU's state:
//! byte offsets into `Cpu` (whose registers are private to this module).

use std::mem::{offset_of, size_of};

use super::{Cpu, CpuFlags, SegCache};

/// EAX..EDI, 4 bytes each in encoding order.
pub const GPR: usize = offset_of!(Cpu, gpr);
pub const EIP: usize = offset_of!(Cpu, eip);
/// EFLAGS, a u32 (`CpuFlags` is transparent).
pub const FLAGS: usize = offset_of!(Cpu, flags);
/// The segment registers' caches, ES, CS, SS, DS, FS, GS.
pub const SEG: usize = offset_of!(Cpu, seg);
pub const SEG_SIZE: usize = size_of::<SegCache>();
pub const SEG_SELECTOR: usize = offset_of!(SegCache, selector);
pub const SEG_BASE: usize = offset_of!(SegCache, base);
pub const SEG_LIMIT: usize = offset_of!(SegCache, limit);
pub const SEG_LO: usize = offset_of!(SegCache, lo);
pub const SEG_HI: usize = offset_of!(SegCache, hi);
pub const SEG_RIGHTS: usize = offset_of!(SegCache, rights);
pub const SEG_ATTR: usize = offset_of!(SegCache, attr);
/// The access byte of a real-mode segment (`Cpu::load_seg_real`).
#[cfg_attr(not(dynrec), allow(dead_code))]
pub const AR_DATA_RW: u16 = super::regs::AR_DATA_RW;
/// `SegCache::rights` bits.
pub const RIGHT_READ: u8 = super::regs::RIGHT_READ;
pub const RIGHT_WRITE: u8 = super::regs::RIGHT_WRITE;
pub const CR0: usize = offset_of!(Cpu, cr0);
pub const CPL: usize = offset_of!(Cpu, cpl);
pub const EXECUTED: usize = offset_of!(Cpu, executed);
pub const ICOUNT: usize = offset_of!(Cpu, bus.clock.icount);
pub const DEADLINE: usize = offset_of!(Cpu, bus.clock.deadline);
pub const A20_MASK: usize = offset_of!(Cpu, bus) + crate::bus::A20_MASK_OFFSET;
/// `Bus::irq_ready` (a bool) and `Cpu::irq_shadow`.
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub const IRQ_READY: usize = offset_of!(Cpu, bus) + crate::bus::IRQ_READY_OFFSET;
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub const IRQ_SHADOW: usize = offset_of!(Cpu, irq_shadow);
/// The counts of `Activity::video_write`, and the gap after which a write
/// starts a burst of its own (which translated code leaves to it).
#[cfg_attr(not(dynrec), allow(dead_code))]
pub const VIDEO_BYTES: usize = offset_of!(Cpu, bus.activity) + crate::autospeed::VIDEO_BYTES_AT;
#[cfg_attr(not(dynrec), allow(dead_code))]
pub const BURST: usize = offset_of!(Cpu, bus.activity) + crate::autospeed::BURST_AT;
#[cfg_attr(not(dynrec), allow(dead_code))]
pub const LAST_WRITE: usize = offset_of!(Cpu, bus.activity) + crate::autospeed::LAST_WRITE_AT;
#[cfg_attr(not(dynrec), allow(dead_code))]
pub const BURST_GAP: u64 = crate::autospeed::BURST_GAP;
/// A TLB entry (`Tlb::entries_ptr`): its tags and physical page, its size,
/// and the entries of each set (supervisor, then user), which a page
/// number modulo it indexes.
pub const TLB_READ_TAG: usize = super::paging::TLB_READ_TAG;
pub const TLB_WRITE_TAG: usize = super::paging::TLB_WRITE_TAG;
pub const TLB_PHYS: usize = super::paging::TLB_PHYS;
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub const TLB_JIT_READ: usize = super::paging::TLB_JIT_READ;
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub const TLB_JIT_WRITE: usize = super::paging::TLB_JIT_WRITE;
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub const TLB_JIT_DELTA: usize = super::paging::TLB_JIT_DELTA;
pub const TLB_ENTRY_SIZE: usize = super::paging::TLB_ENTRY_SIZE;
pub const TLB_SET: usize = super::paging::TLB_SET;
/// The TLB's entries, supervisor then user (`Tlb::entries_ptr`).
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub const TLB: usize = offset_of!(Cpu, tlb) + super::paging::TLB_ENTRIES_AT;

/// The FPU: the stack top (a usize, 0 to 7), the tags (a byte each, by
/// physical register), the status flags and the control word (u16s), and
/// the registers' doubles, 80 bits (16 bytes each) and which of those are
/// stale (a byte each), see `f80::FpuRegs`.
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub mod fpu {
    use super::super::{Cpu, FpuFlags};
    use crate::f80::FpuRegs;
    use std::mem::{offset_of, size_of};

    pub const TOP: usize = offset_of!(Cpu, fpu_top);
    pub const TAGS: usize = offset_of!(Cpu, fpu_tags);
    pub const FLAGS: usize = offset_of!(Cpu, fpu_flags);
    pub const CONTROL: usize = offset_of!(Cpu, fpu_control);
    pub const F64: usize = offset_of!(Cpu, fpu_stack) + FpuRegs::F64_OFFSET;
    pub const X80: usize = offset_of!(Cpu, fpu_stack) + FpuRegs::X80_OFFSET;
    pub const STALE: usize = offset_of!(Cpu, fpu_stack) + FpuRegs::STALE_OFFSET;
    const _: () = assert!(size_of::<FpuFlags>() == 2 && size_of::<crate::f80::F80>() == 16);
}

const _: () = assert!(size_of::<CpuFlags>() == 4);
