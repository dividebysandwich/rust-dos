//! Instruction trace ring buffer. Entries are recorded on the hot path as
//! compact fixed-size structs; disassembly happens only when a range is
//! served, so enabling the trace costs one ~64-byte copy per instruction.

use iced_x86::{Decoder, DecoderOptions};

#[derive(Clone, Copy, Default)]
pub struct TraceEntry {
    /// Microseconds since emulator start. Sampled once per execution batch
    /// (~16 ms), so use `icount` for exact ordering within a batch.
    pub t_us: u64,
    /// Global instruction counter at the time this instruction executed.
    pub icount: u64,
    pub cs: u16,
    pub eip: u32,
    /// EAX, ECX, EDX, EBX, ESP, EBP, ESI, EDI.
    pub gpr: [u32; 8],
    pub ds: u16,
    pub es: u16,
    pub ss: u16,
    pub fs: u16,
    pub gs: u16,
    pub eflags: u32,
    /// The code segment is 32-bit.
    pub code32: bool,
    pub bytes: [u8; 15],
    pub len: u8,
}

/// Register names in `TraceEntry::gpr` order.
const GPR_NAMES: [&str; 8] = ["ax", "cx", "dx", "bx", "sp", "bp", "si", "di"];
/// The order registers are shown in.
const SHOW_ORDER: [usize; 8] = [0, 3, 1, 2, 6, 7, 5, 4];

impl TraceEntry {
    pub fn code_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// Show 32-bit registers: in 32-bit code, or when 16-bit code uses the
    /// upper halves.
    fn wide(&self) -> bool {
        self.code32 || self.eip > 0xFFFF || self.gpr.iter().any(|r| r >> 16 != 0)
    }

    /// Disassemble this entry. Returns (instruction bytes actually used,
    /// text). HLE traps (`FE 38 xx`) are shown as the BIOS/DOS service they
    /// invoke, which is far more useful than the undefined opcode.
    pub fn disasm(&self) -> (usize, String) {
        let bytes = self.code_bytes();
        if bytes.len() >= 3 && bytes[0] == 0xFE && bytes[1] == 0x38 {
            if bytes[2] == crate::shell::SHELL_COMMAND_BOP {
                return (3, "HLE shell command".to_string());
            }
            return (3, format!("HLE INT {:02X}h (AX={:04X})", bytes[2], self.gpr[0] as u16));
        }
        disasm_one(bytes, self.eip, self.code32)
    }

    pub fn to_json(&self) -> serde_json::Value {
        let (len, text) = self.disasm();
        let h = |v: u16| format!("{:04X}", v);
        let h32 = |v: u32| format!("{:08X}", v);
        let mut v = serde_json::json!({
            "icount": self.icount,
            "t_ms": self.t_us as f64 / 1000.0,
            "cs": h(self.cs), "ip": h(self.eip as u16), "eip": h32(self.eip),
            "bytes": hex_bytes(&self.bytes[..len.min(self.len as usize)]),
            "asm": text,
            "ds": h(self.ds), "es": h(self.es), "ss": h(self.ss), "fs": h(self.fs), "gs": h(self.gs),
            "flags": h(self.eflags as u16), "eflags": h32(self.eflags),
            "code32": self.code32,
        });
        for (i, name) in GPR_NAMES.iter().enumerate() {
            v[*name] = h(self.gpr[i] as u16).into();
            v[format!("e{}", name)] = h32(self.gpr[i]).into();
        }
        v
    }

    pub fn to_text(&self) -> String {
        let (len, text) = self.disasm();
        let wide = self.wide();
        let mut regs = String::new();
        for i in SHOW_ORDER {
            let name = GPR_NAMES[i].to_ascii_uppercase();
            if wide {
                regs.push_str(&format!("E{}={:08X} ", name, self.gpr[i]));
            } else {
                regs.push_str(&format!("{}={:04X} ", name, self.gpr[i] as u16));
            }
        }
        let segs = if self.fs != 0 || self.gs != 0 {
            format!(
                "DS={:04X} ES={:04X} SS={:04X} FS={:04X} GS={:04X}",
                self.ds, self.es, self.ss, self.fs, self.gs
            )
        } else {
            format!("DS={:04X} ES={:04X} SS={:04X}", self.ds, self.es, self.ss)
        };
        let ip = if wide { format!("{:04X}:{:08X}", self.cs, self.eip) } else { format!("{:04X}:{:04X}", self.cs, self.eip) };
        let flags = if wide { format!("FL={:08X}", self.eflags) } else { format!("FL={:04X}", self.eflags as u16) };
        format!(
            "{:>12} {:>11.3} {}  {:<20} {:<32} {}{} {}",
            self.icount,
            self.t_us as f64 / 1000.0,
            ip,
            hex_bytes(&self.bytes[..len.min(self.len as usize)]),
            text,
            regs,
            segs,
            flags
        )
    }
}

pub const TEXT_HEADER: &str =
    "      icount        t_ms CS:IP      bytes                asm                              registers (before execution)";

pub fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ")
}

/// Disassemble one instruction of 16-bit or 32-bit code. Returns (length,
/// text).
pub fn disasm_one(bytes: &[u8], ip: u32, code32: bool) -> (usize, String) {
    let bitness = if code32 { 32 } else { 16 };
    let mut decoder = Decoder::with_ip(bitness, bytes, ip as u64, DecoderOptions::NONE);
    let instr = decoder.decode();
    if instr.is_invalid() {
        return (1, "(bad)".to_string());
    }
    (instr.len(), format!("{}", instr))
}

pub struct TraceRing {
    buf: Vec<TraceEntry>,
    capacity: usize,
    /// Next write position.
    head: usize,
    /// Total entries ever pushed (monotonic). `total - len` entries were
    /// overwritten.
    total: u64,
}

impl TraceRing {
    pub fn new(capacity: usize) -> Self {
        Self { buf: Vec::new(), capacity: capacity.max(1), head: 0, total: 0 }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn clear(&mut self) {
        // Keep the allocation; a restarted trace will refill it anyway.
        self.buf.clear();
        self.head = 0;
    }

    #[inline]
    pub fn push(&mut self, e: TraceEntry) {
        if self.buf.len() < self.capacity {
            if self.buf.capacity() == 0 {
                // Allocate lazily so a disabled trace costs no memory.
                self.buf.reserve_exact(self.capacity);
            }
            self.buf.push(e);
        } else {
            self.buf[self.head] = e;
        }
        self.head = (self.head + 1) % self.capacity;
        self.total += 1;
    }

    /// Iterate oldest → newest.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &TraceEntry> {
        let split = if self.buf.len() < self.capacity { 0 } else { self.head };
        self.buf[split..].iter().chain(self.buf[..split].iter())
    }

    /// The newest `n` entries, oldest first.
    pub fn last_n(&self, n: usize) -> Vec<TraceEntry> {
        let mut v: Vec<TraceEntry> = self.iter().rev().take(n).copied().collect();
        v.reverse();
        v
    }

    /// Entries pushed after the entry with sequence number `since_total`
    /// (as returned by `total()`), capped to the newest `max`. Also returns
    /// how many matching entries were dropped because of the cap or because
    /// they were already overwritten.
    pub fn since(&self, since_total: u64, max: usize) -> (Vec<TraceEntry>, u64) {
        let new = self.total.saturating_sub(since_total);
        let available = (new as usize).min(self.buf.len());
        let take = available.min(max);
        let dropped = new - take as u64;
        (self.last_n(take), dropped)
    }
}
