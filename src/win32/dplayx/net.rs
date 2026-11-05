//! DirectPlay's objects on the network: their packets go out on the IPX
//! socket `wire::SOCKET` of the built-in IPX driver, which sends them as
//! Ethernet frames through the instance's switch, and so through the LAN
//! room's relay to the other members of the room, as a DOS game's IPX
//! packets go. A broadcast reaches every instance in the room, so every
//! member's DirectPlay finds the sessions hosted in it.
//!
//! The driver is installed when a program first initializes a
//! connection (unless `ipx=false`): before a room is joined, the packets
//! go no further than this instance. With `ipx=false` there is no driver,
//! and the objects reach each other within this instance alone.
//!
//! `poll` hands the objects what came and lets time pass for them; it
//! runs when time passes for the threads (`thread::wake`) and at the
//! start of the methods that read what came. `settle` sends what the
//! objects left to send and sets the events of the players whose
//! messages came.

use super::session::Play;
use super::wire::SOCKET;
use crate::cpu::Cpu;
use crate::net::frame::Mac;
use crate::win32::object::{self, Kind};
use std::collections::VecDeque;

/// The node the objects have without the IPX driver.
const NO_DRIVER: Mac = Mac([0x02, 0, 0, 0, 0, 0]);
/// The room in a packet without the driver: an Ethernet II packet's.
const NO_DRIVER_ROOM: u32 = 1500 - crate::net::ipx::HEADER as u32;

/// Packets between this instance's objects when there is no IPX driver.
#[derive(Clone, Debug, Default)]
pub struct Local {
    pub packets: VecDeque<(Mac, Vec<u8>)>,
}

/// A program initialized a connection: the IPX driver goes in if it may,
/// and DirectPlay opens its socket on it.
pub fn connect(cpu: &mut Cpu) {
    cpu.bus.install_ipx();
    if let Some(ipx) = cpu.bus.net.ipx.as_mut()
        && !ipx.open_host_socket(SOCKET)
    {
        cpu.bus.log_string(&format!(
            "[DPLAY] IPX socket {:04X}h is a program's: DirectPlay can't reach the network",
            SOCKET
        ));
    }
}

/// The node the objects are on, and the room in a packet.
fn place(cpu: &Cpu) -> (Mac, u32) {
    match &cpu.bus.net.ipx {
        Some(ipx) => (ipx.node, ipx.max_host_data() as u32),
        None => (NO_DRIVER, NO_DRIVER_ROOM),
    }
}

fn plays(cpu: &mut Cpu) -> impl Iterator<Item = &mut Play> {
    let objects = cpu.bus.win32.dplay.objects.iter_mut().flatten();
    objects
        .filter(|o| o.play.provider != 0)
        .map(|o| &mut o.play)
}

/// Bring the object `id`'s place up to date before it acts.
pub fn prepare(cpu: &mut Cpu, id: usize) {
    let (node, room) = place(cpu);
    if let Some(Some(o)) = cpu.bus.win32.dplay.objects.get_mut(id) {
        o.play.node = node;
        o.play.room = room;
    }
}

/// Hand the objects the packets that came, let time pass for them, and
/// send what they have to send.
pub fn poll(cpu: &mut Cpu) {
    if !cpu
        .bus
        .win32
        .dplay
        .objects
        .iter()
        .flatten()
        .any(|o| o.play.provider != 0)
    {
        return;
    }
    let packets: Vec<(Mac, Vec<u8>)> = match cpu.bus.net.ipx.as_mut() {
        Some(ipx) => {
            // The driver drops its sockets when no program is left; the
            // socket opens again for those that go on.
            ipx.open_host_socket(SOCKET);
            let mut packets = Vec::new();
            ipx.host_inbox.retain_mut(|p| {
                if p.socket == SOCKET {
                    packets.push((p.from, std::mem::take(&mut p.data)));
                }
                p.socket != SOCKET
            });
            packets
        }
        None => cpu.bus.win32.dplay.local.packets.drain(..).collect(),
    };
    let (node, room) = place(cpu);
    let now = cpu.bus.clock.now_micros();
    for play in plays(cpu) {
        play.node = node;
        play.room = room;
        for (from, packet) in &packets {
            play.handle(*from, packet, now);
        }
        play.tick(now);
        play.flush(now);
    }
    settle(cpu);
}

/// The object `id` acted: what it has to send goes now.
pub fn after(cpu: &mut Cpu, id: usize) {
    let now = cpu.bus.clock.now_micros();
    if let Some(Some(o)) = cpu.bus.win32.dplay.objects.get_mut(id) {
        o.play.flush(now);
    }
    settle(cpu);
}

/// Send the packets the objects left, set the events of the players
/// whose messages came, and let go of the events of players that went.
/// The handles DirectPlay made for them are closed in the process of the
/// object, not the running one: an object leaves its session also when
/// its process ended, whose handles went with it.
pub fn settle(cpu: &mut Cpu) {
    let mut packets = Vec::new();
    let mut signal = Vec::new();
    let mut released = Vec::new();
    for o in cpu.bus.win32.dplay.objects.iter_mut().flatten() {
        packets.append(&mut o.play.outbox);
        signal.append(&mut o.play.signal);
        let pid = o.pid;
        released.extend(o.play.released.drain(..).map(|(e, h)| (pid, e, h)));
    }
    let (node, _) = place(cpu);
    match cpu.bus.net.ipx.as_mut() {
        Some(ipx) => {
            for (to, bytes) in &packets {
                if !ipx.send_host(SOCKET, *to, SOCKET, bytes) {
                    ipx.log.push(format!(
                        "[DPLAY] A packet of {} bytes can't be sent",
                        bytes.len()
                    ));
                }
            }
        }
        None => {
            let local = &mut cpu.bus.win32.dplay.local.packets;
            for (to, bytes) in packets {
                if to == node || to.is_broadcast() {
                    local.push_back((node, bytes));
                }
            }
        }
    }
    if cpu.bus.net.ipx.is_some() {
        cpu.bus.sync_ipx();
    }
    for id in signal {
        if let Some(o) = object::objects(cpu).get_mut(id)
            && let Kind::Event { signaled, .. } = &mut o.kind
        {
            *signaled = true;
        }
    }
    for (pid, event, handle) in released {
        if handle != 0 {
            object::close_of(cpu, pid, handle);
        }
        if event != 0 {
            object::objects(cpu).release(event);
        }
    }
}
