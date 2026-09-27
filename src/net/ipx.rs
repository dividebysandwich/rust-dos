//! The IPX driver of the built-in DOS, as Novell's IPX.COM and IPXODI
//! present it to programs: found through INT 2Fh AX=7A00h (or INT 7Ah),
//! called with BX the function and ES:SI an event control block (ECB).
//! It is a network card of its own on the instance's switch, with a random
//! address that is its IPX node number, and sends its packets as Ethernet
//! frames (Ethernet II by default), so the IPX drivers of booted systems on
//! other instances' network cards hear it as they would a PC on the same
//! Ethernet.
//!
//! What a program asks to be told of (a packet it sent or received, an
//! event it scheduled) completes in an interrupt, as with the real driver:
//! the driver raises its IRQ, whose handler in the ROM (`bios::IPX_IRQ`)
//! takes each completed ECB from `esr_service`, which fills in what came,
//! and calls the ECB's event service routine (ESR) if it has one.
//! Programs that poll the ECB's in-use flag instead find it cleared there,
//! or at their next call of the driver.

use super::frame::{self, ETHERTYPE_IPX, Mac};
use crate::cpu::{Cpu, CpuFlags};
use std::collections::VecDeque;

/// The IPX header before each packet's data.
pub const HEADER: usize = 30;
/// Where the ECB keeps what the driver reads and writes.
const ECB_ESR: u16 = 4;
const ECB_IN_USE: u16 = 8;
const ECB_COMPLETION: u16 = 9;
const ECB_SOCKET: u16 = 10;
const ECB_IMMEDIATE: u16 = 28;
const ECB_FRAGMENT_COUNT: u16 = 34;
const ECB_FRAGMENTS: u16 = 36;
/// In-use flags: the ECB is free, counting down an event, listening.
const AVAILABLE: u8 = 0x00;
const AES_COUNTING: u8 = 0xFD;
const LISTENING: u8 = 0xFE;
/// Completion codes.
const SUCCESS: u8 = 0x00;
const CANNOT_CANCEL: u8 = 0xF9;
const CANCELLED: u8 = 0xFC;
const MALFORMED: u8 = 0xFD;
const FAILURE: u8 = 0xFF;
/// Sockets open at once, as DOSBox allows.
const MAX_SOCKETS: usize = 150;
/// Where dynamic socket numbers start (DOSBox's first).
const DYNAMIC_SOCKETS: u16 = 0x4002;
/// Packets kept for a socket with no ECB listening, and for how long: a
/// burst of packets arrives between batches, and a program with one ECB
/// takes them one at a time.
const HELD_PER_SOCKET: usize = 16;
const HOLD_TICKS: u64 = crate::timer::PIT_HZ / 10;
/// PIT ticks in one of the BIOS's ticks, the unit of AES delays.
const BIOS_TICK: u64 = 65536;
/// The packet sizes functions 0Dh and 1Ah report, as DOSBox's driver does.
const PACKET_SIZE: u16 = 1024;
const MAX_PACKET_SIZE: u16 = 1424;

/// How packets go into Ethernet frames (`ipxframe`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FrameType {
    /// EtherType 8137h: what Windows 95 and ODI drivers call Ethernet_II.
    #[default]
    EthernetII,
    /// Novell's raw 802.3: the IPX packet right after the length.
    Raw8023,
    /// 802.2 LLC with the IPX SAP (E0h).
    Llc8022,
    /// 802.2 SNAP with EtherType 8137h.
    Snap,
}

impl FrameType {
    pub const ALL: [FrameType; 4] = [FrameType::EthernetII, FrameType::Raw8023, FrameType::Llc8022, FrameType::Snap];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
            "ethernet_ii" | "ethernet2" | "ethernet_2" | "ethernetii" => Some(FrameType::EthernetII),
            "802.3" | "ethernet_802.3" | "raw" => Some(FrameType::Raw8023),
            "802.2" | "ethernet_802.2" | "llc" => Some(FrameType::Llc8022),
            "snap" | "ethernet_snap" => Some(FrameType::Snap),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            FrameType::EthernetII => "ethernet_ii",
            FrameType::Raw8023 => "802.3",
            FrameType::Llc8022 => "802.2",
            FrameType::Snap => "snap",
        }
    }

    /// The bytes before the IPX packet after the length or EtherType.
    fn prefix(self) -> &'static [u8] {
        match self {
            FrameType::EthernetII | FrameType::Raw8023 => &[],
            FrameType::Llc8022 => &[0xE0, 0xE0, 0x03],
            FrameType::Snap => &[0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00, 0x81, 0x37],
        }
    }

    /// The longest IPX packet that fits in a frame.
    pub fn max_packet(self) -> usize {
        frame::MAX_FRAME - frame::HEADER - self.prefix().len()
    }
}

/// `packet` in a frame of `kind` from `source` to `destination`.
pub fn encapsulate(kind: FrameType, destination: Mac, source: Mac, packet: &[u8]) -> Vec<u8> {
    let prefix = kind.prefix();
    let mut payload = Vec::with_capacity(prefix.len() + packet.len());
    payload.extend_from_slice(prefix);
    payload.extend_from_slice(packet);
    let field = match kind {
        FrameType::EthernetII => ETHERTYPE_IPX,
        _ => payload.len() as u16,
    };
    frame::build(destination, source, field, &payload)
}

/// The IPX packet in `frame`, in any of the four frame types, without the
/// padding of short frames.
pub fn decapsulate(frame: &[u8]) -> Option<&[u8]> {
    let field = frame::ethertype(frame)?;
    let payload = &frame[frame::HEADER..];
    let packet = if field == ETHERTYPE_IPX {
        payload
    } else if field <= 1500 {
        let payload = payload.get(..field as usize)?;
        match payload {
            [0xFF, 0xFF, ..] => payload,
            [0xE0, 0xE0, 0x03, rest @ ..] => rest,
            [0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00, 0x81, 0x37, rest @ ..] => rest,
            _ => return None,
        }
    } else {
        return None;
    };
    let length = u16::from_be_bytes(packet.get(2..4)?.try_into().ok()?) as usize;
    if length < HEADER {
        return None;
    }
    packet.get(..length)
}

/// An ECB's address, segment and offset.
pub type Far = (u16, u16);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Socket {
    pub number: u16,
    /// Long-lived sockets stay open when their program ends (TSRs).
    pub long_lived: bool,
    /// The PSP of the program that opened it.
    pub owner: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listen {
    pub ecb: Far,
    pub socket: u16,
    pub owner: u16,
}

/// A scheduled event (functions 05h and 07h), due at a PIT tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Event {
    pub ecb: Far,
    pub due: u64,
    pub owner: u16,
}

/// What completed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Done {
    /// A packet was sent.
    #[default]
    Sent,
    /// An event came due.
    Event,
    /// A packet came for a listening ECB, from a card's address.
    Received { packet: Vec<u8>, from: Mac },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Completion {
    pub ecb: Far,
    pub socket: u16,
    pub owner: u16,
    pub done: Done,
}

/// A packet that came for an open socket without an ECB listening.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub socket: u16,
    pub packet: Vec<u8>,
    pub from: Mac,
    pub until: u64,
}

#[derive(Clone, Debug)]
pub struct Ipx {
    /// The node address, which is also the address its frames come from.
    pub node: Mac,
    /// The IRQ its completions come in.
    pub irq: u8,
    pub frame_type: FrameType,
    pub sockets: Vec<Socket>,
    pub listens: Vec<Listen>,
    pub events: Vec<Event>,
    pub completions: VecDeque<Completion>,
    pub held: VecDeque<Held>,
    /// Frames to send, for the bus to hand to the network.
    pub outgoing: Vec<Vec<u8>>,
    /// Lines for the log.
    pub log: Vec<String>,
    /// Whether a socket was opened since the bus last set up the IRQ.
    pub arm: bool,
    /// What became of the packets, for the debugger.
    pub stats: IpxStats,
}

/// Packets sent and what became of those that came.
#[derive(Clone, Copy, Debug, Default)]
pub struct IpxStats {
    pub sent: u64,
    /// To a listening ECB, or held for one.
    pub received: u64,
    /// For another node.
    pub not_ours: u64,
    /// For a socket that isn't open.
    pub closed_socket: u64,
    /// Held too long, or too many, with no ECB listening.
    pub unheard: u64,
}

impl Default for Ipx {
    fn default() -> Self {
        Self::new(Mac::random_local(), 11, FrameType::default())
    }
}

crate::state_fields!(Ipx { node, irq, frame_type, sockets, listens, events, completions, held } skip { outgoing, log, arm, stats });
crate::state_fields!(Socket { number, long_lived, owner });
crate::state_fields!(Listen { ecb, socket, owner });
crate::state_fields!(Event { ecb, due, owner });
crate::state_fields!(Completion { ecb, socket, owner, done });
crate::state_fields!(Held { socket, packet, from, until });
crate::state_enum!(FrameType { FrameType::EthernetII, FrameType::Raw8023, FrameType::Llc8022, FrameType::Snap });

impl crate::savestate::State for Mac {
    fn save(&self, w: &mut crate::savestate::Writer) {
        self.0.save(w);
    }
    fn load(&mut self, r: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        self.0.load(r)
    }
}

impl crate::savestate::State for Done {
    fn save(&self, w: &mut crate::savestate::Writer) {
        match self {
            Done::Sent => 0u8.save(w),
            Done::Event => 1u8.save(w),
            Done::Received { packet, from } => {
                2u8.save(w);
                packet.save(w);
                from.save(w);
            }
        }
    }
    fn load(&mut self, r: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        let mut kind = 0u8;
        kind.load(r)?;
        *self = match kind {
            0 => Done::Sent,
            1 => Done::Event,
            _ => {
                let (mut packet, mut from) = (Vec::new(), Mac::default());
                packet.load(r)?;
                from.load(r)?;
                Done::Received { packet, from }
            }
        };
        Ok(())
    }
}

/// A program's call of the driver, at its entry point or through INT 7Ah.
pub fn api(cpu: &mut Cpu) {
    let Some(mut ipx) = cpu.bus.net.ipx.take() else { return };
    ipx.call(cpu);
    cpu.bus.net.ipx = Some(ipx);
    cpu.bus.sync_ipx();
}

/// The IRQ handler's service (`Ipx::esr_service`).
pub fn esr(cpu: &mut Cpu) {
    let Some(mut ipx) = cpu.bus.net.ipx.take() else {
        cpu.set_cpu_flag(CpuFlags::CF, true);
        return;
    };
    ipx.esr_service(cpu);
    cpu.bus.net.ipx = Some(ipx);
    cpu.bus.sync_ipx();
}

/// The linear address of `offset` bytes into the ECB or buffer at `at`.
fn lin((segment, base): Far, offset: u16) -> u32 {
    ((segment as u32) << 4) + base.wrapping_add(offset) as u32
}

fn set_al(cpu: &mut Cpu, value: u8) {
    cpu.set_reg8(iced_x86::Register::AL, value);
}

fn read_u16(cpu: &mut Cpu, at: Far, offset: u16) -> u16 {
    cpu.bus.guest_read_16(lin(at, offset))
}

fn read_far(cpu: &mut Cpu, at: Far, offset: u16) -> Far {
    (read_u16(cpu, at, offset + 2), read_u16(cpu, at, offset))
}

fn read_bytes(cpu: &mut Cpu, at: Far, offset: u16, len: usize) -> Vec<u8> {
    let mut data = vec![0; len];
    cpu.bus.guest_read_bytes(lin(at, offset), &mut data);
    data
}

fn write_u8(cpu: &mut Cpu, at: Far, offset: u16, value: u8) {
    cpu.bus.guest_write_8(lin(at, offset), value);
}

fn write_bytes(cpu: &mut Cpu, at: Far, offset: u16, data: &[u8]) {
    cpu.bus.guest_write_bytes(lin(at, offset), data);
}

/// The socket number an ECB is for, which it keeps high byte first.
fn ecb_socket(cpu: &mut Cpu, ecb: Far) -> u16 {
    read_u16(cpu, ecb, ECB_SOCKET).swap_bytes()
}

/// An ECB's fragments: where each is and how long.
fn fragments(cpu: &mut Cpu, ecb: Far) -> Vec<(Far, usize)> {
    let count = read_u16(cpu, ecb, ECB_FRAGMENT_COUNT).min(64);
    (0..count)
        .map(|i| {
            let at = ECB_FRAGMENTS + 6 * i;
            (read_far(cpu, ecb, at), read_u16(cpu, ecb, at + 4) as usize)
        })
        .collect()
}

fn set_flags(cpu: &mut Cpu, ecb: Far, in_use: u8, completion: u8) {
    write_u8(cpu, ecb, ECB_COMPLETION, completion);
    write_u8(cpu, ecb, ECB_IN_USE, in_use);
}

/// Whether an ECB and its fragment list can be written, so a service
/// doesn't act on half a change before a page fault runs it again.
fn ecb_writable(cpu: &mut Cpu, ecb: Far) -> bool {
    let count = read_u16(cpu, ecb, ECB_FRAGMENT_COUNT).min(64) as usize;
    cpu.bus.guest_probe(lin(ecb, 0), ECB_FRAGMENTS as usize + 6 * count, true)
}

impl Ipx {
    pub fn new(node: Mac, irq: u8, frame_type: FrameType) -> Self {
        Self {
            node,
            irq,
            frame_type,
            sockets: Vec::new(),
            listens: Vec::new(),
            events: Vec::new(),
            completions: VecDeque::new(),
            held: VecDeque::new(),
            outgoing: Vec::new(),
            log: Vec::new(),
            arm: false,
            stats: IpxStats::default(),
        }
    }

    pub fn is_open(&self, socket: u16) -> bool {
        self.sockets.iter().any(|s| s.number == socket)
    }

    /// Whether completions wait for the IRQ's handler.
    pub fn wants_irq(&self) -> bool {
        !self.completions.is_empty()
    }

    /// The next PIT tick something comes due at: an event, or the end of
    /// a held packet's wait.
    pub fn next_event(&self) -> Option<u64> {
        let events = self.events.iter().map(|e| e.due);
        let held = self.held.iter().map(|h| h.until);
        events.chain(held).min()
    }

    /// Complete the events due at PIT tick `now` and give up on the held
    /// packets no one came for.
    pub fn advance(&mut self, now: u64) {
        let mut i = 0;
        while i < self.events.len() {
            if self.events[i].due <= now {
                let event = self.events.remove(i);
                self.completions.push_back(Completion {
                    ecb: event.ecb,
                    socket: 0,
                    owner: event.owner,
                    done: Done::Event,
                });
            } else {
                i += 1;
            }
        }
        let before = self.held.len();
        self.held.retain(|h| h.until > now);
        self.stats.unheard += (before - self.held.len()) as u64;
    }

    /// A frame from the network at PIT tick `now`.
    pub fn receive_frame(&mut self, frame: &[u8], now: u64) {
        if let (Some(packet), Some(from)) = (decapsulate(frame), frame::source(frame)) {
            self.receive(packet.to_vec(), from, now);
        }
    }

    /// An IPX packet for this node (or all), from the card at `from`.
    fn receive(&mut self, packet: Vec<u8>, from: Mac, now: u64) {
        if packet.len() < HEADER {
            return;
        }
        let destination = Mac(packet[10..16].try_into().unwrap());
        if destination != self.node && !destination.is_broadcast() {
            self.stats.not_ours += 1;
            return;
        }
        let socket = u16::from_be_bytes([packet[16], packet[17]]);
        if !self.is_open(socket) {
            self.stats.closed_socket += 1;
            return;
        }
        self.stats.received += 1;
        if let Some(i) = self.listens.iter().position(|l| l.socket == socket) {
            let listen = self.listens.remove(i);
            self.completions.push_back(Completion {
                ecb: listen.ecb,
                socket,
                owner: listen.owner,
                done: Done::Received { packet, from },
            });
            return;
        }
        if self.held.iter().filter(|h| h.socket == socket).count() >= HELD_PER_SOCKET
            && let Some(oldest) = self.held.iter().position(|h| h.socket == socket)
        {
            self.held.remove(oldest);
            self.stats.unheard += 1;
        }
        self.held.push_back(Held { socket, packet, from, until: now + HOLD_TICKS });
    }

    /// A program's call (the entry point, or INT 7Ah): function BX.
    pub fn call(&mut self, cpu: &mut Cpu) {
        let ecb = (cpu.es(), cpu.si());
        let now = cpu.bus.clock.now_ticks();
        match cpu.bx() {
            0x00 => self.open_socket(cpu),
            0x01 => self.close_socket(cpu, cpu.dx().swap_bytes()),
            // Get Local Target: the node of the address at ES:SI is the
            // one to send to, as all are on one network.
            0x02 => {
                let node = read_bytes(cpu, ecb, 4, 6);
                write_bytes(cpu, (cpu.es(), cpu.di()), 0, &node);
                cpu.set_cx(1);
                set_al(cpu, SUCCESS);
            }
            0x03 => self.send(cpu, ecb, now),
            0x04 => self.listen(cpu, ecb, now),
            0x05 | 0x07 => {
                if !ecb_writable(cpu, ecb) {
                    return;
                }
                self.events.retain(|e| e.ecb != ecb);
                write_u8(cpu, ecb, ECB_IN_USE, AES_COUNTING);
                let due = now + cpu.ax() as u64 * BIOS_TICK;
                self.events.push(Event { ecb, due, owner: cpu.current_psp });
            }
            0x06 => self.cancel(cpu, ecb),
            // Get Interval Marker: the BIOS's tick count.
            0x08 => {
                let ticks = cpu.bus.guest_read_16(0x046C);
                cpu.set_ax(ticks);
            }
            // Get Internetwork Address: network 0, and the node.
            0x09 => {
                let mut address = [0u8; 10];
                address[4..].copy_from_slice(&self.node.0);
                write_bytes(cpu, ecb, 0, &address);
            }
            // Relinquish Control, Disconnect From Target.
            0x0A | 0x0B => {}
            // Get Packet Size, and the driver's largest.
            0x0D => {
                cpu.set_ax(PACKET_SIZE);
                cpu.set_cx(0);
            }
            0x1A => {
                cpu.set_ax(MAX_PACKET_SIZE);
                cpu.set_cx(0);
            }
            // SPX isn't installed.
            0x10 => set_al(cpu, 0x00),
            function => self.log.push(format!("[IPX] Function {:04X} isn't supported", function)),
        }
        if !cpu.bus.guest_faulted() {
            self.complete_polled(cpu);
        }
    }

    fn open_socket(&mut self, cpu: &mut Cpu) {
        let requested = cpu.dx().swap_bytes();
        if self.sockets.len() >= MAX_SOCKETS {
            set_al(cpu, 0xFE);
            return;
        }
        let number = if requested == 0 {
            match (DYNAMIC_SOCKETS..=0x7FFF).find(|n| !self.is_open(*n)) {
                Some(n) => n,
                None => {
                    set_al(cpu, 0xFE);
                    return;
                }
            }
        } else if self.is_open(requested) {
            set_al(cpu, 0xFF);
            return;
        } else {
            requested
        };
        self.sockets.push(Socket { number, long_lived: cpu.get_al() == 0xFF, owner: cpu.current_psp });
        self.arm = true;
        set_al(cpu, SUCCESS);
        cpu.set_dx(number.swap_bytes());
    }

    fn close_socket(&mut self, cpu: &mut Cpu, socket: u16) {
        if !self.is_open(socket) {
            return;
        }
        // Its ECBs are cancelled, without calling their ESRs.
        let ecbs: Vec<Far> = self
            .listens
            .iter()
            .filter(|l| l.socket == socket)
            .map(|l| l.ecb)
            .chain(self.completions.iter().filter(|c| c.socket == socket).map(|c| c.ecb))
            .collect();
        for &ecb in &ecbs {
            if !cpu.bus.guest_probe(lin(ecb, 0), ECB_SOCKET as usize, true) {
                return;
            }
        }
        for ecb in ecbs {
            set_flags(cpu, ecb, AVAILABLE, CANCELLED);
        }
        self.forget_socket(socket);
    }

    fn forget_socket(&mut self, socket: u16) {
        self.sockets.retain(|s| s.number != socket);
        self.listens.retain(|l| l.socket != socket);
        self.completions.retain(|c| c.socket != socket);
        self.held.retain(|h| h.socket != socket);
    }

    fn send(&mut self, cpu: &mut Cpu, ecb: Far, now: u64) {
        set_al(cpu, SUCCESS);
        let socket = ecb_socket(cpu, ecb);
        let pieces = fragments(cpu, ecb);
        let immediate = Mac(read_bytes(cpu, ecb, ECB_IMMEDIATE, 6).try_into().unwrap());
        let mut packet = Vec::new();
        for &(at, len) in &pieces {
            packet.extend(read_bytes(cpu, at, 0, len.min(frame::MAX_FRAME)));
        }
        if cpu.bus.guest_faulted() || !ecb_writable(cpu, ecb) {
            return;
        }
        let header_ok = pieces.first().is_some_and(|&(_, len)| len >= HEADER);
        if !header_ok || packet.len() > self.frame_type.max_packet() {
            self.log.push(format!("[IPX] A packet of {} bytes can't be sent", packet.len()));
            set_flags(cpu, ecb, AVAILABLE, MALFORMED);
            self.completions.push_back(Completion { ecb, socket, owner: cpu.current_psp, done: Done::Sent });
            return;
        }
        // The driver fills in the checksum, the length and the source, in
        // the program's header too.
        let length = packet.len() as u16;
        packet[0..2].copy_from_slice(&[0xFF, 0xFF]);
        packet[2..4].copy_from_slice(&length.to_be_bytes());
        packet[4] = 0;
        packet[18..22].copy_from_slice(&[0; 4]);
        packet[22..28].copy_from_slice(&self.node.0);
        packet[28..30].copy_from_slice(&socket.to_be_bytes());
        let header = pieces[0].0;
        write_bytes(cpu, header, 0, &packet[0..4]);
        write_bytes(cpu, header, 18, &packet[18..30]);
        set_flags(cpu, ecb, AVAILABLE, SUCCESS);
        let destination = Mac(packet[10..16].try_into().unwrap());
        let to_self = destination == self.node || immediate == self.node;
        if !to_self {
            self.outgoing.push(encapsulate(self.frame_type, immediate, self.node, &packet));
        }
        self.stats.sent += 1;
        // As DOSBox's driver does, a broadcast comes back to its sender too.
        if to_self || immediate.is_broadcast() {
            self.receive(packet, self.node, now);
        }
        self.completions.push_back(Completion { ecb, socket, owner: cpu.current_psp, done: Done::Sent });
    }

    fn listen(&mut self, cpu: &mut Cpu, ecb: Far, now: u64) {
        let socket = ecb_socket(cpu, ecb);
        if cpu.bus.guest_faulted() || !ecb_writable(cpu, ecb) {
            return;
        }
        if !self.is_open(socket) {
            set_flags(cpu, ecb, AVAILABLE, FAILURE);
            set_al(cpu, 0xFF);
            return;
        }
        self.listens.retain(|l| l.ecb != ecb);
        write_u8(cpu, ecb, ECB_IN_USE, LISTENING);
        self.listens.push(Listen { ecb, socket, owner: cpu.current_psp });
        set_al(cpu, SUCCESS);
        // A packet that came before the ECB did.
        if let Some(i) = self.held.iter().position(|h| h.socket == socket) {
            let held = self.held.remove(i).unwrap();
            self.receive(held.packet, held.from, now);
        }
    }

    fn cancel(&mut self, cpu: &mut Cpu, ecb: Far) {
        if !ecb_writable(cpu, ecb) {
            return;
        }
        let listening = self.listens.iter().position(|l| l.ecb == ecb);
        let counting = self.events.iter().position(|e| e.ecb == ecb);
        if let Some(i) = listening {
            self.listens.remove(i);
        } else if let Some(i) = counting {
            self.events.remove(i);
        } else {
            // Completed already, or never given to the driver.
            let completing = self.completions.iter().any(|c| c.ecb == ecb);
            set_al(cpu, if completing { CANNOT_CANCEL } else { FAILURE });
            return;
        }
        set_flags(cpu, ecb, AVAILABLE, CANCELLED);
        set_al(cpu, SUCCESS);
    }

    /// Fill in what `completion` brought in its ECB; false if a page
    /// fault stopped it (the service runs again).
    fn finish(&self, cpu: &mut Cpu, completion: &Completion) -> bool {
        let ecb = completion.ecb;
        match &completion.done {
            Done::Sent => {}
            Done::Event => {
                if !ecb_writable(cpu, ecb) {
                    return false;
                }
                set_flags(cpu, ecb, AVAILABLE, SUCCESS);
            }
            Done::Received { packet, from } => {
                let pieces = fragments(cpu, ecb);
                if cpu.bus.guest_faulted() || !ecb_writable(cpu, ecb) {
                    return false;
                }
                for &(at, len) in &pieces {
                    if len > 0 && !cpu.bus.guest_probe(lin(at, 0), len, true) {
                        return false;
                    }
                }
                let mut rest = &packet[..];
                for &(at, len) in &pieces {
                    let n = len.min(rest.len());
                    write_bytes(cpu, at, 0, &rest[..n]);
                    rest = &rest[n..];
                }
                write_bytes(cpu, ecb, ECB_IMMEDIATE, &from.0);
                set_flags(cpu, ecb, AVAILABLE, if rest.is_empty() { SUCCESS } else { MALFORMED });
            }
        }
        !cpu.bus.guest_faulted()
    }

    /// Finish the completions of ECBs without an ESR, whose programs look
    /// at the in-use flag instead of waiting for the IRQ.
    fn complete_polled(&mut self, cpu: &mut Cpu) {
        let mut i = 0;
        while i < self.completions.len() {
            let completion = self.completions[i].clone();
            let esr = read_far(cpu, completion.ecb, ECB_ESR);
            if cpu.bus.guest_faulted() {
                return;
            }
            if esr != (0, 0) {
                i += 1;
                continue;
            }
            if !self.finish(cpu, &completion) {
                return;
            }
            self.completions.remove(i);
        }
    }

    /// The IRQ handler's service: finish the completions in turn, and hand
    /// back the next whose ESR is to be called, in ES:SI with AL FFh (a
    /// packet) or 00h (an event) and CF clear, or CF set for none left.
    pub fn esr_service(&mut self, cpu: &mut Cpu) {
        while let Some(completion) = self.completions.front().cloned() {
            let esr = read_far(cpu, completion.ecb, ECB_ESR);
            if cpu.bus.guest_faulted() || !self.finish(cpu, &completion) {
                return;
            }
            self.completions.pop_front();
            if esr != (0, 0) {
                cpu.set_es(completion.ecb.0);
                cpu.set_si(completion.ecb.1);
                set_al(cpu, if completion.done == Done::Event { 0x00 } else { 0xFF });
                cpu.set_cpu_flag(CpuFlags::CF, false);
                return;
            }
        }
        cpu.set_cpu_flag(CpuFlags::CF, true);
    }

    /// Close every socket and forget every ECB: no program is left.
    pub fn reset(&mut self) {
        self.sockets.clear();
        self.listens.clear();
        self.events.clear();
        self.completions.clear();
        self.held.clear();
        self.outgoing.clear();
    }

    /// The program with PSP `psp` ended: its short-lived sockets close,
    /// and its events are forgotten, as its memory is no longer its own.
    pub fn program_ended(&mut self, psp: u16) {
        let closing: Vec<u16> =
            self.sockets.iter().filter(|s| s.owner == psp && !s.long_lived).map(|s| s.number).collect();
        for socket in closing {
            self.forget_socket(socket);
        }
        self.listens.retain(|l| l.owner != psp || self.sockets.iter().any(|s| s.number == l.socket && s.long_lived));
        self.events.retain(|e| e.owner != psp);
        let sockets = &self.sockets;
        self.completions.retain(|c| c.owner != psp || c.socket != 0 && sockets.iter().any(|s| s.number == c.socket));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(destination: Mac, socket: u16, data: &[u8]) -> Vec<u8> {
        let mut p = vec![0u8; HEADER];
        p[0..2].copy_from_slice(&[0xFF, 0xFF]);
        p[2..4].copy_from_slice(&((HEADER + data.len()) as u16).to_be_bytes());
        p[10..16].copy_from_slice(&destination.0);
        p[16..18].copy_from_slice(&socket.to_be_bytes());
        p.extend_from_slice(data);
        p
    }

    #[test]
    fn packets_go_in_and_out_of_every_frame_type() {
        let (a, b) = (Mac([2, 0, 0, 0, 0, 1]), Mac([2, 0, 0, 0, 0, 2]));
        let p = packet(b, 0x869C, b"hello");
        for kind in FrameType::ALL {
            let frame = encapsulate(kind, b, a, &p);
            assert_eq!(frame.len(), frame::MIN_FRAME, "short frames are padded");
            assert_eq!(decapsulate(&frame), Some(&p[..]), "{:?}", kind);
            assert_eq!(FrameType::parse(kind.name()), Some(kind));
        }
        // Not IPX.
        assert_eq!(decapsulate(&frame::build(b, a, 0x0800, &p)), None);
        assert_eq!(decapsulate(&frame::build(b, a, 20, &[0x42; 20])), None);
        assert_eq!(FrameType::EthernetII.max_packet(), 1500);
        assert_eq!(FrameType::Snap.max_packet(), 1492);
    }

    #[test]
    fn packets_find_their_listener_or_wait() {
        let node = Mac([2, 0, 0, 0, 0, 1]);
        let mut ipx = Ipx::new(node, 11, FrameType::EthernetII);
        ipx.sockets.push(Socket { number: 0x869C, long_lived: false, owner: 1 });
        // For someone else, for a closed socket: dropped.
        ipx.receive(packet(Mac([2, 0, 0, 0, 0, 9]), 0x869C, b"x"), node, 0);
        ipx.receive(packet(node, 0x1234, b"x"), node, 0);
        assert!(ipx.held.is_empty() && ipx.completions.is_empty());
        // No ECB yet: held, then given up.
        ipx.receive(packet(Mac::BROADCAST, 0x869C, b"1"), node, 0);
        assert_eq!(ipx.held.len(), 1);
        ipx.advance(HOLD_TICKS);
        assert!(ipx.held.is_empty());
        // Held packets are limited per socket.
        for i in 0..HELD_PER_SOCKET + 3 {
            ipx.receive(packet(node, 0x869C, &[i as u8]), node, 0);
        }
        assert_eq!(ipx.held.len(), HELD_PER_SOCKET);
        assert_eq!(ipx.held[0].packet[HEADER], 3, "the oldest went first");
        // With an ECB listening it completes.
        ipx.held.clear();
        ipx.listens.push(Listen { ecb: (0x1000, 0), socket: 0x869C, owner: 1 });
        ipx.receive(packet(node, 0x869C, b"2"), node, 0);
        assert!(ipx.listens.is_empty());
        assert!(ipx.wants_irq());
        assert!(matches!(ipx.completions[0].done, Done::Received { .. }));
        // Events come due.
        ipx.events.push(Event { ecb: (0x2000, 0), due: 100, owner: 1 });
        assert_eq!(ipx.next_event(), Some(100));
        ipx.advance(99);
        assert_eq!(ipx.completions.len(), 1);
        ipx.advance(100);
        assert_eq!(ipx.completions[1].done, Done::Event);
        // The program ends: its socket closes and its events go.
        ipx.program_ended(1);
        assert!(ipx.sockets.is_empty() && ipx.completions.is_empty());
    }
}
