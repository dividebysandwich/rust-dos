//! Decoded-instruction cache for the inner execution loop.
//!
//! Hot DOS loops re-execute the same short body many thousands of times per
//! second (palette blits, string operations, tight game main loops). Running
//! those bytes through `iced_x86::Decoder::decode_out` every single iteration
//! shows up prominently in the profile, so we cache the decoded
//! `iced_x86::Instruction` keyed by its physical address and reuse it on the
//! next hit.
//!
//! Correctness in the face of self-modifying code is handled via a generation
//! counter per small block of RAM on the bus (`Bus::page_gen`). Every write
//! through the bus bumps the gen for the affected block, and every cache slot
//! records the gens that were live at decode time. A slot is only returned on
//! lookup when they still match, so an LZEXE-style unpacker that rewrites its own
//! code simply causes the cache to refill transparently on the next fetch.
//!
//! Two different (cs, ip) pairs can resolve to the same physical address but
//! produce instructions with different `near_branch16` / `next_ip` fields
//! (iced stores absolute targets, not displacements). The cache key
//! therefore includes the instruction pointer, not just the physical
//! address, and the code size, which decides how the bytes decode.
//!
//! Each slot also keeps the handler chosen for the instruction when it was
//! decoded (`instructions::handler`), so a hit goes straight to it, and
//! whether it is simple (see `exec::chain`).

use iced_x86::Instruction;

use crate::instructions::{Handler, execute_instruction, fast};

/// One cache slot, keyed by the physical address, the instruction pointer
/// (iced stores absolute branch targets, so the same bytes decoded at
/// another EIP differ) and the code size. Empty slots have an impossible
/// physical address. 64 bytes: one cache line.
#[derive(Clone, Copy)]
struct Slot {
    /// The physical address (high half) and the instruction pointer.
    addr: u64,
    /// The page generation (bits 1-32) and whether it's 32-bit code (bit
    /// 0); bit 63, `SIMPLE`, is set for a simple instruction.
    version: u64,
    handler: Handler,
    instr: Instruction,
}

impl Slot {
    #[inline(always)]
    fn empty() -> Self {
        Self {
            addr: u64::MAX,
            version: 0,
            handler: execute_instruction,
            instr: Instruction::default(),
        }
    }
}

/// A decode of an instruction fetched a byte at a time (`exec::fetch_slow`),
/// which may run on into a page elsewhere in physical memory that no
/// generation covers: it is kept with the 16 bytes it was decoded from.
#[derive(Clone, Copy)]
struct BytesSlot {
    bytes: [u8; 16],
    /// The instruction pointer, and whether it's 32-bit code (bit 32).
    ip: u64,
    handler: Handler,
    instr: Instruction,
}

/// The bit of `Slot::version` that marks a simple instruction: one that
/// `fast::select` has a handler for, which changes nothing the execution
/// loop checks between instructions but EIP (see `exec::chain`).
const SIMPLE: u64 = 1 << 63;

/// A decoded instruction, its handler, and whether it is simple.
pub type Decoded<'a> = (&'a Instruction, Handler, bool);

/// Slots for instructions fetched a byte at a time.
const BYTES_SLOTS: usize = 256;

/// Direct-mapped decoded-instruction cache. On collision the old entry is
/// simply overwritten — an LRU would add bookkeeping cost on the hot path and
/// empirically direct-mapped behaves well for typical DOS workloads where the
/// working set of PCs is small relative to cache capacity.
pub struct InstrCache {
    slots: Box<[Slot]>,
    mask: usize,
    bytes_slots: Box<[BytesSlot]>,
    /// The bytes each slot's instruction was decoded from, beside the slots
    /// as only a miss looks at them (see `decode`).
    code: Box<[[u8; 16]]>,
    /// Lookups served from the cache, and lookups that had to decode.
    pub hits: u64,
    pub misses: u64,
}

impl Default for InstrCache {
    /// A cache with no slots, as a placeholder while the real one is lent
    /// out (see `exec::run_batch`). It must not be used for lookups.
    fn default() -> Self {
        Self {
            slots: Box::new([]),
            mask: 0,
            bytes_slots: Box::new([]),
            code: Box::new([]),
            hits: 0,
            misses: 0,
        }
    }
}

impl InstrCache {
    /// `capacity_log2` = log2 of the number of slots. 16 → 64K slots ≈ 3.5 MB.
    /// This is comfortably larger than the working set of any DOS program we
    /// care about while staying small enough to fit in L2/L3.
    pub fn new(capacity_log2: u32) -> Self {
        let n = 1usize << capacity_log2;
        let slots = vec![Slot::empty(); n].into_boxed_slice();
        let empty = BytesSlot { bytes: [0; 16], ip: u64::MAX, handler: execute_instruction, instr: Instruction::default() };
        Self {
            slots,
            mask: n - 1,
            bytes_slots: vec![empty; BYTES_SLOTS].into_boxed_slice(),
            code: vec![[0; 16]; n].into_boxed_slice(),
            hits: 0,
            misses: 0,
        }
    }

    #[inline(always)]
    fn index(&self, phys_ip: usize) -> usize {
        phys_ip & self.mask
    }

    /// The decoded instruction at `phys_ip`, decoded at `ip` as 16 or 32-bit
    /// code, and its handler, if the slot holds it and the recorded page
    /// generation still matches the current one. Returning a reference
    /// into the slot saves copying the instruction on every hit.
    #[inline(always)]
    pub fn get(&mut self, phys_ip: usize, ip: u32, code32: bool, page_gen: u32) -> Option<Decoded<'_>> {
        let idx = self.index(phys_ip);
        // SAFETY: idx is always in-bounds because we masked with `self.mask`
        // which is `len - 1` for a power-of-two-sized slots box.
        let slot = unsafe { self.slots.get_unchecked(idx) };
        let addr = (phys_ip as u64) << 32 | ip as u64;
        let version = (page_gen as u64) << 1 | code32 as u64;
        if slot.addr != addr || slot.version & !SIMPLE != version {
            return None;
        }
        self.hits += 1;
        Some((&slot.instr, slot.handler, slot.version & SIMPLE != 0))
    }

    /// Fill the slot of the instruction at `phys_ip` (see `get`), whose
    /// bytes `ram` holds, with what `decode` decodes, and return it. Where
    /// the slot has the instruction and only its page generation changed,
    /// from a write to other bytes near it (a program's variables often
    /// lie right beside its code), and its bytes are still the same, it is
    /// kept as it is. Kept out of line, off the path of the hits.
    #[cold]
    #[inline(never)]
    pub fn decode(
        &mut self,
        phys_ip: usize,
        ip: u32,
        code32: bool,
        page_gen: u32,
        ram: &[u8],
        decode: impl FnOnce(&mut Instruction),
    ) -> Decoded<'_> {
        let idx = self.index(phys_ip);
        let slot = &mut self.slots[idx];
        let addr = (phys_ip as u64) << 32 | ip as u64;
        let len = slot.instr.len();
        if slot.addr == addr
            && slot.version & 1 == code32 as u64
            && !slot.instr.is_invalid()
            && ram[phys_ip..phys_ip + len] == self.code[idx][..len]
        {
            slot.version = (page_gen as u64) << 1 | slot.version & (SIMPLE | 1);
            self.hits += 1;
            return (&slot.instr, slot.handler, slot.version & SIMPLE != 0);
        }
        decode(&mut slot.instr);
        self.code[idx].copy_from_slice(&ram[phys_ip..phys_ip + 16]);
        let fast = fast::select(&slot.instr);
        slot.handler = fast.unwrap_or(execute_instruction);
        slot.addr = addr;
        slot.version = (page_gen as u64) << 1 | code32 as u64 | if fast.is_some() { SIMPLE } else { 0 };
        self.misses += 1;
        (&slot.instr, slot.handler, fast.is_some())
    }

    /// The instruction decoded from `bytes` at `ip` as 16 or 32-bit code,
    /// fetched a byte at a time from `phys_ip` on, and its handler.
    pub fn get_or_decode_bytes(
        &mut self,
        phys_ip: usize,
        ip: u32,
        code32: bool,
        bytes: &[u8; 16],
        decode: impl FnOnce() -> Instruction,
    ) -> (&Instruction, Handler) {
        let slot = &mut self.bytes_slots[phys_ip % BYTES_SLOTS];
        let key = (code32 as u64) << 32 | ip as u64;
        if slot.ip != key || slot.bytes != *bytes {
            slot.instr = decode();
            slot.handler = crate::instructions::handler(&slot.instr);
            slot.ip = key;
            slot.bytes = *bytes;
            self.misses += 1;
        } else {
            self.hits += 1;
        }
        (&slot.instr, slot.handler)
    }
}
