//! Decoded-instruction cache for the inner execution loop.
//!
//! Hot DOS loops re-execute the same short body many thousands of times per
//! second (palette blits, string operations, tight game main loops). Running
//! those bytes through `iced_x86::Decoder::decode_out` every single iteration
//! shows up prominently in the profile, so we cache the decoded
//! `iced_x86::Instruction` keyed by the current (cs, ip) and reuse it on the
//! next hit.
//!
//! Correctness in the face of self-modifying code is handled via a per-4KB-page
//! generation counter on the bus (`Bus::page_gen`). Every write through the bus
//! bumps the gen for the affected page, and every cache slot records the gen
//! that was live at decode time. A slot is only returned on lookup when the
//! page gen still matches, so an LZEXE-style unpacker that rewrites its own
//! code simply causes the cache to refill transparently on the next fetch.
//!
//! Two different (cs, ip) pairs can resolve to the same physical address but
//! produce instructions with different `near_branch16` / `next_ip` fields
//! (iced stores absolute 16-bit targets, not displacements). The cache key
//! therefore includes (cs, ip) directly rather than just phys_ip.

use iced_x86::Instruction;

/// One cache slot. Empty slots carry `cs == u16::MAX` (unreachable as a real
/// segment because segments are 16-bit, but in practice DOS programs never
/// run with CS=0xFFFF so this is a safe sentinel for "nothing cached").
#[derive(Clone, Copy)]
struct Slot {
    cs: u16,
    ip: u16,
    page_gen: u32,
    instr: Instruction,
}

impl Slot {
    #[inline(always)]
    fn empty() -> Self {
        Self {
            cs: u16::MAX,
            ip: 0,
            page_gen: 0,
            instr: Instruction::default(),
        }
    }
}

/// Direct-mapped decoded-instruction cache. On collision the old entry is
/// simply overwritten — an LRU would add bookkeeping cost on the hot path and
/// empirically direct-mapped behaves well for typical DOS workloads where the
/// working set of PCs is small relative to cache capacity.
pub struct InstrCache {
    slots: Box<[Slot]>,
    mask: usize,
    pub hits: u64,
    pub misses: u64,
}

impl InstrCache {
    /// `capacity_log2` = log2 of the number of slots. 16 → 64K slots ≈ 3.5 MB.
    /// This is comfortably larger than the working set of any DOS program we
    /// care about while staying small enough to fit in L2/L3.
    pub fn new(capacity_log2: u32) -> Self {
        let n = 1usize << capacity_log2;
        let slots = vec![Slot::empty(); n].into_boxed_slice();
        Self {
            slots,
            mask: n - 1,
            hits: 0,
            misses: 0,
        }
    }

    #[inline(always)]
    fn index(&self, phys_ip: usize) -> usize {
        // phys_ip's low 20 bits are the real address; the bottom bits already
        // mix cs and ip, giving a decent distribution without a hash step.
        phys_ip & self.mask
    }

    /// Lookup: returns `Some(instr)` only when the slot matches (cs, ip) and
    /// the recorded page generation still matches the current one. Otherwise
    /// the caller should decode fresh and call `insert`.
    #[inline(always)]
    pub fn lookup(&mut self, phys_ip: usize, cs: u16, ip: u16, page_gen: u32) -> Option<Instruction> {
        let idx = self.index(phys_ip);
        // SAFETY: idx is always in-bounds because we masked with `self.mask`
        // which is `len - 1` for a power-of-two-sized slots box.
        let slot = unsafe { self.slots.get_unchecked(idx) };
        if slot.cs == cs && slot.ip == ip && slot.page_gen == page_gen {
            self.hits += 1;
            Some(slot.instr)
        } else {
            self.misses += 1;
            None
        }
    }

    #[inline(always)]
    pub fn insert(&mut self, phys_ip: usize, cs: u16, ip: u16, page_gen: u32, instr: Instruction) {
        let idx = self.index(phys_ip);
        // SAFETY: see `lookup`.
        unsafe {
            *self.slots.get_unchecked_mut(idx) = Slot {
                cs,
                ip,
                page_gen,
                instr,
            };
        }
    }
}
