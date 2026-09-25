//! What a MIDI synthesizer was told that it would need told again to sound
//! the same: the synthesizers (SoundFont, Ultrasound patches, the MT-32,
//! a port of the host) can't be saved, so after a save state is loaded
//! the synthesizer is silenced and this is played to it again.
//!
//! For each channel that is the program with its bank, the controllers,
//! the parameters set through RPNs and NRPNs, the pitch bend and the
//! channel pressure. System Exclusive messages (the MT-32's timbres and
//! setup, GS and XG setups) are kept in the order they came, a later
//! message to the same Roland address replacing an earlier one, up to
//! `SYSEX_BUDGET` bytes.

use std::collections::BTreeMap;

/// Bytes of System Exclusive messages kept; the oldest go first.
const SYSEX_BUDGET: usize = 1 << 20;

/// Controllers that select or set a parameter number, which are played
/// again with the parameter's value rather than as they came.
const DATA_MSB: u8 = 6;
const DATA_LSB: u8 = 38;
const NRPN_LSB: u8 = 98;
const NRPN_MSB: u8 = 99;
const RPN_LSB: u8 = 100;
const RPN_MSB: u8 = 101;
/// Reset All Controllers; 120 and up are channel mode messages.
const RESET_CONTROLLERS: u8 = 121;
const MODE_MESSAGES: u8 = 120;

/// A parameter number: NRPN (true) or RPN, its MSB and LSB.
type Parameter = (bool, u8, u8);

#[derive(Clone, Debug)]
struct Channel {
    /// Controllers set since the last reset of them, by number.
    controllers: [Option<u8>; 128],
    /// The parameter data entry goes to, as the controllers select it.
    parameter: Option<Parameter>,
    /// Values given to parameters: data entry MSB and LSB.
    parameters: BTreeMap<Parameter, (Option<u8>, Option<u8>)>,
    program: Option<u8>,
    bend: Option<(u8, u8)>,
    pressure: Option<u8>,
}

impl Default for Channel {
    fn default() -> Self {
        Self { controllers: [None; 128], parameter: None, parameters: BTreeMap::new(), program: None, bend: None, pressure: None }
    }
}

/// What `replay` plays.
pub enum Midi<'a> {
    /// A channel message: status and data bytes (the second 0 if it has
    /// one).
    Message(u8, u8, u8),
    /// A System Exclusive message, without its F0 and F7.
    Sysex(&'a [u8]),
}

#[derive(Clone, Debug, Default)]
pub struct MidiShadow {
    channels: [Channel; 16],
    sysex: Vec<Vec<u8>>,
}

/// The address a Roland Data Set 1 message writes to, with the model and
/// device: messages to it replace each other.
fn roland_address(body: &[u8]) -> Option<&[u8]> {
    // 41 dev model 12 a a a data... sum (without F0/F7).
    (body.len() > 7 && body[0] == 0x41 && body[3] == 0x12).then(|| &body[1..7])
}

impl MidiShadow {
    /// A channel message the synthesizer got.
    pub fn message(&mut self, status: u8, d1: u8, d2: u8) {
        let channel = &mut self.channels[(status & 0x0F) as usize];
        match status & 0xF0 {
            0xB0 => channel.controller(d1, d2),
            0xC0 => channel.program = Some(d1),
            0xD0 => channel.pressure = Some(d1),
            0xE0 => channel.bend = Some((d1, d2)),
            _ => {}
        }
    }

    /// A System Exclusive message the synthesizer got, without its F0 and
    /// F7.
    pub fn sysex(&mut self, body: &[u8]) {
        match roland_address(body) {
            Some(address) => self.sysex.retain(|kept| roland_address(kept) != Some(address) || kept.len() != body.len()),
            None => self.sysex.retain(|kept| kept.as_slice() != body),
        }
        self.sysex.push(body.to_vec());
        let mut total: usize = self.sysex.iter().map(Vec::len).sum();
        while total > SYSEX_BUDGET {
            total -= self.sysex.remove(0).len();
        }
    }

    /// The synthesizer was reset (a program ended): its channels start
    /// over. Its System Exclusive setup stays, as the MT-32's timbres do.
    pub fn reset_channels(&mut self) {
        self.channels = Default::default();
    }

    /// Play everything to a silenced synthesizer: the System Exclusive
    /// messages, then each channel's state.
    pub fn replay(&self, mut play: impl FnMut(Midi)) {
        for body in &self.sysex {
            play(Midi::Sysex(body));
        }
        let mut message = |status, d1, d2| play(Midi::Message(status, d1, d2));
        for (n, channel) in self.channels.iter().enumerate() {
            let n = n as u8;
            let cc = |message: &mut dyn FnMut(u8, u8, u8), number: u8, value: u8| message(0xB0 | n, number, value);
            // The bank before the program it selects from.
            for number in [0, 32] {
                if let Some(value) = channel.controllers[number as usize] {
                    cc(&mut message, number, value);
                }
            }
            if let Some(program) = channel.program {
                message(0xC0 | n, program, 0);
            }
            for (number, value) in channel.controllers.iter().enumerate() {
                let number = number as u8;
                let parameter_part = matches!(number, DATA_MSB | DATA_LSB | NRPN_LSB..=RPN_MSB | 96 | 97);
                if let Some(value) = value
                    && !matches!(number, 0 | 32)
                    && !parameter_part
                    && number < MODE_MESSAGES
                {
                    cc(&mut message, number, *value);
                }
            }
            for (&(nrpn, msb, lsb), &(data_msb, data_lsb)) in &channel.parameters {
                let (select_msb, select_lsb) = if nrpn { (NRPN_MSB, NRPN_LSB) } else { (RPN_MSB, RPN_LSB) };
                cc(&mut message, select_msb, msb);
                cc(&mut message, select_lsb, lsb);
                if let Some(value) = data_msb {
                    cc(&mut message, DATA_MSB, value);
                }
                if let Some(value) = data_lsb {
                    cc(&mut message, DATA_LSB, value);
                }
            }
            // The parameter selected last, which the program's next data
            // entry goes to; none (7F 7F) if it deselected it.
            if !channel.parameters.is_empty() || channel.parameter.is_some() {
                let (nrpn, msb, lsb) = channel.parameter.unwrap_or((false, 0x7F, 0x7F));
                let (select_msb, select_lsb) = if nrpn { (NRPN_MSB, NRPN_LSB) } else { (RPN_MSB, RPN_LSB) };
                cc(&mut message, select_msb, msb);
                cc(&mut message, select_lsb, lsb);
            }
            if let Some((lsb, msb)) = channel.bend {
                message(0xE0 | n, lsb, msb);
            }
            if let Some(pressure) = channel.pressure {
                message(0xD0 | n, pressure, 0);
            }
        }
    }
}

impl Channel {
    fn controller(&mut self, number: u8, value: u8) {
        match number {
            RESET_CONTROLLERS => {
                // Bank, volume and pan stay, as General MIDI has it.
                let keep = [0usize, 7, 10, 32, 91, 93];
                for (n, controller) in self.controllers.iter_mut().enumerate() {
                    if !keep.contains(&n) {
                        *controller = None;
                    }
                }
                self.parameter = None;
                self.bend = None;
                self.pressure = None;
                return;
            }
            n if n >= MODE_MESSAGES => return,
            NRPN_MSB | NRPN_LSB | RPN_MSB | RPN_LSB => {
                let nrpn = matches!(number, NRPN_MSB | NRPN_LSB);
                let (_, mut msb, mut lsb) = self.parameter.filter(|p| p.0 == nrpn).unwrap_or((nrpn, 0x7F, 0x7F));
                if matches!(number, NRPN_MSB | RPN_MSB) {
                    msb = value;
                } else {
                    lsb = value;
                }
                self.parameter = Some((nrpn, msb, lsb));
            }
            DATA_MSB | DATA_LSB => {
                if let Some(parameter) = self.parameter.filter(|&(_, msb, lsb)| (msb, lsb) != (0x7F, 0x7F)) {
                    let entry = self.parameters.entry(parameter).or_default();
                    if number == DATA_MSB {
                        entry.0 = Some(value);
                    } else {
                        entry.1 = Some(value);
                    }
                }
            }
            _ => {}
        }
        self.controllers[number as usize] = Some(value);
    }
}

impl crate::savestate::State for Channel {
    fn save(&self, w: &mut crate::savestate::Writer) {
        let Channel { controllers, parameter, parameters, program, bend, pressure } = self;
        controllers.save(w);
        parameter.map(|(nrpn, msb, lsb)| (nrpn, (msb, lsb))).save(w);
        parameters.len().save(w);
        for (&(nrpn, msb, lsb), &(data_msb, data_lsb)) in parameters {
            (nrpn, msb, lsb).save(w);
            (data_msb, data_lsb).save(w);
        }
        program.save(w);
        bend.save(w);
        pressure.save(w);
    }
    fn load(&mut self, r: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        let Channel { controllers, parameter, parameters, program, bend, pressure } = self;
        controllers.load(r)?;
        let mut selected: Option<(bool, (u8, u8))> = None;
        selected.load(r)?;
        *parameter = selected.map(|(nrpn, (msb, lsb))| (nrpn, msb, lsb));
        parameters.clear();
        for _ in 0..r.count()? {
            let (mut key, mut value) = ((false, 0u8, 0u8), (None::<u8>, None::<u8>));
            key.load(r)?;
            value.load(r)?;
            parameters.insert(key, value);
        }
        program.load(r)?;
        bend.load(r)?;
        pressure.load(r)?;
        Ok(())
    }
}

crate::state_fields!(MidiShadow { channels, sysex });

#[cfg(test)]
mod tests {
    use super::*;

    fn replayed(shadow: &MidiShadow) -> (Vec<[u8; 3]>, Vec<Vec<u8>>) {
        let (mut messages, mut sysex) = (Vec::new(), Vec::new());
        shadow.replay(|midi| match midi {
            Midi::Message(s, a, b) => messages.push([s, a, b]),
            Midi::Sysex(body) => sysex.push(body.to_vec()),
        });
        (messages, sysex)
    }

    #[test]
    fn channels_are_played_again_in_an_order_that_sets_them_up() {
        let mut shadow = MidiShadow::default();
        // Volume, then a pitch bend range of 12 through RPN 0, then the
        // RPN deselected, as programs do.
        for (number, value) in [(7, 90), (101, 0), (100, 0), (6, 12), (101, 127), (100, 127), (0, 1)] {
            shadow.message(0xB2, number, value);
        }
        shadow.message(0xC2, 30, 0);
        shadow.message(0xE2, 0, 0x50);
        shadow.message(0x92, 60, 100);
        let (messages, _) = replayed(&shadow);
        assert_eq!(
            messages,
            [
                [0xB2, 0, 1],
                [0xC2, 30, 0],
                [0xB2, 7, 90],
                [0xB2, 101, 0],
                [0xB2, 100, 0],
                [0xB2, 6, 12],
                [0xB2, 101, 127],
                [0xB2, 100, 127],
                [0xE2, 0, 0x50],
            ]
        );
        shadow.message(0xB2, RESET_CONTROLLERS, 0);
        let (messages, _) = replayed(&shadow);
        assert!(!messages.contains(&[0xE2, 0, 0x50]) && messages.contains(&[0xB2, 7, 90]));
    }

    #[test]
    fn sysex_to_the_same_address_replaces_the_earlier() {
        let mut shadow = MidiShadow::default();
        let lcd = |c: u8| vec![0x41, 0x10, 0x16, 0x12, 0x20, 0x00, 0x00, c, 0x00];
        let timbre = vec![0x41, 0x10, 0x16, 0x12, 0x08, 0x00, 0x00, 1, 2, 3, 0x00];
        shadow.sysex(&lcd(b'A'));
        shadow.sysex(&timbre);
        shadow.sysex(&lcd(b'B'));
        let (_, sysex) = replayed(&shadow);
        assert_eq!(sysex, [timbre, lcd(b'B')]);
    }
}
