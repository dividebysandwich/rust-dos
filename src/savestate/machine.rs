//! A whole machine's state: the CPU's section, then the bus's (bus/state.rs),
//! in the order they are read back.

use super::{Reader, Result, State, Writer};
use crate::cpu::Cpu;
use std::sync::OnceLock;

const CPU_VERSION: u16 = 3;
const ENV_VERSION: u16 = 1;
const SEP_VERSION: u16 = 1;
const SSE_VERSION: u16 = 1;
const K6_VERSION: u16 = 1;
const IDLE_VERSION: u16 = 1;

/// The machine's state, between batches.
pub fn save(cpu: &Cpu) -> Vec<u8> {
    let mut w = Writer::new();
    write(cpu, &mut w);
    w.buf
}

/// The bytes `save` writes, and the parts they are made of: each
/// section, and in the bus's sections each device (`SOUN.sb`, `CORE.pic`),
/// with its range of the bytes. For telling which device two states
/// differ in.
pub fn save_parts(cpu: &Cpu) -> (Vec<u8>, Vec<(String, std::ops::Range<usize>)>) {
    let mut w = Writer::labelled();
    write(cpu, &mut w);
    let parts = w.parts();
    (w.buf, parts)
}

fn write(cpu: &Cpu, w: &mut Writer) {
    cpu.bus.save_state(w);
    w.section(b"CPU ", CPU_VERSION, |w| cpu.save(w));
    w.section(b"ENV ", ENV_VERSION, |w| cpu.env_injector.save(w));
    w.section(b"SEP ", SEP_VERSION, |w| cpu.sysenter.save(w));
    w.section(b"SSE ", SSE_VERSION, |w| cpu.sse.save(w));
    w.section(b"K6  ", K6_VERSION, |w| cpu.k6.save(w));
    w.section(b"IDLE", IDLE_VERSION, |w| cpu.os_idles.save(w));
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
        // Folders a debugger load asked for are the failed state's, not
        // the machine's from before: a load that failed ahead of the DOS
        // section didn't take them.
        cpu.bus.disk.folders_from_state.clear();
        load_sections(cpu, &before).expect("the state the machine was in loads back");
    }
    if loaded.is_ok() {
        // The settings' variables, whatever the state's environment has.
        cpu.apply_env_rules();
        // The session's count of running programs goes with the state's
        // machine: one, if a program runs in it.
        cpu.programs.running = u32::from(!cpu.program.is_empty());
    }
    cpu.forget_caches();
    cpu.bus.after_load();
    for path in loaded.as_ref().map_or(&[][..], Vec::as_slice) {
        cpu.bus.log_string(&format!("[STATE] {} was open and can't be opened again", path));
    }
    loaded.map(drop)
}

/// Undo a load that failed after a state's hardware was put in place
/// (`Hardware::apply`): the hardware goes back to `old`, as `settings`
/// had it, and the machine to `before`, which `save` took before the
/// hardware changed.
pub fn roll_back(cpu: &mut Cpu, hardware: &mut crate::hardware::Hardware, old: &crate::hardware::Hardware, settings: &crate::config::Settings, before: &[u8]) {
    if hardware != old {
        for warning in hardware.apply(cpu, &old.settings(settings)) {
            cpu.bus.log_string(&format!("[STATE] {}", warning));
        }
    }
    load_sections(cpu, before).expect("the state the machine was in loads back");
    cpu.forget_caches();
    cpu.bus.after_load();
}

fn load_sections(cpu: &mut Cpu, data: &[u8]) -> Result<Vec<String>> {
    let mut r = Reader::new(data);
    let lost = cpu.bus.load_state(&mut r)?;
    cpu.load(&mut r.section(b"CPU ", CPU_VERSION)?)?;
    // A state from before the record was saved has none: its variables count as the guest's.
    if r.next_is(b"ENV ") {
        cpu.env_injector.load(&mut r.section(b"ENV ", ENV_VERSION)?)?;
    } else {
        cpu.env_injector.forget();
    }
    // A state from before the Pentium II has no SYSENTER MSRs: they were 0.
    if r.next_is(b"SEP ") {
        cpu.sysenter.load(&mut r.section(b"SEP ", SEP_VERSION)?)?;
    } else {
        cpu.sysenter = [0; 3];
    }
    // A state from before the Pentium III has no SSE state: the registers
    // were as after a reset.
    if r.next_is(b"SSE ") {
        cpu.sse.load(&mut r.section(b"SSE ", SSE_VERSION)?)?;
    } else {
        cpu.sse = crate::cpu::Sse::default();
    }
    // A state from before the K6-2 has none of its MSRs: they were 0.
    if r.next_is(b"K6  ") {
        cpu.k6.load(&mut r.section(b"K6  ", K6_VERSION)?)?;
    } else {
        cpu.k6 = crate::cpu::K6Msrs::default();
    }
    // A state from before `cycles=auto` noted an idle loop has none seen.
    if r.next_is(b"IDLE") {
        cpu.os_idles.load(&mut r.section(b"IDLE", IDLE_VERSION)?)?;
    } else {
        cpu.os_idles = false;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whether_the_system_idles_goes_into_the_state() {
        let dir = std::env::temp_dir().join(format!("rust-dos-idle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut cpu = Cpu::new(dir);
        cpu.os_idles = true;
        let (state, parts) = save_parts(&cpu);
        cpu.os_idles = false;
        load(&mut cpu, &state).unwrap();
        assert!(cpu.os_idles);
        // A state from before the section, which is the last, has none seen.
        let idle = parts.iter().find(|(name, _)| name == "IDLE").expect("the section").1.clone();
        assert_eq!(idle.end, state.len());
        load(&mut cpu, &state[..idle.start]).unwrap();
        assert!(!cpu.os_idles);
    }
}
