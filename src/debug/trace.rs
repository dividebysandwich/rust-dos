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
    /// Protected mode, not virtual-8086 mode: CS holds a selector, and
    /// `FE 38`/`FE 39` raise #UD instead of running a service.
    pub pm: bool,
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

    /// Whether `no_bios=true` hides this entry: code in the BIOS segment
    /// F000 other than the HLE traps, which stand for the services they run.
    /// In protected mode CS is a selector, which says nothing about the BIOS.
    pub fn is_bios_code(&self) -> bool {
        !self.pm && self.cs >= 0xF000 && !is_trap(&self.bytes, false)
    }

    /// Disassemble this entry. Returns (instruction bytes actually used,
    /// text). HLE traps are shown as what they run (`trap_name`), which is
    /// far more useful than the undefined opcode.
    pub fn disasm(&self) -> (usize, String) {
        let bytes = self.code_bytes();
        if let Some(name) = trap_name(bytes, Some(self.gpr[0] as u16), self.pm) {
            return (3, name);
        }
        disasm_one(bytes, self.eip, self.code32)
    }

    pub fn to_json(self) -> serde_json::Value {
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

    pub fn to_text(self) -> String {
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

/// What the emulator's service trap at the start of `bytes` runs (the
/// `FE 38..3B vv` kinds of `exec::service_trap`), or None if it holds none.
/// An interrupt's line gives AX, its function, when `ax` is known. In
/// protected mode (`pm`) `FE 38` and `FE 39` are the invalid FE /7 opcode
/// they encode, as `exec::service_trap` runs them only in real and
/// virtual-8086 mode.
pub fn trap_name(bytes: &[u8], ax: Option<u16>, pm: bool) -> Option<String> {
    let &[0xFE, kind @ 0x38..=0x3B, vv, ..] = bytes else {
        return None;
    };
    if pm && kind <= 0x39 {
        return None;
    }
    Some(match (kind, ax) {
        (0x38, _) if vv == crate::shell::SHELL_COMMAND_BOP => "HLE shell command".to_string(),
        (0x38, Some(ax)) => format!("HLE INT {vv:02X}h (AX={ax:04X})"),
        (0x38, None) => format!("HLE INT {vv:02X}h"),
        (0x39, _) => format!("HLE service {vv:02X}h"),
        (0x3A, _) => format!("HLE far service {vv:02X}h"),
        _ => format!("HLE DPMI {vv:02X}h"),
    })
}

/// Whether `bytes` start with a service trap that `trap_name` names, in
/// protected mode (`pm`) or outside it, without building the name.
fn is_trap(bytes: &[u8], pm: bool) -> bool {
    matches!(bytes, &[0xFE, kind @ 0x38..=0x3B, _, ..] if !(pm && kind <= 0x39))
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

    /// Entries after sequence number `seq`, oldest first and paired with
    /// their numbers (an entry's number is `total()` right after its push,
    /// so the first is 1). Also returns how many entries after `seq` were
    /// already overwritten.
    pub fn after(&self, seq: u64) -> (impl Iterator<Item = (u64, &TraceEntry)>, u64) {
        let oldest = self.total - self.buf.len() as u64 + 1;
        let overwritten = (oldest - 1).saturating_sub(seq);
        // Skip before zipping: the ring's slices skip in one step, while
        // the zipped pairs would be walked one entry at a time.
        let skip = seq.saturating_sub(oldest - 1).min(self.buf.len() as u64) as usize;
        let entries = (oldest + skip as u64..).zip(self.iter().skip(skip));
        (entries, overwritten)
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

#[cfg(test)]
mod tests {
    use super::{TraceEntry, TraceRing, trap_name};

    fn ring(capacity: usize, pushed: u64) -> TraceRing {
        let mut ring = TraceRing::new(capacity);
        for icount in 1..=pushed {
            ring.push(TraceEntry { icount, ..Default::default() });
        }
        ring
    }

    fn after(ring: &TraceRing, seq: u64) -> (Vec<(u64, u64)>, u64) {
        let (entries, overwritten) = ring.after(seq);
        (entries.map(|(n, e)| (n, e.icount)).collect(), overwritten)
    }

    #[test]
    fn entries_after_a_cursor_come_numbered_oldest_first() {
        let ring = ring(4, 3);
        assert_eq!(after(&ring, 0), (vec![(1, 1), (2, 2), (3, 3)], 0));
        assert_eq!(after(&ring, 2), (vec![(3, 3)], 0));
        assert_eq!(after(&ring, 3), (vec![], 0));
    }

    #[test]
    fn entries_overwritten_since_the_cursor_are_counted() {
        // Entries 1 to 6 are gone; 7 to 10 are kept.
        let ring = ring(4, 10);
        assert_eq!(after(&ring, 2), (vec![(7, 7), (8, 8), (9, 9), (10, 10)], 4));
        assert_eq!(after(&ring, 8), (vec![(9, 9), (10, 10)], 0));
    }

    fn code(bytes: &[u8]) -> TraceEntry {
        let mut entry = TraceEntry { len: bytes.len() as u8, ..Default::default() };
        entry.bytes[..bytes.len()].copy_from_slice(bytes);
        entry.gpr[0] = 0x4C00;
        entry
    }

    #[test]
    fn hle_traps_are_shown_as_what_they_run() {
        // The INT 21h stub, and the timer handler's tick count in the ROM.
        assert_eq!(code(&[0xFE, 0x38, 0x21, 0xCF]).disasm(), (3, "HLE INT 21h (AX=4C00)".to_string()));
        assert_eq!(code(&[0xFE, 0x39, 0x08, 0xCD, 0x1C]).disasm(), (3, "HLE service 08h".to_string()));
        // The Plug and Play BIOS's entry point, and the DPMI host's.
        assert_eq!(code(&[0xFE, 0x3A, 0x01, 0xCB]).disasm(), (3, "HLE far service 01h".to_string()));
        assert_eq!(code(&[0xFE, 0x3B, 0x01, 0x90]).disasm(), (3, "HLE DPMI 01h".to_string()));
        assert_eq!(trap_name(&[0xFE, 0x38, 0x21], None, false).as_deref(), Some("HLE INT 21h"));
        assert_eq!(trap_name(&[0xFE, 0x3C, 0x21], None, false), None);
    }

    #[test]
    fn fe_38_and_39_in_protected_mode_are_the_invalid_opcode() {
        // exec::service_trap runs them only in real and virtual-8086 mode;
        // the far-call and DPMI traps run in protected mode too.
        let pm = |bytes: &[u8]| TraceEntry { pm: true, ..code(bytes) };
        assert_eq!(pm(&[0xFE, 0x38, 0x21]).disasm().1, "(bad)");
        assert_eq!(pm(&[0xFE, 0x39, 0x08]).disasm().1, "(bad)");
        assert_eq!(pm(&[0xFE, 0x3A, 0x01]).disasm(), (3, "HLE far service 01h".to_string()));
        assert_eq!(pm(&[0xFE, 0x3B, 0x01]).disasm(), (3, "HLE DPMI 01h".to_string()));
        assert_eq!(trap_name(&[0xFE, 0x39, 0x08], None, true), None);
    }

    #[test]
    fn no_bios_keeps_the_hle_traps() {
        let at = |cs: u16, bytes: &[u8]| TraceEntry { cs, ..code(bytes) };
        assert!(!at(0xF000, &[0xFE, 0x38, 0x21]).is_bios_code());
        assert!(!at(0xF000, &[0xFE, 0x39, 0x08]).is_bios_code());
        assert!(!at(0xF000, &[0xFE, 0x3A, 0x01]).is_bios_code());
        assert!(!at(0xF000, &[0xFE, 0x3B, 0x01]).is_bios_code());
        assert!(at(0xF000, &[0xCD, 0x1C]).is_bios_code());
        assert!(!at(0x1000, &[0xCD, 0x1C]).is_bios_code());
        // A protected-mode selector of F000 or above is no BIOS segment.
        assert!(!TraceEntry { pm: true, ..at(0xF00F, &[0xCD, 0x1C]) }.is_bios_code());
    }

    #[test]
    fn a_cleared_ring_keeps_its_numbers() {
        let mut ring = ring(4, 5);
        ring.clear();
        assert_eq!(after(&ring, 5), (vec![], 0));
        ring.push(TraceEntry { icount: 99, ..Default::default() });
        assert_eq!(after(&ring, 5), (vec![(6, 99)], 0));
    }
}
