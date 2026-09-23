//! Decoded-instruction cache for the inner execution loop.
//!
//! Hot DOS loops re-execute the same short body many thousands of times per
//! second (palette blits, string operations, tight game main loops). Running
//! those bytes through `iced_x86::Decoder::decode_out` every single iteration
//! shows up prominently in the profile, so we cache the decoded
//! `iced_x86::Instruction` keyed by its physical address and reuse it on the
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
//! (iced stores absolute targets, not displacements). The cache key
//! therefore includes the instruction pointer, not just the physical
//! address, and the code size, which decides how the bytes decode.

use iced_x86::Instruction;

/// One cache slot, keyed by the physical address, the instruction pointer
/// (iced stores absolute branch targets, so the same bytes decoded at
/// another EIP differ) and the code size. Empty slots have an impossible
/// physical address.
#[derive(Clone, Copy)]
struct Slot {
    /// The physical address (high half) and the instruction pointer.
    addr: u64,
    /// The page generation (high half) and whether it's 32-bit code.
    version: u64,
    instr: Instruction,
}

impl Slot {
    #[inline(always)]
    fn empty() -> Self {
        Self {
            addr: u64::MAX,
            version: 0,
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
        Self {
            slots,
            mask: n - 1,
            hits: 0,
            misses: 0,
        }
    }

    #[inline(always)]
    fn index(&self, phys_ip: usize) -> usize {
        phys_ip & self.mask
    }

    /// The decoded instruction at `phys_ip`, decoded at `ip` as 16 or 32-bit
    /// code. The cached decode is used only when the slot matches all
    /// three and the recorded page generation still matches the current
    /// one; otherwise `decode` fills the slot afresh. Returning a reference
    /// into the slot saves copying the instruction on every hit.
    #[inline(always)]
    pub fn get_or_decode(
        &mut self,
        phys_ip: usize,
        ip: u32,
        code32: bool,
        page_gen: u32,
        decode: impl FnOnce(&mut Instruction),
    ) -> &Instruction {
        let idx = self.index(phys_ip);
        // SAFETY: idx is always in-bounds because we masked with `self.mask`
        // which is `len - 1` for a power-of-two-sized slots box.
        let slot = unsafe { self.slots.get_unchecked_mut(idx) };
        let addr = (phys_ip as u64) << 32 | ip as u64;
        let version = (page_gen as u64) << 1 | code32 as u64;
        if slot.addr != addr || slot.version != version {
            decode(&mut slot.instr);
            slot.addr = addr;
            slot.version = version;
            self.misses += 1;
        } else {
            self.hits += 1;
        }
        &slot.instr
    }
}
