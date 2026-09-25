//! A whole machine's state: the CPU's section, then the bus's (bus/state.rs),
//! in the order they are read back.

use super::{Reader, Result, State, Writer};
use crate::cpu::Cpu;
use std::sync::OnceLock;

const CPU_VERSION: u16 = 1;

/// The machine's state, between batches.
pub fn save(cpu: &Cpu) -> Vec<u8> {
    let mut w = Writer::new();
    cpu.bus.save_state(&mut w);
    w.section(b"CPU ", CPU_VERSION, |w| cpu.save(w));
    w.buf
}

/// Load a state `save` wrote into the machine, between batches. A state
/// that can't be loaded (damaged, of a machine with another amount of
/// memory, or with a drive that can't be mounted) leaves the machine as it
/// was. Files open in the state that are gone stay closed, which the log
/// tells.
pub fn load(cpu: &mut Cpu, data: &[u8]) -> Result<()> {
    let before = save(cpu);
    let loaded = load_sections(cpu, data);
    if loaded.is_err() {
        load_sections(cpu, &before).expect("the state the machine was in loads back");
    }
    cpu.forget_caches();
    cpu.bus.after_load();
    for path in loaded.as_ref().map_or(&[][..], Vec::as_slice) {
        cpu.bus.log_string(&format!("[STATE] {} was open and can't be opened again", path));
    }
    loaded.map(drop)
}

fn load_sections(cpu: &mut Cpu, data: &[u8]) -> Result<Vec<String>> {
    let mut r = Reader::new(data);
    let lost = cpu.bus.load_state(&mut r)?;
    cpu.load(&mut r.section(b"CPU ", CPU_VERSION)?)?;
    Ok(lost)
}

/// Whether `RUST_DOS_STATE_CHAOS` is set (to anything but 0): then every
/// batch starts by saving the machine and loading it back (`reload`), so
/// that the tests, run so, fail where a device saves or loads its state
/// wrongly, a cache isn't worked out again after a load, or open files
/// don't open again as they were. (Fields left out of a state stay as they
/// are in a reload; the tests that load into a fresh machine find those.)
pub fn chaos() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUST_DOS_STATE_CHAOS").is_some_and(|v| v != "0"))
}

/// Save the machine and load the state back into it. The FM chip and the
/// MIDI synthesizer, which a state can't hold, go on as they were, as does
/// the sound rendered so far: a load into the machine that saved changes
/// nothing else a program would notice.
pub fn reload(cpu: &mut Cpu) {
    let state = save(cpu);
    load_sections(cpu, &state).expect("a machine loads the state it saved");
    cpu.forget_caches();
    cpu.bus.after_reload();
}
