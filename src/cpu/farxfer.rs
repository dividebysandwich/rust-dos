//! Far transfers in protected mode: JMP and CALL to code segments, call
//! gates and tasks, RETF and IRET to the same or an outer privilege level,
//! and IRET back to virtual-8086 mode.

use super::fault::{CpuResult, Fault, Frame};
use super::seg::{
    CALL_GATE16, CALL_GATE32, Descriptor, TASK_GATE, TSS16_AVAILABLE, TSS32_AVAILABLE, is_null, rpl, sel_error,
};
use super::task::Switch;
use super::{Cpu, CpuFlags, Seg};

impl Cpu {
    /// Check a code segment a JMP or CALL goes to directly: a conforming
    /// segment at the CPL or more privileged, or a non-conforming one at
    /// the CPL with RPL <= CPL.
    fn check_direct_code(&self, selector: u16, desc: &Descriptor) -> CpuResult {
        let err = sel_error(selector);
        let ok = if desc.conforming() {
            desc.dpl() <= self.cpl
        } else {
            rpl(selector) <= self.cpl && desc.dpl() == self.cpl
        };
        if !ok {
            return Err(Fault::gp(err));
        }
        if !desc.present() {
            return Err(Fault::np(err));
        }
        Ok(())
    }

    /// Check the gate and code segment of a far JMP or CALL through a call
    /// gate: (target selector, its descriptor, target offset).
    fn call_gate_target(&mut self, selector: u16, gate: &Descriptor) -> CpuResult<(u16, Descriptor, u32)> {
        let err = sel_error(selector);
        if gate.dpl() < self.cpl || gate.dpl() < rpl(selector) {
            return Err(Fault::gp(err));
        }
        if !gate.present() {
            return Err(Fault::np(err));
        }
        let target = gate.gate_selector();
        if is_null(target) {
            return Err(Fault::gp(0));
        }
        let code = self.fetch_descriptor(target, 0)?;
        let terr = sel_error(target);
        if !code.is_code() || code.dpl() > self.cpl {
            return Err(Fault::gp(terr));
        }
        if !code.present() {
            return Err(Fault::np(terr));
        }
        let offset = gate.gate_offset();
        if offset > code.limit() {
            return Err(Fault::gp(0));
        }
        Ok((target, code, offset))
    }

    /// JMP to `selector:offset` in protected mode.
    pub fn jmp_far_pm(&mut self, selector: u16, offset: u32) -> CpuResult {
        if is_null(selector) {
            return Err(Fault::gp(0));
        }
        let mut desc = self.fetch_descriptor(selector, 0)?;
        let err = sel_error(selector);
        if desc.is_code() {
            self.check_direct_code(selector, &desc)?;
            if offset > desc.limit() {
                return Err(Fault::gp(0));
            }
            let cpl = self.cpl;
            self.load_cs_pm(selector, &mut desc, cpl)?;
            self.eip = offset;
            return Ok(());
        }
        if desc.is_segment() {
            return Err(Fault::gp(err));
        }
        match desc.typ() {
            CALL_GATE16 | CALL_GATE32 => {
                let (target, mut code, offset) = self.call_gate_target(selector, &desc)?;
                // A JMP through a call gate doesn't change privilege.
                if !code.conforming() && code.dpl() != self.cpl {
                    return Err(Fault::gp(sel_error(target)));
                }
                let cpl = self.cpl;
                self.load_cs_pm(target, &mut code, cpl)?;
                self.eip = offset;
                Ok(())
            }
            TASK_GATE | TSS16_AVAILABLE | TSS32_AVAILABLE => self.far_to_task(selector, desc, Switch::Jmp),
            _ => Err(Fault::gp(err)),
        }
    }

    /// CALL to `selector:offset` in protected mode; a direct call pushes
    /// CS:EIP with `size` (2 or 4) bytes each.
    pub fn call_far_pm(&mut self, selector: u16, offset: u32, size: u8) -> CpuResult {
        if is_null(selector) {
            return Err(Fault::gp(0));
        }
        let mut desc = self.fetch_descriptor(selector, 0)?;
        let err = sel_error(selector);
        let (cs, eip) = (self.cs() as u32, self.eip);
        if desc.is_code() {
            self.check_direct_code(selector, &desc)?;
            if offset > desc.limit() {
                return Err(Fault::gp(0));
            }
            let ss = *self.seg_cache(Seg::SS);
            let sp = self.stack_ptr();
            let sp = self.push_frame(&ss, sp, size, &[cs, eip], Fault::ss(0), self.cpl == 3)?;
            let cpl = self.cpl;
            self.load_cs_pm(selector, &mut desc, cpl)?;
            self.set_stack_ptr(sp);
            self.eip = offset;
            return Ok(());
        }
        if desc.is_segment() {
            return Err(Fault::gp(err));
        }
        match desc.typ() {
            CALL_GATE16 | CALL_GATE32 => {
                let (target, mut code, offset) = self.call_gate_target(selector, &desc)?;
                let gsize: u8 = if desc.typ() == CALL_GATE32 { 4 } else { 2 };
                if !code.conforming() && code.dpl() < self.cpl {
                    // To a more privileged level, on its stack from the TSS,
                    // copying the gate's parameters over.
                    let new_cpl = code.dpl();
                    let (ss_sel, esp) = self.tss_stack(new_cpl, 0)?;
                    let ss_cache = self.check_new_stack(ss_sel, new_cpl, 0)?;
                    let mut frame = Frame::new();
                    frame.push(self.ss() as u32);
                    frame.push(self.esp());
                    let params = desc.gate_params();
                    for i in (0..params).rev() {
                        frame.push(self.stack_read(i * gsize as u32, gsize)?);
                    }
                    frame.push(cs);
                    frame.push(eip);
                    let fault = Fault::ss(sel_error(ss_sel));
                    let sp = self.push_frame(&ss_cache, esp, gsize, frame.values(), fault, new_cpl == 3)?;
                    self.mark_accessed(target, &mut code)?;
                    self.set_seg_cache(Seg::SS, ss_cache);
                    self.set_esp(esp);
                    self.set_stack_ptr(sp);
                    self.load_cs_pm(target, &mut code, new_cpl)?;
                } else {
                    let ss = *self.seg_cache(Seg::SS);
                    let sp = self.stack_ptr();
                    let sp = self.push_frame(&ss, sp, gsize, &[cs, eip], Fault::ss(0), self.cpl == 3)?;
                    let cpl = self.cpl;
                    self.load_cs_pm(target, &mut code, cpl)?;
                    self.set_stack_ptr(sp);
                }
                self.eip = offset;
                Ok(())
            }
            TASK_GATE | TSS16_AVAILABLE | TSS32_AVAILABLE => self.far_to_task(selector, desc, Switch::Call),
            _ => Err(Fault::gp(err)),
        }
    }

    /// RETF in protected mode: `size`-byte stack slots, and `release`
    /// bytes of parameters to drop (RETF imm16).
    pub fn ret_far_pm(&mut self, size: u8, release: u32) -> CpuResult {
        let offset = self.stack_read(0, size)?;
        let selector = self.stack_read(size as u32, 2)? as u16;
        let offset = if size == 2 { offset & 0xFFFF } else { offset };
        self.return_far_pm(selector, offset, size, 2 * size as u32, release, None)
    }

    /// IRET in protected mode: to the calling task when NT is set, to
    /// virtual-8086 mode when the popped EFLAGS says so (from level 0),
    /// otherwise to the same or an outer level.
    pub fn iret_pm(&mut self, size: u8) -> CpuResult {
        if self.flags.contains(CpuFlags::NT) {
            return self.task_return();
        }
        let offset = self.stack_read(0, size)?;
        let selector = self.stack_read(size as u32, 2)? as u16;
        let flags = self.stack_read(2 * size as u32, size)?;
        if size == 4 && flags & CpuFlags::VM.bits() != 0 && self.cpl == 0 {
            return self.iret_to_v86(offset, selector, flags);
        }
        let offset = if size == 2 { offset & 0xFFFF } else { offset };
        self.return_far_pm(selector, offset, size, 3 * size as u32, 0, Some(flags))
    }

    /// The part of RETF and IRET after popping CS:EIP (and EFLAGS):
    /// `popped` bytes. A return to an outer level pops that level's SS:ESP
    /// too.
    fn return_far_pm(
        &mut self,
        selector: u16,
        offset: u32,
        size: u8,
        popped: u32,
        release: u32,
        flags: Option<u32>,
    ) -> CpuResult {
        if is_null(selector) {
            return Err(Fault::gp(0));
        }
        let err = sel_error(selector);
        let new_cpl = rpl(selector);
        if new_cpl < self.cpl {
            return Err(Fault::gp(err));
        }
        let mut desc = self.fetch_descriptor(selector, 0)?;
        if !desc.is_code()
            || (desc.conforming() && desc.dpl() > new_cpl)
            || (!desc.conforming() && desc.dpl() != new_cpl)
        {
            return Err(Fault::gp(err));
        }
        if !desc.present() {
            return Err(Fault::np(err));
        }
        if offset > desc.limit() {
            return Err(Fault::gp(0));
        }

        if new_cpl == self.cpl {
            let sp = self.stack_ptr().wrapping_add(popped + release);
            self.load_cs_pm(selector, &mut desc, new_cpl)?;
            self.set_stack_ptr(sp);
            self.eip = offset;
            if let Some(flags) = flags {
                self.load_flags_pm(flags, size, true);
            }
            return Ok(());
        }

        // To an outer level: its stack pointer follows.
        let at = popped + release;
        let new_esp = self.stack_read(at, size)?;
        let ss_sel = self.stack_read(at + size as u32, 2)? as u16;
        let ss_err = sel_error(ss_sel);
        if is_null(ss_sel) {
            return Err(Fault::gp(0));
        }
        if rpl(ss_sel) != new_cpl {
            return Err(Fault::gp(ss_err));
        }
        let mut ss_desc = self.fetch_descriptor(ss_sel, 0)?;
        if !ss_desc.writable_data() || ss_desc.dpl() != new_cpl {
            return Err(Fault::gp(ss_err));
        }
        if !ss_desc.present() {
            return Err(Fault::ss(ss_err));
        }
        self.mark_accessed(ss_sel, &mut ss_desc)?;
        if let Some(flags) = flags {
            // The flags load with the privilege of the level returning.
            self.load_flags_pm(flags, size, true);
        }
        self.load_cs_pm(selector, &mut desc, new_cpl)?;
        self.set_seg_cache(Seg::SS, ss_desc.cache(ss_sel));
        let new_esp = if size == 2 { (self.esp() & 0xFFFF_0000) | (new_esp & 0xFFFF) } else { new_esp };
        self.set_esp(new_esp);
        let sp = self.stack_ptr().wrapping_add(release);
        self.set_stack_ptr(sp);
        self.eip = offset;
        self.null_inaccessible_segments();
        Ok(())
    }

    /// IRETD from level 0 to virtual-8086 mode: the frame holds EIP, CS,
    /// EFLAGS, ESP, SS, ES, DS, FS and GS.
    fn iret_to_v86(&mut self, offset: u32, cs: u16, flags: u32) -> CpuResult {
        let esp = self.stack_read(12, 4)?;
        let mut sels = [0u16; 5];
        for (i, s) in sels.iter_mut().enumerate() {
            *s = self.stack_read(16 + 4 * i as u32, 2)? as u16;
        }
        let [ss, es, ds, fs, gs] = sels;
        let mut writable = 0x0003_7FD5; // everything up to VM
        if self.model == super::CpuModel::I486 {
            writable |= CpuFlags::AC.bits();
        }
        self.flags = CpuFlags::from_bits_retain((flags & writable) | 0x0002);
        self.load_seg_v86(Seg::CS, cs);
        self.load_seg_v86(Seg::SS, ss);
        self.load_seg_v86(Seg::ES, es);
        self.load_seg_v86(Seg::DS, ds);
        self.load_seg_v86(Seg::FS, fs);
        self.load_seg_v86(Seg::GS, gs);
        self.set_esp(esp);
        self.eip = offset & 0xFFFF;
        self.cpl = 3;
        Ok(())
    }

    /// Load (E)FLAGS popped by POPF or IRET in protected or virtual-8086
    /// mode. IOPL changes only at level 0, IF only at a level allowed I/O
    /// (CPL <= IOPL); VM never changes here. IRET also loads RF.
    pub fn load_flags_pm(&mut self, value: u32, size: u8, iret: bool) {
        let mut writable = 0x4DD5; // CF PF AF ZF SF TF DF OF NT
        if size == 4 {
            if iret {
                writable |= CpuFlags::RF.bits();
            }
            if self.model == super::CpuModel::I486 {
                writable |= CpuFlags::AC.bits();
            }
        }
        let iopl = self.iopl();
        if self.cpl == 0 && !self.v86() {
            writable |= CpuFlags::IOPL.bits() | CpuFlags::IF.bits();
        } else if self.cpl <= iopl {
            writable |= CpuFlags::IF.bits();
        }
        if size == 2 {
            writable &= 0xFFFF;
        }
        let bits = (self.flags.bits() & !writable) | (value & writable) | 0x0002;
        self.flags = CpuFlags::from_bits_retain(bits);
    }

    /// The I/O privilege level (EFLAGS bits 12-13).
    #[inline(always)]
    pub fn iopl(&self) -> u8 {
        ((self.flags.bits() >> 12) & 3) as u8
    }
}
