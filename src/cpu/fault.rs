//! CPU exceptions and interrupt delivery.
//!
//! An instruction that faults returns `Err(Fault)`. The execution loop then
//! puts EIP and ESP back to their values before the instruction (all other
//! state is committed only after the last point at which an instruction can
//! fault) and delivers the exception, so the handler sees the faulting
//! instruction's address, as on a 286 and later.

use super::{Cpu, CpuFlags, Seg};

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
    /// Enter the handler of interrupt `vector`: push the flags and the
    /// return address and jump through the interrupt vector table. `EIP` is
    /// the return address: the faulting instruction for faults, the next
    /// instruction for traps and hardware interrupts.
    pub fn deliver_interrupt(&mut self, vector: u8, _source: IntSource) -> CpuResult {
        // Real mode: the IVT at IDTR.base, four bytes per vector.
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

    /// Deliver the exception raised by an instruction, escalating to a
    /// double fault, and to a shutdown when the double fault can't be
    /// delivered either.
    pub fn raise(&mut self, fault: Fault) {
        if self.deliver_interrupt(fault.vector, IntSource::Exception).is_ok() {
            return;
        }
        if self.deliver_interrupt(8, IntSource::Exception).is_ok() {
            return;
        }
        self.shutdown();
    }

    /// Triple fault: the processor stops. An AT's chipset turns that into a
    /// reset; we end the program and go back to the shell.
    pub fn shutdown(&mut self) {
        self.bus.log_string(&format!(
            "[CPU] Shutdown (triple fault) at {:04X}:{:08X}",
            self.cs(),
            self.eip()
        ));
        self.state = super::CpuState::RebootShell;
    }
}
