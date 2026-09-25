//! The MPU-401's MIDI sent out of the host (`midisynth=host`): to a MIDI
//! port of the system, through ALSA, CoreMIDI or Windows's MIDI, where a
//! real MT-32 or Sound Canvas, a software synthesizer such as FluidSynth or
//! munt's own, or anything else listening plays it (`midiport` chooses).

use midir::{MidiOutput, MidiOutputConnection};

/// The ports MIDI can go out of, by name.
pub fn list_ports() -> Vec<String> {
    let Ok(out) = MidiOutput::new("rust-dos") else { return Vec::new() };
    out.ports().iter().filter_map(|p| out.port_name(p).ok()).collect()
}

/// The port `wanted` names among `names`: with nothing asked for, the first
/// that isn't ALSA's "Midi Through", which only passes MIDI on; a number
/// is a port's index; any other text a part of its name, in any case.
pub fn choose_port(names: &[String], wanted: &str) -> Option<usize> {
    let wanted = wanted.trim();
    if wanted.is_empty() || wanted.eq_ignore_ascii_case("default") {
        return names.iter().position(|n| !n.to_ascii_lowercase().contains("midi through")).or_else(|| {
            (!names.is_empty()).then_some(0)
        });
    }
    if let Ok(index) = wanted.parse::<usize>() {
        return (index < names.len()).then_some(index);
    }
    let wanted = wanted.to_ascii_lowercase();
    names.iter().position(|n| n.to_ascii_lowercase().contains(&wanted))
}

/// An open MIDI port.
pub struct HostMidi {
    connection: MidiOutputConnection,
    name: String,
    /// Whether sending failed, which is only told once.
    failed: bool,
}

impl HostMidi {
    /// Open the port `wanted` names (see `choose_port`).
    pub fn open(wanted: &str) -> Result<Self, String> {
        let out = MidiOutput::new("rust-dos").map_err(|e| format!("no MIDI on this system: {}", e))?;
        let ports = out.ports();
        let names: Vec<String> = ports.iter().map(|p| out.port_name(p).unwrap_or_default()).collect();
        let Some(index) = choose_port(&names, wanted) else {
            let listing = if names.is_empty() { "there are none".to_string() } else { names.join(", ") };
            return match wanted.trim() {
                "" => Err(format!("no MIDI port to play through ({})", listing)),
                w => Err(format!("no MIDI port '{}' (the ports: {})", w, listing)),
            };
        };
        let name = names[index].clone();
        let connection = out.connect(&ports[index], "rust-dos").map_err(|e| format!("{}: {}", name, e))?;
        Ok(HostMidi { connection, name, failed: false })
    }

    /// The port's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    fn send(&mut self, bytes: &[u8]) {
        if let Err(e) = self.connection.send(bytes)
            && !self.failed
        {
            self.failed = true;
            eprintln!("[MIDI] Sending to {} failed: {}", self.name, e);
        }
    }

    /// A channel message: two data bytes, or one for program changes and
    /// channel pressure.
    pub fn message(&mut self, status: u8, d1: u8, d2: u8) {
        if matches!(status & 0xF0, 0xC0 | 0xD0) {
            self.send(&[status, d1]);
        } else {
            self.send(&[status, d1, d2]);
        }
    }

    /// A System Exclusive message, without its F0h and F7h.
    pub fn sysex(&mut self, body: &[u8]) {
        let mut framed = Vec::with_capacity(body.len() + 2);
        framed.push(0xF0);
        framed.extend_from_slice(body);
        framed.push(0xF7);
        self.send(&framed);
    }

    /// Silence the notes, as when a program ends: sustain off, controllers
    /// reset and all notes off on every channel.
    pub fn notes_off(&mut self) {
        for channel in 0..16u8 {
            self.message(0xB0 | channel, 64, 0);
            self.message(0xB0 | channel, 121, 0);
            self.message(0xB0 | channel, 123, 0);
        }
    }
}

impl Drop for HostMidi {
    fn drop(&mut self) {
        self.notes_off();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_by_default_index_and_name() {
        let names: Vec<String> =
            ["Midi Through:Midi Through Port-0 14:0", "FLUID Synth (1234):Synth input port 128:0", "UM-ONE:UM-ONE MIDI 1 24:0"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        assert_eq!(choose_port(&names, ""), Some(1));
        assert_eq!(choose_port(&names, "default"), Some(1));
        assert_eq!(choose_port(&names, "0"), Some(0));
        assert_eq!(choose_port(&names, "3"), None);
        assert_eq!(choose_port(&names, "um-one"), Some(2));
        assert_eq!(choose_port(&names, "munt"), None);
        // Only the pass-through port: that one.
        assert_eq!(choose_port(&names[..1], ""), Some(0));
        assert_eq!(choose_port(&[], ""), None);
    }
}
