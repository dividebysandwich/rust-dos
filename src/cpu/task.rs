//! Hardware task switching: JMP and CALL to a TSS or task gate, interrupts
//! through a task gate, and IRET with NT set back to the calling task.
//! Both TSS formats are supported: the 286's 16-bit one and the 386's.

use super::fault::{CpuResult, Fault};
use super::seg::{
    Descriptor, LDT, TASK_GATE, TSS16_AVAILABLE, TSS16_BUSY, TSS32_AVAILABLE, TSS32_BUSY, is_null, rpl,
    sel_error,
};
use super::{CR0_TS, Cpu, CpuFlags, Seg, SegCache};

/// What started a task switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    Jmp,
    Call,
    /// IRET with NT set: back to the task in the back link.
    Iret,
    /// An interrupt or exception through a task gate, with the error code
    /// it pushes on the new task's stack.
    Interrupt(Option<u32>),
}

/// Offsets of the registers in a 32-bit TSS.
const TSS32_CR3: u32 = 0x1C;
const TSS32_EIP: u32 = 0x20;
const TSS32_EFLAGS: u32 = 0x24;
const TSS32_GPR: u32 = 0x28;
const TSS32_SEG: u32 = 0x48;
const TSS32_LDT: u32 = 0x60;
/// Offsets in a 16-bit TSS.
const TSS16_IP: u32 = 0x0E;
const TSS16_FLAGS: u32 = 0x10;
const TSS16_GPR: u32 = 0x12;
const TSS16_SEG: u32 = 0x22;
const TSS16_LDT: u32 = 0x2A;

impl Cpu {
    /// Switch tasks through a task gate (JMP, CALL or an interrupt). The
    /// caller has checked the gate itself.
    pub fn task_gate(&mut self, tss_selector: u16, switch: Switch, ext: u32) -> CpuResult {
        let err = sel_error(tss_selector) | ext;
        if tss_selector & 4 != 0 {
            return Err(Fault::gp(err));
        }
        let desc = self.fetch_descriptor(tss_selector, ext)?;
        if desc.is_segment() || !matches!(desc.typ(), TSS16_AVAILABLE | TSS32_AVAILABLE) {
            return Err(Fault::gp(err));
        }
        if !desc.present() {
            return Err(Fault::np(err));
        }
        self.task_switch(tss_selector, desc, switch, ext)
    }

    /// IRET with NT set: return to the task whose TSS selector is in the
    /// current TSS's back link.
    pub fn task_return(&mut self) -> CpuResult {
        let back = self.sys_read(self.tr.base, 2)? as u16;
        let err = sel_error(back);
        if back & 4 != 0 || is_null(back) {
            return Err(Fault::ts(err));
        }
        let desc = self.fetch_descriptor(back, 0).map_err(|_| Fault::ts(err))?;
        if desc.is_segment() || !matches!(desc.typ(), TSS16_BUSY | TSS32_BUSY) {
            return Err(Fault::ts(err));
        }
        if !desc.present() {
            return Err(Fault::np(err));
        }
        self.task_switch(back, desc, Switch::Iret, 0)
    }

    /// Set or clear the busy bit of the TSS descriptor `selector` names.
    fn set_tss_busy(&mut self, selector: u16, busy: bool) -> CpuResult {
        let addr = self.gdtr.base.wrapping_add((selector & 0xFFF8) as u32).wrapping_add(5);
        let access = self.sys_read(addr, 1)? as u8;
        let access = if busy { access | 0x02 } else { access & !0x02 };
        self.sys_write_u8(addr, access)
    }

    /// Save the current task in its TSS and load the task of `new_desc`.
    pub fn task_switch(&mut self, new_sel: u16, new_desc: Descriptor, switch: Switch, ext: u32) -> CpuResult {
        let err = sel_error(new_sel) | ext;
        let new32 = new_desc.is_32bit_system();
        let min_limit = if new32 { 0x67 } else { 0x2B };
        if new_desc.limit() < min_limit {
            return Err(Fault::ts(err));
        }
        let old = self.tr;
        let old32 = old.attr & 0x08 != 0;
        let new_base = new_desc.base();

        // Read the new task's state first, so a fault leaves the old task
        // untouched.
        let mut regs = [0u32; 8];
        let mut sels = [0u16; 6];
        let (cr3, eip, eflags, ldt);
        if new32 {
            cr3 = Some(self.sys_read(new_base.wrapping_add(TSS32_CR3), 4)?);
            eip = self.sys_read(new_base.wrapping_add(TSS32_EIP), 4)?;
            eflags = self.sys_read(new_base.wrapping_add(TSS32_EFLAGS), 4)?;
            for (i, r) in regs.iter_mut().enumerate() {
                *r = self.sys_read(new_base.wrapping_add(TSS32_GPR + 4 * i as u32), 4)?;
            }
            for (i, s) in sels.iter_mut().enumerate() {
                *s = self.sys_read(new_base.wrapping_add(TSS32_SEG + 4 * i as u32), 2)? as u16;
            }
            ldt = self.sys_read(new_base.wrapping_add(TSS32_LDT), 2)? as u16;
        } else {
            cr3 = None;
            eip = self.sys_read(new_base.wrapping_add(TSS16_IP), 2)?;
            eflags = self.sys_read(new_base.wrapping_add(TSS16_FLAGS), 2)? | (self.flags.bits() & 0xFFFF_0000 & !CpuFlags::VM.bits());
            for (i, r) in regs.iter_mut().enumerate() {
                *r = 0xFFFF_0000 | self.sys_read(new_base.wrapping_add(TSS16_GPR + 2 * i as u32), 2)?;
            }
            // ES, CS, SS, DS; a 286 TSS has no FS and GS.
            for (i, s) in sels.iter_mut().take(4).enumerate() {
                *s = self.sys_read(new_base.wrapping_add(TSS16_SEG + 2 * i as u32), 2)? as u16;
            }
            ldt = self.sys_read(new_base.wrapping_add(TSS16_LDT), 2)? as u16;
        }

        // Save the old task. An IRET leaves it with NT clear.
        let mut old_flags = self.flags.bits();
        if switch == Switch::Iret {
            old_flags &= !CpuFlags::NT.bits();
        }
        if old32 {
            self.sys_write(old.base.wrapping_add(TSS32_EIP), 4, self.eip)?;
            self.sys_write(old.base.wrapping_add(TSS32_EFLAGS), 4, old_flags)?;
            for i in 0..8 {
                let v = self.gpr[i];
                self.sys_write(old.base.wrapping_add(TSS32_GPR + 4 * i as u32), 4, v)?;
            }
            for (i, seg) in Seg::ALL.iter().enumerate() {
                let v = self.seg_cache(*seg).selector as u32;
                self.sys_write(old.base.wrapping_add(TSS32_SEG + 4 * i as u32), 2, v)?;
            }
        } else if old.attr & 0x80 != 0 {
            self.sys_write(old.base.wrapping_add(TSS16_IP), 2, self.eip)?;
            self.sys_write(old.base.wrapping_add(TSS16_FLAGS), 2, old_flags)?;
            for i in 0..8 {
                let v = self.gpr[i];
                self.sys_write(old.base.wrapping_add(TSS16_GPR + 2 * i as u32), 2, v)?;
            }
            for (i, seg) in Seg::ALL.iter().take(4).enumerate() {
                let v = self.seg_cache(*seg).selector as u32;
                self.sys_write(old.base.wrapping_add(TSS16_SEG + 2 * i as u32), 2, v)?;
            }
        }

        let mut new_flags = eflags;
        match switch {
            Switch::Jmp | Switch::Iret => {
                if old.attr & 0x80 != 0 {
                    self.set_tss_busy(old.selector, false)?;
                }
            }
            Switch::Call | Switch::Interrupt(_) => {
                // The new task links back to this one.
                self.sys_write(new_base, 2, old.selector as u32)?;
                new_flags |= CpuFlags::NT.bits();
            }
        }
        if switch != Switch::Iret {
            self.set_tss_busy(new_sel, true)?;
        }

        // From here on the new task runs: faults are its own.
        let mut desc = new_desc;
        desc.0 |= 2 << 40; // busy
        self.tr = desc.cache(new_sel);
        self.cr0 |= CR0_TS;
        if let Some(cr3) = cr3
            && self.cr0 & super::CR0_PG != 0
        {
            self.cr3 = cr3;
            self.tlb.flush();
        }
        self.eip = eip;
        self.flags = CpuFlags::from_bits_retain((new_flags & self.eflags_mask()) | 0x0002);
        self.gpr = regs;
        self.cpl = rpl(sels[Seg::CS as usize]);
        self.load_task_segments(ldt, &sels, ext)?;

        if let Switch::Interrupt(Some(code)) = switch {
            let size = if new32 { 4 } else { 2 };
            self.push_sized(size, code)?;
        }
        Ok(())
    }

    /// EFLAGS bits a task switch loads.
    fn eflags_mask(&self) -> u32 {
        let mut mask = 0x0003_7FD5; // up to NT, RF and VM
        if self.model == super::CpuModel::I486 {
            mask |= CpuFlags::AC.bits();
        }
        mask
    }

    /// Load LDTR and the segment registers of a new task, checking each
    /// descriptor; problems raise #TS (or #NP/#SS) in the new task.
    fn load_task_segments(&mut self, ldt: u16, sels: &[u16; 6], ext: u32) -> CpuResult {
        // Selectors first, so a fault leaves them visible to the handler.
        for (i, seg) in Seg::ALL.iter().enumerate() {
            let mut cache = SegCache::null(sels[i]);
            if self.v86() {
                cache = SegCache::real(sels[i]);
            }
            self.set_seg_cache(*seg, cache);
        }
        self.ldtr = SegCache::null(ldt);

        let ts = |sel: u16| Fault::ts(sel_error(sel) | ext);
        if !is_null(ldt) {
            if ldt & 4 != 0 {
                return Err(ts(ldt));
            }
            let desc = self.fetch_descriptor(ldt, ext).map_err(|_| ts(ldt))?;
            if desc.is_segment() || desc.typ() != LDT {
                return Err(ts(ldt));
            }
            if !desc.present() {
                return Err(ts(ldt));
            }
            self.ldtr = desc.cache(ldt);
        }

        if self.v86() {
            for (i, seg) in Seg::ALL.iter().enumerate() {
                self.load_seg_v86(*seg, sels[i]);
            }
            self.cpl = 3;
            return Ok(());
        }

        let cs = sels[Seg::CS as usize];
        let cpl = rpl(cs);
        if is_null(cs) {
            return Err(ts(cs));
        }
        let mut desc = self.fetch_descriptor(cs, ext).map_err(|_| ts(cs))?;
        if !desc.is_code()
            || (!desc.conforming() && desc.dpl() != cpl)
            || (desc.conforming() && desc.dpl() > cpl)
        {
            return Err(ts(cs));
        }
        if !desc.present() {
            return Err(Fault::np(sel_error(cs) | ext));
        }

        let ss = sels[Seg::SS as usize];
        if is_null(ss) {
            return Err(ts(ss));
        }
        let mut ss_desc = self.fetch_descriptor(ss, ext).map_err(|_| ts(ss))?;
        if rpl(ss) != cpl || ss_desc.dpl() != cpl || !ss_desc.writable_data() {
            return Err(ts(ss));
        }
        if !ss_desc.present() {
            return Err(Fault::ss(sel_error(ss) | ext));
        }
        self.mark_accessed(ss, &mut ss_desc)?;
        self.set_seg_cache(Seg::SS, ss_desc.cache(ss));
        self.load_cs_pm(cs, &mut desc, cpl)?;

        for seg in [Seg::ES, Seg::DS, Seg::FS, Seg::GS] {
            let sel = sels[seg as usize];
            if is_null(sel) {
                continue;
            }
            let mut d = self.fetch_descriptor(sel, ext).map_err(|_| ts(sel))?;
            if !d.readable() || (!d.conforming() && (d.dpl() < cpl || d.dpl() < rpl(sel))) {
                return Err(ts(sel));
            }
            if !d.present() {
                return Err(Fault::np(sel_error(sel) | ext));
            }
            self.mark_accessed(sel, &mut d)?;
            self.set_seg_cache(seg, d.cache(sel));
        }
        Ok(())
    }

    /// JMP or CALL to a TSS descriptor or a task gate named by `selector`,
    /// after the far transfer found `desc` there.
    pub fn far_to_task(&mut self, selector: u16, desc: Descriptor, switch: Switch) -> CpuResult {
        let err = sel_error(selector);
        if desc.dpl() < self.cpl || desc.dpl() < rpl(selector) {
            return Err(Fault::gp(err));
        }
        if desc.typ() == TASK_GATE {
            if !desc.present() {
                return Err(Fault::np(err));
            }
            return self.task_gate(desc.gate_selector(), switch, 0);
        }
        if !matches!(desc.typ(), TSS16_AVAILABLE | TSS32_AVAILABLE) {
            return Err(Fault::gp(err));
        }
        if !desc.present() {
            return Err(Fault::np(err));
        }
        self.task_switch(selector, desc, switch, 0)
    }

    /// May the program use `size` bytes of ports from `port`? In protected
    /// mode with CPL > IOPL, and always in virtual-8086 mode, only if the
    /// I/O permission bitmap in the (32-bit) TSS clears their bits.
    #[inline(always)]
    pub fn check_io(&mut self, port: u16, size: u8) -> CpuResult {
        if !self.pe() || (!self.v86() && self.cpl <= self.iopl()) {
            return Ok(());
        }
        self.check_io_bitmap(port, size)
    }

    fn check_io_bitmap(&mut self, port: u16, size: u8) -> CpuResult {
        let tr = self.tr;
        if tr.attr & 0x80 == 0 || tr.attr & 0x08 == 0 || tr.limit < 0x67 {
            return Err(Fault::gp(0));
        }
        let map = self.sys_read(tr.base.wrapping_add(0x66), 2)?;
        let at = map + port as u32 / 8;
        if at + 1 > tr.limit {
            return Err(Fault::gp(0));
        }
        let bits = self.sys_read(tr.base.wrapping_add(at), 2)?;
        let mask = ((1u32 << size) - 1) << (port % 8);
        if bits & mask != 0 {
            return Err(Fault::gp(0));
        }
        Ok(())
    }
}
