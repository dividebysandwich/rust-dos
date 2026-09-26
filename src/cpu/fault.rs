//! CPU exceptions and interrupt delivery.
//!
//! An instruction that faults returns `Err(Fault)`. The execution loop then
//! puts EIP and ESP back to their values before the instruction (all other
//! state is committed only after the last point at which an instruction can
//! fault) and delivers the exception, so the handler sees the faulting
//! instruction's address, as on a 286 and later.

use super::seg::{
    Descriptor, INT_GATE16, INT_GATE32, TASK_GATE, TRAP_GATE16, TRAP_GATE32, is_null, rpl, sel_error,
};
use super::{Cpu, CpuFlags, Seg, SegCache};

/// The EXT bit of an error code: the fault happened while delivering an
/// event from outside the program (a hardware interrupt or an exception).
pub const EXT: u32 = 1;

/// DR6.BS: the debug exception was the single-step trap.
pub const DR6_BS: u32 = 0x4000;

/// An exception: its vector and, for the exceptions that have one, the error
/// code pushed with it (protected mode only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault {
    pub vector: u8,
    pub error: Option<u32>,
}

impl Fault {
    /// Divide error.
    pub const DE: Fault = Fault::new(0);
    /// Debug exception.
    pub const DB: Fault = Fault::new(1);
    /// Breakpoint (INT3).
    pub const BP: Fault = Fault::new(3);
    /// Overflow (INTO).
    pub const OF: Fault = Fault::new(4);
    /// BOUND range exceeded.
    pub const BR: Fault = Fault::new(5);
    /// Invalid opcode.
    pub const UD: Fault = Fault::new(6);
    /// Coprocessor not available.
    pub const NM: Fault = Fault::new(7);

    pub const fn new(vector: u8) -> Self {
        Fault { vector, error: None }
    }

    pub const fn with_error(vector: u8, error: u32) -> Self {
        Fault {
            vector,
            error: Some(error),
        }
    }

    /// Double fault.
    pub const fn df() -> Self {
        Fault::with_error(8, 0)
    }

    /// Invalid TSS.
    pub const fn ts(error: u32) -> Self {
        Fault::with_error(10, error)
    }

    /// Segment not present.
    pub const fn np(error: u32) -> Self {
        Fault::with_error(11, error)
    }

    /// Stack fault.
    pub const fn ss(error: u32) -> Self {
        Fault::with_error(12, error)
    }

    /// General protection fault.
    pub const fn gp(error: u32) -> Self {
        Fault::with_error(13, error)
    }

    /// Page fault.
    pub const fn pf(error: u32) -> Self {
        Fault::with_error(14, error)
    }

    /// Faults that make a fault during their delivery a double fault.
    fn contributory(&self) -> bool {
        matches!(self.vector, 0 | 10 | 11 | 12 | 13)
    }
}

/// An exception the CPU raised, as kept in `Cpu::exception_log`.
#[derive(Clone, Copy, Debug)]
pub struct ExceptionRecord {
    pub vector: u8,
    pub error: Option<u32>,
    /// Where it happened: the faulting instruction for faults.
    pub cs: u16,
    pub eip: u32,
    /// CR2 when it happened: the address of a page fault.
    pub cr2: u32,
    pub protected: bool,
    /// `Cpu::executed` when it happened.
    pub icount: u64,
}

/// Exceptions kept in `Cpu::exception_log`.
pub const EXCEPTION_LOG_LEN: usize = 64;
/// Exceptions written to the emulator log; later ones are only counted.
const EXCEPTIONS_LOGGED: u64 = 200;

/// Mnemonic of an exception vector, as in #GP.
pub fn exception_name(vector: u8) -> &'static str {
    match vector {
        0 => "#DE",
        1 => "#DB",
        2 => "NMI",
        3 => "#BP",
        4 => "#OF",
        5 => "#BR",
        6 => "#UD",
        7 => "#NM",
        8 => "#DF",
        10 => "#TS",
        11 => "#NP",
        12 => "#SS",
        13 => "#GP",
        14 => "#PF",
        16 => "#MF",
        17 => "#AC",
        _ => "#??",
    }
}

/// Room for the values an interrupt or far call pushes: at most GS, FS, DS,
/// ES, SS, ESP, EFLAGS, CS, EIP and an error code, or the 31 parameters a
/// call gate copies plus SS, ESP, CS and EIP.
pub(crate) struct Frame {
    values: [u32; 36],
    len: usize,
}

impl Frame {
    pub(crate) fn new() -> Self {
        Self { values: [0; 36], len: 0 }
    }

    pub(crate) fn push(&mut self, value: u32) {
        self.values[self.len] = value;
        self.len += 1;
    }

    pub(crate) fn values(&self) -> &[u32] {
        &self.values[..self.len]
    }
}

/// Result of an operation that can raise an exception.
pub type CpuResult<T = ()> = Result<T, Fault>;

/// What raised an interrupt. Protected mode treats them differently: gate
/// privilege checks apply to software interrupts only, and external
/// interrupts set the EXT bit of error codes raised during delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntSource {
    /// An exception raised by the CPU.
    Exception,
    /// INT n, INT3 or INTO.
    Software,
    /// A hardware interrupt from the PIC.
    External,
}

impl Cpu {
    /// Enter the handler of interrupt `vector`. `EIP` is the return
    /// address: the faulting instruction for faults, the next instruction
    /// for traps and hardware interrupts. `error` is the error code
    /// exceptions push in protected mode.
    pub fn deliver_interrupt(&mut self, vector: u8, source: IntSource, error: Option<u32>) -> CpuResult {
        if self.pe() {
            self.deliver_pm(vector, source, error)
        } else {
            self.deliver_real(vector)
        }
    }

    /// Real mode: push the flags and the return address and jump through
    /// the interrupt vector table.
    fn deliver_real(&mut self, vector: u8) -> CpuResult {
        // The IVT at IDTR.base, four bytes per vector.
        let entry = vector as u32 * 4;
        if entry + 3 > self.idtr.limit as u32 {
            return Err(Fault::gp(entry + 2));
        }
        let addr = self.idtr.base.wrapping_add(entry);
        let target_ip = self.read_linear_u16(addr);
        let target_cs = self.read_linear_u16(addr.wrapping_add(2));

        let flags = self.flags16();
        let cs = self.cs();
        let ip = self.ip();
        self.push_sized(2, flags as u32)?;
        self.push_sized(2, cs as u32)?;
        self.push_sized(2, ip as u32)?;

        self.set_cpu_flag(CpuFlags::IF, false);
        self.set_cpu_flag(CpuFlags::TF, false);
        self.set_cpu_flag(CpuFlags::AC, false);
        self.load_seg_real(Seg::CS, target_cs);
        self.set_ip(target_ip);
        Ok(())
    }

    /// Protected mode: enter the handler through the interrupt or trap gate
    /// in the IDT, switching to the stack in the TSS for a handler at a
    /// more privileged level, or switch tasks through a task gate.
    fn deliver_pm(&mut self, vector: u8, source: IntSource, error: Option<u32>) -> CpuResult {
        let ext = if source == IntSource::Software { 0 } else { EXT };
        let idt_error = vector as u32 * 8 + 2 + ext;
        if vector as u32 * 8 + 7 > self.idtr.limit as u32 {
            return Err(Fault::gp(idt_error));
        }
        let entry = self.idtr.base.wrapping_add(vector as u32 * 8);
        let low = self.sys_read_u32(entry)? as u64;
        let high = self.sys_read_u32(entry.wrapping_add(4))? as u64;
        let gate = Descriptor(low | (high << 32));
        let typ = gate.typ();
        if gate.is_segment() || !matches!(typ, TASK_GATE | INT_GATE16 | TRAP_GATE16 | INT_GATE32 | TRAP_GATE32) {
            return Err(Fault::gp(idt_error));
        }
        // INT n, INT3 and INTO may only use gates of their privilege.
        if source == IntSource::Software && gate.dpl() < self.cpl {
            return Err(Fault::gp(idt_error));
        }
        if !gate.present() {
            return Err(Fault::np(idt_error));
        }
        if typ == TASK_GATE {
            return self.task_gate(gate.gate_selector(), super::task::Switch::Interrupt(error), ext);
        }

        let selector = gate.gate_selector();
        if is_null(selector) {
            return Err(Fault::gp(ext));
        }
        let mut desc = self.fetch_descriptor(selector, ext)?;
        let sel_err = sel_error(selector) | ext;
        if !desc.is_code() || desc.dpl() > self.cpl {
            return Err(Fault::gp(sel_err));
        }
        if !desc.present() {
            return Err(Fault::np(sel_err));
        }
        let size: u8 = if gate.is_32bit_system() { 4 } else { 2 };
        let offset = gate.gate_offset();
        if offset > desc.limit() {
            return Err(Fault::gp(ext));
        }
        let v86 = self.v86();
        let eflags = self.flags.bits();
        let (cs, eip) = (self.cs() as u32, self.eip);

        if !desc.conforming() && desc.dpl() < self.cpl {
            // To a more privileged handler, on the stack the TSS holds for
            // its level. From virtual-8086 mode that must be level 0, and
            // the real-mode segment registers are saved too.
            let new_cpl = desc.dpl();
            if v86 && new_cpl != 0 {
                return Err(Fault::gp(sel_err));
            }
            let (ss_sel, esp) = self.tss_stack(new_cpl, ext)?;
            let ss_cache = self.check_new_stack(ss_sel, new_cpl, ext)?;
            let mut frame = Frame::new();
            if v86 {
                for seg in [Seg::GS, Seg::FS, Seg::DS, Seg::ES] {
                    frame.push(self.seg_cache(seg).selector as u32);
                }
            }
            frame.push(self.ss() as u32);
            frame.push(self.esp());
            frame.push(eflags);
            frame.push(cs);
            frame.push(eip);
            if let Some(code) = error {
                frame.push(code);
            }
            let ss_fault = Fault::ss(sel_error(ss_sel) | ext);
            let sp = self.push_frame(&ss_cache, esp, size, frame.values(), ss_fault, new_cpl == 3)?;
            self.mark_accessed(selector, &mut desc)?;
            self.set_seg_cache(Seg::SS, ss_cache);
            self.set_esp(esp);
            self.set_stack_ptr(sp);
            if v86 {
                for seg in [Seg::GS, Seg::FS, Seg::DS, Seg::ES] {
                    self.set_seg_cache(seg, SegCache::null(0));
                }
            }
            self.load_cs_pm(selector, &mut desc, new_cpl)?;
        } else {
            if v86 {
                return Err(Fault::gp(sel_err));
            }
            // Same privilege level (or a conforming handler): the current
            // stack.
            let mut frame = Frame::new();
            frame.push(eflags);
            frame.push(cs);
            frame.push(eip);
            if let Some(code) = error {
                frame.push(code);
            }
            let ss_cache = *self.seg_cache(Seg::SS);
            let sp = self.stack_ptr();
            let sp = self.push_frame(&ss_cache, sp, size, frame.values(), Fault::ss(ext), self.cpl == 3)?;
            self.mark_accessed(selector, &mut desc)?;
            self.set_stack_ptr(sp);
            let cpl = self.cpl;
            self.load_cs_pm(selector, &mut desc, cpl)?;
        }
        self.eip = offset;
        self.flags.remove(CpuFlags::TF | CpuFlags::NT | CpuFlags::RF | CpuFlags::VM);
        if matches!(typ, INT_GATE16 | INT_GATE32) {
            self.flags.remove(CpuFlags::IF);
        }
        Ok(())
    }

    /// Check and write the values of a stack frame, the first value at the
    /// highest address, onto the stack `cache` describes, starting below
    /// `sp`. Returns the new stack pointer. Nothing is written unless every
    /// slot can be: a slot outside the segment raises `fault`, one on an
    /// unmapped page a page fault.
    pub(crate) fn push_frame(
        &mut self,
        cache: &SegCache,
        sp: u32,
        size: u8,
        values: &[u32],
        fault: Fault,
        user: bool,
    ) -> CpuResult<u32> {
        let stack32 = cache.attr & super::ATTR_DB != 0;
        let mask = if stack32 { 0xFFFF_FFFF } else { 0xFFFF };
        let mut refs = [None; 36];
        let mut at = sp;
        for (i, _) in values.iter().enumerate() {
            at = at.wrapping_sub(size as u32) & mask;
            let last = at.wrapping_add(size as u32 - 1);
            if at < cache.lo || last > cache.hi || last < at || cache.rights & super::regs::RIGHT_WRITE == 0 {
                return Err(fault);
            }
            let lin = cache.base.wrapping_add(at);
            refs[i] = Some(self.lin_ref(lin, size, super::Access::Write, user)?);
        }
        for (i, &value) in values.iter().enumerate() {
            if let Some(r) = refs[i] {
                self.mem_write(r, value);
            }
        }
        Ok(at)
    }

    /// The stack pointer for privilege level `cpl` in the current TSS.
    pub(crate) fn tss_stack(&mut self, cpl: u8, ext: u32) -> CpuResult<(u16, u32)> {
        let tr = self.tr;
        let ts = Fault::ts(sel_error(tr.selector) | ext);
        if tr.attr & 0x80 == 0 {
            return Err(ts);
        }
        if tr.attr & 0x08 != 0 {
            // 32-bit TSS: ESPn at 4 + 8n, SSn at 8 + 8n.
            let at = 4 + 8 * cpl as u32;
            if at + 5 > tr.limit {
                return Err(ts);
            }
            let esp = self.sys_read(tr.base.wrapping_add(at), 4)?;
            let ss = self.sys_read(tr.base.wrapping_add(at + 4), 2)? as u16;
            Ok((ss, esp))
        } else {
            // 16-bit TSS: SPn at 2 + 4n, SSn at 4 + 4n.
            let at = 2 + 4 * cpl as u32;
            if at + 3 > tr.limit {
                return Err(ts);
            }
            let sp = self.sys_read(tr.base.wrapping_add(at), 2)?;
            let ss = self.sys_read(tr.base.wrapping_add(at + 2), 2)? as u16;
            Ok((ss, sp))
        }
    }

    /// Check the stack segment a switch to privilege level `cpl` takes
    /// from the TSS, and return the cache to load.
    pub(crate) fn check_new_stack(&mut self, selector: u16, cpl: u8, ext: u32) -> CpuResult<SegCache> {
        let err = sel_error(selector) | ext;
        if is_null(selector) {
            return Err(Fault::ts(ext));
        }
        let mut desc = self.fetch_descriptor(selector, ext).map_err(|f| {
            if f.vector == 13 { Fault::ts(err) } else { f }
        })?;
        if rpl(selector) != cpl || desc.dpl() != cpl || !desc.writable_data() {
            return Err(Fault::ts(err));
        }
        if !desc.present() {
            return Err(Fault::ss(err));
        }
        self.mark_accessed(selector, &mut desc)?;
        Ok(desc.cache(selector))
    }

    /// The single-step trap after an instruction that began with TF set:
    /// DR6.BS, and the debug exception's handler entered with EIP at the
    /// next instruction. It stays out of the exception log, as a program
    /// that traces itself raises one for every instruction it runs.
    pub fn single_step_trap(&mut self) {
        self.dr[6] |= DR6_BS;
        let (eip, esp) = (self.eip, self.esp());
        if let Err(fault) = self.deliver_interrupt(Fault::DB.vector, IntSource::Exception, None) {
            self.eip = eip;
            self.set_esp(esp);
            self.raise(fault);
        }
    }

    /// Deliver the exception raised by an instruction. When its delivery
    /// faults, the second fault is delivered instead, or a double fault
    /// for two contributory faults (or a page fault followed by a
    /// contributory or page fault), and a shutdown when the double fault
    /// can't be delivered either.
    pub fn raise(&mut self, fault: Fault) {
        self.note_exception(fault);
        let mut current = fault;
        for _ in 0..8 {
            let (eip, esp) = (self.eip, self.esp());
            match self.deliver_interrupt(current.vector, IntSource::Exception, self.error_code(current)) {
                Ok(()) => return,
                Err(second) => {
                    self.eip = eip;
                    self.set_esp(esp);
                    if current.vector == 8 {
                        break;
                    }
                    let double = (current.contributory() && second.contributory())
                        || (current.vector == 14 && (second.contributory() || second.vector == 14));
                    current = if double { Fault::df() } else { second };
                }
            }
        }
        self.shutdown();
    }

    /// Keep a record of an exception for debuggers, and log the first ones.
    fn note_exception(&mut self, fault: Fault) {
        self.exceptions += 1;
        let record = ExceptionRecord {
            vector: fault.vector,
            error: fault.error,
            cs: self.cs(),
            eip: self.eip,
            cr2: self.cr2,
            protected: self.pe(),
            icount: self.executed,
        };
        if self.exception_log.len() == EXCEPTION_LOG_LEN {
            self.exception_log.pop_front();
        }
        self.exception_log.push_back(record);
        if self.exceptions <= EXCEPTIONS_LOGGED {
            let error = match (fault.error, self.pe()) {
                (Some(e), true) => format!("({:04X})", e),
                _ => String::new(),
            };
            let cr2 = if fault.vector == 14 { format!(" CR2={:08X}", self.cr2) } else { String::new() };
            self.bus.log_string(&format!(
                "[CPU] {}{} at {:04X}:{:08X}{}{}",
                exception_name(fault.vector),
                error,
                record.cs,
                record.eip,
                cr2,
                if self.exceptions == EXCEPTIONS_LOGGED { " (further exceptions not logged)" } else { "" }
            ));
        }
    }

    /// The error code an exception pushes: in protected mode only.
    fn error_code(&self, fault: Fault) -> Option<u32> {
        if self.pe() { fault.error } else { None }
    }

    /// Triple fault: the processor stops, and an AT's chipset turns that
    /// into a reset.
    pub fn shutdown(&mut self) {
        self.bus.log_string(&format!(
            "[CPU] Shutdown (triple fault) at {:04X}:{:08X}",
            self.cs(),
            self.eip()
        ));
        self.reset();
    }

    /// The processor's state after a reset: real mode, interrupts off,
    /// executing the BIOS reset entry at F000:FFF0, which decides from the
    /// CMOS shutdown code whether to resume a program or start over.
    pub fn reset(&mut self) {
        self.gpr = [0; 8];
        // DX holds the processor signature: family and stepping.
        self.gpr[super::regs::EDX] = match self.model {
            super::CpuModel::I386 => 0x0303,
            super::CpuModel::I486 => 0x0402,
        };
        self.flags = CpuFlags::R1;
        self.reset_to_real_mode();
        self.seg[Seg::CS as usize] = super::SegCache::real(0xF000);
        self.eip = 0xFFF0;
        self.state = super::CpuState::Running;
    }

    /// The system state of a reset, keeping the registers: real mode,
    /// paging off, the descriptor tables at their power-on values and
    /// real-mode segments. The shell starts from it however the last
    /// program left the processor.
    pub fn reset_to_real_mode(&mut self) {
        self.cr0 = super::CR0_ET;
        self.cr2 = 0;
        self.cr3 = 0;
        self.gdtr = super::DescTable { base: 0, limit: 0xFFFF };
        self.idtr = super::DescTable { base: 0, limit: 0x3FF };
        self.ldtr = super::SegCache::null(0);
        self.tr = super::SegCache::null(0);
        self.seg = [super::SegCache::real(0); 6];
        self.flags.remove(CpuFlags::VM | CpuFlags::NT | CpuFlags::RF);
        self.cpl = 0;
        self.tlb.flush();
        self.irq_shadow = false;
    }
}
