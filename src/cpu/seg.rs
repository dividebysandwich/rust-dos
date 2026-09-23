//! Protected-mode segmentation: selectors, descriptors in the GDT and LDT,
//! and loading segment registers with their protection checks.

use super::fault::{CpuResult, Fault};
use super::{Cpu, CpuFlags, Seg, SegCache};

/// System descriptor types (S = 0).
pub const TSS16_AVAILABLE: u8 = 0x1;
pub const LDT: u8 = 0x2;
pub const TSS16_BUSY: u8 = 0x3;
pub const CALL_GATE16: u8 = 0x4;
pub const TASK_GATE: u8 = 0x5;
pub const INT_GATE16: u8 = 0x6;
pub const TRAP_GATE16: u8 = 0x7;
pub const TSS32_AVAILABLE: u8 = 0x9;
pub const TSS32_BUSY: u8 = 0xB;
pub const CALL_GATE32: u8 = 0xC;
pub const INT_GATE32: u8 = 0xE;
pub const TRAP_GATE32: u8 = 0xF;

/// The selector with its RPL cleared, as exceptions report it.
#[inline(always)]
pub fn sel_error(selector: u16) -> u32 {
    (selector & 0xFFFC) as u32
}

/// True for a null selector (index 0 in the GDT), whatever its RPL.
#[inline(always)]
pub fn is_null(selector: u16) -> bool {
    selector & 0xFFFC == 0
}

/// Requested privilege level of a selector.
#[inline(always)]
pub fn rpl(selector: u16) -> u8 {
    (selector & 3) as u8
}

/// An 8-byte segment or gate descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Descriptor(pub u64);

impl Descriptor {
    pub fn base(&self) -> u32 {
        let d = self.0;
        (((d >> 16) & 0x00FF_FFFF) | ((d >> 32) & 0xFF00_0000)) as u32
    }

    /// The limit in bytes: the 20-bit limit, in 4 KB units when G is set.
    pub fn limit(&self) -> u32 {
        let d = self.0;
        let raw = ((d & 0xFFFF) | ((d >> 32) & 0x000F_0000)) as u32;
        if self.attr() & super::ATTR_G != 0 { (raw << 12) | 0xFFF } else { raw }
    }

    /// Access byte and flags, laid out as in `SegCache::attr`.
    pub fn attr(&self) -> u16 {
        let d = self.0;
        (((d >> 40) & 0xFF) | ((d >> 40) & 0xF000)) as u16
    }

    /// The type field (bits 0-3 of the access byte).
    pub fn typ(&self) -> u8 {
        ((self.0 >> 40) & 0xF) as u8
    }

    /// A code or data segment (S = 1), not a system descriptor.
    pub fn is_segment(&self) -> bool {
        self.0 & (1 << 44) != 0
    }

    pub fn is_code(&self) -> bool {
        self.is_segment() && self.typ() & 0x8 != 0
    }

    pub fn is_data(&self) -> bool {
        self.is_segment() && self.typ() & 0x8 == 0
    }

    pub fn conforming(&self) -> bool {
        self.is_code() && self.typ() & 0x4 != 0
    }

    /// Readable code, or any data segment.
    pub fn readable(&self) -> bool {
        self.is_data() || self.typ() & 0x2 != 0
    }

    pub fn writable_data(&self) -> bool {
        self.is_data() && self.typ() & 0x2 != 0
    }

    pub fn dpl(&self) -> u8 {
        ((self.0 >> 45) & 3) as u8
    }

    pub fn present(&self) -> bool {
        self.0 & (1 << 47) != 0
    }

    /// Descriptor with the accessed bit (type bit 0) set.
    pub fn accessed(&self) -> bool {
        self.0 & (1 << 40) != 0
    }

    /// The 32-bit form of a system descriptor (TSS, gates).
    pub fn is_32bit_system(&self) -> bool {
        self.typ() & 0x8 != 0
    }

    /// Target selector of a gate.
    pub fn gate_selector(&self) -> u16 {
        (self.0 >> 16) as u16
    }

    /// Target offset of a gate: 16 bits, or 32 for a 32-bit gate.
    pub fn gate_offset(&self) -> u32 {
        let low = (self.0 & 0xFFFF) as u32;
        if self.is_32bit_system() { low | ((self.0 >> 32) as u32 & 0xFFFF_0000) } else { low }
    }

    /// Number of stack parameters a call gate copies.
    pub fn gate_params(&self) -> u32 {
        ((self.0 >> 32) & 0x1F) as u32
    }

    /// The segment register cache for this descriptor loaded with
    /// `selector`.
    pub fn cache(&self, selector: u16) -> SegCache {
        SegCache::from_descriptor(selector, self.base(), self.limit(), self.attr())
    }
}

impl Cpu {
    /// True in protected mode (CR0.PE), including virtual-8086 mode.
    #[inline(always)]
    pub fn pe(&self) -> bool {
        self.cr0 & super::CR0_PE != 0
    }

    /// True in virtual-8086 mode.
    #[inline(always)]
    pub fn v86(&self) -> bool {
        self.flags.contains(CpuFlags::VM)
    }

    /// Protected mode, not virtual-8086 mode: where segment registers hold
    /// selectors.
    #[inline(always)]
    pub fn pm(&self) -> bool {
        self.pe() && !self.v86()
    }

    /// Linear address of a descriptor, or the fault a selector beyond its
    /// table's limit raises (`#GP(selector)` unless the caller maps it).
    fn descriptor_address(&self, selector: u16) -> Result<u32, ()> {
        let index = (selector & 0xFFF8) as u32;
        let (base, limit) = if selector & 4 != 0 {
            // No LDT loaded (a null LDTR): every LDT selector is invalid.
            if self.ldtr.attr & 0x80 == 0 {
                return Err(());
            }
            (self.ldtr.base, self.ldtr.limit)
        } else {
            (self.gdtr.base, self.gdtr.limit as u32)
        };
        if index + 7 > limit {
            return Err(());
        }
        Ok(base.wrapping_add(index))
    }

    /// Read the descriptor a selector names. A selector past the end of its
    /// table raises `#GP(selector)` with `ext` added to the error code.
    pub fn fetch_descriptor(&mut self, selector: u16, ext: u32) -> CpuResult<Descriptor> {
        let addr = self
            .descriptor_address(selector)
            .map_err(|_| Fault::gp(sel_error(selector) | ext))?;
        let low = self.sys_read_u32(addr)? as u64;
        let high = self.sys_read_u32(addr.wrapping_add(4))? as u64;
        Ok(Descriptor(low | (high << 32)))
    }

    /// Set a descriptor's accessed bit, as loading a segment does.
    pub fn mark_accessed(&mut self, selector: u16, desc: &mut Descriptor) -> CpuResult {
        if desc.accessed() {
            return Ok(());
        }
        let addr = self
            .descriptor_address(selector)
            .map_err(|_| Fault::gp(sel_error(selector)))?;
        desc.0 |= 1 << 40;
        self.sys_write_u8(addr.wrapping_add(5), (desc.0 >> 40) as u8)
    }

    /// Load a data or stack segment register (DS, ES, FS, GS or SS) by MOV,
    /// POP or LDS and friends, with the checks of the current mode.
    pub fn load_segment(&mut self, seg: Seg, selector: u16) -> CpuResult {
        debug_assert!(seg != Seg::CS);
        if !self.pe() {
            self.load_seg_real(seg, selector);
        } else if self.v86() {
            self.load_seg_v86(seg, selector);
        } else {
            let cache = self.check_data_segment(seg, selector)?;
            self.set_seg_cache(seg, cache);
        }
        if seg == Seg::SS {
            // Interrupts wait for the instruction after a stack switch.
            self.irq_shadow = true;
        }
        Ok(())
    }

    /// Protected mode: check a selector for `seg` and return the cache to
    /// load, marking the descriptor accessed.
    pub fn check_data_segment(&mut self, seg: Seg, selector: u16) -> CpuResult<SegCache> {
        let cpl = self.cpl;
        if is_null(selector) {
            if seg == Seg::SS {
                return Err(Fault::gp(0));
            }
            return Ok(SegCache::null(selector));
        }
        let mut desc = self.fetch_descriptor(selector, 0)?;
        let err = sel_error(selector);
        if seg == Seg::SS {
            if rpl(selector) != cpl || !desc.writable_data() || desc.dpl() != cpl {
                return Err(Fault::gp(err));
            }
            if !desc.present() {
                return Err(Fault::ss(err));
            }
        } else {
            if !desc.readable() {
                return Err(Fault::gp(err));
            }
            // Data and non-conforming code need DPL >= max(CPL, RPL).
            if !desc.conforming() && (desc.dpl() < cpl || desc.dpl() < rpl(selector)) {
                return Err(Fault::gp(err));
            }
            if !desc.present() {
                return Err(Fault::np(err));
            }
        }
        self.mark_accessed(selector, &mut desc)?;
        Ok(desc.cache(selector))
    }

    /// Load CS for a transfer to a code segment in protected mode, at
    /// privilege level `cpl` (the selector's RPL becomes the CPL).
    pub fn load_cs_pm(&mut self, selector: u16, desc: &mut Descriptor, cpl: u8) -> CpuResult {
        self.mark_accessed(selector, desc)?;
        let selector = (selector & 0xFFFC) | cpl as u16;
        self.set_seg_cache(Seg::CS, desc.cache(selector));
        self.cpl = cpl;
        Ok(())
    }

    /// After a return to an outer privilege level: data segment registers
    /// the new level may not use are loaded with the null selector.
    pub fn null_inaccessible_segments(&mut self) {
        for seg in [Seg::ES, Seg::DS, Seg::FS, Seg::GS] {
            let cache = *self.seg_cache(seg);
            let conforming_code = cache.attr & 0x1C == 0x1C;
            if !conforming_code && cache.dpl() < self.cpl {
                self.set_seg_cache(seg, SegCache::null(0));
            }
        }
    }
}
