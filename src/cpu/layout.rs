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
/// `SegCache::rights` bits.
pub const RIGHT_READ: u8 = super::regs::RIGHT_READ;
pub const RIGHT_WRITE: u8 = super::regs::RIGHT_WRITE;
pub const CR0: usize = offset_of!(Cpu, cr0);
pub const CPL: usize = offset_of!(Cpu, cpl);
pub const EXECUTED: usize = offset_of!(Cpu, executed);
pub const ICOUNT: usize = offset_of!(Cpu, bus.clock.icount);
pub const DEADLINE: usize = offset_of!(Cpu, bus.clock.deadline);
pub const A20_MASK: usize = offset_of!(Cpu, bus) + crate::bus::A20_MASK_OFFSET;
/// A TLB entry (`Tlb::entries_ptr`): its tags and physical page, its size,
/// and the entries of each set (supervisor, then user), which a page
/// number modulo it indexes.
pub const TLB_READ_TAG: usize = super::paging::TLB_READ_TAG;
pub const TLB_WRITE_TAG: usize = super::paging::TLB_WRITE_TAG;
pub const TLB_PHYS: usize = super::paging::TLB_PHYS;
pub const TLB_ENTRY_SIZE: usize = super::paging::TLB_ENTRY_SIZE;
pub const TLB_SET: usize = super::paging::TLB_SET;

const _: () = assert!(size_of::<CpuFlags>() == 4);
