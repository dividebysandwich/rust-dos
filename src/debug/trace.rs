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
    pub ip: u16,
    pub ax: u16,
    pub bx: u16,
    pub cx: u16,
    pub dx: u16,
    pub si: u16,
    pub di: u16,
    pub bp: u16,
    pub sp: u16,
    pub ds: u16,
    pub es: u16,
    pub ss: u16,
    pub flags: u16,
    pub bytes: [u8; 15],
    pub len: u8,
}

impl TraceEntry {
    pub fn code_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
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
            return (3, format!("HLE INT {:02X}h (AX={:04X})", bytes[2], self.ax));
        }
        disasm_one(bytes, self.ip)
    }

    pub fn to_json(&self) -> serde_json::Value {
        let (len, text) = self.disasm();
        let h = |v: u16| format!("{:04X}", v);
        serde_json::json!({
            "icount": self.icount,
            "t_ms": self.t_us as f64 / 1000.0,
            "cs": h(self.cs), "ip": h(self.ip),
            "bytes": hex_bytes(&self.bytes[..len.min(self.len as usize)]),
            "asm": text,
            "ax": h(self.ax), "bx": h(self.bx), "cx": h(self.cx), "dx": h(self.dx),
            "si": h(self.si), "di": h(self.di), "bp": h(self.bp), "sp": h(self.sp),
            "ds": h(self.ds), "es": h(self.es), "ss": h(self.ss),
            "flags": h(self.flags),
        })
    }

    pub fn to_text(&self) -> String {
        let (len, text) = self.disasm();
        format!(
            "{:>12} {:>11.3} {:04X}:{:04X}  {:<20} {:<32} AX={:04X} BX={:04X} CX={:04X} DX={:04X} SI={:04X} DI={:04X} BP={:04X} SP={:04X} DS={:04X} ES={:04X} SS={:04X} FL={:04X}",
            self.icount,
            self.t_us as f64 / 1000.0,
            self.cs,
            self.ip,
            hex_bytes(&self.bytes[..len.min(self.len as usize)]),
            text,
            self.ax,
            self.bx,
            self.cx,
            self.dx,
            self.si,
            self.di,
            self.bp,
            self.sp,
            self.ds,
            self.es,
            self.ss,
            self.flags
        )
    }
}

pub const TEXT_HEADER: &str =
    "      icount        t_ms CS:IP      bytes                asm                              registers (before execution)";

pub fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ")
}

/// Disassemble one 16-bit instruction. Returns (length, text).
pub fn disasm_one(bytes: &[u8], ip: u16) -> (usize, String) {
    let mut decoder = Decoder::with_ip(16, bytes, ip as u64, DecoderOptions::NONE);
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
