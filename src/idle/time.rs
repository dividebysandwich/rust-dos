//! Loops that read the time from the PIT, skipped exactly.
//!
//! A Windows 98 game that paces its frames with QueryPerformanceCounter
//! reads the PIT in every pass: VTD latches channel 0's count and reads
//! it. No pass leaves the machine as it found it, so `idle`'s proof never
//! holds. What does is that each pass leaves the machine the same function
//! of the count it read. This proof follows the values that come from the
//! count through a pass, as functions of it (`Form`), with the branches and
//! carries they decide, and then works out the state that the passes up to
//! the next timer event leave, from the counts they would read, without
//! running them.
//!
//! **The proof.** The head is the instruction that latches the count (an
//! OUT to port 43h). Two passes from it run on the interpreter, each
//! instruction looked at before it runs (`look`):
//!
//! - The first finds the bytes of the registers and RAM that the pass
//!   leaves as functions of the count it read (`Shadow`): a guess at what
//!   of the machine at the head follows the time.
//! - The second starts from that guess, with those bytes as the same
//!   functions of the count read before. It must come back to the head
//!   with them as those functions of its own count, and with everything
//!   else as it was. Each branch and carry decided by the two counts is
//!   noted (`Check`).
//!
//! The functions are a count read times a constant, plus a constant, and
//! sums and differences of them: what the pass does with them is moves,
//! additions, subtractions and comparisons. Anything else done with a value
//! that follows the time (a multiply, a mask, an address computed from it)
//! ends the proof, as does a read of any port but the latched count.
//! Flags that follow the time may be saved with PUSHF and taken back with
//! POPF, as VTD saves its caller's around the read (`FlagByte`).
//!
//! **The skip.** The passes after the second read the counts channel 0 will
//! have when they latch it, which are known up to the next timer event, and
//! run down until the count reloads: at that event, or halfway to it in
//! mode 3, where the passes after are proven again. When every check comes
//! out as in the second pass for each two successive counts among them,
//! those passes run as the second did. The checks are linear in the two
//! counts, so they come out the same over the parallelogram that holds all
//! the pairs when they do at its corners (`checks_hold`). The state after
//! the last pass is the second pass's functions of its count (`finish`).

use std::collections::BTreeMap;

use iced_x86::{
    Decoder, DecoderOptions, Instruction, Mnemonic, OpAccess, OpKind, Register, UsedMemory,
};

use super::{CHUNK, FACTORY, Head, Phase, Visit, copy_blocks, rflags};
use crate::cpu::alu::{AF, ARITH, CF, PF};
use crate::cpu::{Cpu, CpuFlags, Seg};

/// What the values are taken modulo: every register and operand is 32
/// bits at most.
const MOD: i64 = 1 << 32;
/// Passes skipped at most at once.
const PASSES: u64 = 1 << 20;

/// The general registers, by number.
const GPRS: [Register; 8] = [
    Register::EAX,
    Register::ECX,
    Register::EDX,
    Register::EBX,
    Register::ESP,
    Register::EBP,
    Register::ESI,
    Register::EDI,
];

/// A value that follows the time: `p` times the count the pass before
/// latched, plus `n` times the count this pass latched, plus `c`. Of it,
/// an operand of `s` bytes holds the value modulo 2^(8s).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Form {
    p: i64,
    n: i64,
    c: i64,
}

impl Form {
    /// The count this pass latched.
    const COUNT: Form = Form { p: 0, n: 1, c: 0 };

    fn constant(c: u32) -> Form {
        Form {
            p: 0,
            n: 0,
            c: c as i64,
        }
    }

    fn at(self, p: i64, n: i64) -> i64 {
        self.p * p + self.n * n + self.c
    }

    fn plus(self, o: Form) -> Form {
        Form {
            p: self.p + o.p,
            n: self.n + o.n,
            c: (self.c + o.c).rem_euclid(MOD),
        }
    }

    fn minus(self, o: Form) -> Form {
        Form {
            p: self.p - o.p,
            n: self.n - o.n,
            c: (self.c - o.c).rem_euclid(MOD),
        }
    }

    /// The same function of the count the pass before latched, for one of
    /// this pass's only.
    fn earlier(self) -> Form {
        Form {
            p: self.n,
            n: 0,
            c: self.c,
        }
    }

    /// The same function of this pass's count, for one of the pass
    /// before's only.
    fn later(self) -> Form {
        Form {
            p: 0,
            n: self.p,
            c: self.c,
        }
    }

    fn byte(self, p: i64, n: i64, byte: u8) -> u8 {
        (self.at(p, n) >> (8 * byte)) as u8
    }
}

/// Byte `byte` of a value that follows the time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Part {
    form: Form,
    byte: u8,
}

impl Part {
    fn map(self, f: fn(Form) -> Form) -> Part {
        Part {
            form: f(self.form),
            byte: self.byte,
        }
    }
}

/// The flags an addition (or a subtraction, `sub`) of `size` bytes set
/// from values that follow the time: `a` plus or minus `b` and `carry`.
/// `bits` are those of them that EFLAGS still holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Arith {
    sub: bool,
    size: u8,
    a: Form,
    b: Form,
    carry: bool,
    bits: u32,
}

impl Arith {
    fn map(self, f: fn(Form) -> Form) -> Arith {
        Arith {
            a: f(self.a),
            b: f(self.b),
            ..self
        }
    }

    /// The operands as the instruction sees them, unsigned.
    fn operands(self, p: i64, n: i64) -> (i64, i64) {
        let full = 1i64 << (8 * self.size);
        (
            self.a.at(p, n).rem_euclid(full),
            self.b.at(p, n).rem_euclid(full),
        )
    }

    /// The flags it sets with counts `p` and `n`.
    fn flags(self, p: i64, n: i64) -> u32 {
        let (a, b) = self.operands(p, n);
        let run = if self.sub {
            crate::cpu::alu::sub
        } else {
            crate::cpu::alu::add
        };
        run(self.size, a as u32, b as u32, self.carry).1
    }

    /// Where the operands and the results fall at `p` and `n`: in which
    /// half of their range (so that, where they all fall the same, each
    /// operand and result is the same linear function of the counts as
    /// unsigned and as signed values, with no wrap between), and whether
    /// the result is 0. Where they all fall the same, CF, ZF, SF and OF do.
    fn zones(self, p: i64, n: i64) -> [i64; 5] {
        let full = 1i64 << (8 * self.size);
        let half = full / 2;
        let signed = |v: i64| if v >= half { v - full } else { v };
        let (a, b) = self.operands(p, n);
        let c = self.carry as i64;
        let (unsigned, signed) = if self.sub {
            (a - b - c, signed(a) - signed(b) - c)
        } else {
            (a + b + c, signed(a) + signed(b) + c)
        };
        [
            self.a.at(p, n).div_euclid(half),
            self.b.at(p, n).div_euclid(half),
            unsigned.div_euclid(half),
            (unsigned.rem_euclid(full) == 0) as i64,
            (signed + half).div_euclid(full),
        ]
    }
}

/// Byte `byte` of the flags `arith` set, as PUSHF stored them: its bits
/// of `arith.bits` follow the time, and the others are `rest`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FlagByte {
    arith: Arith,
    byte: u8,
    rest: u8,
}

impl FlagByte {
    fn mask(arith: Arith, byte: u8) -> u8 {
        (arith.bits >> (8 * byte)) as u8
    }

    fn at(self, p: i64, n: i64) -> u8 {
        let mask = FlagByte::mask(self.arith, self.byte);
        (self.rest & !mask) | ((self.arith.flags(p, n) >> (8 * self.byte)) as u8 & mask)
    }

    fn map(self, f: fn(Form) -> Form) -> FlagByte {
        FlagByte {
            arith: self.arith.map(f),
            ..self
        }
    }
}

/// What of the machine follows the time.
#[derive(Clone, Default, PartialEq, Debug)]
struct Shadow {
    /// The general registers' bytes, low byte first.
    regs: [[Option<Part>; 4]; 8],
    /// Bytes of RAM, by physical address.
    mem: BTreeMap<usize, Part>,
    /// Bytes of RAM that hold flags that follow the time, which only POPF
    /// may read.
    flag_mem: BTreeMap<usize, FlagByte>,
    flags: Option<Arith>,
    /// Flags that follow the time some other way: reading them ends the
    /// proof.
    opaque: u32,
}

impl Shadow {
    fn map(&self, f: fn(Form) -> Form) -> Shadow {
        Shadow {
            regs: self.regs.map(|r| r.map(|b| b.map(|b| b.map(f)))),
            mem: self.mem.iter().map(|(&a, &b)| (a, b.map(f))).collect(),
            flag_mem: self.flag_mem.iter().map(|(&a, &b)| (a, b.map(f))).collect(),
            flags: self.flags.map(|a| a.map(f)),
            opaque: self.opaque,
        }
    }

    /// Whether every form is of the count this pass latched only.
    fn latest(&self) -> bool {
        let forms = self
            .regs
            .iter()
            .flatten()
            .flatten()
            .chain(self.mem.values())
            .map(|b| b.form);
        let ariths = self
            .flags
            .iter()
            .chain(self.flag_mem.values().map(|b| &b.arith));
        let flags = ariths.flat_map(|a| [a.a, a.b]);
        forms.chain(flags).all(|f| f.p == 0)
    }

    /// Whether the RAM byte at `a` follows the time.
    fn follows(&self, a: usize) -> bool {
        self.mem.contains_key(&a) || self.flag_mem.contains_key(&a)
    }
}

/// What a skipped pass must decide as the second pass did.
#[derive(Clone, Copy, Debug)]
enum Check {
    /// The flags `read` of `arith`, which were `seen`.
    Flags { arith: Arith, read: u32, seen: u32 },
    /// That `form` stays in `k * 2^bits..(k + 1) * 2^bits`, where its low
    /// `bits` were read with constant bits above them.
    Window { form: Form, bits: u32, k: i64 },
}

/// What an instruction leaves following the time, once it has run.
#[derive(Default)]
struct Pending {
    /// Register bytes (register, byte) it writes.
    regs: Vec<(usize, usize, Option<Part>)>,
    /// Bytes of RAM it writes as values that follow the time; of the
    /// others it writes, the bus tells.
    mem: Vec<(usize, Part)>,
    /// Bytes of RAM PUSHF writes with flags that follow the time, their
    /// other bits found once it has run.
    flag_mem: Vec<(usize, FlagByte)>,
    /// The flags it writes, and those of them that follow the time.
    written: u32,
    flags: Option<Arith>,
    opaque: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    /// Waiting for the CPU to come to the head.
    Armed,
    /// The first pass runs, finding what follows the time.
    First,
    /// The second pass runs, from the first's guess.
    Second,
}

/// A time proof (`Phase::Time`).
pub struct TimeProof {
    pub head: Head,
    pub until: u64,
    stage: Stage,
    /// The visit of the head the running pass started at, with the I/O
    /// stall count then.
    start: Option<(Box<Visit>, u64)>,
    shadow: Shadow,
    /// The second pass's start: the first's guess, of the count before.
    entry: Shadow,
    pending: Option<Pending>,
    exceptions: u64,
    /// When the running pass latched the count, and the count; the count
    /// the pass before it latched; and the bytes read of it.
    latch: Option<(u64, u16)>,
    before: u16,
    reads: u32,
    copies: Vec<(usize, [u8; CHUNK])>,
    checks: Vec<Check>,
}

impl TimeProof {
    pub fn new(head: Head, until: u64) -> TimeProof {
        TimeProof {
            head,
            until,
            stage: Stage::Armed,
            start: None,
            shadow: Shadow::default(),
            entry: Shadow::default(),
            pending: None,
            exceptions: 0,
            latch: None,
            before: 0,
            reads: 0,
            copies: Vec::new(),
            checks: Vec::new(),
        }
    }

    /// The counts of the pass before and of this one, as far as known.
    fn counts(&self) -> (i64, i64) {
        (self.before as i64, self.latch.map_or(0, |l| l.1 as i64))
    }
}

/// Run `f` on the time proof, which it can't reach through the CPU while
/// it runs; a failure ends the proof.
fn with_proof<T>(
    cpu: &mut Cpu,
    f: impl FnOnce(&mut Cpu, &mut TimeProof) -> Result<T, &'static str>,
) -> Option<T> {
    let Phase::Time(mut t) = std::mem::take(&mut cpu.bus.observe.phase) else {
        return None;
    };
    let result = f(cpu, &mut t);
    cpu.bus.observe.phase = Phase::Time(t);
    match result {
        Ok(v) => Some(v),
        Err(why) => {
            cpu.bus.observe.on = true;
            cpu.bus.observe.fail(why);
            None
        }
    }
}

/// After an instruction ran: what it wrote follows the time as `look`
/// found, the rest of what it wrote doesn't, and the count it latched is
/// noted.
pub(super) fn settle(cpu: &mut Cpu) {
    with_proof(cpu, |cpu, t| {
        let writes = std::mem::take(&mut cpu.bus.observe.writes);
        let latched = cpu.bus.observe.latch_at.take();
        let Some(pending) = t.pending.take() else {
            return Ok(());
        };
        if cpu.exceptions != t.exceptions {
            return Err("time fault");
        }
        if let Some(at) = latched {
            if t.latch.is_some() {
                return Err("time latches");
            }
            t.latch = Some((at, cpu.bus.pit0_latched));
        }
        let s = &mut t.shadow;
        for (start, end) in writes {
            s.mem.retain(|&a, _| !(start..end).contains(&a));
            s.flag_mem.retain(|&a, _| !(start..end).contains(&a));
        }
        s.mem.extend(pending.mem);
        for (a, b) in pending.flag_mem {
            let rest = cpu.bus.peek_8(a) & !FlagByte::mask(b.arith, b.byte);
            s.flag_mem.insert(a, FlagByte { rest, ..b });
        }
        for (r, b, part) in pending.regs {
            s.regs[r][b] = part;
        }
        match pending.flags {
            Some(arith) => s.flags = Some(arith),
            None => {
                if let Some(arith) = &mut s.flags {
                    arith.bits &= !pending.written;
                    if arith.bits == 0 {
                        s.flags = None;
                    }
                }
            }
        }
        s.opaque = (s.opaque & !pending.written) | pending.opaque;
        Ok(())
    });
}

/// The CPU is at `here` during a time proof: start a pass at the head, or
/// end one.
pub(super) fn visit(cpu: &mut Cpu, here: Head, now: u64) {
    let o = &mut cpu.bus.observe;
    let Phase::Time(t) = &o.phase else { return };
    let (head, until) = (t.head, t.until);
    if now > until {
        return if o.on { o.fail("limit") } else { o.stop() };
    }
    if here != head {
        return;
    }
    let visit = Visit {
        key: cpu.idle_key(),
        icount: now,
        executed: cpu.executed,
    };
    let stalled = cpu.bus.clock.stalled;
    let skip = with_proof(cpu, |cpu, t| match t.stage {
        Stage::Armed => {
            let o = &mut cpu.bus.observe;
            o.stats.starts += 1;
            o.tally(Some(t.head), |h| h.starts += 1);
            o.on = true;
            o.failed.set(None);
            o.chunks.clear();
            o.writes.clear();
            o.latch_at = None;
            t.until = now + super::PROOF_LIMIT;
            t.stage = Stage::First;
            t.shadow = Shadow::default();
            t.start = Some((Box::new(visit.clone()), stalled));
            t.latch = None;
            t.reads = 0;
            Ok(false)
        }
        Stage::First => {
            end_pass(cpu, t, &visit)?;
            // The guess: what follows this pass's count is the same
            // function of the count before at the start of the next.
            if !t.shadow.latest() {
                return Err("time guess");
            }
            t.entry = t.shadow.map(Form::earlier);
            t.before = t.latch.map_or(0, |l| l.1);
            let chunks = cpu.bus.observe.take_chunks();
            t.copies = copy_blocks(cpu.bus.ram(), chunks.iter().map(|&c| c as usize).collect());
            t.shadow = t.entry.clone();
            t.checks.clear();
            t.stage = Stage::Second;
            t.start = Some((Box::new(visit.clone()), stalled));
            t.latch = None;
            t.reads = 0;
            Ok(false)
        }
        Stage::Second => {
            end_pass(cpu, t, &visit)?;
            if t.shadow != t.entry.map(Form::later) {
                return Err("time guess");
            }
            // The rest of RAM the pass wrote is as it was.
            let chunks = cpu.bus.observe.take_chunks();
            let ram = cpu.bus.ram();
            for &c in &chunks {
                let c = c as usize;
                let Ok(i) = t.copies.binary_search_by_key(&c, |(k, _)| *k) else {
                    return Err("time blocks");
                };
                let start = c * CHUNK;
                let differs = (0..CHUNK)
                    .any(|b| ram[start + b] != t.copies[i].1[b] && !t.entry.follows(start + b));
                if differs {
                    return Err("time blocks");
                }
            }
            Ok(true)
        }
    });
    if skip == Some(true) {
        finish(cpu, visit, stalled);
    }
}

/// A pass came back to the head with `visit`: it latched the count once
/// and read both its bytes, what follows the time is as the bytes and
/// registers hold it, and the registers that don't are as at the start.
fn end_pass(cpu: &mut Cpu, t: &mut TimeProof, visit: &Visit) -> Result<(), &'static str> {
    if t.latch.is_none() || t.reads != 2 || cpu.bus.pit0_latched_active {
        return Err("time reads");
    }
    let s = &t.shadow;
    if s.opaque != 0 {
        return Err("time flags");
    }
    let (p, n) = t.counts();
    for (r, bytes) in s.regs.iter().enumerate() {
        for (b, part) in bytes.iter().enumerate() {
            if part
                .is_some_and(|x| x.form.byte(p, n, x.byte) != (cpu.reg(GPRS[r]) >> (8 * b)) as u8)
            {
                return Err("time model");
            }
        }
    }
    if s.mem
        .iter()
        .any(|(&a, x)| x.form.byte(p, n, x.byte) != cpu.bus.peek_8(a))
    {
        return Err("time model");
    }
    if s.flag_mem
        .iter()
        .any(|(&a, x)| x.at(p, n) != cpu.bus.peek_8(a))
    {
        return Err("time model");
    }
    let Some((start, _)) = &t.start else {
        return Err("time start");
    };
    let mask = |key: &super::CpuKey| {
        let mut key = key.clone();
        for (r, bytes) in s.regs.iter().enumerate() {
            for (b, part) in bytes.iter().enumerate() {
                if part.is_some() {
                    key.regs.gpr[r] &= !(0xFF << (8 * b));
                }
            }
        }
        if let Some(arith) = s.flags {
            key.regs.flags = CpuFlags::from_bits_retain(key.regs.flags.bits() & !arith.bits);
        }
        key
    };
    // At the end of the first pass the registers that follow the time
    // may have been constant at its start, which the second pass checks.
    if mask(&start.key) != mask(&visit.key) {
        return Err("time regs");
    }
    Ok(())
}

/// The second pass came back to the head: skip the passes after it whose
/// counts the checks hold for, up to the next timer event, and leave the
/// machine as the last of them would.
fn finish(cpu: &mut Cpu, visit: Visit, stalled: u64) {
    let o = &mut cpu.bus.observe;
    let Phase::Time(t) = std::mem::take(&mut o.phase) else {
        return;
    };
    let head = Some(t.head);
    o.stop();
    let (Some((start, start_stalled)), Some((latched, count))) = (&t.start, t.latch) else {
        return;
    };
    let period = visit.icount - start.icount;
    let executed = visit.executed - start.executed;
    let stall = stalled - start_stalled;
    let limit = cpu.bus.clock.deadline;
    if period == 0 || !cpu.bus.pit0.is_counting() {
        return;
    }
    // The counts the passes after it latch, while they run down.
    let most = (limit.saturating_sub(visit.icount) / period).min(PASSES);
    let mut counts = vec![count as i64];
    for j in 1..=most {
        let next = cpu
            .bus
            .pit0
            .count(cpu.bus.clock.ticks_at(latched + j * period)) as i64;
        if next > counts[counts.len() - 1] {
            break;
        }
        counts.push(next);
    }
    // The most passes the checks hold for: they hold for fewer if they do
    // for more.
    let (mut good, mut bad) = (0, counts.len());
    while bad - good > 1 {
        let mid = (good + bad) / 2;
        if checks_hold(&t.checks, &counts[..=mid]) {
            good = mid;
        } else {
            bad = mid;
        }
    }
    let o = &mut cpu.bus.observe;
    o.succeed(head);
    if good == 0 {
        return;
    }
    let skipped = super::skip(cpu, period, executed, limit, good as u64);
    if skipped == 0 {
        return;
    }
    let passes = skipped / period;
    cpu.bus.clock.stalled += passes * stall;
    cpu.bus.observe.proved(head);
    // The machine after the last pass: what follows the time, at its count.
    let (p, n) = (counts[passes as usize - 1], counts[passes as usize]);
    let s = &t.shadow;
    for (r, bytes) in s.regs.iter().enumerate() {
        let mut value = cpu.reg(GPRS[r]);
        for (b, part) in bytes.iter().enumerate() {
            if let Some(x) = part {
                value =
                    (value & !(0xFF << (8 * b))) | (x.form.byte(p, n, x.byte) as u32) << (8 * b);
            }
        }
        if bytes.iter().any(Option::is_some) {
            cpu.set_reg(GPRS[r], value);
        }
    }
    for (&a, x) in &s.mem {
        cpu.bus.write_8(a, x.form.byte(p, n, x.byte));
    }
    for (&a, x) in &s.flag_mem {
        cpu.bus.write_8(a, x.at(p, n));
    }
    if let Some(arith) = s.flags {
        let flags = arith.flags(p, n);
        for bit in [
            CF,
            PF,
            AF,
            crate::cpu::alu::ZF,
            crate::cpu::alu::SF,
            crate::cpu::alu::OF,
        ] {
            if arith.bits & bit != 0 {
                cpu.set_cpu_flag(CpuFlags::from_bits_retain(bit), flags & bit != 0);
            }
        }
    }
    cpu.bus.pit0_latched = n as u16;
    // The count reloads before the next timer event (halfway to it in
    // mode 3), and the passes up to there were skipped: those after it are
    // proven again.
    let reloads = (counts.len() as u64) <= most;
    if reloads && passes as usize == counts.len() - 1 {
        cpu.bus.observe.time_from(t.head, cpu.bus.clock.icount);
    }
}

/// Whether every check comes out as in the second pass for each two
/// successive counts of `counts` (the second pass's first).
fn checks_hold(checks: &[Check], counts: &[i64]) -> bool {
    if counts.len() < 2 {
        return true;
    }
    let pairs = counts.windows(2).map(|w| (w[1], w[0] - w[1]));
    let (mut n0, mut n1, mut d0, mut d1) = (i64::MAX, i64::MIN, i64::MAX, i64::MIN);
    for (n, d) in pairs {
        (n0, n1, d0, d1) = (n0.min(n), n1.max(n), d0.min(d), d1.max(d));
    }
    // The corners of the parallelogram that holds the pairs (p, n).
    let corners = [(n0 + d0, n0), (n0 + d1, n0), (n1 + d0, n1), (n1 + d1, n1)];
    checks.iter().all(|check| match *check {
        Check::Flags { arith, read, seen } => {
            let zones = arith.zones(corners[0].0, corners[0].1);
            corners
                .iter()
                .all(|&(p, n)| arith.zones(p, n) == zones && arith.flags(p, n) & read == seen)
        }
        Check::Window { form, bits, k } => corners
            .iter()
            .all(|&(p, n)| form.at(p, n).div_euclid(1 << bits) == k),
    })
}

/// Before an instruction runs during a pass: what it does with what
/// follows the time.
pub(super) fn analyze(cpu: &mut Cpu) {
    with_proof(cpu, |cpu, t| {
        if t.stage == Stage::Armed {
            return Ok(());
        }
        t.exceptions = cpu.exceptions;
        let pending = look(cpu, t)?;
        t.pending = Some(pending);
        Ok(())
    });
}

/// Where an operand is.
#[derive(Clone)]
enum Place {
    /// A general register's bytes: the register and the first byte.
    Reg(usize, usize),
    /// Bytes of RAM, by physical address.
    Mem(Vec<usize>),
    Imm,
}

/// An operand of the instruction about to run.
#[derive(Clone)]
struct Operand {
    place: Place,
    size: usize,
    /// Its value, and what of it follows the time.
    value: u32,
    parts: Vec<Option<Part>>,
}

impl Operand {
    fn follows(&self) -> bool {
        self.parts.iter().any(Option::is_some)
    }
}

/// The general register and first byte of `r`, and its size.
fn reg_bytes(r: Register) -> Option<(usize, usize, usize)> {
    let index = |base: Register| r as usize - base as usize;
    if r.is_gpr32() {
        Some((index(Register::EAX), 0, 4))
    } else if r.is_gpr16() {
        Some((index(Register::AX), 0, 2))
    } else if (Register::AL..=Register::BL).contains(&r) {
        Some((index(Register::AL), 0, 1))
    } else if (Register::AH..=Register::BH).contains(&r) {
        Some((index(Register::AH), 1, 1))
    } else {
        None
    }
}

/// The instruction at CS:EIP, its bytes found through the page tables.
fn decode(cpu: &Cpu) -> Option<Instruction> {
    let cs = cpu.seg_cache(Seg::CS);
    let bitness = if cs.attr & crate::cpu::ATTR_DB != 0 {
        32
    } else {
        16
    };
    let lin = cs.base.wrapping_add(cpu.eip());
    let mut bytes = [0u8; 15];
    let mut known = 0;
    // Through the page tables once a page: the instruction is in one or
    // two of them.
    let mut page = None;
    for (i, b) in bytes.iter_mut().enumerate() {
        let at = lin.wrapping_add(i as u32);
        if i == 0 || at & 0xFFF == 0 {
            page = cpu.peek_translate(at & !0xFFF);
        }
        let Some(base) = page else {
            break;
        };
        *b = cpu.bus.peek_8((base | (at & 0xFFF)) as usize);
        known = i + 1;
    }
    let instr = Decoder::with_ip(
        bitness,
        &bytes[..known],
        cpu.eip() as u64,
        DecoderOptions::NONE,
    )
    .decode();
    (!instr.is_invalid()).then_some(instr)
}

/// The physical addresses of `m`'s bytes.
fn mem_bytes(cpu: &Cpu, m: &UsedMemory) -> Option<Vec<usize>> {
    let seg_base = |r: Register| Seg::from_register(r).map(|s| cpu.seg_cache(s).base as u64);
    let lin = m.virtual_address(0, |r, _, _| seg_base(r).or(Some(cpu.reg(r) as u64)))? as u32;
    let size = m.memory_size().size().max(1);
    (0..size)
        .map(|i| {
            cpu.peek_translate(lin.wrapping_add(i as u32))
                .map(|a| a as usize)
        })
        .collect()
}

/// What the instruction about to run does with what follows the time, to
/// be settled once it has run; or why the proof can't follow it.
fn look(cpu: &mut Cpu, t: &mut TimeProof) -> Result<Pending, &'static str> {
    let instr = decode(cpu).ok_or("time code")?;
    let (registers, memory) = FACTORY.with_borrow_mut(|f| {
        let info = f.info(&instr);
        (info.used_registers().to_vec(), info.used_memory().to_vec())
    });
    let (p, n) = t.counts();
    let s = &t.shadow;
    let follows_reg = |r: Register| {
        reg_bytes(r).is_some_and(|(i, first, size)| {
            s.regs[i][first..first + size].iter().any(Option::is_some)
        })
    };
    // An address computed from what follows the time.
    let addresses = memory
        .iter()
        .map(|m| (m, mem_bytes(cpu, m)))
        .collect::<Vec<_>>();
    for (m, bytes) in &addresses {
        if follows_reg(m.base()) || follows_reg(m.index()) {
            return Err("time address");
        }
        if bytes.is_none() {
            return Err("time page");
        }
    }
    let mnemonic = instr.mnemonic();
    let setcc = matches!(
        mnemonic,
        Mnemonic::Seta
            | Mnemonic::Setae
            | Mnemonic::Setb
            | Mnemonic::Setbe
            | Mnemonic::Sete
            | Mnemonic::Setg
            | Mnemonic::Setge
            | Mnemonic::Setl
            | Mnemonic::Setle
            | Mnemonic::Setne
            | Mnemonic::Setno
            | Mnemonic::Setns
            | Mnemonic::Seto
            | Mnemonic::Sets
    );
    // The count's bytes, read after a latch in this pass.
    if mnemonic == Mnemonic::In && instr.op0_register() == Register::AL {
        let port = if instr.op1_kind() == OpKind::Immediate8 {
            instr.immediate8() as u16
        } else {
            cpu.dx()
        };
        let bus = &cpu.bus;
        if port != 0x40 || t.latch.is_none() || !bus.pit0_latched_active || bus.pit0_access != 3 {
            return Err("time port");
        }
        let byte = bus.pit0_read_msb as u8;
        cpu.bus.observe.port_ok = Some(0x40);
        t.reads += 1;
        return Ok(Pending {
            regs: vec![(
                0,
                0,
                Some(Part {
                    form: Form::COUNT,
                    byte,
                }),
            )],
            ..Pending::default()
        });
    }
    let reads = |a: OpAccess| {
        matches!(
            a,
            OpAccess::Read | OpAccess::ReadWrite | OpAccess::CondRead | OpAccess::ReadCondWrite
        )
    };
    // Flags kept on the stack: PUSHF stores those that follow the time
    // and POPF takes them back. Nothing else may read them.
    let stack = |access: OpAccess| -> Result<Vec<usize>, &'static str> {
        let (_, bytes) = addresses
            .iter()
            .find(|(m, _)| m.access() == access)
            .ok_or("time op")?;
        bytes.clone().ok_or("time page")
    };
    match mnemonic {
        Mnemonic::Pushf | Mnemonic::Pushfd if s.opaque == 0 && s.flags.is_some() => {
            let arith = s.flags.ok_or("time flags")?;
            let bytes = stack(OpAccess::Write)?;
            let flag_mem = (0..2u8)
                .filter(|&b| FlagByte::mask(arith, b) != 0)
                .map(|b| {
                    (
                        bytes[b as usize],
                        FlagByte {
                            arith,
                            byte: b,
                            rest: 0,
                        },
                    )
                })
                .collect();
            return Ok(Pending {
                flag_mem,
                ..Pending::default()
            });
        }
        Mnemonic::Popf | Mnemonic::Popfd => {
            let bytes = stack(OpAccess::Read)?;
            if let Some(first) = bytes.iter().find_map(|a| s.flag_mem.get(a)) {
                let arith = first.arith;
                let whole = bytes.iter().enumerate().all(|(b, a)| {
                    let stored =
                        (b < 2 && FlagByte::mask(arith, b as u8) != 0).then_some((arith, b as u8));
                    s.flag_mem.get(a).map(|x| (x.arith, x.byte)) == stored && !s.mem.contains_key(a)
                });
                if !whole {
                    return Err("time flags");
                }
                return Ok(Pending {
                    written: ARITH,
                    flags: Some(arith),
                    ..Pending::default()
                });
            }
        }
        _ => {}
    }
    let flag_mem_read = addresses.iter().any(|(m, bytes)| {
        reads(m.access())
            && bytes
                .as_ref()
                .is_some_and(|b| b.iter().any(|a| s.flag_mem.contains_key(a)))
    });
    if flag_mem_read {
        return Err("time flags");
    }
    let reg_read = registers
        .iter()
        .any(|u| reads(u.access()) && follows_reg(u.register()));
    let mem_read = addresses.iter().any(|(m, bytes)| {
        reads(m.access())
            && bytes
                .as_ref()
                .is_some_and(|b| b.iter().any(|a| s.mem.contains_key(a)))
    });
    let interrupt = matches!(
        mnemonic,
        Mnemonic::Int
            | Mnemonic::Int1
            | Mnemonic::Int3
            | Mnemonic::Into
            | Mnemonic::Pushf
            | Mnemonic::Pushfd
    );
    let flags_read = if interrupt {
        ARITH
    } else {
        rflags(instr.rflags_read()) & ARITH
    };
    let live = s.flags.map_or(0, |a| a.bits) | s.opaque;
    let flag_follows = flags_read & live != 0;
    let written = rflags(instr.rflags_modified()) & ARITH;
    if !reg_read && !mem_read && !flag_follows {
        // Nothing it reads follows the time: neither does what it writes.
        let mut pending = Pending {
            written,
            ..Pending::default()
        };
        for u in &registers {
            let Some((i, first, size)) = reg_bytes(u.register()) else {
                continue;
            };
            match u.access() {
                OpAccess::Write | OpAccess::ReadWrite => {
                    pending
                        .regs
                        .extend((first..first + size).map(|b| (i, b, None)));
                }
                OpAccess::CondWrite | OpAccess::ReadCondWrite if follows_reg(u.register()) => {
                    return Err("time op");
                }
                _ => {}
            }
        }
        return Ok(pending);
    }
    // Flags read that follow the time: a branch, SETcc, or a carry, which
    // the skipped passes must see the same.
    let mut checks = Vec::new();
    if flag_follows {
        if flags_read & s.opaque != 0 {
            return Err("time flags");
        }
        let arith = s.flags.ok_or("time flags")?;
        let read = flags_read & arith.bits;
        if read & (PF | AF) != 0 {
            return Err("time parity");
        }
        let seen = cpu.get_cpu_flags().bits() & read;
        checks.push(Check::Flags { arith, read, seen });
    }
    let operand = |i: u32| -> Result<Operand, &'static str> {
        match instr.op_kind(i) {
            OpKind::Register => {
                let (r, first, size) = reg_bytes(instr.op_register(i)).ok_or("time op")?;
                let value = cpu.reg(instr.op_register(i));
                Ok(Operand {
                    place: Place::Reg(r, first),
                    size,
                    value,
                    parts: s.regs[r][first..first + size].to_vec(),
                })
            }
            OpKind::Memory => {
                let (m, bytes) = addresses.first().ok_or("time op")?;
                let bytes = bytes.clone().ok_or("time page")?;
                let size = m.memory_size().size();
                if bytes.len() != size || size > 4 {
                    return Err("time op");
                }
                let value = bytes
                    .iter()
                    .rev()
                    .fold(0u32, |v, &a| v << 8 | cpu.bus.peek_8(a) as u32);
                let parts = bytes.iter().map(|a| s.mem.get(a).copied()).collect();
                Ok(Operand {
                    place: Place::Mem(bytes),
                    size,
                    value,
                    parts,
                })
            }
            OpKind::Immediate8
            | OpKind::Immediate16
            | OpKind::Immediate32
            | OpKind::Immediate8to16
            | OpKind::Immediate8to32 => {
                let size = (instr.op0_kind() == OpKind::Register)
                    .then(|| reg_bytes(instr.op0_register()).map(|r| r.2))
                    .flatten()
                    .unwrap_or_else(|| instr.memory_size().size());
                Ok(Operand {
                    place: Place::Imm,
                    size,
                    value: instr.immediate(i) as u32,
                    parts: vec![None; size],
                })
            }
            _ => Err("time op"),
        }
    };
    // The value of an operand as a function of the counts.
    let mut form = |o: &Operand| -> Result<Form, &'static str> {
        let full = if o.size == 4 {
            u32::MAX
        } else {
            (1u32 << (8 * o.size)) - 1
        };
        let value = o.value & full;
        if !o.follows() {
            return Ok(Form::constant(value));
        }
        let first = o.parts[0].ok_or("time bytes")?.form;
        let low = o
            .parts
            .iter()
            .take_while(|b| b.is_some_and(|b| b.form == first))
            .count();
        let ordered = o.parts[..low]
            .iter()
            .enumerate()
            .all(|(i, b)| b.is_some_and(|b| b.byte as usize == i));
        if !ordered || o.parts[low..].iter().any(Option::is_some) {
            return Err("time bytes");
        }
        if (0..low).any(|i| first.byte(p, n, i as u8) != (value >> (8 * i)) as u8) {
            return Err("time model");
        }
        if low == o.size {
            return Ok(first);
        }
        // The low bytes follow the time and the high ones are constant:
        // the value is linear while the form's high bits stay.
        let bits = 8 * low as u32;
        let k = first.at(p, n).div_euclid(1 << bits);
        checks.push(Check::Window {
            form: first,
            bits,
            k,
        });
        let high = (value as i64 >> bits) << bits;
        Ok(Form {
            c: (first.c - (k << bits) + high).rem_euclid(MOD),
            ..first
        })
    };
    let parts_of = |f: Form, size: usize| {
        (0..size)
            .map(|b| {
                Some(Part {
                    form: f,
                    byte: b as u8,
                })
            })
            .collect::<Vec<_>>()
    };
    let mut pending = Pending {
        written,
        ..Pending::default()
    };
    let put = |pending: &mut Pending, place: &Place, parts: &[Option<Part>]| match place {
        Place::Reg(r, first) => {
            pending
                .regs
                .extend(parts.iter().enumerate().map(|(b, &x)| (*r, first + b, x)));
        }
        Place::Mem(bytes) => {
            pending.mem.extend(
                bytes
                    .iter()
                    .zip(parts)
                    .filter_map(|(&a, &x)| x.map(|x| (a, x))),
            );
        }
        Place::Imm => {}
    };
    // The registers it writes, as nothing that follows the time; those it
    // writes with a value that does are put after.
    for u in &registers {
        if let Some((i, first, size)) = reg_bytes(u.register())
            && matches!(u.access(), OpAccess::Write | OpAccess::ReadWrite)
        {
            pending
                .regs
                .extend((first..first + size).map(|b| (i, b, None)));
        }
    }
    let same_register = instr.op_count() == 2
        && instr.op0_kind() == OpKind::Register
        && instr.op1_kind() == OpKind::Register
        && instr.op0_register() == instr.op1_register();
    match mnemonic {
        Mnemonic::Mov => {
            let (dst, src) = (operand(0)?, operand(1)?);
            if dst.size != src.size {
                return Err("time op");
            }
            put(&mut pending, &dst.place, &src.parts);
        }
        Mnemonic::Movzx => {
            let (dst, src) = (operand(0)?, operand(1)?);
            let mut parts = src.parts.clone();
            parts.resize(dst.size, None);
            put(&mut pending, &dst.place, &parts);
        }
        Mnemonic::Xchg => {
            let (a, b) = (operand(0)?, operand(1)?);
            put(&mut pending, &a.place, &b.parts);
            put(&mut pending, &b.place, &a.parts);
        }
        Mnemonic::Push if instr.op0_kind() == OpKind::Register => {
            let src = operand(0)?;
            let (m, bytes) = addresses
                .iter()
                .find(|(m, _)| m.access() == OpAccess::Write)
                .ok_or("time op")?;
            let bytes = bytes.clone().ok_or("time page")?;
            if m.memory_size().size() != src.size {
                return Err("time op");
            }
            put(&mut pending, &Place::Mem(bytes), &src.parts);
        }
        Mnemonic::Pop if instr.op0_kind() == OpKind::Register => {
            let dst = operand(0)?;
            let (_, bytes) = addresses
                .iter()
                .find(|(m, _)| m.access() == OpAccess::Read)
                .ok_or("time op")?;
            let bytes = bytes.clone().ok_or("time page")?;
            if bytes.len() != dst.size {
                return Err("time op");
            }
            let parts: Vec<_> = bytes.iter().map(|a| s.mem.get(a).copied()).collect();
            put(&mut pending, &dst.place, &parts);
        }
        Mnemonic::Sub | Mnemonic::Xor if same_register => {}
        Mnemonic::Add | Mnemonic::Sub | Mnemonic::Adc | Mnemonic::Sbb | Mnemonic::Cmp => {
            let (dst, src) = (operand(0)?, operand(1)?);
            let (a, b) = (form(&dst)?, form(&src)?);
            let sub = matches!(mnemonic, Mnemonic::Sub | Mnemonic::Sbb | Mnemonic::Cmp);
            let carry =
                matches!(mnemonic, Mnemonic::Adc | Mnemonic::Sbb) && cpu.get_cpu_flag(CpuFlags::CF);
            let result = if sub {
                a.minus(b).minus(Form::constant(carry as u32))
            } else {
                a.plus(b).plus(Form::constant(carry as u32))
            };
            let size = dst.size as u8;
            if dst.follows() || src.follows() {
                pending.flags = Some(Arith {
                    sub,
                    size,
                    a,
                    b,
                    carry,
                    bits: ARITH,
                });
                if mnemonic != Mnemonic::Cmp {
                    put(&mut pending, &dst.place, &parts_of(result, dst.size));
                }
            }
        }
        Mnemonic::Inc | Mnemonic::Dec | Mnemonic::Neg => {
            let dst = operand(0)?;
            if !dst.follows() {
                return Err("time op");
            }
            let a = form(&dst)?;
            let one = Form::constant(1);
            let arith = match mnemonic {
                Mnemonic::Inc => Arith {
                    sub: false,
                    size: dst.size as u8,
                    a,
                    b: one,
                    carry: false,
                    bits: ARITH & !CF,
                },
                Mnemonic::Dec => Arith {
                    sub: true,
                    size: dst.size as u8,
                    a,
                    b: one,
                    carry: false,
                    bits: ARITH & !CF,
                },
                _ => Arith {
                    sub: true,
                    size: dst.size as u8,
                    a: Form::constant(0),
                    b: a,
                    carry: false,
                    bits: ARITH,
                },
            };
            // CF stays from an instruction before, which mustn't follow the
            // time too.
            if arith.bits & CF == 0 && live & CF != 0 {
                return Err("time carry");
            }
            let result = if arith.sub {
                arith.a.minus(arith.b)
            } else {
                arith.a.plus(arith.b)
            };
            pending.flags = Some(arith);
            put(&mut pending, &dst.place, &parts_of(result, dst.size));
        }
        Mnemonic::And | Mnemonic::Or | Mnemonic::Xor | Mnemonic::Test => {
            // Only what leaves the value as it is: AND with all ones, OR
            // and XOR with 0. The flags then follow the time some way this
            // doesn't.
            let (dst, src) = (operand(0)?, operand(1)?);
            let full = if dst.size == 4 {
                u32::MAX
            } else {
                (1u32 << (8 * dst.size)) - 1
            };
            let keeps = match mnemonic {
                Mnemonic::And => src.value & full == full,
                Mnemonic::Test => true,
                _ => src.value & full == 0,
            };
            if src.follows() || !keeps {
                return Err("time op");
            }
            if mnemonic != Mnemonic::Test {
                put(&mut pending, &dst.place, &dst.parts);
            }
            pending.opaque = ARITH;
        }
        // Branches and SETcc read only the flags.
        _ if (instr.is_jcc_short_or_near() || setcc) && !reg_read && !mem_read => {}
        _ => return Err("time op"),
    }
    t.checks.extend(checks);
    Ok(pending)
}
