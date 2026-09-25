//! The CPU's part of a save state: its registers, the FPU, and what the
//! DOS and the shell rust-dos runs in Rust keep in it (the batch file
//! running, the processes, the environment). The bus is saved on its own
//! (bus/state.rs), and the caches of decoded and translated code are
//! thrown away after a load.

use super::{Cpu, CpuFlags, CpuSnapshot, CpuState, DescTable, FpuFlags, ProcessContext, SegCache};
use crate::savestate::{Reader, Result, State, Writer};

crate::state_fields!(DescTable { base, limit });
crate::state_enum!(CpuState { CpuState::Running, CpuState::Halted, CpuState::RebootShell });
crate::state_fields!(CpuSnapshot { gpr, eip, flags, seg });
crate::state_fields!(ProcessContext { regs, psp, heap_pointer, dta });

impl State for CpuFlags {
    fn save(&self, w: &mut Writer) {
        self.bits().save(w);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let mut bits = 0u32;
        bits.load(r)?;
        *self = CpuFlags::from_bits_retain(bits);
        Ok(())
    }
}

impl State for FpuFlags {
    fn save(&self, w: &mut Writer) {
        self.bits().save(w);
    }
    fn load(&mut self, r: &mut Reader) -> Result<()> {
        let mut bits = 0u16;
        bits.load(r)?;
        *self = FpuFlags::from_bits_retain(bits);
        Ok(())
    }
}

impl Default for ProcessContext {
    fn default() -> Self {
        ProcessContext {
            regs: CpuSnapshot { gpr: [0; 8], eip: 0, flags: CpuFlags::empty(), seg: [SegCache::real(0); 6] },
            psp: 0,
            heap_pointer: 0,
            dta: (0, 0),
        }
    }
}

crate::state_fields!(Cpu {
    gpr, eip, seg, cr0, cr2, cr3, dr, gdtr, idtr, ldtr, tr, cpl, flags, state,
    pending_command, shell_wait, shell_prompt_at, secondary_shells, batch,
    environment, current_psp, heap_pointer, resident_end, resident_upper,
    last_child_exit, errorlevel, last_dos_error, con_pending_scan, con_line,
    con_pending, alloc_strategy, bios_wait_until,
    fpu_stack, fpu_top, fpu_flags, fpu_control, fpu_tags,
    process_stack, irq_shadow, executed, idle, hle_retry, dyn_latched,
} skip {
    // Saved in sections of its own.
    bus,
    // Set from the configuration, which a state carries in its header.
    model, core,
    // Caches, emptied after a load.
    tlb, decode_cache, dynrec,
    // The host's: the lines typed at the prompt stay the user's, and the
    // counts and logs are the debugger's.
    shell_history, shell_completion, null_interrupts, mode_switches, exceptions, exception_log,
    // Only set while a command runs, never between the batches a state
    // is saved in.
    secondary, stdout_capture,
});

impl Cpu {
    /// Throw away what was worked out from the machine's memory and
    /// registers before a load: translated and decoded code, and the
    /// paging's cached translations.
    pub(crate) fn forget_caches(&mut self) {
        self.tlb.flush();
        self.decode_cache = crate::instr_cache::InstrCache::new(16);
        self.dynrec.flush();
    }
}
