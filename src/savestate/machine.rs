//! A whole machine's state: the CPU's section, then the bus's (bus/state.rs),
//! in the order they are read back.

use super::{Reader, Result, State, Writer};
use crate::cpu::Cpu;

const CPU_VERSION: u16 = 1;

/// The machine's state, between batches.
pub fn save(cpu: &Cpu) -> Vec<u8> {
    let mut w = Writer::new();
    cpu.bus.save_state(&mut w);
    w.section(b"CPU ", CPU_VERSION, |w| cpu.save(w));
    w.buf
}

/// Load a state `save` wrote into the machine, between batches. A state
/// that can't be loaded (damaged, or of a machine with another amount of
/// memory) leaves the machine as it was.
pub fn load(cpu: &mut Cpu, data: &[u8]) -> Result<()> {
    let before = save(cpu);
    let loaded = load_sections(cpu, data);
    if loaded.is_err() {
        load_sections(cpu, &before).expect("the state the machine was in loads back");
    }
    cpu.forget_caches();
    cpu.bus.after_load();
    loaded
}

fn load_sections(cpu: &mut Cpu, data: &[u8]) -> Result<()> {
    let mut r = Reader::new(data);
    cpu.bus.load_state(&mut r)?;
    cpu.load(&mut r.section(b"CPU ", CPU_VERSION)?)?;
    Ok(())
}
