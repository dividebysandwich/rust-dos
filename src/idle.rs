//! Busy-wait loops skipped exactly.
//!
//! Programs wait in loops: DOS polls the keyboard through the BIOS between
//! its idle calls, games read a timer count until it changes. Where a loop
//! comes back to where it started with nothing changed (the registers, the
//! memory it wrote and everything else a later instruction could see), every
//! pass after it is the same pass again until a device's event changes
//! something. Those passes come to an instruction count and nothing else,
//! so the clock is moved on by them instead of running them, up to the next
//! timer event, and the front end sleeps through the time, as for HLT.
//!
//! **The proof.** The interpreter runs the loop from a place it comes back
//! to (its head) while the bus notes which blocks of RAM it writes
//! (`Observe`). When the CPU is at the head with the registers of an earlier
//! visit, a pass that changes nothing may have been found: the blocks
//! written so far are copied, and the loop runs on until it comes back with
//! the same registers and those blocks as copied, having written no others.
//! The machine is then in the state it was in a whole number of passes
//! before, and runs the same instructions again from there, with nothing
//! else to tell them apart:
//!
//! - no device runs between timer events, and none is reached: port I/O,
//!   memory that isn't RAM or ROM, RDTSC, and every emulator service but the
//!   BIOS keyboard's status calls end the proof;
//! - no interrupt comes before the next event, which ends it too.
//!
//! The passes skipped all end by the next timer event, as they would have
//! run, and count as executed instructions: the registers, memory and
//! counts come out as without the skip.
//!
//! **Where loops come from.** A head is armed at the service trap where the
//! BIOS keyboard's status call found no key many times in a row, and where
//! two timer events in a row found the CPU at the same instruction with the
//! same registers (a tight loop). Failed proofs make the next tries rarer.

use std::cell::Cell;

use crate::bus::GEN_SHIFT;
use crate::cpu::Cpu;

/// Bytes in a block of RAM the bus notes writes of.
const CHUNK: usize = 1 << GEN_SHIFT;
/// Instructions a proof may run before it gives up.
const PROOF_LIMIT: u64 = 50_000;
/// Instructions the CPU may take to come to an armed head.
const ARM_LIMIT: u64 = 20_000;
/// Visits of the head a proof looks back over for the same registers.
const VISITS: usize = 64;
/// Blocks of RAM a loop may write.
const CHUNKS: usize = 256;
/// Keyboard status calls finding no key at one place before it is armed.
const POLLS: u32 = 8;
/// Reads of the VGA's status in a row before the loop is armed.
const STATUS_POLLS: u32 = 64;

/// An instruction's place: its CS selector and EIP, and the mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Head {
    cs: u16,
    eip: u32,
    pe: bool,
}

/// What the proof compares at the head: everything of the CPU's an
/// instruction can see (`Cpu::idle_key`).
#[derive(Clone, PartialEq)]
pub struct CpuKey {
    pub regs: crate::cpu::CpuSnapshot,
    pub cr: [u32; 4],
    pub dr: [u32; 8],
    pub cpl: u8,
    pub a20: u32,
    pub tables: [crate::cpu::DescTable; 2],
    pub system: [crate::cpu::SegCache; 2],
    pub fpu: crate::cpu::FpuKey,
    pub shadow: bool,
    /// AL holds a value read from a port, and isn't compared (see
    /// `Observe::tainted`).
    pub tainted: bool,
}

#[derive(Clone)]
struct Visit {
    key: CpuKey,
    icount: u64,
    executed: u64,
}

#[derive(Default)]
enum Phase {
    /// Nothing watched.
    #[default]
    Off,
    /// Waiting for the CPU to come to the head.
    Armed { head: Head, until: u64 },
    /// A port was polled in a loop: the next instruction is the head.
    ArmHere,
    /// Running from the head, looking for a visit with the registers of an
    /// earlier one.
    Learn { head: Head, visits: Vec<Visit>, until: u64 },
    /// Running on from `base` (a visit whose registers came round), until
    /// a visit with its registers and the blocks in `copies` as they were.
    Verify { head: Head, base: Box<Visit>, copies: Vec<(usize, [u8; CHUNK])>, until: u64 },
}

/// Counts for the statistics.
#[derive(Clone, Copy, Debug, Default)]
pub struct IdleStats {
    /// Loops proven to change nothing, proofs that failed, and the
    /// instructions skipped.
    pub proofs: u64,
    pub failures: u64,
    pub skipped: u64,
    /// Why the last proof failed.
    pub failure: &'static str,
}

/// The prover's state. It is the bus's, whose memory and port accesses
/// report to it while a proof runs.
pub struct Observe {
    /// A proof runs: the bus notes writes and anything else.
    pub on: bool,
    /// Skipping is on.
    pub enabled: bool,
    /// Set by the BIOS keyboard's status call finding no key.
    pub polled: bool,
    /// Set by an access the proof can't follow, to what it was.
    failed: Cell<Option<&'static str>>,
    /// The blocks written since the last look.
    chunks: Vec<u32>,
    phase: Phase,
    /// Where the keyboard status call found no key last, and how many times
    /// in a row.
    poll: (Option<Head>, u32),
    /// Where the CPU was at the last timer event, with its general
    /// registers and flags.
    last_event: Option<(Head, [u32; 8], u32)>,
    /// The port the instruction about to run may read: an IN AL of a port
    /// `Bus::idle_port` knows.
    port_ok: Option<u16>,
    /// The ports read since the base, with the bits of them the loop looked
    /// at, and when the first was read.
    ports: Vec<(u16, u8)>,
    first_read: Option<u64>,
    /// AL holds the value of port `al_port`, of which only TEST and AND
    /// with a constant may look at the bits they keep. Its other bits then
    /// never matter, and the bits kept stay as they are until the port's
    /// `Bus::idle_port` time.
    tainted: bool,
    al_port: u16,
    /// When AL was read from it.
    al_read: u64,
    /// Reads of the VGA's status port in a row, which arm the loop.
    polls: u32,
    /// Chances to arm a head to let pass, after failed proofs.
    backoff: u32,
    failures_in_row: u32,
    pub stats: IdleStats,
}

impl Default for Observe {
    fn default() -> Self {
        Observe {
            on: false,
            enabled: default_enabled(),
            polled: false,
            failed: Cell::new(None),
            chunks: Vec::new(),
            phase: Phase::Off,
            poll: (None, 0),
            last_event: None,
            port_ok: None,
            ports: Vec::new(),
            first_read: None,
            tainted: false,
            al_port: 0,
            al_read: 0,
            polls: 0,
            backoff: 0,
            failures_in_row: 0,
            stats: IdleStats::default(),
        }
    }
}

/// Skipping is on unless `RUST_DOS_IDLE_SKIP=0` (the tests compare runs
/// with and without it).
fn default_enabled() -> bool {
    std::env::var("RUST_DOS_IDLE_SKIP").map_or(true, |v| v != "0")
}

impl Observe {
    /// A head is armed or a proof runs: the execution loop runs one
    /// instruction at a time through `before_instruction`.
    #[inline(always)]
    pub fn watching(&self) -> bool {
        !matches!(self.phase, Phase::Off)
    }

    /// Blocks `first..=last` of RAM were written.
    #[cold]
    pub fn wrote(&mut self, first: usize, last: usize) {
        for chunk in first..=last {
            self.chunks.push(chunk as u32);
        }
        if self.chunks.len() > 4 * CHUNKS {
            self.chunks.sort_unstable();
            self.chunks.dedup();
            if self.chunks.len() > CHUNKS {
                self.opaque_because("blocks");
            }
        }
    }

    /// Port I/O, RDTSC or a service: something the proof can't follow.
    #[inline(always)]
    pub fn opaque(&self) {
        self.opaque_because("service");
    }

    /// RDTSC.
    pub fn time_read(&self) {
        self.opaque_because("time");
    }

    #[inline(always)]
    fn opaque_because(&self, why: &'static str) {
        if self.on && self.failed.get().is_none() {
            self.failed.set(Some(why));
        }
    }

    /// A port is read: one the proof allowed (`port_ok`), or something it
    /// can't follow. Outside proofs, reads of the VGA's status in a row arm
    /// the loop they are in.
    #[inline(always)]
    pub fn port_read(&mut self, port: u16) {
        if self.on {
            if self.port_ok.take() != Some(port) {
                self.opaque_because("port read");
            }
        } else if matches!(port, 0x3DA | 0x3BA) {
            self.polls += 1;
            if self.polls >= STATUS_POLLS {
                self.polls = 0;
                self.arm_here();
            }
        } else {
            self.polls = 0;
        }
    }

    /// A port is written.
    #[inline(always)]
    pub fn port_write(&mut self) {
        self.polls = 0;
        self.opaque_because("port write");
    }

    /// Arm the loop the CPU is in, at the next instruction.
    #[cold]
    fn arm_here(&mut self) {
        if !self.enabled || self.watching() {
            return;
        }
        if self.backoff > 0 {
            self.backoff -= 1;
            return;
        }
        self.phase = Phase::ArmHere;
    }

    /// A read at `addr` that isn't plain RAM. The ROMs and the RAM between
    /// them read as RAM does; video memory and devices end the proof.
    #[cold]
    pub fn mapped(&self, addr: usize, ram_len: usize) {
        if !(0xC_0000..0x10_0000).contains(&addr) || addr >= ram_len {
            self.opaque_because("device read");
        }
    }

    /// A write at `addr` that isn't plain RAM, as `mapped`: the ROMs ignore
    /// it, and the RAM between them is noted.
    #[cold]
    pub fn mapped_write(&mut self, addr: usize, ram_len: usize) {
        if (0xC_0000..0x10_0000).contains(&addr) && addr + 4 <= ram_len {
            self.wrote(addr >> GEN_SHIFT, (addr + 3) >> GEN_SHIFT);
        } else {
            self.opaque_because("device write");
        }
    }

    fn arm(&mut self, head: Head, now: u64) {
        if !self.enabled || self.watching() {
            return;
        }
        if self.backoff > 0 {
            self.backoff -= 1;
            return;
        }
        self.phase = Phase::Armed { head, until: now + ARM_LIMIT };
    }

    fn stop(&mut self) {
        self.on = false;
        self.failed.set(None);
        self.chunks.clear();
        self.phase = Phase::Off;
        self.forget_ports();
        self.tainted = false;
    }

    fn forget_ports(&mut self) {
        self.port_ok = None;
        self.ports.clear();
        self.first_read = None;
    }

    /// From a new base on, the ports read count from there, but for the
    /// value of one AL still holds, which the passes after it may test.
    fn restart_ports(&mut self) {
        self.forget_ports();
        if self.tainted {
            self.ports.push((self.al_port, 0));
            self.first_read = Some(self.al_read);
        }
    }

    fn fail(&mut self, why: &'static str) {
        self.stop();
        self.stats.failures += 1;
        self.stats.failure = why;
        self.failures_in_row = (self.failures_in_row + 1).min(12);
        self.backoff = 1 << self.failures_in_row;
    }

    /// The blocks written since the last look, each once.
    fn take_chunks(&mut self) -> Vec<u32> {
        let mut chunks = std::mem::take(&mut self.chunks);
        chunks.sort_unstable();
        chunks.dedup();
        chunks
    }
}

/// Whether the emulator service `kind` (FE 38h-3Bh) of `vector` with AX
/// leaves nothing changed but RAM and registers, which a proof can follow:
///
/// - the BIOS keyboard's status and shift state (the keystrokes are in the
///   BIOS data area on a booted system, and come between batches
///   otherwise), and the BIOS tick count in the data area;
/// - DOS's date and time, whose state is in its swappable data area, and
///   the mouse driver's position, which moves between batches. The time is
///   the host's: the loop is only proven where it read the same time in the
///   passes compared, and skipping them is running them on a host fast
///   enough to read that time in all of them.
pub fn pure_service(kind: u8, vector: u8, ax: u16) -> bool {
    let ah = (ax >> 8) as u8;
    kind == 0x38
        && match vector {
            0x16 => matches!(ah, 0x01 | 0x02 | 0x11 | 0x12),
            0x1A => ah == 0x00,
            0x21 => matches!(ah, 0x2A | 0x2C),
            0x33 => ax == 0x0003,
            _ => false,
        }
}

fn head_of(cpu: &Cpu) -> Head {
    Head { cs: cpu.cs(), eip: cpu.eip(), pe: cpu.pe() }
}

/// The BIOS keyboard's status call found no key, at the service trap at
/// `cs:eip`. Arms it once that happened enough times in a row.
pub fn keyboard_poll(cpu: &mut Cpu, cs: u16, eip: u32) {
    let head = Head { cs, eip, pe: cpu.pe() };
    let now = cpu.bus.clock.icount;
    let o = &mut cpu.bus.observe;
    if !o.enabled {
        return;
    }
    if o.poll.0 == Some(head) {
        o.poll.1 += 1;
    } else {
        o.poll = (Some(head), 1);
    }
    if o.poll.1 >= POLLS {
        o.poll.1 = 0;
        o.arm(head, now);
    }
}

/// A timer event is due. A proof running ends, as the event may change what
/// the loop sees (it is tried again after it); a CPU at the same instruction
/// with the same registers as at the last event arms that instruction.
#[cold]
pub fn at_event(cpu: &mut Cpu) {
    if !cpu.bus.observe.enabled {
        return;
    }
    if cpu.bus.observe.on {
        cpu.bus.observe.stop();
    }
    let regs = cpu.snapshot();
    let here = (head_of(cpu), regs.gpr, regs.flags.bits());
    let now = cpu.bus.clock.icount;
    let o = &mut cpu.bus.observe;
    let same = o.last_event == Some(here);
    o.last_event = Some(here);
    if same {
        o.arm(here.0, now);
    }
}

/// An interrupt was delivered, which changes what the proof compares.
#[cold]
pub fn interrupted(cpu: &mut Cpu) {
    if cpu.bus.observe.on {
        cpu.bus.observe.stop();
    }
}

/// What a visit of the head comes to.
enum Step {
    Start,
    Remember,
    Copy,
    Compare,
}

/// Before the instruction at CS:EIP runs while `watching`: start a proof at
/// the armed head, or look at a visit of it, which may skip passes; then
/// look at the instruction, during a proof.
pub fn before_instruction(cpu: &mut Cpu) {
    let now = cpu.bus.clock.icount;
    visit(cpu, now);
    if cpu.bus.observe.on {
        inspect(cpu, now);
    }
}

fn visit(cpu: &mut Cpu, now: u64) {
    let here = head_of(cpu);
    let o = &mut cpu.bus.observe;
    if let Some(why) = o.failed.get() {
        return o.fail(why);
    }
    if matches!(o.phase, Phase::ArmHere) {
        o.phase = Phase::Armed { head: here, until: now + ARM_LIMIT };
    }
    let (head, until) = match &o.phase {
        Phase::Off | Phase::ArmHere => return,
        Phase::Armed { head, until } => (*head, *until),
        Phase::Learn { head, until, .. } | Phase::Verify { head, until, .. } => (*head, *until),
    };
    if now > until {
        return if o.on { o.fail("limit") } else { o.stop() };
    }
    if here != head {
        return;
    }
    let mut key = cpu.idle_key();
    if cpu.bus.observe.tainted {
        key.regs.gpr[0] &= !0xFF;
        key.tainted = true;
    }
    let visit = Visit { key, icount: now, executed: cpu.executed };
    let o = &mut cpu.bus.observe;
    let step = match &o.phase {
        Phase::Off | Phase::ArmHere => return,
        Phase::Armed { .. } => Step::Start,
        Phase::Learn { visits, .. } if visits.iter().any(|v| v.key == visit.key) => Step::Copy,
        Phase::Learn { visits, .. } if visits.len() < VISITS => Step::Remember,
        Phase::Learn { .. } => return o.fail("visits"),
        Phase::Verify { base, .. } if base.key == visit.key => Step::Compare,
        Phase::Verify { .. } => return,
    };
    match step {
        Step::Start => {
            o.on = true;
            o.failed.set(None);
            o.chunks.clear();
            o.forget_ports();
            o.phase = Phase::Learn { head, visits: vec![visit], until: now + PROOF_LIMIT };
        }
        Step::Remember => {
            if let Phase::Learn { visits, .. } = &mut o.phase {
                visits.push(visit);
            }
        }
        Step::Copy => {
            // The registers came round: copy the blocks written so far, and
            // see whether they come round as well, with the ports read from
            // here on.
            let chunks = o.take_chunks();
            o.restart_ports();
            let copies = copy_blocks(cpu.bus.ram(), chunks.iter().map(|&c| c as usize).collect());
            cpu.bus.observe.phase = Phase::Verify { head, base: Box::new(visit), copies, until };
        }
        Step::Compare => compare(cpu, visit),
    }
}

/// At a visit with the registers of the base: if the blocks written since
/// are as they were there, the loop is proven and passes are skipped.
fn compare(cpu: &mut Cpu, visit: Visit) {
    let chunks = cpu.bus.observe.take_chunks();
    let Phase::Verify { base, copies, .. } = &cpu.bus.observe.phase else { return };
    let ram = cpu.bus.ram();
    let mut same = true;
    let mut known = true;
    for &c in &chunks {
        match copies.binary_search_by_key(&(c as usize), |(k, _)| *k) {
            Ok(i) => {
                let start = c as usize * CHUNK;
                same &= ram[start..start + CHUNK] == copies[i].1;
            }
            Err(_) => known = false,
        }
    }
    if !known {
        // A block the passes before the base didn't write, whose contents
        // there aren't known: this visit is the base instead, with copies
        // of all the blocks written so far.
        let blocks: Vec<usize> = copies.iter().map(|(k, _)| *k).chain(chunks.iter().map(|&c| c as usize)).collect();
        let copies = copy_blocks(ram, blocks);
        let o = &mut cpu.bus.observe;
        o.restart_ports();
        if let Phase::Verify { base, copies: old, .. } = &mut o.phase {
            **base = visit;
            *old = copies;
        }
        return;
    }
    if !same {
        // Not the base's state yet: the blocks written since it are
        // compared again at the next visits.
        cpu.bus.observe.chunks.extend(chunks);
        return;
    }
    let period = visit.icount - base.icount;
    let executed = visit.executed - base.executed;
    // The passes read the ports' bits as they were since the first read
    // after the base, until the time they change.
    let mut limit = cpu.bus.clock.deadline;
    let ports = std::mem::take(&mut cpu.bus.observe.ports);
    let from = cpu.bus.observe.first_read.unwrap_or(visit.icount);
    for (port, mask) in ports {
        match cpu.bus.idle_port(port, mask, from) {
            Some(until) => limit = limit.min(until),
            None => return cpu.bus.observe.fail("port"),
        }
    }
    // AL may hold a port's value whose bits the loop doesn't look at, but
    // what comes after it may: it is made what the last pass skipped read.
    let held = cpu.bus.observe.tainted.then_some((cpu.bus.observe.al_port, cpu.bus.observe.al_read));
    let o = &mut cpu.bus.observe;
    o.stop();
    o.failures_in_row = 0;
    if period > 0 && limit > visit.icount {
        let skipped = skip(cpu, period, executed, limit);
        if let Some((port, read)) = held
            && skipped > 0
        {
            let value = cpu.bus.idle_port_value(port, read + skipped);
            cpu.set_reg8(iced_x86::Register::AL, value);
        }
    }
}

/// During a proof, the instruction about to run: an IN AL of a port whose
/// bits follow the time may run, and what reads AL after it is checked (see
/// `Observe::tainted`). Only with paging off, where its bytes are found
/// without side effects.
fn inspect(cpu: &mut Cpu, now: u64) {
    use iced_x86::{Decoder, DecoderOptions, InstructionInfoFactory, Mnemonic, OpAccess, OpKind, Register};
    if cpu.cr0 & crate::cpu::CR0_PG != 0 {
        return;
    }
    let cs = cpu.seg_cache(crate::cpu::Seg::CS);
    let bitness = if cs.attr & crate::cpu::ATTR_DB != 0 { 32 } else { 16 };
    let lin = cs.base.wrapping_add(cpu.eip()) & cpu.bus.a20_mask();
    let mut bytes = [0u8; 15];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = cpu.bus.peek_8(lin as usize + i);
    }
    let instr = Decoder::with_ip(bitness, &bytes, cpu.eip() as u64, DecoderOptions::NONE).decode();
    let o = &mut cpu.bus.observe;
    if instr.mnemonic() == Mnemonic::In && instr.op0_register() == Register::AL {
        let port = if instr.op1_kind() == OpKind::Immediate8 { instr.immediate8() as u16 } else { cpu.dx() };
        let o = &mut cpu.bus.observe;
        o.port_ok = Some(port);
        o.first_read.get_or_insert(now);
        if !o.ports.iter().any(|&(p, _)| p == port) {
            o.ports.push((port, 0));
        }
        o.tainted = true;
        o.al_port = port;
        o.al_read = now;
        return;
    }
    if !o.tainted {
        return;
    }
    // Services and interrupts read registers the decoder doesn't name.
    if instr.is_invalid() || matches!(instr.mnemonic(), Mnemonic::Int | Mnemonic::Int1 | Mnemonic::Int3 | Mnemonic::Into) {
        return o.fail("taint");
    }
    let al = |r: Register| matches!(r, Register::AL | Register::AX | Register::EAX);
    let mut factory = InstructionInfoFactory::new();
    let info = factory.info(&instr);
    let reads = info.used_registers().iter().any(|u| {
        al(u.register()) && matches!(u.access(), OpAccess::Read | OpAccess::ReadWrite | OpAccess::CondRead | OpAccess::ReadCondWrite)
    });
    let writes = info.used_registers().iter().any(|u| al(u.register()) && u.access() == OpAccess::Write);
    if reads {
        let masks = matches!(instr.mnemonic(), Mnemonic::Test | Mnemonic::And)
            && instr.op0_register() == Register::AL
            && instr.op1_kind() == OpKind::Immediate8;
        if !masks {
            return o.fail("taint");
        }
        let mask = instr.immediate8();
        let port = o.al_port;
        if let Some(entry) = o.ports.iter_mut().find(|(p, _)| *p == port) {
            entry.1 |= mask;
        }
        if instr.mnemonic() == Mnemonic::And {
            // AL now holds the bits kept, which don't change.
            o.tainted = false;
        }
    } else if writes {
        o.tainted = false;
    }
}

/// Copies of the blocks of RAM `blocks` (in any order, repeated or not),
/// sorted by block.
fn copy_blocks(ram: &[u8], mut blocks: Vec<usize>) -> Vec<(usize, [u8; CHUNK])> {
    blocks.sort_unstable();
    blocks.dedup();
    blocks
        .into_iter()
        .map(|c| {
            let mut copy = [0; CHUNK];
            copy.copy_from_slice(&ram[c * CHUNK..c * CHUNK + CHUNK]);
            (c, copy)
        })
        .collect()
}

/// Move the clock on by whole passes of `period` instructions (`executed`
/// of them executed), up to `limit`: the next timer event, or when a port
/// the loop reads changes.
/// Returns the instruction count skipped.
fn skip(cpu: &mut Cpu, period: u64, executed: u64, limit: u64) -> u64 {
    let clock = &mut cpu.bus.clock;
    let passes = limit.min(clock.deadline).saturating_sub(clock.icount) / period;
    if passes == 0 {
        return 0;
    }
    clock.icount += passes * period;
    clock.idle += passes * period;
    cpu.executed += passes * executed;
    cpu.bus.observe.stats.proofs += 1;
    cpu.bus.observe.stats.skipped += passes * period;
    passes * period
}
