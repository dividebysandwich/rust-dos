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
//!   BIOS keyboard's status calls and the built-in Windows's clock ends the
//!   proof;
//! - no interrupt comes before the next event, which ends it too.
//!
//! The passes skipped all end by the next timer event, as they would have
//! run, and count as executed instructions: the registers, memory and
//! counts come out as without the skip.
//!
//! **Loops that read the time.** A loop that latches the PIT's count and
//! reads it in every pass, as Windows 98's QueryPerformanceCounter does,
//! never leaves the machine as it was, but may leave it the same function
//! of the count it read. A proof that sees the count latched hands the loop
//! to `time`, which proves that and skips such passes from the instruction
//! that latches.
//!
//! **Loops that read the clock.** The built-in Windows's GetTickCount and
//! timeGetTime read the emulated clock in milliseconds, with no device or
//! state of their own, so a pass that calls them is the same pass again
//! until the next millisecond. The passes skipped end before it, and the
//! loop is proven again from there (`clock_read`).
//!
//! **Where loops come from.** A head is armed at the service trap where the
//! BIOS keyboard's status call found no key, the built-in Windows's clock
//! was read or its Sleep(0) found no other thread to run many times in a
//! row, after many reads of the VGA's status or latches of the PIT's count
//! with no other port between them, and where two timer events in a row
//! found the CPU at the same instruction with the same flags and registers
//! but for one (a loop, whose register may be a time read that the event
//! between moved on). A head whose passes were skipped is armed again at
//! the next event, as its loop usually goes on after it, and whenever an
//! event finds the CPU there later, as a game comes back to its frame
//! limiter every frame. Failed proofs at a head make the next tries at it
//! rarer.

mod count;
mod time;

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
/// Keyboard status calls finding no key, or reads of the built-in Windows's
/// clock, at one place in a row before it is armed.
const POLLS: u32 = 8;
/// Heads whose failed proofs are remembered, to arm them less often.
const BACKOFF_HEADS: usize = 8;
/// Heads whose passes were skipped that are remembered, to arm them when
/// an event finds the CPU there.
const KNOWN_HEADS: usize = 16;
/// Reads of the VGA's status in a row before the loop is armed.
const STATUS_POLLS: u32 = 64;
/// Bytes of RAM a pass may change and still be skipped, as counters (see
/// `Phase::Count`).
const COUNTER_BYTES: usize = 16;
/// Instructions of a pass `Phase::Count` records.
const PASS_LIMIT: usize = 20_000;
/// Bytes either side of the stack's top that a pass `Phase::Count`
/// records takes a service, or an instruction that pushes the flags or
/// goes through the interrupt table, to reach (`record`).
const STACK_REACH: u32 = 64;
/// Bytes a REP string instruction with 32-bit addresses may reach in a
/// pass `Phase::Count` records.
const REP_SPAN: u64 = 1 << 20;
/// Passes skipped at most over counters at once.
const COUNTED_PASSES: u64 = 1 << 20;
/// Instructions the proofs over counts (`Phase::Count`) at a head must
/// save for each instruction the proof after each runs, or the head backs
/// off as after a failure. Such a proof steps the interpreter and records
/// every instruction: in Metal Marines' title loop, with paging on, that
/// takes the host about 350 ns an instruction, where translated code
/// takes one or two. A loop that holds only until the clock's next
/// millisecond and takes a tenth of it to prove costs more than it saves.
/// The proof after one that found the loop's shape runs one pass
/// (`Shape`), the first that finds it several. The head's recent proofs
/// are taken together (`Ledger`): one that ends late in a batch skips
/// less than the one before it.
const PAYOFF: u64 = 200;
/// The most a head whose proofs cost a little more than they save backs
/// off: to one chance in 64, and 16 times as far each time what they
/// cost doubles against what they save, up to one in 64K. Its proofs
/// hold, and the loop may save more later: Metal Marines' title loop has
/// spells where a timer event comes every half millisecond, and skips of
/// less than one don't pay, and spells where the skips run to the next
/// millisecond, which do.
const COSTLY_BACKOFF: u32 = 6;
/// How much less a proof counts in a head's ledger (`Ledger`) with each
/// proof after it: by a 64th. A ledger then covers about the last 64
/// proofs, so that one failure, which may run many passes, doesn't
/// outweigh the dozens of proofs of one pass each that come with it in
/// Metal Marines' title loop.
const LEDGER_DECAY: u64 = 64;
/// What a call of the built-in Windows that the passes skipped would have
/// made costs the host, in instructions of translated code as `PAYOFF`
/// counts them (one or two nanoseconds each): its trap, the Rust function
/// behind it and the way back. A pass of Metal Marines' title loop, about
/// 600 instructions, makes seven Win16 calls, five of which go on to a
/// Win32 function and back: about 17 traps. On main the host took 19 ns
/// an instruction there, two thirds of it in the calls, about 450 ns a
/// call.
const SERVICE_COST: u64 = 256;

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
    /// `Win32::calls`.
    calls: u64,
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
    /// `loose`: the general registers that hold counts (`count`), which
    /// differ from pass to pass.
    Verify {
        head: Head,
        base: Box<Visit>,
        copies: Vec<(usize, [u8; CHUNK])>,
        until: u64,
        loose: u8,
    },
    /// The registers came round but for counts in `loose`, and the blocks
    /// but for a few bytes: one pass from `base` is recorded, following
    /// the counts through it (`count`), to find out whether the passes
    /// after it run the same until their counts reach where one of them
    /// would take another way. `rounds`: passes recorded before this one,
    /// which found counts in more places.
    Count {
        head: Head,
        base: Box<Visit>,
        copies: Vec<(usize, [u8; CHUNK])>,
        pass: Box<count::Pass>,
        until: u64,
        loose: u8,
        rounds: u8,
    },
    /// A loop that reads the time from the PIT (see `time`).
    Time(Box<time::TimeProof>),
}

/// What a proof over counts found at a head: the cells holding counts
/// there and the blocks its pass wrote. The pass `Phase::Count` records
/// from a visit of the head, with copies of the blocks, proves the loop by
/// itself: it compares all the CPU's state and every block written at the
/// next visit, and the passes after it run as the model of it makes them.
/// The passes before it that `Phase::Learn` and `Phase::Verify` run only
/// find the counts and the blocks, and a loop whose skips end at every
/// millisecond, as one that reads the clock does in deterministic runs,
/// would run them again each time. With its shape, the next proof at the
/// head records the first pass from where it starts. Where the loop wrote
/// another block or counted elsewhere, that proof fails, and the head's
/// next proof finds them again.
struct Shape {
    head: Head,
    seeds: Vec<count::Cell>,
    blocks: Vec<usize>,
}

/// What the proofs at a head saved, and what they cost (`PAYOFF`): the
/// instructions each failed proof ran, and for each that skipped, what
/// the proof after it is expected to run. Each proof counts less by
/// `LEDGER_DECAY` with every proof after it.
struct Ledger {
    head: Head,
    saved: u64,
    cost: u64,
}

/// A head whose pass used the time the clock read in a way a count proof
/// doesn't follow, as Tribal Rage's does with FILD in the pass that
/// draws a frame once the time comes. Its next `left` proofs that skip
/// hold what the clock read to its next millisecond (`tick_count`), as
/// for any other loop; then they follow the clock again. `run` doubles
/// with each such failure in a row, up to 64, and a proof that follows
/// the clock and skips ends it.
struct PlainClock {
    head: Head,
    left: u32,
    run: u32,
}

/// Bytes of RAM an instruction reached: `len` from `start`.
#[derive(Clone, Copy)]
struct Access {
    start: usize,
    len: usize,
}

impl Access {
    fn covers(self, addr: usize) -> bool {
        addr.wrapping_sub(self.start) < self.len
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
    /// Heads armed, chances to arm one let pass after failed proofs,
    /// proofs started at one, and proofs a timer event or an interrupt
    /// ended before they came to anything.
    pub arms: u64,
    pub backed_off: u64,
    pub starts: u64,
    pub cut: u64,
    /// Proofs after which the head's proofs together saved less than
    /// `PAYOFF` times the instructions they ran, which back off as
    /// failures do.
    pub costly: u64,
    /// The instructions the proofs that skipped ran, from the visit of
    /// the head that started them to the one that proved the loop.
    pub proving: u64,
    /// Of the proofs that skipped, those that started from what the last
    /// proof at their head found (`Shape`).
    pub shaped: u64,
    /// The failures by why, in the order the reasons first came up (an
    /// empty one is a free slot).
    pub failed_by: [(&'static str, u64); 16],
    /// The same counts by head, for the first heads armed.
    pub heads: [HeadStats; 16],
}

/// `IdleStats` of one head: its CS:EIP and mode, and the last reason a
/// proof at it failed. A free slot has no arms.
#[derive(Clone, Copy, Debug, Default)]
pub struct HeadStats {
    pub cs: u16,
    pub eip: u32,
    pub pe: bool,
    pub arms: u64,
    pub starts: u64,
    pub proofs: u64,
    pub failures: u64,
    pub cut: u64,
    pub failure: &'static str,
}

/// The prover's state. It is the bus's, whose memory and port accesses
/// report to it while a proof runs.
pub struct Observe {
    /// A proof runs: the bus notes writes and anything else.
    pub on: bool,
    /// Skipping is on.
    pub enabled: bool,
    /// Set by the BIOS keyboard's status call finding no key, and by a
    /// Sleep(0) of the built-in Windows that found no other thread to run.
    pub polled: bool,
    /// Set by an access the proof can't follow, to what it was.
    failed: Cell<Option<&'static str>>,
    /// The blocks written since the last look.
    chunks: Vec<u32>,
    phase: Phase,
    /// The millisecond the first read of the built-in Windows's clock since
    /// the base found (`clock_read`), up to which its passes read the same.
    clock_read: Option<u64>,
    /// `Clock::run_count` at the last read of that clock.
    clock_read_at: u64,
    /// Where the keyboard status call found no key or the clock was read
    /// last, and how many times in a row.
    poll: (Option<Head>, u32),
    /// Where the CPU was at the last timer event, with its general
    /// registers and flags.
    last_event: Option<(Head, [u32; 8], u32)>,
    /// The head whose passes were skipped up to the next event.
    proven: Option<Head>,
    /// Whether `proven` latches the PIT's count: a time proof starts
    /// there again at once (`time`).
    proven_time: bool,
    /// The instruction count when the proof running started.
    started: u64,
    /// The last `KNOWN_HEADS` heads whose passes were skipped, oldest
    /// first.
    known: Vec<Head>,
    /// What the last proof over counts at each of the last `KNOWN_HEADS`
    /// such heads found, oldest first: a proof there starts recording a
    /// pass at once (`Shape`).
    shapes: Vec<Shape>,
    /// The ledgers of the last `KNOWN_HEADS` heads proven over counts or
    /// failed, oldest first.
    ledgers: Vec<Ledger>,
    /// The instruction count at the last visit of a head.
    seen: u64,
    /// The shape the proof over counts running found, kept once it skips.
    found: Option<Shape>,
    /// The proof running started from the shape of its head.
    shaped: bool,
    /// Heads whose passes used the time the clock read in a way a count
    /// proof doesn't follow, the most recent `KNOWN_HEADS` (`PlainClock`).
    plain_clock: Vec<PlainClock>,
    /// The instructions `count::record` decoded, by place
    /// (`count::Decoded`), and the buffers it fills for each.
    decoded: Vec<Option<std::sync::Arc<count::Decoded>>>,
    scratch: (Vec<Access>, Vec<Access>),
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
    /// Latches of the PIT's count with no other port access between them,
    /// which arm the loop too.
    latch_polls: u32,
    /// A proof that isn't a time proof read the PIT's channel 0.
    pit_read: bool,
    /// The heads whose last proofs failed, the most recent `BACKOFF_HEADS`.
    backoffs: Vec<Backoff>,
    /// The instruction running, during a proof.
    current: Option<Head>,
    /// Where a proof saw the PIT's count latched: a loop that reads the
    /// time, proven from there instead (`time`).
    latch_head: Option<Head>,
    /// When a time proof's pass latched the count.
    latch_at: Option<u64>,
    /// The bytes written since the last look, `start..end`, during a time
    /// proof.
    writes: Vec<(usize, usize)>,
    /// The built-in Windows' service running said it changed nothing but
    /// RAM and registers (`pure_call`).
    pure: bool,
    /// It said too that it read no RAM but its arguments on the stack
    /// and what it names (`reads_arguments`), as a pass `Phase::Count`
    /// records needs where RAM holds counts.
    arguments: bool,
    /// Of the service running: the linear address of the stack's top at
    /// its trap, and whether the stack is 32-bit; the general registers
    /// there; its arguments, the bytes from the top that hold them and
    /// the return address (`arguments_at`); and what else it said it read
    /// (`reads_also`), as linear addresses and lengths.
    service_top: u32,
    service_flat: bool,
    service_regs: [u32; 8],
    service_args: Option<(u32, u32)>,
    service_reads: Vec<(u32, u32)>,
    /// The function running set EAX to the value it returns
    /// (`returned`).
    service_returned: bool,
    /// The function running read the built-in Windows' clock as the value
    /// it returns (`tick_count`): the milliseconds, the instruction count
    /// it read them at, and when they move on.
    service_clock: Option<(u32, u64, u64)>,
    /// The function running was Win16Return, which loaded the 16-bit
    /// context with DX:AX the value in EAX if true (`resumed16`).
    resumed16: Option<bool>,
    /// The last service's read of the clock is one the pass `Phase::Count`
    /// records follows (`count::after_service`), which `clock_read`
    /// doesn't hold the proof to.
    clock_modelled: bool,
    /// Built-in functions a service calls from Rust that run (`nested`).
    nesting: u8,
    /// Since the base: the first instruction count at which what the
    /// services read would read otherwise, and when the last service
    /// started the thread's turn over.
    holds_until: Option<u64>,
    turn_at: Option<u64>,
    pub stats: IdleStats,
    /// Since when the execution loop has been watching (one instruction
    /// at a time on the interpreter), with the clock's run count then
    /// (`run_count`).
    watch_since: Option<(web_time::Instant, u64)>,
    /// The host time spent watching and the instructions run meanwhile,
    /// since `take_watched`.
    watched: (std::time::Duration, u64),
}

/// Chances to arm `head` to let pass (`left`), after `in_row` failed proofs
/// at it in a row. Each head has its own, so a loop that fails (as the
/// BIOS's at boot) doesn't keep the detector from another one.
struct Backoff {
    head: Head,
    left: u32,
    in_row: u32,
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
            clock_read: None,
            clock_read_at: 0,
            poll: (None, 0),
            last_event: None,
            proven: None,
            proven_time: false,
            started: 0,
            known: Vec::new(),
            shapes: Vec::new(),
            ledgers: Vec::new(),
            seen: 0,
            found: None,
            shaped: false,
            plain_clock: Vec::new(),
            decoded: Vec::new(),
            scratch: (Vec::new(), Vec::new()),
            port_ok: None,
            ports: Vec::new(),
            first_read: None,
            tainted: false,
            al_port: 0,
            al_read: 0,
            polls: 0,
            latch_polls: 0,
            pit_read: false,
            backoffs: Vec::new(),
            current: None,
            latch_head: None,
            latch_at: None,
            writes: Vec::new(),
            pure: false,
            arguments: false,
            service_top: 0,
            service_flat: false,
            service_regs: [0; 8],
            service_args: None,
            service_reads: Vec::new(),
            service_returned: false,
            service_clock: None,
            resumed16: None,
            clock_modelled: false,
            nesting: 0,
            holds_until: None,
            turn_at: None,
            stats: IdleStats::default(),
            watch_since: None,
            watched: (std::time::Duration::ZERO, 0),
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

    /// The execution loop watches from here, with the clock's run count
    /// `run`.
    #[cold]
    pub fn watch_from(&mut self, run: u64) {
        self.watch_since.get_or_insert_with(|| (web_time::Instant::now(), run));
    }

    /// The execution loop stopped watching, at run count `run`.
    #[cold]
    pub fn watch_to(&mut self, run: u64) {
        if let Some((since, from)) = self.watch_since.take() {
            self.watched.0 += since.elapsed();
            self.watched.1 += run.saturating_sub(from);
        }
    }

    /// Whether the execution loop is in a span of watching.
    #[inline(always)]
    pub fn in_watch(&self) -> bool {
        self.watch_since.is_some()
    }

    /// The host time spent watching and the instructions run meanwhile
    /// since the last call, in the batches run since (`run_batch` ends a
    /// span of watching when it returns). Watching costs far more per
    /// instruction than running the code does, and comes with timer events
    /// rather than with the instructions a frame runs: the host speed
    /// estimate counts it as a frame's fixed work (`Pacer::end_cpu_frame`).
    pub fn take_watched(&mut self) -> (std::time::Duration, u64) {
        std::mem::take(&mut self.watched)
    }

    /// A state was loaded: nothing armed or proven before it goes on, as
    /// in a machine that starts from the state. What the prover keeps
    /// counts instructions of the run before the load (a proof's limit,
    /// the last timer event, the last read of the clock), and after a
    /// load that moves the count back, an armed head would keep the
    /// execution loop watching, one instruction at a time on the
    /// interpreter, until the count caught up. Whether skipping is on,
    /// the statistics and the time spent watching stay.
    pub fn after_load(&mut self) {
        *self = Observe {
            enabled: self.enabled,
            stats: self.stats,
            watched: self.watched,
            ..Observe::default()
        };
    }

    /// The bytes of RAM `start..end` were written.
    #[cold]
    pub fn wrote(&mut self, start: usize, end: usize) {
        self.note_write(start, end, true);
    }

    /// RAM `start..end` was written: `exact`ly those bytes, or some of
    /// them.
    fn note_write(&mut self, start: usize, end: usize, exact: bool) {
        if self.timing() {
            self.writes.push((start, end));
        }
        if let Phase::Count { pass, .. } = &mut self.phase
            && !pass.wrote(start, end, exact)
        {
            self.opaque_because("pass");
        }
        for chunk in start >> GEN_SHIFT..=(end - 1) >> GEN_SHIFT {
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
    /// the loop they are in. A proof that isn't a time proof lets the PIT's
    /// channel 0 be read until it latches the count, where a time proof
    /// starts, but fails at the head before that.
    #[inline(always)]
    pub fn port_read(&mut self, port: u16) {
        if port != 0x40 {
            self.latch_polls = 0;
        }
        if self.on {
            if port == 0x40 && !self.timing() {
                self.pit_read = true;
                self.port_ok = None;
            } else if self.port_ok.take() != Some(port) {
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

    /// A port is written, at instruction count `at`. A command that
    /// latches the PIT's channel 0 is a read of the time, which a time
    /// proof follows (`time`); in another proof it starts one where it is.
    #[inline(always)]
    pub fn port_write(&mut self, port: u16, value: u8, at: u64) {
        self.polls = 0;
        let latch = port == 0x43 && value & 0xF0 == 0;
        if !latch {
            self.latch_polls = 0;
        } else if !self.on {
            // Latches with no other port between them: a loop that reads
            // the time, armed at the next instruction.
            self.latch_polls += 1;
            if self.latch_polls >= STATUS_POLLS {
                self.latch_polls = 0;
                self.arm_here();
            }
        }
        if self.on && latch {
            if self.timing() {
                if self.latch_at.replace(at).is_some() {
                    self.opaque_because("time latches");
                }
            } else {
                self.latch_head = self.current;
            }
            return;
        }
        self.opaque_because("port write");
    }

    /// A time proof's pass runs: the bytes written are noted one by one.
    #[inline(always)]
    pub fn timing(&self) -> bool {
        self.on && matches!(self.phase, Phase::Time(_))
    }

    /// Arm the loop the CPU is in, at the next instruction.
    #[cold]
    fn arm_here(&mut self) {
        if !self.enabled || self.watching() {
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
    /// it, and the RAM between them is noted, but for a time proof, which
    /// follows the bytes written one by one and isn't told them here. (Of
    /// the four bytes noted it may write fewer.)
    #[cold]
    pub fn mapped_write(&mut self, addr: usize, ram_len: usize) {
        if (0xC_0000..0x10_0000).contains(&addr) && addr + 4 <= ram_len && !self.timing() {
            self.note_write(addr, addr + 4, false);
        } else {
            self.opaque_because("device write");
        }
    }

    fn arm(&mut self, head: Head, now: u64) {
        if !self.enabled || self.watching() || self.backs_off(head) {
            return;
        }
        self.stats.arms += 1;
        self.tally(Some(head), |h| h.arms += 1);
        self.phase = Phase::Armed { head, until: now + ARM_LIMIT };
    }

    fn stop(&mut self) {
        self.on = false;
        self.failed.set(None);
        self.chunks.clear();
        self.phase = Phase::Off;
        self.forget_ports();
        self.tainted = false;
        self.latch_head = None;
        self.latch_at = None;
        self.pit_read = false;
        self.clock_read = None;
        self.clock_modelled = false;
        self.writes.clear();
        self.found = None;
        self.shaped = false;
    }

    fn forget_ports(&mut self) {
        self.port_ok = None;
        self.ports.clear();
        self.first_read = None;
        self.holds_until = None;
        self.turn_at = None;
    }

    /// From a new base on, the ports and the clock read count from there,
    /// but for the value of one AL still holds, which the passes after it
    /// may test.
    fn restart_ports(&mut self) {
        self.forget_ports();
        self.clock_read = None;
        if self.tainted {
            self.ports.push((self.al_port, 0));
            self.first_read = Some(self.al_read);
        }
    }

    /// The head armed or proven at, if any.
    fn head(&self) -> Option<Head> {
        match &self.phase {
            Phase::Off | Phase::ArmHere => None,
            Phase::Armed { head, .. }
            | Phase::Learn { head, .. }
            | Phase::Verify { head, .. }
            | Phase::Count { head, .. } => Some(*head),
            Phase::Time(t) => Some(t.head),
        }
    }

    /// Count something for `head` in `stats.heads`, if it has a slot.
    fn tally(&mut self, head: Option<Head>, f: impl FnOnce(&mut HeadStats)) {
        let Some(head) = head else { return };
        let slot = self
            .stats
            .heads
            .iter_mut()
            .find(|h| (h.cs, h.eip, h.pe) == (head.cs, head.eip, head.pe) || h.arms == 0);
        if let Some(h) = slot {
            (h.cs, h.eip, h.pe) = (head.cs, head.eip, head.pe);
            f(h);
        }
    }

    /// A timer event or an interrupt ended the proof running.
    fn cut(&mut self) {
        self.stats.cut += 1;
        self.tally(self.head(), |h| h.cut += 1);
        self.stop();
    }

    /// Whether a chance to arm `head` is let pass, after failed proofs at
    /// it.
    fn backs_off(&mut self, head: Head) -> bool {
        let Some(b) = self.backoffs.iter_mut().find(|b| b.head == head) else {
            return false;
        };
        if b.left == 0 {
            return false;
        }
        b.left -= 1;
        self.stats.backed_off += 1;
        true
    }

    fn fail(&mut self, why: &'static str) {
        let head = self.head();
        if self.shaped {
            self.shapes.retain(|s| Some(s.head) != head);
        }
        // What the failed proof ran counts against the proofs that skip
        // there: a loop that fails every other proof pays for two. A pass
        // that fails before it comes back to the head ran what it
        // recorded. Only heads proven over counts keep a ledger.
        let counting = matches!(self.phase, Phase::Count { .. });
        if let Some(head) = head
            && (counting || self.ledgers.iter().any(|l| l.head == head))
        {
            let recorded = match &self.phase {
                Phase::Count { pass, .. } => pass.recorded(),
                _ => 0,
            };
            let ran = self.seen.saturating_sub(self.started) + recorded;
            self.charge(head, 0, ran);
        }
        // A pass that used the time the clock read in a way the model
        // doesn't follow: the next proofs at the head hold that time to the
        // clock's next millisecond (`PlainClock`), and the next comes at
        // the head's next chance.
        let clocked = matches!(&self.phase, Phase::Count { pass, .. } if pass.reads_clock());
        if let (true, Some(head)) = (clocked, head) {
            match self.plain_clock.iter_mut().find(|p| p.head == head) {
                Some(p) => {
                    p.run = (p.run * 2).min(64);
                    p.left = p.run;
                }
                None => {
                    if self.plain_clock.len() == KNOWN_HEADS {
                        self.plain_clock.remove(0);
                    }
                    self.plain_clock.push(PlainClock {
                        head,
                        left: 1,
                        run: 1,
                    });
                }
            }
        }
        self.tally(head, |h| {
            h.failures += 1;
            h.failure = why;
        });
        self.stop();
        self.stats.failures += 1;
        self.stats.failure = why;
        if let Some(slot) = self
            .stats
            .failed_by
            .iter_mut()
            .find(|(w, _)| *w == why || w.is_empty())
        {
            *slot = (why, slot.1 + 1);
        }
        if let (false, Some(head)) = (clocked, head) {
            self.back_off(head, 16);
        }
    }

    /// Arm `head` less often: twice as seldom as the last time in a row,
    /// up to once in `1 << most` arms. (A loop that changes something
    /// every pass, as DOS 7's keyboard poll counts down, is looked at
    /// about once in 64K arms.)
    fn back_off(&mut self, head: Head, most: u32) {
        let at = match self.backoffs.iter().position(|b| b.head == head) {
            Some(at) => at,
            None => {
                if self.backoffs.len() == BACKOFF_HEADS {
                    self.backoffs.remove(0);
                }
                self.backoffs.push(Backoff {
                    head,
                    left: 0,
                    in_row: 0,
                });
                self.backoffs.len() - 1
            }
        };
        let b = &mut self.backoffs[at];
        b.in_row = (b.in_row + 1).min(most);
        b.left = 1 << b.in_row;
    }

    /// A proof at `head` came through: it is armed as often as it comes up
    /// again.
    fn succeed(&mut self, head: Option<Head>) {
        self.backoffs.retain(|b| Some(b.head) != head);
    }

    /// Passes were skipped at `head`: it is armed again at the next event,
    /// and whenever an event finds the CPU there.
    fn proved(&mut self, head: Option<Head>) {
        self.tally(head, |h| h.proofs += 1);
        self.proven = head;
        self.proven_time = false;
        if let Some(head) = head {
            self.known.retain(|&k| k != head);
            if self.known.len() == KNOWN_HEADS {
                self.known.remove(0);
            }
            self.known.push(head);
        }
    }

    /// Adds `saved` and `cost` to `head`'s ledger, after the decay, and
    /// returns it.
    fn charge(&mut self, head: Head, saved: u64, cost: u64) -> (u64, u64) {
        let at = match self.ledgers.iter().position(|l| l.head == head) {
            Some(at) => at,
            None => {
                if self.ledgers.len() == KNOWN_HEADS {
                    self.ledgers.remove(0);
                }
                self.ledgers.push(Ledger {
                    head,
                    saved: 0,
                    cost: 0,
                });
                self.ledgers.len() - 1
            }
        };
        let l = &mut self.ledgers[at];
        l.saved = l.saved - l.saved / LEDGER_DECAY + saved;
        l.cost = l.cost - l.cost / LEDGER_DECAY + cost;
        (l.saved, l.cost)
    }

    /// Keep what a proof over counts found at its head, for the next proof
    /// there.
    fn keep_shape(&mut self, shape: Shape) {
        self.shapes.retain(|s| s.head != shape.head);
        if self.shapes.len() == KNOWN_HEADS {
            self.shapes.remove(0);
        }
        self.shapes.push(shape);
    }

    /// A proof saw the PIT's count latched at `head`: the loop reads the
    /// time, and is proven from there as such (`time`), unless proofs at
    /// `head` failed lately.
    fn time_from(&mut self, head: Head, now: u64) {
        self.stop();
        if !self.enabled || self.backs_off(head) {
            return;
        }
        self.stats.arms += 1;
        self.tally(Some(head), |h| h.arms += 1);
        self.phase = Phase::Time(Box::new(time::TimeProof::new(head, now + ARM_LIMIT)));
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

/// Before a service of the built-in Windows runs during a proof: one the
/// proof can't follow, unless it says otherwise (`pure_call`). In a pass
/// `Phase::Count` records, where the stack's top is and the registers
/// are, which `after_service` looks at.
#[inline(always)]
pub fn before_service(cpu: &mut Cpu) {
    cpu.bus.observe.pure = false;
    cpu.bus.observe.arguments = false;
    if matches!(cpu.bus.observe.phase, Phase::Count { .. }) {
        let ss = cpu.seg_cache(crate::cpu::Seg::SS);
        let flat = ss.attr & crate::cpu::ATTR_DB != 0;
        let top = ss.base.wrapping_add(cpu.esp());
        let regs = cpu.snapshot().gpr;
        let o = &mut cpu.bus.observe;
        o.service_top = top;
        o.service_flat = flat;
        o.service_regs = regs;
        o.service_args = None;
        o.service_reads.clear();
        o.service_returned = false;
        o.service_clock = None;
        o.resumed16 = None;
    }
}

/// After it, during a proof: a service that said it changed nothing but
/// RAM and registers is followed, but not in a time proof, which doesn't
/// look at `holds_until`, nor one that found a page missing, which runs
/// again after the system's handler. Any other ends the proof. In a pass
/// `Phase::Count` records it must also have kept the registers that hold
/// counts and read no RAM that does (`count::after_service`): the passes
/// after it may otherwise not run the same.
pub fn after_service(cpu: &mut Cpu) {
    let faulted = cpu.bus.guest_faulted();
    let o = &mut cpu.bus.observe;
    if !(o.pure && !faulted && !matches!(o.phase, Phase::Time(_))) {
        o.opaque();
    } else if matches!(o.phase, Phase::Count { .. }) {
        count::after_service(cpu);
    }
}

/// The built-in Windows' service running changed nothing but RAM and
/// registers, and what it read of the machine stays as it was until the
/// instruction count `until` (None: as long as nothing else changes it).
/// A service that says so takes its arguments on the stack, reads no
/// register but ESP (nor the flags), and keeps EBX, ESI, EDI and EBP, as
/// the built-in Windows' functions do (stdcall): a count may be in those
/// across it (`count`).
/// These are the services a Windows 3.1 program's PeekMessage loop calls
/// in every pass: GetCurrentTime, a PeekMessage that finds nothing, and
/// the thunks between them (Johnny Castaway's animation loop, #358).
/// What they keep outside RAM is as it was after every pass, but for the
/// count of calls and when the thread's turn started (`turn_started`),
/// which a skip moves on as the passes would have, and the thread's last
/// error. A service may set that (a SetFilePointer that leaves the pointer
/// where it was sets it to 0): only services the proof can't follow read
/// it, and every pass sets it as the one before did, so the passes skipped
/// leave it as the last pass run left it. The call trace in the log counts
/// only the calls that ran.
pub fn pure_call(cpu: &mut Cpu, until: Option<u64>) {
    if !cpu.bus.observe.on {
        return;
    }
    let o = &mut cpu.bus.observe;
    o.pure = true;
    if let Some(until) = until {
        o.holds_until = Some(o.holds_until.map_or(until, |h| h.min(until)));
    }
}

/// The service running that called `pure_call` read the general
/// registers too, which the services `pure_call` describes don't: a
/// Win16 function of the register convention, as KERNEL's Catch, which
/// writes them into the program's buffer. A proof whose passes differ in
/// a count kept in one of them can't follow it (`count`); one whose
/// passes leave everything as it was can.
pub fn reads_registers(cpu: &mut Cpu) {
    if cpu.bus.observe.on {
        count::registers_read(cpu);
    }
}

/// The service running changed more after all: what it called it
/// `pure_call` for did, but not what came after.
pub fn not_pure(cpu: &mut Cpu) {
    cpu.bus.observe.pure = false;
}

/// The service running read no RAM but its arguments on the stack
/// (`arguments_at`, or the `STACK_REACH` bytes above the stack's top
/// where the call doesn't say where they are) and what it names with
/// `reads_also`. A count elsewhere then can't change what it does, and a
/// pass that counts may call it. What it wrote is noted as it writes it,
/// wherever it is.
pub fn reads_arguments(cpu: &mut Cpu) {
    cpu.bus.observe.arguments = true;
}

/// The built-in Windows' function running takes `count` dwords of
/// arguments above the return address at the stack offset `esp`.
#[inline(always)]
pub fn arguments_at(cpu: &mut Cpu, esp: u32, count: u32) {
    if cpu.bus.observe.on {
        let lin = cpu.seg_cache(crate::cpu::Seg::SS).base.wrapping_add(esp);
        cpu.bus.observe.service_args = Some((lin, 4 + 4 * count));
    }
}

/// The Win16 function running takes `bytes` bytes of arguments above its
/// far return address on the 16-bit stack (`builtin::call` read them).
#[inline(always)]
pub fn arguments16(cpu: &mut Cpu, bytes: u32) {
    if cpu.bus.observe.on {
        let ss = cpu.seg_cache(crate::cpu::Seg::SS).base;
        let lin = ss.wrapping_add(cpu.sp() as u32);
        cpu.bus.observe.service_args = Some((lin, 4 + bytes));
    }
}

/// The built-in Windows' function running returned a value in EAX, which
/// came from what it read: a count EAX held is gone.
#[inline(always)]
pub fn returned(cpu: &mut Cpu) {
    if cpu.bus.observe.on {
        cpu.bus.observe.service_returned = true;
    }
}

/// The service running read the `len` bytes at the linear address `lin`
/// too, beyond its arguments: an interface block, an array the caller
/// passed (`reads_arguments`).
#[inline(always)]
pub fn reads_also(cpu: &mut Cpu, lin: u32, len: u32) {
    if cpu.bus.observe.on {
        cpu.bus.observe.service_reads.push((lin, len));
    }
}

/// Run `f`, a built-in function the service running calls from Rust (as
/// a tree view calls GetTickCount). The service stays pure only if what
/// it said before the call holds and `f` says it changed nothing but RAM
/// and registers too (`pure_call` in `f`, not left to the service's
/// claim alone). A function that changes more ends the service's purity
/// even when the service had said it changed nothing. What the calls
/// read still holds only as long as they say (`holds_until`).
pub fn nested<T>(cpu: &mut Cpu, f: impl FnOnce(&mut Cpu) -> T) -> T {
    let claimed = cpu.bus.observe.pure;
    let arguments = cpu.bus.observe.arguments;
    cpu.bus.observe.pure = false;
    cpu.bus.observe.arguments = false;
    cpu.bus.observe.nesting += 1;
    let value = f(cpu);
    cpu.bus.observe.nesting -= 1;
    cpu.bus.observe.pure &= claimed;
    cpu.bus.observe.arguments &= arguments;
    value
}

/// The service `pure_call` follows started the running thread's turn
/// over, now.
pub fn turn_started(cpu: &mut Cpu) {
    let now = cpu.bus.clock.icount;
    if cpu.bus.observe.on {
        cpu.bus.observe.turn_at = Some(now);
    }
}

/// The milliseconds since the machine started, by its clock, as
/// GetTickCount reads them. The service changes nothing, and what it read
/// stays the same until the next millisecond. A pass `Phase::Count`
/// records follows the value where the service returns it (as
/// GetTickCount and timeGetTime do, but not a function that calls them
/// from Rust), and the passes after it read it anew; elsewhere they hold
/// up to the next millisecond.
pub fn tick_count(cpu: &mut Cpu) -> u32 {
    let micros = cpu.bus.clock.now_micros();
    let ms = (micros / 1000) as u32;
    if cpu.bus.observe.on {
        let next = cpu.bus.clock.icount_at_micros((micros / 1000 + 1) * 1000);
        let o = &mut cpu.bus.observe;
        let follows = match &o.phase {
            Phase::Count { head, .. } => {
                !o.plain_clock.iter().any(|p| p.head == *head && p.left > 0)
            }
            _ => false,
        };
        if follows && o.nesting == 0 {
            o.service_clock = Some((ms, cpu.bus.clock.icount, next));
            pure_call(cpu, None);
        } else {
            pure_call(cpu, Some(next));
        }
        reads_arguments(cpu);
    }
    ms
}

/// Win16Return loaded the 16-bit context that called 32-bit code, with
/// DX:AX the value EAX holds if `as_is`, and else a value made from it
/// (a handle, a BOOL).
pub fn resumed16(cpu: &mut Cpu, as_is: bool) {
    if cpu.bus.observe.on {
        cpu.bus.observe.resumed16 = Some(as_is);
    }
}

fn head_of(cpu: &Cpu) -> Head {
    Head { cs: cpu.cs(), eip: cpu.eip(), pe: cpu.pe() }
}

/// A service set `Observe::polled`, at the service trap at `cs:eip`: the
/// BIOS keyboard's status call found no key, or a Sleep(0) of the
/// built-in Windows found no other thread to run. Either way the program
/// waits in a loop for something else to change, and the place is armed
/// once that happened enough times in a row. A Sleep(0) loop runs only
/// while the other threads wait, often for a few milliseconds between two
/// events, which an event finding the CPU in it would arm too late.
pub fn service_polled(cpu: &mut Cpu, cs: u16, eip: u32) {
    let head = Head { cs, eip, pe: cpu.pe() };
    poll_at(cpu, head);
}

/// The built-in Windows's clock was read, at the service trap at `cs:eip`.
/// What it returns moves on once a millisecond, so a proof's passes hold
/// up to the next one (`finish`), and the loop is proven again from there,
/// as a game's frame limiter that reads it in every pass goes on. Reads
/// further apart than a head stays armed are no such loop, and arm nothing:
/// a program that reads the clock now and then doesn't go through the
/// interpreter for it.
pub fn clock_read(cpu: &mut Cpu, cs: u16, eip: u32) {
    let head = Head { cs, eip, pe: cpu.pe() };
    let run = cpu.bus.clock.run_count();
    let o = &mut cpu.bus.observe;
    if o.on && !std::mem::take(&mut o.clock_modelled) {
        let ms = cpu.bus.clock.now_micros() / 1000;
        o.clock_read.get_or_insert(ms);
    }
    if run.saturating_sub(std::mem::replace(&mut o.clock_read_at, run)) > ARM_LIMIT {
        o.poll = (None, 0);
    }
    poll_at(cpu, head);
}

/// A service was polled at `head`: armed once that happened enough times
/// in a row.
fn poll_at(cpu: &mut Cpu, head: Head) {
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
/// the loop sees (it is tried again after it). The head of passes skipped up
/// to it is armed again, as a time proof when it latched the PIT's count.
/// Else the CPU's instruction is armed when passes were skipped from it
/// before (a game's frame limiter, which it comes back to every frame), or
/// when the last event found the CPU there with the same flags and all its
/// registers but one the same. That one may be what a loop reads of a time
/// the event between moved on: `GetTickCount`'s count on Windows 98 (#160).
#[cold]
pub fn at_event(cpu: &mut Cpu) {
    if !cpu.bus.observe.enabled {
        return;
    }
    if cpu.bus.observe.on {
        cpu.bus.observe.cut();
    }
    let regs = cpu.snapshot();
    let here = (head_of(cpu), regs.gpr, regs.flags.bits());
    let now = cpu.bus.clock.icount;
    let o = &mut cpu.bus.observe;
    let near = o.last_event.is_some_and(|(head, gpr, flags)| {
        let differ = gpr.iter().zip(&here.1).filter(|(a, b)| a != b).count();
        head == here.0 && flags == here.2 && differ <= 1
    });
    o.last_event = Some(here);
    if let Some(head) = o.proven.take() {
        if std::mem::take(&mut o.proven_time) {
            o.time_from(head, now);
            if let Phase::Time(t) = &mut o.phase {
                t.wait = true;
            }
        } else {
            // A chance at a head that backs off, let pass, keeps it for
            // the next event: the loop goes on, and the head's next
            // chance comes after the events its back-off lets pass,
            // rather than when an event happens to find the CPU there.
            let passes = o.enabled
                && !o.watching()
                && o.backoffs.iter().any(|b| b.head == head && b.left > 0);
            o.arm(head, now);
            if passes {
                o.proven = Some(head);
            }
        }
    } else if near || o.known.contains(&here.0) {
        o.arm(here.0, now);
    }
}

/// An interrupt was delivered, which changes what the proof compares.
#[cold]
pub fn interrupted(cpu: &mut Cpu) {
    if cpu.bus.observe.on {
        cpu.bus.observe.cut();
    }
}

/// What a visit of the head comes to.
enum Step {
    Start,
    Remember,
    /// With the general registers that hold counts.
    Copy(u8),
    Compare,
    Count,
}

/// Whether the keys `a` and `b` are the same but for the general
/// registers in `loose`.
fn same_but(a: &CpuKey, b: &CpuKey, loose: u8) -> bool {
    if loose == 0 {
        return a == b;
    }
    let mut b = b.clone();
    for (i, r) in b.regs.gpr.iter_mut().enumerate() {
        if loose & 1 << i != 0 {
            *r = a.regs.gpr[i];
        }
    }
    *a == b
}

/// The general registers that count the passes, at a visit with `key`
/// after `visits`: those that the last two passes each moved on by the
/// same step, with everything else the same, as a loop counts its passes
/// in a register (MFC's idle loop, #1308). Not ESP, which moves the
/// stack. None (0) where the registers came round instead, or differ
/// otherwise.
fn counting_registers(visits: &[Visit], key: &CpuKey) -> u8 {
    let [.., a, b] = visits else {
        return 0;
    };
    let (x, y, z) = (&a.key.regs.gpr, &b.key.regs.gpr, &key.regs.gpr);
    let mut loose = 0;
    for i in 0..8 {
        if z[i] != y[i] {
            let step = z[i].wrapping_sub(y[i]);
            if i == 4 || y[i].wrapping_sub(x[i]) != step {
                return 0;
            }
            loose |= 1 << i;
        }
    }
    if loose != 0 && same_but(&a.key, &b.key, loose) && same_but(&b.key, key, loose) {
        loose
    } else {
        0
    }
}

/// Before the instruction at CS:EIP runs while `watching`: start a proof at
/// the armed head, or look at a visit of it, which may skip passes; then
/// look at the instruction, during a proof.
pub fn before_instruction(cpu: &mut Cpu) {
    let now = cpu.bus.clock.icount;
    if cpu.bus.observe.timing() {
        time::settle(cpu);
    }
    visit(cpu, now);
    if cpu.bus.observe.timing() {
        time::analyze(cpu);
    } else if cpu.bus.observe.on {
        cpu.bus.observe.current = Some(head_of(cpu));
        if matches!(cpu.bus.observe.phase, Phase::Count { .. }) {
            count::record(cpu);
        }
        inspect(cpu, now);
    }
}

fn visit(cpu: &mut Cpu, now: u64) {
    let here = head_of(cpu);
    let o = &mut cpu.bus.observe;
    o.seen = now;
    if let Some(why) = o.failed.get() {
        return o.fail(why);
    }
    if let Some(head) = o.latch_head.take() {
        return o.time_from(head, now);
    }
    if matches!(o.phase, Phase::Time(_)) {
        return time::visit(cpu, here, now);
    }
    if matches!(o.phase, Phase::ArmHere) {
        if o.backs_off(here) {
            return o.stop();
        }
        o.stats.arms += 1;
        o.tally(Some(here), |h| h.arms += 1);
        o.phase = Phase::Armed { head: here, until: now + ARM_LIMIT };
    }
    let (head, until) = match &o.phase {
        Phase::Off | Phase::ArmHere | Phase::Time(_) => return,
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
    if o.pit_read {
        // A pass read the PIT's count without latching it.
        return o.fail("port read");
    }
    let mut key = cpu.idle_key();
    if cpu.bus.observe.tainted {
        key.regs.gpr[0] &= !0xFF;
        key.tainted = true;
    }
    let visit = Visit {
        key,
        icount: now,
        executed: cpu.executed,
        calls: cpu.bus.win32.calls,
    };
    let o = &mut cpu.bus.observe;
    let counts = match &o.phase {
        Phase::Learn { visits, .. } => counting_registers(visits, &visit.key),
        _ => 0,
    };
    let step = match &o.phase {
        Phase::Off | Phase::ArmHere | Phase::Time(_) => return,
        Phase::Armed { .. } => Step::Start,
        Phase::Learn { visits, .. } if visits.iter().any(|v| v.key == visit.key) => Step::Copy(0),
        Phase::Learn { .. } if counts != 0 => Step::Copy(counts),
        Phase::Learn { visits, .. } if visits.len() < VISITS => Step::Remember,
        Phase::Learn { .. } => return o.fail("visits"),
        Phase::Verify { base, loose, .. } if same_but(&base.key, &visit.key, *loose) => {
            Step::Compare
        }
        Phase::Count { base, loose, .. } if same_but(&base.key, &visit.key, *loose) => Step::Count,
        Phase::Verify { .. } | Phase::Count { .. } => return,
    };
    match step {
        Step::Start => {
            o.stats.starts += 1;
            o.tally(Some(head), |h| h.starts += 1);
            o.on = true;
            o.started = now;
            o.failed.set(None);
            o.chunks.clear();
            o.forget_ports();
            let until = now + PROOF_LIMIT;
            let Some(shape) = o.shapes.iter().find(|s| s.head == head) else {
                o.phase = Phase::Learn {
                    head,
                    visits: vec![visit],
                    until,
                };
                return;
            };
            // The loop's shape is known: its pass is recorded from here.
            let pass = count::Pass::new(shape.seeds.clone());
            let blocks = shape.blocks.clone();
            o.shaped = true;
            o.restart_ports();
            let copies = copy_blocks(cpu.bus.ram(), blocks);
            cpu.bus.observe.phase = Phase::Count {
                head,
                base: Box::new(visit),
                copies,
                loose: pass.registers(),
                pass: Box::new(pass),
                until,
                rounds: 0,
            };
        }
        Step::Remember => {
            if let Phase::Learn { visits, .. } = &mut o.phase {
                visits.push(visit);
            }
        }
        Step::Copy(loose) => {
            // The registers came round, but for counts in `loose`: copy the
            // blocks written so far, and see whether they come round as
            // well, with the ports read from here on.
            let chunks = o.take_chunks();
            o.restart_ports();
            let copies = copy_blocks(cpu.bus.ram(), chunks.iter().map(|&c| c as usize).collect());
            cpu.bus.observe.phase = Phase::Verify {
                head,
                base: Box::new(visit),
                copies,
                until,
                loose,
            };
        }
        Step::Compare => compare(cpu, visit),
        Step::Count => count::count(cpu, visit),
    }
}

/// At a visit with the registers of the base: if the blocks written since
/// are as they were there, the loop is proven and passes are skipped.
fn compare(cpu: &mut Cpu, visit: Visit) {
    let chunks = cpu.bus.observe.take_chunks();
    let Phase::Verify { base, copies, loose, .. } = &cpu.bus.observe.phase else { return };
    let (base_icount, base_executed, base_calls) = (base.icount, base.executed, base.calls);
    let loose = *loose;
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
    if !same || loose != 0 {
        // Not the base's state yet: the blocks written since it are
        // compared again at the next visits. Where only a few bytes differ
        // and the registers in `loose` (counts), the passes may differ in
        // those alone: the next one is recorded from here, to see.
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
        if counters.len() <= COUNTER_BYTES {
            let blocks: Vec<usize> = copies.iter().map(|(k, _)| *k).collect();
            let copies = copy_blocks(ram, blocks);
            let o = &mut cpu.bus.observe;
            o.restart_ports();
            let Phase::Verify { head, until, .. } = o.phase else { return };
            let seeds = counters
                .iter()
                .map(|&a| a as count::Cell)
                .chain(count::register_cells(loose))
                .collect();
            let pass = count::Pass::new(seeds);
            o.phase = Phase::Count {
                head,
                base: Box::new(visit),
                copies,
                loose: pass.registers(),
                pass: Box::new(pass),
                until,
                rounds: 0,
            };
            return;
        }
        cpu.bus.observe.chunks.extend(chunks);
        return;
    }
    finish(cpu, &visit, base_icount, base_executed, base_calls, None);
}

/// A loop is proven from the visit before `visit` at `base` instructions
/// (`executed` executed, `calls` the `Win32::calls` there): skip passes up
/// to the next event, the time a port it read changes or the millisecond
/// after the clock it read, and with `counted` as long as the passes'
/// counters let them run the same (see `count`).
fn finish(
    cpu: &mut Cpu,
    visit: &Visit,
    base: u64,
    executed: u64,
    calls: u64,
    counted: Option<Box<count::Model>>,
) {
    let period = visit.icount - base;
    let executed = visit.executed - executed;
    let calls = visit.calls.wrapping_sub(calls);
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
    if let Some(ms) = cpu.bus.observe.clock_read {
        limit = limit.min(cpu.bus.clock.icount_at_ms(ms + 1));
    }
    // AL may hold a port's value whose bits the loop doesn't look at, but
    // what comes after it may: it is made what the last pass skipped read.
    let held = cpu.bus.observe.tainted.then_some((cpu.bus.observe.al_port, cpu.bus.observe.al_read));
    // What the built-in Windows' services read holds until a time, as a
    // port's bits do: the next millisecond of GetTickCount, the next timer
    // of a PeekMessage. The loop is likely to be proven again from there.
    let o = &mut cpu.bus.observe;
    let turn_at = o.turn_at;
    let by_service = o.holds_until.filter(|&until| until < limit);
    if let Some(until) = by_service {
        limit = until;
    }
    // The passes the counters let run the same. Where there was room for
    // one and none does, as when a compare of a counter that a jump reads
    // comes out otherwise in the next pass, the loop is no closer to a
    // skip at its next proof than at this one: that is a failure, which
    // backs off, rather than a proof started again every few passes.
    let mut counted_run = None;
    if let Some(pass) = &counted
        && period > 0
        && limit > visit.icount
    {
        let room = limit.min(cpu.bus.clock.deadline) - visit.icount;
        let most = (room / period).min(COUNTED_PASSES);
        let (same, values, flags) = count::counted_passes(cpu, pass, most, visit.icount, period);
        if most > 0 && same == 0 {
            return cpu.bus.observe.fail("count passes");
        }
        counted_run = Some((same, values, flags));
    }
    let o = &mut cpu.bus.observe;
    let head = o.head();
    let found = o.found.take();
    let shaped = o.shaped;
    o.stop();
    if period > 0 && limit > visit.icount {
        let mut passes = u64::MAX;
        let counting = counted_run.is_some();
        if let Some((same, values, (mask, bits))) = counted_run {
            for (cell, value) in values {
                count::set_cell(cpu, cell, value);
            }
            cpu.set_flag_bits(mask, bits);
            passes = same;
        }
        let skipped = skip(cpu, period, executed, limit, passes);
        let ran = visit.icount.saturating_sub(cpu.bus.observe.started);
        let saved = skipped
            + (skipped / period)
                .saturating_mul(calls)
                .saturating_mul(SERVICE_COST);
        let o = &mut cpu.bus.observe;
        // The head's recent proofs with this one (`Ledger`): what they
        // saved, against what the next proof at the head will run, one
        // pass where it starts from the shape this one found, else as many
        // as this one.
        let next = if found.is_some() { period } else { ran };
        let ledger = match head {
            Some(head) if counting => o.charge(head, saved, next),
            _ => (saved, next),
        };
        if counting && ledger.0 < ledger.1.saturating_mul(PAYOFF) {
            // The skips are real, but proving them again would cost the
            // host more than running the loop: the head backs off further
            // each time in a row, the further the more it falls short.
            o.stats.costly += 1;
            if let Some(head) = head {
                let short = ledger.1.saturating_mul(PAYOFF) / ledger.0.max(1);
                let most = COSTLY_BACKOFF + 4 * short.max(1).ilog2();
                o.back_off(head, most.min(16));
            }
        } else {
            o.succeed(head);
        }
        if skipped > 0 {
            cpu.bus.observe.stats.proving += ran;
            cpu.bus.observe.stats.shaped += shaped as u64;
            if let Some(shape) = found {
                cpu.bus.observe.keep_shape(shape);
            }
            cpu.bus.observe.proved(head);
            let plain = &mut cpu.bus.observe.plain_clock;
            if let Some(at) = plain.iter().position(|p| Some(p.head) == head) {
                if plain[at].left == 0 {
                    plain.remove(at);
                } else {
                    plain[at].left -= 1;
                }
            }
            // The calls the passes skipped would have made, and the turn
            // the last of them would have started.
            let win32 = &mut cpu.bus.win32;
            win32.calls = win32.calls.wrapping_add((skipped / period).wrapping_mul(calls));
            if let Some(at) = turn_at {
                win32.slice_start = cpu.bus.clock.micros_at(at + skipped);
            }
            if let (Some(_), Some(head)) = (by_service, head) {
                let now = cpu.bus.clock.icount;
                cpu.bus.observe.arm(head, now);
            }
        }
        if let Some((port, read)) = held
            && skipped > 0
        {
            let value = cpu.bus.idle_port_value(port, read + skipped);
            cpu.set_reg8(iced_x86::Register::AL, value);
        }
    }
}

thread_local! {
    static FACTORY: std::cell::RefCell<iced_x86::InstructionInfoFactory> =
        std::cell::RefCell::new(iced_x86::InstructionInfoFactory::new());
}

/// The 15 bytes an instruction at linear address `lin` may have, and how
/// many of them are in pages that are there (all of them with paging
/// off).
fn code_at(cpu: &Cpu, lin: u32) -> ([u8; 15], usize) {
    let mut bytes = [0u8; 15];
    let mut done = 0;
    for page in pages(cpu, lin, 15, false) {
        let Some(Access { start, len }) = page else {
            break;
        };
        let to = &mut bytes[done..done + len];
        if cpu.bus.is_plain_ram(start, len) {
            to.copy_from_slice(&cpu.bus.ram()[start..start + len]);
        } else {
            for (i, b) in to.iter_mut().enumerate() {
                *b = cpu.bus.peek_8(start + i);
            }
        }
        done += len;
    }
    (bytes, done)
}

/// The physical address an access of the CPU at linear address `lin`
/// goes to (a write's with `write`), found without changing anything:
/// through the TLB where it has the page, as the access goes, else
/// through the page tables, as the access would walk them. With paging
/// off, `lin` through the A20 gate.
fn physical_at(cpu: &Cpu, lin: u32, write: bool) -> Option<u32> {
    cpu.translated_for(lin, write, cpu.cpl == 3)
        .or_else(|| cpu.peek_translate(lin))
}

/// The RAM the `len` bytes at linear address `lin` are in, added to `out`:
/// a range for each run of them that follows on in RAM (`physical_at`).
/// False where a page isn't there, whose access faults.
fn physical_into(cpu: &Cpu, lin: u32, len: u64, write: bool, out: &mut Vec<Access>) -> bool {
    let from = out.len();
    for piece in pages(cpu, lin, len, write) {
        let Some(piece) = piece else {
            return false;
        };
        let joins = out.len() > from;
        match out.last_mut() {
            Some(last) if joins && last.start + last.len == piece.start => last.len += piece.len,
            _ => out.push(piece),
        }
    }
    true
}

/// As `physical_into`, without the pages that aren't there: the RAM a
/// service or an interrupt may reach near the stack's top or in the IDT,
/// which it can't reach in a missing page without a fault.
fn present(cpu: &Cpu, lin: u32, len: u64, write: bool) -> Vec<Access> {
    merged(pages(cpu, lin, len, write).flatten())
}

/// `pieces`, with each that starts where the one before it ends made one
/// with it.
fn merged(pieces: impl IntoIterator<Item = Access>) -> Vec<Access> {
    let mut out: Vec<Access> = Vec::new();
    for piece in pieces {
        match out.last_mut() {
            Some(last) if last.start + last.len == piece.start => last.len += piece.len,
            _ => out.push(piece),
        }
    }
    out
}

/// The linear runs of the `len` bytes (64K at most) from offset `off` of
/// the segment at `base`, whose 16-bit offsets wrap at 64K: the second is
/// empty unless they wrap.
fn segment_runs(base: u32, off: u32, len: u32) -> [(u32, u64); 2] {
    let first = (0x1_0000 - off).min(len);
    [
        (base.wrapping_add(off), first as u64),
        (base, (len - first) as u64),
    ]
}

/// The page directory and page table entries that find the pages of the
/// `len` bytes at linear address `lin`, with paging on (none with it off),
/// added to `found`. The CPU reads them for the passes after the one
/// recorded too, where its TLB doesn't have the page: a counter in one
/// would move the page.
fn entries_into(cpu: &Cpu, lin: u32, len: u64, found: &mut Vec<Access>) {
    if cpu.cr0 & crate::cpu::CR0_PG == 0 {
        return;
    }
    let mut done = 0;
    while done < len {
        let at = lin.wrapping_add(done as u32);
        done += (0x1000 - (at & 0xFFF) as u64).min(len - done);
        let (pde, _, pte) = cpu.page_walk(at);
        found.push(Access {
            start: pde as usize,
            len: 4,
        });
        if let Some((pte, _)) = pte {
            found.push(Access {
                start: pte as usize,
                len: 4,
            });
        }
    }
}

/// The RAM of each page the `len` bytes at linear address `lin` are in,
/// or None for a page that isn't there. With paging off the A20 gate
/// decides, which may wrap them at 1 MB, a page's end.
fn pages(cpu: &Cpu, lin: u32, len: u64, write: bool) -> impl Iterator<Item = Option<Access>> {
    let mut done = 0;
    std::iter::from_fn(move || {
        if done >= len {
            return None;
        }
        let at = lin.wrapping_add(done as u32);
        let run = (0x1000 - (at & 0xFFF) as u64).min(len - done);
        done += run;
        let start = physical_at(cpu, at, write);
        Some(start.map(|start| Access {
            start: start as usize,
            len: run as usize,
        }))
    })
}

/// The arithmetic flags of iced's `RflagsBits`.
fn rflags(bits: u32) -> u32 {
    use crate::cpu::alu::{AF, CF, OF, PF, SF, ZF};
    use iced_x86::RflagsBits as R;
    [(R::OF, OF), (R::SF, SF), (R::ZF, ZF), (R::AF, AF), (R::CF, CF), (R::PF, PF)]
        .iter()
        .fold(0, |f, &(r, x)| if bits & r != 0 { f | x } else { f })
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
    let (bytes, _) = code_at(cpu, lin);
    let instr = Decoder::with_ip(bitness, &bytes, cpu.eip() as u64, DecoderOptions::NONE).decode();
    let o = &mut cpu.bus.observe;
    if instr.mnemonic() == Mnemonic::In && instr.op0_register() == Register::AL {
        let port = if instr.op1_kind() == OpKind::Immediate8 { instr.immediate8() as u16 } else { cpu.dx() };
        if port == 0x40 {
            // The PIT's count, which only a time proof follows.
            return;
        }
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
/// of them executed), up to `limit`: the next timer event, when a port the
/// loop reads changes, or the clock's next millisecond; `most` passes at
/// most.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::savestate::machine;

    #[test]
    fn a_load_watches_nothing_the_machine_before_it_watched() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let state = machine::save(&cpu);
        // The machine runs on far past the state, and a busy-wait loop is
        // armed there, until its clock gets ARM_LIMIT further.
        cpu.bus.clock.icount += 1_000_000_000;
        cpu.bus.observe.enabled = true;
        let head = Head {
            cs: 0x1234,
            eip: 0x10,
            pe: false,
        };
        cpu.bus.observe.arm(head, cpu.bus.clock.icount);
        assert!(cpu.bus.observe.watching());
        cpu.bus.observe.stats.arms = 7;
        machine::load(&mut cpu, &state).unwrap();
        // The state's clock is 10^9 instructions behind that limit: the
        // loop would run one instruction at a time on the interpreter until
        // it got there.
        assert!(!cpu.bus.observe.watching());
        assert!(cpu.bus.observe.enabled);
        assert_eq!(cpu.bus.observe.stats.arms, 7);
    }
}
