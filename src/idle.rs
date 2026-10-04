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
/// Bytes of RAM a pass may change and still be skipped, as counters (see
/// `Phase::Count`).
const COUNTER_BYTES: usize = 16;
/// Instructions of a pass `Phase::Count` records.
const PASS_LIMIT: usize = 20_000;
/// Passes skipped at most over counters at once.
const COUNTED_PASSES: u64 = 1 << 20;

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
    /// The registers came round, and the blocks but for a few bytes
    /// (`counters`): one pass from `base` is recorded, with what each of
    /// its instructions does to the flags (`Pass`), to find out whether the
    /// passes after it run the same until their counters reach where one
    /// of them would take another way (see `count`).
    Count { head: Head, base: Box<Visit>, copies: Vec<(usize, [u8; CHUNK])>, pass: Box<Pass>, until: u64 },
}

/// The pass `Phase::Count` records.
#[derive(Default)]
struct Pass {
    /// The bytes that changed, sorted.
    counters: Vec<usize>,
    /// The instructions that read or change them, in order.
    ops: Vec<Counting>,
    /// The arithmetic flags each instruction reads, and those it sets
    /// whatever they were.
    flags: Vec<(u32, u32)>,
    /// What the other instructions wrote.
    writes: Vec<Access>,
}

/// An instruction of the pass that reads or changes counters: INC, DEC,
/// ADD, SUB or CMP of an operand of `size` bytes at `addr` with a constant,
/// the `at`th of the pass, which found `before` there.
#[derive(Clone, Copy)]
struct Counting {
    op: CountOp,
    addr: usize,
    size: u8,
    at: usize,
    before: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum CountOp {
    Inc,
    Dec,
    Add(u32),
    Sub(u32),
    Cmp(u32),
}

impl CountOp {
    /// The operand's new value (for CMP the same) and the flags, of those
    /// it sets: INC and DEC leave CF.
    fn run(self, size: u8, value: u32) -> (u32, u32) {
        use crate::cpu::alu;
        let m = alu::size_mask(size);
        match self {
            CountOp::Inc => (alu::add(size, value, 1, false).0, alu::add(size, value, 1, false).1 & !alu::CF),
            CountOp::Dec => (alu::sub(size, value, 1, false).0, alu::sub(size, value, 1, false).1 & !alu::CF),
            CountOp::Add(imm) => alu::add(size, value, imm & m, false),
            CountOp::Sub(imm) => alu::sub(size, value, imm & m, false),
            CountOp::Cmp(imm) => (value, alu::sub(size, value, imm & m, false).1),
        }
    }

    /// The flags it sets.
    fn sets(self) -> u32 {
        use crate::cpu::alu;
        match self {
            CountOp::Inc | CountOp::Dec => alu::ARITH & !alu::CF,
            _ => alu::ARITH,
        }
    }
}

/// Bytes of RAM an instruction reached: `len` from `start`, or an element
/// of `size` bytes at offset `off` from `base` and the `count - 1` after it
/// (before it, `down`), as a REP string instruction reaches them, the
/// offset wrapping at 64K.
#[derive(Clone, Copy)]
enum Access {
    Range { start: usize, len: usize },
    Rep { base: usize, off: u32, size: u32, count: u32, down: bool },
}

impl Access {
    fn covers(self, addr: usize) -> bool {
        match self {
            Access::Range { start, len } => addr.wrapping_sub(start) < len,
            Access::Rep { base, off, size, count, down } => {
                let at = addr.wrapping_sub(base);
                if at >= 0x1_0000 {
                    return false;
                }
                let span = count as u64 * size as u64;
                let from = if down { (off + size - 1).wrapping_sub(at as u32) } else { (at as u32).wrapping_sub(off) } & 0xFFFF;
                span >= 0x1_0000 || (from as u64) < span
            }
        }
    }
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
        // (A loop that changes something every pass, as DOS 7's keyboard
        // poll counts down, is looked at about once in 64K arms.)
        self.failures_in_row = (self.failures_in_row + 1).min(16);
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
///   enough to read that time in all of them;
/// - Windows' device call-outs (INT 2Fh AX=1607h), which only DOS's
///   DOSMGR interface answers, in registers: Windows 3.1's VMPOLL makes
///   one in each pass of its idle loop.
pub fn pure_service(kind: u8, vector: u8, ax: u16) -> bool {
    let ah = (ax >> 8) as u8;
    kind == 0x38
        && match vector {
            0x16 => matches!(ah, 0x01 | 0x02 | 0x11 | 0x12),
            0x1A => ah == 0x00,
            0x21 => matches!(ah, 0x2A | 0x2C),
            0x2F => ax == 0x1607,
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
    Count,
}

/// Before the instruction at CS:EIP runs while `watching`: start a proof at
/// the armed head, or look at a visit of it, which may skip passes; then
/// look at the instruction, during a proof.
pub fn before_instruction(cpu: &mut Cpu) {
    let now = cpu.bus.clock.icount;
    visit(cpu, now);
    if cpu.bus.observe.on {
        if matches!(cpu.bus.observe.phase, Phase::Count { .. }) {
            record(cpu);
        }
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
        Phase::Learn { head, until, .. } | Phase::Verify { head, until, .. } | Phase::Count { head, until, .. } => {
            (*head, *until)
        }
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
        Phase::Count { base, .. } if base.key == visit.key => Step::Count,
        Phase::Verify { .. } | Phase::Count { .. } => return,
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
        Step::Count => count(cpu, visit),
    }
}

/// At a visit with the registers of the base: if the blocks written since
/// are as they were there, the loop is proven and passes are skipped.
fn compare(cpu: &mut Cpu, visit: Visit) {
    let chunks = cpu.bus.observe.take_chunks();
    let Phase::Verify { base, copies, .. } = &cpu.bus.observe.phase else { return };
    let (base_icount, base_executed) = (base.icount, base.executed);
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
        // compared again at the next visits. Where only a few bytes differ
        // (counters), the passes may differ in those alone: the next one is
        // recorded from here, to see.
        let counters: Vec<usize> = chunks
            .iter()
            .filter_map(|&c| copies.binary_search_by_key(&(c as usize), |(k, _)| *k).ok())
            .flat_map(|i| {
                let (c, copy) = &copies[i];
                let start = c * CHUNK;
                (0..CHUNK).filter(move |&b| ram[start + b] != copy[b]).map(move |b| start + b)
            })
            .take(COUNTER_BYTES + 1)
            .collect();
        if counters.len() <= COUNTER_BYTES && cpu.cr0 & crate::cpu::CR0_PG == 0 {
            let blocks: Vec<usize> = copies.iter().map(|(k, _)| *k).collect();
            let copies = copy_blocks(ram, blocks);
            let o = &mut cpu.bus.observe;
            o.restart_ports();
            let Phase::Verify { head, until, .. } = o.phase else { return };
            let pass = Box::new(Pass { counters, ..Pass::default() });
            o.phase = Phase::Count { head, base: Box::new(visit), copies, pass, until };
            return;
        }
        cpu.bus.observe.chunks.extend(chunks);
        return;
    }
    finish(cpu, &visit, base_icount, base_executed, None);
}

/// A loop is proven from the visit before `visit` at `base` instructions
/// (`executed` executed): skip passes up to the next event, or the time a
/// port it read changes, and with `counted` as long as the passes' counters
/// let them run the same (see `count`).
fn finish(cpu: &mut Cpu, visit: &Visit, base: u64, executed: u64, counted: Option<Box<Pass>>) {
    let period = visit.icount - base;
    let executed = visit.executed - executed;
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
        let mut passes = u64::MAX;
        if let Some(pass) = counted {
            let most = ((limit.min(cpu.bus.clock.deadline) - visit.icount) / period).min(COUNTED_PASSES);
            let (same, values) = counted_passes(cpu, &pass, most);
            for (addr, value) in values {
                cpu.bus.write_8(addr, value);
            }
            passes = same;
        }
        let skipped = skip(cpu, period, executed, limit, passes);
        if let Some((port, read)) = held
            && skipped > 0
        {
            let value = cpu.bus.idle_port_value(port, read + skipped);
            cpu.set_reg8(iced_x86::Register::AL, value);
        }
    }
}

/// During `Phase::Count`, the instruction about to run: what it does to
/// the flags, and the RAM it reaches. Of the counters only INC, DEC, ADD,
/// SUB and CMP with a constant may reach them (which `count` follows);
/// anything else ends the proof.
fn record(cpu: &mut Cpu) {
    use iced_x86::{Decoder, DecoderOptions, Mnemonic, OpAccess, OpKind, Register};
    use crate::cpu::alu::ARITH;
    if cpu.cr0 & crate::cpu::CR0_PG != 0 {
        return cpu.bus.observe.fail("paging");
    }
    let cs = cpu.seg_cache(crate::cpu::Seg::CS);
    let bitness = if cs.attr & crate::cpu::ATTR_DB != 0 { 32 } else { 16 };
    let a20 = cpu.bus.a20_mask() as usize;
    let lin = cs.base.wrapping_add(cpu.eip()) as usize & a20;
    let bytes = code_at(cpu, lin);
    let instr = Decoder::with_ip(bitness, &bytes, cpu.eip() as u64, DecoderOptions::NONE).decode();
    let mnemonic = instr.mnemonic();
    // An emulator service trap (which decodes as nothing), and what pushes
    // the flags or goes through the interrupt table: all the flags may be
    // read, and RAM near the stack and the table reached.
    let opaque = instr.is_invalid()
        || matches!(
            mnemonic,
            Mnemonic::Int | Mnemonic::Int1 | Mnemonic::Int3 | Mnemonic::Into | Mnemonic::Iret | Mnemonic::Iretd | Mnemonic::Pushf | Mnemonic::Pushfd
        );
    // (The count is in CX or ECX by the address size.)
    let addr32 = instr.op_kinds().any(|k| matches!(k, OpKind::MemorySegESI | OpKind::MemorySegEDI | OpKind::MemoryESEDI));
    let rep_count = |cpu: &Cpu| if addr32 { cpu.ecx() } else { cpu.cx() as u32 };
    let repeated = instr.is_string_instruction() && (instr.has_rep_prefix() || instr.has_repne_prefix());
    let flags = if opaque {
        (u32::MAX, 0)
    } else {
        let read = rflags(instr.rflags_read());
        let mut kill = rflags(instr.rflags_written() | instr.rflags_cleared() | instr.rflags_set());
        // A shift by a count of 0 and a REP of none leave the flags.
        let shift = matches!(
            mnemonic,
            Mnemonic::Rol | Mnemonic::Ror | Mnemonic::Rcl | Mnemonic::Rcr | Mnemonic::Shl | Mnemonic::Sal | Mnemonic::Shr | Mnemonic::Sar | Mnemonic::Shld | Mnemonic::Shrd
        );
        let count = match instr.op_kinds().last() {
            Some(OpKind::Immediate8) => instr.immediate8() as u32,
            _ => cpu.cx() as u32 & 0xFF,
        };
        if shift && count & 0x1F == 0 || repeated && rep_count(cpu) == 0 {
            kill = 0;
        }
        (read & ARITH, kill & ARITH)
    };
    // The RAM it reaches.
    let mut accesses: Vec<(Access, bool, bool)> = Vec::with_capacity(4);
    let used = FACTORY.with_borrow_mut(|f| f.info(&instr).used_memory().to_vec());
    for m in &used {
        let write = matches!(m.access(), OpAccess::Write | OpAccess::CondWrite | OpAccess::ReadWrite | OpAccess::ReadCondWrite);
        let seg_base = |r: Register| crate::cpu::Seg::from_register(r).map(|s| cpu.seg_cache(s).base as u64);
        let Some(addr) = m.virtual_address(0, |r, _, _| seg_base(r).or(Some(cpu.reg(r) as u64))) else {
            return cpu.bus.observe.fail("count");
        };
        let size = m.memory_size().size().max(1);
        let operand = !repeated && instr.op_count() > 0 && instr.op0_kind() == OpKind::Memory && m.base() == instr.memory_base() && m.displacement() == instr.memory_displacement64();
        let access = if repeated {
            if m.address_size() != iced_x86::CodeSize::Code16 {
                return cpu.bus.observe.fail("count");
            }
            let base = seg_base(m.segment()).unwrap_or(0) as usize;
            let off = addr.wrapping_sub(base as u64) as u32 & 0xFFFF;
            let down = cpu.get_cpu_flag(crate::cpu::CpuFlags::DF);
            Access::Rep { base, off, size: size as u32, count: rep_count(cpu), down }
        } else {
            Access::Range { start: addr as usize & a20, len: size }
        };
        accesses.push((access, write, operand));
    }
    if opaque {
        let ss = cpu.seg_cache(crate::cpu::Seg::SS).base as usize;
        let off = (cpu.sp() as u32).wrapping_sub(64) & 0xFFFF;
        accesses.push((Access::Rep { base: ss, off, size: 1, count: 128, down: false }, true, false));
        accesses.push((Access::Range { start: 0, len: 0x400 }, false, false));
    }
    let op = match (mnemonic, instr.op_count()) {
        (Mnemonic::Inc, 1) => Some(CountOp::Inc),
        (Mnemonic::Dec, 1) => Some(CountOp::Dec),
        (Mnemonic::Add | Mnemonic::Sub | Mnemonic::Cmp, 2) if matches!(instr.op1_kind(), OpKind::Immediate8 | OpKind::Immediate16 | OpKind::Immediate32 | OpKind::Immediate8to16 | OpKind::Immediate8to32) => {
            let imm = instr.immediate(1) as u32;
            Some(match mnemonic {
                Mnemonic::Add => CountOp::Add(imm),
                Mnemonic::Sub => CountOp::Sub(imm),
                _ => CountOp::Cmp(imm),
            })
        }
        _ => None,
    }
    .filter(|_| instr.op0_kind() == OpKind::Memory && !instr.has_lock_prefix());
    // What each operand held before it ran.
    let befores: Vec<u32> = accesses
        .iter()
        .map(|&(access, _, _)| match access {
            Access::Range { start, len } if len <= 4 => {
                (0..len).fold(0u32, |v, i| v | (cpu.bus.peek_8((start + i) & a20) as u32) << (8 * i))
            }
            _ => 0,
        })
        .collect();
    let o = &mut cpu.bus.observe;
    let Phase::Count { pass, .. } = &mut o.phase else { return };
    if pass.flags.len() >= PASS_LIMIT {
        return o.fail("pass");
    }
    let at = pass.flags.len();
    pass.flags.push(flags);
    let mut failed = false;
    for ((access, write, operand), before) in accesses.into_iter().zip(befores) {
        let reaches = pass.counters.iter().any(|&c| access.covers(c));
        match (op, access) {
            (Some(op), Access::Range { start, len }) if operand && reaches && len <= 4 => {
                pass.ops.push(Counting { op, addr: start, size: len as u8, at, before });
            }
            _ if reaches => failed = true,
            _ if write => pass.writes.push(access),
            _ => {}
        }
    }
    if failed {
        o.fail("counter");
    }
}

thread_local! {
    static FACTORY: std::cell::RefCell<iced_x86::InstructionInfoFactory> =
        std::cell::RefCell::new(iced_x86::InstructionInfoFactory::new());
}

/// The 15 bytes an instruction at linear address `lin` may have, with
/// paging off.
fn code_at(cpu: &Cpu, lin: usize) -> [u8; 15] {
    let mut bytes = [0u8; 15];
    if cpu.bus.is_plain_ram(lin, 15) {
        bytes.copy_from_slice(&cpu.bus.ram()[lin..lin + 15]);
    } else {
        let a20 = cpu.bus.a20_mask() as usize;
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = cpu.bus.peek_8((lin + i) & a20);
        }
    }
    bytes
}

/// The arithmetic flags of iced's `RflagsBits`.
fn rflags(bits: u32) -> u32 {
    use crate::cpu::alu::{AF, CF, OF, PF, SF, ZF};
    use iced_x86::RflagsBits as R;
    [(R::OF, OF), (R::SF, SF), (R::ZF, ZF), (R::AF, AF), (R::CF, CF), (R::PF, PF)]
        .iter()
        .fold(0, |f, &(r, x)| if bits & r != 0 { f | x } else { f })
}

/// At the visit after the pass `Phase::Count` recorded, with the
/// registers of its base: the blocks written must be as at the base but
/// for the counters, which only the pass's counting instructions reached
/// (`record`), and which come out as those instructions make them. The
/// passes after it then run the same as long as each counting instruction
/// sets the flags anything after it reads as it did in the recorded pass.
fn count(cpu: &mut Cpu, visit: Visit) {
    let chunks = cpu.bus.observe.take_chunks();
    let o = &mut cpu.bus.observe;
    let Phase::Count { base, copies, pass, .. } = std::mem::take(&mut o.phase) else { return };
    o.phase = Phase::Off;
    let ram = cpu.bus.ram();
    let fail = |cpu: &mut Cpu, why| {
        cpu.bus.observe.on = true;
        cpu.bus.observe.fail(why)
    };
    let counter = |a: usize| pass.counters.binary_search(&a).is_ok();
    for &c in &chunks {
        let Ok(i) = copies.binary_search_by_key(&(c as usize), |(k, _)| *k) else { return fail(cpu, "count blocks") };
        let start = c as usize * CHUNK;
        if (0..CHUNK).any(|b| ram[start + b] != copies[i].1[b] && !counter(start + b)) {
            return fail(cpu, "count blocks");
        }
    }
    // The bytes the counting instructions reach: only they write them, and
    // they change all the counters.
    let mut reached: Vec<usize> = pass.ops.iter().flat_map(|op| op.addr..op.addr + op.size as usize).collect();
    reached.sort_unstable();
    reached.dedup();
    if !pass.counters.iter().all(|c| reached.binary_search(c).is_ok())
        || pass.writes.iter().any(|w| reached.iter().any(|&a| w.covers(a)))
    {
        return fail(cpu, "count writes");
    }
    // The pass as the counting instructions make it, from the bytes at the
    // base: what each found must be what it did find, and the bytes must
    // come out as they are now.
    let at_base = |a: usize| match copies.binary_search_by_key(&(a / CHUNK), |(k, _)| *k) {
        Ok(i) => copies[i].1[a % CHUNK],
        Err(_) => ram[a],
    };
    let mut values: Vec<(usize, u8)> = reached.iter().map(|&a| (a, at_base(a))).collect();
    let mut seen = Vec::with_capacity(pass.ops.len());
    for op in &pass.ops {
        let (before, flags) = run_op(&mut values, op);
        if before != op.before {
            return fail(cpu, "count model");
        }
        seen.push(flags);
    }
    if values.iter().any(|&(a, v)| ram[a] != v) {
        return fail(cpu, "count model");
    }
    // The flags of each that something reads before they are set again (or
    // at the head, whose flags the passes compare).
    let live: Vec<u32> = pass
        .ops
        .iter()
        .map(|op| {
            let mut left = op.op.sets();
            let mut live = 0;
            for &(read, kill) in &pass.flags[op.at + 1..] {
                live |= read & left;
                left &= !kill;
                if left == 0 {
                    break;
                }
            }
            live | left
        })
        .collect();
    let pass = Box::new(Pass { flags: live.iter().zip(&seen).map(|(&l, &f)| (l, f & l)).collect(), ..*pass });
    cpu.bus.observe.on = true;
    finish(cpu, &visit, base.icount, base.executed, Some(pass));
}

/// Run counting instruction `op` on the counters' bytes `values`: the
/// value it found and the flags it set.
fn run_op(values: &mut [(usize, u8)], op: &Counting) -> (u32, u32) {
    let byte = |values: &[(usize, u8)], a: usize| values.iter().find(|&&(b, _)| b == a).map_or(0, |&(_, v)| v);
    let before = (0..op.size as usize).fold(0u32, |v, i| v | (byte(values, op.addr + i) as u32) << (8 * i));
    let (after, flags) = op.op.run(op.size, before);
    if after != before {
        for i in 0..op.size as usize {
            if let Some(slot) = values.iter_mut().find(|(b, _)| *b == op.addr + i) {
                slot.1 = (after >> (8 * i)) as u8;
            }
        }
    }
    (before, flags)
}

/// How many of the next passes, up to `most`, run as the recorded one
/// (`count`): those whose counting instructions set the flags read after
/// them (`Pass::flags`, live and as seen) as it did. Also the counters'
/// bytes after them.
fn counted_passes(cpu: &Cpu, pass: &Pass, most: u64) -> (u64, Vec<(usize, u8)>) {
    let mut reached: Vec<usize> = pass.ops.iter().flat_map(|op| op.addr..op.addr + op.size as usize).collect();
    reached.sort_unstable();
    reached.dedup();
    let mut values: Vec<(usize, u8)> = reached.iter().map(|&a| (a, cpu.bus.ram()[a])).collect();
    let mut done = values.clone();
    let mut passes = 0;
    'passes: while passes < most {
        for (op, &(live, seen)) in pass.ops.iter().zip(&pass.flags) {
            let (_, flags) = run_op(&mut values, op);
            if flags & live != seen {
                break 'passes;
            }
        }
        passes += 1;
        done.clone_from(&values);
    }
    (passes, done)
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
    if !cpu.bus.observe.tainted {
        // Only an IN AL matters while AL holds no port's value: past the
        // prefixes, E4h or ECh (or E5h and EDh, which the decoder then
        // turns away). Decoding every instruction would cost more than
        // the loops it looks at.
        let mut at = lin as usize;
        let mut opcode = cpu.bus.peek_8(at);
        for _ in 0..4 {
            if !matches!(opcode, 0x26 | 0x2E | 0x36 | 0x3E | 0x64 | 0x65 | 0x66 | 0x67 | 0xF0 | 0xF2 | 0xF3) {
                break;
            }
            at += 1;
            opcode = cpu.bus.peek_8(at);
        }
        if !matches!(opcode, 0xE4 | 0xE5 | 0xEC | 0xED) {
            return;
        }
    }
    let bytes = code_at(cpu, lin as usize);
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
/// the loop reads changes; `most` passes at most.
/// Returns the instruction count skipped.
fn skip(cpu: &mut Cpu, period: u64, executed: u64, limit: u64, most: u64) -> u64 {
    let clock = &mut cpu.bus.clock;
    let passes = (limit.min(clock.deadline).saturating_sub(clock.icount) / period).min(most);
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
