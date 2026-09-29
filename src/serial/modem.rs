//! A Hayes-compatible modem. In command mode it takes AT commands and
//! answers with result codes; `ATD` calls, `ATA` takes a call that rings,
//! and a call that goes through puts it online, where what the program
//! sends goes to the other end until the call ends, or the program sends
//! the escape (`+++`, with a second without data before and after) and it
//! takes commands again.
//!
//! A number with a dot, a colon or a letter in it is a host on the
//! internet (`ATDT example.com:23`, port 23 unless given), which the
//! network thread connects to over TCP; any other number calls the other
//! player in the LAN room, whose modem rings. With another player in the
//! room, a program that sends something other than an AT command, or
//! whose other end does, finds the modem online at once, as over a null
//! modem cable: games that set their modems up with AT commands and those
//! that expect a cable both work on it.

use super::uart::{MCR_DTR, MSR_CTS, MSR_DCD, MSR_DSR, MSR_RI, Uart};
use super::{LinkCmd, LinkEvent};
use crate::timer::PIT_HZ;

/// The S-registers there are, and their values after ATZ.
const S_REGS: usize = 16;
const S_DEFAULTS: [u8; S_REGS] = [0, 0, 43, 13, 10, 8, 2, 50, 2, 6, 14, 95, 50, 0, 0, 0];
/// Rings come every 6 seconds; RI is on for 2 of them.
const RING_TICKS: u64 = 6 * PIT_HZ;
const RI_TICKS: u64 = 2 * PIT_HZ;
/// The longest command line.
const LINE: usize = 80;
/// How much longer the answering modem takes to have the carrier.
const ANSWER_TICKS: u64 = PIT_HZ * 3 / 10;
/// What starts a command, or comes between them: all else a program sends
/// in command mode with another player in the room goes to them.
const COMMAND_START: &[u8] = b"Aa\r\n +~";

/// Result codes: numeric, and in words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Result {
    Ok,
    Connect,
    Ring,
    NoCarrier,
    Error,
    NoAnswer,
}

impl Result {
    fn code(self) -> u8 {
        match self {
            Result::Ok => 0,
            Result::Connect => 1,
            Result::Ring => 2,
            Result::NoCarrier => 3,
            Result::Error => 4,
            Result::NoAnswer => 8,
        }
    }

    fn text(self) -> &'static str {
        match self {
            Result::Ok => "OK",
            Result::Connect => "CONNECT",
            Result::Ring => "RING",
            Result::NoCarrier => "NO CARRIER",
            Result::Error => "ERROR",
            Result::NoAnswer => "NO ANSWER",
        }
    }
}

/// Where a call is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Call {
    #[default]
    None,
    /// Dialed, waiting for the other end until the tick given.
    Dialing(u64),
    /// Ringing here.
    Ringing,
    /// Answered: the carrier is there at the tick given, as the answering
    /// modem's handshake takes a while longer than the caller's. (Two
    /// emulated machines alike otherwise would see the same time when the
    /// call goes through, where games like DOOM's SERSETUP take their
    /// player's number from it.)
    Answering(u64),
    Connected,
}

#[derive(Clone, Debug)]
pub struct Modem {
    echo: bool,
    verbose: bool,
    quiet: bool,
    s: [u8; S_REGS],
    /// &C: DCD follows the carrier (1) or is always on (0). &D: what
    /// dropping DTR does: nothing (0), command mode (1), hang up (2, 3).
    amp_c: u8,
    amp_d: u8,
    /// The command line being typed, and the last one (for `A/`).
    line: Vec<u8>,
    last: Vec<u8>,
    pub call: Call,
    /// Online: what the program sends goes to the other end.
    pub online: bool,
    /// Online to the other player in the room without a call, as over a
    /// null modem cable.
    pub direct: bool,
    /// Another player is in the room.
    pub peer: bool,
    dtr: bool,
    /// When the next ring comes, when RI goes off, and whether it is on.
    ring_due: Option<u64>,
    ri_off: Option<u64>,
    ri: bool,
    /// The escape: how many escape characters came, when the last data
    /// did, and when the escape is complete.
    plus: u8,
    last_data: u64,
    escape_due: Option<u64>,
    /// What came from the other end before the carrier was there here.
    held: Vec<u8>,
}

impl Default for Modem {
    fn default() -> Self {
        Self {
            echo: true,
            verbose: true,
            quiet: false,
            s: S_DEFAULTS,
            amp_c: 1,
            amp_d: 2,
            line: Vec::new(),
            last: Vec::new(),
            call: Call::None,
            online: false,
            direct: false,
            peer: false,
            dtr: false,
            ring_due: None,
            ri_off: None,
            ri: false,
            plus: 0,
            last_data: 0,
            escape_due: None,
            held: Vec::new(),
        }
    }
}

// The call is the network's, not the machine's: it stays as it is.
crate::state_fields!(Modem {
    echo, verbose, quiet, s, amp_c, amp_d, line, last, dtr
} skip { call, online, direct, peer, ring_due, ri_off, ri, plus, last_data, escape_due, held });

impl Modem {
    /// The guard time of the escape: S12, in fiftieths of a second.
    fn guard(&self) -> u64 {
        self.s[12] as u64 * PIT_HZ / 50
    }

    fn carrier(&self) -> bool {
        self.call == Call::Connected || self.direct
    }

    /// The lines to the port: CTS and DSR while the modem is on, DCD with
    /// the carrier (or always, with &C0), RI while it rings.
    fn lines(&self) -> u8 {
        let mut lines = MSR_CTS | MSR_DSR;
        if self.carrier() || self.amp_c == 0 {
            lines |= MSR_DCD;
        }
        if self.ri {
            lines |= MSR_RI;
        }
        lines
    }

    fn say(&self, uart: &mut Uart, text: &str, now: u64) {
        let (cr, lf) = (self.s[3], self.s[4]);
        let mut bytes = vec![cr, lf];
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(&[cr, lf]);
        uart.receive(&bytes, now);
    }

    fn result(&self, uart: &mut Uart, result: Result, now: u64) {
        if self.quiet {
            return;
        }
        if self.verbose {
            let text = match result {
                Result::Connect => format!("CONNECT {}", uart.baud()),
                _ => result.text().to_string(),
            };
            self.say(uart, &text, now);
        } else {
            let mut bytes = result.code().to_string().into_bytes();
            bytes.push(self.s[3]);
            uart.receive(&bytes, now);
        }
    }

    /// Back to command mode with no call: the carrier and the ringing
    /// stop.
    fn drop_call(&mut self) {
        self.call = Call::None;
        self.online = false;
        self.direct = false;
        self.ring_due = None;
        self.ri_off = None;
        self.ri = false;
        self.escape_due = None;
        self.plus = 0;
        self.held.clear();
    }

    /// End the call there is, telling the network.
    fn hang_up(&mut self, link: &mut Vec<LinkCmd>) {
        if matches!(self.call, Call::Dialing(_) | Call::Connected | Call::Ringing | Call::Answering(_)) {
            link.push(LinkCmd::Hangup);
        }
        self.drop_call();
    }

    pub fn control(&mut self, uart: &mut Uart, mcr: u8, now: u64, link: &mut Vec<LinkCmd>) {
        let dtr = mcr & MCR_DTR != 0;
        if self.dtr && !dtr {
            match self.amp_d {
                1 if self.online => {
                    self.online = self.direct;
                    self.result(uart, Result::Ok, now);
                }
                2 | 3 if self.carrier() || self.call != Call::None => self.hang_up(link),
                _ => {}
            }
        }
        self.dtr = dtr;
        uart.set_lines(self.lines());
    }

    /// Characters from the program.
    pub fn transmit(&mut self, uart: &mut Uart, bytes: &[u8], now: u64, link: &mut Vec<LinkCmd>) {
        let mut data = Vec::new();
        for (i, &b) in bytes.iter().enumerate() {
            if self.online {
                self.watch_escape(b, now);
                data.push(b);
                continue;
            }
            // Not a command: with another player there, online to them.
            if self.line.is_empty() && self.peer && self.call == Call::None && !COMMAND_START.contains(&b) {
                self.go_direct(uart, now);
                data.extend_from_slice(&bytes[i..]);
                break;
            }
            self.command_char(uart, b, now, link);
        }
        if !data.is_empty() {
            link.push(LinkCmd::Bytes(data));
        }
        uart.set_lines(self.lines());
    }

    fn go_direct(&mut self, uart: &mut Uart, _now: u64) {
        self.direct = true;
        self.online = true;
        self.plus = 0;
        uart.set_lines(self.lines());
    }

    /// The escape: the escape character three times, with the guard time
    /// without data before the first and after the last.
    fn watch_escape(&mut self, b: u8, now: u64) {
        let escape = self.s[2];
        if escape <= 127 && b == escape && self.plus < 3 && (self.plus > 0 || now >= self.last_data + self.guard()) {
            self.plus += 1;
            if self.plus == 3 {
                self.escape_due = Some(now + self.guard());
            }
        } else {
            self.plus = 0;
            self.escape_due = None;
            self.last_data = now;
        }
    }

    fn command_char(&mut self, uart: &mut Uart, b: u8, now: u64, link: &mut Vec<LinkCmd>) {
        if self.echo && !matches!(self.call, Call::Dialing(_)) {
            uart.receive(&[b], now);
        }
        if let Call::Dialing(_) = self.call {
            // A key while dialing ends the call.
            self.hang_up(link);
            self.result(uart, Result::NoCarrier, now);
            return;
        }
        if b == self.s[3] {
            let line = std::mem::take(&mut self.line);
            if line.len() >= 2 && line[..2].eq_ignore_ascii_case(b"AT") {
                self.last = line.clone();
                self.execute(uart, &line[2..], now, link);
            }
            return;
        }
        if b == self.s[5] {
            self.line.pop();
            return;
        }
        if b < b' ' || self.line.len() >= LINE {
            return;
        }
        self.line.push(b);
        match self.line.as_slice() {
            [a] if !a.eq_ignore_ascii_case(&b'A') => self.line.clear(),
            [_, b'/'] => {
                self.line.clear();
                let last = self.last.clone();
                if last.len() >= 2 {
                    self.execute(uart, &last[2..], now, link);
                }
            }
            [_, t] if !t.eq_ignore_ascii_case(&b'T') => self.line.clear(),
            _ => {}
        }
    }

    /// Run the commands after `AT`.
    fn execute(&mut self, uart: &mut Uart, commands: &[u8], now: u64, link: &mut Vec<LinkCmd>) {
        let text = String::from_utf8_lossy(commands).to_string();
        let mut chars = text.chars().peekable();
        let number = |chars: &mut std::iter::Peekable<std::str::Chars>| {
            let mut n: Option<u32> = None;
            while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
                n = Some(n.unwrap_or(0).saturating_mul(10).saturating_add(d));
                chars.next();
            }
            n
        };
        while let Some(c) = chars.next() {
            match c.to_ascii_uppercase() {
                ' ' => {}
                'D' => {
                    let rest: String = chars.collect();
                    return self.dial(uart, &rest, now, link);
                }
                'A' => {
                    if self.call == Call::Ringing {
                        link.push(LinkCmd::Answer);
                        self.ring_due = None;
                        self.ri = false;
                        self.ri_off = None;
                        uart.set_lines(self.lines());
                    } else {
                        self.result(uart, Result::NoCarrier, now);
                    }
                    return;
                }
                'H' => {
                    number(&mut chars);
                    self.hang_up(link);
                    uart.set_lines(self.lines());
                }
                'O' => {
                    number(&mut chars);
                    if self.carrier() {
                        self.online = true;
                        self.result(uart, Result::Connect, now);
                    } else {
                        self.result(uart, Result::NoCarrier, now);
                    }
                    return;
                }
                'Z' => {
                    number(&mut chars);
                    self.hang_up(link);
                    self.reset_settings();
                    uart.set_lines(self.lines());
                }
                'E' => self.echo = number(&mut chars).unwrap_or(0) != 0,
                'V' => self.verbose = number(&mut chars).unwrap_or(0) != 0,
                'Q' => self.quiet = number(&mut chars).unwrap_or(0) != 0,
                'X' | 'L' | 'M' | 'B' | 'N' | 'W' | 'P' | 'T' | 'Y' => {
                    number(&mut chars);
                }
                'I' => {
                    let n = number(&mut chars).unwrap_or(0);
                    let info = match n {
                        0 => "56000".to_string(),
                        3 => "rust-dos modem".to_string(),
                        _ => "OK".to_string(),
                    };
                    self.say(uart, &info, now);
                }
                'S' => {
                    let Some(n) = number(&mut chars) else {
                        return self.result(uart, Result::Error, now);
                    };
                    match chars.next() {
                        Some('=') => {
                            let value = number(&mut chars).unwrap_or(0).min(255) as u8;
                            if let Some(r) = self.s.get_mut(n as usize) {
                                *r = value;
                            }
                        }
                        Some('?') => {
                            let value = self.s.get(n as usize).copied().unwrap_or(0);
                            self.say(uart, &format!("{:03}", value), now);
                        }
                        _ => return self.result(uart, Result::Error, now),
                    }
                }
                '&' => {
                    let Some(c) = chars.next() else { return self.result(uart, Result::Error, now) };
                    let n = number(&mut chars).unwrap_or(0).min(9) as u8;
                    match c.to_ascii_uppercase() {
                        'C' => self.amp_c = n,
                        'D' => self.amp_d = n,
                        'F' => {
                            self.reset_settings();
                            uart.set_lines(self.lines());
                        }
                        _ => {}
                    }
                }
                '\\' | '%' | '+' | '#' => {
                    // Error correction, compression and the like: nothing
                    // to set up here.
                    chars.next();
                    number(&mut chars);
                    if chars.peek() == Some(&'=') {
                        chars.next();
                        while chars.peek().is_some_and(|c| c.is_ascii_alphanumeric() || *c == ',') {
                            chars.next();
                        }
                    }
                }
                _ => return self.result(uart, Result::Error, now),
            }
        }
        self.result(uart, Result::Ok, now);
    }

    fn reset_settings(&mut self) {
        let dtr = self.dtr;
        let peer = self.peer;
        *self = Self { dtr, peer, ..Self::default() };
    }

    /// `ATD` and what follows it: a host on the internet, or the other
    /// player in the room.
    fn dial(&mut self, uart: &mut Uart, rest: &str, now: u64, link: &mut Vec<LinkCmd>) {
        if self.call != Call::None {
            return self.result(uart, Result::Error, now);
        }
        let mut number = rest.trim().trim_end_matches(';').trim();
        if let Some(n) = number.strip_prefix(['T', 't', 'P', 'p']) {
            number = n.trim();
        }
        let host = number.contains(['.', ':']) || number.chars().any(|c| c.is_ascii_alphabetic() && !"TPWtpw".contains(c));
        let target = if host {
            let address: String = number.chars().filter(|c| !c.is_whitespace()).collect();
            let address = if address.contains(':') { address } else { format!("{}:23", address) };
            Some(address)
        } else if self.peer {
            None
        } else {
            return self.result(uart, Result::NoCarrier, now);
        };
        link.push(LinkCmd::Dial(target));
        self.call = Call::Dialing(now + self.s[7].max(1) as u64 * PIT_HZ);
    }

    /// Something from the network.
    pub fn event(&mut self, uart: &mut Uart, event: LinkEvent, now: u64, link: &mut Vec<LinkCmd>) {
        match event {
            LinkEvent::Peer(there) => {
                self.peer = there;
                if !there && self.direct {
                    self.drop_call();
                    self.result(uart, Result::NoCarrier, now);
                }
            }
            LinkEvent::Connected => match self.call {
                Call::Dialing(_) => self.connect(uart, now),
                Call::Ringing => {
                    self.call = Call::Answering(now + ANSWER_TICKS);
                    self.ring_due = None;
                    self.ri_off = None;
                    self.ri = false;
                }
                _ => {}
            },
            LinkEvent::NoCarrier => match self.call {
                Call::Ringing => self.drop_call(),
                Call::Dialing(_) | Call::Connected | Call::Answering(_) => {
                    self.drop_call();
                    self.result(uart, Result::NoCarrier, now);
                }
                Call::None => {}
            },
            LinkEvent::Ring => {
                if self.call == Call::None && !self.direct {
                    self.call = Call::Ringing;
                    self.s[1] = 0;
                    self.ring_due = Some(now);
                    self.advance(uart, now, link);
                } else {
                    // Busy.
                    link.push(LinkCmd::Hangup);
                }
            }
            LinkEvent::Bytes(bytes) => {
                if self.call == Call::None && !self.direct && self.peer {
                    // The other player is online without a call: so is
                    // this end.
                    self.go_direct(uart, now);
                }
                if self.carrier() {
                    uart.receive(&bytes, now);
                } else if matches!(self.call, Call::Answering(_)) && self.held.len() < 64 * 1024 {
                    self.held.extend_from_slice(&bytes);
                }
            }
            LinkEvent::Lines { .. } => {}
        }
        uart.set_lines(self.lines());
    }

    /// The call is through: CONNECT, and online.
    fn connect(&mut self, uart: &mut Uart, now: u64) {
        self.call = Call::Connected;
        self.online = true;
        self.direct = false;
        self.ring_due = None;
        self.ri_off = None;
        self.ri = false;
        self.last_data = now;
        self.plus = 0;
        self.result(uart, Result::Connect, now);
        let held = std::mem::take(&mut self.held);
        uart.receive(&held, now);
    }

    /// The program ended: online to the other player without a call ends
    /// with it (a call stays, as a modem's does).
    pub fn program_ended(&mut self, uart: &mut Uart) {
        if self.direct {
            self.drop_call();
            self.line.clear();
            uart.set_lines(self.lines());
        }
    }

    pub fn next_event(&self) -> Option<u64> {
        let dialing = match self.call {
            Call::Dialing(until) | Call::Answering(until) => Some(until),
            _ => None,
        };
        [self.ring_due, self.ri_off, self.escape_due, dialing].into_iter().flatten().min()
    }

    pub fn advance(&mut self, uart: &mut Uart, now: u64, link: &mut Vec<LinkCmd>) {
        if let Some(due) = self.ri_off
            && due <= now
        {
            self.ri_off = None;
            self.ri = false;
        }
        if let Some(due) = self.ring_due
            && due <= now
            && self.call == Call::Ringing
        {
            self.s[1] = self.s[1].saturating_add(1);
            self.ri = true;
            self.ri_off = Some(now + RI_TICKS);
            self.ring_due = Some(now + RING_TICKS);
            self.result(uart, Result::Ring, now);
            if self.s[0] > 0 && self.s[1] >= self.s[0] {
                link.push(LinkCmd::Answer);
                self.ring_due = None;
            }
        }
        if let Some(due) = self.escape_due
            && due <= now
        {
            self.escape_due = None;
            self.plus = 0;
            self.online = false;
            self.result(uart, Result::Ok, now);
        }
        if let Call::Answering(due) = self.call
            && due <= now
        {
            self.connect(uart, now);
        }
        if let Call::Dialing(until) = self.call
            && until <= now
        {
            self.hang_up(link);
            self.result(uart, Result::NoAnswer, now);
        }
        uart.set_lines(self.lines());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serial::uart::{Chip, MCR_RTS};

    struct Rig {
        uart: Uart,
        modem: Modem,
        link: Vec<LinkCmd>,
        now: u64,
    }

    impl Rig {
        fn new() -> Self {
            let mut rig = Rig { uart: Uart::new(0x2F8, 3, Chip::Ns16550), modem: Modem::default(), link: Vec::new(), now: 0 };
            rig.modem.control(&mut rig.uart, MCR_DTR | MCR_RTS, 0, &mut rig.link);
            rig
        }

        fn send(&mut self, text: &str) {
            self.modem.transmit(&mut self.uart, text.as_bytes(), self.now, &mut self.link);
        }

        fn event(&mut self, event: LinkEvent) {
            self.modem.event(&mut self.uart, event, self.now, &mut self.link);
        }

        fn wait(&mut self, ticks: u64) {
            self.now += ticks;
            self.modem.advance(&mut self.uart, self.now, &mut self.link);
        }

        fn output(&mut self) -> String {
            let mut out = Vec::new();
            loop {
                self.now += self.uart.char_ticks();
                match self.uart.take(self.now) {
                    Some(b) => out.push(b),
                    None if self.uart.rx_idle() => break,
                    None => {}
                }
            }
            String::from_utf8_lossy(&out).to_string()
        }
    }

    #[test]
    fn commands_and_results() {
        let mut rig = Rig::new();
        rig.send("ATZ\r");
        assert_eq!(rig.output(), "ATZ\r\r\nOK\r\n");
        rig.send("ATE0 S0=2 S0?\r");
        assert_eq!(rig.output(), "ATE0 S0=2 S0?\r\r\n002\r\n\r\nOK\r\n");
        rig.send("ATV0\r");
        assert_eq!(rig.output(), "0\r");
        rig.send("ATQ\r");
        assert_eq!(rig.output(), "0\r");
        rig.send("ATV1&C1&D2\\N3%C0X4\r");
        assert_eq!(rig.output(), "\r\nOK\r\n");
        rig.send("ATK?\r");
        assert_eq!(rig.output(), "\r\nERROR\r\n");
        rig.send("A/");
        assert_eq!(rig.output(), "\r\nERROR\r\n");
        // Without another player, a number goes nowhere.
        rig.send("ATDT5551234\r");
        assert_eq!(rig.output(), "\r\nNO CARRIER\r\n");
        assert!(rig.link.iter().all(|c| matches!(c, LinkCmd::Lines { .. })));
    }

    #[test]
    fn dialing_a_host_and_the_escape() {
        let mut rig = Rig::new();
        rig.send("ATE0\r");
        rig.output();
        rig.link.clear();
        rig.send("ATDT example.com\r");
        assert_eq!(rig.link, [LinkCmd::Dial(Some("example.com:23".into()))]);
        rig.link.clear();
        rig.event(LinkEvent::Connected);
        assert_eq!(rig.output(), "\r\nCONNECT 9600\r\n");
        assert_ne!(rig.uart.lines() & MSR_DCD, 0);
        rig.send("hi");
        assert_eq!(rig.link, [LinkCmd::Bytes(b"hi".to_vec())]);
        rig.link.clear();
        rig.wait(PIT_HZ + 1);
        rig.send("+");
        rig.send("+");
        rig.send("+");
        rig.wait(PIT_HZ / 2);
        assert!(rig.modem.online);
        rig.wait(PIT_HZ);
        assert!(!rig.modem.online);
        assert_eq!(rig.output(), "\r\nOK\r\n");
        rig.link.clear();
        rig.send("ATH\r");
        assert_eq!(rig.link, [LinkCmd::Hangup]);
        assert_eq!(rig.uart.lines() & MSR_DCD, 0);
    }

    #[test]
    fn ringing_and_auto_answer() {
        let mut rig = Rig::new();
        rig.send("ATE0S0=2\r");
        rig.output();
        rig.link.clear();
        rig.event(LinkEvent::Ring);
        assert_eq!(rig.output(), "\r\nRING\r\n");
        assert!(rig.link.is_empty());
        rig.wait(RING_TICKS);
        assert_eq!(rig.link, [LinkCmd::Answer]);
        rig.event(LinkEvent::Connected);
        // What the caller sends before the carrier is there here waits.
        rig.event(LinkEvent::Bytes(b"early".to_vec()));
        assert!(rig.output().ends_with("RING\r\n"));
        rig.wait(ANSWER_TICKS);
        assert_eq!(rig.output(), "\r\nCONNECT 9600\r\nearly");
        rig.event(LinkEvent::Bytes(b"x".to_vec()));
        assert_eq!(rig.output(), "x");
        rig.event(LinkEvent::NoCarrier);
        assert_eq!(rig.output(), "\r\nNO CARRIER\r\n");
    }

    #[test]
    fn room_player_without_commands() {
        let mut rig = Rig::new();
        rig.event(LinkEvent::Peer(true));
        rig.link.clear();
        // A game that expects a cable sends its own data.
        rig.send("\r\x05hello");
        assert_eq!(rig.link, [LinkCmd::Bytes(b"\x05hello".to_vec())]);
        assert!(rig.modem.direct);
        assert_ne!(rig.uart.lines() & MSR_DCD, 0);
        rig.event(LinkEvent::Peer(false));
        assert!(!rig.modem.online);
        assert!(rig.output().ends_with("NO CARRIER\r\n"));
        // Or the other end does.
        let mut rig = Rig::new();
        rig.event(LinkEvent::Peer(true));
        rig.event(LinkEvent::Bytes(b"abc".to_vec()));
        assert_eq!(rig.output(), "abc");
        // A game that dials calls them; one that went online without a
        // call leaves the modem in command mode when it ends.
        rig.event(LinkEvent::Peer(true));
        rig.send("x");
        assert!(rig.modem.direct);
        rig.modem.program_ended(&mut rig.uart);
        assert!(!rig.modem.online && !rig.modem.direct);
        let mut rig = Rig::new();
        rig.event(LinkEvent::Peer(true));
        rig.link.clear();
        rig.send("ATDT555\r");
        assert_eq!(rig.link, [LinkCmd::Dial(None)]);
    }
}
