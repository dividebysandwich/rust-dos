//! Save states as the frontend keeps them: in a buffer of the size the
//! core gives it, for its slots, rewinding and run-ahead. The machine's
//! state as the save state files have it (savestate/machine.rs), with the
//! header of the slots (savestate/slots.rs) for the hardware it was saved
//! with, but not compressed, which rewinding would wait for every frame.

use rust_dos::config_ui::Host;
use rust_dos::savestate::{self, slots};

use crate::host::Machine;

const MAGIC: &[u8; 4] = b"RDLR";
const FORMAT: u32 = 1;
/// The size the buffer is rounded up to, with room for the state to grow
/// (the programs' open files, a device's buffers).
const ROUND: usize = 1 << 20;

/// The state of the machine now: the header's JSON and the machine's
/// state, each after its length.
fn snapshot(m: &Machine) -> Vec<u8> {
    let game = m.game.as_ref().map(|g| (g.id.as_str(), g.name.as_str()));
    let header = slots::header(&m.cpu, &m.hardware.settings(&m.settings), game);
    let json = encoded_header(&header);
    let state = savestate::machine::save(&m.cpu);
    let mut out = Vec::with_capacity(16 + json.len() + state.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT.to_le_bytes());
    for part in [&json[..], &state[..]] {
        out.extend_from_slice(&(part.len() as u32).to_le_bytes());
        out.extend_from_slice(part);
    }
    out
}

fn encoded_header(header: &slots::Header) -> Vec<u8> {
    // The slots' own encoding, with no picture and the state left out.
    let encoded = slots::encode(header, &[], &[]);
    let (_, _, at) = slots::read_header(&encoded).expect("a header just encoded");
    encoded[..at].to_vec()
}

/// The size of the buffer for a state: the state now, with room to grow.
pub fn size(m: &Machine) -> usize {
    let n = snapshot(m).len();
    (n + n / 8 + ROUND).div_ceil(ROUND) * ROUND
}

/// Save the machine into `buf`, zeros after it. False if it doesn't fit.
pub fn save(m: &Machine, buf: &mut [u8]) -> bool {
    let data = snapshot(m);
    if data.len() > buf.len() {
        return false;
    }
    buf[..data.len()].copy_from_slice(&data);
    buf[data.len()..].fill(0);
    true
}

/// Load the state in `buf`: the hardware it was saved with first, then
/// the machine. A state of another memory size is refused.
pub fn load(m: &mut Machine, buf: &[u8]) -> Result<slots::Header, String> {
    let bad = || "not a rust-dos save state".to_string();
    if buf.get(..4) != Some(&MAGIC[..]) {
        return Err(bad());
    }
    let format = u32::from_le_bytes(buf.get(4..8).ok_or_else(bad)?.try_into().unwrap());
    if format != FORMAT {
        return Err(format!("a save state of another rust-dos (format {})", format));
    }
    let mut at = 8;
    let mut part = || -> Result<&[u8], String> {
        let len = u32::from_le_bytes(buf.get(at..at + 4).ok_or_else(bad)?.try_into().unwrap()) as usize;
        let bytes = buf.get(at + 4..at + 4 + len).ok_or_else(bad)?;
        at += 4 + len;
        Ok(bytes)
    };
    let header_bytes = part()?;
    let state = part()?;
    let (header, _, _) = slots::read_header(header_bytes)?;
    if let Some(why) = slots::refusal(&header, m.cpu.bus.ram().len() >> 20) {
        return Err(why);
    }
    let hardware = slots::machine_settings(&header.machine, &m.settings);
    if let Err(e) = m.apply(&hardware) {
        m.warn(&e);
    }
    if m.hardware.differs(&m.settings) {
        let settings = m.settings.clone();
        for warning in m.hardware.apply(&mut m.cpu, &settings) {
            m.warn(&warning);
        }
    }
    savestate::machine::load(&mut m.cpu, state).map_err(|e| e.to_string())?;
    Ok(header)
}
